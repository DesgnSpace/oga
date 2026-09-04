//! Unix-domain NDJSON delivery for task watchers.

use std::{
    collections::HashMap,
    fs, io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use oga_domain::{
    BatchFrame, BatchTask, HelloFrame, HelloPayload, Provider, Task, TaskEvent, WaitedTaskEvent,
};
use oga_store::{Store, StoreError};
use rusqlite::{Row, ToSql, params_from_iter};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    runtime::Handle,
    sync::watch,
    task::JoinHandle,
    time::timeout,
};

use crate::event_view;

pub const MAX_SOCK_PATH: usize = 103;
pub const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;
pub const BACKPRESSURE_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_KEEPALIVE: Duration = Duration::from_secs(30);
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_BATCH_EVENTS: usize = 100;
const SUBSCRIBE_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum SocketError {
    #[error("could not bind event socket {path}: {source}")]
    Bind { path: PathBuf, source: io::Error },
    #[error("event socket requires a Tokio runtime")]
    NoRuntime,
}

#[derive(Debug, Clone)]
pub struct EventSocketOptions {
    pub path: PathBuf,
    pub hello: HelloPayload,
    pub keepalive: Duration,
    pub poll_interval: Duration,
}

impl EventSocketOptions {
    pub fn new(path: impl Into<PathBuf>, hello: HelloPayload) -> Self {
        Self {
            path: path.into(),
            hello,
            keepalive: DEFAULT_KEEPALIVE,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    pub fn with_keepalive(mut self, keepalive: Duration) -> Self {
        self.keepalive = keepalive;
        self
    }

    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval.max(Duration::from_millis(1));
        self
    }
}

/// A running event socket. Dropping the handle stops its accept loop and
/// removes the socket path, but never removes the database or task history.
pub struct EventSocketHandle {
    pub path: Option<PathBuf>,
    shutdown: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl EventSocketHandle {
    pub fn stop(&self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.lock().expect("socket task lock").take() {
            task.abort();
        }
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for EventSocketHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts a broker-owned Unix socket. Paths beyond macOS's `sun_path` limit
/// return a disabled handle; bind failures remain errors.
pub fn start_event_socket(
    store: Arc<Store>,
    options: EventSocketOptions,
) -> Result<EventSocketHandle, SocketError> {
    let path_bytes = options.path.as_os_str().as_bytes().len();
    if path_bytes > MAX_SOCK_PATH {
        let (shutdown, _) = watch::channel(true);
        return Ok(EventSocketHandle {
            path: None,
            shutdown,
            task: Mutex::new(None),
        });
    }

    let runtime = Handle::try_current().map_err(|_| SocketError::NoRuntime)?;
    let _ = fs::remove_file(&options.path);
    let listener = UnixListener::bind(&options.path).map_err(|source| SocketError::Bind {
        path: options.path.clone(),
        source,
    })?;
    let (shutdown, receiver) = watch::channel(false);
    let path = options.path.clone();
    let task = runtime.spawn(accept_loop(listener, store, options, receiver));
    Ok(EventSocketHandle {
        path: Some(path),
        shutdown,
        task: Mutex::new(Some(task)),
    })
}

/// Resolves the default socket beside a database file.
pub fn event_socket_path(database: impl AsRef<Path>) -> PathBuf {
    database
        .as_ref()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("oga.sock")
}

async fn accept_loop(
    listener: UnixListener,
    store: Arc<Store>,
    options: EventSocketOptions,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { return; };
                let connection_shutdown = shutdown.clone();
                let connection_store = store.clone();
                let connection_options = options.clone();
                tokio::spawn(async move {
                    serve_connection(connection_store, connection_options, stream, connection_shutdown).await;
                });
            }
        }
    }
}

async fn serve_connection(
    store: Arc<Store>,
    options: EventSocketOptions,
    stream: UnixStream,
    mut shutdown: watch::Receiver<bool>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let read = tokio::select! {
        changed = shutdown.changed() => {
            if changed.is_err() || *shutdown.borrow() { return; }
            return;
        }
        result = reader.read_line(&mut line) => result,
    };
    let Ok(bytes) = read else {
        return;
    };
    if bytes == 0 || bytes > SUBSCRIBE_BYTES {
        return;
    }
    let Ok(frame) = serde_json::from_str::<Value>(line.trim()) else {
        let _ = write_json(
            &mut writer,
            &json!({ "error": "invalid JSON subscribe frame" }),
        )
        .await;
        return;
    };
    let (task_ids, requested_cursor) = match parse_subscribe(&frame) {
        Ok(value) => value,
        Err(message) => {
            let _ = write_json(&mut writer, &json!({ "error": message })).await;
            return;
        }
    };
    for task_id in &task_ids {
        match store.repositories().tasks().get(task_id) {
            Ok(Some(_)) => {}
            Ok(None) => {
                let _ = write_json(
                    &mut writer,
                    &json!({ "error": unknown_task_message(task_id) }),
                )
                .await;
                return;
            }
            Err(_) => return,
        }
    }

    let initial_cursor = match latest_event_id(&store, &task_ids, true) {
        Ok(cursor) => cursor,
        Err(_) => return,
    };
    let stream_floor = match oldest_event_id(&store, &task_ids) {
        Ok(cursor) => cursor,
        Err(_) => return,
    };
    let stale = requested_cursor > 0 && stream_floor > 0 && requested_cursor < stream_floor;
    let mut cursor = if stale {
        stream_floor - 1
    } else {
        requested_cursor
    };
    let hello = HelloFrame {
        hello: HelloPayload {
            initial_cursor: Some(initial_cursor),
            stream_floor: Some(stream_floor),
            stale: stale.then_some(true),
            ..options.hello.clone()
        },
    };
    if !write_json(&mut writer, &hello).await {
        return;
    }

    let mut watched = task_ids;
    loop {
        if *shutdown.borrow() {
            return;
        }
        let Some(waited) = wait_for_tasks(
            &store,
            &watched,
            cursor,
            options.keepalive,
            options.poll_interval,
            &mut shutdown,
        )
        .await
        else {
            return;
        };
        let contexts = match load_task_contexts(&store, &waited.events, &watched) {
            Ok(contexts) => contexts,
            Err(_) => return,
        };
        let mut events = Vec::with_capacity(waited.events.len());
        for event in &waited.events {
            cursor = cursor.max(event.id);
            let Some(context) = contexts.get(&event.task_id) else {
                continue;
            };
            events.push(crate::event_to_batch(
                &waited_event(event, context.provider),
                Some(&context.task),
            ));
        }
        cursor = cursor.max(waited.cursor);
        let tasks = watched
            .iter()
            .filter_map(|id| {
                contexts
                    .get(id)
                    .map(|context| crate::task_to_batch(&context.task))
            })
            .collect::<Vec<BatchTask>>();
        let batch = BatchFrame {
            events,
            tasks,
            cursor,
            has_more: waited.has_more,
        };
        if !write_json(&mut writer, &batch).await {
            return;
        }
        if waited.has_more {
            continue;
        }
        watched.retain(|id| {
            contexts
                .get(id)
                .is_none_or(|context| !context.task.state.settled())
        });
        if watched.is_empty() {
            return;
        }
    }
}

#[derive(Debug)]
struct WaitedBatch {
    events: Vec<TaskEvent>,
    cursor: i64,
    has_more: bool,
}

async fn wait_for_tasks(
    store: &Store,
    task_ids: &[String],
    cursor: i64,
    wait: Duration,
    poll_interval: Duration,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<WaitedBatch> {
    let deadline = Instant::now() + wait;
    loop {
        let events = list_events(store, cursor, MAX_BATCH_EVENTS + 1, task_ids, true).ok()?;
        let tasks = load_tasks(store, task_ids).ok()?;
        let settled = tasks.iter().any(|task| task.state.settled());
        if !events.is_empty() || settled {
            let has_more = events.len() > MAX_BATCH_EVENTS;
            let events = events
                .into_iter()
                .take(MAX_BATCH_EVENTS)
                .collect::<Vec<_>>();
            let next_cursor = events.last().map_or(cursor, |event| event.id);
            return Some(WaitedBatch {
                events,
                cursor: next_cursor,
                has_more,
            });
        }
        let now = Instant::now();
        if now >= deadline {
            let latest = latest_event_id(store, task_ids, true).ok()?;
            return Some(WaitedBatch {
                events: Vec::new(),
                cursor: cursor.max(latest),
                has_more: false,
            });
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return None; }
            }
            _ = tokio::time::sleep(poll_interval.min(deadline - now)) => {}
        }
    }
}

