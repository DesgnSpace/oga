//! State polling and per-task read routes.

use std::{collections::BTreeMap, path::Path, time::Duration};

use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
    response::IntoResponse,
};
use oga_domain::{
    ActivityCounts, ArchivedFilter, FailureCode, MemoryProject, ProfileFailure, ProfileView,
    ScopeGrant, SpendTotals, Task, TaskCompletion, TaskEvent, TaskEventView, TaskHoldView,
    TaskKind, TaskState, TaskSummary, TaskWorktree,
};
use oga_events::{event_view, mark_repeated_retries};
use oga_store::Store;
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::router::{HttpError, HttpState};

const TASK_COLUMNS: &str = "id,kind,profile_id,model,prompt,shipped_prompt,cwd,branch,origin_cwd,worktree_path,worktree_branch,worktree_links_json,state,output,error,question,parent_task_id,orchestrator_id,caller_id,scope_json,grant_id,allow_questions,timeout_ms,effort,effort_actual,tldr,title,session_id,completion_json,attempts_json,cost_usd,cost_usd_estimated,turns,archived_at,created_at,updated_at,can_delegate";

#[derive(Debug, Deserialize, Default)]
pub struct StateQuery {
    pub view: Option<String>,
    pub compact: Option<String>,
    pub archived: Option<String>,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EventQuery {
    pub after: Option<i64>,
    #[serde(rename = "waitMs")]
    pub wait_ms: Option<u64>,
    pub last: Option<u64>,
    pub before: Option<i64>,
    pub limit: Option<u64>,
}

pub async fn get_state(
    State(state): State<HttpState>,
    Query(query): Query<StateQuery>,
) -> Result<impl IntoResponse, HttpError> {
    let store = state.store.clone();
    let body = run_read(move || {
        let summary = query.view.as_deref() == Some("summary");
        let compact = query.compact.as_deref() == Some("1");
        let archived = archived_filter(query.archived.as_deref());
        let limit = query.limit.unwrap_or(50).clamp(1, 2_000);
        let profiles = public_profiles(&store)?;
        let (tasks, tasks_has_more) = list_tasks(&store, archived, summary, limit)?;
        let memory_projects = list_memory_projects(&store)?;
        let spend = spend_totals(&store)?;

        let mut body = serde_json::Map::new();
        body.insert("profiles".into(), serde_json::to_value(profiles).unwrap());
        body.insert("tasks".into(), tasks);
        if summary {
            body.insert("tasksHasMore".into(), json!(tasks_has_more));
        }
        if !compact {
            body.insert(
                "profileFailures".into(),
                serde_json::to_value(list_profile_failures(&store)?).unwrap(),
            );
            body.insert(
                "grants".into(),
                serde_json::to_value(list_grants(&store)?).unwrap(),
            );
        }
        body.insert(
            "memoryProjects".into(),
            serde_json::to_value(memory_projects).unwrap(),
        );
        body.insert("spend".into(), serde_json::to_value(spend).unwrap());
        Ok(Value::Object(body))
    })
    .await?;
    Ok(Json(body))
}

pub async fn get_task(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
) -> Result<impl IntoResponse, HttpError> {
    let store = state.store.clone();
    let task = run_read(move || {
        load_task(&store, &id)?
            .filter(|task| task.kind != Some(TaskKind::Orchestrator))
            .ok_or_else(|| HttpError::not_found("unknown task"))
    })
    .await?;
    Ok(Json(serde_json::to_value(task).unwrap()))
}

pub async fn get_task_turns(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
) -> Result<impl IntoResponse, HttpError> {
    let task = load_task(&state.store, &id)?
        .filter(|task| task.kind != Some(TaskKind::Orchestrator))
        .ok_or_else(|| HttpError::not_found("unknown task"))?;
    let turns = state.store.repositories().turns().list(&task.id)?;
    Ok(Json(json!({ "turns": turns })))
}

/// The task's checkout as git sees it, rather than as its worker described it.
pub async fn get_task_diff(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
) -> Result<impl IntoResponse, HttpError> {
    let task = load_task(&state.store, &id)?
        .filter(|task| task.kind != Some(TaskKind::Orchestrator))
        .ok_or_else(|| HttpError::not_found("unknown task"))?;
    let diff = oga_worktree::task_diff(&task)
        .await
        .map_err(|error| HttpError::conflict(error.to_string()))?;
    Ok(Json(serde_json::to_value(diff).unwrap()))
}

pub async fn get_task_events(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<EventQuery>,
) -> Result<Json<Value>, HttpError> {
    let store = state.store.clone();
    let task = run_read(move || {
        load_task(&store, &id)?
            .filter(|task| task.kind != Some(TaskKind::Orchestrator))
            .ok_or_else(|| HttpError::not_found("unknown task"))
    })
    .await?;
    event_response(&state, &task, &query).await
}

pub async fn mark_task_viewed(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
) -> Result<impl IntoResponse, HttpError> {
    if load_task(&state.store, &id)?.is_none() {
        return Err(HttpError::not_found("unknown task"));
    }
    state.store.set_viewed_task(id);
    Ok(Json(json!({ "ok": true })))
}

pub async fn unmark_task_viewed(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
) -> Result<impl IntoResponse, HttpError> {
    state.store.clear_viewed_task(&id);
    Ok(Json(json!({ "ok": true })))
}

pub async fn get_agent_turns(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
) -> Result<impl IntoResponse, HttpError> {
    let task = load_orchestrator(&state.store, &id)?;
    let turns = state.store.repositories().turns().list(&task.id)?;
    Ok(Json(json!({ "turns": turns })))
}

pub async fn get_agent_events(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<EventQuery>,
) -> Result<Json<Value>, HttpError> {
    let task = load_orchestrator(&state.store, &id)?;
    event_response(&state, &task, &query).await
}

pub(crate) fn load_orchestrator(store: &Store, id: &str) -> Result<Task, HttpError> {
    load_task(store, id)?
        .filter(|task| task.kind == Some(TaskKind::Orchestrator))
        .ok_or_else(|| HttpError::not_found("unknown orchestrator"))
}

/// A slow decode of a large page runs off the executor with a bound on its
/// wait, or one heavy task's history can stall the caller behind it forever.
const DB_READ_TIMEOUT: Duration = Duration::from_secs(10);

async fn run_read<T, F>(work: F) -> Result<T, HttpError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, HttpError> + Send + 'static,
{
    run_read_with_timeout(DB_READ_TIMEOUT, work).await
}

async fn run_read_with_timeout<T, F>(timeout: Duration, work: F) -> Result<T, HttpError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, HttpError> + Send + 'static,
{
    tokio::time::timeout(timeout, crate::router::run_blocking(work))
        .await
        .unwrap_or_else(|_| {
            Err(HttpError::timeout(
                "the broker did not answer in time; try again",
            ))
        })
}

async fn event_response(
    state: &HttpState,
    task: &Task,
    query: &EventQuery,
) -> Result<Json<Value>, HttpError> {
    let store = state.store.clone();
    let profile_id = task.profile_id.clone();
    let provider = run_read(move || {
        store
            .repositories()
            .profiles()
            .get(&profile_id)
            .map_err(HttpError::from)?
            .ok_or_else(|| HttpError::not_found("unknown task profile"))
            .map(|profile| profile.provider)
    })
    .await?;

    let tail = query.last.is_some() || query.before.is_some();
    let after = query.after.unwrap_or(0).max(0);
    let wait_ms = query.wait_ms.unwrap_or(0).min(30_000);
    if !tail && after > 0 && wait_ms > 0 && !task.state.settled() {
        state
            .events
            .wait_for_change(
                after,
                std::slice::from_ref(&task.id),
                Duration::from_millis(wait_ms),
            )
            .await
            .map_err(HttpError::from)?;
    }

    let store = state.store.clone();
    let task_id = task.id.clone();
    let read_query = query.clone();
    let (views, tail_page) = run_read(move || {
        let query = read_query;
        let events = read_events(&store, &task_id, &query)?;
        let views = mark_repeated_retries(
            events
                .iter()
                .map(|event| event_view(event, provider))
                .collect::<Vec<TaskEventView>>(),
        );
        if query.last.is_some() || query.before.is_some() {
            let cursor = views
                .last()
                .map_or(query.after.unwrap_or(0), |event| event.id);
            let oldest_id = views.first().map_or(0, |event| event.id);
            let has_earlier = oldest_id > 0 && has_event_before(&store, &task_id, oldest_id)?;
            return Ok((views, Some((cursor, oldest_id, has_earlier))));
        }
        Ok((views, None))
    })
    .await?;

    if let Some((cursor, oldest_id, has_earlier)) = tail_page {
        return Ok(Json(json!({
            "events": views,
            "cursor": cursor,
            "hasMore": false,
            "oldestId": oldest_id,
            "hasEarlier": has_earlier,
        })));
    }

    if query.after.is_none() && query.wait_ms.is_none() {
        let limit = query.limit.unwrap_or(5_000).clamp(1, 5_000) as usize;
        return Ok(Json(
            serde_json::to_value(views.into_iter().take(limit).collect::<Vec<_>>()).unwrap(),
        ));
    }

    let limit = query.limit.unwrap_or(5_000).clamp(1, 5_000) as usize;
    let has_more = views.len() > limit;
    let events = views.into_iter().take(limit).collect::<Vec<_>>();
    let cursor = events
        .last()
        .map_or(query.after.unwrap_or(0), |event| event.id);
    Ok(Json(json!({
        "events": events,
        "cursor": cursor,
        "hasMore": has_more,
    })))
}

/// The tray and dock badge need two numbers, and loading the task list to
/// count them costs megabytes of JSON per poll. Count the states instead.
pub async fn get_activity(State(state): State<HttpState>) -> Result<impl IntoResponse, HttpError> {
    let counts = state.store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT state, COUNT(*) FROM tasks WHERE archived_at IS NULL GROUP BY state",
        )?;
        let mut counts = ActivityCounts::default();
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as usize,
            ))
        })?;
        for row in rows {
            let (state, count) = row?;
            let Ok(state) = serde_json::from_value::<TaskState>(Value::String(state)) else {
                continue;
            };
            if state.is_active() {
                counts.running += count;
            }
        }
        Ok(counts)
    })?;
    Ok(Json(counts))
}

