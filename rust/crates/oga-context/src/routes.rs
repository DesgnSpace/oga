//! Learned routes: the hint phrases workers and people attach to a place in
//! the code. They outrank everything the parser found, and they follow their
//! target through a rename or a move.

use std::path::Path;

use oga_store::{Store, StoreError};
use rusqlite::{OptionalExtension, params};

use crate::store::{self};
use crate::text::{fts_query, hint_key, hint_words, normalize_word};

/// The most aliases one route keeps. A route that answers to everything
/// answers nothing. Teaching past the cap trims the words taught longest ago.
const MAX_ROUTE_ALIASES: usize = 8;
/// The most taught phrases one route answers to word for word, oldest first
/// to go.
const MAX_ROUTE_PHRASES: usize = 6;
pub const MAX_HINTS_CHARS: usize = 160;

const SYNONYM_GROUPS: &[&[&str]] = &[
    &[
        "auth",
        "authentication",
        "login",
        "signin",
        "sign",
        "session",
    ],
    &["config", "configuration", "setting", "settings"],
    &["db", "database", "store", "storage"],
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnedRoute {
    pub id: i64,
    pub path: String,
    pub symbol: Option<String>,
    pub aliases: String,
    /// True when the question is one of the phrases this route was taught,
    /// whatever order its words came in.
    pub exact: bool,
}

/// One route's target before and after a reconcile moved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteMove {
    pub from_path: String,
    pub from_symbol: Option<String>,
    pub to_path: String,
    pub to_symbol: Option<String>,
}

pub struct RouteRecord<'a> {
    pub aliases: &'a str,
    /// The taught phrases, each folded to one order-free key, joined by `|`.
    pub phrases: &'a str,
    pub path: &'a str,
    pub symbol: &'a str,
    pub source_digest: &'a str,
    pub task_id: &'a str,
    pub attempt: i64,
    pub profile_id: &'a str,
    pub model: &'a str,
}

/// Fold hints into the bounded alias set a route is stored under, expanding
/// the few word families people reach for interchangeably.
pub fn aliases(hints: &[String]) -> String {
    let mut aliases = Vec::new();
    for hint in hints {
        for part in hint.split('|') {
            for word in hint_words(part) {
                push(&mut aliases, word);
            }
        }
    }
    for term in aliases.clone() {
        for group in SYNONYM_GROUPS {
            if group
                .iter()
                .any(|candidate| normalize_word(candidate) == term)
            {
                for synonym in *group {
                    for word in hint_words(synonym) {
                        push(&mut aliases, word);
                    }
                }
            }
        }
    }
    aliases.join(" ")
}

/// The phrases a route answers to word for word, each folded to one key so
/// the order someone taught them in never decides whether they match. `|`
/// separates alternatives; everything else is one phrase.
pub fn phrases(hints: &[String]) -> String {
    let mut keys = Vec::new();
    for part in hints.join(" ").split('|') {
        let key = hint_key(&[part.to_owned()]);
        if !key.is_empty() && !keys.contains(&key) && keys.len() < MAX_ROUTE_PHRASES {
            keys.push(key);
        }
    }
    keys.join("|")
}

fn push(aliases: &mut Vec<String>, alias: String) {
    if aliases.len() < MAX_ROUTE_ALIASES && !aliases.contains(&alias) {
        aliases.push(alias);
    }
}

/// The bounded alias set, newest words first. A full set that refused the
/// words just taught would leave the route unable to answer what someone had
/// only now told it, so the cap trims the oldest instead.
fn merge(kept: &str, taught: &str) -> String {
    let mut aliases = Vec::new();
    for alias in taught.split_whitespace().chain(kept.split_whitespace()) {
        push(&mut aliases, alias.to_owned());
    }
    aliases.join(" ")
}

/// The phrases this route answers to word for word, newest first, for the
/// same reason.
fn merge_phrases(kept: &str, taught: &str) -> String {
    let mut keys = Vec::new();
    for key in taught.split('|').chain(kept.split('|')) {
        if !key.is_empty() && !keys.contains(&key) && keys.len() < MAX_ROUTE_PHRASES {
            keys.push(key);
        }
    }
    keys.join("|")
}

