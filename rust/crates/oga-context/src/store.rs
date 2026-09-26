//! Every read and write the index makes against SQLite. Writes go in batches
//! of about [`BATCH_SYMBOLS`] symbols, one transaction each, so a large build
//! never holds the broker's one writer for long.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use oga_domain::SymbolKind;
use oga_store::{Store, StoreError};
use rusqlite::{OptionalExtension, Row, Transaction, params};

use crate::text::fts_query;

/// The index layout this binary writes. An index built by an older layout is
/// rebuilt rather than read.
pub(crate) const INDEX_SCHEME: u32 = 11;

/// Symbols one write transaction carries at most, give or take one file.
const BATCH_SYMBOLS: usize = 1_000;
/// Files one transaction removes or copies.
const BATCH_FILES: usize = 100;

/// What the index knows about its own last build for one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRow {
    pub scheme: u32,
    pub state: String,
    pub symbol_count: usize,
}

/// One file as the index holds it, before its symbols are loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    pub id: i64,
    pub path: String,
    pub lang: String,
    pub digest: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub ctime_ms: i64,
    pub lines: u64,
}

/// One symbol as the index holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRow {
    pub path: String,
    pub kind: SymbolKind,
    pub name: String,
    pub qualified: String,
    pub parent: Option<String>,
    pub line: u64,
    pub end_line: u64,
    pub signature: String,
    pub doc: Option<String>,
    pub exported: bool,
    pub digest: String,
    pub name_key: String,
    pub tokens: String,
}

/// A file and its symbols, ready to write.
#[derive(Debug, Clone)]
pub struct FileUpdate {
    pub path: String,
    pub lang: String,
    pub digest: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub ctime_ms: i64,
    pub lines: u64,
    pub symbols: Vec<SymbolRow>,
}

const FILE_COLUMNS: &str = "id,path,lang,digest,size,mtime_ms,ctime_ms,lines";
const SYMBOL_COLUMNS: &str =
    "path,kind,name,qualified,parent,line,end_line,signature,doc,exported,digest";

pub fn file_rows(store: &Store, cwd: &Path) -> Result<HashMap<String, FileRow>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(&format!(
            "SELECT {FILE_COLUMNS} FROM context_files WHERE cwd=? ORDER BY path"
        ))?;
        let rows = statement
            .query_map([cwd.display().to_string()], file_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect())
    })
}