fn archived_filter(value: Option<&str>) -> ArchivedFilter {
    match value {
        Some("only") => ArchivedFilter::Only,
        Some("include") => ArchivedFilter::Include,
        _ => ArchivedFilter::Active,
    }
}

fn public_profiles(store: &Store) -> Result<Vec<ProfileView>, HttpError> {
    store
        .repositories()
        .profiles()
        .list()?
        .iter()
        .map(|profile| Ok(crate::profiles::public_profile(profile)))
        .collect()
}

fn list_tasks(
    store: &Store,
    archived: ArchivedFilter,
    summary: bool,
    limit: u64,
) -> Result<(Value, bool), HttpError> {
    let archive_clause = match archived {
        ArchivedFilter::Active => "archived_at IS NULL",
        ArchivedFilter::Only => "archived_at IS NOT NULL",
        ArchivedFilter::Include => "1 = 1",
    };
    let row_limit = if summary { limit + 1 } else { 200 };
    store.with_connection(|connection| {
        let sql = format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE kind='delegated' AND {archive_clause} ORDER BY updated_at DESC,id DESC LIMIT ?"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement
            .query_map([row_limit], task_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        let mut tasks = rows;
        attach_task_read_fields(connection, &mut tasks)?;
        let has_more = summary && tasks.len() > limit as usize;
        let selected = tasks.into_iter().take(limit as usize);
        let value = if summary {
            Value::Array(
                selected
                    .map(|task| serde_json::to_value(task_summary(&task)).unwrap())
                    .map(|mut value| {
                        if let Some(object) = value.as_object_mut() {
                            object.remove("sessionId");
                        }
                        value
                    })
                    .collect(),
            )
        } else {
            Value::Array(selected.map(|task| serde_json::to_value(task).unwrap()).collect())
        };
        Ok((value, has_more))
    })
    .map_err(HttpError::from)
}

pub(crate) fn load_task(store: &Store, id: &str) -> Result<Option<Task>, HttpError> {
    store
        .with_connection(|connection| {
            let mut task = connection
                .query_row(
                    &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id=?"),
                    [id],
                    task_from_row,
                )
                .optional()?;
            if let Some(task) = &mut task {
                attach_task_read_fields(connection, std::slice::from_mut(task))?;
            }
            Ok(task)
        })
        .map_err(HttpError::from)
}

/// Reads the follow-up counts and holds for a whole page of tasks in two
/// queries rather than two per task. Both tables hold a handful of rows —
/// a page of two hundred tasks was costing four hundred round trips to read
/// them.
fn attach_task_read_fields(
    connection: &rusqlite::Connection,
    tasks: &mut [Task],
) -> rusqlite::Result<()> {
    if tasks.is_empty() {
        return Ok(());
    }
    let mut counts = BTreeMap::<String, u64>::new();
    let mut statement =
        connection.prepare("SELECT task_id,COUNT(*) FROM task_follow_ups GROUP BY task_id")?;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
    })? {
        let (task_id, count) = row?;
        counts.insert(task_id, count);
    }

    let mut holds = BTreeMap::<String, TaskHoldView>::new();
    let mut statement = connection.prepare(
        "SELECT task_id,verb,start_at,await_profile,await_model,args_json,note,expires_at,next_check_at FROM task_holds",
    )?;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, hold_view_from_row_offset(row, 1)?))
    })? {
        let (task_id, hold) = row?;
        holds.insert(task_id, hold);
    }

    for task in tasks {
        task.queued_follow_ups = counts.get(&task.id).copied().filter(|count| *count > 0);
        task.hold = holds.get(&task.id).cloned();
    }
    Ok(())
}

