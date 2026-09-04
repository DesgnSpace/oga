//! Cursor-based broker event streaming.

use std::{
    collections::HashMap,
    convert::Infallible,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{RawQuery, State},
    http::header,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
};
use oga_domain::{EventKind, EventPointer, Provider, Task, TaskEvent, TaskKind};
use oga_events::event_view;
use oga_store::{Store, StoreError};
use rusqlite::{Row, ToSql, params_from_iter};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    router::{HttpError, HttpState},
    state,
};

pub const MAX_BATCH_EVENTS: usize = 100;
pub const KEEPALIVE: Duration = Duration::from_secs(30);
pub const BACKPRESSURE_TIMEOUT: Duration = Duration::from_secs(10);
pub const STREAM_CHANNEL_CAPACITY: usize = 64;

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_SUMMARY_CHARS: usize = 500;

/// The event log read helper shared by SSE and per-task long polls.
#[derive(Clone)]
pub struct EventFanout {
    store: Arc<Store>,
    poll_interval: Duration,
}

impl EventFanout {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            poll_interval: POLL_INTERVAL,
        }
    }

    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval.max(Duration::from_millis(1));
        self
    }

    /// Waits until the cursor has new rows, or the timeout expires.
    ///
    /// Event writes can come from a different store handle or process, so the
    /// durable log remains the source of truth. The short recovery poll closes
    /// the same insert/wait race as an in-process notification without making
    /// callers depend on a writer-side callback.
    pub async fn wait_for_change(
        &self,
        after: i64,
        task_ids: &[String],
        wait: Duration,
    ) -> Result<bool, StoreError> {
        if self.has_events_after(after, task_ids)? {
            return Ok(true);
        }
        if wait.is_zero() {
            return Ok(false);
        }

        let deadline = Instant::now() + wait;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            sleep(self.poll_interval.min(deadline - now)).await;
            if self.has_events_after(after, task_ids)? {
                return Ok(true);
            }
        }
    }

    pub fn latest_event_id(&self, task_ids: &[String]) -> Result<i64, StoreError> {
        aggregate_event_id(&self.store, "MAX", task_ids)
    }

    pub fn oldest_event_id(&self, task_ids: &[String]) -> Result<i64, StoreError> {
        aggregate_event_id(&self.store, "MIN", task_ids)
    }

    fn has_events_after(&self, after: i64, task_ids: &[String]) -> Result<bool, StoreError> {
        self.store.with_connection(|connection| {
            let task_filter = task_clause(task_ids, "task_id");
            let sql = format!("SELECT EXISTS(SELECT 1 FROM task_events WHERE id > ?{task_filter})");
            let mut values: Vec<&dyn ToSql> = vec![&after];
            values.extend(task_ids.iter().map(|id| id as &dyn ToSql));
            Ok(connection.query_row(&sql, params_from_iter(values), |row| row.get(0))?)
        })
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct EventQuery {
    pub after: Option<String>,
    pub task: Vec<String>,
    pub tasks: Vec<String>,
    pub kinds: Vec<String>,
    pub agents: Option<String>,
}

/// Handles `GET /api/events` and returns pointers only. Event bodies remain on
/// the per-task event route so reconnecting clients do not receive transcripts.
pub async fn events(
    State(state): State<HttpState>,
    RawQuery(raw_query): RawQuery,
) -> Result<Response, HttpError> {
    let query = parse_query(raw_query.as_deref())?;
    let after = parse_cursor(query.after.as_deref())?;
    let task_ids = task_ids(&query);
    for task_id in &task_ids {
        if state::load_task(&state.store, task_id)?.is_none() {
            return Err(HttpError::not_found(format!(
                "unknown task: {task_id} — call tasks to list recent task ids"
            )));
        }
    }
    let kinds = parse_kinds(&query.kinds)?;
    let include_orchestrators = query.agents.as_deref() == Some("1");
    let fanout = state.events.clone();
    let store = state.store.clone();
    let stream_floor = fanout.oldest_event_id(&task_ids)?;
    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let options = StreamOptions {
        after,
        task_ids,
        stream_floor,
        kinds,
        include_orchestrators,
    };

    tokio::spawn(async move {
        run_stream(sender, store, fanout, options).await;
    });

    let stream = ReceiverStream::new(receiver);
    let mut response = Sse::new(stream).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, "no-cache, no-store".parse().unwrap());
    headers.insert(header::CONNECTION, "keep-alive".parse().unwrap());
    headers.insert("x-accel-buffering", "no".parse().unwrap());
    Ok(response)
}

pub async fn head(State(state): State<HttpState>) -> Result<axum::Json<Value>, HttpError> {
    Ok(axum::Json(
        json!({ "cursor": state.events.latest_event_id(&[])? }),
    ))
}

fn parse_query(raw_query: Option<&str>) -> Result<EventQuery, HttpError> {
    let mut query = EventQuery::default();
    for (key, value) in serde_urlencoded::from_str::<Vec<(String, String)>>(raw_query.unwrap_or(""))
        .map_err(|_| HttpError::bad_request("invalid query string"))?
    {
        match key.as_str() {
            "after" if query.after.is_none() => query.after = Some(value),
            "task" => query.task.push(value),
            "tasks" => query.tasks.push(value),
            "kinds" => query.kinds.push(value),
            "agents" if query.agents.is_none() => query.agents = Some(value),
            _ => {}
        }
    }
    Ok(query)
}