pub fn save(
    store: &Store,
    cwd: &Path,
    records: &[RouteRecord<'_>],
    now: &str,
) -> Result<(), StoreError> {
    let cwd = cwd.display().to_string();
    store.transaction(|transaction| {
        for record in records {
            let existing = transaction
                .query_row(
                    "SELECT id,aliases,hint_keys FROM context_learned_routes \
                     WHERE cwd=? AND learned_path=? AND learned_symbol=? LIMIT 1",
                    params![cwd, record.path, record.symbol],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            match existing {
                Some((id, existing_aliases, existing_phrases)) => {
                    transaction.execute(
                        "UPDATE context_learned_routes SET aliases=?,hint_keys=?,source_digest=?,\
                         task_id=?,attempt=?,profile_id=?,model=?,last_confirmed_at=? WHERE id=?",
                        params![
                            merge(&existing_aliases, record.aliases),
                            merge_phrases(&existing_phrases, record.phrases),
                            record.source_digest,
                            record.task_id,
                            record.attempt,
                            record.profile_id,
                            record.model,
                            now,
                            id
                        ],
                    )?;
                }
                None => {
                    transaction.execute(
                        "INSERT INTO context_learned_routes(cwd,aliases,hint_keys,learned_path,\
                         learned_symbol,source_digest,task_id,attempt,profile_id,model,created_at,\
                         last_confirmed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                        params![
                            cwd,
                            record.aliases,
                            record.phrases,
                            record.path,
                            record.symbol,
                            record.source_digest,
                            record.task_id,
                            record.attempt,
                            record.profile_id,
                            record.model,
                            now,
                            now
                        ],
                    )?;
                }
            }
        }
        Ok(())
    })
}

/// Routes whose aliases answer this question, exact matches first.
pub fn matching(
    store: &Store,
    cwd: &Path,
    question: &str,
    terms: &[String],
) -> Result<Vec<LearnedRoute>, StoreError> {
    let exact_key = hint_key(&[question.to_owned()]);
    if exact_key.is_empty() {
        return Ok(Vec::new());
    }
    let cwd = cwd.display().to_string();
    store.with_connection(|connection| {
        let mut routes = Vec::new();
        let mut statement = connection.prepare(
            "SELECT id,learned_path,learned_symbol,aliases FROM context_learned_routes \
             WHERE cwd=? AND instr('|'||hint_keys||'|', ?)>0 \
             ORDER BY last_confirmed_at DESC LIMIT 8",
        )?;
        for row in statement.query_map(params![cwd, format!("|{exact_key}|")], |row| {
            route_from_row(row, true)
        })? {
            routes.push(row?);
        }
        let query = fts_query(terms);
        if !query.is_empty() {
            let mut statement = connection.prepare(
                "SELECT r.id,r.learned_path,r.learned_symbol,r.aliases \
                 FROM context_learned_routes r \
                 JOIN context_learned_routes_fts f ON f.rowid=r.id \
                 WHERE r.cwd=? AND context_learned_routes_fts MATCH ? \
                 ORDER BY r.last_confirmed_at DESC LIMIT 24",
            )?;
            for row in statement.query_map(params![cwd, query], |row| route_from_row(row, false))? {
                let route = row?;
                if !routes.iter().any(|kept: &LearnedRoute| {
                    kept.path == route.path && kept.symbol == route.symbol
                }) {
                    routes.push(route);
                }
            }
        }
        Ok(routes)
    })
}

pub fn forget(store: &Store, id: i64) -> Result<(), StoreError> {
    store.transaction(|transaction| {
        transaction.execute("DELETE FROM context_learned_routes WHERE id=?", [id])?;
        Ok(())
    })
}

/// Point every route at where its target lives now. A route whose file and
/// symbol both survived is confirmed; one whose symbol body turns up
/// elsewhere follows it; one with neither is dropped.
pub fn heal(
    store: &Store,
    cwd: &Path,
    moves: &[(String, String)],
) -> Result<(usize, usize, Vec<RouteMove>), StoreError> {
    let rows = store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT id,learned_path,learned_symbol,source_digest FROM context_learned_routes WHERE cwd=?",
        )?;
        Ok(statement
            .query_map([cwd.display().to_string()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?)
    })?;
    let mut confirmed = 0;
    let mut dropped = 0;
    let mut route_moves = Vec::new();
    for (id, from_path, from_symbol, source_digest) in rows {
        let path = moves
            .iter()
            .find(|(from, _)| from == &from_path)
            .map_or(from_path.clone(), |(_, to)| to.clone());
        let resolved = if from_symbol.is_empty() {
            store::file_exists(store, cwd, &path)?
                .then(|| (path.clone(), String::new(), String::new()))
        } else {
            match store::symbol_at(store, cwd, &path, &from_symbol)? {
                Some(symbol) => Some((path.clone(), symbol.name, symbol.digest)),
                None => store::symbol_by_digest(store, cwd, &source_digest)?
                    .map(|symbol| (symbol.path, symbol.name, symbol.digest)),
            }
        };
        let Some((path, symbol, digest)) = resolved else {
            forget(store, id)?;
            dropped += 1;
            continue;
        };
        store.transaction(|transaction| {
            transaction.execute(
                "UPDATE context_learned_routes SET learned_path=?,learned_symbol=?,\
                 source_digest=? WHERE id=?",
                params![path, symbol, digest, id],
            )?;
            Ok(())
        })?;
        confirmed += 1;
        if path != from_path || symbol != from_symbol {
            route_moves.push(RouteMove {
                from_path,
                from_symbol: (!from_symbol.is_empty()).then_some(from_symbol),
                to_path: path,
                to_symbol: (!symbol.is_empty()).then_some(symbol),
            });
        }
    }
    Ok((confirmed, dropped, route_moves))
}

/// The terms of `question` this route claims to answer.
pub fn overlap(route: &LearnedRoute, terms: &[String]) -> usize {
    terms
        .iter()
        .filter(|term| {
            route
                .aliases
                .split_whitespace()
                .any(|alias| alias == term.as_str())
        })
        .count()
}

fn route_from_row(row: &rusqlite::Row<'_>, exact: bool) -> rusqlite::Result<LearnedRoute> {
    let symbol: String = row.get(2)?;
    Ok(LearnedRoute {
        id: row.get(0)?,
        path: row.get(1)?,
        symbol: (!symbol.is_empty()).then_some(symbol),
        aliases: row.get(3)?,
        exact,
    })
}