/// `first` is the column the hold's own fields start at, so a query that
/// selects the task id alongside them can share this reader.
fn hold_view_from_row_offset(row: &Row<'_>, first: usize) -> rusqlite::Result<TaskHoldView> {
    let verb: String = row.get(first)?;
    let start_at: Option<String> = row.get(first + 1)?;
    let await_profile: Option<String> = row.get(first + 2)?;
    let await_model: Option<String> = row.get(first + 3)?;
    let args: String = row.get(first + 4)?;
    let note: String = row.get(first + 5)?;
    let expires_at: String = row.get(first + 6)?;
    let next_check_at: String = row.get(first + 7)?;
    let args: Value = serde_json::from_str(&args).unwrap_or_default();
    let kind = if args.get("scheduled") == Some(&Value::Bool(true)) {
        oga_domain::HoldViewKind::Time
    } else if verb == "delegate" {
        oga_domain::HoldViewKind::Dependency
    } else if args.get("restart").is_some() {
        oga_domain::HoldViewKind::Restart
    } else if args.get("network").is_some() {
        oga_domain::HoldViewKind::Network
    } else if await_profile.is_some() || await_model.is_some() {
        oga_domain::HoldViewKind::ProfileAvailable
    } else {
        oga_domain::HoldViewKind::Time
    };
    let waiting_on = if kind == oga_domain::HoldViewKind::Dependency {
        note.strip_prefix("waiting for ")
            .and_then(|text| text.split(" to finish").next())
            .map(str::to_owned)
    } else {
        match (await_profile, await_model) {
            (Some(profile), Some(model)) => Some(format!("{profile}/{model}")),
            (Some(profile), None) => Some(profile),
            (None, Some(model)) => Some(model),
            (None, None) => None,
        }
    };
    Ok(TaskHoldView {
        kind,
        // A wait without a scheduled start still has an expected pick-up: the
        // next look. Callers show one time either way.
        until: start_at.or(Some(next_check_at)),
        waiting_on,
        note,
        expires_at,
    })
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<Task> {
    let kind = decode_json::<TaskKind>(&format!("\"{}\"", row.get::<_, String>(1)?), 1)?;
    let state = decode_json::<TaskState>(&format!("\"{}\"", row.get::<_, String>(12)?), 12)?;
    let scope = decode_json::<oga_domain::TaskScope>(&row.get::<_, String>(19)?, 19)?;
    let completion = row
        .get::<_, Option<String>>(28)?
        .map(|value| decode_json::<TaskCompletion>(&value, 28))
        .transpose()?;
    let attempts = row
        .get::<_, Option<String>>(29)?
        .map(|value| decode_json(&value, 29))
        .transpose()?
        .unwrap_or_default();
    let worktree = match (
        row.get::<_, Option<String>>(8)?,
        row.get::<_, Option<String>>(9)?,
        row.get::<_, Option<String>>(10)?,
    ) {
        (Some(origin_cwd), Some(path), Some(branch)) => Some(TaskWorktree {
            origin_cwd,
            path,
            branch,
            links: row
                .get::<_, Option<String>>(11)?
                .map(|value| decode_json(&value, 11))
                .transpose()?,
        }),
        _ => None,
    };
    let worktree_label = worktree.as_ref().map(|worktree| {
        oga_worktree::worktree_label(Path::new(&worktree.origin_cwd), Path::new(&worktree.path))
    });
    Ok(Task {
        id: row.get(0)?,
        kind: Some(kind),
        profile_id: row.get(2)?,
        model: row.get(3)?,
        prompt: row.get(4)?,
        shipped_prompt: row.get(5)?,
        cwd: row.get(6)?,
        branch: row.get(7)?,
        worktree,
        worktree_label,
        state,
        output: row.get(13)?,
        error: row.get(14)?,
        question: row.get(15)?,
        parent_task_id: row.get(16)?,
        orchestrator_id: row.get(17)?,
        scope,
        grant_id: row.get(20)?,
        allow_questions: row.get::<_, i64>(21)? != 0,
        can_delegate: row.get::<_, i64>(36)? != 0,
        timeout_ms: row.get(22)?,
        effort: row.get(23)?,
        effort_actual: row.get(24)?,
        tldr: row.get(25)?,
        title: row.get(26)?,
        session_id: row.get(27)?,
        completion,
        attempts,
        cost_usd: row.get(30)?,
        cost_usd_estimated: row.get::<_, Option<i64>>(31)?.unwrap_or(0) != 0,
        turns: row.get(32)?,
        archived_at: row.get(33)?,
        created_at: row.get(34)?,
        updated_at: row.get(35)?,
        ..Task::default()
    })
}

fn decode_json<T: DeserializeOwned>(value: &str, column: usize) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn task_summary(task: &Task) -> TaskSummary {
    TaskSummary {
        id: task.id.clone(),
        profile_id: task.profile_id.clone(),
        model: task.model.clone(),
        cwd: task.cwd.clone(),
        origin_cwd: task
            .worktree
            .as_ref()
            .map(|worktree| worktree.origin_cwd.clone()),
        branch: task.effective_branch().map(str::to_owned),
        worktree_label: task.worktree_label.clone(),
        state: task.state,
        prompt_preview: task
            .prompt
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(240)
            .collect(),
        tldr: task.tldr.clone(),
        title: task.title.clone(),
        created_at: task.created_at.clone(),
        updated_at: task.updated_at.clone(),
        error: task
            .error
            .as_ref()
            .map(|error| error.chars().take(500).collect()),
        question: task.question.clone(),
        parent_task_id: task.parent_task_id.clone(),
        orchestrator_id: task.orchestrator_id.clone(),
        grant_id: task.grant_id.clone(),
        session_id: task.session_id.clone(),
        completion: task.completion.clone(),
        cost_usd: task.cost_usd,
        cost_usd_estimated: task.cost_usd_estimated,
        archived_at: task.archived_at.clone(),
        hold: task.hold.clone(),
    }
}

fn read_events(
    store: &Store,
    task_id: &str,
    query: &EventQuery,
) -> Result<Vec<TaskEvent>, HttpError> {
    store
        .with_connection(|connection| {
            let tail = query.last.is_some() || query.before.is_some();
            let limit = query.limit.unwrap_or(5_000).clamp(1, 5_000) as i64;
            let rows = if tail {
                let before = query.before.unwrap_or(0);
                let count = query.last.unwrap_or(5_000).clamp(1, 5_000) as i64;
                let mut statement = if before > 0 {
                    connection.prepare(
                        "SELECT id,task_id,event_type,state,payload,created_at,turn_id FROM task_events WHERE task_id=? AND id<? ORDER BY id DESC LIMIT ?",
                    )?
                } else {
                    connection.prepare(
                        "SELECT id,task_id,event_type,state,payload,created_at,turn_id FROM task_events WHERE task_id=? ORDER BY id DESC LIMIT ?",
                    )?
                };
                let rows = if before > 0 {
                    statement
                        .query_map(params![task_id, before, count], event_from_row)?
                        .collect::<Result<Vec<_>, _>>()?
                } else {
                    statement
                        .query_map(params![task_id, count], event_from_row)?
                        .collect::<Result<Vec<_>, _>>()?
                };
                rows.into_iter().rev().collect()
            } else {
                let after = query.after.unwrap_or(0);
                let count = limit + 1;
                let mut statement = connection.prepare(
                    "SELECT id,task_id,event_type,state,payload,created_at,turn_id FROM task_events WHERE task_id=? AND id>? ORDER BY id LIMIT ?",
                )?;
                statement
                    .query_map(params![task_id, after, count], event_from_row)?
                    .collect::<Result<Vec<_>, _>>()?
            };
            Ok(rows)
        })
        .map_err(HttpError::from)
}

fn event_from_row(row: &Row<'_>) -> rusqlite::Result<TaskEvent> {
    let state = decode_json::<TaskState>(&format!("\"{}\"", row.get::<_, String>(3)?), 3)?;
    let payload = decode_json::<BTreeMap<String, Value>>(&row.get::<_, String>(4)?, 4)?;
    Ok(TaskEvent {
        id: row.get(0)?,
        task_id: row.get(1)?,
        kind: row.get(2)?,
        state,
        payload,
        created_at: row.get(5)?,
        turn_id: row.get(6)?,
    })
}

fn has_event_before(store: &Store, task_id: &str, id: i64) -> Result<bool, HttpError> {
    store
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM task_events WHERE task_id=? AND id<?)",
                params![task_id, id],
                |row| row.get(0),
            )?)
        })
        .map_err(HttpError::from)
}