fn parse_cursor(value: Option<&str>) -> Result<i64, HttpError> {
    let value =
        value.ok_or_else(|| HttpError::bad_request("after must be a non-negative integer"))?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(HttpError::bad_request(
            "after must be a non-negative integer",
        ));
    }
    let cursor = value
        .parse::<i64>()
        .map_err(|_| HttpError::bad_request("after must be a non-negative integer"))?;
    if cursor > 9_007_199_254_740_991 {
        return Err(HttpError::bad_request(
            "after must be a non-negative integer",
        ));
    }
    Ok(cursor)
}

fn task_ids(query: &EventQuery) -> Vec<String> {
    let mut ids = Vec::new();
    for id in &query.task {
        if !ids.iter().any(|seen| seen == id) {
            ids.push(id.clone());
        }
    }
    for value in &query.tasks {
        for id in value.split(',').map(str::trim).filter(|id| !id.is_empty()) {
            if !ids.iter().any(|seen| seen == id) {
                ids.push(id.to_owned());
            }
        }
    }
    ids
}

fn parse_kinds(values: &[String]) -> Result<Option<Vec<EventKind>>, HttpError> {
    let mut kinds = Vec::new();
    for value in values.iter().flat_map(|value| value.split(',')) {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let kind = match value {
            "lifecycle" => EventKind::Lifecycle,
            "message" => EventKind::Message,
            "reasoning" => EventKind::Reasoning,
            "tool" => EventKind::Tool,
            "command" => EventKind::Command,
            "file" => EventKind::File,
            "error" => EventKind::Error,
            "usage" => EventKind::Usage,
            "raw" => EventKind::Raw,
            "retry" => EventKind::Retry,
            _ => {
                return Err(HttpError::bad_request(
                    "unknown event kind; expected one of lifecycle, message, reasoning, tool, command, file, error, usage, raw, retry",
                ));
            }
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    Ok((!kinds.is_empty()).then_some(kinds))
}

struct StreamOptions {
    after: i64,
    task_ids: Vec<String>,
    stream_floor: i64,
    kinds: Option<Vec<EventKind>>,
    include_orchestrators: bool,
}

async fn run_stream(
    sender: mpsc::Sender<Result<Event, Infallible>>,
    store: Arc<Store>,
    fanout: Arc<EventFanout>,
    options: StreamOptions,
) {
    let StreamOptions {
        after,
        task_ids,
        stream_floor,
        kinds,
        include_orchestrators,
    } = options;
    let stale = after > 0 && stream_floor > 0 && after < stream_floor;
    let mut cursor = if stale {
        fanout.latest_event_id(&task_ids).unwrap_or(after)
    } else {
        after
    };
    let mut ready = json!({
        "version": 1,
        "cursor": cursor,
        "streamFloor": stream_floor,
    });
    if !task_ids.is_empty() {
        ready["tasks"] = json!(task_ids);
    }
    if let Some(kinds) = &kinds {
        ready["kinds"] = json!(kinds.iter().map(|kind| kind.as_str()).collect::<Vec<_>>());
    }
    if include_orchestrators {
        ready["agents"] = Value::Bool(true);
    }
    if stale {
        ready["stale"] = Value::Bool(true);
    }
    if !send_event(&sender, json_event("ready", ready)).await {
        return;
    }

    loop {
        if sender.is_closed() {
            return;
        }
        let rows = match list_events(&store, cursor, MAX_BATCH_EVENTS, &task_ids) {
            Ok(rows) => rows,
            Err(_) => return,
        };
        if !rows.is_empty() {
            let tasks = match load_tasks(&store, &rows) {
                Ok(tasks) => tasks,
                Err(_) => return,
            };
            for event in rows {
                if sender.is_closed() {
                    return;
                }
                cursor = event.id;
                let Some((task, provider)) = tasks.get(&event.task_id) else {
                    continue;
                };
                if task.kind == Some(TaskKind::Orchestrator) && !include_orchestrators {
                    continue;
                }
                let pointer = event_pointer(&event, task, *provider);
                if kinds
                    .as_ref()
                    .is_some_and(|allowed| !allowed.contains(&pointer.kind))
                {
                    continue;
                }
                if !send_event(
                    &sender,
                    json_event("task", serde_json::to_value(pointer).unwrap()),
                )
                .await
                {
                    return;
                }
            }
            if !send_event(&sender, json_event("cursor", json!({ "cursor": cursor }))).await {
                return;
            }
            continue;
        }

        match fanout.wait_for_change(cursor, &task_ids, KEEPALIVE).await {
            Ok(true) => continue,
            Ok(false) => {
                if !send_event(
                    &sender,
                    json_event("keepalive", json!({ "cursor": cursor })),
                )
                .await
                {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

async fn send_event(sender: &mpsc::Sender<Result<Event, Infallible>>, event: Event) -> bool {
    timeout(BACKPRESSURE_TIMEOUT, sender.send(Ok(event)))
        .await
        .is_ok_and(|result| result.is_ok())
}

fn json_event(name: &'static str, value: Value) -> Event {
    Event::default()
        .event(name)
        .json_data(value)
        .expect("JSON event data")
}

fn event_pointer(event: &TaskEvent, task: &Task, provider: Provider) -> EventPointer {
    let view = event_view(event, provider);
    let kind = view.kind;
    let summary = if event.kind == "agent.system" {
        event
            .payload
            .get("model")
            .and_then(Value::as_str)
            .map_or_else(
                || view.title.clone(),
                |model| format!("{}: {model}", view.title),
            )
    } else if event.kind == "agent.assistant" {
        event
            .payload
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .and_then(|content| content.iter().find_map(|item| item.get("text")))
            .and_then(Value::as_str)
            .map_or_else(
                || view.title.clone(),
                |text| format!("{}: {text}", view.title),
            )
    } else if event.kind == "agent.tool_use" {
        event
            .payload
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .and_then(|content| content.iter().find_map(|item| item.get("input")))
            .and_then(|input| input.get("file_path"))
            .and_then(Value::as_str)
            .map_or_else(|| view.title.clone(), |path| format!("Edited · {path}"))
    } else if event.kind == "agent.result" {
        let cost = event.payload.get("total_cost_usd").and_then(Value::as_f64);
        let turns = event.payload.get("num_turns").and_then(Value::as_u64);
        match (cost, turns) {
            (Some(cost), Some(turns)) => format!("{}: ${cost:.2} · {turns} turns", view.title),
            _ => view.title.clone(),
        }
    } else if event.kind == "failed" {
        event
            .payload
            .get("error")
            .and_then(Value::as_str)
            .map_or_else(
                || view.title.clone(),
                |error| format!("{}: {error}", view.title),
            )
    } else {
        view.detail.as_deref().map_or_else(
            || view.title.clone(),
            |detail| format!("{}: {detail}", view.title),
        )
    };
    EventPointer {
        id: event.id,
        cursor: event.id,
        task_id: event.task_id.clone(),
        event_type: event.kind.clone(),
        kind,
        state: event.state,
        at: event.created_at.clone(),
        title: task
            .title
            .as_deref()
            .or(task.tldr.as_deref())
            .unwrap_or(&event.task_id)
            .to_owned(),
        summary: summary.chars().take(MAX_SUMMARY_CHARS).collect(),
        turn_id: event.turn_id,
    }
}

fn list_events(
    store: &Store,
    after: i64,
    limit: usize,
    task_ids: &[String],
) -> Result<Vec<TaskEvent>, StoreError> {
    store.with_connection(|connection| {
        let task_filter = task_clause(task_ids, "task_id");
        let sql = format!(
            "SELECT id,task_id,event_type,state,payload,created_at,turn_id FROM task_events WHERE id > ?{task_filter} ORDER BY id LIMIT ?"
        );
        let mut statement = connection.prepare(&sql)?;
        let limit = limit as i64;
        let mut values: Vec<&dyn ToSql> = vec![&after];
        values.extend(task_ids.iter().map(|id| id as &dyn ToSql));
        values.push(&limit);
        Ok(statement
            .query_map(params_from_iter(values), event_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    })
}

fn load_tasks(
    store: &Store,
    events: &[TaskEvent],
) -> Result<HashMap<String, (Task, Provider)>, HttpError> {
    let mut tasks = HashMap::new();
    for task_id in events.iter().map(|event| &event.task_id) {
        if tasks.contains_key(task_id) {
            continue;
        }
        let Some(task) = state::load_task(store, task_id)? else {
            continue;
        };
        let provider = store
            .repositories()
            .profiles()
            .get(&task.profile_id)?
            .map_or(Provider::Claude, |profile| profile.provider);
        tasks.insert(task_id.clone(), (task, provider));
    }
    Ok(tasks)
}

fn event_from_row(row: &Row<'_>) -> rusqlite::Result<TaskEvent> {
    let state =
        serde_json::from_str(&format!("\"{}\"", row.get::<_, String>(3)?)).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    let payload = serde_json::from_str(&row.get::<_, String>(4)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(error))
    })?;
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

fn aggregate_event_id(
    store: &Store,
    aggregate: &str,
    task_ids: &[String],
) -> Result<i64, StoreError> {
    store.with_connection(|connection| {
        let task_filter = task_clause(task_ids, "task_id");
        let sql =
            format!("SELECT COALESCE({aggregate}(id), 0) FROM task_events WHERE 1=1{task_filter}");
        let mut values: Vec<&dyn ToSql> = Vec::with_capacity(task_ids.len());
        values.extend(task_ids.iter().map(|id| id as &dyn ToSql));
        Ok(connection.query_row(&sql, params_from_iter(values), |row| row.get(0))?)
    })
}

fn task_clause(task_ids: &[String], column: &str) -> String {
    if task_ids.is_empty() {
        String::new()
    } else {
        format!(
            " AND {column} IN ({})",
            std::iter::repeat_n("?", task_ids.len())
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}
