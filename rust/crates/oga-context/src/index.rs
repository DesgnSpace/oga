//! Building the index, keeping it in step with disk, and answering from it.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use oga_domain::Task;
use oga_store::{Store, StoreError};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::query::{self, CANDIDATE_POOL, DEFAULT_LIMIT, Ranking, Scored, TermWeights};
use crate::routes::{self, MAX_HINTS_CHARS, RouteMove, RouteRecord};
use crate::store::{self as index_store, FileUpdate, INDEX_SCHEME, SymbolRow};
use crate::symbols::extract_symbols;
use crate::text::{fts_query, name_key, prompt_terms};
use crate::walk::{self, WalkFile};

/// The most files one project contributes. Past this the walk reports itself
/// partial rather than silently indexing half a tree.
const MAX_BUILD_FILES: usize = 20_000;
/// Files past this size are generated, minified, or data. Either way their
/// symbols are not what anyone is looking for.
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_SYMBOLS_PER_CWD: usize = 200_000;
const MAX_LEARNED_ROUTES: usize = 12;
const MAX_FILE_BODY_LINES: usize = 120;
const DIGEST_CHARS: usize = 16;

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("invalid context request: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    pub max_files: usize,
    pub max_symbols: usize,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            max_files: MAX_BUILD_FILES,
            max_symbols: MAX_SYMBOLS_PER_CWD,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildResult {
    pub partial: bool,
    pub file_count: usize,
    pub symbol_count: usize,
    pub routes_confirmed: usize,
    pub routes_dropped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileResult {
    pub partial: bool,
    pub changed: bool,
    pub file_count: usize,
    pub symbol_count: usize,
    pub refreshed: usize,
    pub moved: usize,
    pub removed: usize,
    pub routes_confirmed: usize,
    pub routes_dropped: usize,
    pub route_moves: Vec<RouteMove>,
}

/// Where a lookup runs and what it may read. A task working in its own
/// checkout answers from the origin's index but reads the checkout's files.
#[derive(Debug, Clone, Default)]
pub struct ContextTarget {
    pub cwd: PathBuf,
    pub source_cwd: Option<PathBuf>,
    pub scope: oga_domain::TaskScope,
}

impl ContextTarget {
    pub fn new(cwd: impl Into<PathBuf>, scope: oga_domain::TaskScope) -> Self {
        Self {
            cwd: cwd.into(),
            source_cwd: None,
            scope,
        }
    }

    pub fn worktree(
        cwd: impl Into<PathBuf>,
        origin: impl Into<PathBuf>,
        scope: oga_domain::TaskScope,
    ) -> Self {
        Self {
            cwd: cwd.into(),
            source_cwd: Some(origin.into()),
            scope,
        }
    }

    fn index_cwd(&self) -> &Path {
        self.source_cwd.as_deref().unwrap_or(&self.cwd)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionCandidate {
    pub path: String,
    pub line: u64,
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct QuestionOptions {
    pub limit: Option<usize>,
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextResult {
    pub markdown: String,
    pub candidates: Vec<QuestionCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnRouteProposal {
    pub hints: Vec<String>,
    pub path: String,
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnRouteRejection {
    pub index: usize,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnRoutesResult {
    pub accepted: usize,
    pub rejected: Vec<LearnRouteRejection>,
}

#[derive(Clone)]
pub struct ContextIndex<'a> {
    store: &'a Store,
}

impl<'a> ContextIndex<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Read every candidate file and write the whole index from scratch.
    pub fn build(
        &self,
        cwd: impl AsRef<Path>,
        options: BuildOptions,
    ) -> Result<BuildResult, ContextError> {
        let cwd = cwd.as_ref();
        let walk = walk::walk_files(cwd, options.max_files);
        let mut updates = parse_all(cwd, &walk.files);
        let partial = walk.partial || truncate_to_budget(&mut updates, options.max_symbols);
        let now = timestamp_now();
        index_store::replace_files(self.store, cwd, &updates, &now)?;
        let (file_count, symbol_count) = index_store::counts(self.store, cwd)?;
        index_store::save_index(self.store, cwd, partial, file_count, symbol_count, &now)?;
        let (routes_confirmed, routes_dropped, _) = routes::heal(self.store, cwd, &[])?;
        Ok(BuildResult {
            partial,
            file_count,
            symbol_count,
            routes_confirmed,
            routes_dropped,
        })
    }

    /// Build the index the first time, and rebuild it whenever this binary
    /// writes a layout the stored one predates.
    pub fn ensure(&self, cwd: impl AsRef<Path>) -> Result<(), ContextError> {
        let cwd = cwd.as_ref();
        if index_store::index_row(self.store, cwd)?.is_none_or(|row| row.scheme != INDEX_SCHEME) {
            self.build(cwd, BuildOptions::default())?;
        }
        Ok(())
    }

    /// Re-read only what changed on disk, and follow every saved route to
    /// wherever its target moved.
    pub fn reconcile(
        &self,
        cwd: impl AsRef<Path>,
        options: BuildOptions,
    ) -> Result<ReconcileResult, ContextError> {
        let cwd = cwd.as_ref();
        let known = index_store::file_rows(self.store, cwd)?;
        if known.is_empty() || index_store::index_row(self.store, cwd)?.is_none() {
            let built = self.build(cwd, options)?;
            return Ok(ReconcileResult {
                partial: built.partial,
                changed: built.file_count > 0,
                file_count: built.file_count,
                symbol_count: built.symbol_count,
                refreshed: built.file_count,
                moved: 0,
                removed: 0,
                routes_confirmed: built.routes_confirmed,
                routes_dropped: built.routes_dropped,
                route_moves: Vec::new(),
            });
        }
        let walk = walk::walk_files(cwd, options.max_files);
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        for file in &walk.files {
            seen.insert(file.path.clone());
            let unchanged = known
                .get(&file.path)
                .is_some_and(|known| known.size == file.size && known.mtime_ms == file.mtime_ms);
            if !unchanged {
                candidates.push(file.clone());
            }
        }
        let mut updates = parse_all(cwd, &candidates);
        let mut touched = Vec::new();
        updates.retain(|update| {
            let same = known
                .get(&update.path)
                .is_some_and(|known| known.digest == update.digest);
            if same {
                touched.push((update.path.clone(), update.mtime_ms));
            }
            !same
        });
        let vanished = known
            .keys()
            .filter(|path| !seen.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        let moved = detect_moves(&known, &updates, &vanished);
        let moved_from = moved.iter().map(|(from, _)| from).collect::<HashSet<_>>();
        let removed = vanished
            .iter()
            .filter(|path| !moved_from.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        let changed = !updates.is_empty() || !removed.is_empty() || !moved.is_empty();
        let now = timestamp_now();
        for (from, to) in &moved {
            index_store::move_file(self.store, cwd, from, to)?;
        }
        index_store::merge_files(self.store, cwd, &updates, &removed, &now)?;
        index_store::touch_files(self.store, cwd, &touched, &now)?;
        let (confirmed, dropped, route_moves) = routes::heal(self.store, cwd, &moved)?;
        let (file_count, symbol_count) = index_store::counts(self.store, cwd)?;
        if changed {
            index_store::save_index(
                self.store,
                cwd,
                walk.partial,
                file_count,
                symbol_count,
                &now,
            )?;
        }
        Ok(ReconcileResult {
            partial: walk.partial,
            changed,
            file_count,
            symbol_count,
            refreshed: updates.len(),
            moved: moved.len(),
            removed: removed.len(),
            routes_confirmed: confirmed,
            routes_dropped: dropped,
            route_moves,
        })
    }

    pub fn question(
        &self,
        target: &ContextTarget,
        question: &str,
    ) -> Result<ContextResult, ContextError> {
        self.question_with_options(target, question, QuestionOptions::default())
    }

    pub fn question_with_options(
        &self,
        target: &ContextTarget,
        question: &str,
        options: QuestionOptions,
    ) -> Result<ContextResult, ContextError> {
        self.ensure(target.index_cwd())?;
        let index_cwd = target.index_cwd();
        let terms = prompt_terms(question);
        if terms.is_empty() {
            return Ok(miss(question, &[]));
        }
        let ranked = self.rank(index_cwd, question, &terms)?;
        let confident = query::is_confident(&ranked, &terms);
        let reachable = self.reachable(target, &ranked, options.limit)?;
        if reachable.kept.is_empty() {
            let absent = terms
                .iter()
                .filter(|term| !ranked.iter().any(|hit| hit.matched.contains(term)))
                .cloned()
                .collect::<Vec<_>>();
            return Ok(miss(question, &absent));
        }
        let candidates = reachable
            .kept
            .iter()
            .map(|candidate| QuestionCandidate {
                path: candidate.symbol.path.clone(),
                line: candidate.symbol.line.max(1),
                symbol: (!candidate.symbol.name.is_empty()).then(|| candidate.symbol.name.clone()),
                code: options.code.then(|| source_body(target, &candidate.symbol)),
            })
            .collect::<Vec<_>>();
        let mut lines = answer_lines(
            &candidates,
            &reachable.kept,
            &terms,
            confident,
            options.code,
        );
        if reachable.outside_scope > 0 {
            lines.push(omitted(
                reachable.outside_scope,
                "outside this task's read scope",
            ));
        }
        if reachable.gone > 0 {
            lines.push(omitted(reachable.gone, "no longer on disk"));
        }
        Ok(ContextResult {
            markdown: lines.join("\n"),
            candidates,
        })
    }

    /// The best candidates this task may read that are still on disk, with a
    /// count of what each rule left out.
    fn reachable(
        &self,
        target: &ContextTarget,
        ranked: &[Scored],
        limit: Option<usize>,
    ) -> Result<Reachable, ContextError> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT).max(1);
        let mut reachable = Reachable::default();
        for candidate in ranked.iter().take(CANDIDATE_POOL) {
            if !scope_covers_path(&target.scope.read, &target.cwd, &candidate.symbol.path) {
                reachable.outside_scope += 1;
                continue;
            }
            let Some(symbol) = self.locate(target, &candidate.symbol)? else {
                reachable.gone += 1;
                continue;
            };
            reachable.kept.push(Scored {
                symbol,
                ..candidate.clone()
            });
            if reachable.kept.len() == limit {
                break;
            }
        }
        Ok(reachable)
    }

    /// Save the routes a worker learned while running a task.
    pub fn learn_routes(
        &self,
        task: &Task,
        proposals: &[LearnRouteProposal],
    ) -> Result<LearnRoutesResult, ContextError> {
        if proposals.len() > MAX_LEARNED_ROUTES {
            return Ok(LearnRoutesResult {
                accepted: 0,
                rejected: vec![LearnRouteRejection {
                    index: MAX_LEARNED_ROUTES,
                    reason: format!("at most {MAX_LEARNED_ROUTES} routes are allowed"),
                }],
            });
        }
        let read_cwd = PathBuf::from(&task.cwd);
        let index_cwd = task.worktree.as_ref().map_or_else(
            || read_cwd.clone(),
            |worktree| PathBuf::from(&worktree.origin_cwd),
        );
        let scope = task
            .scope
            .read
            .iter()
            .chain(task.scope.write.iter())
            .cloned()
            .collect::<Vec<_>>();
        let mut prepared = Vec::new();
        let mut rejected = Vec::new();
        for (index, proposal) in proposals.iter().enumerate() {
            match self.prepare_route(proposal, &read_cwd, &scope) {
                Ok(route) => prepared.push(route),
                Err(reason) => rejected.push(LearnRouteRejection { index, reason }),
            }
        }
        if !rejected.is_empty() {
            return Ok(LearnRoutesResult {
                accepted: 0,
                rejected,
            });
        }
        let attempt = task.attempts.len() as i64 + 1;
        let records = prepared
            .iter()
            .map(|route| RouteRecord {
                aliases: &route.aliases,
                path: &route.path,
                symbol: &route.symbol,
                source_digest: &route.digest,
                task_id: &task.id,
                attempt,
                profile_id: &task.profile_id,
                model: &task.model,
            })
            .collect::<Vec<_>>();
        routes::save(self.store, &index_cwd, &records, &timestamp_now())?;
        Ok(LearnRoutesResult {
            accepted: prepared.len(),
            rejected,
        })
    }

    /// Save a route someone typed at `oga relearn`.
    pub fn learn_user_route(
        &self,
        cwd: impl AsRef<Path>,
        proposal: &LearnRouteProposal,
    ) -> Result<(), ContextError> {
        let cwd = cwd.as_ref();
        let route = self
            .prepare_route(proposal, cwd, &["**".to_owned()])
            .map_err(ContextError::Invalid)?;
        routes::save(
            self.store,
            cwd,
            &[RouteRecord {
                aliases: &route.aliases,
                path: &route.path,
                symbol: &route.symbol,
                source_digest: &route.digest,
                task_id: "",
                attempt: 0,
                profile_id: "user",
                model: "",
            }],
            &timestamp_now(),
        )?;
        Ok(())
    }

    fn rank(
        &self,
        index_cwd: &Path,
        question: &str,
        terms: &[String],
    ) -> Result<Vec<Scored>, ContextError> {
        let (_, total) = index_store::counts(self.store, index_cwd)?;
        let weights = TermWeights::new(
            &index_store::term_hits(self.store, index_cwd, terms)?,
            total,
        );
        let mut ranking = Ranking::default();
        for route in routes::matching(self.store, index_cwd, question, terms)? {
            if !route.exact && routes::overlap(&route, terms) == 0 {
                continue;
            }
            let Some(symbol) = self.route_target(index_cwd, &route)? else {
                routes::forget(self.store, route.id)?;
                continue;
            };
            ranking.add_route(&route, symbol, terms, &weights);
        }
        let question_key = name_key(question);
        let mut keys = vec![question_key.clone()];
        keys.extend(terms.iter().cloned());
        keys.sort();
        keys.dedup();
        for symbol in index_store::symbols_by_name(self.store, index_cwd, &keys, 32)? {
            ranking.add_symbol(symbol, terms, &question_key, &weights, None);
        }
        let search = index_store::symbols_by_search(
            self.store,
            index_cwd,
            &fts_query(terms),
            CANDIDATE_POOL,
        )?;
        for (symbol, rank) in search {
            ranking.add_symbol(symbol, terms, &question_key, &weights, Some(rank));
        }
        Ok(ranking.ranked())
    }

    /// Where a route points now, as a symbol row. A route saved against a
    /// whole file answers with the file itself.
    fn route_target(
        &self,
        index_cwd: &Path,
        route: &routes::LearnedRoute,
    ) -> Result<Option<SymbolRow>, ContextError> {
        match &route.symbol {
            Some(name) => Ok(index_store::symbol_at(
                self.store,
                index_cwd,
                &route.path,
                name,
            )?),
            None => Ok(
                index_store::file_exists(self.store, index_cwd, &route.path)?
                    .then(|| file_anchor(&route.path)),
            ),
        }
    }

    /// Confirm a candidate still exists where the index says, re-reading the
    /// task's own checkout when it holds a different copy of the file.
    fn locate(
        &self,
        target: &ContextTarget,
        symbol: &SymbolRow,
    ) -> Result<Option<SymbolRow>, ContextError> {
        let Ok(source) = fs::read_to_string(target.cwd.join(&symbol.path)) else {
            // A checkout that has not materialised the file yet still answers
            // from the origin it was cut from.
            let origin = target.index_cwd().join(&symbol.path);
            return Ok(origin.is_file().then(|| symbol.clone()));
        };
        if target.source_cwd.is_none() || symbol.name.is_empty() {
            return Ok(Some(symbol.clone()));
        }
        if digest_of(source.as_bytes()) == file_digest(target.index_cwd(), &symbol.path) {
            return Ok(Some(symbol.clone()));
        }
        let Some(extracted) = extract_symbols(&symbol.path, &source) else {
            return Ok(None);
        };
        Ok(extracted
            .symbols
            .into_iter()
            .find(|found| found.name == symbol.name)
            .map(|found| SymbolRow {
                line: found.line,
                end_line: found.end_line,
                ..symbol.clone()
            }))
    }

    /// Check one proposal against the files a task may read, and hash the
    /// symbol it names so the route can follow a later rename.
    fn prepare_route(
        &self,
        proposal: &LearnRouteProposal,
        read_cwd: &Path,
        scope: &[String],
    ) -> Result<PreparedRoute, String> {
        let aliases = routes::aliases(&proposal.hints);
        if aliases.is_empty() || aliases.len() > MAX_HINTS_CHARS {
            return Err(format!(
                "hints must contain at least one useful word and at most {MAX_HINTS_CHARS} characters"
            ));
        }
        let path = proposal.path.trim().replace('\\', "/");
        let relative = safe_relative(read_cwd, &path)
            .ok_or("path must name a mapped source file inside the task cwd")?;
        if !walk::is_indexable(&relative) || !scope_covers_path(scope, read_cwd, &relative) {
            return Err("path is outside the task scope or not mapped".into());
        }
        let unreadable = || format!("{relative} is missing, oversized, or not indexable");
        let source = fs::read_to_string(read_cwd.join(&relative)).map_err(|_| unreadable())?;
        let Some(symbol) = proposal
            .symbol
            .as_deref()
            .map(str::trim)
            .filter(|symbol| !symbol.is_empty())
        else {
            return Ok(PreparedRoute {
                aliases,
                digest: digest_of(source.as_bytes()),
                path: relative,
                symbol: String::new(),
            });
        };
        let extracted = extract_symbols(&relative, &source).ok_or_else(unreadable)?;
        let found = extracted
            .symbols
            .iter()
            .find(|candidate| candidate.name == symbol)
            .ok_or_else(|| format!("symbol {symbol} is not present in {relative}"))?;
        Ok(PreparedRoute {
            aliases,
            digest: symbol_digest(&source, &found.name, found.line, found.end_line),
            path: relative,
            symbol: symbol.to_owned(),
        })
    }
}

/// A route proposal that passed every check, ready to save.
struct PreparedRoute {
    aliases: String,
    path: String,
    /// Empty when the route names a whole file.
    symbol: String,
    digest: String,
}

/// Read and parse every file, on as many threads as the machine has.
fn parse_all(cwd: &Path, files: &[WalkFile]) -> Vec<FileUpdate> {
    files
        .par_iter()
        .filter(|file| file.size <= MAX_FILE_BYTES)
        .filter_map(|file| {
            let bytes = fs::read(walk::absolute_path(cwd, &file.path)).ok()?;
            let source = String::from_utf8(bytes).ok()?;
            let extracted = extract_symbols(&file.path, &source)?;
            Some(FileUpdate {
                path: file.path.clone(),
                lang: extracted.lang.to_owned(),
                digest: digest_of(source.as_bytes()),
                size: file.size,
                mtime_ms: file.mtime_ms,
                lines: line_count(source.as_bytes()) as u64,
                symbols: extracted
                    .symbols
                    .into_iter()
                    .map(|symbol| SymbolRow {
                        digest: symbol_digest(&source, &symbol.name, symbol.line, symbol.end_line),
                        path: file.path.clone(),
                        kind: symbol.kind,
                        name: symbol.name,
                        qualified: symbol.qualified,
                        parent: symbol.parent,
                        line: symbol.line,
                        end_line: symbol.end_line,
                        signature: symbol.signature,
                        doc: symbol.doc,
                        exported: symbol.exported,
                    })
                    .collect(),
            })
        })
        .collect()
}

/// Stop before the project's symbol budget, keeping whole files.
fn truncate_to_budget(updates: &mut Vec<FileUpdate>, max_symbols: usize) -> bool {
    let mut total = 0;
    for (index, update) in updates.iter().enumerate() {
        total += update.symbols.len();
        if total > max_symbols {
            updates.truncate(index);
            return true;
        }
    }
    false
}

/// A file that vanished and a file that appeared with the same contents is one
/// file that moved — but only when each side is unambiguous.
fn detect_moves(
    known: &HashMap<String, index_store::FileRow>,
    updates: &[FileUpdate],
    vanished: &[String],
) -> Vec<(String, String)> {
    let mut old = HashMap::<&str, Vec<&String>>::new();
    for path in vanished {
        if let Some(file) = known.get(path) {
            old.entry(file.digest.as_str()).or_default().push(path);
        }
    }
    let mut new = HashMap::<&str, Vec<&String>>::new();
    for update in updates {
        new.entry(update.digest.as_str())
            .or_default()
            .push(&update.path);
    }
    old.into_iter()
        .filter_map(|(digest, from)| {
            let to = new.get(digest)?;
            (from.len() == 1 && to.len() == 1).then(|| (from[0].clone(), to[0].clone()))
        })
        .collect()
}

fn file_digest(cwd: &Path, path: &str) -> String {
    fs::read(cwd.join(path))
        .map(|bytes| digest_of(&bytes))
        .unwrap_or_default()
}

/// A symbol's declaration with its own name masked out, so a rename does not
/// change the hash and the route can follow it.
fn symbol_digest(source: &str, name: &str, line: u64, end_line: u64) -> String {
    let lines = source.lines().collect::<Vec<_>>();
    let start = line.saturating_sub(1) as usize;
    let end = (end_line as usize).min(lines.len());
    let body = lines.get(start..end).unwrap_or_default().join("\n");
    digest_of(body.replacen(name, "<symbol>", 1).as_bytes())
}

fn file_anchor(path: &str) -> SymbolRow {
    SymbolRow {
        path: path.to_owned(),
        kind: oga_domain::SymbolKind::Module,
        name: String::new(),
        qualified: path.to_owned(),
        parent: None,
        line: 1,
        end_line: 1,
        signature: String::new(),
        doc: None,
        exported: true,
        digest: String::new(),
    }
}

fn miss(question: &str, absent: &[String]) -> ContextResult {
    let detail = if absent.is_empty() {
        String::new()
    } else {
        format!(" Not indexed: {}.", absent.join(", "))
    };
    ContextResult {
        markdown: format!(
            "No confident match for \"{question}\" in this project.{detail} Search the tree or read likely files directly."
        ),
        candidates: Vec::new(),
    }
}

/// What survived the scope and existence checks, and how many did not.
#[derive(Debug, Default)]
struct Reachable {
    kept: Vec<Scored>,
    outside_scope: usize,
    gone: usize,
}

/// One bare anchor when the ranking landed on a single sure answer, otherwise
/// every candidate the caller may read, ranked, each with the words it
/// matched so the reader can judge them instead of trusting one arbitrary
/// pick.
fn answer_lines(
    candidates: &[QuestionCandidate],
    kept: &[Scored],
    terms: &[String],
    confident: bool,
    code: bool,
) -> Vec<String> {
    let bare = confident && candidates.len() == 1;
    let mut lines = Vec::new();
    for (candidate, scored) in candidates.iter().zip(kept) {
        let anchor = entry_line(
            &candidate.path,
            candidate.symbol.as_deref(),
            Vec::new(),
            Some(candidate.line),
        );
        lines.push(if bare {
            anchor
        } else {
            let matched = terms
                .iter()
                .filter(|term| scored.matched.contains(term))
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            format!("{anchor} (matched: {matched})")
        });
        if code {
            lines.push(
                candidate
                    .code
                    .as_deref()
                    .unwrap_or("source unavailable")
                    .to_owned(),
            );
        }
    }
    lines
}

fn omitted(count: usize, reason: &str) -> String {
    format!(
        "({count} candidate{} omitted: {reason})",
        if count == 1 { "" } else { "s" }
    )
}

fn source_body(target: &ContextTarget, symbol: &SymbolRow) -> String {
    let path = target.cwd.join(&symbol.path);
    let Ok(source) = fs::read_to_string(&path) else {
        return format!("source unavailable: {}", path.display());
    };
    let lines = source.lines().collect::<Vec<_>>();
    let (start, end) = if symbol.name.is_empty() {
        (1, lines.len())
    } else {
        (symbol.line as usize, symbol.end_line as usize)
    };
    if start == 0 || start > end || end > lines.len() {
        return format!("source span unavailable: {}:{start}-{end}", path.display());
    }
    let mut body = lines[start - 1..end]
        .iter()
        .map(|line| (*line).to_owned())
        .collect::<Vec<_>>();
    if body.len() > MAX_FILE_BODY_LINES {
        let remaining = body.len() - MAX_FILE_BODY_LINES;
        body.truncate(MAX_FILE_BODY_LINES);
        body.push(format!("… {remaining} more lines"));
    }
    format!("```text\n{}\n```", body.join("\n"))
}

fn entry_line(path: &str, symbol: Option<&str>, notes: Vec<String>, line: Option<u64>) -> String {
    let mut entry = path.to_owned();
    if let Some(line) = line {
        entry.push_str(&format!(":{line}"));
    }
    if let Some(symbol) = symbol.filter(|symbol| !symbol.is_empty()) {
        entry.push('#');
        entry.push_str(symbol);
    }
    let notes = notes
        .into_iter()
        .filter(|note| !note.is_empty())
        .collect::<Vec<_>>();
    if !notes.is_empty() {
        entry.push_str(" # ");
        entry.push_str(&notes.join(" · "));
    }
    entry
}

fn line_count(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let count = bytes.iter().filter(|byte| **byte == b'\n').count();
    if bytes.last() == Some(&b'\n') {
        count
    } else {
        count + 1
    }
}

fn relative_inside(cwd: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(cwd)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| {
            relative
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        })
}

fn safe_relative(cwd: &Path, raw: &str) -> Option<String> {
    let path = Path::new(raw);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    let relative = path.to_string_lossy().replace('\\', "/");
    relative_inside(cwd, &cwd.join(&relative))
}

fn scope_covers_path(rules: &[String], cwd: &Path, target: &str) -> bool {
    let target = cwd.join(target);
    rules.iter().any(|rule| {
        let rule = rule.trim().replace('\\', "/");
        if rule == "**" {
            return relative_inside(cwd, &target).is_some();
        }
        let recursive = rule.ends_with("/**");
        let base = cwd.join(rule.trim_end_matches("/**").trim_end_matches('/'));
        if recursive || base.is_dir() {
            target == base || relative_inside(&base, &target).is_some()
        } else {
            target == base
        }
    })
}

fn digest_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .chars()
        .take(DIGEST_CHARS)
        .collect()
}

fn timestamp_now() -> String {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let days = elapsed.as_secs() / 86_400;
    let seconds = elapsed.as_secs() % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3_600,
        (seconds / 60) % 60,
        seconds % 60,
        elapsed.subsec_millis()
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month as u32, day as u32)
}