fn parse_subscribe(frame: &Value) -> Result<(Vec<String>, i64), String> {
    let Some(watch) = frame.get("watch").and_then(Value::as_array) else {
        return Err("subscribe frame must contain a non-empty watch array".into());
    };
    if watch.is_empty() {
        return Err("subscribe frame must contain a non-empty watch array".into());
    }
    let mut task_ids = Vec::with_capacity(watch.len());
    for value in watch {
        let Some(id) = value.as_str() else {
            return Err("subscribe watch array must contain string task ids".into());
        };
        if !task_ids.iter().any(|seen| seen == id) {
            task_ids.push(id.to_owned());
        }
    }
    let after = frame
        .get("afterCursor")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .map_or(0, |value| value.floor().clamp(0.0, i64::MAX as f64) as i64);
    Ok((task_ids, after))
}

fn unknown_task_message(task_id: &str) -> String {
    format!("unknown task: {task_id} — call tasks to list recent task ids")
}

#[derive(Debug)]
struct TaskContext {
    task: Task,
    provider: Provider,
}

fn load_tasks(store: &Store, task_ids: &[String]) -> Result<Vec<Task>, StoreError> {
    task_ids
        .iter()
        .map(|id| {
            store
                .repositories()
                .tasks()
                .get(id)?
                .ok_or_else(|| StoreError::Refusal(format!("unknown task: {id}")))
        })
        .collect()
}

