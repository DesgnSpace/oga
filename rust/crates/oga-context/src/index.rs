use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oga_domain::{
    ContextFile, ContextMapRow, ContextMapState, ContextRefs, ContextSymbol, MapFileStatus,
    SourceLang, SymbolKind, Task, TaskScope,
};
use oga_store::{Store, StoreError};
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::symbols::{ExtractedFile, ExtractedSymbol, extract_refs, extract_symbols};
use crate::text::{
    MAP_STOP_WORDS, fts_query, hint_key, identifier_tokens, normalize_word, prompt_terms,
    raw_words, words,
};
use crate::walk::{
    ContextWalkFile, WalkOptions, absolute_path, mapped_extension, mtime_ms, walk_context_files,
};

pub const MAP_SCHEME: u32 = 6;
pub const MAX_BUILD_FILES: usize = 2_000;
pub const BUILD_BUDGET: Duration = Duration::from_secs(2);
pub const MAX_SYMBOLS_PER_CWD: usize = 5_000;
pub const MAX_FILE_BYTES: u64 = 500 * 1024;
pub const MAX_SYMBOLS_PER_FILE: usize = 40;
const MAX_LEARNED_ROUTES: usize = 12;
const MAX_HINTS_CHARS: usize = 160;
const MAX_ROUTE_ALIASES: usize = 8;
const CANDIDATE_POOL: usize = 24;

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid context request: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    pub max_files: usize,
    pub budget: Duration,
    pub max_symbols: usize,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            max_files: MAX_BUILD_FILES,
            budget: BUILD_BUDGET,
            max_symbols: MAX_SYMBOLS_PER_CWD,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildResult {
    pub partial: bool,
    pub file_count: usize,
    pub symbol_count: usize,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteMove {
    pub from_path: String,
    pub from_symbol: Option<String>,
    pub to_path: String,
    pub to_symbol: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ContextTarget {
    pub cwd: PathBuf,
    pub source_cwd: Option<PathBuf>,
    pub scope: TaskScope,
}

impl ContextTarget {
    pub fn new(cwd: impl Into<PathBuf>, scope: TaskScope) -> Self {
        Self {
            cwd: cwd.into(),
            source_cwd: None,
            scope,
        }
    }

    pub fn worktree(cwd: impl Into<PathBuf>, origin: impl Into<PathBuf>, scope: TaskScope) -> Self {
        Self {
            cwd: cwd.into(),
            source_cwd: Some(origin.into()),
            scope,
        }
    }

    fn map_cwd(&self) -> &Path {
        self.source_cwd.as_deref().unwrap_or(&self.cwd)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTier {
    Full,
    Skeleton,
    Index,
}

#[derive(Debug, Clone, Default)]
pub struct QueryOptions {
    pub paths: Vec<String>,
    pub symbols: Vec<String>,
    pub tier: Option<RenderTier>,
    pub depth: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionCandidate {
    pub path: String,
    pub line: u64,
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextResult {
    pub markdown: String,
    pub files: Vec<ContextFile>,
    pub outside_scope: usize,
    pub gone: usize,
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

#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeVerification {
    pub path: String,
    pub source_digest: String,
    pub checkout_digest: Option<String>,
    pub changed: bool,
    pub file: Option<ContextFile>,
}

#[derive(Clone)]
pub struct ContextIndex<'a> {
    store: &'a Store,
}

impl<'a> ContextIndex<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn build(
        &self,
        cwd: impl AsRef<Path>,
        options: BuildOptions,
    ) -> Result<BuildResult, ContextError> {
        let cwd = cwd.as_ref();
        let existing = self.list_files(cwd)?;
        let walk = walk_context_files(
            cwd,
            WalkOptions {
                max_files: options.max_files,
                budget: options.budget,
            },
        );
        let mut seen = HashSet::new();
        let mut symbol_count = 0;
        let mut partial = walk.partial;
        for file in &walk.files {
            let Some(bytes) = read_indexable_file(cwd, file) else {
                continue;
            };
            let extracted = extract_symbols(
                &String::from_utf8_lossy(&bytes),
                file.entry.lang,
                file.entry.generic,
            );
            if extracted.symbols.is_empty() && !extracted.unparsed {
                continue;
            }
            if symbol_count + extracted.symbols.len() > options.max_symbols {
                partial = true;
                break;
            }
            let path = file.path.clone();
            let previous = existing.get(&path);
            let record = self.make_file_record(cwd, file, &bytes, extracted, previous, false)?;
            symbol_count += record.symbols.len();
            seen.insert(path.clone());
            self.upsert_file(&record, &symbol_digests(&bytes, &record.symbols))?;
        }
        if !partial {
            for path in existing.keys().filter(|path| !seen.contains(*path)) {
                self.delete_file(cwd, path)?;
            }
        }
        self.recompute_importance(cwd)?;
        let files = self.list_files(cwd)?;
        let symbol_count = files.values().map(|file| file.symbols.len()).sum();
        let now = timestamp_now();
        self.set_map(
            cwd,
            &ContextMapRow {
                cwd: cwd.display().to_string(),
                scheme: MAP_SCHEME,
                state: if partial {
                    ContextMapState::Partial
                } else {
                    ContextMapState::Ready
                },
                built_at: Some(now.clone()),
                file_count: files.len() as u64,
                symbol_count: symbol_count as u64,
                pending_prose: pending_prose(&files),
                updated_at: now.clone(),
            },
        )?;
        self.set_search_state(cwd, "ready", &now, files.len(), symbol_count, None)?;
        Ok(BuildResult {
            partial,
            file_count: files.len(),
            symbol_count,
        })
    }

    pub fn ensure(&self, cwd: impl AsRef<Path>) -> Result<(), ContextError> {
        let cwd = cwd.as_ref();
        let map = self.map(cwd)?;
        if map.as_ref().is_none_or(|map| map.scheme != MAP_SCHEME) {
            self.build(cwd, BuildOptions::default())?;
        }
        Ok(())
    }

    pub fn reconcile(
        &self,
        cwd: impl AsRef<Path>,
        options: BuildOptions,
    ) -> Result<ReconcileResult, ContextError> {
        let cwd = cwd.as_ref();
        let existing = self.list_files(cwd)?;
        if existing.is_empty() {
            let result = self.build(cwd, options)?;
            return Ok(ReconcileResult {
                partial: result.partial,
                changed: result.file_count > 0,
                file_count: result.file_count,
                symbol_count: result.symbol_count,
                refreshed: result.file_count,
                moved: 0,
                removed: 0,
                routes_confirmed: 0,
                routes_dropped: 0,
                route_moves: Vec::new(),
            });
        }
        let walk = walk_context_files(
            cwd,
            WalkOptions {
                max_files: options.max_files,
                budget: options.budget,
            },
        );
        let mut touched = Vec::new();
        let mut seen = HashSet::new();
        let mut partial = walk.partial;
        let mut symbol_count = 0;
        for file in &walk.files {
            let previous = existing.get(&file.path);
            if previous.is_some_and(|previous| {
                previous.size == file.metadata.len()
                    && previous.mtime_ms == mtime_ms(&file.metadata) as f64
            }) {
                seen.insert(file.path.clone());
                symbol_count += previous.map_or(0, |file| file.symbols.len());
                if symbol_count > options.max_symbols {
                    partial = true;
                    break;
                }
                continue;
            }
            let Some(bytes) = read_indexable_file(cwd, file) else {
                continue;
            };
            let extracted = extract_symbols(
                &String::from_utf8_lossy(&bytes),
                file.entry.lang,
                file.entry.generic,
            );
            if extracted.symbols.is_empty() && !extracted.unparsed {
                continue;
            }
            if symbol_count + extracted.symbols.len() > options.max_symbols {
                partial = true;
                break;
            }
            symbol_count += extracted.symbols.len();
            seen.insert(file.path.clone());
            touched.push((file.clone(), bytes, extracted));
        }
        if partial {
            let result = self.build(cwd, options)?;
            return Ok(ReconcileResult {
                partial: true,
                changed: true,
                file_count: result.file_count,
                symbol_count: result.symbol_count,
                refreshed: result.file_count,
                moved: 0,
                removed: 0,
                routes_confirmed: 0,
                routes_dropped: 0,
                route_moves: Vec::new(),
            });
        }

        let vanished = existing
            .keys()
            .filter(|path| !seen.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        let moved = self.match_file_moves(&existing, &touched, &vanished)?;
        for (from, to) in &moved {
            self.move_file(cwd, from, to)?;
        }
        let moved_from = moved.iter().map(|(from, _)| from).collect::<HashSet<_>>();
        let changed = !touched.is_empty() || !vanished.is_empty() || !moved.is_empty();
        let affected = touched
            .iter()
            .map(|(file, _, _)| file.path.clone())
            .chain(moved.iter().map(|(_, to)| to.clone()))
            .chain(
                vanished
                    .iter()
                    .filter(|path| !moved_from.contains(*path))
                    .cloned(),
            )
            .collect::<Vec<_>>();
        for (file, bytes, extracted) in touched {
            let previous = existing.get(&file.path).or_else(|| {
                moved
                    .iter()
                    .find_map(|(from, to)| (to == &file.path).then(|| existing.get(from)).flatten())
            });
            let record = self.make_file_record(cwd, &file, &bytes, extracted, previous, false)?;
            self.upsert_file(&record, &symbol_digests(&bytes, &record.symbols))?;
        }
        for path in &vanished {
            if !moved_from.contains(path) {
                self.delete_file(cwd, path)?;
            }
        }
        let healed = self.heal_routes(cwd, &affected, &moved)?;
        if changed {
            self.recompute_importance(cwd)?;
        }
        let files = self.list_files(cwd)?;
        let symbol_count = files.values().map(|file| file.symbols.len()).sum();
        let now = timestamp_now();
        if changed {
            self.set_map_counts(cwd, files.len(), symbol_count, pending_prose(&files), &now)?;
            self.set_search_state(cwd, "ready", &now, files.len(), symbol_count, None)?;
        }
        Ok(ReconcileResult {
            partial: false,
            changed,
            file_count: files.len(),
            symbol_count,
            refreshed: affected
                .len()
                .saturating_sub(vanished.len().saturating_sub(moved.len())),
            moved: moved.len(),
            removed: vanished.len().saturating_sub(moved.len()),
            routes_confirmed: healed.0,
            routes_dropped: healed.1,
            route_moves: healed.2,
        })
    }

    pub fn fold_task(&self, task: &Task) -> Result<(), ContextError> {
        let mut refused = HashSet::new();
        let events = self.store.repositories().events().list(&task.id)?;
        for event in &events {
            if event.kind == "scope_refusal"
                && let Some(path) = event.payload.get("path").and_then(Value::as_str)
            {
                refused.insert(relative_path(task.cwd.as_str(), path));
            }
        }
        let mut candidates = BTreeSet::new();
        for event in &events {
            if !event.kind.starts_with("agent.") {
                continue;
            }
            for path in write_targets(&event.payload) {
                let absolute = Path::new(&task.cwd).join(&path);
                let Some(relative) = relative_inside(Path::new(&task.cwd), &absolute) else {
                    continue;
                };
                if mapped_extension(&relative).is_some() && !refused.contains(&relative) {
                    candidates.insert(relative);
                }
            }
        }
        let corrections = parse_corrections(&task.output, Path::new(&task.cwd));
        for path in &corrections.paths {
            candidates.insert(path.clone());
        }
        for path in &candidates {
            self.heal_file(Path::new(&task.cwd), path, true)?;
            self.apply_correction(Path::new(&task.cwd), path, &corrections)?;
        }
        if !candidates.is_empty() {
            self.recompute_importance(Path::new(&task.cwd))?;
        }
        let files = self.list_files(Path::new(&task.cwd))?;
        self.set_map_counts(
            Path::new(&task.cwd),
            files.len(),
            files.values().map(|file| file.symbols.len()).sum(),
            pending_prose(&files),
            &timestamp_now(),
        )?;
        self.set_search_state(
            Path::new(&task.cwd),
            "ready",
            &timestamp_now(),
            files.len(),
            files.values().map(|file| file.symbols.len()).sum(),
            None,
        )?;
        Ok(())
    }

    pub fn list(
        &self,
        target: &ContextTarget,
        options: &QueryOptions,
    ) -> Result<ContextResult, ContextError> {
        self.ensure(target.map_cwd())?;
        let map_cwd = target.map_cwd();
        let rows = self.list_files(map_cwd)?.into_values().collect::<Vec<_>>();
        let mut selected = BTreeMap::new();
        for path in &options.paths {
            let directory = path.is_empty()
                || path == "."
                || path.ends_with('/')
                || map_cwd.join(path).is_dir();
            for file in &rows {
                if (directory
                    && path_prefix(path, &file.path)
                    && depth_allowed(map_cwd, path, &file.path, options.depth))
                    || (!directory && file.path == *path)
                {
                    selected.insert(file.path.clone(), file.clone());
                }
            }
        }
        if options.paths.is_empty() && options.symbols.is_empty() {
            selected.extend(rows.iter().map(|file| (file.path.clone(), file.clone())));
        }
        for requested in &options.symbols {
            let prefix = requested.strip_suffix('*');
            for file in &rows {
                if file.symbols.iter().any(|symbol| {
                    prefix.map_or(symbol.name == *requested, |prefix| {
                        symbol.name.starts_with(prefix)
                    })
                }) {
                    selected.insert(file.path.clone(), file.clone());
                }
            }
        }
        let mut files = Vec::new();
        let mut outside_scope = 0;
        let mut gone = 0;
        for file in selected.into_values() {
            if !scope_covers_path(&target.scope.read, &target.cwd, &file.path) {
                outside_scope += 1;
                continue;
            }
            let Some(file) = self.verify_file(target, &file)? else {
                gone += 1;
                continue;
            };
            files.push(file);
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let map = self.map(map_cwd)?;
        let mut lines = vec![map_header(
            map_cwd,
            map.as_ref(),
            files.len(),
            files.iter().map(|file| file.symbols.len()).sum(),
        )];
        if let Some(depth) = options.depth {
            lines.push(format!("scope depth {depth}"));
        }
        if files.is_empty() {
            lines.push("No files are in this map area. Expand the map area or check the task's readable paths.".into());
        }
        for file in &files {
            let tier = options.tier.unwrap_or_else(|| {
                if options.paths.iter().any(|path| {
                    path_prefix(path, &file.path) && (path.ends_with('/') || path == ".")
                }) {
                    RenderTier::Skeleton
                } else {
                    RenderTier::Full
                }
            });
            lines.push(render_file(file, tier));
        }
        if outside_scope > 0 {
            lines.push(format!(
                "({outside_scope} path{} omitted: outside this task's read scope)",
                if outside_scope == 1 { "" } else { "s" }
            ));
        }
        if gone > 0 {
            lines.push(format!(
                "({gone} path{} omitted: no longer on disk)",
                if gone == 1 { "" } else { "s" }
            ));
        }
        Ok(ContextResult {
            markdown: lines.join("\n"),
            files,
            outside_scope,
            gone,
            candidates: Vec::new(),
        })
    }

    pub fn question(
        &self,
        target: &ContextTarget,
        question: &str,
    ) -> Result<ContextResult, ContextError> {
        self.ensure(target.map_cwd())?;
        let map_cwd = target.map_cwd();
        let rows = self.list_files(map_cwd)?.into_values().collect::<Vec<_>>();
        let terms = prompt_terms(question);
        if terms.is_empty() {
            return Ok(ContextResult {
                markdown: format!(
                    "No confident match for \"{question}\" in this map. Search the tree or read likely files directly."
                ),
                files: Vec::new(),
                outside_scope: 0,
                gone: 0,
                candidates: Vec::new(),
            });
        }
        let fts = self.fts_ranks(map_cwd, &terms).unwrap_or_default();
        let mut candidates = rows
            .iter()
            .filter_map(|file| {
                score_file(file, &terms, fts.get(&file.path).copied())
                    .map(|score| (file.clone(), score))
            })
            .collect::<Vec<_>>();
        let mut existing_anchors = candidates
            .iter()
            .map(|(file, score)| {
                format!(
                    "{}#{}",
                    file.path,
                    score.symbol.as_deref().unwrap_or_default()
                )
            })
            .collect::<HashSet<_>>();
        for route in self.learned_route_scores(map_cwd, question, &terms)? {
            let Some(file) = rows.iter().find(|file| file.path == route.path) else {
                self.forget_route(route.id)?;
                continue;
            };
            if let Some(symbol) = &route.symbol
                && !file
                    .symbols
                    .iter()
                    .any(|candidate| &candidate.name == symbol)
            {
                self.forget_route(route.id)?;
                continue;
            }
            let matched = terms
                .iter()
                .filter(|term| {
                    route
                        .aliases
                        .split_whitespace()
                        .any(|hint| equivalent_match(hint, term))
                })
                .cloned()
                .collect::<HashSet<_>>();
            if matched.is_empty() {
                continue;
            }
            let anchor = format!(
                "{}#{}",
                file.path,
                route.symbol.as_deref().unwrap_or_default()
            );
            if existing_anchors.contains(&anchor) {
                continue;
            }
            let relevance = if route.exact {
                1_000_000.0
            } else {
                500_000.0 + matched.len() as f64 * 1_000.0
            };
            candidates.push((
                file.clone(),
                FileScore {
                    total: relevance,
                    relevance,
                    strongest: 1_000.0,
                    matched,
                    symbol: route.symbol,
                    exact_identifier: false,
                },
            ));
            existing_anchors.insert(anchor);
        }
        candidates.sort_by(|left, right| {
            right
                .1
                .exact_identifier
                .cmp(&left.1.exact_identifier)
                .then_with(|| {
                    right
                        .1
                        .total
                        .partial_cmp(&left.1.total)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| left.0.path.cmp(&right.0.path))
        });
        let present = terms
            .iter()
            .filter(|term| rows.iter().any(|file| file_contains_term(file, term)))
            .cloned()
            .collect::<HashSet<_>>();
        let mut legal = Vec::new();
        let mut outside_scope = 0;
        let mut gone = 0;
        for (file, score) in candidates.iter().take(CANDIDATE_POOL) {
            if !scope_covers_path(&target.scope.read, &target.cwd, &file.path) {
                outside_scope += 1;
                continue;
            }
            let Some(file) = self.verify_file(target, file)? else {
                gone += 1;
                continue;
            };
            legal.push((file, score.clone()));
            if legal.len() == 3 {
                break;
            }
        }
        let question_candidates = legal
            .iter()
            .map(|(file, score)| QuestionCandidate {
                path: file.path.clone(),
                line: score
                    .symbol
                    .as_ref()
                    .and_then(|name| file.symbols.iter().find(|symbol| &symbol.name == name))
                    .map_or(1, |symbol| symbol.line),
                symbol: score.symbol.clone(),
            })
            .collect::<Vec<_>>();
        let mut lines = Vec::new();
        let top = legal.first();
        let absent = terms
            .iter()
            .filter(|term| !present.contains(*term))
            .cloned()
            .collect::<Vec<_>>();
        let confident = top.is_some_and(|(_, score)| {
            score.relevance >= 40.0
                && (score.matched.len() >= 2 || score.strongest >= 500.0)
                && legal
                    .get(1)
                    .is_none_or(|(_, other)| score.relevance >= other.relevance * 1.8)
        });
        if let Some((file, score)) = top {
            if confident || score.exact_identifier {
                lines.push(entry_line(
                    &file.path,
                    score.symbol.as_deref(),
                    Vec::new(),
                    Some(question_candidates[0].line),
                ));
            } else {
                let mut candidates = legal.iter().collect::<Vec<_>>();
                candidates.sort_by_key(|(_, score)| std::cmp::Reverse(score.matched.len()));
                for (file, score) in candidates {
                    let matched = terms
                        .iter()
                        .filter(|term| score.matched.contains(*term))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    let line = score
                        .symbol
                        .as_ref()
                        .and_then(|name| file.symbols.iter().find(|symbol| &symbol.name == name))
                        .map_or(1, |symbol| symbol.line);
                    lines.push(format!(
                        "{} (matched: {matched})",
                        entry_line(&file.path, score.symbol.as_deref(), Vec::new(), Some(line),)
                    ));
                }
            }
        } else {
            lines.push(format!("No confident match for \"{question}\" in this map.{} Search the tree or read likely files directly.", if absent.is_empty() { String::new() } else { format!(" No map match: {}.", absent.join(", ")) }));
        }
        if outside_scope > 0 {
            lines.push(format!(
                "({outside_scope} candidate{} omitted: outside this task's read scope)",
                if outside_scope == 1 { "" } else { "s" }
            ));
        }
        if gone > 0 {
            lines.push(format!(
                "({gone} candidate{} omitted: no longer on disk)",
                if gone == 1 { "" } else { "s" }
            ));
        }
        Ok(ContextResult {
            markdown: lines.join("\n"),
            files: legal.into_iter().map(|(file, _)| file).collect(),
            outside_scope,
            gone,
            candidates: question_candidates,
        })
    }

    pub fn learn_routes(
        &self,
        task: &Task,
        routes: &[LearnRouteProposal],
    ) -> Result<LearnRoutesResult, ContextError> {
        if routes.len() > MAX_LEARNED_ROUTES {
            return Ok(LearnRoutesResult {
                accepted: 0,
                rejected: vec![LearnRouteRejection {
                    index: MAX_LEARNED_ROUTES,
                    reason: format!("at most {MAX_LEARNED_ROUTES} routes are allowed"),
                }],
            });
        }
        let source_cwd = task.worktree.as_ref().map_or_else(
            || PathBuf::from(&task.cwd),
            |worktree| PathBuf::from(&worktree.origin_cwd),
        );
        let attempt = task.attempts.len() as i64 + 1;
        let mut prepared = Vec::new();
        let mut rejected = Vec::new();
        let mut seen = HashSet::new();
        for (index, route) in routes.iter().enumerate() {
            let aliases = route_aliases(&route.hints);
            let path = route.path.trim().replace('\\', "/");
            let symbol = route
                .symbol
                .as_ref()
                .map(|symbol| symbol.trim().to_owned())
                .filter(|symbol| !symbol.is_empty());
            if aliases.is_empty() || aliases.len() > MAX_HINTS_CHARS {
                rejected.push(LearnRouteRejection {
                    index,
                    reason: format!("hints must contain at least one useful word and at most {MAX_HINTS_CHARS} characters"),
                });
                continue;
            }
            let Some(relative) = safe_relative(Path::new(&task.cwd), &path) else {
                rejected.push(LearnRouteRejection {
                    index,
                    reason: "path must name a mapped source file inside the task cwd".into(),
                });
                continue;
            };
            if mapped_extension(&relative).is_none()
                || !scope_covers_path(
                    &task
                        .scope
                        .read
                        .iter()
                        .chain(task.scope.write.iter())
                        .cloned()
                        .collect::<Vec<_>>(),
                    Path::new(&task.cwd),
                    &relative,
                )
            {
                rejected.push(LearnRouteRejection {
                    index,
                    reason: "path is outside the task scope or not mapped".into(),
                });
                continue;
            }
            let source_path = Path::new(&task.cwd).join(&relative);
            let Some((digest, symbols)) = read_source_digest(&source_path) else {
                rejected.push(LearnRouteRejection {
                    index,
                    reason: format!("{relative} is missing, oversized, or not indexable"),
                });
                continue;
            };
            if let Some(symbol) = &symbol
                && !symbols.contains(symbol)
            {
                rejected.push(LearnRouteRejection {
                    index,
                    reason: format!("symbol {symbol} is not present in {relative}"),
                });
                continue;
            }
            let digest = route_digest(&source_path, symbol.as_deref()).unwrap_or(digest);
            let dedupe = format!(
                "{aliases}\n{relative}\n{}",
                symbol.as_deref().unwrap_or_default()
            );
            if seen.insert(dedupe) {
                prepared.push((aliases, relative, symbol, digest));
            }
        }
        if !rejected.is_empty() {
            return Ok(LearnRoutesResult {
                accepted: 0,
                rejected,
            });
        }
        if task.worktree.is_none() {
            for (_, path, _, _) in &prepared {
                self.heal_file(Path::new(&task.cwd), path, false)?;
            }
        }
        let now = timestamp_now();
        self.store.transaction(|transaction| {
            for (aliases, path, symbol, digest) in &prepared {
                let entity_id: Option<i64> = transaction
                    .query_row(
                        "SELECT id FROM context_entities WHERE cwd=? AND path=? AND ((? = '' AND kind='file') OR (? != '' AND kind != 'file' AND name=?)) LIMIT 1",
                        params![source_cwd.display().to_string(), path, symbol.as_deref().unwrap_or_default(), symbol.as_deref().unwrap_or_default(), symbol.as_deref().unwrap_or_default()],
                        |row| row.get(0),
                    )
                    .optional()?;
                save_route(
                    transaction,
                    RouteRecord {
                        cwd: &source_cwd.display().to_string(),
                        entity_id,
                        path,
                        symbol: symbol.as_deref().unwrap_or_default(),
                        aliases,
                        source_digest: digest,
                        task_id: &task.id,
                        attempt,
                        profile_id: &task.profile_id,
                        model: &task.model,
                        now: &now,
                    },
                )?;
            }
            Ok(())
        })?;
        Ok(LearnRoutesResult {
            accepted: prepared.len(),
            rejected,
        })
    }

    pub fn learn_user_route(
        &self,
        cwd: impl AsRef<Path>,
        route: &LearnRouteProposal,
    ) -> Result<(), ContextError> {
        let cwd = cwd.as_ref();
        let aliases = route_aliases(&route.hints);
        if aliases.is_empty() || aliases.len() > MAX_HINTS_CHARS {
            return Err(ContextError::Invalid(
                "hints must contain at least one useful word and at most 160 characters".into(),
            ));
        }
        let path = route.path.trim().replace('\\', "/");
        let Some(relative) = safe_relative(cwd, &path) else {
            return Err(ContextError::Invalid(
                "path must name a mapped source file inside the current directory".into(),
            ));
        };
        let files = self.list_files(cwd)?;
        let Some(file) = files.get(&relative) else {
            return Err(ContextError::Invalid(
                "path must name a mapped source file inside the current directory".into(),
            ));
        };
        let symbol = route
            .symbol
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(symbol) = symbol
            && !file
                .symbols
                .iter()
                .any(|candidate| candidate.name == symbol)
        {
            return Err(ContextError::Invalid(format!(
                "symbol {symbol} is not present in {relative}"
            )));
        }
        let now = timestamp_now();
        self.store.transaction(|transaction| {
            let entity_id: i64 = transaction.query_row(
                "SELECT id FROM context_entities WHERE cwd=? AND path=? AND ((?='' AND kind='file') OR (? != '' AND kind!='file' AND name=?)) LIMIT 1",
                params![cwd.display().to_string(), relative, symbol.unwrap_or_default(), symbol.unwrap_or_default(), symbol.unwrap_or_default()],
                |row| row.get(0),
            )?;
            let digest = route_digest(cwd.join(&relative).as_path(), symbol)
                .unwrap_or_else(|| file.digest.clone());
            save_route(
                transaction,
                RouteRecord {
                    cwd: &cwd.display().to_string(),
                    entity_id: Some(entity_id),
                    path: &relative,
                    symbol: symbol.unwrap_or_default(),
                    aliases: &aliases,
                    source_digest: &digest,
                    task_id: "",
                    attempt: 0,
                    profile_id: "user",
                    model: "",
                    now: &now,
                },
            )?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn verify_worktree(
        &self,
        origin_cwd: impl AsRef<Path>,
        checkout_cwd: impl AsRef<Path>,
        paths: &[String],
    ) -> Result<Vec<WorktreeVerification>, ContextError> {
        let origin_cwd = origin_cwd.as_ref();
        let checkout_cwd = checkout_cwd.as_ref();
        let rows = self.list_files(origin_cwd)?;
        let by_path = rows
            .into_values()
            .map(|file| (file.path.clone(), file))
            .collect::<HashMap<_, _>>();
        let mut result = Vec::new();
        for path in paths {
            let Some(source) = by_path.get(path) else {
                continue;
            };
            let checkout = checkout_cwd.join(path);
            let checkout_digest = read_source_digest(&checkout).map(|(digest, _)| digest);
            let file = checkout_digest.as_ref().and_then(|digest| {
                if digest == &source.digest {
                    Some(source.clone())
                } else {
                    read_context_file(checkout_cwd, path, Some(source))
                        .ok()
                        .flatten()
                }
            });
            result.push(WorktreeVerification {
                path: path.clone(),
                source_digest: source.digest.clone(),
                changed: checkout_digest.as_ref() != Some(&source.digest),
                checkout_digest,
                file,
            });
        }
        Ok(result)
    }

    pub fn verify_task_worktree(
        &self,
        task: &Task,
        paths: &[String],
    ) -> Result<Vec<WorktreeVerification>, ContextError> {
        let Some(worktree) = &task.worktree else {
            return Ok(Vec::new());
        };
        self.verify_worktree(&worktree.origin_cwd, &worktree.path, paths)
    }

    pub fn map(&self, cwd: impl AsRef<Path>) -> Result<Option<ContextMapRow>, ContextError> {
        let cwd = cwd.as_ref().display().to_string();
        Ok(self.store.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT cwd,scheme,state,built_at,file_count,symbol_count,pending_prose,updated_at FROM context_maps WHERE cwd=?",
                    [&cwd],
                    map_from_row,
                )
                .optional()
                .map_err(Into::into)
        })?)
    }

    pub fn files(&self, cwd: impl AsRef<Path>) -> Result<Vec<ContextFile>, ContextError> {
        Ok(self.list_files(cwd.as_ref())?.into_values().collect())
    }

    pub fn learned_routes(
        &self,
        cwd: impl AsRef<Path>,
        question: &str,
    ) -> Result<Vec<QuestionCandidate>, ContextError> {
        let cwd = cwd.as_ref();
        let terms = prompt_terms(question);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .learned_route_scores(cwd, question, &terms)?
            .into_iter()
            .map(|route| QuestionCandidate {
                path: route.path,
                line: route.line.unwrap_or(1),
                symbol: route.symbol,
            })
            .collect())
    }

    /// Drop a route whose target left the map; its path or symbol no longer exists.
    fn forget_route(&self, id: i64) -> Result<(), ContextError> {
        self.store.transaction(|transaction| {
            transaction.execute("DELETE FROM context_learned_routes WHERE id=?", [id])?;
            Ok(())
        })?;
        Ok(())
    }

    fn heal_routes(
        &self,
        cwd: &Path,
        paths: &[String],
        moves: &[(String, String)],
    ) -> Result<(usize, usize, Vec<RouteMove>), ContextError> {
        let files = self.list_files(cwd)?;
        let mut confirmed = 0;
        let mut dropped = 0;
        let mut route_moves = Vec::new();
        let routes = self.store.with_connection(|connection| {
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
        for (id, mut path, symbol, source_digest) in routes {
            let from_path = path.clone();
            let from_symbol = non_empty(symbol.clone());
            if let Some((_, to)) = moves.iter().find(|(from, _)| from == &path) {
                path = to.clone();
            }
            if !paths.contains(&path) && path == from_path {
                continue;
            }
            let direct = files.get(&path).and_then(|file| {
                (symbol.is_empty()
                    || file
                        .symbols
                        .iter()
                        .any(|candidate| candidate.name == symbol))
                .then_some((path.clone(), symbol.clone(), file.digest.clone()))
            });
            let resolved = if let Some((path, symbol, digest)) = direct {
                self.store.with_connection(|connection| {
                    Ok(connection
                        .query_row(
                            "SELECT id,path,CASE WHEN kind='file' THEN '' ELSE name END,digest FROM context_entities WHERE cwd=? AND path=? AND ((?='' AND kind='file') OR (? != '' AND kind!='file' AND name=?)) LIMIT 1",
                            params![cwd.display().to_string(), path, symbol, symbol, symbol],
                            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, digest)),
                        )
                        .optional()?)
                })?
            } else {
                self.store.with_connection(|connection| {
                    let mut statement = connection.prepare(
                        "SELECT id,path,name,digest FROM context_entities WHERE cwd=? AND kind!='file' AND digest=? LIMIT 2",
                    )?;
                    let matches = statement
                        .query_map(params![cwd.display().to_string(), source_digest], |row| {
                            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok((matches.len() == 1).then(|| matches.into_iter().next().expect("one route target")))
                })?
            };
            let Some((entity_id, path, resolved_symbol, digest)) = resolved else {
                self.forget_route(id)?;
                dropped += 1;
                continue;
            };
            let source_digest =
                route_digest(cwd.join(&path).as_path(), Some(&resolved_symbol)).unwrap_or(digest);
            self.store.transaction(|transaction| {
                transaction.execute("UPDATE context_learned_routes SET learned_path=?,learned_symbol=?,entity_id=?,source_digest=?,last_confirmed_at=? WHERE id=?", params![path, resolved_symbol, entity_id, source_digest, timestamp_now(), id])?;
                Ok(())
            })?;
            if from_path != path || from_symbol.as_deref() != Some(resolved_symbol.as_str()) {
                route_moves.push(RouteMove {
                    from_path,
                    from_symbol,
                    to_path: path,
                    to_symbol: non_empty(resolved_symbol),
                });
            }
            confirmed += 1;
        }
        Ok((confirmed, dropped, route_moves))
    }

    fn list_files(&self, cwd: &Path) -> Result<HashMap<String, ContextFile>, ContextError> {
        Ok(self.store.with_connection(|connection| {
            let cwd = cwd.display().to_string();
            let mut statement = connection.prepare(
                "SELECT id,cwd,path,lang,purpose,lines,size,mtime_ms,digest,status,touch_count,touched_at,mapped_at,updated_at,header_comment,refs_json,importance FROM context_entities WHERE cwd=? AND kind='file' ORDER BY path",
            )?;
            let rows = statement
                .query_map([cwd.as_str()], |row| file_from_row(row, connection))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().map(|file| (file.path.clone(), file)).collect())
        })?)
    }

    fn set_map(&self, _cwd: &Path, map: &ContextMapRow) -> Result<(), ContextError> {
        self.store.transaction(|transaction| {
            transaction.execute(
                "INSERT INTO context_maps(cwd,scheme,state,built_at,file_count,symbol_count,pending_prose,updated_at,search_state,search_indexed_at,search_indexed_map_updated_at,search_indexed_file_count,search_indexed_symbol_count,search_last_error) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(cwd) DO UPDATE SET scheme=excluded.scheme,state=excluded.state,built_at=excluded.built_at,file_count=excluded.file_count,symbol_count=excluded.symbol_count,pending_prose=excluded.pending_prose,updated_at=excluded.updated_at",
                params![map.cwd, map.scheme, map_state(map.state), map.built_at, map.file_count, map.symbol_count, map.pending_prose, map.updated_at, "building", Option::<String>::None, Option::<String>::None, 0_i64, 0_i64, Option::<String>::None],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    fn set_map_counts(
        &self,
        cwd: &Path,
        file_count: usize,
        symbol_count: usize,
        pending: u64,
        now: &str,
    ) -> Result<(), ContextError> {
        let Some(mut map) = self.map(cwd)? else {
            return Ok(());
        };
        map.file_count = file_count as u64;
        map.symbol_count = symbol_count as u64;
        map.pending_prose = pending;
        map.updated_at = now.to_owned();
        self.set_map(cwd, &map)
    }

    fn set_search_state(
        &self,
        cwd: &Path,
        state: &str,
        now: &str,
        file_count: usize,
        symbol_count: usize,
        error: Option<&str>,
    ) -> Result<(), ContextError> {
        self.store.transaction(|transaction| {
            transaction.execute(
                "UPDATE context_maps SET search_state=?,search_indexed_at=?,search_indexed_map_updated_at=updated_at,search_indexed_file_count=?,search_indexed_symbol_count=?,search_last_error=? WHERE cwd=?",
                params![state, now, file_count, symbol_count, error, cwd.display().to_string()],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    fn make_file_record(
        &self,
        cwd: &Path,
        file: &ContextWalkFile,
        bytes: &[u8],
        extracted: ExtractedFile,
        previous: Option<&ContextFile>,
        touch: bool,
    ) -> Result<ContextFile, ContextError> {
        let now = timestamp_now();
        let symbols = reconcile_symbols(
            extracted.symbols,
            previous.map_or(&[], |file| &file.symbols),
        );
        let purpose = if let Some(reason) = extracted.unparsed_reason {
            Some(format!("Symbols unavailable: {reason}"))
        } else {
            previous.and_then(|file| {
                (!file
                    .purpose
                    .as_deref()
                    .is_some_and(|purpose| purpose.starts_with("Symbols unavailable:")))
                .then(|| file.purpose.clone())
                .flatten()
            })
        };
        let stat =
            fs::metadata(absolute_path(cwd, &file.path)).map_err(|source| ContextError::Io {
                path: absolute_path(cwd, &file.path),
                source,
            })?;
        let refs = extract_refs(
            &String::from_utf8_lossy(bytes),
            file.entry.lang,
            file.entry.generic,
        );
        Ok(ContextFile {
            cwd: cwd.display().to_string(),
            path: file.path.clone(),
            lang: file.entry.lang,
            purpose,
            header_comment: extracted
                .header_comment
                .or_else(|| previous.and_then(|file| file.header_comment.clone())),
            lines: line_count(bytes) as u64,
            size: bytes.len() as u64,
            mtime_ms: mtime_ms(&stat) as f64,
            digest: digest_of(bytes),
            symbols,
            status: if extracted.unparsed {
                MapFileStatus::Unparsed
            } else {
                MapFileStatus::Mapped
            },
            touch_count: if touch {
                previous.map_or(0, |file| file.touch_count) + 1
            } else {
                previous.map_or(0, |file| file.touch_count)
            },
            touched_at: if touch {
                Some(now.clone())
            } else {
                previous.and_then(|file| file.touched_at.clone())
            },
            mapped_at: previous.map_or_else(|| now.clone(), |file| file.mapped_at.clone()),
            updated_at: now,
            refs,
            importance: previous.map_or(0.0, |file| file.importance),
        })
    }

    fn upsert_file(
        &self,
        file: &ContextFile,
        symbol_digests: &HashMap<String, String>,
    ) -> Result<(), ContextError> {
        let now = &file.updated_at;
        let file_purpose = file.purpose.clone();
        let header = file.header_comment.clone().unwrap_or_default();
        let refs_json = serde_json::to_string(&file.refs)
            .map_err(|error| ContextError::Invalid(error.to_string()))?;
        let file_name = file.path.rsplit('/').next().unwrap_or(&file.path);
        let file_name = file_name
            .rsplit_once('.')
            .map_or(file_name, |(name, _)| name);
        let file_identifiers = identifier_tokens(&[file_name, &file.path]);
        let file_refs = [file.refs.imports.clone(), file.refs.calls.clone()]
            .concat()
            .join(" ");
        let file_id = self.store.transaction(|transaction| {
            transaction.execute(
                "INSERT INTO context_entities(cwd,kind,parent_id,path,line,end_line,name,digest,purpose,confirmed,params,returns,exported,comments_json,lang,status,lines,size,mtime_ms,touch_count,touched_at,mapped_at,header_comment,refs_json,importance,path_text,comments,signature,refs,identifier_tokens,symbol_kind,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(cwd,path) WHERE kind='file' DO UPDATE SET path=excluded.path,line=excluded.line,end_line=excluded.end_line,name=excluded.name,digest=excluded.digest,purpose=excluded.purpose,comments_json=excluded.comments_json,lang=excluded.lang,status=excluded.status,lines=excluded.lines,size=excluded.size,mtime_ms=excluded.mtime_ms,touch_count=excluded.touch_count,touched_at=excluded.touched_at,mapped_at=excluded.mapped_at,header_comment=excluded.header_comment,refs_json=excluded.refs_json,importance=excluded.importance,path_text=excluded.path_text,comments=excluded.comments,signature=excluded.signature,refs=excluded.refs,identifier_tokens=excluded.identifier_tokens,symbol_kind=excluded.symbol_kind,updated_at=excluded.updated_at",
                params![
                    file.cwd,
                    "file",
                    Option::<i64>::None,
                    file.path,
                    file.lines,
                    file.lines,
                    file_name,
                    file.digest,
                    file_purpose,
                    Option::<i64>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<i64>::None,
                    "[]",
                    lang_string(file.lang),
                    status_string(file.status),
                    file.lines,
                    file.size,
                    file.mtime_ms as i64,
                    file.touch_count,
                    file.touched_at,
                    file.mapped_at,
                    header,
                    refs_json,
                    file.importance,
                    file.path,
                    file.header_comment.clone().unwrap_or_default(),
                    "",
                    file_refs,
                    file_identifiers,
                    "file",
                    now,
                    now,
                ],
            )?;
            let id: i64 = transaction.query_row(
                "SELECT id FROM context_entities WHERE cwd=? AND path=? AND kind='file'",
                params![file.cwd, file.path],
                |row| row.get(0),
            )?;
            let existing = transaction
                .prepare("SELECT id,name FROM context_entities WHERE parent_id=? AND kind!='file'")?
                .query_map([id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            let existing_by_name = existing
                .into_iter()
                .map(|(id, name)| (name, id))
                .collect::<HashMap<_, _>>();
            let mut keep = HashSet::new();
            for symbol in &file.symbols {
                let symbol_digest = symbol_digests.get(&symbol.name).cloned().unwrap_or_default();
                let comments = serde_json::to_string(&symbol.comments).map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                let kind = symbol_kind(symbol.kind);
                let signature = [Some(kind), symbol.params.as_deref(), symbol.returns.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                let identifiers = identifier_tokens(&[&symbol.name]);
                if let Some(id) = existing_by_name.get(&symbol.name) {
                    transaction.execute(
                        "UPDATE context_entities SET kind=?,path=?,line=?,end_line=?,name=?,digest=?,purpose=?,confirmed=?,params=?,returns=?,exported=?,comments_json=?,lang=NULL,status=NULL,lines=NULL,size=NULL,mtime_ms=NULL,header_comment='',refs_json='{}',importance=0,path_text=?,comments=?,signature=?,refs='',identifier_tokens=?,symbol_kind=?,updated_at=? WHERE id=?",
                        params![kind, file.path, symbol.line, symbol.end_line, symbol.name, symbol_digest, symbol.purpose, i64::from(symbol.confirmed), symbol.params, symbol.returns, i64::from(symbol.exported), comments, file.path, symbol.comments.join("\n\n"), signature, identifiers, kind, now, id],
                    )?;
                    keep.insert(*id);
                } else {
                    transaction.execute(
                        "INSERT INTO context_entities(cwd,kind,parent_id,path,line,end_line,name,digest,purpose,confirmed,params,returns,exported,comments_json,lang,status,lines,size,mtime_ms,touch_count,touched_at,mapped_at,header_comment,refs_json,importance,path_text,comments,signature,refs,identifier_tokens,symbol_kind,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                        params![file.cwd, kind, id, file.path, symbol.line, symbol.end_line, symbol.name, symbol_digest, symbol.purpose, i64::from(symbol.confirmed), symbol.params, symbol.returns, i64::from(symbol.exported), comments, Option::<String>::None, Option::<String>::None, Option::<i64>::None, Option::<i64>::None, Option::<i64>::None, 0_i64, Option::<String>::None, Option::<String>::None, "", "{}", 0_i64, file.path, symbol.comments.join("\n\n"), signature, "", identifiers, kind, now, now],
                    )?;
                }
            }
            for (name, id) in existing_by_name {
                if !file
                    .symbols
                    .iter()
                    .any(|symbol| symbol.name == name.as_str())
                    && !keep.contains(&id)
                {
                    transaction.execute("DELETE FROM context_entities WHERE id=?", [id])?;
                }
            }
            Ok(id)
        })?;
        let _ = file_id;
        Ok(())
    }

    fn delete_file(&self, cwd: &Path, path: &str) -> Result<(), ContextError> {
        self.store.transaction(|transaction| {
            transaction.execute(
                "DELETE FROM context_entities WHERE cwd=? AND path=? AND kind='file'",
                params![cwd.display().to_string(), path],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    fn apply_correction(
        &self,
        cwd: &Path,
        path: &str,
        corrections: &Corrections,
    ) -> Result<(), ContextError> {
        let file_purpose = corrections.file_purposes.get(path);
        let symbols = corrections
            .symbol_purposes
            .iter()
            .filter(|((candidate_path, _), _)| candidate_path == path)
            .collect::<Vec<_>>();
        if file_purpose.is_none() && symbols.is_empty() {
            return Ok(());
        }
        self.store.transaction(|transaction| {
            if let Some(purpose) = file_purpose {
                transaction.execute(
                    "UPDATE context_entities SET purpose=?,confirmed=0 WHERE cwd=? AND path=? AND kind='file'",
                    params![purpose, cwd.display().to_string(), path],
                )?;
            }
            for ((_, name), purpose) in symbols {
                transaction.execute(
                    "UPDATE context_entities SET purpose=?,confirmed=0 WHERE cwd=? AND path=? AND kind!='file' AND name=?",
                    params![purpose, cwd.display().to_string(), path, name],
                )?;
            }
            Ok(())
        })?;
        Ok(())
    }

    fn move_file(&self, cwd: &Path, from: &str, to: &str) -> Result<(), ContextError> {
        self.store.transaction(|transaction| {
            transaction.execute(
                "UPDATE context_entities SET path=?,path_text=? WHERE cwd=? AND path=?",
                params![to, to, cwd.display().to_string(), from],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    fn heal_file(
        &self,
        cwd: &Path,
        path: &str,
        touch: bool,
    ) -> Result<Option<ContextFile>, ContextError> {
        let entry = mapped_extension(path);
        let Some(entry) = entry else { return Ok(None) };
        let absolute = cwd.join(path);
        let metadata = match fs::metadata(&absolute) {
            Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_FILE_BYTES => metadata,
            _ => {
                self.delete_file(cwd, path)?;
                return Ok(None);
            }
        };
        let bytes = fs::read(&absolute).map_err(|source| ContextError::Io {
            path: absolute.clone(),
            source,
        })?;
        let extracted =
            extract_symbols(&String::from_utf8_lossy(&bytes), entry.lang, entry.generic);
        if extracted.symbols.is_empty() && !extracted.unparsed {
            self.delete_file(cwd, path)?;
            return Ok(None);
        }
        let previous = self.list_files(cwd)?.remove(path);
        let walk = ContextWalkFile {
            path: path.to_owned(),
            entry,
            metadata,
        };
        let record =
            self.make_file_record(cwd, &walk, &bytes, extracted, previous.as_ref(), touch)?;
        self.upsert_file(&record, &symbol_digests(&bytes, &record.symbols))?;
        Ok(Some(record))
    }

    fn verify_file(
        &self,
        target: &ContextTarget,
        file: &ContextFile,
    ) -> Result<Option<ContextFile>, ContextError> {
        let source = target.source_cwd.as_deref().unwrap_or(&target.cwd);
        let absolute = source.join(&file.path);
        let Ok(bytes) = fs::read(&absolute) else {
            return Ok(None);
        };
        if digest_of(&bytes) == file.digest && source == target.map_cwd() {
            return Ok(Some(file.clone()));
        }
        let Some(entry) = mapped_extension(&file.path) else {
            return Ok(None);
        };
        let metadata = fs::metadata(&absolute).map_err(|source| ContextError::Io {
            path: absolute.clone(),
            source,
        })?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Ok(None);
        }
        let extracted =
            extract_symbols(&String::from_utf8_lossy(&bytes), entry.lang, entry.generic);
        let walk = ContextWalkFile {
            path: file.path.clone(),
            entry,
            metadata,
        };
        self.make_file_record(source, &walk, &bytes, extracted, Some(file), false)
            .map(Some)
    }

    fn match_file_moves(
        &self,
        existing: &HashMap<String, ContextFile>,
        touched: &[(ContextWalkFile, Vec<u8>, ExtractedFile)],
        vanished: &[String],
    ) -> Result<Vec<(String, String)>, ContextError> {
        let mut old_by_digest = HashMap::<String, Vec<String>>::new();
        for path in vanished {
            if let Some(file) = existing.get(path)
                && !file.digest.is_empty()
            {
                old_by_digest
                    .entry(file.digest.clone())
                    .or_default()
                    .push(path.clone());
            }
        }
        let mut new_by_digest = HashMap::<String, Vec<String>>::new();
        for (file, bytes, _) in touched {
            new_by_digest
                .entry(digest_of(bytes))
                .or_default()
                .push(file.path.clone());
        }
        let mut moves = Vec::new();
        for (digest, old) in old_by_digest {
            let Some(new) = new_by_digest.get(&digest) else {
                continue;
            };
            if old.len() == 1 && new.len() == 1 {
                moves.push((old[0].clone(), new[0].clone()));
            }
        }
        let old_symbols = self.store.with_connection(|connection| {
            let mut by_digest = HashMap::<String, Vec<String>>::new();
            let mut statement = connection.prepare(
                "SELECT path,digest FROM context_entities WHERE cwd=? AND kind!='file' AND path=?",
            )?;
            for path in vanished {
                for row in statement.query_map(
                    params![
                        existing
                            .values()
                            .next()
                            .map_or("", |file| file.cwd.as_str()),
                        path
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )? {
                    let (path, digest) = row?;
                    by_digest.entry(digest).or_default().push(path);
                }
            }
            Ok(by_digest)
        })?;
        let mut new_symbols = HashMap::<String, Vec<String>>::new();
        for (file, bytes, extracted) in touched {
            let symbols = extracted
                .symbols
                .iter()
                .map(|symbol| ContextSymbol {
                    line: symbol.line,
                    end_line: symbol.end_line,
                    kind: symbol.kind,
                    name: symbol.name.clone(),
                    params: symbol.params.clone(),
                    returns: symbol.returns.clone(),
                    exported: symbol.exported,
                    purpose: symbol.purpose.clone(),
                    comments: symbol.comments.clone(),
                    confirmed: false,
                })
                .collect::<Vec<_>>();
            for digest in symbol_digests(bytes, &symbols).into_values() {
                new_symbols
                    .entry(digest)
                    .or_default()
                    .push(file.path.clone());
            }
        }
        for (digest, old) in old_symbols {
            let Some(new) = new_symbols.get(&digest) else {
                continue;
            };
            if old.len() == 1 && new.len() == 1 && !moves.iter().any(|(from, _)| from == &old[0]) {
                moves.push((old[0].clone(), new[0].clone()));
            }
        }
        Ok(moves)
    }

    fn recompute_importance(&self, cwd: &Path) -> Result<(), ContextError> {
        let files = self.list_files(cwd)?.into_values().collect::<Vec<_>>();
        if files.is_empty() {
            return Ok(());
        }
        let index = files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.path.clone(), index))
            .collect::<HashMap<_, _>>();
        let mut definitions = HashMap::<String, Vec<usize>>::new();
        for (index, file) in files.iter().enumerate() {
            for symbol in &file.symbols {
                definitions
                    .entry(symbol.name.clone())
                    .or_default()
                    .push(index);
            }
        }
        let mut links = vec![Vec::<(usize, f64)>::new(); files.len()];
        for (from, file) in files.iter().enumerate() {
            let mut targets = HashMap::<usize, f64>::new();
            for imported in &file.refs.imports {
                if let Some(imported_path) = resolve_import(&file.path, imported, &index)
                    && let Some(to) = index.get(&imported_path)
                    && *to != from
                {
                    targets.insert(*to, 1.0);
                }
            }
            for call in &file.refs.calls {
                if let Some(definers) = definitions.get(call)
                    && definers.len() == 1
                    && definers[0] != from
                {
                    let weight = if call.starts_with('_') { 0.1 } else { 1.0 };
                    targets
                        .entry(definers[0])
                        .and_modify(|value| *value = value.max(weight))
                        .or_insert(weight);
                }
            }
            links[from] = targets.into_iter().collect();
        }
        let mut ranks = vec![1.0 / files.len() as f64; files.len()];
        for _ in 0..20 {
            let dangling = ranks
                .iter()
                .enumerate()
                .filter(|(index, _)| links[*index].is_empty())
                .map(|(_, rank)| *rank)
                .sum::<f64>();
            let mut next =
                vec![0.15 / files.len() as f64 + 0.85 * dangling / files.len() as f64; files.len()];
            for (from, outgoing) in links.iter().enumerate() {
                if outgoing.is_empty() {
                    continue;
                }
                let total = outgoing.iter().map(|(_, weight)| *weight).sum::<f64>();
                for (to, weight) in outgoing {
                    next[*to] += 0.85 * ranks[from] * weight / total;
                }
            }
            ranks = next;
        }
        self.store.transaction(|transaction| {
            for (index, file) in files.iter().enumerate() {
                transaction.execute(
                    "UPDATE context_entities SET importance=? WHERE cwd=? AND path=? AND kind='file'",
                    params![ranks[index], cwd.display().to_string(), file.path],
                )?;
            }
            Ok(())
        })?;
        Ok(())
    }

    fn learned_route_scores(
        &self,
        cwd: &Path,
        question: &str,
        terms: &[String],
    ) -> Result<Vec<LearnedRoute>, ContextError> {
        let exact_aliases = hint_key(&[question.to_owned()]);
        if exact_aliases.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self.store.with_connection(|connection| {
            let mut routes = Vec::new();
            let mut exact_statement = connection.prepare(&format!(
                "{LIVE_ROUTE_SELECT} WHERE r.cwd=? AND r.aliases=? ORDER BY r.last_confirmed_at DESC LIMIT 24"
            ))?;
            let exact_rows = exact_statement.query_map(
                params![cwd.display().to_string(), exact_aliases],
                |row| live_route_from_row(row, true),
            )?;
            for row in exact_rows {
                routes.push(row?);
            }
            let query = fts_query(terms);
            if !query.is_empty() {
                let mut searched_statement = connection.prepare(&format!(
                    "{LIVE_ROUTE_SELECT} JOIN context_learned_routes_fts f ON f.rowid=r.id WHERE r.cwd=? AND context_learned_routes_fts MATCH ? ORDER BY r.last_confirmed_at DESC LIMIT 48"
                ))?;
                let searched_rows = searched_statement.query_map(
                    params![cwd.display().to_string(), query],
                    |row| live_route_from_row(row, false),
                )?;
                for row in searched_rows {
                    let route = row?;
                    if !routes.iter().any(|existing: &LearnedRoute| {
                        existing.path == route.path && existing.symbol == route.symbol
                    }) {
                        routes.push(route);
                    }
                }
            }
            Ok(routes)
        })?)
    }

    fn fts_ranks(
        &self,
        cwd: &Path,
        terms: &[String],
    ) -> Result<HashMap<String, f64>, ContextError> {
        let query = fts_query(terms);
        if query.is_empty() {
            return Ok(HashMap::new());
        }
        Ok(self.store.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT e.path, MIN(bm25(context_entities_fts,12.0,6.0,8.0,4.0,3.0,1.0,10.0,1.0)) FROM context_entities_fts JOIN context_entities e ON e.id=context_entities_fts.rowid WHERE context_entities_fts MATCH ? AND e.cwd=? GROUP BY e.path",
            )?;
            let rows = statement
                .query_map(params![query, cwd.display().to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().map(|(path, rank)| (path, -rank)).collect())
        })?)
    }
}

#[derive(Debug, Clone)]
struct FileScore {
    total: f64,
    relevance: f64,
    strongest: f64,
    matched: HashSet<String>,
    symbol: Option<String>,
    exact_identifier: bool,
}

/// A learned route resolved to where its target lives now. The entity row is
/// authoritative when the route still points at one; the learned text is the
/// fallback for routes that never resolved to an entity.
#[derive(Debug, Clone)]
struct LearnedRoute {
    id: i64,
    path: String,
    symbol: Option<String>,
    line: Option<u64>,
    aliases: String,
    exact: bool,
}

const LIVE_ROUTE_SELECT: &str = "SELECT r.id, COALESCE(e.path, r.learned_path), COALESCE(CASE WHEN e.kind='file' THEN '' ELSE e.name END, r.learned_symbol), e.line, r.aliases FROM context_learned_routes r LEFT JOIN context_entities e ON e.id=r.entity_id";

fn live_route_from_row(row: &Row<'_>, exact: bool) -> rusqlite::Result<LearnedRoute> {
    Ok(LearnedRoute {
        id: row.get(0)?,
        path: row.get(1)?,
        symbol: non_empty(row.get(2)?),
        line: row.get::<_, Option<i64>>(3)?.map(|line| line as u64),
        aliases: row.get(4)?,
        exact,
    })
}

#[derive(Debug, Default)]
struct Corrections {
    paths: Vec<String>,
    file_purposes: HashMap<String, String>,
    symbol_purposes: HashMap<(String, String), String>,
}

fn score_file(file: &ContextFile, terms: &[String], fts_rank: Option<f64>) -> Option<FileScore> {
    let path_words = words(&file.path);
    let file_name = file.path.rsplit('/').next().unwrap_or(&file.path);
    let file_name = file_name
        .rsplit_once('.')
        .map_or(file_name, |(name, _)| name);
    let purpose_words = words(&format!(
        "{} {}",
        file.purpose.as_deref().unwrap_or_default(),
        file.header_comment.as_deref().unwrap_or_default()
    ));
    let refs_words = words(&format!(
        "{} {}",
        file.refs.imports.join(" "),
        file.refs.calls.join(" ")
    ));
    let mut matched = HashSet::new();
    let mut relevance = 0.0;
    let mut strongest: f64 = 0.0;
    let mut best_symbol = None;
    let mut best_symbol_score = 0.0;
    for term in terms {
        let mut best: f64 = 0.0;
        if contains_term(file_name, term) {
            best = best.max(240.0);
        } else if path_words.iter().any(|word| equivalent_match(word, term)) {
            best = best.max(120.0);
        }
        if purpose_words
            .iter()
            .any(|word| equivalent_match(word, term))
        {
            best = best.max(96.0);
        }
        if refs_words.iter().any(|word| equivalent_match(word, term)) {
            best = best.max(48.0);
        }
        for symbol in &file.symbols {
            let name = words(&symbol.name);
            let prose = words(&format!(
                "{} {} {}",
                symbol.purpose.as_deref().unwrap_or_default(),
                symbol.comments.join(" "),
                symbol.params.as_deref().unwrap_or_default()
            ));
            let name_match = name.iter().any(|word| equivalent_match(word, term));
            let prose_match = prose.iter().any(|word| equivalent_match(word, term));
            if name_match {
                best = best.max(300.0);
            } else if prose_match {
                best = best.max(144.0);
            }
            if name_match {
                let score = 300.0 + if symbol.exported { 60.0 } else { 0.0 };
                if score > best_symbol_score {
                    best_symbol_score = score;
                    best_symbol = Some(symbol.name.clone());
                }
            }
        }
        if best > 0.0 {
            matched.insert(term.clone());
            relevance += best;
            strongest = strongest.max(best);
        }
    }
    if matched.is_empty() {
        return None;
    }
    let exact_identifier = terms.len() == 1
        && file
            .symbols
            .iter()
            .any(|symbol| normalize_word(&symbol.name.to_ascii_lowercase()) == terms[0]);
    if exact_identifier {
        relevance += 500.0;
    }
    let category = category_weight(&file.path);
    let importance = file.importance * 50.0;
    let fts = fts_rank.unwrap_or_default().max(0.0);
    Some(FileScore {
        total: (relevance + importance + fts) * category,
        relevance: relevance * category,
        strongest,
        matched,
        symbol: best_symbol,
        exact_identifier,
    })
}

fn file_contains_term(file: &ContextFile, term: &str) -> bool {
    score_file(file, &[term.to_owned()], None).is_some()
}

fn contains_term(name: &str, term: &str) -> bool {
    normalize_word(&name.to_ascii_lowercase()) == term
        || words(name).iter().any(|word| equivalent_match(word, term))
}

fn equivalent_match(word: &str, term: &str) -> bool {
    word == term
        || match term {
            "create" | "make" | "new" | "add" | "init" | "initialize" => {
                ["create", "make", "new", "add", "init", "initializ"].contains(&word)
            }
            "remove" | "delete" | "drop" | "destroy" | "forget" => {
                ["remov", "delet", "drop", "destroy", "forget"].contains(&word)
            }
            "load" | "list" | "read" => ["load", "list", "read"].contains(&word),
            "render" | "view" | "display" => ["render", "view", "display"].contains(&word),
            "schedule" | "arm" | "park" | "queue" => {
                ["schedul", "arm", "park", "queu"].contains(&word)
            }
            "decision" | "choose" | "pick" | "select" => {
                ["decis", "choos", "pick", "select"].contains(&word)
            }
            "record" | "store" | "save" | "persist" | "write" => {
                ["record", "store", "save", "persist", "writ"].contains(&word)
            }
            "directory" | "folder" | "path" | "dir" | "cwd" => {
                ["directory", "folder", "path", "dir", "cwd"].contains(&word)
            }
            "route" | "router" | "routing" => ["route", "router"].contains(&word),
            "push" | "send" | "notify" | "notification" | "alert" => {
                ["push", "send", "notify", "notific", "alert"].contains(&word)
            }
            "inject" | "injector" | "injection" => ["inject", "injector", "inject"].contains(&word),
            "broker" | "server" => ["broker", "server"].contains(&word),
            "talk" | "communicate" | "connect" | "request" | "api" => {
                ["talk", "communicat", "connect", "request", "api", "server"].contains(&word)
            }
            "limit" | "bound" | "cap" | "truncate" => {
                ["limit", "bound", "cap", "truncat"].contains(&word)
            }
            _ => false,
        }
}

fn resolve_import(
    source_path: &str,
    imported: &str,
    files: &HashMap<String, usize>,
) -> Option<String> {
    let imported = imported.trim();
    if imported.is_empty() {
        return None;
    }
    let resolved = if imported.starts_with('.') {
        let parent = source_path
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        normalize_relative(parent, imported)
    } else {
        imported.trim_start_matches('/').replace('\\', "/")
    };
    if files.contains_key(&resolved) {
        return Some(resolved);
    }
    [
        "ts", "tsx", "js", "jsx", "mjs", "cjs", "swift", "py", "rs", "go",
    ]
    .iter()
    .map(|extension| format!("{resolved}.{extension}"))
    .find(|candidate| files.contains_key(candidate))
    .or_else(|| {
        ["ts", "tsx", "js", "jsx", "swift", "py", "rs", "go"]
            .iter()
            .map(|extension| format!("{resolved}/index.{extension}"))
            .find(|candidate| files.contains_key(candidate))
    })
}

fn normalize_relative(parent: &str, imported: &str) -> String {
    let mut parts = parent
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    for part in imported.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

fn category_weight(path: &str) -> f64 {
    let lower = path.to_ascii_lowercase();
    if lower.split('/').any(|part| {
        [
            "test",
            "tests",
            "fixture",
            "fixtures",
            "mock",
            "mocks",
            "snapshot",
            "vendor",
            "vendored",
            "generated",
            "dist",
            "build",
            "coverage",
        ]
        .contains(&part)
    }) || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.ends_with(".snap")
    {
        0.4
    } else {
        1.0
    }
}

fn reconcile_symbols(
    extracted: Vec<ExtractedSymbol>,
    stored: &[ContextSymbol],
) -> Vec<ContextSymbol> {
    let stored = stored
        .iter()
        .map(|symbol| (&symbol.name, symbol))
        .collect::<HashMap<_, _>>();
    extracted
        .into_iter()
        .map(|symbol| {
            let previous = stored.get(&symbol.name).copied();
            let signature_changed = previous.is_some_and(|previous| {
                previous.params != symbol.params || previous.returns != symbol.returns
            });
            ContextSymbol {
                line: symbol.line,
                end_line: symbol.end_line,
                kind: symbol.kind,
                name: symbol.name.clone(),
                params: symbol.params,
                returns: symbol.returns,
                exported: symbol.exported,
                purpose: previous
                    .and_then(|previous| previous.purpose.clone())
                    .or(symbol.purpose),
                comments: if symbol.comments.is_empty() {
                    previous.map_or_else(Vec::new, |previous| previous.comments.clone())
                } else {
                    symbol.comments
                },
                confirmed: previous
                    .is_some_and(|previous| previous.confirmed && !signature_changed),
            }
        })
        .collect()
}

fn symbol_digests(bytes: &[u8], symbols: &[ContextSymbol]) -> HashMap<String, String> {
    let source = String::from_utf8_lossy(bytes);
    let lines = source.lines().collect::<Vec<_>>();
    symbols
        .iter()
        .map(|symbol| {
            let start = symbol.line.saturating_sub(1) as usize;
            let end = symbol.end_line as usize;
            let declaration = lines.get(start..end).unwrap_or(&[]).join("\n");
            let body = declaration.replacen(&symbol.name, "<symbol>", 1);
            (symbol.name.clone(), digest_of(body.as_bytes()))
        })
        .collect()
}

const ROUTE_SYNONYM_GROUPS: &[&[&str]] = &[
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

fn route_aliases(hints: &[String]) -> String {
    let mut aliases = Vec::new();
    for hint in hints {
        for part in hint.split('|') {
            for word in raw_route_terms(part) {
                push_route_alias(&mut aliases, word);
            }
        }
    }
    let explicit = aliases.clone();
    for term in explicit {
        for group in ROUTE_SYNONYM_GROUPS {
            if group
                .iter()
                .any(|candidate| normalize_word(candidate) == term)
            {
                for synonym in *group {
                    for word in raw_route_terms(synonym) {
                        push_route_alias(&mut aliases, word);
                    }
                }
            }
        }
    }
    aliases.join(" ")
}

fn raw_route_terms(value: &str) -> Vec<String> {
    raw_words(value)
        .into_iter()
        .map(|word| normalize_word(&word))
        .filter(|word| word.len() > 2 && !MAP_STOP_WORDS.contains(&word.as_str()))
        .collect()
}

fn push_route_alias(aliases: &mut Vec<String>, alias: String) {
    if aliases.len() < MAX_ROUTE_ALIASES && !aliases.contains(&alias) {
        aliases.push(alias);
    }
}

fn merge_route_aliases(existing: &str, incoming: &str) -> String {
    let mut aliases = Vec::new();
    for alias in existing
        .split_whitespace()
        .chain(incoming.split_whitespace())
    {
        push_route_alias(&mut aliases, alias.to_owned());
    }
    aliases.join(" ")
}

struct RouteRecord<'a> {
    cwd: &'a str,
    entity_id: Option<i64>,
    path: &'a str,
    symbol: &'a str,
    aliases: &'a str,
    source_digest: &'a str,
    task_id: &'a str,
    attempt: i64,
    profile_id: &'a str,
    model: &'a str,
    now: &'a str,
}

fn save_route(
    transaction: &rusqlite::Transaction<'_>,
    route: RouteRecord<'_>,
) -> rusqlite::Result<()> {
    let existing = if let Some(entity_id) = route.entity_id {
        transaction
            .query_row(
                "SELECT id,aliases FROM context_learned_routes WHERE cwd=? AND entity_id=? LIMIT 1",
                params![route.cwd, entity_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
    } else {
        transaction
            .query_row(
                "SELECT id,aliases FROM context_learned_routes WHERE cwd=? AND learned_path=? AND learned_symbol=? LIMIT 1",
                params![route.cwd, route.path, route.symbol],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
    };
    if let Some((id, existing_aliases)) = existing {
        transaction.execute(
            "UPDATE context_learned_routes SET aliases=?,entity_id=?,learned_path=?,learned_symbol=?,source_digest=?,task_id=?,attempt=?,profile_id=?,model=?,last_confirmed_at=? WHERE id=?",
            params![merge_route_aliases(&existing_aliases, route.aliases), route.entity_id, route.path, route.symbol, route.source_digest, route.task_id, route.attempt, route.profile_id, route.model, route.now, id],
        )?;
    } else {
        transaction.execute(
            "INSERT INTO context_learned_routes(cwd,aliases,entity_id,learned_path,learned_symbol,source_digest,task_id,attempt,profile_id,model,created_at,last_confirmed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            params![route.cwd, route.aliases, route.entity_id, route.path, route.symbol, route.source_digest, route.task_id, route.attempt, route.profile_id, route.model, route.now, route.now],
        )?;
    }
    Ok(())
}

fn route_digest(path: &Path, symbol: Option<&str>) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let symbol = symbol?;
    let entry = mapped_extension(&path.to_string_lossy())?;
    let extracted = extract_symbols(&String::from_utf8_lossy(&bytes), entry.lang, entry.generic);
    let symbols = extracted
        .symbols
        .into_iter()
        .map(|symbol| ContextSymbol {
            line: symbol.line,
            end_line: symbol.end_line,
            kind: symbol.kind,
            name: symbol.name,
            params: symbol.params,
            returns: symbol.returns,
            exported: symbol.exported,
            purpose: symbol.purpose,
            comments: symbol.comments,
            confirmed: false,
        })
        .collect::<Vec<_>>();
    symbol_digests(&bytes, &symbols).remove(symbol)
}

fn read_indexable_file(cwd: &Path, file: &ContextWalkFile) -> Option<Vec<u8>> {
    if file.metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    fs::read(absolute_path(cwd, &file.path)).ok()
}

fn read_source_digest(path: &Path) -> Option<(String, HashSet<String>)> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let entry = mapped_extension(&path.to_string_lossy())?;
    let extracted = extract_symbols(&String::from_utf8_lossy(&bytes), entry.lang, entry.generic);
    Some((
        digest_of(&bytes),
        extracted
            .symbols
            .into_iter()
            .map(|symbol| symbol.name)
            .collect(),
    ))
}

fn read_context_file(
    cwd: &Path,
    path: &str,
    previous: Option<&ContextFile>,
) -> Result<Option<ContextFile>, ContextError> {
    let absolute = cwd.join(path);
    let metadata = match fs::metadata(&absolute) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_FILE_BYTES => metadata,
        _ => return Ok(None),
    };
    let Some(entry) = mapped_extension(path) else {
        return Ok(None);
    };
    let bytes = fs::read(&absolute).map_err(|source| ContextError::Io {
        path: absolute.clone(),
        source,
    })?;
    let extracted = extract_symbols(&String::from_utf8_lossy(&bytes), entry.lang, entry.generic);
    let walk = ContextWalkFile {
        path: path.to_owned(),
        entry,
        metadata,
    };
    let mut symbols = reconcile_symbols(
        extracted.symbols,
        previous.map_or(&[], |file| &file.symbols),
    );
    for symbol in &mut symbols {
        if symbol.comments.is_empty()
            && let Some(previous) =
                previous.and_then(|file| file.symbols.iter().find(|old| old.name == symbol.name))
        {
            symbol.comments = previous.comments.clone();
        }
    }
    let extracted = ExtractedFile {
        symbols: symbols
            .into_iter()
            .map(|symbol| ExtractedSymbol {
                line: symbol.line,
                end_line: symbol.end_line,
                kind: symbol.kind,
                name: symbol.name,
                params: symbol.params,
                returns: symbol.returns,
                exported: symbol.exported,
                purpose: symbol.purpose,
                comments: symbol.comments,
            })
            .collect(),
        unparsed: false,
        unparsed_reason: None,
        header_comment: previous.and_then(|file| file.header_comment.clone()),
    };
    let file = ContextFile {
        cwd: cwd.display().to_string(),
        path: path.to_owned(),
        lang: entry.lang,
        purpose: previous.and_then(|file| file.purpose.clone()),
        header_comment: extracted.header_comment,
        lines: line_count(&bytes) as u64,
        size: bytes.len() as u64,
        mtime_ms: mtime_ms(&walk.metadata) as f64,
        digest: digest_of(&bytes),
        symbols: extracted
            .symbols
            .into_iter()
            .map(|symbol| ContextSymbol {
                line: symbol.line,
                end_line: symbol.end_line,
                kind: symbol.kind,
                name: symbol.name,
                params: symbol.params,
                returns: symbol.returns,
                exported: symbol.exported,
                purpose: symbol.purpose,
                comments: symbol.comments,
                confirmed: false,
            })
            .collect(),
        status: MapFileStatus::Mapped,
        touch_count: previous.map_or(0, |file| file.touch_count),
        touched_at: previous.and_then(|file| file.touched_at.clone()),
        mapped_at: previous.map_or_else(timestamp_now, |file| file.mapped_at.clone()),
        updated_at: timestamp_now(),
        refs: extract_refs(&String::from_utf8_lossy(&bytes), entry.lang, entry.generic),
        importance: previous.map_or(0.0, |file| file.importance),
    };
    Ok(Some(file))
}

fn file_from_row(
    row: &Row<'_>,
    connection: &rusqlite::Connection,
) -> rusqlite::Result<ContextFile> {
    let id: i64 = row.get(0)?;
    let cwd: String = row.get(1)?;
    let path: String = row.get(2)?;
    let symbols = load_symbols(connection, id)?;
    Ok(ContextFile {
        cwd,
        path,
        lang: parse_lang(&row.get::<_, String>(3)?),
        purpose: row.get(4)?,
        header_comment: optional_text(row.get(14)?),
        lines: row.get::<_, i64>(5)?.max(0) as u64,
        size: row.get::<_, i64>(6)?.max(0) as u64,
        mtime_ms: row.get(7)?,
        digest: row.get(8)?,
        symbols,
        status: parse_status(&row.get::<_, String>(9)?),
        touch_count: row.get::<_, i64>(10)?.max(0) as u64,
        touched_at: row.get(11)?,
        mapped_at: row.get(12)?,
        updated_at: row.get(13)?,
        refs: parse_refs(&row.get::<_, String>(15)?),
        importance: row.get(16)?,
    })
}

fn load_symbols(
    connection: &rusqlite::Connection,
    parent_id: i64,
) -> rusqlite::Result<Vec<ContextSymbol>> {
    let mut statement = connection.prepare(
        "SELECT line,end_line,kind,name,purpose,params,returns,exported,comments_json,confirmed FROM context_entities WHERE parent_id=? ORDER BY line,id",
    )?;
    statement
        .query_map([parent_id], |row| {
            let comments =
                serde_json::from_str::<Vec<String>>(&row.get::<_, String>(8)?).unwrap_or_default();
            Ok(ContextSymbol {
                line: row.get::<_, i64>(0)?.max(0) as u64,
                end_line: row.get::<_, i64>(1)?.max(0) as u64,
                kind: parse_symbol_kind(&row.get::<_, String>(2)?),
                name: row.get(3)?,
                purpose: row.get(4)?,
                params: row.get(5)?,
                returns: row.get(6)?,
                exported: row.get::<_, Option<i64>>(7)?.unwrap_or_default() != 0,
                comments,
                confirmed: row.get::<_, Option<i64>>(9)?.unwrap_or_default() != 0,
            })
        })?
        .collect()
}

fn map_from_row(row: &Row<'_>) -> rusqlite::Result<ContextMapRow> {
    Ok(ContextMapRow {
        cwd: row.get(0)?,
        scheme: row.get::<_, i64>(1)?.max(0) as u32,
        state: parse_map_state(&row.get::<_, String>(2)?),
        built_at: row.get(3)?,
        file_count: row.get::<_, i64>(4)?.max(0) as u64,
        symbol_count: row.get::<_, i64>(5)?.max(0) as u64,
        pending_prose: row.get::<_, i64>(6)?.max(0) as u64,
        updated_at: row.get(7)?,
    })
}

fn parse_lang(value: &str) -> SourceLang {
    match value {
        "ts" => SourceLang::Ts,
        "swift" => SourceLang::Swift,
        _ => SourceLang::Generic,
    }
}

fn parse_status(value: &str) -> MapFileStatus {
    if value == "unparsed" {
        MapFileStatus::Unparsed
    } else {
        MapFileStatus::Mapped
    }
}

fn parse_map_state(value: &str) -> ContextMapState {
    match value {
        "building" => ContextMapState::Building,
        "partial" => ContextMapState::Partial,
        _ => ContextMapState::Ready,
    }
}

fn parse_symbol_kind(value: &str) -> SymbolKind {
    match value {
        "fn" => SymbolKind::Fn,
        "class" => SymbolKind::Class,
        "type" => SymbolKind::Type,
        "const" => SymbolKind::Const,
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "ext" => SymbolKind::Ext,
        "view" => SymbolKind::View,
        _ => SymbolKind::Type,
    }
}

fn parse_refs(value: &str) -> ContextRefs {
    serde_json::from_str(value).unwrap_or_default()
}

fn optional_text(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn map_state(state: ContextMapState) -> &'static str {
    match state {
        ContextMapState::Building => "building",
        ContextMapState::Ready => "ready",
        ContextMapState::Partial => "partial",
    }
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn lang_string(lang: SourceLang) -> &'static str {
    match lang {
        SourceLang::Ts => "ts",
        SourceLang::Swift => "swift",
        SourceLang::Generic => "generic",
    }
}

fn status_string(status: MapFileStatus) -> &'static str {
    match status {
        MapFileStatus::Mapped => "mapped",
        MapFileStatus::Unparsed => "unparsed",
    }
}

fn symbol_kind(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Fn => "fn",
        SymbolKind::Class => "class",
        SymbolKind::Type => "type",
        SymbolKind::Const => "const",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Ext => "ext",
        SymbolKind::View => "view",
    }
}

fn pending_prose(files: &HashMap<String, ContextFile>) -> u64 {
    files
        .values()
        .map(|file| {
            file.symbols
                .iter()
                .filter(|symbol| symbol.purpose.is_none())
                .count() as u64
        })
        .sum()
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

fn relative_path(cwd: &str, path: &str) -> String {
    let cwd = cwd.trim_end_matches('/');
    path.strip_prefix(&format!("{cwd}/"))
        .unwrap_or(path)
        .replace('\\', "/")
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
    let absolute = cwd.join(&relative);
    relative_inside(cwd, &absolute)
}

fn scope_covers_path(rules: &[String], cwd: &Path, target: &str) -> bool {
    let target = cwd.join(target);
    rules.iter().any(|rule| {
        let rule = rule.trim().replace('\\', "/");
        if rule == "**" {
            return relative_inside(cwd, &target).is_some();
        }
        let recursive = rule.ends_with("/**");
        let base_rule = rule.trim_end_matches("/**").trim_end_matches('/');
        let base = cwd.join(base_rule);
        if recursive || base.is_dir() {
            target == base || relative_inside(&base, &target).is_some()
        } else {
            target == base
        }
    })
}

fn path_prefix(prefix: &str, path: &str) -> bool {
    let prefix = prefix.trim().trim_matches('/');
    prefix.is_empty() || prefix == "." || path == prefix || path.starts_with(&format!("{prefix}/"))
}

fn depth_allowed(cwd: &Path, scope: &str, path: &str, depth: Option<usize>) -> bool {
    let Some(depth) = depth else { return true };
    let base = cwd.join(if scope.is_empty() || scope == "." {
        ""
    } else {
        scope
    });
    let target = cwd.join(path);
    let Some(relative) = relative_inside(&base, &target) else {
        return false;
    };
    relative.split('/').count().saturating_sub(1) <= depth
}

fn render_file(file: &ContextFile, tier: RenderTier) -> String {
    if tier == RenderTier::Index {
        return file.path.clone();
    }
    let purpose = note_for(file.purpose.as_deref(), &file.path, None);
    let unavailable = (file.status == MapFileStatus::Unparsed
        && !file
            .purpose
            .as_deref()
            .unwrap_or_default()
            .starts_with("Symbols unavailable:"))
    .then_some("symbols unavailable".to_owned());
    if tier == RenderTier::Skeleton {
        let count = file
            .symbols
            .iter()
            .filter(|symbol| informative_symbol(symbol))
            .count();
        return entry_line(
            &file.path,
            None,
            [
                purpose,
                (count > 0).then(|| {
                    format!(
                        "{count} symbol{} not listed",
                        if count == 1 { "" } else { "s" }
                    )
                }),
                unavailable,
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
            None,
        );
    }
    let symbols = file
        .symbols
        .iter()
        .filter(|symbol| informative_symbol(symbol))
        .map(|symbol| {
            entry_line(
                &file.path,
                Some(&symbol.name),
                note_for(symbol.purpose.as_deref(), &file.path, Some(&symbol.name))
                    .into_iter()
                    .collect(),
                Some(symbol.line),
            )
        })
        .collect::<Vec<_>>();
    let header = entry_line(
        &file.path,
        None,
        [purpose, unavailable].into_iter().flatten().collect(),
        None,
    );
    if symbols.is_empty() || header != file.path {
        std::iter::once(header)
            .chain(symbols)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        symbols.join("\n")
    }
}

fn informative_symbol(symbol: &ContextSymbol) -> bool {
    symbol.kind != SymbolKind::Const
        || symbol.purpose.is_some()
        || symbol.params.is_some()
        || symbol.returns.is_some()
}

fn note_for(purpose: Option<&str>, path: &str, symbol: Option<&str>) -> Option<String> {
    let purpose = purpose?.trim();
    if purpose.is_empty() {
        return None;
    }
    let phrase = purpose
        .split_once('.')
        .map_or(purpose, |(first, _)| first)
        .trim()
        .trim_end_matches([' ', '*', '/', '.', ',', ';', ':', '—', '-'])
        .to_owned();
    let phrase = if phrase.chars().count() > 60 {
        let truncated = phrase.chars().take(60).collect::<String>();
        if let Some((head, _)) = truncated.rsplit_once(' ') {
            format!("{head}…")
        } else {
            truncated
        }
    } else {
        phrase
    };
    let own = words(&format!("{path} {}", symbol.unwrap_or_default()));
    let words_in_phrase = words(&phrase);
    let restates = !words_in_phrase.is_empty()
        && words_in_phrase.iter().all(|word| {
            word.len() < 3 || MAP_STOP_WORDS.contains(&word.as_str()) || own.contains(word)
        });
    (!restates).then_some(phrase)
}

fn entry_line(path: &str, symbol: Option<&str>, notes: Vec<String>, line: Option<u64>) -> String {
    let mut entry = path.to_owned();
    if let Some(line) = line {
        entry.push_str(&format!(":{line}"));
    }
    if let Some(symbol) = symbol {
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

fn map_header(
    cwd: &Path,
    map: Option<&ContextMapRow>,
    file_count: usize,
    symbol_count: usize,
) -> String {
    format!(
        "# Context map — {}\nscheme {} · {} files · {} symbols · state {} · generated {}",
        cwd.display(),
        MAP_SCHEME,
        file_count,
        symbol_count,
        map.map_or("unknown", |map| map_state(map.state)),
        map.and_then(|map| map.built_at.as_deref())
            .unwrap_or("unknown"),
    )
}

fn write_targets(payload: &BTreeMap<String, Value>) -> Vec<String> {
    let mut paths = Vec::new();
    for (key, value) in payload {
        if ["file_path", "filePath", "path"].contains(&key.as_str())
            && let Some(path) = value.as_str()
        {
            paths.push(path.to_owned());
        }
        collect_write_targets(value, &mut paths);
    }
    paths
}

fn collect_write_targets(value: &Value, paths: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if ["file_path", "filePath", "path"].contains(&key.as_str())
                    && let Some(path) = value.as_str()
                {
                    paths.push(path.to_owned());
                }
                collect_write_targets(value, paths);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_write_targets(value, paths);
            }
        }
        _ => {}
    }
}

fn parse_corrections(output: &str, cwd: &Path) -> Corrections {
    let mut section = false;
    let mut corrections = Corrections::default();
    for line in output.lines().map(str::trim) {
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading == "Map corrections";
            continue;
        }
        if !section || corrections.paths.len() >= 20 {
            continue;
        }
        let Some((left, purpose)) = line.split_once('—') else {
            continue;
        };
        let purpose = purpose.trim();
        if purpose.is_empty() {
            continue;
        }
        let (raw_path, symbol) = left
            .trim()
            .rsplit_once(':')
            .filter(|(_, symbol)| is_symbol_name(symbol.trim()))
            .map_or((left.trim(), None), |(path, symbol)| {
                (path.trim(), Some(symbol.trim().to_owned()))
            });
        let Some(path) = safe_relative(cwd, raw_path) else {
            continue;
        };
        if mapped_extension(&path).is_none() {
            continue;
        }
        if !corrections.paths.contains(&path) {
            corrections.paths.push(path.clone());
        }
        let purpose = purpose.chars().take(200).collect::<String>();
        if let Some(symbol) = symbol {
            corrections
                .symbol_purposes
                .insert((path, symbol), purpose.chars().take(100).collect());
        } else {
            corrections
                .file_purposes
                .insert(path, purpose.chars().take(100).collect());
        }
    }
    corrections
}

fn is_symbol_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().enumerate().all(|(index, char)| {
            char.is_ascii_alphanumeric()
                || char == '_'
                || char == '$'
                || (index > 0 && matches!(char, '?' | '!'))
        })
}

fn digest_of(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .chars()
        .take(7)
        .collect()
}

fn timestamp_now() -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
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