/// The indexed files a path names: the file itself, or every file whose path
/// ends with it at a directory boundary, so `query.rs` and `src/query.rs`
/// both resolve. Shortest path first — the least of it someone had to type.
pub fn files_by_path(
    store: &Store,
    cwd: &Path,
    path: &str,
    limit: usize,
) -> Result<Vec<String>, StoreError> {
    let path = path.trim_start_matches("./").trim_start_matches('/');
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let suffix = format!(
        "%/{}",
        path.replace('\\', r"\\")
            .replace('%', r"\%")
            .replace('_', r"\_")
    );
    store.with_connection(|connection| {
        let mut statement = connection.prepare(&format!(
            "SELECT path FROM context_files \
             WHERE cwd=? AND (path=? OR path LIKE ? ESCAPE '\\') \
             ORDER BY length(path) LIMIT {limit}"
        ))?;
        let rows = statement
            .query_map(params![cwd.display().to_string(), path, suffix], |row| {
                row.get(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// Symbols whose name folds to one of `names`, or holds one of them as whole
/// words. Exact names come off the name index and words off the search index,
/// so neither reads every symbol in the project.
pub fn symbols_by_name(
    store: &Store,
    cwd: &Path,
    names: &[String],
    paths: Option<&[String]>,
) -> Result<Vec<SymbolRow>, StoreError> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let words = names
        .iter()
        .flat_map(|name| name.split_whitespace())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let exact_sql = format!(
        "SELECT id,{SYMBOL_COLUMNS},name_key FROM context_symbols WHERE cwd=? AND name_key IN ({})",
        vec!["?"; names.len()].join(",")
    );
    // The search index stems words, so its hits are held to the same
    // whole-word test the name key is built for.
    let word_sql = (!words.is_empty()).then(|| {
        format!(
            " UNION SELECT s.id,{},s.name_key FROM context_symbols_fts \
             JOIN context_symbols s ON s.id=context_symbols_fts.rowid \
             WHERE context_symbols_fts MATCH ? AND ({})",
            prefixed_symbol_columns(),
            names
                .iter()
                .map(|_| "instr(' ' || s.name_key || ' ', ' ' || ? || ' ') > 0")
                .collect::<Vec<_>>()
                .join(" OR ")
        )
    });
    store.with_connection(|connection| {
        let path_sql = paths
            .filter(|paths| !paths.is_empty())
            .map(|paths| {
                format!(
                    " WHERE ({})",
                    paths
                        .iter()
                        .map(|_| "path = ? OR path LIKE ? || '/%' ESCAPE '\\'")
                        .collect::<Vec<_>>()
                        .join(" OR ")
                )
            })
            .unwrap_or_default();
        let mut statement = connection.prepare(&format!(
            "SELECT {SYMBOL_COLUMNS} FROM ({exact_sql}{}){path_sql} \
             ORDER BY length(name_key), exported DESC, length(qualified)",
            word_sql.unwrap_or_default()
        ))?;
        let path_values = paths
            .unwrap_or_default()
            .iter()
            .map(|path| path.trim_end_matches("/**").to_owned())
            .collect::<Vec<_>>();
        let word_match = project_match(cwd, "name_key", &fts_query(&words));
        let cwd = cwd.display().to_string();
        let mut arguments: Vec<&dyn rusqlite::ToSql> =
            Vec::with_capacity(names.len() * 2 + 2 + path_values.len() * 2);
        arguments.push(&cwd);
        for name in names {
            arguments.push(name);
        }
        if !words.is_empty() {
            arguments.push(&word_match);
            for name in names {
                arguments.push(name);
            }
        }
        let escaped = path_values
            .iter()
            .map(|path| path.replace('%', "\\%").replace('_', "\\_"))
            .collect::<Vec<_>>();
        for (path, value) in path_values.iter().zip(&escaped) {
            arguments.push(path);
            arguments.push(value);
        }
        let rows = statement
            .query_map(arguments.as_slice(), symbol_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// Symbols the search index matched, best rank first.
pub fn symbols_by_search(
    store: &Store,
    cwd: &Path,
    query: &str,
    limit: usize,
    paths: Option<&[String]>,
) -> Result<Vec<(SymbolRow, f64)>, StoreError> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    store.with_connection(|connection| {
        let path_sql = paths
            .filter(|paths| !paths.is_empty())
            .map(|paths| {
                format!(
                    " AND ({})",
                    paths
                        .iter()
                        .map(|_| "s.path = ? OR s.path LIKE ? || '/%' ESCAPE '\\'")
                        .collect::<Vec<_>>()
                        .join(" OR ")
                )
            })
            .unwrap_or_default();
        let mut statement = connection.prepare(&format!(
            "SELECT {}, bm25(context_symbols_fts, 0.0, 12.0, 8.0, 10.0, 3.0, 4.0, 2.0, 0.0) AS rank \
             FROM context_symbols_fts \
             JOIN context_symbols s ON s.id=context_symbols_fts.rowid \
              WHERE context_symbols_fts MATCH ? {path_sql} \
             ORDER BY rank LIMIT {limit}",
            prefixed_symbol_columns()
        ))?;
        let match_query = project_match(cwd, SYMBOL_TEXT, query);
        let mut arguments: Vec<&dyn rusqlite::ToSql> = vec![&match_query];
        let path_values = paths
            .unwrap_or_default()
            .iter()
            .map(|path| path.trim_end_matches("/**").to_owned())
            .collect::<Vec<_>>();
        let escaped = path_values
            .iter()
            .map(|path| path.replace('%', "\\%").replace('_', "\\_"))
            .collect::<Vec<_>>();
        for (path, value) in path_values.iter().zip(&escaped) {
            arguments.push(path);
            arguments.push(value);
        }
        let rows = statement
            .query_map(arguments.as_slice(), |row| {
                Ok((symbol_from_row(row)?, -row.get::<_, f64>(11)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// How many symbols each term reaches. A word that names almost everything
/// says almost nothing about which answer is right.
pub fn term_hits(
    store: &Store,
    cwd: &Path,
    terms: &[String],
) -> Result<HashMap<String, u64>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT COUNT(*) FROM context_symbols_fts \
             JOIN context_symbols s ON s.id=context_symbols_fts.rowid \
              WHERE context_symbols_fts MATCH ?",
        )?;
        let mut hits = HashMap::with_capacity(terms.len());
        for term in terms {
            let count: i64 = statement.query_row(
                [project_match(
                    cwd,
                    SYMBOL_TEXT,
                    &format!("\"{}\"", term.replace('"', "\"\"")),
                )],
                |row| row.get(0),
            )?;
            hits.insert(term.clone(), count.max(0) as u64);
        }
        Ok(hits)
    })
}

/// The one symbol in the project whose body still hashes to `digest`, if
/// exactly one does. Two matches mean the body is boilerplate, not identity.
pub fn symbol_by_digest(
    store: &Store,
    cwd: &Path,
    digest: &str,
) -> Result<Option<SymbolRow>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(&format!(
            "SELECT {SYMBOL_COLUMNS} FROM context_symbols WHERE cwd=? AND digest=? LIMIT 2"
        ))?;
        let rows = statement
            .query_map(params![cwd.display().to_string(), digest], symbol_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok((rows.len() == 1).then(|| rows.into_iter().next().expect("one match")))
    })
}

/// The symbol a written-out `path#name` means: the plain name, or the
/// qualified one, so `TermWeights::of` and `workspace.package.version` land as
/// readily as `of` and `version`. A plain-name match wins over a qualified one.
pub fn symbol_named(
    store: &Store,
    cwd: &Path,
    path: &str,
    name: &str,
) -> Result<Option<SymbolRow>, StoreError> {
    store.with_connection(|connection| {
        Ok(connection
            .query_row(
                &format!(
                    "SELECT {SYMBOL_COLUMNS} FROM context_symbols \
                     WHERE cwd=?1 AND path=?2 AND (name=?3 OR qualified=?3) \
                     ORDER BY name<>?3, line LIMIT 1"
                ),
                params![cwd.display().to_string(), path, name],
                symbol_from_row,
            )
            .optional()?)
    })
}

pub fn symbol_at(
    store: &Store,
    cwd: &Path,
    path: &str,
    name: &str,
) -> Result<Option<SymbolRow>, StoreError> {
    store.with_connection(|connection| {
        Ok(connection
            .query_row(
                &format!(
                    "SELECT {SYMBOL_COLUMNS} FROM context_symbols WHERE cwd=? AND path=? AND name=? ORDER BY line LIMIT 1"
                ),
                params![cwd.display().to_string(), path, name],
                symbol_from_row,
            )
            .optional()?)
    })
}

/// The whole-file hash the index holds for one path, if it holds the file.
pub fn file_digest(store: &Store, cwd: &Path, path: &str) -> Result<Option<String>, StoreError> {
    store.with_connection(|connection| {
        Ok(connection
            .query_row(
                "SELECT digest FROM context_files WHERE cwd=? AND path=? LIMIT 1",
                params![cwd.display().to_string(), path],
                |row| row.get(0),
            )
            .optional()?)
    })
}

pub fn file_exists(store: &Store, cwd: &Path, path: &str) -> Result<bool, StoreError> {
    store.with_connection(|connection| {
        Ok(connection
            .query_row(
                "SELECT 1 FROM context_files WHERE cwd=? AND path=? LIMIT 1",
                params![cwd.display().to_string(), path],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    })
}

/// Write the whole project, dropping whatever the index held for it before.
/// The project reads as `building` until [`save_index`] marks it done.
pub fn replace_files(
    store: &Store,
    cwd: &Path,
    updates: &[FileUpdate],
    now: &str,
) -> Result<(), StoreError> {
    mark_building(store, cwd, now)?;
    clear(store, cwd)?;
    merge_files(store, cwd, updates, &[], now)
}

/// Write the files that changed and drop the ones that went, leaving the rest
/// of the index untouched.
pub fn merge_files(
    store: &Store,
    cwd: &Path,
    updates: &[FileUpdate],
    removed: &[String],
    now: &str,
) -> Result<(), StoreError> {
    let cwd = cwd.display().to_string();
    for chunk in removed.chunks(BATCH_FILES) {
        store.transaction(|transaction| {
            for path in chunk {
                delete_file(transaction, &cwd, path)?;
            }
            Ok(())
        })?;
    }
    let mut start = 0;
    while start < updates.len() {
        let mut end = start;
        let mut symbols = 0;
        while end < updates.len() && (end == start || symbols < BATCH_SYMBOLS) {
            symbols += updates[end].symbols.len();
            end += 1;
        }
        write(store, &cwd, &updates[start..end], now)?;
        start = end;
    }
    Ok(())
}

/// Drop every row the index holds for `cwd`.
pub fn clear(store: &Store, cwd: &Path) -> Result<(), StoreError> {
    let cwd = cwd.display().to_string();
    loop {
        let deleted = store.transaction(|transaction| {
            Ok(transaction.execute(
                "DELETE FROM context_symbols WHERE id IN \
                 (SELECT id FROM context_symbols WHERE cwd=? LIMIT ?)",
                params![cwd, BATCH_SYMBOLS],
            )?)
        })?;
        if deleted < BATCH_SYMBOLS {
            break;
        }
    }
    store.transaction(|transaction| {
        transaction.execute("DELETE FROM context_files WHERE cwd=?", [&cwd])?;
        Ok(())
    })
}

/// Drop `cwd`'s index: its rows and the record that it was built.
pub fn forget(store: &Store, cwd: &Path) -> Result<(), StoreError> {
    clear(store, cwd)?;
    store.transaction(|transaction| {
        transaction.execute(
            "DELETE FROM context_index WHERE cwd=?",
            [cwd.display().to_string()],
        )?;
        Ok(())
    })
}

/// Every folder the index holds rows for, with the layout its build was
/// written to. Rows left without a build record carry no layout.
pub fn indexed_folders(store: &Store) -> Result<Vec<(String, Option<u32>)>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT cwd, scheme FROM context_index \
             UNION SELECT DISTINCT cwd, NULL FROM context_files \
             WHERE cwd NOT IN (SELECT cwd FROM context_index)",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?
                        .map(|scheme| scheme.max(0) as u32),
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?)
    })
}

/// Start `cwd`'s index as a copy of `origin`'s, a batch of files at a time.
/// Stamps are copied too, so the next reconcile re-reads every file but
/// parses only those whose contents differ.
pub fn seed(store: &Store, cwd: &Path, origin: &Path, now: &str) -> Result<(), StoreError> {
    let cwd = cwd.display().to_string();
    let origin = origin.display().to_string();
    let paths = store.with_connection(|connection| {
        let mut statement =
            connection.prepare("SELECT path FROM context_files WHERE cwd=? ORDER BY path")?;
        Ok(statement
            .query_map([&origin], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?)
    })?;
    for chunk in paths.chunks(BATCH_FILES) {
        let (first, last) = (&chunk[0], &chunk[chunk.len() - 1]);
        store.transaction(|transaction| {
            transaction.execute(
                "INSERT INTO context_files(cwd,path,lang,digest,size,mtime_ms,ctime_ms,lines,updated_at) \
                 SELECT ?1,path,lang,digest,size,mtime_ms,ctime_ms,lines,?3 FROM context_files \
                 WHERE cwd=?2 AND path BETWEEN ?4 AND ?5",
                params![cwd, origin, now, first, last],
            )?;
            transaction.execute(
                "INSERT INTO context_symbols(file_id,cwd,path,kind,name,name_key,qualified,parent,\
                 line,end_line,signature,doc,exported,digest,tokens) \
                 SELECT f.id,?1,s.path,s.kind,s.name,s.name_key,s.qualified,s.parent,\
                 s.line,s.end_line,s.signature,s.doc,s.exported,s.digest,s.tokens \
                 FROM context_symbols s JOIN context_files f ON f.cwd=?1 AND f.path=s.path \
                 WHERE s.cwd=?2 AND s.path BETWEEN ?3 AND ?4 ORDER BY s.id",
                params![cwd, origin, first, last],
            )?;
            Ok(())
        })?;
    }
    Ok(())
}

/// One batch of changed files, each replacing whatever the index held for it.
fn write(store: &Store, cwd: &str, updates: &[FileUpdate], now: &str) -> Result<(), StoreError> {
    store.transaction(|transaction| {
        let mut upsert_file = transaction.prepare(
            "INSERT INTO context_files(cwd,path,lang,digest,size,mtime_ms,ctime_ms,lines,updated_at) \
             VALUES(?,?,?,?,?,?,?,?,?) \
             ON CONFLICT(cwd,path) DO UPDATE SET \
             lang=excluded.lang,digest=excluded.digest,size=excluded.size,\
             mtime_ms=excluded.mtime_ms,ctime_ms=excluded.ctime_ms,lines=excluded.lines,\
             updated_at=excluded.updated_at \
             RETURNING id",
        )?;
        let mut clear_symbols =
            transaction.prepare("DELETE FROM context_symbols WHERE file_id=?")?;
        let mut insert_symbol = transaction.prepare(
            "INSERT INTO context_symbols(file_id,cwd,path,kind,name,name_key,qualified,parent,\
             line,end_line,signature,doc,exported,digest,tokens) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )?;
        for update in updates {
            let file_id: i64 = upsert_file.query_row(
                params![
                    cwd,
                    update.path,
                    update.lang,
                    update.digest,
                    update.size,
                    update.mtime_ms,
                    update.ctime_ms,
                    update.lines,
                    now
                ],
                |row| row.get(0),
            )?;
            clear_symbols.execute([file_id])?;
            for symbol in &update.symbols {
                insert_symbol.execute(params![
                    file_id,
                    cwd,
                    update.path,
                    symbol.kind.as_str(),
                    symbol.name,
                    symbol.name_key,
                    symbol.qualified,
                    symbol.parent.as_deref().unwrap_or_default(),
                    symbol.line,
                    symbol.end_line,
                    symbol.signature,
                    symbol.doc.as_deref().unwrap_or_default(),
                    i64::from(symbol.exported),
                    symbol.digest,
                    symbol.tokens,
                ])?;
            }
        }
        Ok(())
    })
}

/// A file rewritten with the same contents, carrying the stamps it now wears.
pub struct TouchedFile {
    pub path: String,
    pub mtime_ms: i64,
    pub ctime_ms: i64,
}

/// Record that a file was rewritten with the same contents, so the next run
/// takes the cheap path again instead of re-reading it.
pub fn touch_files(
    store: &Store,
    cwd: &Path,
    touched: &[TouchedFile],
    now: &str,
) -> Result<(), StoreError> {
    if touched.is_empty() {
        return Ok(());
    }
    let cwd = cwd.display().to_string();
    store.transaction(|transaction| {
        let mut statement = transaction.prepare(
            "UPDATE context_files SET mtime_ms=?,ctime_ms=?,updated_at=? WHERE cwd=? AND path=?",
        )?;
        for touched in touched {
            statement.execute(params![
                touched.mtime_ms,
                touched.ctime_ms,
                now,
                cwd,
                touched.path
            ])?;
        }
        Ok(())
    })
}

/// Carry a file's rows to its new path without re-reading it.
pub fn move_file(store: &Store, cwd: &Path, from: &str, to: &str) -> Result<(), StoreError> {
    let cwd = cwd.display().to_string();
    store.transaction(|transaction| {
        delete_file(transaction, &cwd, to)?;
        transaction.execute(
            "UPDATE context_files SET path=? WHERE cwd=? AND path=?",
            params![to, cwd, from],
        )?;
        transaction.execute(
            "UPDATE context_symbols SET path=? WHERE cwd=? AND path=?",
            params![to, cwd, from],
        )?;
        Ok(())
    })
}

fn delete_file(transaction: &Transaction<'_>, cwd: &str, path: &str) -> rusqlite::Result<()> {
    transaction.execute(
        "DELETE FROM context_symbols WHERE cwd=? AND path=?",
        params![cwd, path],
    )?;
    transaction.execute(
        "DELETE FROM context_files WHERE cwd=? AND path=?",
        params![cwd, path],
    )?;
    Ok(())
}

pub fn counts(store: &Store, cwd: &Path) -> Result<(usize, usize), StoreError> {
    store.with_connection(|connection| {
        let cwd = cwd.display().to_string();
        let files: i64 = connection.query_row(
            "SELECT COUNT(*) FROM context_files WHERE cwd=?",
            [&cwd],
            |row| row.get(0),
        )?;
        let symbols: i64 = connection.query_row(
            "SELECT COUNT(*) FROM context_symbols WHERE cwd=?",
            [&cwd],
            |row| row.get(0),
        )?;
        Ok((files.max(0) as usize, symbols.max(0) as usize))
    })
}

pub fn index_row(store: &Store, cwd: &Path) -> Result<Option<IndexRow>, StoreError> {
    store.with_connection(|connection| {
        Ok(connection
            .query_row(
                "SELECT scheme, state, symbol_count FROM context_index WHERE cwd=?",
                [cwd.display().to_string()],
                |row| {
                    Ok(IndexRow {
                        scheme: row.get::<_, i64>(0)?.max(0) as u32,
                        state: row.get(1)?,
                        symbol_count: row.get::<_, i64>(2)?.max(0) as usize,
                    })
                },
            )
            .optional()?)
    })
}

fn prefixed_symbol_columns() -> String {
    SYMBOL_COLUMNS
        .split(',')
        .map(|column| format!("s.{column}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Search-table columns a question's words are matched against: the
/// symbol's own text, never the project token.
const SYMBOL_TEXT: &str = "name qualified tokens signature doc path";

/// `query` over `columns`, within the project at `cwd` alone.
fn project_match(cwd: &Path, columns: &str, query: &str) -> String {
    let token = cwd
        .display()
        .to_string()
        .bytes()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    format!("{{project}} : \"{token}\" AND {{{columns}}} : ({query})")
}

pub fn save_index(
    store: &Store,
    cwd: &Path,
    partial: bool,
    file_count: usize,
    symbol_count: usize,
    now: &str,
) -> Result<(), StoreError> {
    let state = if partial { "partial" } else { "ready" };
    upsert_index(store, cwd, state, file_count, symbol_count, now)
}

/// Mark `cwd`'s index as being written, so an interrupted build is rebuilt
/// rather than read.
pub fn mark_building(store: &Store, cwd: &Path, now: &str) -> Result<(), StoreError> {
    upsert_index(store, cwd, "building", 0, 0, now)
}

fn upsert_index(
    store: &Store,
    cwd: &Path,
    state: &str,
    file_count: usize,
    symbol_count: usize,
    now: &str,
) -> Result<(), StoreError> {
    store.transaction(|transaction| {
        transaction.execute(
            "INSERT INTO context_index(cwd,scheme,state,built_at,file_count,symbol_count,updated_at) \
             VALUES(?,?,?,?,?,?,?) \
             ON CONFLICT(cwd) DO UPDATE SET scheme=excluded.scheme,state=excluded.state,\
             built_at=excluded.built_at,file_count=excluded.file_count,\
             symbol_count=excluded.symbol_count,updated_at=excluded.updated_at",
            params![
                cwd.display().to_string(),
                INDEX_SCHEME,
                state,
                now,
                file_count,
                symbol_count,
                now
            ],
        )?;
        Ok(())
    })
}

fn file_from_row(row: &Row<'_>) -> rusqlite::Result<FileRow> {
    Ok(FileRow {
        id: row.get(0)?,
        path: row.get(1)?,
        lang: row.get(2)?,
        digest: row.get(3)?,
        size: row.get::<_, i64>(4)?.max(0) as u64,
        mtime_ms: row.get(5)?,
        ctime_ms: row.get(6)?,
        lines: row.get::<_, i64>(7)?.max(0) as u64,
    })
}

fn symbol_from_row(row: &Row<'_>) -> rusqlite::Result<SymbolRow> {
    Ok(SymbolRow {
        path: row.get(0)?,
        kind: SymbolKind::parse(&row.get::<_, String>(1)?).unwrap_or(SymbolKind::Fn),
        name: row.get(2)?,
        qualified: row.get(3)?,
        parent: non_empty(row.get(4)?),
        line: row.get::<_, i64>(5)?.max(0) as u64,
        end_line: row.get::<_, i64>(6)?.max(0) as u64,
        signature: row.get(7)?,
        doc: non_empty(row.get(8)?),
        exported: row.get::<_, i64>(9)? != 0,
        digest: row.get(10)?,
        name_key: String::new(),
        tokens: String::new(),
    })
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}
