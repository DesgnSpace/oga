//! Plain-language code lookup, answered from the project's own index.
//!
//! Every lookup reconciles the index against disk first, so files that changed,
//! moved, or vanished since the last call are re-read before answering.

use std::{path::Path, process::Command, time::Instant};

use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use oga_context::{BuildOptions, ContextError, ContextIndex, ContextTarget};
use oga_domain::{TaskKind, TaskScope};
use oga_store::Store;
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
}

#[derive(Debug, Deserialize, Default)]
pub struct InitParams {
    pub cwd: Option<String>,
    pub force: Option<bool>,
}

/// Answer a question about a project's code.
///
/// A `task` names the checkout the answer must stay inside: the ranking runs
/// against the origin project's index, but only what that task may read comes
/// back.
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
        None => {
            let cwd = require_directory(query.cwd.as_deref())?;
            match origin_of_worktree(&state.store, &cwd)? {
                Some(origin) => ContextTarget::worktree(&cwd, &origin, everything()),
                None => ContextTarget::new(&cwd, everything()),
            }
        }
    };
    let store = state.store.clone();
    let options = oga_context::QuestionOptions {
        limit: query.limit.map(|limit| limit as usize),
        code: query.code.unwrap_or(false),
    };
    let result = run_blocking(move || {
        let index = ContextIndex::new(&store);
        let index_cwd = target
            .source_cwd
            .as_deref()
            .unwrap_or(&target.cwd)
            .display()
            .to_string();
        refresh(&index, &index_cwd)?;
        Ok(index.question_with_options(&target, &question, options)?)
    })
    .await?;
    Ok(Json(json!({
        "markdown": result.markdown,
        "candidates": result.candidates,
    })))
}

pub async fn init_index(
    State(state): State<HttpState>,
    Query(query): Query<InitParams>,
) -> Result<impl IntoResponse, HttpError> {
    let cwd = require_directory(query.cwd.as_deref())?;
    let force = query.force.unwrap_or(false);
    let store = state.store.clone();
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

/// The origin project a task worktree was cut from, when `cwd` is one.
///
/// A worktree shares the origin's code, so a lookup run inside one belongs on
/// the origin's index instead of building a second index per checkout.
fn origin_of_worktree(store: &Store, cwd: &str) -> Result<Option<String>, HttpError> {
    store
        .with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT origin_cwd FROM tasks \
                     WHERE rtrim(worktree_path, '/')=? AND origin_cwd IS NOT NULL LIMIT 1",
                    [cwd],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })
        .map_err(HttpError::from)
}

/// Bring the index for `cwd` up to date with disk before answering from it.
fn refresh(index: &ContextIndex<'_>, cwd: &str) -> Result<(), HttpError> {
    let reconciled = index.reconcile(cwd, BuildOptions::default())?;
    if reconciled.file_count == 0 {
        return Err(HttpError::conflict(format!(
            "{cwd} is not indexed; run 'oga query --init' there to index it"
        )));
    }
    Ok(())
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