fn load_task_contexts(
    store: &Store,
    events: &[TaskEvent],
    task_ids: &[String],
) -> Result<HashMap<String, TaskContext>, StoreError> {
    let mut ids = task_ids.to_vec();
    ids.extend(events.iter().map(|event| event.task_id.clone()));
    ids.sort();
    ids.dedup();
    let mut contexts = HashMap::new();
    for id in ids {
        let Some(mut task) = store.repositories().tasks().get(&id)? else {
            continue;
        };
        let (title, tldr) = store.with_connection(|connection| {
            Ok(
                connection.query_row("SELECT title,tldr FROM tasks WHERE id=?", [&id], |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                })?,
            )
        })?;
        task.title = title;
        task.tldr = tldr;
        let provider = store
            .repositories()
            .profiles()
            .get(&task.profile_id)?
            .map_or(Provider::Claude, |profile| profile.provider);
        contexts.insert(id, TaskContext { task, provider });
    }
    Ok(contexts)
}

fn waited_event(event: &TaskEvent, provider: Provider) -> WaitedTaskEvent {
    let view = event_view(event, provider);
    WaitedTaskEvent {
        id: event.id,
        task_id: event.task_id.clone(),
        event_type: event.kind.clone(),
        state: event.state,
        at: event.created_at.clone(),
        kind: view.kind,
        summary: event_summary(&view),
        minor: view.minor,
    }
}

fn event_summary(view: &oga_domain::TaskEventView) -> String {
    if view.verb.is_some() {
        return [
            view.verb.as_deref(),
            view.target.as_deref(),
            view.result.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    }
    view.detail.as_deref().map_or_else(
        || view.title.clone(),
        |detail| format!("{}: {detail}", view.title),
    )
}

fn list_events(
    store: &Store,
    after: i64,
    limit: usize,
    task_ids: &[String],
    meaningful_only: bool,
) -> Result<Vec<TaskEvent>, StoreError> {
    store.with_connection(|connection| {
        let task_filter = task_clause(task_ids, "task_id");
        let noise_filter = if meaningful_only {
            " AND event_type NOT IN ('agent.system','heartbeat')"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id,task_id,event_type,state,payload,created_at,turn_id FROM task_events WHERE id > ?{task_filter}{noise_filter} ORDER BY id LIMIT ?"
        );
        let limit = limit as i64;
        let mut values: Vec<&dyn ToSql> = vec![&after];
        values.extend(task_ids.iter().map(|id| id as &dyn ToSql));
        values.push(&limit);
        let mut statement = connection.prepare(&sql)?;
        Ok(statement
            .query_map(params_from_iter(values), event_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    })
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

fn latest_event_id(
    store: &Store,
    task_ids: &[String],
    meaningful_only: bool,
) -> Result<i64, StoreError> {
    aggregate_event_id(store, "MAX", task_ids, meaningful_only)
}

fn oldest_event_id(store: &Store, task_ids: &[String]) -> Result<i64, StoreError> {
    aggregate_event_id(store, "MIN", task_ids, false)
}

fn aggregate_event_id(
    store: &Store,
    aggregate: &str,
    task_ids: &[String],
    meaningful_only: bool,
) -> Result<i64, StoreError> {
    store.with_connection(|connection| {
        let task_filter = task_clause(task_ids, "task_id");
        let noise_filter = if meaningful_only {
            " AND event_type NOT IN ('agent.system','heartbeat')"
        } else {
            ""
        };
        let sql = format!(
            "SELECT COALESCE({aggregate}(id),0) FROM task_events WHERE 1=1{task_filter}{noise_filter}"
        );
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

async fn write_json<W: AsyncWrite + Unpin>(writer: &mut W, value: &impl serde::Serialize) -> bool {
    let Ok(mut bytes) = serde_json::to_vec(value) else {
        return false;
    };
    bytes.push(b'\n');
    if bytes.len() > MAX_PENDING_BYTES {
        return false;
    }
    timeout(BACKPRESSURE_TIMEOUT, writer.write_all(&bytes))
        .await
        .is_ok_and(|result| result.is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn socket_rejects_frames_over_the_pending_limit() {
        let mut writer = tokio::io::sink();
        assert!(
            !write_json(
                &mut writer,
                &json!({ "payload": "x".repeat(MAX_PENDING_BYTES) }),
            )
            .await
        );
    }
}