pub(crate) fn list_profile_failures(store: &Store) -> Result<Vec<ProfileFailure>, HttpError> {
    store
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT profile_id,code,message,failed_at,consecutive_failures,retry_at,model FROM profile_failures ORDER BY profile_id,model",
            )?;
            Ok(statement
                .query_map([], |row| {
                    let code = decode_json::<FailureCode>(
                        &format!("\"{}\"", row.get::<_, String>(1)?),
                        1,
                    )?;
                    let model = row.get::<_, String>(6)?;
                    let model = (!model.is_empty()).then_some(model);
                    Ok(ProfileFailure {
                        profile_id: row.get(0)?,
                        code,
                        message: row.get(2)?,
                        failed_at: row.get(3)?,
                        consecutive_failures: row.get(4)?,
                        retry_at: row.get(5)?,
                        model,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(HttpError::from)
}

fn list_grants(store: &Store) -> Result<Vec<ScopeGrant>, HttpError> {
    store
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,cwd,profile_id,scope_json,created_at,last_used_at,use_count FROM scope_grants ORDER BY last_used_at DESC,id",
            )?;
            Ok(statement
                .query_map([], |row| {
                    Ok(ScopeGrant {
                        id: row.get(0)?,
                        cwd: row.get(1)?,
                        profile_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        scope: decode_json(&row.get::<_, String>(3)?, 3)?,
                        created_at: row.get(4)?,
                        last_used_at: row.get(5)?,
                        use_count: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(HttpError::from)
}

fn list_memory_projects(store: &Store) -> Result<Vec<MemoryProject>, HttpError> {
    store
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT cwd,COUNT(*),COALESCE(SUM(length(value)),0),MAX(updated_at) FROM memories GROUP BY cwd ORDER BY cwd",
            )?;
            Ok(statement
                .query_map([], |row| {
                    Ok(MemoryProject {
                        cwd: row.get(0)?,
                        count: row.get(1)?,
                        chars: row.get(2)?,
                        updated_at: row.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(HttpError::from)
}

fn spend_totals(store: &Store) -> Result<SpendTotals, HttpError> {
    let now = now_iso();
    let since_ms = oga_routing::now_ms().saturating_sub(24 * 60 * 60 * 1_000);
    let since = oga_routing::format_rfc3339_ms(since_ms);
    store
        .repositories()
        .spend()
        .totals(&since, &now, 24 * 60 * 60 * 1_000)
        .map_err(HttpError::from)
}

fn now_iso() -> String {
    oga_routing::format_rfc3339_ms(oga_routing::now_ms())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::http::StatusCode;

    use super::run_read_with_timeout;

    #[tokio::test]
    async fn a_stuck_read_times_out_instead_of_hanging_forever() {
        let result: Result<(), super::HttpError> =
            run_read_with_timeout(Duration::from_millis(20), || {
                std::thread::sleep(Duration::from_millis(200));
                Ok(())
            })
            .await;

        let error = result.expect_err("a read stuck behind the connection lock must fail");
        assert_eq!(error.status, StatusCode::GATEWAY_TIMEOUT);
    }

    #[tokio::test]
    async fn a_fast_read_still_resolves_normally() {
        let result = run_read_with_timeout(Duration::from_secs(5), || Ok(42)).await;

        assert_eq!(result.expect("a fast read must succeed"), 42);
    }
}
