//! Context-map and plain-language query routes.
//!
//! Every lookup reconciles the map against disk first, so files that changed,
//! moved, or vanished since the last call are re-read before answering.

use std::{path::Path, process::Command, time::Instant};

use axum::{
    Json,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use oga_context::{
    BuildOptions, ContextError, ContextIndex, ContextResult, ContextTarget, QueryOptions,
    RenderTier,
};
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
    pub q: Option<String>,
    pub limit: Option<u64>,
    pub code: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct MapParams {
    pub task: Option<String>,
    #[serde(default)]
    pub path: Vec<String>,
    #[serde(default)]
    pub symbol: Vec<String>,
    pub depth: Option<u64>,
    pub q: Option<String>,
    pub tier: Option<String>,
    pub limit: Option<u64>,
    pub code: Option<bool>,
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
    let cwd = require_directory(query.cwd.as_deref())?;
    let question = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|question| !question.is_empty())
        .ok_or_else(|| HttpError::bad_request("usage: oga query \"<question>\""))?
        .to_owned();
    let store = state.store.clone();
    let options = oga_context::QuestionOptions {
        limit: query.limit.map(|limit| limit as usize),
        code: query.code.unwrap_or(false),
    };
    let result = run_blocking(move || {
        let index = ContextIndex::new(&store);
        let target = match origin_of_worktree(&store, &cwd)? {
            Some(origin) => {
                refresh(&index, &origin)?;
                ContextTarget::worktree(&cwd, &origin, everything())
            }
            None => {
                refresh(&index, &cwd)?;
                ContextTarget::new(&cwd, everything())
            }
        };
        Ok(index.question_with_options(&target, &question, options)?)
    })
    .await?;
    Ok(Json(json!({
        "markdown": result.markdown,
        "candidates": result.candidates,
    })))
}

pub async fn get_map(
    State(state): State<HttpState>,
    Query(query): Query<MapParams>,
    headers: HeaderMap,
) -> Result<Response, HttpError> {
    let task_id = query
        .task
        .as_deref()
        .ok_or_else(|| HttpError::bad_request("task is required"))?;
    let task = state::load_task(&state.store, task_id)?
        .filter(|task| task.kind != Some(TaskKind::Orchestrator) && task.archived_at.is_none())
        .ok_or_else(|| HttpError::not_found("unknown task"))?;
    let question = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    if query.path.is_empty()
        && query.symbol.is_empty()
        && query.depth.is_none()
        && question.is_none()
    {
        return Err(HttpError::bad_request("path, symbol, or q is required"));
    }
    let target = task.worktree.as_ref().map_or_else(
        || ContextTarget::new(&task.cwd, task.scope.clone()),
        |worktree| ContextTarget::worktree(&task.cwd, &worktree.origin_cwd, task.scope.clone()),
    );
    let question = question.map(str::to_owned);
    let list_options = QueryOptions {
        paths: query.path.clone(),
        symbols: query.symbol.clone(),
        tier: query.tier.as_deref().map(parse_tier).transpose()?,
        depth: query.depth.map(|depth| depth as usize),
    };
    let question_options = oga_context::QuestionOptions {
        limit: query.limit.map(|limit| limit as usize),
        code: query.code.unwrap_or(false),
    };
    let store = state.store.clone();
    let result = run_blocking(move || {
        let index = ContextIndex::new(&store);
        let map_cwd = target
            .source_cwd
            .as_deref()
            .unwrap_or(&target.cwd)
            .display()
            .to_string();
        refresh(&index, &map_cwd)?;
        Ok(match question {
            Some(question) => index.question_with_options(&target, &question, question_options)?,
            None => index.list(&target, &list_options)?,
        })
    })
    .await?;
    if headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("application/json"))
    {
        return Ok(Json(map_json(&result)).into_response());
    }
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(result.markdown))
        .expect("valid text response"))
}

pub async fn init_map(
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

/// Bring the map for `cwd` up to date with disk before answering from it.
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

fn parse_tier(tier: &str) -> Result<RenderTier, HttpError> {
    match tier {
        "full" => Ok(RenderTier::Full),
        "skeleton" => Ok(RenderTier::Skeleton),
        "index" => Ok(RenderTier::Index),
        other => Err(HttpError::bad_request(format!(
            "tier must be full, skeleton, or index; got {other}"
        ))),
    }
}

fn map_json(result: &ContextResult) -> serde_json::Value {
    json!({
        "markdown": result.markdown,
        "files": result.files,
        "candidates": result.candidates,
        "omitted": {
            "outsideScope": result.outside_scope,
            "gone": result.gone,
        },
    })
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
