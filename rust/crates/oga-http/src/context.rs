//! Plain-language code lookup reconciled against each project's index.

use std::{
    collections::HashMap,
    path::Path,
    process::Command,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant, SystemTime},
};

use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use oga_context::{
    BuildOptions, ContextError, ContextIndex, ContextResult, ContextTarget, QuestionOptions,
};
use oga_domain::{TaskKind, TaskScope};
use oga_store::{Store, StoreError};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::json;

use crate::{
    router::{HttpError, HttpState, run_blocking},
    state,
};

#[derive(Debug, Deserialize, Default)]
pub struct QueryParams {
    pub cwd: Option<String>,
    pub task: Option<String>,
    pub q: Option<String>,
    pub limit: Option<u64>,
    pub code: Option<bool>,
    #[serde(rename = "in")]
    pub in_paths: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct InitParams {
    pub cwd: Option<String>,
    pub force: Option<bool>,
}

pub async fn get_query(
    State(state): State<HttpState>,
    Query(query): Query<QueryParams>,
) -> Result<impl IntoResponse, HttpError> {
    let question = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|question| !question.is_empty())
        .ok_or_else(|| HttpError::bad_request("usage: oga query \"<question>\""))?
        .to_owned();
    let target = match query.task.as_deref() {
        Some(task_id) => {
            let task = state::load_task(&state.store, task_id)?
                .filter(|task| {
                    task.kind != Some(TaskKind::Orchestrator) && task.archived_at.is_none()
                })
                .ok_or_else(|| HttpError::not_found("unknown task"))?;
            task.worktree.as_ref().map_or_else(
                || ContextTarget::new(&task.cwd, task.scope.clone()),
                |worktree| {
                    ContextTarget::worktree(&task.cwd, &worktree.origin_cwd, task.scope.clone())
                },
            )
        }
        None => target_for(&state.store, &require_directory(query.cwd.as_deref())?)?,
    };
    let store = state.store.clone();
    let gate = state.reconcile_debounce.clone();
    let options = QuestionOptions {
        limit: query.limit.map(|limit| limit as usize),
        code: query.code,
        paths: split_paths(query.in_paths.as_deref()),
    };
    let result = run_blocking(move || {
        let cwd = target.cwd.display().to_string();
        answer(&gate, &store, &target, &question, options)?.ok_or_else(|| {
            HttpError::conflict(format!(
                "{cwd} is not indexed; run 'oga query --init' there to index it"
            ))
        })
    })
    .await?;
    Ok(Json(json!({ "markdown": result.markdown })))
}

pub async fn init_index(
    State(state): State<HttpState>,
    Query(query): Query<InitParams>,
) -> Result<impl IntoResponse, HttpError> {
    let cwd = require_directory(query.cwd.as_deref())?;
    let force = query.force.unwrap_or(false);
    let store = state.store.clone();
    let gate = state.reconcile_debounce.clone();
    let body = run_blocking(move || {
        let is_repository = Command::new("git")
            .args(["-C", &cwd, "rev-parse", "--show-toplevel"])
            .output()
            .is_ok_and(|output| output.status.success());
        if !is_repository {
            return Err(HttpError::bad_request(format!(
                "{cwd} is not a project repository; enter a git repository, then run 'oga query --init'"
            )));
        }
        let index = ContextIndex::new(&store);
        let started = Instant::now();
        let checkout = oga_context::checkout_stamp(Path::new(&cwd));
        let project = gate.project(&cwd);
        let mut walked = project.write().expect("reconcile gate poisoned");
        let (file_count, symbol_count, partial, changed) = if force {
            let built = index.build(&cwd, BuildOptions::default())?;
            (built.file_count, built.symbol_count, built.partial, true)
        } else {
            let reconciled = index.reconcile(&cwd, BuildOptions::default())?;
            (
                reconciled.file_count,
                reconciled.symbol_count,
                reconciled.partial,
                reconciled.changed,
            )
        };
        *walked = Some(Walked {
            began: started,
            checkout,
        });
        drop(walked);
        if file_count == 0 {
            return Err(HttpError::bad_request(format!(
                "no indexable files found in {cwd}; add source files, then run 'oga query --init'"
            )));
        }
        Ok(json!({
            "fileCount": file_count,
            "symbolCount": symbol_count,
            "partial": partial,
            "changed": changed,
            "elapsedMs": started.elapsed().as_millis() as u64,
        }))
    })
    .await?;
    Ok(Json(body))
}

/// Where a lookup in `cwd` runs.
///
/// The answer comes from that checkout's own index, so it reflects the branch
/// and the edits in front of whoever asked. A task worktree also names the
/// origin it was cut from, which is where that project's learned routes live.
pub fn target_for(store: &Store, cwd: &str) -> Result<ContextTarget, StoreError> {
    let origin = store.with_connection(|connection| {
        Ok(connection
            .query_row(
                "SELECT origin_cwd FROM tasks \
                 WHERE rtrim(worktree_path, '/')=? AND origin_cwd IS NOT NULL LIMIT 1",
                [cwd],
                |row| row.get::<_, String>(0),
            )
            .optional()?)
    })?;
    Ok(match origin {
        Some(origin) => ContextTarget::worktree(cwd, origin, everything()),
        None => ContextTarget::new(cwd, everything()),
    })
}

/// A question this soon after a walk answers from that walk, unless git has
/// rewritten the checkout since. An edit to a file the answer names is still
/// read from disk.
const RECENT_WALK: Duration = Duration::from_secs(2);

/// One entry per project, holding when that project's tree was last walked.
///
/// A reconcile takes the entry's write lock, so only one walks a project at a
/// time and no answer is assembled while one commits. Answers take the read
/// lock and so never wait on each other.
#[derive(Clone, Default)]
pub struct ReconcileDebounce(Arc<Mutex<HashMap<String, Project>>>);

/// When one project's tree was last walked, and the lock that says who may
/// walk it or read from it.
type Project = Arc<RwLock<Option<Walked>>>;

#[derive(Clone, Copy)]
struct Walked {
    began: Instant,
    /// The checkout's git stamp, read before the walk began.
    checkout: Option<SystemTime>,
}

impl ReconcileDebounce {
    fn project(&self, cwd: &str) -> Project {
        self.0
            .lock()
            .expect("reconcile gate poisoned")
            .entry(cwd.to_owned())
            .or_default()
            .clone()
    }
}

/// Bring a checkout's index up to date and answer one question from it, or
/// `None` when nothing there is indexed.
///
/// The walk is skipped when another question's walk *began* after this one
/// was asked, or began within [`RECENT_WALK`] with git having moved nothing
/// since. The answer is then assembled under the read lock, so it reads one
/// committed index state rather than straddling a reconcile.
pub fn answer(
    gate: &ReconcileDebounce,
    store: &Store,
    target: &ContextTarget,
    question: &str,
    options: QuestionOptions,
) -> Result<Option<ContextResult>, ContextError> {
    let asked = Instant::now();
    let cwd = target.cwd.display().to_string();
    let index = ContextIndex::new(store);
    let project = gate.project(&cwd);
    {
        let checkout = oga_context::checkout_stamp(&target.cwd);
        let mut walked = project.write().expect("reconcile gate poisoned");
        let current = walked.is_some_and(|walked| {
            walked.began >= asked
                || (walked.began.elapsed() < RECENT_WALK && walked.checkout == checkout)
        });
        if !current {
            let began = Instant::now();
            if index.reconcile(&cwd, BuildOptions::default())?.file_count == 0 {
                return Ok(None);
            }
            *walked = Some(Walked { began, checkout });
        }
    }
    let _reading = project.read().expect("reconcile gate poisoned");
    Ok(Some(
        index.question_with_options(target, question, options)?,
    ))
}

fn split_paths(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

fn everything() -> TaskScope {
    TaskScope {
        read: vec!["**".into()],
        write: Vec::new(),
    }
}

fn require_directory(cwd: Option<&str>) -> Result<String, HttpError> {
    let cwd = cwd.ok_or_else(|| {
        HttpError::bad_request("cannot index : choose a directory, then run 'oga query --init'")
    })?;
    let path = Path::new(cwd);
    if !path.is_absolute() || !path.is_dir() {
        return Err(HttpError::bad_request(format!(
            "cannot index {cwd}: choose a directory, then run 'oga query --init'"
        )));
    }
    Ok(oga_config::canonical_cwd(cwd).display().to_string())
}

impl From<ContextError> for HttpError {
    fn from(error: ContextError) -> Self {
        match error {
            ContextError::Invalid(message) => HttpError::bad_request(message),
            other => HttpError::internal(other.to_string()),
        }
    }
}
