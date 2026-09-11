//! Event bounds, normalization, presentation, and wire records.

pub mod socket;

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use oga_domain::{
    BatchEvent, BatchTask, EventKind, EventLevel, EventPhase, EventSource, MAX_EVENT_OUTCOME,
    MAX_EVENT_TITLE, OUTCOME_STATES, PresentationType, Provider, Task, TaskEvent,
    TaskEventPresentation, TaskEventView, TaskState, WireTaskOutcome,
};
use oga_store::{Store, StoreError};
use serde_json::{Map, Value};
use tokio::{sync::watch, time::sleep};

pub use socket::{
    EventSocketHandle, EventSocketOptions, SocketError, event_socket_path, start_event_socket,
};

pub const MAX_EVENT_PAYLOAD_BYTES: usize = 8 * 1024;
const MAX_REASONING_PAYLOAD_BYTES: usize = 32 * 1024;
const MIN_TRUNCATABLE_BYTES: usize = 256;
const MARKER_RESERVE_BYTES: usize = 80;
pub const RETRY_REPEAT_THRESHOLD: usize = 3;

/// Maximum number of event keys retained by one feed.
pub const MAX_TRACKED_EVENTS: usize = 4_096;
const DEFAULT_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A shared, lossless hint that the durable event log may have advanced.
///
/// The feed retains only event ids and task ids. Readers still load the rows
/// from SQLite, so a coalesced or evicted hint can never discard history.
#[derive(Clone)]
pub struct EventFeed {
    store: Arc<Store>,
    poll_interval: Duration,
    state: Arc<FeedState>,
}

struct FeedState {
    updates: watch::Sender<u64>,
    snapshot: Mutex<FeedSnapshot>,
    metrics: FeedMetrics,
    users: AtomicUsize,
    polling: AtomicBool,
}

#[derive(Debug, Clone, Copy, Default)]
/// Counters for the feed's durable polling and bounded key window.
pub struct EventFeedStats {
    pub database_queries: u64,
    pub poll_queries: u64,
    pub observed_events: u64,
    pub notifications: u64,
    pub retained_events: usize,
}

#[derive(Default)]
struct FeedMetrics {
    database_queries: AtomicU64,
    poll_queries: AtomicU64,
    observed_events: AtomicU64,
    notifications: AtomicU64,
}

#[derive(Default)]
struct FeedSnapshot {
    head: i64,
    generation: u64,
    events: VecDeque<TrackedEvent>,
    error: Option<String>,
}

struct TrackedEvent {
    id: i64,
    task_id: String,
}

/// Keeps the shared feed alive while a stream or waiter uses it.
pub struct EventFeedGuard {
    state: Arc<FeedState>,
}

impl Drop for EventFeedGuard {
    fn drop(&mut self) {
        self.state.users.fetch_sub(1, Ordering::AcqRel);
    }
}

impl EventFeed {
    pub fn new(store: Arc<Store>) -> Self {
        Self::with_poll_interval(store, DEFAULT_EVENT_POLL_INTERVAL)
    }

    pub fn with_poll_interval(store: Arc<Store>, poll_interval: Duration) -> Self {
        let (updates, _) = watch::channel(0);
        Self {
            store,
            poll_interval: poll_interval.max(Duration::from_millis(1)),
            state: Arc::new(FeedState {
                updates,
                snapshot: Mutex::new(FeedSnapshot::default()),
                metrics: FeedMetrics::default(),
                users: AtomicUsize::new(0),
                polling: AtomicBool::new(false),
            }),
        }
    }

    /// Retain one bounded change feed user and start its shared poller.
    pub fn retain(&self) -> Result<EventFeedGuard, StoreError> {
        self.state.users.fetch_add(1, Ordering::AcqRel);
        if self
            .state
            .polling
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.state
                .metrics
                .database_queries
                .fetch_add(1, Ordering::Relaxed);
            let head = match latest_event_id(&self.store, &[]) {
                Ok(head) => head,
                Err(error) => {
                    self.state.users.fetch_sub(1, Ordering::AcqRel);
                    self.state.polling.store(false, Ordering::Release);
                    return Err(error);
                }
            };
            let Ok(mut snapshot) = self.state.snapshot.lock() else {
                self.state.users.fetch_sub(1, Ordering::AcqRel);
                self.state.polling.store(false, Ordering::Release);
                return Err(StoreError::Refusal("event feed lock poisoned".into()));
            };
            snapshot.head = head;
            snapshot.generation = 0;
            snapshot.events.clear();
            snapshot.error = None;
            let state = self.state.clone();
            let store = self.store.clone();
            let poll_interval = self.poll_interval;
            tokio::spawn(async move {
                poll_changes(store, state, poll_interval).await;
            });
        }
        Ok(EventFeedGuard {
            state: self.state.clone(),
        })
    }

    /// Waits for a durable event after `after` that matches `task_ids`.
    ///
    /// The shared poller wakes all potentially interested readers once per
    /// observed batch. A bounded task-id window avoids unrelated SQLite
    /// existence queries; an unknown window falls back to the durable check.
    pub async fn wait_for_change(
        &self,
        after: i64,
        task_ids: &[String],
        wait: Duration,
    ) -> Result<bool, StoreError> {
        if wait.is_zero() {
            return self.has_events_after(after, task_ids);
        }

        let mut updates = self.state.updates.subscribe();
        let _guard = self.retain()?;
        let generation = *updates.borrow_and_update();
        let initial = {
            let snapshot = self
                .state
                .snapshot
                .lock()
                .map_err(|_| StoreError::Refusal("event feed lock poisoned".into()))?;
            if let Some(error) = &snapshot.error {
                return Err(StoreError::Refusal(error.clone()));
            }
            if snapshot.head <= after {
                Some(false)
            } else if generation == 0 {
                if task_ids.is_empty() {
                    Some(true)
                } else {
                    None
                }
            } else {
                relevant_change(&snapshot, generation, after, task_ids)
            }
        };
        if initial == Some(true) || (initial.is_none() && self.has_events_after(after, task_ids)?) {
            return Ok(true);
        }

        let deadline = Instant::now() + wait;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            if tokio::time::timeout(remaining, updates.changed())
                .await
                .is_err()
            {
                return Ok(false);
            }
            let generation = *updates.borrow_and_update();
            let snapshot = self
                .state
                .snapshot
                .lock()
                .map_err(|_| StoreError::Refusal("event feed lock poisoned".into()))?;
            if let Some(error) = &snapshot.error {
                return Err(StoreError::Refusal(error.clone()));
            }
            let relevant = relevant_change(&snapshot, generation, after, task_ids);
            drop(snapshot);
            match relevant {
                Some(true) => return Ok(true),
                Some(false) => {}
                None => {
                    if self.has_events_after(after, task_ids)? {
                        return Ok(true);
                    }
                }
            }
        }
    }

    pub fn latest_event_id(&self, task_ids: &[String]) -> Result<i64, StoreError> {
        aggregate_event_id(&self.store, "MAX", task_ids)
    }

    pub fn oldest_event_id(&self, task_ids: &[String]) -> Result<i64, StoreError> {
        aggregate_event_id(&self.store, "MIN", task_ids)
    }

    /// Returns counters collected since this feed was created.
    pub fn stats(&self) -> EventFeedStats {
        let retained_events = self
            .state
            .snapshot
            .lock()
            .map_or(0, |snapshot| snapshot.events.len());
        EventFeedStats {
            database_queries: self.state.metrics.database_queries.load(Ordering::Relaxed),
            poll_queries: self.state.metrics.poll_queries.load(Ordering::Relaxed),
            observed_events: self.state.metrics.observed_events.load(Ordering::Relaxed),
            notifications: self.state.metrics.notifications.load(Ordering::Relaxed),
            retained_events,
        }
    }

    fn has_events_after(&self, after: i64, task_ids: &[String]) -> Result<bool, StoreError> {
        self.state
            .metrics
            .database_queries
            .fetch_add(1, Ordering::Relaxed);
        self.store.with_connection(|connection| {
            let task_filter = task_clause(task_ids, "task_id");
            let sql = format!("SELECT EXISTS(SELECT 1 FROM task_events WHERE id > ?{task_filter})");
            let mut values: Vec<&dyn rusqlite::ToSql> = vec![&after];
            values.extend(task_ids.iter().map(|id| id as &dyn rusqlite::ToSql));
            Ok(connection.query_row(&sql, rusqlite::params_from_iter(values), |row| row.get(0))?)
        })
    }
}

async fn poll_changes(store: Arc<Store>, state: Arc<FeedState>, poll_interval: Duration) {
    let mut after = state
        .snapshot
        .lock()
        .ok()
        .map_or(0, |snapshot| snapshot.head);
    loop {
        if state.users.load(Ordering::Acquire) == 0 {
            state.polling.store(false, Ordering::Release);
            restart_poller_if_needed(&store, &state, poll_interval);
            return;
        }

        state
            .metrics
            .database_queries
            .fetch_add(1, Ordering::Relaxed);
        state.metrics.poll_queries.fetch_add(1, Ordering::Relaxed);
        match list_event_keys(&store, after, MAX_TRACKED_EVENTS + 1) {
            Ok(events) => {
                if let Some(last) = events.last() {
                    state
                        .metrics
                        .observed_events
                        .fetch_add(events.len() as u64, Ordering::Relaxed);
                    after = last.0;
                    if let Ok(mut snapshot) = state.snapshot.lock() {
                        snapshot.head = after;
                        for (id, task_id) in events {
                            snapshot.events.push_back(TrackedEvent { id, task_id });
                        }
                        while snapshot.events.len() > MAX_TRACKED_EVENTS {
                            snapshot.events.pop_front();
                        }
                        snapshot.generation = snapshot.generation.wrapping_add(1);
                        snapshot.error = None;
                        state.metrics.notifications.fetch_add(1, Ordering::Relaxed);
                        let _ = state.updates.send(snapshot.generation);
                    }
                }
            }
            Err(error) => {
                if let Ok(mut snapshot) = state.snapshot.lock() {
                    snapshot.error = Some(error.to_string());
                    snapshot.generation = snapshot.generation.wrapping_add(1);
                    let _ = state.updates.send(snapshot.generation);
                }
                state.polling.store(false, Ordering::Release);
                return;
            }
        }
        sleep(poll_interval).await;
    }
}

fn restart_poller_if_needed(store: &Arc<Store>, state: &Arc<FeedState>, poll_interval: Duration) {
    if state.users.load(Ordering::Acquire) == 0
        || state
            .polling
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }
    tokio::spawn(poll_changes(store.clone(), state.clone(), poll_interval));
}

fn relevant_change(
    snapshot: &FeedSnapshot,
    generation: u64,
    after: i64,
    task_ids: &[String],
) -> Option<bool> {
    if generation == 0 || snapshot.head <= after {
        return Some(false);
    }
    let first = snapshot.events.front()?.id;
    if after < first.saturating_sub(1) {
        return None;
    }
    if task_ids.is_empty() {
        return Some(true);
    }
    Some(
        snapshot.events.iter().any(|event| {
            event.id > after && task_ids.iter().any(|task_id| task_id == &event.task_id)
        }),
    )
}

fn list_event_keys(
    store: &Store,
    after: i64,
    limit: usize,
) -> Result<Vec<(i64, String)>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection
            .prepare("SELECT id,task_id FROM task_events WHERE id > ? ORDER BY id LIMIT ?")?;
        Ok(statement
            .query_map(rusqlite::params![after, limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?)
    })
}

fn latest_event_id(store: &Store, task_ids: &[String]) -> Result<i64, StoreError> {
    aggregate_event_id(store, "MAX", task_ids)
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
        let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(task_ids.len());
        values.extend(task_ids.iter().map(|id| id as &dyn rusqlite::ToSql));
        Ok(connection.query_row(&sql, rusqlite::params_from_iter(values), |row| row.get(0))?)
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

fn byte_len(value: &str) -> usize {
    value.len()
}

fn truncate_bytes(value: &str, max: usize) -> String {
    let mut end = max.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn collect_leaves(value: &Value, path: &mut Vec<String>, leaves: &mut Vec<(Vec<String>, usize)>) {
    match value {
        Value::String(text) => leaves.push((path.clone(), byte_len(text))),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                path.push(index.to_string());
                collect_leaves(item, path, leaves);
                path.pop();
            }
        }
        Value::Object(items) => {
            for (key, item) in items {
                path.push(key.clone());
                collect_leaves(item, path, leaves);
                path.pop();
            }
        }
        _ => {}
    }
}

fn value_at<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(root, |value, segment| match value {
        Value::Object(items) => items.get(segment),
        Value::Array(items) => segment
            .parse::<usize>()
            .ok()
            .and_then(|index| items.get(index)),
        _ => None,
    })
}

fn value_at_mut<'a>(root: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    let mut value = root;
    for segment in path {
        value = match value {
            Value::Object(items) => items.get_mut(segment)?,
            Value::Array(items) => items.get_mut(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(value)
}

/// Whether a leaf holds the model's own reasoning rather than payload plumbing.
/// The key alone identifies it across providers: Claude and Antigravity write
/// `thinking`, Codex and the OpenCode part shape write `reasoning`.
fn is_reasoning_leaf(path: &[String]) -> bool {
    matches!(
        path.last().map(String::as_str),
        Some("thinking" | "reasoning")
    )
}

/// Bounds free-text leaves while retaining structural keys and short values.
///
/// Reasoning text gets a wider budget and is the last thing trimmed. A single
/// extended-thinking block routinely runs past the ordinary cap on its own, and
/// it ships alongside a signature blob the reader can do nothing with — under
/// one shared cap the signature would survive and the words would not.
pub fn bound_event_payload(payload: &Map<String, Value>) -> Map<String, Value> {
    let size = serde_json::to_vec(payload).unwrap().len();
    if size <= MAX_EVENT_PAYLOAD_BYTES {
        return payload.clone();
    }

    let mut bounded = Value::Object(payload.clone());
    let mut leaves = Vec::new();
    collect_leaves(&bounded, &mut Vec::new(), &mut leaves);
    let budget = if leaves.iter().any(|(path, _)| is_reasoning_leaf(path)) {
        MAX_REASONING_PAYLOAD_BYTES
    } else {
        MAX_EVENT_PAYLOAD_BYTES
    };
    if size <= budget {
        return payload.clone();
    }
    leaves.sort_by_key(|(path, bytes)| (is_reasoning_leaf(path), std::cmp::Reverse(*bytes)));
    let mut remaining = size;
    for (path, original_bytes) in leaves {
        if remaining <= budget || original_bytes < MIN_TRUNCATABLE_BYTES {
            continue;
        }
        let Some(Value::String(value)) = value_at(&bounded, &path) else {
            continue;
        };
        let overshoot = remaining - budget;
        let keep = original_bytes.saturating_sub(overshoot + MARKER_RESERVE_BYTES);
        if keep >= original_bytes || keep < MIN_TRUNCATABLE_BYTES / 2 {
            continue;
        }
        let kept = truncate_bytes(value, keep);
        let replacement = format!(
            "{} …[truncated: kept {} of {} bytes]",
            kept,
            byte_len(&kept),
            original_bytes
        );
        let replacement_bytes = byte_len(&replacement);
        if let Some(slot) = value_at_mut(&mut bounded, &path) {
            *slot = Value::String(replacement);
            remaining = remaining - original_bytes + replacement_bytes;
        }
    }
    match bounded {
        Value::Object(items) => items,
        _ => unreachable!(),
    }
}

fn cap(value: &str, limit: usize) -> (String, bool) {
    if value.len() <= limit {
        (value.to_owned(), false)
    } else {
        (truncate_bytes(value, limit), true)
    }
}

/// Converts task completion fields into the compact watch-socket outcome.
pub fn wire_task_outcome(task: &Task) -> WireTaskOutcome {
    let (text, code) = match task.state {
        TaskState::Completed => (task.tldr.as_deref(), None),
        TaskState::NeedsInput => (task.question.as_deref(), None),
        TaskState::Failed | TaskState::Blocked => (
            task.error.as_deref().or(task
                .completion
                .as_ref()
                .and_then(|completion| completion.reason.as_deref())),
            task.completion.as_ref().map(|completion| completion.code),
        ),
        TaskState::Cancelled => (
            task.error.as_deref().or(task
                .completion
                .as_ref()
                .and_then(|completion| completion.reason.as_deref())),
            None,
        ),
        _ => (None, None),
    };
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return WireTaskOutcome {
            text: None,
            code,
            truncated: false,
            more: matches!(
                task.state,
                TaskState::Completed
                    | TaskState::NeedsInput
                    | TaskState::Failed
                    | TaskState::Blocked
            ),
        };
    };
    let (text, truncated) = cap(text, MAX_EVENT_OUTCOME);
    WireTaskOutcome {
        text: Some(text),
        code,
        truncated,
        more: false,
    }
}

pub fn event_to_batch(event: &oga_domain::WaitedTaskEvent, task: Option<&Task>) -> BatchEvent {
    let title = task
        .and_then(|task| task.title.as_deref())
        .map(|title| cap(title, MAX_EVENT_TITLE));
    let outcome = task
        .filter(|_| OUTCOME_STATES.contains(&event.state))
        .map(wire_task_outcome);
    let truncated = title.as_ref().is_some_and(|(_, cut)| *cut)
        || outcome.as_ref().is_some_and(|outcome| outcome.truncated);
    BatchEvent {
        id: event.id,
        task_id: event.task_id.clone(),
        kind: event.kind,
        minor: event.minor,
        state: event.state,
        at: event.at.clone(),
        summary: event.summary.clone(),
        title: title.map(|(text, _)| text).filter(|text| !text.is_empty()),
        outcome: outcome.as_ref().and_then(|outcome| outcome.text.clone()),
        code: outcome.as_ref().and_then(|outcome| outcome.code),
        truncated: truncated.then_some(true),
        more: outcome
            .as_ref()
            .and_then(|outcome| outcome.more.then_some(true)),
    }
}

pub fn task_to_batch(task: &Task) -> BatchTask {
    let title = task
        .title
        .as_deref()
        .map(|title| cap(title, MAX_EVENT_TITLE));
    let question = task
        .question
        .as_deref()
        .map(|text| cap(text, MAX_EVENT_OUTCOME));
    let error = task
        .error
        .as_deref()
        .map(|text| cap(text, MAX_EVENT_OUTCOME));
    let outcome = OUTCOME_STATES
        .contains(&task.state)
        .then(|| wire_task_outcome(task));
    let truncated = title.as_ref().is_some_and(|(_, cut)| *cut)
        || question.as_ref().is_some_and(|(_, cut)| *cut)
        || error.as_ref().is_some_and(|(_, cut)| *cut)
        || outcome.as_ref().is_some_and(|outcome| outcome.truncated);
    BatchTask {
        id: task.id.clone(),
        state: task.state,
        question: question.map(|value| value.0),
        error: error.map(|value| value.0),
        title: title.map(|value| value.0),
        archived_at: task.archived_at.clone(),
        tldr: (task.state == TaskState::Completed)
            .then(|| outcome.as_ref().and_then(|value| value.text.clone()))
            .flatten(),
        code: outcome.as_ref().and_then(|value| value.code),
        truncated: truncated.then_some(true),
        more: outcome.and_then(|value| value.more.then_some(true)),
        duration_ms: task.duration_ms,
    }
}

/// Upgrades the third and later retry for one target into a visible failure.
pub fn mark_repeated_retries(views: Vec<TaskEventView>) -> Vec<TaskEventView> {
    let mut misses = HashMap::<String, usize>::new();
    views
        .into_iter()
        .map(|view| {
            if view.kind != EventKind::Retry {
                return view;
            }
            let key = view
                .presentation
                .as_ref()
                .and_then(|presentation| presentation.path.clone())
                .unwrap_or_else(|| view.title.clone());
            let count = misses.entry(key).or_insert(0);
            *count += 1;
            if *count < RETRY_REPEAT_THRESHOLD {
                return view;
            }
            let detail = match view.detail {
                Some(detail) => format!("{} · {} failed attempt", detail, ordinal(*count)),
                None => format!("{} failed attempt", ordinal(*count)),
            };
            TaskEventView {
                kind: EventKind::Error,
                phase: EventPhase::Failed,
                detail: Some(detail),
                ..view
            }
        })
        .collect()
}

fn ordinal(value: usize) -> String {
    let suffix = if value % 10 == 1 && value % 100 != 11 {
        "st"
    } else if value % 10 == 2 && value % 100 != 12 {
        "nd"
    } else if value % 10 == 3 && value % 100 != 13 {
        "rd"
    } else {
        "th"
    };
    format!("{value}{suffix}")
}

/// Produces a provider-neutral view for events whose payload already carries a
/// normalized `kind`, `phase`, and `title`. Provider adapters can enrich it.
pub fn event_view(event: &TaskEvent, provider: Provider) -> TaskEventView {
    let hook_name = (event.kind == "agent.hook")
        .then(|| {
            text_value(event.payload.get("hook_event_name"))
                .or_else(|| text_value(event.payload.get("hookEventName")))
        })
        .flatten();
    let hook_tool = (event.kind == "agent.hook")
        .then(|| {
            text_value(event.payload.get("tool_name"))
                .or_else(|| text_value(event.payload.get("toolName")))
        })
        .flatten();
    let hook_input = hook_tool.and_then(|_| hook_tool_input(&event.payload));
    let hook_action = match (hook_name, hook_tool) {
        (Some(name), Some(tool)) if name.contains("ToolUse") => {
            Some((tool, hook_phase(name, event.state)))
        }
        _ => None,
    };
    let hook_view_presentation = hook_action.and_then(|_| hook_presentation(&event.payload));
    let kind = event
        .payload
        .get("kind")
        .and_then(Value::as_str)
        .and_then(parse_kind)
        .or_else(|| hook_view_presentation.as_ref().map(presentation_kind))
        .unwrap_or_else(|| default_kind(&event.kind, &event.payload));
    let phase = event
        .payload
        .get("phase")
        .and_then(Value::as_str)
        .and_then(parse_phase)
        .unwrap_or_else(|| {
            hook_action.map(|(_, phase)| phase).unwrap_or_else(|| {
                if event.kind == "agent.hook" && hook_error_detail(&event.payload).is_some() {
                    EventPhase::Failed
                } else {
                    event_phase(&event.kind, event.state, &event.payload)
                }
            })
        });
    let title = text_value(event.payload.get("title"))
        .map(str::to_owned)
        .or_else(|| hook_tool.map(|tool| tool_title_with_input(tool, hook_input)))
        .unwrap_or_else(|| event_title(&event.kind, &event.payload));
    let action_id =
        hook_action.and_then(|_| tree_value(&event.payload, &["tool_use_id", "toolUseId"]));
    let action_fields = hook_action.map(|(tool, phase)| {
        let complete = matches!(phase, EventPhase::Completed | EventPhase::Failed);
        let presentation = hook_presentation(&event.payload);
        let target = presentation_target(presentation.as_ref()).or_else(|| Some(tool_title(tool)));
        (
            Some(tool_verb_with_input(tool, hook_input, complete)),
            target,
            Some(action_id.clone()),
            Some(complete),
        )
    });
    let raw_text =
        (!event.payload.is_empty()).then(|| serde_json::to_string_pretty(&event.payload).unwrap());
    if event.kind.starts_with("agent.")
        && let Some(view) = provider_event_view(event, provider, raw_text.clone())
    {
        return view;
    }
    let lifecycle_detail = hook_error_detail(&event.payload)
        .or_else(|| {
            hook_action
                .and_then(|_| presentation_detail(hook_presentation(&event.payload).as_ref()))
        })
        .or_else(|| hook_lifecycle_detail(&event.payload))
        .or_else(|| event_detail(&event.kind, &event.payload));
    let hook_result = hook_view_presentation
        .as_ref()
        .and_then(|presentation| presentation.outcome.clone());
    TaskEventView {
        id: event.id,
        task_id: event.task_id.clone(),
        source: EventSource::from_event_type(&event.kind, provider),
        event_type: event.kind.clone(),
        kind,
        phase,
        title: named(title),
        detail: (event.kind != "agent.hook"
            || event.payload.contains_key("tool_name")
            || hook_lifecycle_title(&event.payload).is_some())
        .then_some(lifecycle_detail)
        .flatten(),
        verb: action_fields.as_ref().and_then(|fields| fields.0.clone()),
        target: action_fields.as_ref().and_then(|fields| fields.1.clone()),
        result: hook_result,
        presentation: hook_view_presentation,
        raw_text,
        created_at: event.created_at.clone(),
        parent_action_id: None,
        turn_id: event.turn_id,
        action_id: action_id.clone(),
        source_id: action_fields
            .as_ref()
            .and_then(|fields| fields.2.clone())
            .or_else(|| action_id.as_ref().map(|id| Some(id.clone()))),
        complete: action_fields.as_ref().and_then(|fields| fields.3),
        minor: event
            .payload
            .get("minor")
            .and_then(Value::as_bool)
            .or_else(|| is_minor_event(&event.kind, &event.payload)),
    }
}

fn provider_event_view(
    event: &TaskEvent,
    provider: Provider,
    raw_text: Option<String>,
) -> Option<TaskEventView> {
    let payload = &event.payload;
    let event_type = text_value(payload.get("type"));
    if let Some("item.started" | "item.updated" | "item.completed") = event_type {
        return item_event_view(event, provider, payload, raw_text);
    }
    if event_type == Some("error") {
        return Some(agent_error_view(event, provider, payload, raw_text));
    }
    match provider {
        Provider::Claude => match event_type {
            Some("system") => claude_system_view(event, provider, payload, raw_text),
            Some("rate_limit_event") => {
                Some(claude_rate_limit_view(event, provider, payload, raw_text))
            }
            Some("assistant") => claude_assistant_view(event, provider, payload, raw_text),
            Some("result") => {
                let usage = payload.get("usage").and_then(Value::as_object);
                let tokens_in =
                    number_u64(usage.and_then(|value| value.get("input_tokens"))).map(|value| {
                        value
                            + number_u64(
                                usage.and_then(|value| value.get("cache_creation_input_tokens")),
                            )
                            .unwrap_or_default()
                    });
                let tokens_out = number_u64(usage.and_then(|value| value.get("output_tokens")));
                let cost = number_f64(payload.get("total_cost_usd"));
                let turns = number_u64(payload.get("num_turns"));
                let mut presentation = usage_presentation();
                presentation.cost_usd = cost;
                presentation.turns = turns;
                presentation.tokens_in = tokens_in;
                presentation.tokens_out = tokens_out;
                let detail = [
                    cost.map(format_cost),
                    turns.map(|value| {
                        format!("{} turn{}", value, if value == 1 { "" } else { "s" })
                    }),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                Some(provider_view(
                    event,
                    provider,
                    EventKind::Usage,
                    EventPhase::Completed,
                    "Run summary",
                    ProviderViewOptions {
                        detail: (!detail.is_empty()).then_some(detail),
                        presentation: Some(presentation),
                        minor: None,
                        raw_text,
                    },
                ))
            }
            _ => None,
        },
        Provider::Codex => match event_type {
            Some("thread.started") => Some(provider_view(
                event,
                provider,
                EventKind::Lifecycle,
                EventPhase::Info,
                "Session started",
                ProviderViewOptions {
                    detail: payload
                        .get("thread_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    presentation: None,
                    minor: Some(true),
                    raw_text,
                },
            )),
            Some("turn.completed") => {
                let usage = payload.get("usage").and_then(Value::as_object);
                let cached = number_u64(usage.and_then(|value| value.get("cached_input_tokens")));
                let input = number_u64(usage.and_then(|value| value.get("input_tokens")))
                    .map(|value| value.saturating_sub(cached.unwrap_or_default()));
                let output = number_u64(usage.and_then(|value| value.get("output_tokens")));
                let mut presentation = usage_presentation();
                presentation.tokens_in = input;
                presentation.tokens_cached = cached.filter(|value| *value > 0);
                presentation.tokens_out = output;
                presentation.tokens_thinking =
                    number_u64(usage.and_then(|value| value.get("reasoning_output_tokens")))
                        .filter(|value| *value > 0);
                let detail = [
                    output.map(|value| format_count(value) + " tokens out"),
                    cached
                        .filter(|value| *value > 0)
                        .map(|value| format_count(value) + " cached"),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                Some(provider_view(
                    event,
                    provider,
                    EventKind::Usage,
                    EventPhase::Completed,
                    "Turn completed",
                    ProviderViewOptions {
                        detail: (!detail.is_empty()).then_some(detail),
                        presentation: Some(presentation),
                        minor: None,
                        raw_text,
                    },
                ))
            }
            // A turn-level failure (API limits, auth) is not tied to any item
            // id; it floats as its own row rather than pairing with a started
            // event that never happened.
            Some("turn.failed") => {
                let message = payload
                    .get("error")
                    .and_then(Value::as_object)
                    .and_then(|error| text_value(error.get("message")))
                    .map(str::to_owned);
                let mut view = provider_view(
                    event,
                    provider,
                    EventKind::Error,
                    EventPhase::Failed,
                    "Error",
                    ProviderViewOptions {
                        detail: message.clone(),
                        presentation: None,
                        minor: None,
                        raw_text,
                    },
                );
                view.verb = Some("Failed".to_owned());
                view.target = message.as_deref().map(error_target);
                view.complete = Some(true);
                Some(view)
            }
            _ => None,
        },
        Provider::OpenCode | Provider::OpenCode2 => match event_type {
            Some("step_start") => Some(provider_view(
                event,
                provider,
                EventKind::Lifecycle,
                EventPhase::Started,
                "Step started",
                ProviderViewOptions {
                    detail: None,
                    presentation: None,
                    minor: Some(true),
                    raw_text,
                },
            )),
            Some("step_finish") => {
                let part = payload.get("part").and_then(Value::as_object);
                let tokens = part
                    .and_then(|value| value.get("tokens"))
                    .and_then(Value::as_object);
                let input = number_u64(tokens.and_then(|value| value.get("input")));
                let output = number_u64(tokens.and_then(|value| value.get("output")));
                let mut presentation = usage_presentation();
                presentation.tokens_in = input;
                presentation.tokens_out = output;
                presentation.tokens_thinking =
                    number_u64(tokens.and_then(|value| value.get("reasoning")))
                        .filter(|value| *value > 0);
                let reason = part
                    .and_then(|value| value.get("reason"))
                    .and_then(Value::as_str);
                let detail = [
                    reason.map(|value| value.replace('-', " ")),
                    output.map(|value| format_count(value) + " tokens out"),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                Some(provider_view(
                    event,
                    provider,
                    EventKind::Usage,
                    EventPhase::Completed,
                    "Step finished",
                    ProviderViewOptions {
                        detail: (!detail.is_empty()).then_some(detail),
                        presentation: Some(presentation),
                        minor: Some(true),
                        raw_text,
                    },
                ))
            }
            // OpenCode streams what the agent says as `text` parts and only
            // rarely as `message`; both carry the words in the same place.
            Some("message" | "text") => {
                let text = payload
                    .get("part")
                    .and_then(|value| value.get("text"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                text.map(|text| {
                    provider_view(
                        event,
                        provider,
                        EventKind::Message,
                        EventPhase::Info,
                        "Agent message",
                        ProviderViewOptions {
                            detail: Some(text.clone()),
                            presentation: Some(message_presentation(text)),
                            minor: None,
                            raw_text,
                        },
                    )
                })
            }
            // A reasoning part carries its words where a text part does.
            // Whether one arrives at all is the model's choice; no run flag
            // asks for them.
            Some("reasoning") => Some(reasoning_view(
                event,
                provider,
                payload
                    .get("part")
                    .and_then(|value| text_value(value.get("text")))
                    .map(str::to_owned),
                raw_text,
            )),
            Some("tool_use") => opencode_tool_view(event, provider, payload, raw_text),
            _ => None,
        },
        Provider::Antigravity => {
            // Antigravity streams its own protocol dialogue as `agent.assistant`,
            // the exact event name Claude Code uses for its primary carrier —
            // but here it only echoes work the `step_update.tool`/`agent_response`
            // rows already cover. Rendering it too would duplicate every tool
            // call and response on its own row.
            if event_type == Some("assistant") {
                return Some(provider_view(
                    event,
                    provider,
                    EventKind::Raw,
                    EventPhase::Info,
                    "Assistant protocol message",
                    ProviderViewOptions {
                        detail: None,
                        presentation: None,
                        minor: Some(true),
                        raw_text,
                    },
                ));
            }
            // `agent.result` (top-level) is the provider's own run outcome, distinct
            // from the `result` sub-kind nested inside `agent.event`. A clean run
            // says nothing a reader needs; only a failure earns a row.
            if event.kind == "agent.result" {
                let is_error = payload
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !is_error {
                    return Some(provider_view(
                        event,
                        provider,
                        EventKind::Usage,
                        EventPhase::Completed,
                        "Run summary",
                        ProviderViewOptions {
                            detail: None,
                            presentation: None,
                            minor: Some(true),
                            raw_text,
                        },
                    ));
                }
                let reason = text_value(payload.get("terminal_reason")).map(str::to_owned);
                let status = payload.get("api_error_status").and_then(Value::as_u64);
                let detail = match (reason, status) {
                    (Some(reason), Some(status)) => Some(format!("{reason} (HTTP {status})")),
                    (Some(reason), None) => Some(reason),
                    (None, Some(status)) => Some(format!("HTTP {status}")),
                    (None, None) => None,
                };
                return Some(provider_view(
                    event,
                    provider,
                    EventKind::Error,
                    EventPhase::Failed,
                    "Provider error",
                    ProviderViewOptions {
                        detail,
                        presentation: None,
                        minor: None,
                        raw_text,
                    },
                ));
            }
            match payload.get("event").and_then(Value::as_str) {
                Some("init") => Some(provider_view(
                    event,
                    provider,
                    EventKind::Lifecycle,
                    EventPhase::Info,
                    "Session started",
                    ProviderViewOptions {
                        detail: payload
                            .get("init")
                            .and_then(|value| value.get("model"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        presentation: None,
                        minor: None,
                        raw_text,
                    },
                )),
                Some("result") => Some(provider_view(
                    event,
                    provider,
                    EventKind::Usage,
                    EventPhase::Completed,
                    "Run summary",
                    ProviderViewOptions {
                        detail: None,
                        presentation: Some(usage_presentation()),
                        minor: None,
                        raw_text,
                    },
                )),
                Some("step_update") => antigravity_step_view(event, provider, payload, raw_text),
                _ => None,
            }
        }
        Provider::Pi => {
            // `agent.event` is Pi's rare catch-all for provider-internal init
            // chatter (often Gemini session setup); it carries no fixed shape
            // worth parsing, so it stays a low-detail reference row.
            if event.kind == "agent.event" {
                return Some(provider_view(
                    event,
                    provider,
                    EventKind::Raw,
                    EventPhase::Info,
                    "Event",
                    ProviderViewOptions {
                        detail: None,
                        presentation: None,
                        minor: Some(true),
                        raw_text,
                    },
                ));
            }
            match event_type {
                Some("session") => Some(provider_view(
                    event,
                    provider,
                    EventKind::Lifecycle,
                    EventPhase::Info,
                    "Session started",
                    ProviderViewOptions {
                        detail: payload
                            .get("cwd")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        presentation: None,
                        minor: None,
                        raw_text,
                    },
                )),
                Some("message_start" | "message_update" | "message_end") => {
                    pi_message_view(event, provider, payload, raw_text)
                }
                // Turn and agent boundaries are markers, not work: the messages
                // and tool calls that happened during them already have rows.
                Some("turn_start") => Some(provider_view(
                    event,
                    provider,
                    EventKind::Lifecycle,
                    EventPhase::Info,
                    "Turn started",
                    ProviderViewOptions {
                        detail: None,
                        presentation: None,
                        minor: Some(true),
                        raw_text,
                    },
                )),
                Some("turn_end") => Some(provider_view(
                    event,
                    provider,
                    EventKind::Lifecycle,
                    EventPhase::Info,
                    "Turn ended",
                    ProviderViewOptions {
                        detail: None,
                        presentation: None,
                        minor: Some(true),
                        raw_text,
                    },
                )),
                Some("agent_start") => Some(provider_view(
                    event,
                    provider,
                    EventKind::Lifecycle,
                    EventPhase::Info,
                    "Agent started",
                    ProviderViewOptions {
                        detail: None,
                        presentation: None,
                        minor: Some(true),
                        raw_text,
                    },
                )),
                Some("agent_end") => Some(provider_view(
                    event,
                    provider,
                    EventKind::Lifecycle,
                    EventPhase::Info,
                    "Agent finished",
                    ProviderViewOptions {
                        detail: None,
                        presentation: None,
                        minor: Some(true),
                        raw_text,
                    },
                )),
                Some("agent_settled") => Some(provider_view(
                    event,
                    provider,
                    EventKind::Lifecycle,
                    EventPhase::Info,
                    "Agent settled",
                    ProviderViewOptions {
                        detail: None,
                        presentation: None,
                        minor: Some(true),
                        raw_text,
                    },
                )),
                Some("auto_retry_start") => {
                    let attempt = number_u64(payload.get("attempt"));
                    let max_attempts = number_u64(payload.get("maxAttempts"));
                    let error = text_value(payload.get("errorMessage"));
                    let detail = match (attempt, max_attempts, error) {
                        (Some(attempt), Some(max), Some(error)) => {
                            Some(format!("Attempt {attempt}/{max}: {error}"))
                        }
                        (Some(attempt), Some(max), None) => {
                            Some(format!("Attempt {attempt}/{max}"))
                        }
                        (_, _, Some(error)) => Some(error.to_owned()),
                        _ => None,
                    };
                    Some(provider_view(
                        event,
                        provider,
                        EventKind::Lifecycle,
                        EventPhase::Info,
                        "Retrying",
                        ProviderViewOptions {
                            detail,
                            presentation: None,
                            minor: Some(true),
                            raw_text,
                        },
                    ))
                }
                Some("auto_retry_end") => {
                    let succeeded = payload
                        .get("success")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    if succeeded {
                        Some(provider_view(
                            event,
                            provider,
                            EventKind::Lifecycle,
                            EventPhase::Completed,
                            "Retry succeeded",
                            ProviderViewOptions {
                                detail: None,
                                presentation: None,
                                minor: Some(true),
                                raw_text,
                            },
                        ))
                    } else {
                        let attempt = number_u64(payload.get("attempt"));
                        Some(provider_view(
                            event,
                            provider,
                            EventKind::Error,
                            EventPhase::Failed,
                            "Retry failed",
                            ProviderViewOptions {
                                detail: attempt.map(|value| format!("Attempt {value} failed")),
                                presentation: None,
                                minor: None,
                                raw_text,
                            },
                        ))
                    }
                }
                Some("tool_execution_start" | "tool_execution_update" | "tool_execution_end") => {
                    pi_tool_view(event, provider, payload, raw_text)
                }
                // Pi's rare raw tool-call shape shares opencode's `part.tool`
                // envelope byte for byte, `callID` included.
                Some("tool_use") => opencode_tool_view(event, provider, payload, raw_text),
                _ => None,
            }
        }
    }
}

/// Claude reports a subagent's run as a pair of `system` events sharing a
/// `task_id`: `task_started` names the work, `task_notification` says how it
/// ended. The trace pairs them so the sub-agent's own tool calls — the ones
/// carrying its `tool_use_id` as their parent — read as its work rather than
/// as the caller's.
pub const SUBAGENT_STARTED_TITLE: &str = "Subagent started";
pub const SUBAGENT_FINISHED_TITLE: &str = "Subagent finished";
/// One line for every drop, however many drops there were.
pub const ACTIVITY_SKIPPED_TITLE: &str = "Some activity was not recorded";
/// Shared by every provider's reasoning blocks: it is what lets the trace
/// recognise a run of them and fold it into one row.
pub const REASONING_TITLE: &str = "Thinking";

fn claude_system_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> Option<TaskEventView> {
    match text_value(payload.get("subtype"))? {
        "init" => Some(provider_view(
            event,
            provider,
            EventKind::Lifecycle,
            EventPhase::Info,
            "Session started",
            ProviderViewOptions {
                detail: text_value(payload.get("model")).map(str::to_owned),
                presentation: None,
                minor: None,
                raw_text,
            },
        )),
        // The counter is cumulative; the delta is what can be added up across a
        // run without counting the same tokens on every tick.
        "thinking_tokens" => number_u64(
            payload
                .get("estimated_tokens_delta")
                .or_else(|| payload.get("estimated_tokens")),
        )
        .map(|tokens| reasoning_tokens_view(event, provider, tokens, raw_text)),
        "task_started" => Some(provider_view(
            event,
            provider,
            EventKind::Lifecycle,
            EventPhase::Started,
            SUBAGENT_STARTED_TITLE,
            ProviderViewOptions {
                detail: text_value(payload.get("description")).map(str::to_owned),
                presentation: None,
                minor: None,
                raw_text,
            },
        )),
        "task_notification" => {
            let status = text_value(payload.get("status"));
            let failed = matches!(status, Some("failed" | "error" | "cancelled"));
            Some(provider_view(
                event,
                provider,
                EventKind::Lifecycle,
                if failed {
                    EventPhase::Failed
                } else {
                    EventPhase::Completed
                },
                SUBAGENT_FINISHED_TITLE,
                ProviderViewOptions {
                    // A clean finish adds nothing the group heading has not
                    // already said; anything else is the reason to look.
                    detail: status.filter(|value| *value != "completed").map(humanize),
                    presentation: None,
                    minor: None,
                    raw_text,
                },
            ))
        }
        "permission_denied" => {
            let tool = text_value(payload.get("tool_name")).map(tool_title);
            let reason = text_value(payload.get("decision_reason"))
                .or_else(|| text_value(payload.get("message")))
                .map(str::to_owned);
            let text = match (tool, reason) {
                (Some(tool), Some(reason)) => format!("{tool} — {reason}"),
                (Some(tool), None) => tool,
                (None, Some(reason)) => reason,
                (None, None) => "A tool call was not allowed to run".to_owned(),
            };
            Some(provider_view(
                event,
                provider,
                EventKind::Lifecycle,
                EventPhase::Info,
                "Permission needed",
                ProviderViewOptions {
                    detail: Some(text.clone()),
                    presentation: Some(signal_presentation(text, EventLevel::Warning)),
                    minor: Some(false),
                    raw_text,
                },
            ))
        }
        _ => None,
    }
}

/// The window numbers matter on the receipt, where a reader is already looking
/// at what the run cost. Repeating them mid-trace, once per turn, tells them
/// nothing they can act on.
fn claude_rate_limit_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> TaskEventView {
    provider_view(
        event,
        provider,
        EventKind::Lifecycle,
        EventPhase::Info,
        "Usage window",
        ProviderViewOptions {
            detail: rate_limit_detail(payload),
            presentation: None,
            minor: Some(true),
            raw_text,
        },
    )
}

fn claude_assistant_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> Option<TaskEventView> {
    let content = payload
        .get("message")
        .and_then(|value| value.get("content"))
        .and_then(Value::as_array)?;
    if let Some(text) = content.iter().find_map(|value| {
        (value.get("type").and_then(Value::as_str) == Some("text"))
            .then(|| value.get("text").and_then(Value::as_str))
            .flatten()
    }) {
        return Some(provider_view(
            event,
            provider,
            EventKind::Message,
            EventPhase::Info,
            "Agent message",
            ProviderViewOptions {
                detail: Some(text.to_owned()),
                presentation: Some(message_presentation(text.to_owned())),
                minor: None,
                raw_text,
            },
        ));
    }
    let Some(tool) = content
        .iter()
        .find(|value| value.get("type").and_then(Value::as_str) == Some("tool_use"))
    else {
        // What is left is extended thinking. Claude sends the words in
        // `thinking` when the account is allowed to see them and an opaque
        // `signature` alone when it is not, so the text is optional here and
        // the block still counts either way.
        let thinking = content
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
            .filter_map(|block| text_value(block.get("thinking")))
            .collect::<Vec<_>>();
        return Some(reasoning_view(
            event,
            provider,
            (!thinking.is_empty()).then(|| thinking.join("\n\n")),
            raw_text,
        ));
    };
    let name = text_value(tool.get("name"))?;
    let input = tool.get("input").and_then(Value::as_object)?;
    let presentation = tool_presentation(name, input, None);
    let action_id = text_value(tool.get("id")).map(str::to_owned);
    let mut view = provider_view(
        event,
        provider,
        presentation
            .as_ref()
            .map_or(EventKind::Tool, presentation_kind),
        EventPhase::Started,
        &tool_title_with_input(name, Some(input)),
        ProviderViewOptions {
            detail: presentation_detail(presentation.as_ref()),
            presentation: presentation.clone(),
            minor: None,
            raw_text,
        },
    );
    view.verb = Some(tool_verb_with_input(name, Some(input), false));
    view.target = presentation_target(presentation.as_ref());
    view.action_id = action_id.clone();
    view.source_id = Some(action_id);
    view.complete = Some(false);
    Some(view)
}

/// Codex reports work as items rather than tool calls. Each item type names its
/// own subject under its own key, and decides what the row is: a shell call, a
/// file edit, a search, a message.
fn item_event_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> Option<TaskEventView> {
    let item = payload.get("item")?.as_object()?;
    let item_type = text_value(item.get("type"))?;
    let event_type = text_value(payload.get("type")).unwrap_or_default();
    let complete = event_type == "item.completed";
    let status = text_value(item.get("status"));
    // The item id is what lets a `started`/`updated`/`completed` triple settle
    // into one row (see `foldActions` in the activity domain); without it every
    // phase of the same item would render as its own row.
    let item_id = string_value(item, &["id"]);

    if item_type == "agent_message" {
        let text = string_value(item, &["text"])?;
        let mut view = provider_view(
            event,
            provider,
            EventKind::Message,
            EventPhase::Info,
            "Agent message",
            ProviderViewOptions {
                detail: Some(text.clone()),
                presentation: Some(message_presentation(text)),
                minor: None,
                raw_text,
            },
        );
        view.verb = Some("Said".to_owned());
        view.complete = Some(true);
        view.action_id = item_id.clone();
        view.source_id = Some(item_id);
        return Some(view);
    }
    // Codex opens a reasoning item empty and fills it in on completion, and
    // puts the words under `summary` when the model returned a condensed
    // version rather than the raw chain.
    if item_type == "reasoning" {
        let text = string_value(item, &["text"]).or_else(|| {
            let parts = item
                .get("summary")
                .and_then(Value::as_array)?
                .iter()
                .filter_map(|entry| {
                    text_value(Some(entry)).or_else(|| text_value(entry.get("text")))
                })
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        });
        return Some(reasoning_view(event, provider, text, raw_text));
    }
    if item_type == "error" {
        let message = string_value(item, &["message", "error"]);
        let target = message.as_deref().map(error_target);
        let mut view = provider_view(
            event,
            provider,
            EventKind::Error,
            EventPhase::Failed,
            "Error",
            ProviderViewOptions {
                detail: message,
                presentation: None,
                minor: None,
                raw_text,
            },
        );
        view.verb = Some("Failed".to_owned());
        view.target = target;
        view.complete = Some(true);
        view.action_id = item_id.clone();
        view.source_id = Some(item_id);
        return Some(view);
    }

    let (title, verb, presentation) = match item_type {
        "command_execution" => {
            let command = string_value(item, &["command"])?;
            let exit_code = item.get("exit_code").and_then(Value::as_i64);
            // Codex has no dedicated search tool: `rg`/`grep`/`find` run as a
            // plain shell command, same as Claude's Bash tool and Pi's `bash`
            // tool call, so it shares their search-command detection instead
            // of always rendering as a generic command row.
            let search = search_command_subject(&command);
            let mut presentation = usage_presentation();
            match &search {
                Some(subject) => {
                    presentation.kind = PresentationType::Tool;
                    presentation.text = Some(subject.clone());
                }
                None => {
                    presentation.kind = PresentationType::Command;
                    presentation.text = Some(command_summary(&command));
                    presentation.command = Some(command);
                    presentation.exit_code = exit_code;
                }
            }
            // A non-zero exit is definitive for failure; the first non-blank
            // line of output is the only part of a long run worth showing. A
            // clean search reports its own match count, same as every other
            // provider's search row.
            presentation.outcome = match exit_code.filter(|code| *code != 0) {
                Some(code) => {
                    let first_line =
                        string_value(item, &["aggregated_output"]).and_then(|output| {
                            output
                                .lines()
                                .find(|line| !line.trim().is_empty())
                                .map(str::to_owned)
                        });
                    Some(match first_line {
                        Some(line) => format!("exit {code}: {}", cap(line.trim(), 120).0),
                        None => format!("exit {code}"),
                    })
                }
                None if search.is_some() => string_value(item, &["aggregated_output"])
                    .and_then(|output| search_outcome_from_text(&output)),
                None => None,
            };
            (
                tool_title_with_input("bash", Some(item)),
                tool_verb_with_input("bash", Some(item), complete),
                Some(presentation),
            )
        }
        "file_change" => {
            let changes = item.get("changes").and_then(Value::as_array)?;
            let paths = changes
                .iter()
                .filter_map(|change| text_value(change.get("path")))
                .collect::<Vec<_>>();
            let first = *paths.first()?;
            let kinds = changes
                .iter()
                .filter_map(|change| text_value(change.get("kind")))
                .collect::<Vec<_>>();
            let uniform_kind = kinds
                .first()
                .copied()
                .filter(|kind| kinds.iter().all(|other| other == kind));
            let mut presentation = file_presentation(first.to_owned());
            presentation.change = (paths.len() > 1).then(|| {
                let rest = paths.len() - 1;
                format!("{rest} more file{}", if rest == 1 { "" } else { "s" })
            });
            let title = if paths.len() == 1 {
                "Edit file".to_owned()
            } else {
                "Edit files".to_owned()
            };
            let verb = match uniform_kind {
                Some("add" | "create") => {
                    if complete {
                        "Created"
                    } else {
                        "Creating"
                    }
                }
                Some("delete" | "remove") => {
                    if complete {
                        "Deleted"
                    } else {
                        "Deleting"
                    }
                }
                _ => {
                    if complete {
                        "Edited"
                    } else {
                        "Editing"
                    }
                }
            }
            .to_owned();
            (title, verb, Some(presentation))
        }
        // The `query` field on the started event is a draft, often empty; the
        // completed event's `action.queries` is the real, possibly multi-query,
        // search plan and is what the row must show.
        "web_search" => {
            let queries = item
                .get("action")
                .and_then(Value::as_object)
                .and_then(|action| action.get("queries"))
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| text_value(Some(value)).map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut presentation = usage_presentation();
            presentation.kind = PresentationType::Tool;
            presentation.text = queries
                .first()
                .cloned()
                .or_else(|| string_value(item, &["query"]));
            presentation.outcome =
                (queries.len() > 1).then(|| format!("{} queries", queries.len()));
            (
                "Web search".to_owned(),
                tool_verb("websearch", complete),
                Some(presentation),
            )
        }
        "todo_list" => {
            let items = item.get("items").and_then(Value::as_array)?;
            let total = items.len() as u64;
            let completed_count = items
                .iter()
                .filter(|entry| entry.get("completed").and_then(Value::as_bool) == Some(true))
                .count() as u64;
            let mut presentation = usage_presentation();
            presentation.kind = PresentationType::Todo;
            presentation.total = Some(total);
            presentation.completed = Some(completed_count);
            presentation.text = Some(format!("{total} step{}", if total == 1 { "" } else { "s" }));
            presentation.outcome = items
                .iter()
                .find(|entry| entry.get("completed").and_then(Value::as_bool) != Some(true))
                .and_then(|entry| text_value(entry.get("text")))
                .map(str::to_owned)
                .or_else(|| (total > 0 && completed_count == total).then(|| "all done".to_owned()));
            // Distinct verbs per phase, not just started/completed: an
            // in-between `item.updated` is the agent checking off progress.
            let verb = match event_type {
                "item.started" => "Planned",
                "item.updated" => "Checked",
                _ => "Completed",
            }
            .to_owned();
            ("Todo list".to_owned(), verb, Some(presentation))
        }
        "mcp_tool_call" => {
            let tool = text_value(item.get("tool"))?;
            let server = string_value(item, &["server"]);
            let input = item.get("arguments").and_then(Value::as_object);
            let oga = oga_call(tool, input, server.as_deref());
            let mut presentation = oga
                .as_ref()
                .map(|call| oga_presentation(&call.operation, call.input, None))
                .unwrap_or_else(usage_presentation);
            if oga.is_none() {
                presentation.kind = PresentationType::Tool;
                presentation.text = Some(match &server {
                    Some(server) => format!("{server}/{tool}"),
                    None => tool.to_owned(),
                });
            }
            let output = item.get("result").and_then(mcp_result_text);
            presentation.outcome = oga
                .as_ref()
                .and_then(|call| {
                    output
                        .as_deref()
                        .and_then(|text| oga_result_summary(&call.operation, text))
                })
                .or_else(|| mcp_result_summary(item));
            let title = oga
                .as_ref()
                .map(|call| oga_tool_title(&call.operation, call.input))
                .unwrap_or_else(|| tool_title(tool));
            let verb = oga
                .as_ref()
                .map(|call| oga_tool_verb(&call.operation, call.input, complete))
                .unwrap_or_else(|| if complete { "Called" } else { "Calling" }.into());
            (title, verb, Some(presentation))
        }
        other => (
            tool_title(other),
            tool_verb(other, complete),
            tool_presentation(other, item, None),
        ),
    };

    let phase = match (status, complete) {
        (Some("failed" | "error"), _) => EventPhase::Failed,
        (_, true) => EventPhase::Completed,
        _ => EventPhase::Started,
    };
    let mut view = provider_view(
        event,
        provider,
        presentation
            .as_ref()
            .map_or(EventKind::Tool, presentation_kind),
        phase,
        &title,
        ProviderViewOptions {
            detail: presentation_detail(presentation.as_ref()),
            presentation: presentation.clone(),
            minor: None,
            raw_text,
        },
    );
    view.verb = Some(verb);
    view.target = presentation_target(presentation.as_ref());
    view.result = presentation
        .as_ref()
        .and_then(|value| value.outcome.clone());
    view.complete = Some(complete);
    view.action_id = item_id.clone();
    view.source_id = Some(item_id);
    Some(view)
}

/// The most useful phrase in a Codex error item's message: the words after the
/// last `: `, which is usually the underlying OS/permission reason rather than
/// the "Failed to ..." framing around it.
fn error_target(message: &str) -> String {
    let message = message.trim();
    let phrase = message
        .rsplit_once(": ")
        .map(|(_, after)| after)
        .unwrap_or(message);
    cap(phrase, 60).0
}

/// An MCP tool's result is opaque JSON/YAML/markdown/plain text nested under
/// `result.content[0].text`; the row shows a compact, bounded summary.
fn mcp_result_summary(item: &Map<String, Value>) -> Option<String> {
    let text = mcp_result_text(item.get("result")?)?;
    compact_output(&text)
}

fn opencode_tool_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> Option<TaskEventView> {
    let part = payload.get("part")?.as_object()?;
    let name = text_value(part.get("tool"))?;
    let state = part.get("state").and_then(Value::as_object);
    let input = state
        .and_then(|value| value.get("input"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let status = state.and_then(|value| text_value(value.get("status")));
    let complete = matches!(status, Some("completed" | "success" | "error" | "failed"));
    let phase = match status {
        Some("error" | "failed") => EventPhase::Failed,
        Some("completed" | "success") => EventPhase::Completed,
        Some("running" | "started" | "pending") => EventPhase::Started,
        _ if event.state == TaskState::Running => EventPhase::Started,
        _ => phase_for_state(event.state),
    };
    let mut presentation = tool_presentation(name, &input, state);
    // An edit row names the file it touched, not the strings it swapped —
    // both are too long to read inline and the diff itself is available
    // via the row's own expansion.
    if matches!(name.to_ascii_lowercase().as_str(), "edit" | "multiedit")
        && let Some(value) = presentation.as_mut()
    {
        value.change = None;
    }
    let target = presentation_target(presentation.as_ref());
    let output = state.and_then(|value| text_value(value.get("output")));
    let result = state
        .and_then(|value| text_value(value.get("error")))
        .map(str::to_owned)
        .or_else(|| {
            presentation
                .as_ref()
                .and_then(|value| value.outcome.clone())
        })
        .or_else(|| {
            // A file's contents and a skill's instructions are the body of the
            // call, not an account of it. Only output that reports what
            // happened belongs on the row.
            (!matches!(tool_family(name), ToolFamily::File | ToolFamily::Skill))
                .then(|| output.and_then(compact_output))
                .flatten()
        });
    let action_id = string_value(
        part,
        &[
            "callID",
            "call_id",
            "callId",
            "toolCallId",
            "tool_call_id",
            "tool_use_id",
            "toolUseId",
            "id",
        ],
    )
    .or_else(|| {
        tree_value(
            payload,
            &[
                "tool_use_id",
                "toolUseId",
                "toolCallId",
                "tool_call_id",
                "callId",
            ],
        )
    });
    let kind = presentation
        .as_ref()
        .map_or(EventKind::Tool, presentation_kind);
    let title = tool_title_with_input(name, Some(&input));
    let mut view = provider_view(
        event,
        provider,
        kind,
        phase,
        &title,
        ProviderViewOptions {
            detail: presentation_detail(presentation.as_ref()),
            presentation,
            minor: is_captured_output_read(name, Some(&input)).then_some(true),
            raw_text,
        },
    );
    view.verb = Some(tool_verb_with_input(name, Some(&input), complete));
    view.target = target;
    view.result = result;
    view.action_id = action_id.clone();
    view.source_id = Some(action_id);
    view.complete = Some(complete);
    let parent_keys = [
        "parent_tool_use_id",
        "parentToolUseId",
        "origin_activity_id",
        "originActivityId",
    ];
    view.parent_action_id =
        string_value(part, &parent_keys).or_else(|| tree_value(payload, &parent_keys));
    Some(view)
}

fn antigravity_step_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> Option<TaskEventView> {
    let step = payload.get("step_update")?.as_object()?;
    let step_index = number_u64(step.get("step_index"));
    match text_value(step.get("step_type")) {
        Some("tool") => antigravity_tool_step_view(event, provider, step, raw_text),
        Some("agent_response") => Some(antigravity_usage_step_view(
            event,
            provider,
            step,
            "Assistant responded",
            raw_text,
        )),
        Some("checkpoint") => Some(antigravity_usage_step_view(
            event,
            provider,
            step,
            "Checkpoint",
            raw_text,
        )),
        Some("user_input") => Some(provider_view(
            event,
            provider,
            EventKind::Message,
            EventPhase::Info,
            "User input",
            ProviderViewOptions {
                detail: step_index.map(|value| format!("step {value}")),
                presentation: None,
                minor: None,
                raw_text,
            },
        )),
        Some("error_message") => {
            let message = string_value(step, &["message", "text", "content"]);
            Some(provider_view(
                event,
                provider,
                EventKind::Error,
                EventPhase::Failed,
                "Error",
                ProviderViewOptions {
                    detail: message.or_else(|| step_index.map(|value| format!("step {value}"))),
                    presentation: None,
                    minor: None,
                    raw_text,
                },
            ))
        }
        Some("system_message") => Some(provider_view(
            event,
            provider,
            EventKind::Lifecycle,
            EventPhase::Info,
            "System message",
            ProviderViewOptions {
                detail: step_index.map(|value| format!("step {value}")),
                presentation: None,
                minor: Some(true),
                raw_text,
            },
        )),
        // `unknown` is exactly that to us too — provider-internal bookkeeping
        // with no documented shape. It stays behind the technical toggle
        // rather than surfacing an "Unknown step" row nobody can act on.
        _ => Some(provider_view(
            event,
            provider,
            EventKind::Raw,
            EventPhase::Info,
            "Unknown step",
            ProviderViewOptions {
                detail: step_index.map(|value| format!("step {value}")),
                presentation: None,
                minor: Some(true),
                raw_text,
            },
        )),
    }
}

fn antigravity_tool_step_view(
    event: &TaskEvent,
    provider: Provider,
    step: &Map<String, Value>,
    raw_text: Option<String>,
) -> Option<TaskEventView> {
    let name = text_value(step.get("tool_name").or_else(|| step.get("toolName")))?;
    let info = step.get("tool_info").and_then(Value::as_object);
    let input = info
        .and_then(|value| value.get("parameters"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let output = info.and_then(|value| text_value(value.get("output")));
    let state = text_value(step.get("state"));
    let phase = if state.is_some_and(|value| {
        value.eq_ignore_ascii_case("failed") || value.eq_ignore_ascii_case("error")
    }) {
        EventPhase::Failed
    } else if state.is_some_and(|value| {
        value.eq_ignore_ascii_case("done")
            || value.eq_ignore_ascii_case("completed")
            || value.eq_ignore_ascii_case("success")
    }) {
        EventPhase::Completed
    } else {
        EventPhase::Started
    };
    let complete = matches!(phase, EventPhase::Completed | EventPhase::Failed);
    let oga = oga_call(name, Some(&input), None);
    let mut presentation = tool_presentation(name, &input, None);
    if let Some(value) = presentation.as_mut() {
        if let Some(call) = &oga {
            value.outcome = output
                .and_then(|text| oga_result_summary(&call.operation, text))
                .or(value.outcome.clone());
        } else if matches!(tool_family(name), ToolFamily::Search) {
            value.outcome = output.and_then(search_outcome_from_text);
        }
    }
    let action_id = info
        .and_then(|value| {
            string_value(
                value,
                &["id", "call_id", "callId", "tool_use_id", "toolUseId"],
            )
        })
        .or_else(|| {
            string_value(
                step,
                &["id", "call_id", "callId", "tool_use_id", "toolUseId"],
            )
        });
    let kind = presentation
        .as_ref()
        .map_or(EventKind::Tool, presentation_kind);
    let target = presentation_target(presentation.as_ref());
    let result = presentation
        .as_ref()
        .and_then(|value| value.outcome.clone())
        .or_else(|| {
            (phase == EventPhase::Failed)
                .then(|| output.and_then(compact_output))
                .flatten()
        });
    let mut view = provider_view(
        event,
        provider,
        kind,
        phase,
        &tool_title_with_input(name, Some(&input)),
        ProviderViewOptions {
            detail: presentation_detail(presentation.as_ref()),
            presentation,
            minor: None,
            raw_text,
        },
    );
    view.verb = Some(tool_verb_with_input(name, Some(&input), complete));
    view.target = target;
    view.result = result;
    view.action_id = action_id.clone();
    view.source_id = Some(action_id);
    view.complete = Some(complete);
    Some(view)
}

/// `agent_response` and `checkpoint` steps both report a token-usage summary
/// for a slice of the conversation; only the title and which usage fields
/// matter differ, so both build off the same duration/token formatting.
fn antigravity_usage_step_view(
    event: &TaskEvent,
    provider: Provider,
    step: &Map<String, Value>,
    title: &str,
    raw_text: Option<String>,
) -> TaskEventView {
    let duration = number_f64(step.get("duration_seconds"));
    let usage = step.get("usage").and_then(Value::as_object);
    let input_tokens = number_u64(usage.and_then(|value| value.get("input_tokens")));
    let output_tokens = number_u64(usage.and_then(|value| value.get("output_tokens")));
    let cached_tokens = number_u64(usage.and_then(|value| value.get("cache_read_tokens")))
        .filter(|value| *value > 0);
    let mut presentation = usage_presentation();
    presentation.tokens_in = input_tokens;
    presentation.tokens_out = output_tokens;
    presentation.tokens_cached = cached_tokens;
    presentation.tokens_thinking =
        number_u64(usage.and_then(|value| value.get("thinking_tokens"))).filter(|value| *value > 0);
    presentation.duration_ms = duration.map(|value| value * 1000.0);
    let tokens = match (input_tokens, output_tokens) {
        (Some(input), Some(output)) => Some(format!(
            "{} → {} tokens",
            format_count(input),
            format_count(output)
        )),
        (None, Some(output)) => Some(format!("{} tokens out", format_count(output))),
        (Some(input), None) => Some(format!("{} tokens in", format_count(input))),
        (None, None) => None,
    };
    let detail = [
        duration.map(|value| format!("{value:.1}s")),
        tokens,
        cached_tokens.map(|value| format!("cached {}", format_count(value))),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    provider_view(
        event,
        provider,
        EventKind::Usage,
        EventPhase::Completed,
        title,
        ProviderViewOptions {
            detail: (!detail.is_empty()).then_some(detail),
            presentation: Some(presentation),
            minor: None,
            raw_text,
        },
    )
}

fn pi_tool_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> Option<TaskEventView> {
    let name = text_value(payload.get("toolName").or_else(|| payload.get("tool_name")))?;
    let input = payload
        .get("args")
        .or_else(|| payload.get("arguments"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let event_type = text_value(payload.get("type")).unwrap_or_default();
    let output = pi_result_text(payload);
    let failed = event_type == "tool_execution_end"
        && (payload.get("isError")).and_then(Value::as_bool) == Some(true);
    let phase = if failed {
        EventPhase::Failed
    } else if event_type == "tool_execution_end" {
        EventPhase::Completed
    } else {
        EventPhase::Started
    };
    let complete = matches!(phase, EventPhase::Completed | EventPhase::Failed);
    let oga = oga_call(name, Some(&input), None);
    let mut presentation = tool_presentation(name, &input, None);
    let output_outcome = output.and_then(search_outcome_from_text);
    let command_search =
        matches!(tool_family(name), ToolFamily::Command) && search_command(&input).is_some();
    if let Some(value) = presentation.as_mut() {
        if let Some(call) = &oga {
            value.outcome = output
                .and_then(|text| oga_result_summary(&call.operation, text))
                .or(value.outcome.clone());
        } else if matches!(tool_family(name), ToolFamily::Search) || command_search {
            value.outcome = output_outcome.clone();
        }
    } else if output_outcome.is_some() {
        let mut value = usage_presentation();
        value.kind = PresentationType::Tool;
        value.outcome = output_outcome.clone();
        presentation = Some(value);
    }
    let action_id = text_value(
        payload
            .get("toolCallId")
            .or_else(|| payload.get("tool_call_id")),
    )
    .map(str::to_owned);
    let kind = presentation
        .as_ref()
        .map_or(EventKind::Tool, presentation_kind);
    let target = presentation_target(presentation.as_ref());
    let result = failed.then(|| output.and_then(compact_output)).flatten();
    let mut view = provider_view(
        event,
        provider,
        kind,
        phase,
        &tool_title_with_input(name, Some(&input)),
        ProviderViewOptions {
            detail: presentation_detail(presentation.as_ref()),
            presentation,
            minor: (event_type == "tool_execution_update").then_some(true),
            raw_text,
        },
    );
    view.verb = Some(tool_verb_with_input(name, Some(&input), complete));
    view.target = target;
    view.result = result;
    view.action_id = action_id.clone();
    view.source_id = Some(action_id);
    view.complete = Some(complete);
    Some(view)
}

fn pi_result_text(payload: &BTreeMap<String, Value>) -> Option<&str> {
    let result = payload.get("result")?;
    text_value(Some(result)).or_else(|| {
        result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|content| {
                content
                    .iter()
                    .find_map(|value| text_value(value.get("text")))
            })
    })
}

/// Pi carries no id across a message's `start`/`update`*/`end` triple, unlike
/// its tool calls (`toolCallId`) — the payloads show no field that survives
/// the whole stream. One message stream is open at a time, so a fixed key
/// stands in for a real id: `foldActions` merges same-key rows while the slot
/// is still open and reopens a fresh row once a prior one has closed
/// (`message_end` completes it), which is exactly the start/update/end
/// lifecycle this collapses.
const PI_MESSAGE_ACTION_ID: &str = "pi:message-stream";

/// `message_start`/`update`/`end` all carry the same `message.role`/`content`
/// shape. A `message_update` may carry that shape in
/// `assistantMessageEvent.partial` instead of the top-level `message`; a
/// text-typed content item grows a little on each update and arrives complete
/// on `message_end`. Deriving title/detail identically for all three, from the
/// one field they share, means each later event's own fields safely overwrite
/// the row's without ever needing special-case merge behaviour: role does not
/// change mid-stream, so the title is stable, and an update with nothing new
/// simply carries `detail: None`, which `settleAction` already treats as "keep
/// what the row already has".
fn pi_message_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> Option<TaskEventView> {
    let event_type = text_value(payload.get("type")).unwrap_or_default();
    let message = payload
        .get("message")
        .and_then(Value::as_object)
        .or_else(|| {
            payload
                .get("assistantMessageEvent")
                .and_then(|value| value.get("partial"))
                .and_then(Value::as_object)
        });
    let role = message
        .and_then(|value| text_value(value.get("role")))
        .unwrap_or("assistant");
    let content = message
        .and_then(|value| value.get("content"))
        .and_then(Value::as_array);
    let text = content.and_then(|items| items.iter().find_map(|item| text_value(item.get("text"))));
    // Pi streams reasoning into the same message it later fills with the reply,
    // so an update that has thinking and no text yet is a reasoning block, not
    // an agent message with nothing in it. Each update repeats the whole block
    // it has so far; the trace folds the growing copies back into one row.
    if role == "assistant" && text.is_none() {
        let thinking = content
            .into_iter()
            .flatten()
            .filter_map(|item| text_value(item.get("thinking")))
            .collect::<Vec<_>>();
        if !thinking.is_empty() {
            return Some(reasoning_view(
                event,
                provider,
                Some(thinking.join("\n\n")),
                raw_text,
            ));
        }
    }
    let title = match role {
        "user" => "User message",
        "toolResult" => "Tool result",
        _ => "Agent message",
    };
    let complete = event_type == "message_end";
    let phase = if complete {
        EventPhase::Completed
    } else {
        EventPhase::Started
    };
    let mut view = provider_view(
        event,
        provider,
        EventKind::Message,
        phase,
        title,
        ProviderViewOptions {
            detail: text.map(str::to_owned),
            presentation: text.map(|value| message_presentation(value.to_owned())),
            minor: None,
            raw_text,
        },
    );
    view.action_id = Some(PI_MESSAGE_ACTION_ID.to_owned());
    view.source_id = Some(view.action_id.clone());
    view.complete = Some(complete);
    Some(view)
}

struct ProviderViewOptions {
    detail: Option<String>,
    presentation: Option<TaskEventPresentation>,
    minor: Option<bool>,
    raw_text: Option<String>,
}

fn provider_view(
    event: &TaskEvent,
    provider: Provider,
    kind: EventKind,
    phase: EventPhase,
    title: &str,
    options: ProviderViewOptions,
) -> TaskEventView {
    TaskEventView {
        id: event.id,
        task_id: event.task_id.clone(),
        source: EventSource::from_event_type(&event.kind, provider),
        event_type: event.kind.clone(),
        kind,
        phase,
        title: named(title.to_owned()),
        detail: options.detail,
        verb: None,
        target: None,
        result: None,
        presentation: options.presentation,
        raw_text: options.raw_text,
        created_at: event.created_at.clone(),
        parent_action_id: None,
        turn_id: event.turn_id,
        action_id: None,
        source_id: None,
        complete: None,
        minor: options.minor,
    }
}

/// An `agent.error` event is the agent/system layer failing outright, not a
/// tool call failing — it carries its own `error.type`/`error.message`, never
/// a `tool_use_id`.
fn agent_error_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> TaskEventView {
    let error = payload.get("error").and_then(Value::as_object);
    let message = error
        .and_then(|value| text_value(value.get("message")))
        .map(str::to_owned);
    let error_type = error.and_then(|value| text_value(value.get("type")));
    let status = error.and_then(|value| number_u64(value.get("status")));
    let detail = match (message.clone(), status) {
        (Some(message), Some(status)) => Some(format!("{message} (HTTP {status})")),
        (Some(message), None) => Some(message),
        (None, _) => error_type.map(str::to_owned),
    };
    provider_view(
        event,
        provider,
        EventKind::Error,
        EventPhase::Failed,
        "Error",
        ProviderViewOptions {
            detail,
            presentation: None,
            minor: None,
            raw_text,
        },
    )
}

fn usage_presentation() -> TaskEventPresentation {
    TaskEventPresentation {
        kind: PresentationType::Usage,
        path: None,
        change: None,
        command: None,
        status: None,
        exit_code: None,
        text: None,
        completed: None,
        total: None,
        cost_usd: None,
        tokens_in: None,
        tokens_out: None,
        tokens_cached: None,
        tokens_thinking: None,
        outcome: None,
        turns: None,
        duration_ms: None,
        level: None,
    }
}

/// One block of the model's own reasoning, however the provider framed it.
///
/// Every block carries the same title so the trace can fold a run of them into
/// a single row. A block the provider redacted arrives text-less rather than
/// not at all: it still says the model paused to think, and dropping it would
/// shorten the elapsed time the row reports.
fn reasoning_view(
    event: &TaskEvent,
    provider: Provider,
    text: Option<String>,
    raw_text: Option<String>,
) -> TaskEventView {
    provider_view(
        event,
        provider,
        EventKind::Reasoning,
        EventPhase::Info,
        REASONING_TITLE,
        ProviderViewOptions {
            detail: text.clone(),
            presentation: text.map(message_presentation),
            minor: None,
            raw_text,
        },
    )
}

/// A running count of reasoning tokens, which providers tick out far faster
/// than they produce readable blocks. It belongs on the run's receipt, never on
/// a row of its own, so it ships minor and carries nothing but the number.
fn reasoning_tokens_view(
    event: &TaskEvent,
    provider: Provider,
    tokens: u64,
    raw_text: Option<String>,
) -> TaskEventView {
    let mut presentation = usage_presentation();
    presentation.tokens_thinking = Some(tokens);
    provider_view(
        event,
        provider,
        EventKind::Usage,
        EventPhase::Info,
        "Thinking tokens",
        ProviderViewOptions {
            detail: None,
            presentation: Some(presentation),
            minor: Some(true),
            raw_text,
        },
    )
}

fn message_presentation(text: String) -> TaskEventPresentation {
    let mut presentation = usage_presentation();
    presentation.kind = PresentationType::Message;
    presentation.text = Some(text);
    presentation
}

fn signal_presentation(text: String, level: EventLevel) -> TaskEventPresentation {
    let mut presentation = usage_presentation();
    presentation.kind = PresentationType::Signal;
    presentation.text = Some(text);
    presentation.level = Some(level);
    presentation
}

fn file_presentation(path: String) -> TaskEventPresentation {
    let mut presentation = usage_presentation();
    presentation.kind = PresentationType::File;
    presentation.path = Some(path);
    presentation
}

fn tool_family(tool: &str) -> ToolFamily {
    match tool.to_ascii_lowercase().as_str() {
        "skill" => ToolFamily::Skill,
        "bash" | "run_command" | "run command" => ToolFamily::Command,
        "read"
        | "read_file"
        | "view_file"
        | "edit"
        | "multiedit"
        | "write"
        | "write_file"
        | "apply_patch"
        | "replace_file_content"
        | "write_to_file"
        | "list_dir" => ToolFamily::File,
        "glob" | "find_by_name" | "grep" | "grep_search" | "code_search" | "websearch"
        | "search_web" => ToolFamily::Search,
        _ => ToolFamily::Other,
    }
}

enum ToolFamily {
    Skill,
    Command,
    File,
    Search,
    Other,
}

struct OgaToolCall<'a> {
    operation: String,
    input: Option<&'a Map<String, Value>>,
}

fn oga_call<'a>(
    tool: &str,
    input: Option<&'a Map<String, Value>>,
    server: Option<&str>,
) -> Option<OgaToolCall<'a>> {
    let wrapper = is_mcp_wrapper(tool);
    let nested_server = input
        .and_then(|value| string_value(value, &["ServerName", "serverName", "server", "Server"]));
    let operation = oga_operation(tool).or_else(|| {
        server
            .or(nested_server.as_deref())
            .filter(|value| is_oga_server(value))
            .and_then(|_| {
                (!wrapper)
                    .then(|| canonical_oga_operation(tool))
                    .flatten()
                    .or_else(|| {
                        input.and_then(|value| {
                            string_value(value, &["ToolName", "toolName", "tool"])
                                .and_then(|name| canonical_oga_operation(&name))
                        })
                    })
            })
    })?;
    let input = if wrapper {
        input
            .and_then(|value| {
                ["Arguments", "arguments", "args"]
                    .into_iter()
                    .find_map(|key| value.get(key).and_then(Value::as_object))
            })
            .or(input)
    } else {
        input
    };
    Some(OgaToolCall { operation, input })
}

fn is_mcp_wrapper(tool: &str) -> bool {
    matches!(
        tool.to_ascii_lowercase().as_str(),
        "call_mcp_tool" | "call_mcp" | "mcp_tool"
    )
}

fn is_oga_server(server: &str) -> bool {
    matches!(
        server.trim().to_ascii_lowercase().as_str(),
        "oga" | "oga-mcp"
    )
}

fn oga_operation(tool: &str) -> Option<String> {
    let normalized = tool.trim().to_ascii_lowercase();
    if let Some(rest) = normalized.strip_prefix("oga_") {
        return canonical_oga_operation(rest);
    }
    if let Some(rest) = normalized.strip_prefix("oga-") {
        return canonical_oga_operation(rest);
    }
    let mut segments = normalized.strip_prefix("mcp__")?.split("__");
    let server = segments.next()?;
    if !is_oga_server(server) {
        return None;
    }
    canonical_oga_operation(segments.last()?)
}

fn canonical_oga_operation(operation: &str) -> Option<String> {
    let operation = operation
        .trim()
        .trim_start_matches("oga_")
        .trim_start_matches("oga-")
        .replace('_', "-");
    (!operation.is_empty()).then_some(operation)
}

fn oga_tool_title(operation: &str, input: Option<&Map<String, Value>>) -> String {
    match operation {
        "tasks" => "List tasks".into(),
        "query" => "Find code".into(),
        "delegate" => "Send work".into(),
        "inspect" => "View task".into(),
        "memory" => match oga_action(input).as_deref() {
            Some("set") => "Save project note".into(),
            Some("remove") => "Remove project note".into(),
            _ => "Read project notes".into(),
        },
        "health" => "Check connection".into(),
        "models" => "Check available models".into(),
        "reply" => "Answer question".into(),
        "resume" => "Continue task".into(),
        "steer" => "Guide task".into(),
        "handoff" => "Move task".into(),
        "cancel" => "Stop task".into(),
        "complete" => "Confirm task complete".into(),
        "archive" => {
            let archived = input
                .and_then(|value| value.get("archived"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if archived {
                "Archive task".into()
            } else {
                "Restore task".into()
            }
        }
        "worktree-remove" => "Remove task copy".into(),
        _ => humanize(operation),
    }
}

fn oga_tool_verb(operation: &str, input: Option<&Map<String, Value>>, complete: bool) -> String {
    let completed = |done: &'static str, open: &'static str| if complete { done } else { open };
    match operation {
        "tasks" => completed("Listed", "Listing"),
        "query" => completed("Searched", "Searching"),
        "delegate" => completed("Sent", "Sending"),
        "inspect" => completed("Viewed", "Viewing"),
        "memory" => match oga_action(input).as_deref() {
            Some("set") => completed("Saved", "Saving"),
            Some("remove") => completed("Removed", "Removing"),
            _ => completed("Read", "Reading"),
        },
        "health" | "models" => completed("Checked", "Checking"),
        "reply" => completed("Answered", "Answering"),
        "resume" => completed("Continued", "Continuing"),
        "steer" => completed("Guided", "Guiding"),
        "handoff" => completed("Moved", "Moving"),
        "cancel" => completed("Stopped", "Stopping"),
        "complete" => completed("Confirmed", "Confirming"),
        "archive" => {
            let archived = input
                .and_then(|value| value.get("archived"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if archived {
                completed("Archived", "Archiving")
            } else {
                completed("Restored", "Restoring")
            }
        }
        "worktree-remove" => completed("Removed", "Removing"),
        _ => completed("Ran", "Running"),
    }
    .into()
}

fn oga_subject(operation: &str, input: Option<&Map<String, Value>>) -> Option<String> {
    let input = input?;
    let subject = match operation {
        "tasks" => string_value(input, &["query", "q"])
            .or_else(|| {
                string_value(input, &["state"])
                    .map(|state| format!("{} tasks", oga_state_label(&state)))
            })
            .or_else(|| {
                input
                    .get("archived")
                    .and_then(Value::as_bool)
                    .map(|archived| {
                        if archived {
                            "archived tasks"
                        } else {
                            "active tasks"
                        }
                        .into()
                    })
            }),
        "query" => string_value(input, &["q", "query"]),
        "delegate" => string_value(input, &["title", "tldr", "description", "prompt"]),
        "memory" => string_value(input, &["key", "cwd"]),
        "health" => Some("broker".into()),
        "models" => string_value(input, &["query", "profile", "provider"])
            .or_else(|| Some("available models".into())),
        "worktree-remove" => string_value(input, &["project"]).or_else(|| task_id_subject(input)),
        "inspect" | "reply" | "resume" | "steer" | "handoff" | "cancel" | "complete"
        | "archive" => task_id_subject(input),
        _ => ["query", "pattern", "description", "name", "path", "project"]
            .into_iter()
            .find_map(|key| string_value(input, &[key])),
    }?;
    Some(cap(&subject, 120).0)
}

fn oga_action(input: Option<&Map<String, Value>>) -> Option<String> {
    input.and_then(|value| string_value(value, &["action"]))
}

fn task_id_subject(input: &Map<String, Value>) -> Option<String> {
    let value = input.get("taskId")?;
    if let Some(tasks) = value.as_array() {
        return Some(format_counted(tasks.len(), "task"));
    }
    text_value(Some(value)).map(|_| "selected task".into())
}

fn oga_presentation(
    operation: &str,
    input: Option<&Map<String, Value>>,
    state: Option<&Map<String, Value>>,
) -> TaskEventPresentation {
    let mut presentation = usage_presentation();
    presentation.kind = PresentationType::Tool;
    presentation.status =
        state.and_then(|value| text_value(value.get("status")).map(str::to_owned));
    presentation.text = oga_subject(operation, input);
    presentation.outcome = state
        .and_then(tool_state_output)
        .and_then(|output| oga_result_summary(operation, &output));
    presentation
}

/// What a tool call shows on its row, derived per tool rather than from one
/// shared key: a search is named by its pattern, a shell call by its command, a
/// skill by its name, a file operation by its path.
fn tool_presentation(
    tool: &str,
    input: &Map<String, Value>,
    state: Option<&Map<String, Value>>,
) -> Option<TaskEventPresentation> {
    if let Some(call) = oga_call(tool, Some(input), None) {
        return Some(oga_presentation(&call.operation, call.input, state));
    }
    let normalized = tool.to_ascii_lowercase();
    let status = state.and_then(|value| text_value(value.get("status")).map(str::to_owned));
    let mut presentation = usage_presentation();
    presentation.status = status;
    if matches!(normalized.as_str(), "todowrite" | "todoread") {
        let items = input
            .get("todos")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let total = items.len() as u64;
        let completed = items
            .iter()
            .filter(|item| item.get("status").and_then(Value::as_str) == Some("completed"))
            .count() as u64;
        presentation.kind = PresentationType::Todo;
        presentation.completed = Some(completed);
        presentation.total = Some(total);
        presentation.outcome = Some(format!("{total} todo{}", if total == 1 { "" } else { "s" }));
        return Some(presentation);
    }
    match tool_family(tool) {
        ToolFamily::Skill => {
            presentation.kind = PresentationType::Tool;
            presentation.text = Some(string_value(input, &["name", "skill"])?);
        }
        ToolFamily::Command => {
            let command =
                string_value(input, &["command", "cmd", "CommandLine"]).or_else(|| {
                    state.and_then(|value| text_value(value.get("command")).map(str::to_owned))
                })?;
            if let Some(subject) = search_command_subject(&command) {
                presentation.kind = PresentationType::Tool;
                presentation.text = Some(subject);
                presentation.outcome = state
                    .and_then(search_outcome)
                    .or_else(|| state.and_then(search_output_outcome));
            } else {
                presentation.kind = PresentationType::Command;
                presentation.text = Some(command_summary(&command));
                presentation.command = Some(command);
                presentation.exit_code = state.and_then(exit_code);
            }
        }
        ToolFamily::File => {
            let path = string_value(
                input,
                &[
                    "filePath",
                    "file_path",
                    "path",
                    "AbsolutePath",
                    "TargetFile",
                    "DirectoryPath",
                ],
            );
            presentation.kind = PresentationType::File;
            presentation.path = path.clone();
            if let (Some(old), Some(new)) = (
                string_value(input, &["oldString", "old_string", "oldText", "old_text"]),
                string_value(input, &["newString", "new_string", "newText", "new_text"]),
            ) {
                presentation.change = Some(compact_change(&old, &new));
            } else if matches!(
                normalized.as_str(),
                "write" | "write_file" | "write_to_file"
            ) {
                if let Some(content) = string_value(input, &["content", "text"]) {
                    let lines = content.lines().count().max(1);
                    presentation.change = Some(format!(
                        "{} line{}",
                        lines,
                        if lines == 1 { "" } else { "s" }
                    ));
                }
            } else if let Some(patch) = input_patch(tool, Some(input)) {
                presentation.path = Some(patch.paths[0].clone());
                presentation.change = patch.change();
            } else if normalized == "apply_patch" && path.is_none() {
                presentation.change = Some("Patch text".into());
            } else if matches!(normalized.as_str(), "read" | "read_file" | "view_file") {
                presentation.outcome = read_range(input);
            }
        }
        ToolFamily::Search => {
            presentation.kind = PresentationType::Tool;
            presentation.text = Some(search_subject(input)?);
            presentation.outcome = state
                .and_then(search_outcome)
                .or_else(|| state.and_then(search_output_outcome));
        }
        ToolFamily::Other => {
            let mut subject = [
                "query",
                "pattern",
                "url",
                "description",
                "subagent_type",
                "name",
                "path",
                "filePath",
                "file_path",
                "patchText",
                "TaskId",
                "TimerCondition",
            ]
            .into_iter()
            .filter_map(|key| string_value(input, &[key]))
            .take(2)
            .map(|value| cap(&value, 120).0)
            .collect::<Vec<_>>();
            // A prompt is only worth showing when nothing else names the call
            // (e.g. `Agent` sends both `description` and a full `prompt`; the
            // description alone is the row, never the two joined).
            if subject.is_empty()
                && let Some(prompt) = string_value(input, &["prompt"])
            {
                subject.push(cap(&prompt, 120).0);
            }
            presentation.kind = PresentationType::Tool;
            presentation.text = Some(if subject.is_empty() {
                // The provider's own label for the call is the last thing worth
                // reading; without it the row falls back to naming its tool.
                state
                    .and_then(|value| text_value(value.get("title")))
                    .map(|title| cap(title, 120).0)?
            } else {
                subject.join(" · ")
            });
        }
    }
    Some(presentation)
}

/// OpenCode spills a long tool result to a file and has the model read it back.
/// That read is the same call again, not a new file the run cares about.
fn is_captured_output_read(tool: &str, input: Option<&Map<String, Value>>) -> bool {
    matches!(
        tool.to_ascii_lowercase().as_str(),
        "read" | "read_file" | "view_file"
    ) && input
        .and_then(|input| string_value(input, &["filePath", "file_path", "path"]))
        .is_some_and(|path| path.contains("/tool-output/tool_"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PatchOp {
    Add,
    Update,
    Delete,
}

/// What a `*** Begin Patch` envelope actually touches. OpenCode sends the whole
/// patch as one `patchText` string, so the files it names and the size of the
/// change are only knowable by reading it.
struct PatchSummary {
    paths: Vec<String>,
    op: Option<PatchOp>,
    added: usize,
    removed: usize,
}

impl PatchSummary {
    fn title(&self) -> &'static str {
        match (self.op, self.paths.len()) {
            (Some(PatchOp::Add), 1) => "Write file",
            (Some(PatchOp::Delete), 1) => "Delete file",
            (_, 1) => "Edit file",
            _ => "Edit files",
        }
    }

    fn verb(&self, complete: bool) -> &'static str {
        match (self.op, complete) {
            (Some(PatchOp::Add), true) => "Wrote",
            (Some(PatchOp::Add), false) => "Writing",
            (Some(PatchOp::Delete), true) => "Deleted",
            (Some(PatchOp::Delete), false) => "Deleting",
            (_, true) => "Edited",
            (_, false) => "Editing",
        }
    }

    fn change(&self) -> Option<String> {
        let rest = self.paths.len().saturating_sub(1);
        let files =
            (rest > 0).then(|| format!("{rest} more file{}", if rest == 1 { "" } else { "s" }));
        let lines =
            (self.added + self.removed > 0).then(|| format!("+{} −{}", self.added, self.removed));
        match (files, lines) {
            (Some(files), Some(lines)) => Some(format!("{files} · {lines}")),
            (files, lines) => files.or(lines),
        }
    }
}

fn input_patch(tool: &str, input: Option<&Map<String, Value>>) -> Option<PatchSummary> {
    if !tool.eq_ignore_ascii_case("apply_patch") {
        return None;
    }
    let text = string_value(
        input?,
        &["patchText", "patch_text", "patch", "input", "diff"],
    )?;
    parse_patch(&text)
}

fn parse_patch(text: &str) -> Option<PatchSummary> {
    let mut paths = Vec::new();
    let mut ops = Vec::new();
    let mut added = 0;
    let mut removed = 0;
    for line in text.lines() {
        let trimmed = line.trim_end();
        let header = trimmed.strip_prefix("*** ").and_then(|rest| {
            [
                ("Add File: ", PatchOp::Add),
                ("Update File: ", PatchOp::Update),
                ("Delete File: ", PatchOp::Delete),
            ]
            .into_iter()
            .find_map(|(prefix, op)| rest.strip_prefix(prefix).map(|path| (path.trim(), op)))
        });
        if let Some((path, op)) = header {
            if !path.is_empty() {
                paths.push(path.to_owned());
                ops.push(op);
            }
            continue;
        }
        if trimmed.starts_with("+++") || trimmed.starts_with("---") {
            continue;
        }
        if trimmed.starts_with('+') {
            added += 1;
        } else if trimmed.starts_with('-') {
            removed += 1;
        }
    }
    if paths.is_empty() {
        return None;
    }
    let first = ops[0];
    let op = ops.iter().all(|other| *other == first).then_some(first);
    Some(PatchSummary {
        paths,
        op,
        added,
        removed,
    })
}

/// A read is named by the slice it asked for, never by the file body it got
/// back. `offset` is the first line, so a `limit` counts forward from it;
/// opencode sends the pair as `filePath`/`offset`/`limit`, Claude as
/// `file_path`/`offset`/`limit`, and either may send one without the other.
/// Antigravity's `view_file` and Pi's `read` carry no such fields at all, so
/// this falls through to `None` for both — the row names the file alone
/// rather than inventing a range.
fn read_range(input: &Map<String, Value>) -> Option<String> {
    let first = number_u64(input.get("offset"));
    let count = number_u64(input.get("limit")).filter(|count| *count > 0);
    match (first, count) {
        (Some(first), Some(count)) => Some(format!("lines {first}–{}", first + count - 1)),
        (Some(first), None) => Some(format!("from line {first}")),
        (None, Some(count)) => Some(format!("lines 1–{count}")),
        (None, None) => None,
    }
}

/// A search says how much it found in its own key, and each key counts a
/// different thing: a file search reports `count` files, a text search reports
/// `matches`. Either can come back capped.
fn search_outcome(state: &Map<String, Value>) -> Option<String> {
    let metadata = state.get("metadata").and_then(Value::as_object)?;
    let (found, noun) = [("count", "file"), ("matches", "match")]
        .into_iter()
        .find_map(|(key, noun)| Some((metadata.get(key)?.as_u64()?, noun)))?;
    let capped = metadata
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or_default();
    Some(format_search_outcome(found, noun, capped))
}

fn search_subject(input: &Map<String, Value>) -> Option<String> {
    let pattern = string_value(
        input,
        &["pattern", "query", "glob", "Pattern", "Query", "Glob"],
    )?;
    let scope = string_value(
        input,
        &[
            "path",
            "filePath",
            "file_path",
            "SearchPath",
            "search_path",
            "directory",
            "Directory",
            "scope",
        ],
    );
    let pattern = cap(&pattern, 120).0;
    Some(match scope {
        Some(scope) => format!("{pattern} in {}", cap(&scope, 80).0),
        None => pattern,
    })
}

/// What a shell command is really doing, so a row can say it and consecutive
/// rows of the same kind can fold. Everything a shell can do that is not one of
/// these stays an unclassified command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CommandRole {
    Read,
    Search,
    Find,
    List,
    Inspect,
    Check(CheckKind),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CheckKind {
    Lint,
    Types,
    Tests,
    Build,
    Other,
}

impl CommandRole {
    fn title(self) -> &'static str {
        match self {
            CommandRole::Read => "Read file",
            CommandRole::Search => "Search code",
            CommandRole::Find => "Find files",
            CommandRole::List => "List directory",
            CommandRole::Inspect => "Inspect changes",
            CommandRole::Check(CheckKind::Lint) => "Check lint",
            CommandRole::Check(CheckKind::Types) => "Check types",
            CommandRole::Check(CheckKind::Tests) => "Check tests",
            CommandRole::Check(CheckKind::Build) => "Check build",
            CommandRole::Check(CheckKind::Other) => "Run checks",
        }
    }

    fn verb(self, complete: bool) -> &'static str {
        match (self, complete) {
            (CommandRole::Read, _) => "Read",
            (CommandRole::Search, true) => "Searched",
            (CommandRole::Search, false) => "Searching",
            (CommandRole::Find, true) => "Found",
            (CommandRole::Find, false) => "Finding",
            (CommandRole::List, true) => "Listed",
            (CommandRole::List, false) => "Listing",
            (CommandRole::Inspect, true) => "Inspected",
            (CommandRole::Inspect, false) => "Inspecting",
            (CommandRole::Check(_), true) => "Checked",
            (CommandRole::Check(_), false) => "Checking",
        }
    }

    /// Only a search names itself by its pattern; the rest read better as the
    /// command they were, exit code and all.
    fn shows_subject(self) -> bool {
        matches!(self, CommandRole::Search | CommandRole::Find)
    }
}

/// A shell line is rarely one program: `cd x && cat a | head -20` is three, and
/// the row has to name what the line as a whole did. Each segment is judged on
/// its own, plumbing (`cd`, `echo`, env assignments) is ignored, a verification
/// step wins over anything it is piped into, and one unrecognised segment makes
/// the whole line an ordinary command again.
fn command_role(command: &str) -> Option<CommandRole> {
    let mut roles = Vec::new();
    for segment in command_segments(command) {
        match segment_role(segment) {
            Segment::Ignored => {}
            Segment::Unknown => return None,
            Segment::Role(role) => roles.push(role),
        }
    }
    roles
        .iter()
        .copied()
        .find(|role| matches!(role, CommandRole::Check(_)))
        .or_else(|| roles.first().copied())
}

enum Segment {
    Ignored,
    Unknown,
    Role(CommandRole),
}

/// Providers wrap a command in the shell that ran it (`/bin/zsh -lc '…'`); the
/// row and its classification want the line the model actually wrote.
fn shell_body(command: &str) -> &str {
    let command = command.trim();
    command
        .split_once(" -lc ")
        .map(|(_, body)| {
            body.trim()
                .trim_matches(|character| character == '"' || character == '\'')
        })
        .unwrap_or(command)
}

/// Splits on the operators that chain one program to the next, and only when
/// they are unquoted — a `|` inside a search pattern is part of the pattern.
/// A quote counts whether or not it arrived escaped: unwrapping the shell
/// wrapper leaves the inner quotes still spelled `\"`.
fn command_segments(command: &str) -> Vec<&str> {
    let body = shell_body(command);
    let mut segments = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    for (index, character) in body.char_indices() {
        match (quote, character) {
            (Some(open), _) if character == open => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(character),
            (None, '|' | ';' | '&' | '\n') => {
                segments.push(&body[start..index]);
                start = index + character.len_utf8();
            }
            (None, _) => {}
        }
    }
    segments.push(&body[start..]);
    segments
        .into_iter()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// The words of one segment with its preamble taken off: a leading `VAR=value`
/// is setup rather than the program, and `do`, `then` and `else` open a block
/// the real command follows.
fn command_words(segment: &str) -> impl Iterator<Item = &str> {
    segment
        .split_whitespace()
        .skip_while(|word| word.contains('=') && !word.starts_with('-'))
        .skip_while(|word| matches!(*word, "do" | "then" | "else"))
}

fn segment_role(segment: &str) -> Segment {
    let mut words = command_words(segment);
    let Some(first) = words.next() else {
        return Segment::Ignored;
    };
    let program = first
        .trim_matches(|character| character == '"' || character == '\'')
        .rsplit('/')
        .next()
        .unwrap_or(first);
    let rest = words.collect::<Vec<_>>();
    // What `2>&1` leaves behind once the line is split on its operators.
    if program.starts_with(['>', '<'])
        || program.chars().all(|character| character.is_ascii_digit())
    {
        return Segment::Ignored;
    }
    if matches!(
        program,
        "cd" | "echo"
            | "printf"
            | "export"
            | "set"
            | "source"
            | "pwd"
            | "true"
            | "test"
            | "which"
            | "sort"
            | "uniq"
            | "cut"
            | "awk"
            | "tr"
            | "xargs"
            | "tee"
            // A loop or a branch is scaffolding; what it runs is in its body.
            | "for"
            | "while"
            | "until"
            | "if"
            | "elif"
            | "case"
            | "done"
            | "fi"
            | "esac"
    ) {
        return Segment::Ignored;
    }
    if let Some(check) = check_role(program, &rest) {
        return Segment::Role(CommandRole::Check(check));
    }
    // An in-place flag turns a reader into a writer, and the row would say the
    // opposite of what happened.
    if matches!(program, "sed" | "perl")
        && rest
            .iter()
            .any(|word| word.starts_with("-i") || word.starts_with("-pi"))
    {
        return Segment::Unknown;
    }
    let role = match program {
        "cat" | "head" | "tail" | "bat" | "sed" | "nl" | "less" | "more" => CommandRole::Read,
        "rg" | "grep" | "egrep" | "fgrep" | "ag" | "ack" | "ripgrep" => CommandRole::Search,
        "find" | "fd" | "fdfind" => CommandRole::Find,
        "ls" | "ll" | "tree" | "exa" | "eza" => CommandRole::List,
        "wc" | "stat" | "du" | "file" | "basename" | "dirname" | "sysctl" | "uname" | "date" => {
            CommandRole::Inspect
        }
        "git" => match rest.first().copied() {
            Some("grep") => CommandRole::Search,
            Some(
                "log" | "diff" | "status" | "show" | "blame" | "branch" | "remote" | "ls-files",
            ) => CommandRole::Inspect,
            _ => return Segment::Unknown,
        },
        // Oga's own read-only subcommands: a worker reaches for them the way it
        // reaches for a search tool, and the row reads better saying so.
        "oga" | "oga-cli" => match rest.first().copied() {
            Some("query") => CommandRole::Search,
            Some("tasks" | "inspect" | "models" | "health") => CommandRole::Inspect,
            _ => return Segment::Unknown,
        },
        "gh" => match (rest.first().copied(), rest.get(1).copied()) {
            (
                Some("pr" | "issue" | "run"),
                Some("view" | "list" | "status" | "diff" | "checks"),
            ) => CommandRole::Inspect,
            _ => return Segment::Unknown,
        },
        _ => return Segment::Unknown,
    };
    Segment::Role(role)
}

/// A verification step is named by what it proves, not by the runner that
/// launched it, so `bun run lint`, `oxlint`, and `cargo clippy` all read as one
/// kind of check.
fn check_role(program: &str, rest: &[&str]) -> Option<CheckKind> {
    // Standing alone, only a tool's own name proves anything — `test` on its
    // own is the shell builtin, not a test suite.
    let tool = |name: &str| match name {
        "oxlint" | "eslint" | "biome" | "ruff" | "pint" | "prettier" | "phpstan" => {
            Some(CheckKind::Lint)
        }
        "tsc" | "mypy" => Some(CheckKind::Types),
        "pytest" | "vitest" | "jest" | "phpunit" => Some(CheckKind::Tests),
        _ => None,
    };
    let script = |name: &str| {
        tool(name).or(match name {
            "lint" | "clippy" | "fmt" | "format" => Some(CheckKind::Lint),
            "typecheck" | "types" | "vet" => Some(CheckKind::Types),
            "test" | "tests" => Some(CheckKind::Tests),
            "build" | "compile" => Some(CheckKind::Build),
            "check" | "verify" | "ci" => Some(CheckKind::Other),
            _ => None,
        })
    };
    match program {
        "bun" | "bunx" | "npm" | "pnpm" | "yarn" | "npx" | "deno" | "composer" | "make"
        | "just" | "cargo" | "go" | "task" => {
            let mut rest = rest.iter().copied();
            let first = rest.next()?;
            let first = if first == "run" { rest.next()? } else { first };
            script(first.rsplit(':').next().unwrap_or(first))
        }
        _ => tool(program),
    }
}

/// The part of a shell line a row has room for. Most of a line is plumbing —
/// the directory it ran in, the redirection, the pager it piped into — and it
/// buries the one program that did the work. The whole line stays under the
/// row, in the terminal a reader can open.
fn command_summary(command: &str) -> String {
    let segments = command_segments(command);
    let start = segments
        .iter()
        .position(|segment| !matches!(segment_role(segment), Segment::Ignored))
        .unwrap_or(0);
    // A command the model wrapped over several lines is still one command.
    let mut words = Vec::new();
    for segment in &segments[start..] {
        let trimmed = segment.trim_end();
        words.extend(command_words(trimmed.trim_end_matches('\\')));
        if !trimmed.ends_with('\\') {
            break;
        }
    }
    if let Some(index) = words.iter().position(|word| is_redirection(word)) {
        words.truncate(index);
    }
    if words.is_empty() {
        return shell_body(command).trim().to_owned();
    }
    words.join(" ")
}

/// `2>&1`, `> log`, `2>/dev/null` — where a line stops saying what it did and
/// starts saying where the output went.
fn is_redirection(word: &str) -> bool {
    word.trim_start_matches(|character: char| character.is_ascii_digit())
        .starts_with(['>', '<'])
}

fn search_command_subject(command: &str) -> Option<String> {
    let role = command_role(command)?;
    role.shows_subject()
        .then(|| cap(&command_summary(command), 120).0)
}

fn search_command(input: &Map<String, Value>) -> Option<String> {
    string_value(input, &["command", "cmd"]).and_then(|command| search_command_subject(&command))
}

fn input_command(input: Option<&Map<String, Value>>) -> Option<String> {
    string_value(input?, &["command", "cmd", "CommandLine"])
}

fn search_output_outcome(state: &Map<String, Value>) -> Option<String> {
    string_value(state, &["output", "stdout", "result", "summary"])
        .and_then(|output| search_outcome_from_text(&output))
}

fn search_outcome_from_text(output: &str) -> Option<String> {
    let words = output.split_whitespace().collect::<Vec<_>>();
    for window in words.windows(3) {
        if !window[0].eq_ignore_ascii_case("found") {
            continue;
        }
        let Some((found, capped)) = parse_search_count(window[1]) else {
            continue;
        };
        let noun = match window[2]
            .trim_matches(|character: char| !character.is_ascii_alphabetic())
            .to_ascii_lowercase()
            .as_str()
        {
            "file" | "files" => "file",
            "match" | "matches" => "match",
            _ => continue,
        };
        return Some(format_search_outcome(found, noun, capped));
    }
    let lower = output.to_ascii_lowercase();
    if lower.contains("no files found") {
        return Some("0 files".into());
    }
    if lower.contains("no matches found") || lower.contains("no matches") {
        return Some("0 matches".into());
    }
    None
}

fn parse_search_count(value: &str) -> Option<(u64, bool)> {
    let capped = value.trim_end().ends_with('+');
    let digits = value.trim_matches(|character: char| !character.is_ascii_digit());
    if digits.is_empty() {
        return None;
    }
    Some((digits.parse().ok()?, capped))
}

fn format_search_outcome(found: u64, noun: &str, capped: bool) -> String {
    let plural = if noun == "file" { "s" } else { "es" };
    format!(
        "{found}{} {noun}{}",
        if capped { "+" } else { "" },
        if found == 1 { "" } else { plural }
    )
}

/// A shell call reports how it ended in whichever key its provider uses.
fn exit_code(state: &Map<String, Value>) -> Option<i64> {
    state
        .get("exit_code")
        .or_else(|| state.get("metadata").and_then(|value| value.get("exit")))
        .and_then(Value::as_i64)
}

/// A provider that sends `"title": ""` has said nothing. Every string read on
/// the way to a row goes through here, so a present-but-blank field counts as
/// missing instead of rendering as an empty line.
fn text_value(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn string_value(values: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| text_value(values.get(*key)))
        .map(str::to_owned)
}

fn tree_value(values: &BTreeMap<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| text_value(values.get(*key)))
        .map(str::to_owned)
}

/// Walks a nested path (e.g. `completion.reason`), unlike `tree_value`'s flat
/// list of alternative top-level keys.
fn nested_value(payload: &BTreeMap<String, Value>, path: &[&str]) -> Option<String> {
    let (first, rest) = path.split_first()?;
    let mut value = payload.get(*first)?;
    for key in rest {
        value = value.get(key)?;
    }
    text_value(Some(value)).map(str::to_owned)
}

/// Only a file operation counts as a file and only a shell call as a command,
/// so a group of them is counted by the right noun.
fn presentation_kind(presentation: &TaskEventPresentation) -> EventKind {
    match presentation.kind {
        PresentationType::File => EventKind::File,
        PresentationType::Command => EventKind::Command,
        _ => EventKind::Tool,
    }
}

/// The last guard before a row reaches a reader: a row that could not derive a
/// subject still names itself rather than rendering as an empty line.
fn named(title: String) -> String {
    if title.trim().is_empty() {
        "Activity".to_owned()
    } else {
        title
    }
}

fn presentation_target(presentation: Option<&TaskEventPresentation>) -> Option<String> {
    presentation.and_then(|value| match value.kind {
        PresentationType::File => value.path.clone(),
        PresentationType::Command => value.text.clone().or_else(|| value.command.clone()),
        PresentationType::Tool | PresentationType::Message | PresentationType::Todo => {
            value.text.clone()
        }
        _ => None,
    })
}

fn presentation_detail(presentation: Option<&TaskEventPresentation>) -> Option<String> {
    presentation.and_then(|value| match value.kind {
        PresentationType::File => match (&value.path, &value.change) {
            (Some(path), Some(change)) => Some(format!("{path} · {change}")),
            (Some(path), None) => Some(path.clone()),
            _ => value.change.clone(),
        },
        PresentationType::Command => value.command.clone(),
        PresentationType::Tool | PresentationType::Message | PresentationType::Todo => {
            value.text.clone()
        }
        _ => None,
    })
}

fn compact_change(old: &str, new: &str) -> String {
    let old = old.lines().next().unwrap_or(old).trim();
    let new = new.lines().next().unwrap_or(new).trim();
    format!(
        "{} → {}",
        cap(old, 80).0,
        if new.is_empty() {
            "∅".into()
        } else {
            cap(new, 80).0
        }
    )
}

fn compact_output(output: &str) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(value) = serde_json::from_str::<Value>(trimmed)
    {
        return Some(match value {
            Value::Array(items) => format!(
                "{} item{}",
                items.len(),
                if items.len() == 1 { "" } else { "s" }
            ),
            Value::Object(items) => format!(
                "{} field{}",
                items.len(),
                if items.len() == 1 { "" } else { "s" }
            ),
            _ => trimmed.to_owned(),
        });
    }
    let compact = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(cap(&compact, 160).0)
}

fn tool_state_output(state: &Map<String, Value>) -> Option<String> {
    ["output", "result", "stdout", "text"]
        .into_iter()
        .find_map(|key| state.get(key).and_then(mcp_result_text))
}

/// MCP servers put text in content blocks, while provider wrappers may expose
/// the same value as `output`, `stdout`, or `result`. Only the text is useful
/// to the row and its expansion; transport objects are not.
fn mcp_result_text(value: &Value) -> Option<String> {
    match value {
        Value::String(_) => text_value(Some(value)).map(str::to_owned),
        Value::Array(values) => values.iter().find_map(mcp_result_text),
        Value::Object(values) => [
            "content", "text", "stdout", "output", "result", "response", "message",
        ]
        .into_iter()
        .find_map(|key| values.get(key).and_then(mcp_result_text)),
        _ => None,
    }
}

fn oga_result_summary(operation: &str, output: &str) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }
    let value = serde_json::from_str::<Value>(trimmed)
        .ok()
        .map(|value| match value {
            Value::String(text) => {
                serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text))
            }
            value => value,
        });
    let Some(value) = value else {
        if operation == "query" {
            return query_result_summary(trimmed);
        }
        return first_output_line(trimmed);
    };
    if let Some(error) = result_error(&value) {
        return Some(error);
    }
    match operation {
        "tasks" => {
            count_result(&value, "tasks", "task").or_else(|| first_output_line_value(&value))
        }
        "models" => {
            count_result(&value, "models", "model").or_else(|| first_output_line_value(&value))
        }
        "query" => match &value {
            Value::String(text) => query_result_summary(text),
            _ => first_output_line_value(&value),
        },
        "memory" => memory_result_summary(&value),
        "health" => value.get("ok").and_then(Value::as_bool).map(|ok| {
            if ok {
                "Connected".into()
            } else {
                "Unavailable".into()
            }
        }),
        "delegate" => task_state_result(&value, "Work").or_else(|| Some("Work sent".into())),
        "inspect" => task_state_result(&value, "Task"),
        "reply" => Some("Answer sent".into()),
        "resume" => Some("Task continued".into()),
        "steer" => Some("Instruction sent".into()),
        "handoff" => Some("Task moved".into()),
        "cancel" => action_result_summary(&value, "Task stopped", "tasks", "task"),
        "complete" => Some("Task marked complete".into()),
        "archive" => action_result_summary(&value, "Task archived", "tasks", "task"),
        "worktree-remove" => {
            action_result_summary(&value, "Task copy removed", "removed", "task copy")
        }
        _ => first_output_line_value(&value),
    }
}

fn count_result(value: &Value, key: &str, noun: &str) -> Option<String> {
    let count = value
        .as_array()
        .map(Vec::len)
        .or_else(|| value.get(key).and_then(Value::as_array).map(Vec::len))?;
    Some(format_counted(count, noun))
}

fn format_counted(count: usize, noun: &str) -> String {
    let plural = match noun {
        "match" => "matches".to_owned(),
        "task copy" => "task copies".to_owned(),
        _ => format!("{noun}s"),
    };
    format!("{count} {}", if count == 1 { noun } else { &plural })
}

fn first_output_line(output: &str) -> Option<String> {
    output
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| cap(line.trim(), 120).0)
}

fn first_output_line_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => first_output_line(text),
        Value::Null => None,
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Array(_) => count_result(value, "items", "item"),
        Value::Object(_) => Some(compact_output(&serde_json::to_string(value).ok()?)?),
    }
}

fn query_result_summary(text: &str) -> Option<String> {
    let first = text.lines().find(|line| !line.trim().is_empty())?;
    if first.to_ascii_lowercase().contains("no confident match") {
        return Some("No matching code".into());
    }
    let count = text.lines().filter(|line| query_anchor_line(line)).count();
    if count > 0 {
        return Some(format_counted(count, "match"));
    }
    first_output_line(text)
}

fn query_anchor_line(line: &str) -> bool {
    let line = line.trim();
    if line.contains("(matched:") {
        return true;
    }
    let Some((path, rest)) = line.split_once(':') else {
        return false;
    };
    if !path.contains('/') && !path.contains('.') {
        return false;
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0
}

fn memory_result_summary(value: &Value) -> Option<String> {
    match value {
        Value::Array(entries) => Some(format_counted(entries.len(), "note")),
        Value::Null => Some("No note found".into()),
        Value::Object(object) => {
            if let Some(removed) = object.get("removed").and_then(Value::as_bool) {
                return Some(if removed {
                    "Note removed".into()
                } else {
                    "No note removed".into()
                });
            }
            if object.contains_key("key") && object.contains_key("value") {
                return Some("Note found".into());
            }
            object
                .get("version")
                .and_then(Value::as_u64)
                .map(|_| "Note saved".into())
        }
        _ => first_output_line_value(value),
    }
}

fn task_state_result(value: &Value, subject: &str) -> Option<String> {
    let state = value
        .get("state")
        .and_then(|value| text_value(Some(value)))?;
    Some(format!("{subject} is {}", oga_state_label(state)))
}

fn oga_state_label(state: &str) -> &'static str {
    match state.to_ascii_lowercase().as_str() {
        "queued" => "waiting to start",
        "preparing_checkout" => "preparing checkout",
        "removing_checkout" => "removing checkout",
        "pending" => "waiting to start",
        "running" | "answered" => "in progress",
        "needs_input" => "waiting for an answer",
        "completed" => "complete",
        "failed" => "failed",
        "blocked" => "blocked",
        "cancelled" => "stopped",
        _ => "status unavailable",
    }
}

fn action_result_summary(value: &Value, fallback: &str, key: &str, noun: &str) -> Option<String> {
    count_result(value, key, noun)
        .or_else(|| result_error(value))
        .or_else(|| Some(fallback.into()))
}

fn result_error(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(mcp_result_text)
        .map(|error| format!("Error: {}", cap(&error, 120).0))
}

fn hook_presentation(payload: &BTreeMap<String, Value>) -> Option<TaskEventPresentation> {
    let tool = tree_value(payload, &["tool_name", "toolName"])?;
    let input = hook_tool_input(payload)?;
    let mut presentation = tool_presentation(&tool, input, None)?;
    if let Some(call) = oga_call(&tool, Some(input), None) {
        presentation.outcome = hook_output(payload)
            .as_deref()
            .and_then(|output| oga_result_summary(&call.operation, output))
            .or(presentation.outcome);
    } else if matches!(tool_family(&tool), ToolFamily::Search) {
        presentation.outcome = hook_output(payload)
            .as_deref()
            .and_then(search_outcome_from_text)
            .or(presentation.outcome);
    }
    Some(presentation)
}

fn hook_tool_input(payload: &BTreeMap<String, Value>) -> Option<&Map<String, Value>> {
    payload
        .get("tool_input")
        .or_else(|| payload.get("toolInput"))
        .and_then(Value::as_object)
}

/// Failure text a tool call or a session end reports under whichever key its
/// hook uses: `PostToolUseFailure` carries `error` (sometimes `error_detail`
/// too), `StopFailure` carries `error` and the last thing the model said.
fn hook_error_detail(payload: &BTreeMap<String, Value>) -> Option<String> {
    let hook_name = tree_value(payload, &["hook_event_name", "hookEventName"])?;
    if !hook_name.contains("Failure") {
        return None;
    }
    if let Some(last_message) = tree_value(payload, &["last_assistant_message"]) {
        return Some(last_message);
    }
    let error = tree_value(payload, &["error"]);
    let detail = tree_value(payload, &["error_detail", "errorDetail"]);
    match (error, detail) {
        (Some(error), Some(detail)) => Some(format!("{error}: {detail}")),
        (Some(error), None) => Some(error),
        (None, Some(detail)) => Some(detail),
        (None, None) => None,
    }
}

/// `SubagentStart`/`SubagentStop` name a spawned agent by its id, not a tool;
/// the row still needs a subject, so it names the agent type instead. Both
/// titles match the `SUBAGENT_*_TITLE` pair the trace groups on, so the hook
/// pair nests the same way the streamed `task_started`/`task_notification`
/// pair already does.
fn hook_lifecycle_title(payload: &BTreeMap<String, Value>) -> Option<String> {
    match tree_value(payload, &["hook_event_name", "hookEventName"])?.as_str() {
        "SubagentStart" => Some(SUBAGENT_STARTED_TITLE.into()),
        "SubagentStop" => Some(SUBAGENT_FINISHED_TITLE.into()),
        "StopFailure" => Some("Session error".into()),
        _ => None,
    }
}

fn hook_lifecycle_detail(payload: &BTreeMap<String, Value>) -> Option<String> {
    match tree_value(payload, &["hook_event_name", "hookEventName"])?.as_str() {
        "SubagentStart" | "SubagentStop" => tree_value(payload, &["agent_type", "agentType"]),
        _ => None,
    }
}

fn hook_output(payload: &BTreeMap<String, Value>) -> Option<String> {
    let response = payload
        .get("tool_response")
        .or_else(|| payload.get("toolResponse"));
    response.and_then(mcp_result_text).or_else(|| {
        ["stdout", "output", "result"]
            .into_iter()
            .find_map(|key| payload.get(key).and_then(mcp_result_text))
    })
}

fn number_f64(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn number_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| number_f64(Some(value)).map(|value| value as u64))
    })
}

fn format_count(value: u64) -> String {
    value.to_string()
}

fn format_cost(value: f64) -> String {
    if value >= 0.01 || value == 0.0 {
        format!("${value:.2}")
    } else {
        format!("${value:.4}")
    }
}

fn event_detail(event_type: &str, payload: &BTreeMap<String, Value>) -> Option<String> {
    match event_type {
        "worker_spawned" => {
            let provider = tree_value(payload, &["provider"]).map(|value| humanize(&value));
            let model = tree_value(payload, &["model"]).map(|value| humanize(&value));
            match (provider, model) {
                (Some(provider), Some(model)) => Some(format!("{provider} {model}")),
                (None, Some(model)) => Some(model),
                (provider, None) => provider,
            }
        }
        "model_changed" => tree_value(payload, &["model"]),
        "handed_off" => {
            let from_profile = tree_value(payload, &["fromProfile"]);
            let to_profile = tree_value(payload, &["toProfile"]);
            let context = payload
                .get("sessionPreserved")
                .and_then(Value::as_bool)
                .map(|preserved| {
                    if preserved {
                        "conversation carried"
                    } else {
                        "rebuilt brief"
                    }
                });
            let route = if from_profile.is_some() && from_profile == to_profile {
                let from_model = tree_value(payload, &["fromModel"]);
                let model = tree_value(payload, &["model"]);
                match (from_model, model) {
                    (Some(from_model), Some(model)) => Some(format!("{from_model} → {model}")),
                    _ => None,
                }
            } else {
                match (from_profile, to_profile) {
                    (Some(from), Some(to)) => Some(format!("{from} → {to}")),
                    _ => None,
                }
            };
            match (route, context) {
                (Some(route), Some(context)) => Some(format!("{route} · {context}")),
                (route, None) => route,
                (None, Some(context)) => Some(context.into()),
            }
        }
        "steered" => tree_value(payload, &["instruction"]).map(|value| truncate_bytes(&value, 160)),
        "steer_accepted" | "follow_up_queued" | "follow_up_started" | "resumed" => {
            tree_value(payload, &["instruction"]).or_else(|| {
                (event_type == "resumed")
                    .then(|| tree_value(payload, &["previousState"]))
                    .flatten()
                    .map(|state| format!("Resuming from {state} state"))
            })
        }
        "steer_rejected" => {
            tree_value(payload, &["reason"]).map(|reason| format!("Reason: {reason}"))
        }
        "run_interrupted" => tree_value(payload, &["reason"]),
        "queued" => tree_value(payload, &["note"]),
        "needs_input" => tree_value(payload, &["question"]),
        "answered" => tree_value(payload, &["answer"]).map(|value| truncate_bytes(&value, 160)),
        "follow_ups_paused" => number_u64(payload.get("waiting"))
            .map(|waiting| format!("{waiting} waiting until this task finishes cleanly")),
        "follow_ups_dropped" => {
            let dropped = number_u64(payload.get("dropped"));
            let reason = tree_value(payload, &["reason"]);
            match (dropped, reason) {
                (Some(dropped), Some(reason)) => Some(format!("{dropped} dropped ({reason})")),
                (Some(dropped), None) => Some(format!("{dropped} dropped")),
                (None, reason) => reason,
            }
        }
        "completion_asserted" => {
            tree_value(payload, &["reason"]).map(|reason| truncate_bytes(&reason, 160))
        }
        "scope_refusal" => {
            let path = tree_value(payload, &["path"]);
            let error = tree_value(payload, &["error"]);
            match (path, error) {
                (Some(path), Some(error)) => Some(format!("{path} — {error}")),
                (Some(path), None) => Some(path),
                (None, error) => error,
            }
        }
        "oga.spawned" => tree_value(payload, &["orchestratorId"]),
        "oga.decision" => nested_value(payload, &["decision", "action"]),
        "effort_mismatch" => {
            let requested = tree_value(payload, &["requested"]);
            let actual = tree_value(payload, &["actual"]);
            match (requested, actual) {
                (Some(requested), Some(actual)) => {
                    Some(format!("requested {requested}, ran at {actual}"))
                }
                _ => None,
            }
        }
        "handoff_brief" => {
            let tier = tree_value(payload, &["tier"]);
            let chars = number_u64(payload.get("chars"));
            match (tier, chars) {
                (Some(tier), Some(chars)) => Some(format!("{tier} carry-over, {chars} chars")),
                (tier, None) => tier,
                (None, Some(chars)) => Some(format!("{chars} chars")),
            }
        }
        "hold_armed" | "hold_released" => tree_value(payload, &["note"]),
        "hold_released_late" => {
            let note = tree_value(payload, &["note"]);
            let late = number_u64(payload.get("lateMinutes"));
            match (note, late) {
                (Some(note), Some(late)) => Some(format!(
                    "released {late} minutes after expected time — {note}"
                )),
                (None, Some(late)) => Some(format!("released {late} minutes after expected time")),
                (note, None) => note,
            }
        }
        "hold_dropped" => {
            tree_value(payload, &["reason"]).or_else(|| tree_value(payload, &["note"]))
        }
        "hold_expired" => {
            let reason = tree_value(payload, &["reason"]);
            let attempt = number_u64(payload.get("attempt"));
            match (reason, attempt) {
                (Some(reason), Some(attempt)) => Some(format!("{reason} (attempt {attempt})")),
                (reason, None) => reason,
                (None, Some(attempt)) => Some(format!("attempt {attempt}")),
            }
        }
        "hold_cross_cwd" => tree_value(payload, &["blockerId"]).map(|id| format!("Task {id}")),
        "session_rejected" => tree_value(payload, &["profile"])
            .map(|profile| format!("{profile} could not reopen it; starting fresh")),
        "network_retry_scheduled" => {
            let error = tree_value(payload, &["originalError"]);
            let expires = tree_value(payload, &["expiresAt"]);
            match (error, expires) {
                (Some(error), Some(expires)) => {
                    Some(format!("{error} — retries expire at {expires}"))
                }
                (error, None) => error,
                (None, Some(expires)) => Some(format!("retries expire at {expires}")),
            }
        }
        "network_retry_exhausted" => number_u64(payload.get("attempts"))
            .map(|attempts| format!("{attempts} attempts, giving up")),
        // One shape across all three, so a trace that drops output twice reads
        // as one notice with one total rather than a stack of near-identical
        // rows.
        "line_dropped" => {
            let malformed = number_u64(payload.get("malformed")).unwrap_or(0);
            let oversized = number_u64(payload.get("oversized")).unwrap_or(0);
            skipped_detail(malformed + oversized, "line")
        }
        "event_dropped" => skipped_detail(1, "event"),
        "events_truncated" => skipped_detail(number_u64(payload.get("dropped"))?, "event"),
        "scope_auto_completed" => {
            let added = payload
                .get("added")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            (added > 0).then(|| format!("{added} path(s) added to read scope"))
        }
        "scope_inherited" => tree_value(payload, &["approvedFor"])
            .map(|approved_for| format!("originally approved for {approved_for}")),
        "scope_ungranted" => tree_value(payload, &["reason"]),
        "resume_fallback" => number_u64(payload.get("chars"))
            .map(|chars| format!("{chars} chars of message history used"))
            .or_else(|| tree_value(payload, &["reason"])),
        "worker_stderr" => tree_value(payload, &["text"]).map(|value| truncate_bytes(&value, 160)),
        "failed" | "blocked" => nested_value(payload, &["completion", "reason"])
            .or_else(|| tree_value(payload, &["error"])),
        "cancelled" => tree_value(payload, &["error"])
            .or_else(|| nested_value(payload, &["completion", "reason"])),
        "agent.rate_limit_event" => rate_limit_detail(payload),
        _ => tree_value(payload, &["error", "text", "message", "detail"])
            .or_else(|| {
                (event_type == "session_captured")
                    .then(|| "Root provider session mapped".to_owned())
            })
            .or_else(|| tree_value(payload, &["provider", "model"])),
    }
}

fn skipped_detail(count: u64, noun: &str) -> Option<String> {
    (count > 0).then(|| {
        format!(
            "{count} {noun}{} skipped",
            if count == 1 { "" } else { "s" }
        )
    })
}

/// Antigravity's rate-limit notice is actionable only when it says what
/// fraction is used and which window it counts against; the bare status
/// string tells a reader nothing they can act on.
fn rate_limit_detail(payload: &BTreeMap<String, Value>) -> Option<String> {
    let info = payload.get("rate_limit_info").and_then(Value::as_object)?;
    if let Some(windows) = info.get("unifiedWindows").and_then(Value::as_object) {
        // Only the window closest to its ceiling is worth reporting; the reset
        // clock is formatted where the reader's timezone is known.
        let busiest = windows
            .values()
            .filter_map(Value::as_object)
            .filter_map(|window| number_f64(window.get("utilization")))
            .max_by(f64::total_cmp);
        if let Some(utilization) = busiest {
            return Some(format!("{}%", (utilization * 100.0).round() as i64));
        }
    }
    let percent = number_f64(info.get("utilization")).map(|value| (value * 100.0).round() as i64);
    let window = text_value(info.get("rateLimitType")).map(|value| value.replace('_', " "));
    let parts = [
        percent.map(|value| format!("{value}% used")),
        window.map(|value| format!("{value} window")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if parts.is_empty() {
        return text_value(info.get("status")).map(str::to_owned);
    }
    Some(parts.join(" · "))
}

fn parse_kind(value: &str) -> Option<EventKind> {
    serde_json::from_value(Value::String(value.to_owned())).ok()
}
fn parse_phase(value: &str) -> Option<EventPhase> {
    serde_json::from_value(Value::String(value.to_owned())).ok()
}
fn phase_for_state(state: TaskState) -> EventPhase {
    if matches!(
        state,
        TaskState::Failed | TaskState::Blocked | TaskState::Cancelled
    ) {
        EventPhase::Failed
    } else if state == TaskState::Completed {
        EventPhase::Completed
    } else if state == TaskState::Running {
        EventPhase::Started
    } else {
        EventPhase::Info
    }
}

fn event_phase(
    event_type: &str,
    state: TaskState,
    payload: &BTreeMap<String, Value>,
) -> EventPhase {
    if matches!(event_type, "agent.system" | "agent.assistant")
        || (event_type == "agent.hook" && !payload.contains_key("tool_name"))
    {
        EventPhase::Info
    } else {
        phase_for_state(state)
    }
}

fn hook_phase(event_type: &str, state: TaskState) -> EventPhase {
    if event_type.contains("Failure") {
        EventPhase::Failed
    } else if event_type.starts_with("Pre") {
        EventPhase::Started
    } else if event_type.starts_with("Post") {
        EventPhase::Completed
    } else {
        phase_for_state(state)
    }
}

fn tool_title(tool: &str) -> String {
    if let Some(operation) = oga_operation(tool) {
        return oga_tool_title(&operation, None);
    }
    // A connected tool arrives wired as `mcp__server__tool`; only its last
    // segment is something a reader recognises.
    let tool = tool
        .strip_prefix("mcp__")
        .and_then(|rest| rest.rsplit("__").next())
        .unwrap_or(tool);
    match tool.to_ascii_lowercase().as_str() {
        "read" | "read_file" | "view_file" => "Read file".into(),
        "edit" | "multiedit" | "replace_file_content" => "Edit file".into(),
        "write" | "write_file" | "write_to_file" => "Write file".into(),
        "apply_patch" => "Apply patch".into(),
        "bash" | "run_command" | "run command" => "Run command".into(),
        "grep" | "grep_search" | "code_search" => "Search code".into(),
        "glob" | "find_by_name" => "Find files".into(),
        "list_dir" => "List directory".into(),
        "skill" => "Load skill".into(),
        "todowrite" | "todoread" => "Todo list".into(),
        "websearch" | "search_web" => "Web search".into(),
        "webfetch" | "read_url_content" => "Fetch page".into(),
        "task" | "agent" | "invoke_subagent" => "Subagent".into(),
        "manage_task" => "Task status".into(),
        "schedule" => "Schedule check".into(),
        "list_permissions" => "Check permissions".into(),
        _ => humanize(tool),
    }
}

fn tool_title_with_input(tool: &str, input: Option<&Map<String, Value>>) -> String {
    if let Some(call) = oga_call(tool, input, None) {
        return oga_tool_title(&call.operation, call.input);
    }
    if let Some(role) = input_command_role(tool, input) {
        return role.title().into();
    }
    if let Some(patch) = input_patch(tool, input) {
        return patch.title().into();
    }
    if is_captured_output_read(tool, input) {
        return "Read full output".into();
    }
    tool_title(tool)
}

fn input_command_role(tool: &str, input: Option<&Map<String, Value>>) -> Option<CommandRole> {
    matches!(tool_family(tool), ToolFamily::Command)
        .then(|| input_command(input))
        .flatten()
        .as_deref()
        .and_then(command_role)
}

/// A file row names the change, not the act, so it reads the same whether or
/// not the call has landed. Everything else takes its tense from the call.
fn tool_verb(tool: &str, complete: bool) -> String {
    if let Some(operation) = oga_operation(tool) {
        return oga_tool_verb(&operation, None, complete);
    }
    match (tool.to_ascii_lowercase().as_str(), complete) {
        ("read" | "read_file" | "view_file", _) => "Read".into(),
        ("edit" | "multiedit" | "replace_file_content", _) => "Edited".into(),
        ("write" | "write_file" | "write_to_file", _) => "Wrote".into(),
        ("apply_patch", _) => "Applied".into(),
        ("bash" | "run_command" | "run command", true) => "Ran".into(),
        ("bash" | "run_command" | "run command", false) => "Running".into(),
        ("grep" | "grep_search" | "code_search", true) => "Searched".into(),
        ("grep" | "grep_search" | "code_search", false) => "Searching".into(),
        ("glob" | "find_by_name", true) => "Found".into(),
        ("glob" | "find_by_name", false) => "Finding".into(),
        ("list_dir", true) => "Listed".into(),
        ("list_dir", false) => "Listing".into(),
        ("skill", true) => "Loaded".into(),
        ("skill", false) => "Loading".into(),
        ("websearch" | "search_web", true) => "Searched".into(),
        ("websearch" | "search_web", false) => "Searching".into(),
        ("webfetch" | "read_url_content", true) => "Fetched".into(),
        ("webfetch" | "read_url_content", false) => "Fetching".into(),
        ("todowrite" | "todoread", true) => "Updated".into(),
        ("todowrite" | "todoread", false) => "Updating".into(),
        ("task" | "agent" | "invoke_subagent", true) => "Delegated".into(),
        ("task" | "agent" | "invoke_subagent", false) => "Delegating".into(),
        ("manage_task" | "list_permissions", true) => "Checked".into(),
        ("manage_task" | "list_permissions", false) => "Checking".into(),
        ("schedule", true) => "Scheduled".into(),
        ("schedule", false) => "Scheduling".into(),
        (_, true) => "Used".into(),
        (_, false) => "Using".into(),
    }
}

fn tool_verb_with_input(tool: &str, input: Option<&Map<String, Value>>, complete: bool) -> String {
    if let Some(call) = oga_call(tool, input, None) {
        return oga_tool_verb(&call.operation, call.input, complete);
    }
    if let Some(role) = input_command_role(tool, input) {
        return role.verb(complete).into();
    }
    if let Some(patch) = input_patch(tool, input) {
        return patch.verb(complete).into();
    }
    if is_captured_output_read(tool, input) {
        return "Read".into();
    }
    tool_verb(tool, complete)
}

fn lifecycle_title(event_type: &str) -> String {
    match event_type {
        "created" | "queued" => "Task queued",
        "started" | "worker_spawned" => "Worker started",
        "resumed" => "Resumed",
        "completed" => "Task completed",
        "failed" => "Task failed",
        "needs_input" => "Worker needs input",
        "answered" => "Question answered",
        "blocked" => "Task blocked",
        "cancelled" => "Task cancelled",
        "checkout_prepared" => "Checkout ready",
        "checkout_preparation_failed" => "Checkout preparation failed",
        "checkout_removal_started" => "Removing checkout",
        "checkout_removed" => "Checkout removed",
        "checkout_removal_failed" => "Checkout removal failed",
        "line_dropped" | "event_dropped" | "events_truncated" => ACTIVITY_SKIPPED_TITLE,
        "history_dropped" => "History removed",
        "handed_off" => "Handed off",
        "handoff_brief" => "Handoff brief",
        "run_interrupted" => "Run stopped",
        "hold_armed" => "Waiting",
        "hold_released" => "Continuing",
        "hold_released_late" => "Continuing (late)",
        "hold_expired" => "Wait timed out",
        "hold_dropped" => "Waiting ended",
        "hold_cross_cwd" => "Blocked by another task",
        "network_retry_scheduled" => "Network error, retrying",
        "network_retry_exhausted" => "Network retries exhausted",
        "effort_mismatch" => "Effort mismatch",
        "steered" => "Instruction sent",
        "steer_accepted" => "Instruction accepted",
        "steer_rejected" => "Instruction rejected",
        "model_changed" => "Model changed",
        "follow_up_queued" => "Follow-up queued",
        "follow_up_started" => "Follow-up started",
        "follow_ups_paused" => "Follow-ups paused",
        "follow_ups_dropped" => "Follow-ups removed",
        "scope_refusal" => "Write refused by scope",
        "scope_auto_completed" => "Scope expanded",
        "scope_inherited" => "Using approved scope",
        "scope_ungranted" => "Default scope used",
        "resume_fallback" => "Session rebuilt from message history",
        "session_rejected" => "Previous session unavailable",
        "worker_stderr" => "Worker stderr",
        "completion_asserted" => "Completion asserted",
        "oga.orchestrator" => "Orchestrator started",
        "oga.spawned" => "Spawned by orchestrator",
        "oga.decision" => "Orchestrator decided",
        // Anything unnamed still arrives as words rather than a wire symbol.
        other => return humanize(other),
    }
    .to_owned()
}

/// Claude `system` subtypes the trace names and shows; everything else under
/// `agent.system` is provider bookkeeping the reader never asked for.
const NAMED_SYSTEM_SUBTYPES: [&str; 4] = [
    "init",
    "task_started",
    "task_notification",
    "permission_denied",
];

fn system_subtype(payload: &BTreeMap<String, Value>) -> &str {
    text_value(payload.get("subtype")).unwrap_or_default()
}

fn default_kind(event_type: &str, payload: &BTreeMap<String, Value>) -> EventKind {
    match event_type {
        "failed"
        | "blocked"
        | "cancelled"
        | "checkout_preparation_failed"
        | "checkout_removal_failed" => EventKind::Error,
        "scope_refusal" | "hold_expired" | "network_retry_exhausted" => EventKind::Error,
        // Losing a line of output is a gap in the record, not a failed run;
        // it reads as one notice rather than an error per drop.
        "line_dropped" | "event_dropped" | "events_truncated" => EventKind::Lifecycle,
        "agent.system" if NAMED_SYSTEM_SUBTYPES.contains(&system_subtype(payload)) => {
            EventKind::Lifecycle
        }
        "agent.system" | "agent.user" => EventKind::Raw,
        "agent.rate_limit_event" => EventKind::Lifecycle,
        "agent.assistant" | "agent.text" => EventKind::Message,
        "agent.tool_use" => EventKind::Tool,
        "agent.result" => EventKind::Usage,
        "agent.hook" if payload.contains_key("tool_name") => EventKind::Tool,
        "agent.hook" => EventKind::Raw,
        _ => EventKind::Lifecycle,
    }
}

fn event_title(event_type: &str, payload: &BTreeMap<String, Value>) -> String {
    match event_type {
        "agent.system" => match system_subtype(payload) {
            "init" => "Session started".into(),
            "task_started" => SUBAGENT_STARTED_TITLE.into(),
            "task_notification" => SUBAGENT_FINISHED_TITLE.into(),
            "permission_denied" => "Permission needed".into(),
            "" => "System".into(),
            subtype => humanize(subtype),
        },
        "agent.assistant" | "agent.text" => "Agent message".into(),
        "agent.tool_use" => payload
            .get("part")
            .and_then(|value| text_value(value.get("tool")))
            .map(tool_title)
            .unwrap_or_else(|| "Tool call".into()),
        "agent.result" => "Run summary".into(),
        "agent.hook" if payload.contains_key("tool_name") => text_value(payload.get("tool_name"))
            .map(tool_title)
            .unwrap_or_else(|| "Event".into()),
        "agent.hook" => hook_lifecycle_title(payload).unwrap_or_else(|| "Event".into()),
        "agent.user" => "Tool result".into(),
        "agent.rate_limit_event" => "Usage window".into(),
        // Every other provider stream name — `agent.tool_progress`,
        // `agent.item.started`, `agent.step_update` — is a wire symbol. Drop
        // the routing prefix and give the reader words.
        other if other.starts_with("agent.") => humanize(&other["agent.".len()..]),
        _ => lifecycle_title(event_type),
    }
}

/// A wire subtype reads as `compact_boundary`; a person reads "Compact
/// boundary". Anything with no better name at least arrives in their language.
fn humanize(value: &str) -> String {
    let mut words = value.replace(['_', '-', '.'], " ");
    if let Some(first) = words.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    words
}

/// Provider bookkeeping the reader never asked for. It stays in the record and
/// behind the technical toggle, but it is not part of the story of the run.
fn is_minor_event(event_type: &str, payload: &BTreeMap<String, Value>) -> Option<bool> {
    if event_type == "agent.system" {
        return Some(!NAMED_SYSTEM_SUBTYPES.contains(&system_subtype(payload)));
    }
    // A tool heartbeat says only that a call still runs; the call's own row
    // already says that.
    if matches!(event_type, "agent.user" | "agent.tool_progress") {
        return Some(true);
    }

    // Never show: routing, bookkeeping, and high-volume keepalives the
    // catalogue marks as pure plumbing.
    if matches!(
        event_type,
        "heartbeat"
            | "session_captured"
            | "session_reused"
            | "provider_retry"
            | "broker_restarted"
            | "worktree_recreated"
            | "learn_routes"
            | "learn_routes_duplicate"
            | "learn_routes_invalid"
    ) {
        return Some(true);
    }

    // Show only when it matters: hidden unless the payload shows the
    // triggering condition held. `hold_armed`, `hold_released`,
    // `hold_cross_cwd`, and `network_retry_scheduled` need the outcome of a
    // later event (a late release, an exhausted retry, an interesting
    // cross-worktree blocker) that a single-event mapper cannot see, so they
    // stay hidden; the event that carries the outcome (`hold_expired`,
    // `hold_released_late`, `network_retry_exhausted`) always shows instead.
    match event_type {
        "line_dropped" => {
            let malformed = number_u64(payload.get("malformed")).unwrap_or(0);
            let oversized = number_u64(payload.get("oversized")).unwrap_or(0);
            Some(malformed == 0 && oversized == 0)
        }
        "scope_auto_completed" => {
            let empty = match payload.get("added").and_then(Value::as_array) {
                Some(values) => values.is_empty(),
                None => true,
            };
            Some(empty)
        }
        "scope_inherited" => {
            let approved_for = tree_value(payload, &["approvedFor"]);
            let used_by = tree_value(payload, &["usedBy"]);
            Some(approved_for.is_none() || approved_for == used_by)
        }
        "worker_stderr" => {
            let empty = tree_value(payload, &["text"]).is_none();
            Some(empty)
        }
        // A wait nobody asked for — the connection died, the account ran out —
        // is the reader's answer to "why is this task not running", so it shows
        // both when it starts and when it ends. A scheduled start or a
        // prerequisite is plumbing they already know about.
        "hold_armed" | "hold_released" => Some(tree_value(payload, &["wait"]).is_none()),
        "hold_cross_cwd" | "network_retry_scheduled" => Some(true),
        "hold_dropped" => {
            let blocker_state = tree_value(payload, &["blockerState"]).unwrap_or_default();
            let reason = tree_value(payload, &["reason"]).unwrap_or_default();
            // Most `hold_dropped` events carry no structured `blockerState`
            // field at all and only say the blocker's fate in `reason`
            // ("... ended failed"/"blocked"/"cancelled").
            let terminal = matches!(blocker_state.as_str(), "failed" | "cancelled" | "blocked")
                || ["ended failed", "ended blocked", "ended cancelled"]
                    .iter()
                    .any(|marker| reason.contains(marker));
            Some(!terminal)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_never_reach_a_reader() {
        let system =
            BTreeMap::from([("subtype".to_owned(), serde_json::json!("compact_boundary"))]);
        assert_eq!(event_title("agent.system", &system), "Compact boundary");
        assert_eq!(is_minor_event("agent.system", &system), Some(true));

        let init = BTreeMap::from([("subtype".to_owned(), serde_json::json!("init"))]);
        assert_eq!(event_title("agent.system", &init), "Session started");
        assert_ne!(is_minor_event("agent.system", &init), Some(true));

        let empty = BTreeMap::new();
        assert_eq!(event_title("agent.user", &empty), "Tool result");
        assert_eq!(
            event_title("agent.rate_limit_event", &empty),
            "Usage window"
        );
        assert_eq!(lifecycle_title("steer_rejected"), "Instruction rejected");
        assert_eq!(lifecycle_title("follow_ups_dropped"), "Follow-ups removed");
        assert_eq!(lifecycle_title("worktree_recreated"), "Worktree recreated");
        // Every provider stream name, handled or not: none of them may reach a
        // reader as a dotted or underscored symbol.
        for event_type in [
            "agent.system",
            "agent.user",
            "agent.rate_limit_event",
            "agent.tool_progress",
            "agent.item.started",
            "agent.turn.completed",
            "agent.step_update",
            "agent.tool_execution_start",
            "agent.message_update",
        ] {
            let title = event_title(event_type, &empty);
            assert!(
                !title.contains(['.', '_']),
                "{event_type} leaked its wire name as {title:?}"
            );
        }
        assert_eq!(event_title("agent.tool_progress", &empty), "Tool progress");
        assert_eq!(event_title("agent.item.started", &empty), "Item started");
    }

    #[test]
    fn claude_subagent_lifecycle_reads_as_start_and_finish() {
        let started = event_view(
            &provider_event(
                1,
                "agent.system",
                serde_json::json!({
                    "type": "system",
                    "subtype": "task_started",
                    "task_id": "b5r75b4nr",
                    "tool_use_id": "toolu_01SxK3ufYTNnScrVD6a4FK1d",
                    "description": "Test playwright import from bun",
                    "is_backgrounded": false,
                    "task_type": "local_bash"
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(started.title, SUBAGENT_STARTED_TITLE);
        assert_eq!(started.kind, EventKind::Lifecycle);
        assert_eq!(started.phase, EventPhase::Started);
        assert_eq!(
            started.detail.as_deref(),
            Some("Test playwright import from bun")
        );
        assert_ne!(started.minor, Some(true));

        let finished = event_view(
            &provider_event(
                2,
                "agent.system",
                serde_json::json!({
                    "type": "system",
                    "subtype": "task_notification",
                    "task_id": "b5r75b4nr",
                    "tool_use_id": "toolu_01SxK3ufYTNnScrVD6a4FK1d",
                    "status": "completed",
                    "summary": "Test playwright import from bun"
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(finished.title, SUBAGENT_FINISHED_TITLE);
        assert_eq!(finished.phase, EventPhase::Completed);
        assert_eq!(finished.detail, None);
        assert_ne!(finished.minor, Some(true));

        let failed = event_view(
            &provider_event(
                3,
                "agent.system",
                serde_json::json!({
                    "type": "system",
                    "subtype": "task_notification",
                    "task_id": "b5r75b4nr",
                    "status": "failed"
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(failed.phase, EventPhase::Failed);
        assert_eq!(failed.detail.as_deref(), Some("Failed"));
    }

    #[test]
    fn permission_denied_names_the_tool_and_the_reason() {
        let view = event_view(
            &provider_event(
                1,
                "agent.system",
                serde_json::json!({
                    "type": "system",
                    "subtype": "permission_denied",
                    "tool_name": "Write",
                    "tool_use_id": "toolu_01DenQ3JCdsvNhfgMAaf4RS8",
                    "decision_reason_type": "workingDir",
                    "decision_reason": "Path is outside allowed working directories",
                    "message": "Claude requested permissions to write to /tmp/x, but you haven't granted it yet."
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(view.title, "Permission needed");
        assert_eq!(view.minor, Some(false));
        let detail = view.detail.expect("permission detail");
        assert!(detail.contains("Path is outside allowed working directories"));
        let presentation = view.presentation.expect("permission presentation");
        assert_eq!(presentation.kind, PresentationType::Signal);
        assert_eq!(presentation.level, Some(EventLevel::Warning));
    }

    #[test]
    fn signature_only_thinking_still_counts_as_a_block() {
        let signature = event_view(
            &provider_event(
                1,
                "agent.assistant",
                serde_json::json!({
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [{"type": "thinking", "thinking": "", "signature": "CAIStAIK"}]
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(signature.kind, EventKind::Reasoning);
        assert_eq!(signature.title, REASONING_TITLE);
        assert_eq!(signature.detail, None);
        assert_eq!(signature.minor, None);

        let reasoning = event_view(
            &provider_event(
                2,
                "agent.assistant",
                serde_json::json!({
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [{"type": "thinking", "thinking": "Weighing two options", "signature": "CAIStAIK"}]
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(reasoning.kind, EventKind::Reasoning);
        assert_eq!(reasoning.title, REASONING_TITLE);
        assert_eq!(reasoning.detail.as_deref(), Some("Weighing two options"));
        assert_eq!(
            reasoning
                .presentation
                .and_then(|value| value.text)
                .as_deref(),
            Some("Weighing two options")
        );
    }

    #[test]
    fn claude_reasoning_blocks_join_in_reading_order() {
        let view = event_view(
            &provider_event(
                1,
                "agent.assistant",
                serde_json::json!({
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"type": "thinking", "thinking": "First the shape."},
                            {"type": "thinking", "thinking": "", "signature": "CAIStAIK"},
                            {"type": "thinking", "thinking": "Then the cost."}
                        ]
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(
            view.detail.as_deref(),
            Some("First the shape.\n\nThen the cost.")
        );
    }

    #[test]
    fn claude_thinking_token_ticks_are_a_counter_not_a_row() {
        let view = event_view(
            &provider_event(
                1,
                "agent.system",
                serde_json::json!({
                    "type": "system",
                    "subtype": "thinking_tokens",
                    "estimated_tokens": 79,
                    "estimated_tokens_delta": 29
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(view.kind, EventKind::Usage);
        assert_eq!(view.minor, Some(true));
        assert_eq!(view.detail, None);
        assert_eq!(
            view.presentation.and_then(|value| value.tokens_thinking),
            Some(29)
        );
    }

    #[test]
    fn codex_reasoning_item_reads_as_a_thinking_block() {
        let started = event_view(
            &provider_event(
                1,
                "agent.item.started",
                serde_json::json!({"type": "item.started", "item": {"id": "r1", "type": "reasoning"}}),
            ),
            Provider::Codex,
        );
        assert_eq!(started.kind, EventKind::Reasoning);
        assert_eq!(started.detail, None);

        let completed = event_view(
            &provider_event(
                2,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "r1",
                        "type": "reasoning",
                        "summary": ["Weighing two options", {"text": "Picking the smaller one"}]
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(completed.kind, EventKind::Reasoning);
        assert_eq!(
            completed.detail.as_deref(),
            Some("Weighing two options\n\nPicking the smaller one")
        );
    }

    /// turn.completed carries reasoning_output_tokens alongside input/output/cached.
    #[test]
    fn codex_turn_completed_carries_reasoning_tokens() {
        let view = event_view(
            &provider_event(
                1,
                "agent.turn.completed",
                serde_json::json!({
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 1_427_576,
                        "cached_input_tokens": 1_333_504,
                        "cache_write_input_tokens": 0,
                        "output_tokens": 10_885,
                        "reasoning_output_tokens": 2_568
                    }
                }),
            ),
            Provider::Codex,
        );
        let presentation = view
            .presentation
            .expect("turn.completed carries a usage presentation");
        assert_eq!(presentation.tokens_in, Some(94_072));
        assert_eq!(presentation.tokens_out, Some(10_885));
        assert_eq!(presentation.tokens_cached, Some(1_333_504));
        assert_eq!(presentation.tokens_thinking, Some(2_568));
    }

    #[test]
    fn opencode_reports_reasoning_tokens_and_reasoning_parts() {
        let step = event_view(
            &provider_event(
                1,
                "agent.step_finish",
                serde_json::json!({
                    "type": "step_finish",
                    "part": {"reason": "tool-calls", "tokens": {"input": 26913, "output": 67, "reasoning": 89}}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(
            step.presentation.and_then(|value| value.tokens_thinking),
            Some(89)
        );

        let part = event_view(
            &provider_event(
                2,
                "agent.reasoning",
                serde_json::json!({"type": "reasoning", "part": {"type": "reasoning", "text": "Checking the index first"}}),
            ),
            Provider::OpenCode,
        );
        assert_eq!(part.kind, EventKind::Reasoning);
        assert_eq!(part.detail.as_deref(), Some("Checking the index first"));
    }

    #[test]
    fn antigravity_reports_reasoning_tokens_on_the_response_step() {
        let view = event_view(
            &provider_event(
                1,
                "agent.event",
                serde_json::json!({
                    "event": "step_update",
                    "step_update": {
                        "step_type": "agent_response",
                        "usage": {"input_tokens": 29000, "output_tokens": 818, "thinking_tokens": 412}
                    }
                }),
            ),
            Provider::Antigravity,
        );
        assert_eq!(
            view.presentation.and_then(|value| value.tokens_thinking),
            Some(412)
        );
    }

    #[test]
    fn pi_thinking_only_update_reads_as_a_thinking_block() {
        let view = event_view(
            &provider_event(
                1,
                "agent.message_update",
                serde_json::json!({
                    "type": "message_update",
                    "assistantMessageEvent": {"type": "thinking_delta", "delta": " shape"},
                    "message": {
                        "role": "assistant",
                        "content": [{"type": "thinking", "thinking": "First the shape"}]
                    }
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(view.kind, EventKind::Reasoning);
        assert_eq!(view.detail.as_deref(), Some("First the shape"));
    }

    #[test]
    fn long_reasoning_survives_the_signature_beside_it() {
        let thinking = "t".repeat(30_000);
        let signature = "s".repeat(6_000);
        let payload = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "thinking", "thinking": thinking, "signature": signature}]}
        });
        let bounded = bound_event_payload(payload.as_object().unwrap());
        let kept = bounded["message"]["content"][0]["thinking"]
            .as_str()
            .unwrap();
        assert_eq!(
            kept, thinking,
            "reasoning text was trimmed before the signature"
        );
        assert!(
            bounded["message"]["content"][0]["signature"]
                .as_str()
                .unwrap()
                .len()
                < signature.len()
        );
    }

    #[test]
    fn claude_usage_window_reports_the_busiest_window_off_the_trace() {
        let view = event_view(
            &provider_event(
                1,
                "agent.rate_limit_event",
                serde_json::json!({
                    "type": "rate_limit_event",
                    "rate_limit_info": {
                        "status": "allowed",
                        "resetsAt": 1_788_322_800i64,
                        "rateLimitType": "five_hour",
                        "unifiedWindows": {
                            "five_hour": {"utilization": 0.0, "resetsAt": 1_788_322_800i64},
                            "seven_day": {"utilization": 0.13, "resetsAt": 1_788_570_000i64}
                        }
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(view.title, "Usage window");
        assert_eq!(view.detail.as_deref(), Some("13%"));
        assert_eq!(view.minor, Some(true));
    }

    #[test]
    fn tool_progress_heartbeats_stay_off_the_trace() {
        let view = event_view(
            &provider_event(
                1,
                "agent.tool_progress",
                serde_json::json!({
                    "type": "tool_progress",
                    "tool_use_id": "toolu_01Exk-heartbeat-0",
                    "tool_name": "Bash",
                    "parent_tool_use_id": "toolu_01Exk",
                    "elapsed_time_seconds": 30,
                    "heartbeat": true
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(view.title, "Tool progress");
        assert_eq!(view.minor, Some(true));
    }

    #[test]
    fn every_skipped_activity_notice_shares_one_shape() {
        let truncated = event_view(
            &lifecycle_event(
                "events_truncated",
                TaskState::Running,
                BTreeMap::from([
                    ("dropped".into(), serde_json::json!(12)),
                    ("limit".into(), serde_json::json!(5000)),
                ]),
            ),
            Provider::Claude,
        );
        assert_eq!(truncated.title, ACTIVITY_SKIPPED_TITLE);
        assert_eq!(truncated.detail.as_deref(), Some("12 events skipped"));
        assert_eq!(truncated.kind, EventKind::Lifecycle);

        let dropped = event_view(
            &lifecycle_event(
                "event_dropped",
                TaskState::Running,
                BTreeMap::from([("bytes".into(), serde_json::json!(20_000))]),
            ),
            Provider::Claude,
        );
        assert_eq!(dropped.title, ACTIVITY_SKIPPED_TITLE);
        assert_eq!(dropped.detail.as_deref(), Some("1 event skipped"));
    }

    #[test]
    fn hook_tool_input_stays_in_the_activity_story() {
        let event = TaskEvent {
            id: 1,
            task_id: "task".into(),
            kind: "agent.hook".into(),
            state: TaskState::Running,
            payload: BTreeMap::from([
                ("hook_event_name".into(), serde_json::json!("PreToolUse")),
                ("tool_name".into(), serde_json::json!("Bash")),
                ("tool_use_id".into(), serde_json::json!("call-1")),
                (
                    "tool_input".into(),
                    serde_json::json!({"command": "cargo test"}),
                ),
            ]),
            created_at: "2026-01-01T00:00:00Z".into(),
            turn_id: None,
        };

        let view = event_view(&event, Provider::Claude);
        assert_eq!(view.kind, EventKind::Command);
        assert_eq!(view.detail.as_deref(), Some("cargo test"));
        assert_eq!(
            view.presentation
                .as_ref()
                .and_then(|p| p.command.as_deref()),
            Some("cargo test")
        );
        assert_ne!(view.minor, Some(true));
    }

    #[test]
    fn agent_text_is_a_message_with_its_text_as_detail() {
        let event = TaskEvent {
            id: 1,
            task_id: "task".into(),
            kind: "agent.text".into(),
            state: TaskState::Running,
            payload: BTreeMap::from([("text".into(), serde_json::json!("Message body"))]),
            created_at: "2026-01-01T00:00:00Z".into(),
            turn_id: None,
        };

        let view = event_view(&event, Provider::Claude);
        assert_eq!(view.kind, EventKind::Message);
        assert_eq!(view.title, "Agent message");
        assert_eq!(view.detail.as_deref(), Some("Message body"));
    }

    fn opencode_event(id: i64, tool: &str, state: Value) -> TaskEvent {
        provider_event(
            id,
            "agent.tool_use",
            serde_json::json!({
                "type": "tool_use",
                "part": {
                    "type": "tool",
                    "tool": tool,
                    "callID": format!("call_{id}"),
                    "state": state
                }
            }),
        )
    }

    fn provider_event(id: i64, kind: &str, payload: Value) -> TaskEvent {
        TaskEvent {
            id,
            task_id: "task".into(),
            kind: kind.into(),
            state: TaskState::Running,
            payload: payload.as_object().unwrap().clone().into_iter().collect(),
            created_at: "2026-08-31T00:00:00Z".into(),
            turn_id: None,
        }
    }

    #[test]
    fn opencode_todo_tools_show_the_real_item_count() {
        let view = event_view(
            &opencode_event(
                1,
                "todowrite",
                serde_json::json!({
                    "status": "completed",
                    "input": {
                        "todos": [
                            {"content": "Read the files", "status": "completed"},
                            {"content": "Update the view", "status": "in_progress"},
                            {"content": "Run the tests", "status": "pending"}
                        ]
                    },
                    "output": "[{\"content\":\"Read the files\"}]"
                }),
            ),
            Provider::OpenCode,
        );

        assert_eq!(view.title, "Todo list");
        assert_eq!(view.target, None);
        assert_eq!(view.result.as_deref(), Some("3 todos"));
        let presentation = view.presentation.expect("todo presentation");
        assert_eq!(presentation.kind, PresentationType::Todo);
        assert_eq!(presentation.completed, Some(1));
        assert_eq!(presentation.total, Some(3));
    }

    /// Shape taken from a real opencode run: what the agent says arrives as a
    /// `text` part, not a `message` one.
    #[test]
    fn opencode_text_parts_carry_what_the_agent_said() {
        let view = event_view(
            &provider_event(
                1,
                "agent.text",
                serde_json::json!({
                    "type": "text",
                    "part": {
                        "type": "text",
                        "messageID": "msg_05f76073f001flOl4vYx3Zlzkv",
                        "text": "Locating the toast implementation and reference behavior\n",
                        "metadata": {"openai": {"phase": "commentary"}}
                    }
                }),
            ),
            Provider::OpenCode,
        );

        assert_eq!(view.kind, EventKind::Message);
        assert_eq!(
            view.detail.as_deref(),
            Some("Locating the toast implementation and reference behavior")
        );
        assert_eq!(
            view.presentation.and_then(|p| p.text).as_deref(),
            Some("Locating the toast implementation and reference behavior")
        );
    }

    #[test]
    fn opencode_tool_events_keep_subject_kind_verb_and_call_id() {
        let read = event_view(
            &opencode_event(
                1,
                "read",
                serde_json::json!({
                    "status": "completed",
                    "title": "web/package.json",
                    "input": {"filePath": "/repo/web/package.json"},
                    "output": "<path>/repo/web/package.json</path>\n<type>file</type>\n<content>\n1: {\n2:   \"name\": \"oga-web\",\n"
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(
            (read.kind, read.title, read.verb, read.target, read.complete),
            (
                EventKind::File,
                "Read file".into(),
                Some("Read".into()),
                Some("/repo/web/package.json".into()),
                Some(true)
            )
        );
        assert_eq!(read.action_id.as_deref(), Some("call_1"));
        assert_eq!(read.result, None);
        assert_eq!(read.detail.as_deref(), Some("/repo/web/package.json"));

        let command = event_view(
            &opencode_event(
                2,
                "bash",
                serde_json::json!({
                    "status": "completed",
                    "title": "git diff --stat -- rust/crates web/src",
                    "input": {"command": "git diff --stat -- rust/crates web/src"},
                    "metadata": {"output": "3 files changed", "exit": 0}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(command.kind, EventKind::Command);
        assert_eq!(command.title, "Inspect changes");
        assert_eq!(command.verb.as_deref(), Some("Inspected"));
        assert_eq!(
            command.target.as_deref(),
            Some("git diff --stat -- rust/crates web/src")
        );
        assert_ne!(command.kind, EventKind::File);
        assert_eq!(
            command.presentation.as_ref().and_then(|p| p.exit_code),
            Some(0)
        );

        let search = event_view(
            &opencode_event(
                3,
                "grep",
                serde_json::json!({
                    "status": "completed",
                    "title": "trace-[\\w-]+",
                    "input": {"pattern": "trace-[\\w-]+", "path": "web/src"},
                    "metadata": {"matches": 51, "truncated": false}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(search.kind, EventKind::Tool);
        assert_eq!(search.title, "Search code");
        assert_eq!(search.target.as_deref(), Some("trace-[\\w-]+ in web/src"));
        assert_eq!(search.result.as_deref(), Some("51 matches"));

        let output_search = event_view(
            &opencode_event(
                8,
                "grep",
                serde_json::json!({
                    "status": "completed",
                    "input": {"pattern": "TaskEventView", "path": "rust"},
                    "output": "Found 4 matches"
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(
            output_search.target.as_deref(),
            Some("TaskEventView in rust")
        );
        assert_eq!(output_search.result.as_deref(), Some("4 matches"));

        let find = event_view(
            &opencode_event(
                4,
                "glob",
                serde_json::json!({
                    "status": "completed",
                    "title": "",
                    "input": {"pattern": "web/src/**/*.{css,scss,tsx,ts}"},
                    "output": "/repo/web/src/index.css",
                    "metadata": {"count": 99, "truncated": false}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(find.title, "Find files");
        assert_eq!(find.verb.as_deref(), Some("Found"));
        assert_eq!(
            find.target.as_deref(),
            Some("web/src/**/*.{css,scss,tsx,ts}")
        );
        assert_eq!(find.result.as_deref(), Some("99 files"));

        let capped = event_view(
            &opencode_event(
                7,
                "glob",
                serde_json::json!({
                    "status": "completed",
                    "title": "",
                    "input": {"pattern": "**/*.rs"},
                    "metadata": {"count": 1, "truncated": true}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(capped.result.as_deref(), Some("1+ file"));

        let patch = event_view(
            &opencode_event(
                5,
                "apply_patch",
                serde_json::json!({
                    "status": "completed",
                    "input": {"patchText": "*** Begin Patch"},
                    "output": "Applied"
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(patch.kind, EventKind::File);
        assert_eq!(patch.verb.as_deref(), Some("Applied"));
        assert!(
            patch
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("Patch text")
        );

        let skill = event_view(
            &opencode_event(
                6,
                "skill",
                serde_json::json!({
                    "status": "completed",
                    "title": "Loaded skill: refactor",
                    "input": {"name": "refactor"},
                    "output": "<skill_content name=\"refactor\">\n# Skill: refactor",
                    "metadata": {"name": "refactor", "dir": "/skills/refactor"}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(skill.kind, EventKind::Tool);
        assert_eq!(skill.title, "Load skill");
        assert_eq!(skill.verb.as_deref(), Some("Loaded"));
        assert_eq!(skill.target.as_deref(), Some("refactor"));
        assert_eq!(skill.result, None);
        assert_ne!(skill.kind, EventKind::File);
    }

    /// OpenCode sends the whole change as one patchText; without reading it,
    /// every edit reads "Applied Patch text".
    #[test]
    fn opencode_apply_patch_names_the_files_the_patch_touches() {
        let update = event_view(
            &opencode_event(
                1,
                "apply_patch",
                serde_json::json!({
                    "status": "completed",
                    "input": {"patchText": "*** Begin Patch\n*** Update File: web/src/oga.css\n@@\n-  gap: 4px;\n+  gap: 8px;\n+  padding: 0;\n*** End Patch\n"}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(update.kind, EventKind::File);
        assert_eq!(update.title, "Edit file");
        assert_eq!(update.verb.as_deref(), Some("Edited"));
        assert_eq!(update.target.as_deref(), Some("web/src/oga.css"));
        assert_eq!(update.detail.as_deref(), Some("web/src/oga.css · +2 −1"));

        let add = event_view(
            &opencode_event(
                2,
                "apply_patch",
                serde_json::json!({
                    "status": "completed",
                    "input": {"patchText": "*** Begin Patch\n*** Add File: web/src/components/ToastViewport.tsx\n+export function ToastViewport() {}\n*** End Patch\n"}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(add.title, "Write file");
        assert_eq!(add.verb.as_deref(), Some("Wrote"));
        assert_eq!(
            add.target.as_deref(),
            Some("web/src/components/ToastViewport.tsx")
        );

        let many = event_view(
            &opencode_event(
                3,
                "apply_patch",
                serde_json::json!({
                    "status": "completed",
                    "input": {"patchText": "*** Begin Patch\n*** Update File: a.ts\n-old\n+new\n*** Delete File: b.ts\n*** End Patch\n"}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(many.title, "Edit files");
        assert_eq!(many.verb.as_deref(), Some("Edited"));
        assert_eq!(many.target.as_deref(), Some("a.ts"));
        assert_eq!(many.detail.as_deref(), Some("a.ts · 1 more file · +1 −1"));
    }

    /// OpenCode truncates a long tool result and writes the rest to
    /// `~/.local/share/opencode/tool-output/tool_*`; the model reading that
    /// back is the same call again, not a file the run touched.
    #[test]
    fn opencode_reading_captured_output_is_not_a_file_row() {
        let reread = event_view(
            &opencode_event(
                1,
                "read",
                serde_json::json!({
                    "status": "completed",
                    "input": {"filePath": "/Users/x/.local/share/opencode/tool-output/tool_05f7a37f600183HjP"}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(reread.title, "Read full output");
        assert_eq!(reread.minor, Some(true));

        let ordinary = event_view(
            &opencode_event(
                2,
                "read",
                serde_json::json!({
                    "status": "completed",
                    "input": {"filePath": "/repo/web/package.json"}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(ordinary.title, "Read file");
        assert_ne!(ordinary.minor, Some(true));
    }

    /// Every one of these ran in the two traces this folding was measured on.
    /// A shell line is judged whole: plumbing is ignored, a check outranks what
    /// it pipes into, and one unknown program keeps the line a plain command.
    #[test]
    fn shell_commands_are_named_by_what_they_do() {
        let cases = [
            ("cat src/app.ts", "Read file", "Read"),
            (
                "sed -n '466,485p' web/src/bridge/types.ts",
                "Read file",
                "Read",
            ),
            (
                "cd /repo && cat a.ts && echo === && cat b.ts",
                "Read file",
                "Read",
            ),
            ("ls web/src && ls rust", "List directory", "Listed"),
            ("find src -type d | head -50", "Find files", "Found"),
            (
                "rg -n \"Workspace|settings\" src",
                "Search code",
                "Searched",
            ),
            ("git log --oneline -5", "Inspect changes", "Inspected"),
            (
                "cd /repo && git diff --stat",
                "Inspect changes",
                "Inspected",
            ),
            ("wc -l src/app.ts", "Inspect changes", "Inspected"),
            ("bun run lint", "Check lint", "Checked"),
            ("bunx oxlint src/components", "Check lint", "Checked"),
            ("bun run typecheck 2>&1 | tail -8", "Check types", "Checked"),
            ("bun test 2>&1 | tail -30", "Check tests", "Checked"),
            ("cargo clippy -p oga-events", "Check lint", "Checked"),
            ("bun run build", "Check build", "Checked"),
            ("rm -rf scripts && ls", "Run command", "Ran"),
            ("bun scripts/render-settings.mjs", "Run command", "Ran"),
        ];
        for (command, title, verb) in cases {
            let input = serde_json::json!({"command": command});
            let input = input.as_object().unwrap();
            assert_eq!(
                tool_title_with_input("bash", Some(input)),
                title,
                "title for {command}"
            );
            assert_eq!(
                tool_verb_with_input("bash", Some(input), true),
                verb,
                "verb for {command}"
            );
        }
    }

    #[test]
    fn opencode_edit_row_names_the_file_not_the_swapped_strings() {
        let edit = event_view(
            &opencode_event(
                1,
                "edit",
                serde_json::json!({
                    "status": "completed",
                    "input": {
                        "filePath": "web/src/components/Button.tsx",
                        "oldString": "color: blue;",
                        "newString": "color: red;"
                    }
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(edit.verb.as_deref(), Some("Edited"));
        assert_eq!(
            edit.target.as_deref(),
            Some("web/src/components/Button.tsx")
        );
        assert_eq!(
            edit.detail.as_deref(),
            Some("web/src/components/Button.tsx")
        );
        assert_eq!(edit.complete, Some(true));
        assert_eq!(edit.phase, EventPhase::Completed);
    }

    #[test]
    fn opencode_write_row_names_the_file_it_wrote() {
        let write = event_view(
            &opencode_event(
                1,
                "write",
                serde_json::json!({
                    "status": "completed",
                    "input": {"filePath": "src/config.ts", "content": "export const x = 1;\n"}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(write.verb.as_deref(), Some("Wrote"));
        assert_eq!(write.target.as_deref(), Some("src/config.ts"));
        assert_eq!(write.complete, Some(true));
    }

    #[test]
    fn opencode_bash_row_names_the_command_and_finishes() {
        let running = event_view(
            &opencode_event(
                1,
                "bash",
                serde_json::json!({
                    "status": "running",
                    "input": {"command": "bun test web/src/domain/trace"}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(running.title, "Check tests");
        assert_eq!(running.verb.as_deref(), Some("Checking"));
        assert_eq!(running.phase, EventPhase::Started);
        assert_eq!(running.complete, Some(false));

        let done = event_view(
            &opencode_event(
                2,
                "bash",
                serde_json::json!({
                    "status": "completed",
                    "input": {"command": "bun test web/src/domain/trace"},
                    "output": "3 pass\n"
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(done.verb.as_deref(), Some("Checked"));
        assert_eq!(
            done.target.as_deref(),
            Some("bun test web/src/domain/trace")
        );
        assert_eq!(done.phase, EventPhase::Completed);
        assert_eq!(done.complete, Some(true));
    }

    #[test]
    fn opencode_grep_and_glob_rows_never_fall_out_as_read() {
        let grep = event_view(
            &opencode_event(
                1,
                "grep",
                serde_json::json!({
                    "status": "completed",
                    "input": {"pattern": "TODO", "path": "src", "include": "*.ts"},
                    "output": "Found 5 matches\nsrc/main.ts:10: TODO: fix auth"
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(grep.title, "Search code");
        assert_ne!(grep.title, "Read file");
        assert_eq!(grep.verb.as_deref(), Some("Searched"));
        assert_eq!(grep.target.as_deref(), Some("TODO in src"));
        assert_eq!(grep.result.as_deref(), Some("5 matches"));

        let glob = event_view(
            &opencode_event(
                2,
                "glob",
                serde_json::json!({
                    "status": "completed",
                    "input": {"pattern": "src/**/*.ts", "cwd": "app"}
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(glob.title, "Find files");
        assert_ne!(glob.title, "Read file");
        assert_eq!(glob.verb.as_deref(), Some("Found"));
        assert_eq!(glob.target.as_deref(), Some("src/**/*.ts"));
    }

    #[test]
    fn opencode_task_row_names_its_delegated_description() {
        let task = event_view(
            &opencode_event(
                1,
                "task",
                serde_json::json!({
                    "status": "completed",
                    "input": {"description": "Investigate failing test", "model": "sonnet"}
                }),
            ),
            Provider::OpenCode,
        );
        assert_ne!(task.title, "Read file");
        assert_eq!(task.verb.as_deref(), Some("Delegated"));
        assert_eq!(task.target.as_deref(), Some("Investigate failing test"));
        assert_eq!(task.complete, Some(true));
    }

    #[test]
    fn opencode_error_status_carries_the_state_error_as_a_failed_row() {
        let failed = event_view(
            &opencode_event(
                1,
                "read",
                serde_json::json!({
                    "status": "error",
                    "input": {"filePath": "src/missing.ts"},
                    "error": "File not found: src/missing.ts"
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(failed.phase, EventPhase::Failed);
        assert_eq!(failed.complete, Some(true));
        assert_eq!(failed.target.as_deref(), Some("src/missing.ts"));
        assert_eq!(
            failed.result.as_deref(),
            Some("File not found: src/missing.ts")
        );
    }

    #[test]
    fn a_read_row_is_built_from_its_input_never_its_output() {
        let body = "<path>/repo/web/src/domain/trace/index.ts</path>\n<type>file</type>\n<content>\n380:   if (mi\n381:   }\n";
        let slice = event_view(
            &opencode_event(
                1,
                "read",
                serde_json::json!({
                    "status": "completed",
                    "input": {
                        "filePath": "/repo/web/src/domain/trace/index.ts",
                        "offset": 60,
                        "limit": 95
                    },
                    "output": body
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(
            slice.target.as_deref(),
            Some("/repo/web/src/domain/trace/index.ts")
        );
        assert_eq!(slice.result.as_deref(), Some("lines 60–154"));
        for field in [&slice.result, &slice.detail, &slice.target] {
            let text = field.as_deref().unwrap_or_default();
            assert!(!text.contains("<path>"), "the file body leaked: {text}");
            assert!(!text.contains("<content>"), "the file body leaked: {text}");
        }

        let head = event_view(
            &opencode_event(
                2,
                "read",
                serde_json::json!({
                    "status": "completed",
                    "input": {"filePath": "/repo/README.md", "limit": 200},
                    "output": body
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(head.result.as_deref(), Some("lines 1–200"));

        let tail = event_view(
            &opencode_event(
                3,
                "read",
                serde_json::json!({
                    "status": "completed",
                    "input": {"filePath": "/repo/README.md", "offset": 975},
                    "output": body
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(tail.result.as_deref(), Some("from line 975"));

        let whole = event_view(
            &opencode_event(
                4,
                "read",
                serde_json::json!({
                    "status": "completed",
                    "input": {"filePath": "/repo/README.md"},
                    "output": body
                }),
            ),
            Provider::OpenCode,
        );
        assert_eq!(whole.result, None);

        let hooked = event_view(
            &provider_event(
                5,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "Read",
                    "tool_use_id": "toolu_5",
                    "tool_input": {
                        "file_path": "/repo/examples/incident-room/index.html",
                        "offset": 243,
                        "limit": 25
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(hooked.kind, EventKind::File);
        assert_eq!(
            hooked.target.as_deref(),
            Some("/repo/examples/incident-room/index.html")
        );
        assert_eq!(
            hooked
                .presentation
                .as_ref()
                .and_then(|p| p.outcome.as_deref()),
            Some("lines 243–267")
        );

        let assistant = event_view(
            &provider_event(
                6,
                "agent.assistant",
                serde_json::json!({
                    "type": "assistant",
                    "message": {"content": [{
                        "type": "tool_use",
                        "id": "toolu_6",
                        "name": "Read",
                        "input": {
                            "file_path": "/repo/examples/incident-room/test.mjs",
                            "offset": 74,
                            "limit": 3
                        }
                    }]}
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(assistant.kind, EventKind::File);
        assert_eq!(
            assistant.target.as_deref(),
            Some("/repo/examples/incident-room/test.mjs")
        );
        assert_eq!(
            assistant
                .presentation
                .as_ref()
                .and_then(|p| p.outcome.as_deref()),
            Some("lines 74–76")
        );

        // Codex has no read tool; it reads through the shell, and that row
        // stays a command named by its command.
        let shell = event_view(
            &provider_event(
                7,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_2",
                        "type": "command_execution",
                        "command": "/bin/zsh -lc \"sed -n '1,240p' README.md\"",
                        "aggregated_output": "# Oga\n\nGlobal coding-agent switchboard.",
                        "exit_code": 0,
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(shell.kind, EventKind::Command);
        // The shell wrapper is plumbing: the row names the line the model
        // wrote, and the full invocation stays in the expansion.
        assert_eq!(shell.target.as_deref(), Some("sed -n '1,240p' README.md"));
    }

    #[test]
    fn codex_items_name_their_own_subject() {
        let command = event_view(
            &provider_event(
                1,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_2",
                        "type": "command_execution",
                        "command": "/bin/zsh -lc \"sed -n '1,240p' README.md\"",
                        "aggregated_output": "# Oga",
                        "exit_code": 0,
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(command.kind, EventKind::Command);
        assert_eq!(command.title, "Read file");
        assert_eq!(command.verb.as_deref(), Some("Read"));
        assert_eq!(command.target.as_deref(), Some("sed -n '1,240p' README.md"));
        assert_eq!(command.phase, EventPhase::Completed);

        let changed = event_view(
            &provider_event(
                2,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_8",
                        "type": "file_change",
                        "changes": [
                            {"path": "/repo/src/cli.ts", "kind": "update"},
                            {"path": "/repo/tests/store.test.ts", "kind": "update"}
                        ],
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(changed.kind, EventKind::File);
        assert_eq!(changed.title, "Edit files");
        assert_eq!(changed.verb.as_deref(), Some("Edited"));
        assert_eq!(changed.target.as_deref(), Some("/repo/src/cli.ts"));
        assert_eq!(
            changed.detail.as_deref(),
            Some("/repo/src/cli.ts · 1 more file")
        );

        let message = event_view(
            &provider_event(
                3,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_0",
                        "type": "agent_message",
                        "text": "- **Checking**: Current tree and unfinished work"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(message.kind, EventKind::Message);
        assert_eq!(
            message.detail.as_deref(),
            Some("- **Checking**: Current tree and unfinished work")
        );

        // The query comes back empty; the row still names the search.
        let search = event_view(
            &provider_event(
                4,
                "agent.item.started",
                serde_json::json!({
                    "type": "item.started",
                    "item": {
                        "id": "call_9hlK",
                        "type": "web_search",
                        "query": "",
                        "action": {"type": "other"}
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(search.title, "Web search");
        assert_eq!(search.verb.as_deref(), Some("Searching"));
        assert_eq!(search.phase, EventPhase::Started);
        assert_eq!(search.target, None);

        let failed = event_view(
            &provider_event(
                5,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_6",
                        "type": "command_execution",
                        "command": "bun run lint",
                        "exit_code": 1,
                        "status": "failed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(failed.phase, EventPhase::Failed);
        assert_eq!(
            failed.presentation.as_ref().and_then(|p| p.exit_code),
            Some(1)
        );
    }

    /// A command execution's `started`/`completed` pair shares `$.item.id`;
    /// the mapper must carry it as `action_id`/`source_id` so the activity
    /// layer can fold the two into one row. A failing run's detail names its
    /// exit code and the first line of output, per the catalogue.
    #[test]
    fn codex_command_execution_pairs_by_item_id_and_reports_failure_detail() {
        let started = event_view(
            &provider_event(
                1,
                "agent.item.started",
                serde_json::json!({
                    "type": "item.started",
                    "item": {
                        "id": "item_2",
                        "type": "command_execution",
                        "command": "npm test",
                        "status": "in_progress"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(started.action_id.as_deref(), Some("item_2"));
        assert_eq!(
            started.source_id.clone().flatten().as_deref(),
            Some("item_2")
        );
        assert_eq!(started.complete, Some(false));
        assert_eq!(started.phase, EventPhase::Started);
        assert_eq!(started.verb.as_deref(), Some("Checking"));

        let completed = event_view(
            &provider_event(
                2,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_6",
                        "type": "command_execution",
                        "command": "sed -n '1,240p' SKILL.md",
                        "exit_code": 1,
                        "aggregated_output": "sed: SKILL.md: No such file or directory",
                        "status": "failed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(completed.action_id.as_deref(), Some("item_6"));
        assert_eq!(completed.complete, Some(true));
        assert_eq!(completed.phase, EventPhase::Failed);
        assert_eq!(completed.verb.as_deref(), Some("Read"));
        assert_eq!(
            completed.result.as_deref(),
            Some("exit 1: sed: SKILL.md: No such file or directory")
        );
    }

    /// Codex has no dedicated search tool — `rg`/`grep`/`find` run as a plain
    /// `command_execution` shell call, same as Claude's Bash tool and Pi's
    /// `bash` tool call. Verified against a real payload (`~/.oga/oga.db`
    /// id 195595): real `rg` output is bare match lines with no self-reported
    /// count, so the row names the command without inventing one.
    #[test]
    fn codex_search_command_names_the_pattern_from_a_real_run() {
        let search = event_view(
            &provider_event(
                1,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_20",
                        "type": "command_execution",
                        "command": "/bin/zsh -lc 'rg --files node_modules/@modelcontextprotocol/server node_modules/@modelcontextprotocol/core | head -n 120'",
                        "aggregated_output": "node_modules/@modelcontextprotocol/core/package.json\nnode_modules/@modelcontextprotocol/server/package.json\nnode_modules/@modelcontextprotocol/core/README.md\nnode_modules/@modelcontextprotocol/core/LICENSE\nnode_modules/@modelcontextprotocol/server/README.md\nnode_modules/@modelcontextprotocol/server/LICENSE\n",
                        "exit_code": 0,
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(search.kind, EventKind::Tool);
        assert_eq!(search.title, "Search code");
        assert_eq!(search.verb.as_deref(), Some("Searched"));
        // The pager is plumbing, not the search: the row names the pattern,
        // and the full line stays in the expansion underneath.
        assert_eq!(
            search.target.as_deref(),
            Some(
                "rg --files node_modules/@modelcontextprotocol/server node_modules/@modelcontextprotocol/core"
            )
        );
        assert_eq!(search.phase, EventPhase::Completed);
        assert_eq!(search.result, None);
    }

    /// Real payload (`~/.oga/oga.db` id 244241): a bare `rg` command that
    /// exits 1. Codex's own `item.status` calls this "failed" the same way it
    /// would for any other nonzero exit — there is no protocol-level signal
    /// distinguishing "ripgrep found nothing" from a genuine error (here, zsh
    /// failed to glob-expand `[app]` in the path), so the row keeps the exit
    /// detail rather than guessing "0 matches".
    #[test]
    fn codex_search_command_that_fails_keeps_the_error_not_a_guessed_count() {
        let failed = event_view(
            &provider_event(
                1,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_39",
                        "type": "command_execution",
                        "command": r#"/bin/zsh -lc "rg -n \"reportMutationError|catch \\{\" pwa/src/app/a/apps-index.tsx pwa/src/app/a/[app]/content/content-list.tsx""#,
                        "aggregated_output": "zsh:1: no matches found: pwa/src/app/a/[app]/content/content-list.tsx\n",
                        "exit_code": 1,
                        "status": "failed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(failed.title, "Search code");
        assert_eq!(failed.phase, EventPhase::Failed);
        assert_eq!(failed.verb.as_deref(), Some("Searched"));
        assert!(
            failed
                .target
                .as_deref()
                .unwrap_or_default()
                .contains("reportMutationError")
        );
        assert_eq!(
            failed.result.as_deref(),
            Some("exit 1: zsh:1: no matches found: pwa/src/app/a/[app]/content/content-list.tsx")
        );
    }

    /// `changes[].kind` picks the verb (create/delete/update); mixed kinds in
    /// one item fall back to a generic "edited".
    #[test]
    fn codex_file_change_verb_follows_the_change_kind() {
        let created = event_view(
            &provider_event(
                1,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_28",
                        "type": "file_change",
                        "changes": [{"path": "/app/new.ts", "kind": "add"}],
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(created.verb.as_deref(), Some("Created"));
        assert_eq!(created.action_id.as_deref(), Some("item_28"));

        let deleted = event_view(
            &provider_event(
                2,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_29",
                        "type": "file_change",
                        "changes": [{"path": "/app/old.ts", "kind": "delete"}],
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(deleted.verb.as_deref(), Some("Deleted"));
    }

    #[test]
    fn codex_file_change_keeps_attached_git_patches_in_the_view() {
        let patch = "@@ -1,2 +1,2 @@\n old\n-new\n+new\n";
        let view = event_view(
            &provider_event(
                1,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item-30",
                        "type": "file_change",
                        "changes": [{
                            "path": "/repo/src/main.rs",
                            "kind": "update",
                            "patch": patch
                        }],
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );

        assert_eq!(view.kind, EventKind::File);
        assert_eq!(
            view.presentation.as_ref().and_then(|p| p.path.as_deref()),
            Some("/repo/src/main.rs")
        );
        let raw = view.raw_text.expect("raw provider payload");
        let raw: Value = serde_json::from_str(&raw).expect("raw JSON");
        assert_eq!(raw["item"]["changes"][0]["patch"], patch);
    }

    /// The started event's `query` is a draft, often empty; the completed
    /// event's `action.queries` is the real multi-query plan and is what the
    /// row must show, with a count in the detail.
    #[test]
    fn codex_web_search_uses_the_completed_action_queries() {
        let completed = event_view(
            &provider_event(
                1,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "exec-abc",
                        "type": "web_search",
                        "query": "",
                        "action": {
                            "type": "search",
                            "queries": [
                                "site:docs.anthropic.com hooks",
                                "site:github.com/openai hooks"
                            ]
                        },
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(
            completed.target.as_deref(),
            Some("site:docs.anthropic.com hooks")
        );
        assert_eq!(completed.result.as_deref(), Some("2 queries"));
        assert_eq!(completed.action_id.as_deref(), Some("exec-abc"));
    }

    /// The row names the server the call went to alongside the tool, and
    /// summarizes whatever opaque text the result carries.
    #[test]
    fn codex_mcp_tool_call_names_server_and_tool() {
        let completed = event_view(
            &provider_event(
                1,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_11",
                        "type": "mcp_tool_call",
                        "server": "laravel-boost",
                        "tool": "application-info",
                        "arguments": {},
                        "result": {"content": [{"type": "text", "text": "6 packages, 8 versions"}]},
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(
            completed.target.as_deref(),
            Some("laravel-boost/application-info")
        );
        assert_eq!(completed.result.as_deref(), Some("6 packages, 8 versions"));
        assert_eq!(completed.verb.as_deref(), Some("Called"));
    }

    #[test]
    fn oga_mcp_hooks_name_the_action_and_summarize_task_results() {
        let completed = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "oga_tasks",
                    "title": "",
                    "tool_use_id": "toolu_tasks",
                    "tool_input": {
                        "query": "trace presentation",
                        "archived": "active",
                        "fields": ["label"],
                        "limit": 5
                    },
                    "tool_response": {
                        "content": [{
                            "type": "text",
                            "text": "[{\"id\":\"task-1\",\"state\":\"running\"},{\"id\":\"task-2\",\"state\":\"completed\"}]"
                        }]
                    }
                }),
            ),
            Provider::Claude,
        );

        assert_eq!(completed.title, "List tasks");
        assert_eq!(completed.verb.as_deref(), Some("Listed"));
        assert_eq!(completed.target.as_deref(), Some("trace presentation"));
        assert_eq!(completed.result.as_deref(), Some("2 tasks"));
        assert_eq!(
            completed
                .presentation
                .as_ref()
                .and_then(|presentation| presentation.outcome.as_deref()),
            Some("2 tasks")
        );
    }

    #[test]
    fn oga_mcp_codex_calls_use_the_operation_and_result_count() {
        let completed = event_view(
            &provider_event(
                1,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_oga",
                        "type": "mcp_tool_call",
                        "server": "oga",
                        "tool": "query",
                        "arguments": {"q": "trace rows", "cwd": "/repo"},
                        "result": {
                            "content": [{"type": "text", "text": "src/trace.ts:12#TraceRow\nsrc/trace.test.ts:4#trace"}]
                        },
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );

        assert_eq!(completed.title, "Find code");
        assert_eq!(completed.verb.as_deref(), Some("Searched"));
        assert_eq!(completed.target.as_deref(), Some("trace rows"));
        assert_eq!(completed.result.as_deref(), Some("2 matches"));
    }

    #[test]
    fn oga_task_actions_hide_ids_and_report_result_errors() {
        let removed = event_view(
            &provider_event(
                1,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_remove",
                        "type": "mcp_tool_call",
                        "server": "oga",
                        "tool": "worktree-remove",
                        "arguments": {"taskId": "secret-task-id"},
                        "result": {"content": [{"type": "text", "text": "{\"removed\":true}"}]},
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(removed.title, "Remove task copy");
        assert_eq!(removed.target.as_deref(), Some("selected task"));
        assert_eq!(removed.result.as_deref(), Some("Task copy removed"));
        assert!(
            !removed
                .target
                .as_deref()
                .unwrap_or_default()
                .contains("secret-task-id")
        );

        let failed = event_view(
            &provider_event(
                2,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_complete",
                        "type": "mcp_tool_call",
                        "server": "oga",
                        "tool": "complete",
                        "arguments": {"taskId": "task-1"},
                        "result": {"content": [{"type": "text", "text": "{\"error\":{\"message\":\"task is already complete\"}}"}]},
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(
            failed.result.as_deref(),
            Some("Error: task is already complete")
        );
    }

    /// A todo list's verb tracks which phase produced it — planned, checked,
    /// or completed — and the detail names the next incomplete step, or says
    /// so once every step is done.
    #[test]
    fn codex_todo_list_verb_and_detail_track_progress() {
        let planned = event_view(
            &provider_event(
                1,
                "agent.item.started",
                serde_json::json!({
                    "type": "item.started",
                    "item": {
                        "id": "item_5",
                        "type": "todo_list",
                        "items": [
                            {"text": "Step 1", "completed": false},
                            {"text": "Step 2", "completed": false}
                        ],
                        "status": "in_progress"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(planned.verb.as_deref(), Some("Planned"));
        assert_eq!(planned.target.as_deref(), Some("2 steps"));
        assert_eq!(planned.result.as_deref(), Some("Step 1"));
        assert_eq!(planned.action_id.as_deref(), Some("item_5"));

        let checked = event_view(
            &provider_event(
                2,
                "agent.item.updated",
                serde_json::json!({
                    "type": "item.updated",
                    "item": {
                        "id": "item_5",
                        "type": "todo_list",
                        "items": [
                            {"text": "Step 1", "completed": true},
                            {"text": "Step 2", "completed": false}
                        ],
                        "status": "in_progress"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(checked.verb.as_deref(), Some("Checked"));
        assert_eq!(checked.result.as_deref(), Some("Step 2"));

        let completed = event_view(
            &provider_event(
                3,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_5",
                        "type": "todo_list",
                        "items": [
                            {"text": "Step 1", "completed": true},
                            {"text": "Step 2", "completed": true}
                        ],
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(completed.verb.as_deref(), Some("Completed"));
        assert_eq!(completed.result.as_deref(), Some("all done"));
    }

    /// Agent messages and errors are singletons — completed only, no started
    /// event — so they render standalone rather than waiting to pair.
    #[test]
    fn codex_agent_message_and_error_render_standalone() {
        let message = event_view(
            &provider_event(
                1,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_1",
                        "type": "agent_message",
                        "text": "Loading Laravel, Pest, and refactor guidance",
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(message.verb.as_deref(), Some("Said"));
        assert_eq!(
            message.detail.as_deref(),
            Some("Loading Laravel, Pest, and refactor guidance")
        );

        let error = event_view(
            &provider_event(
                2,
                "agent.item.completed",
                serde_json::json!({
                    "type": "item.completed",
                    "item": {
                        "id": "item_0",
                        "type": "error",
                        "message": "Failed to read global AGENTS.md: Operation not permitted",
                        "status": "completed"
                    }
                }),
            ),
            Provider::Codex,
        );
        assert_eq!(error.kind, EventKind::Error);
        assert_eq!(error.phase, EventPhase::Failed);
        assert_eq!(error.verb.as_deref(), Some("Failed"));
        assert_eq!(error.target.as_deref(), Some("Operation not permitted"));
        assert_eq!(
            error.detail.as_deref(),
            Some("Failed to read global AGENTS.md: Operation not permitted")
        );
    }

    #[test]
    fn antigravity_search_steps_name_query_scope_and_count() {
        let search = event_view(
            &provider_event(
                1,
                "agent.event",
                serde_json::json!({
                    "event": "step_update",
                    "step_update": {
                        "state": "DONE",
                        "step_type": "tool",
                        "tool_name": "grep_search",
                        "tool_info": {
                            "parameters": {
                                "Query": "TaskEventView",
                                "SearchPath": "web/src"
                            },
                            "output": "Found 4 matches"
                        }
                    }
                }),
            ),
            Provider::Antigravity,
        );
        assert_eq!(search.kind, EventKind::Tool);
        assert_eq!(search.title, "Search code");
        assert_eq!(search.verb.as_deref(), Some("Searched"));
        assert_eq!(search.target.as_deref(), Some("TaskEventView in web/src"));
        assert_eq!(search.result.as_deref(), Some("4 matches"));
        assert_eq!(search.phase, EventPhase::Completed);
        assert_eq!(search.complete, Some(true));

        let empty = event_view(
            &provider_event(
                2,
                "agent.event",
                serde_json::json!({
                    "event": "step_update",
                    "step_update": {
                        "state": "ACTIVE",
                        "step_type": "tool",
                        "tool_name": "search_web",
                        "tool_info": {
                            "parameters": {"Query": "Rust release", "SearchPath": ""},
                            "output": "No files found"
                        }
                    }
                }),
            ),
            Provider::Antigravity,
        );
        assert_eq!(empty.title, "Web search");
        assert_eq!(empty.target.as_deref(), Some("Rust release"));
        assert_eq!(empty.result.as_deref(), Some("0 files"));
        assert_eq!(empty.phase, EventPhase::Started);
    }

    #[test]
    fn antigravity_oga_mcp_wrapper_uses_nested_arguments() {
        let query = event_view(
            &provider_event(
                1,
                "agent.event",
                serde_json::json!({
                    "event": "step_update",
                    "step_update": {
                        "state": "DONE",
                        "step_type": "tool",
                        "tool_name": "call_mcp_tool",
                        "tool_info": {
                            "parameters": {
                                "ServerName": "oga",
                                "ToolName": "query",
                                "Arguments": {"query": "trace rows"}
                            },
                            "output": "src/trace.ts:12#TraceRow"
                        }
                    }
                }),
            ),
            Provider::Antigravity,
        );

        assert_eq!(query.title, "Find code");
        assert_eq!(query.verb.as_deref(), Some("Searched"));
        assert_eq!(query.target.as_deref(), Some("trace rows"));
        assert_eq!(query.result.as_deref(), Some("1 match"));
    }

    #[test]
    fn claude_search_hooks_name_pattern_scope_and_count() {
        let search = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "Grep",
                    "tool_use_id": "toolu_search",
                    "tool_input": {"pattern": "TaskEventView", "path": "rust"},
                    "tool_response": {"stdout": "Found 2 matches"}
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(search.title, "Search code");
        assert_eq!(search.verb.as_deref(), Some("Searched"));
        assert_eq!(search.target.as_deref(), Some("TaskEventView in rust"));
        assert_eq!(
            search
                .presentation
                .as_ref()
                .and_then(|presentation| presentation.outcome.as_deref()),
            Some("2 matches")
        );

        let assistant = event_view(
            &provider_event(
                2,
                "agent.assistant",
                serde_json::json!({
                    "type": "assistant",
                    "message": {"content": [{
                        "type": "tool_use",
                        "id": "toolu_search_2",
                        "name": "Glob",
                        "input": {"pattern": "web/src/**/*.tsx", "path": "web"}
                    }]}
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(assistant.title, "Find files");
        assert_eq!(assistant.target.as_deref(), Some("web/src/**/*.tsx in web"));
    }

    #[test]
    fn pi_tool_events_keep_search_subjects_from_bash_args() {
        let search = event_view(
            &provider_event(
                1,
                "agent.tool_execution_start",
                serde_json::json!({
                    "type": "tool_execution_start",
                    "toolCallId": "call_pi_search",
                    "toolName": "bash",
                    "args": {"command": "rg -n TaskEventView rust web"}
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(search.kind, EventKind::Tool);
        assert_eq!(search.title, "Search code");
        assert_eq!(search.verb.as_deref(), Some("Searching"));
        assert_eq!(
            search.target.as_deref(),
            Some("rg -n TaskEventView rust web")
        );
        assert_eq!(search.action_id.as_deref(), Some("call_pi_search"));

        let done = event_view(
            &provider_event(
                2,
                "agent.tool_execution_end",
                serde_json::json!({
                    "type": "tool_execution_end",
                    "toolCallId": "call_pi_search",
                    "toolName": "bash",
                    "result": {"content": [{"type": "text", "text": "Found 3 matches"}]},
                    "isError": false
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(done.phase, EventPhase::Completed);
        assert_eq!(done.result.as_deref(), None);
        assert_eq!(
            done.presentation
                .as_ref()
                .and_then(|presentation| presentation.outcome.as_deref()),
            Some("3 matches")
        );
    }

    #[test]
    fn claude_tool_calls_name_their_own_subject() {
        let skill = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Skill",
                    "tool_use_id": "toolu_1",
                    "tool_input": {"skill": "update-config", "args": "allow node"}
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(skill.title, "Load skill");
        assert_eq!(skill.kind, EventKind::Tool);
        assert_eq!(skill.verb.as_deref(), Some("Loading"));
        assert_eq!(skill.target.as_deref(), Some("update-config"));

        let fetch = event_view(
            &provider_event(
                2,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "WebFetch",
                    "tool_use_id": "toolu_2",
                    "tool_input": {
                        "url": "https://example.com/mark.svg",
                        "prompt": "Output the raw contents"
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(fetch.title, "Fetch page");
        assert_eq!(fetch.verb.as_deref(), Some("Fetched"));
        assert_eq!(fetch.phase, EventPhase::Completed);
        assert!(
            fetch
                .target
                .as_deref()
                .unwrap_or_default()
                .contains("https://example.com/mark.svg")
        );

        let connected = event_view(
            &provider_event(
                3,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "mcp__github-cli-local__create_pull_request",
                    "tool_input": {"title": "Fix the trace rows"}
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(connected.title, "Create pull request");

        let assistant = event_view(
            &provider_event(
                4,
                "agent.assistant",
                serde_json::json!({
                    "type": "assistant",
                    "message": {"content": [{
                        "type": "tool_use",
                        "id": "toolu_3",
                        "name": "Bash",
                        "input": {"command": "git status --short", "description": "Check tree"}
                    }]}
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(assistant.kind, EventKind::Command);
        assert_eq!(assistant.title, "Inspect changes");
        assert_eq!(assistant.verb.as_deref(), Some("Inspecting"));
        assert_eq!(assistant.target.as_deref(), Some("git status --short"));
        assert_eq!(assistant.action_id.as_deref(), Some("toolu_3"));
    }

    #[test]
    fn claude_read_row_names_the_file_and_range_from_input() {
        let event = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "Read",
                    "tool_use_id": "toolu_read",
                    "tool_input": {
                        "file_path": "/repo/web/src/bridge/types.ts",
                        "offset": 70,
                        "limit": 140
                    },
                    "tool_response": {
                        "type": "text",
                        "file": {
                            "filePath": "/repo/web/src/bridge/types.ts",
                            "content": "export interface TaskWireOutcome { ... }",
                            "numLines": 500
                        }
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(event.title, "Read file");
        assert_eq!(
            event.target.as_deref(),
            Some("/repo/web/src/bridge/types.ts")
        );
        assert_eq!(event.phase, EventPhase::Completed);
        let detail = event.detail.as_deref().unwrap_or_default();
        assert!(
            !detail.contains("export interface"),
            "the file body leaked into the row: {detail}"
        );
    }

    /// Antigravity's `view_file` carries only `AbsolutePath` in its
    /// parameters — no offset/limit-shaped field exists anywhere in the
    /// payload. The row must still name the file; it must not borrow the
    /// line count that only shows up in the tool's own `output` once done.
    #[test]
    fn antigravity_read_row_names_the_file_with_no_range() {
        let event = event_view(
            &provider_event(
                1,
                "agent.event",
                serde_json::json!({
                    "event": "step_update",
                    "step_update": {
                        "state": "DONE",
                        "step_type": "tool",
                        "tool_name": "view_file",
                        "tool_info": {
                            "parameters": {"AbsolutePath": "/repo/docs/dev-tools-in-sandbox.md"},
                            "output": "224 lines, 13.7 KiB"
                        }
                    }
                }),
            ),
            Provider::Antigravity,
        );
        assert_eq!(event.title, "Read file");
        assert_eq!(event.verb.as_deref(), Some("Read"));
        assert_eq!(
            event.target.as_deref(),
            Some("/repo/docs/dev-tools-in-sandbox.md")
        );
        assert_eq!(
            event
                .presentation
                .as_ref()
                .and_then(|p| p.outcome.as_deref()),
            None
        );
    }

    /// Pi's `read` tool sends only `args.path` — never an offset or limit.
    /// The catalogue's own example row borrows a line count from the
    /// output text, but the mapper must not do that: the range comes from
    /// the payload's input shape or not at all.
    #[test]
    fn pi_read_row_names_the_file_with_no_range() {
        let event = event_view(
            &provider_event(
                1,
                "agent.tool_execution_start",
                serde_json::json!({
                    "type": "tool_execution_start",
                    "toolCallId": "call_pi_read",
                    "toolName": "read",
                    "args": {"path": "src/types.ts"}
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(event.title, "Read file");
        assert_eq!(event.verb.as_deref(), Some("Read"));
        assert_eq!(event.target.as_deref(), Some("src/types.ts"));
        assert_eq!(
            event
                .presentation
                .as_ref()
                .and_then(|p| p.outcome.as_deref()),
            None
        );
    }

    #[test]
    fn claude_bash_row_names_the_command_not_its_stdout() {
        let event = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "Bash",
                    "tool_use_id": "toolu_bash",
                    "duration_ms": 1308,
                    "tool_input": {
                        "command": "ls web/src/domain/trace/",
                        "description": "List trace domain files"
                    },
                    "tool_response": {
                        "stdout": "index.test.ts\nindex.ts",
                        "stderr": "",
                        "returnCodeInterpretation": "Success"
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(event.kind, EventKind::Command);
        assert_eq!(event.title, "List directory");
        assert_eq!(event.verb.as_deref(), Some("Listed"));
        assert_eq!(event.target.as_deref(), Some("ls web/src/domain/trace/"));
        assert_eq!(event.phase, EventPhase::Completed);
        assert_eq!(event.action_id.as_deref(), Some("toolu_bash"));
    }

    #[test]
    fn claude_edit_row_names_the_file_and_the_change() {
        let event = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "Edit",
                    "tool_use_id": "toolu_edit",
                    "tool_input": {
                        "file_path": "/repo/src/main.ts",
                        "old_string": "const x = 1;",
                        "new_string": "const x = 2;",
                        "replace_all": false
                    },
                    "tool_response": {
                        "filePath": "/repo/src/main.ts",
                        "oldString": "const x = 1;",
                        "newString": "const x = 2;"
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(event.title, "Edit file");
        assert_eq!(event.target.as_deref(), Some("/repo/src/main.ts"));
        let detail = event.detail.as_deref().unwrap_or_default();
        assert!(detail.contains("const x = 1;") && detail.contains("const x = 2;"));
        assert_eq!(event.phase, EventPhase::Completed);
    }

    #[test]
    fn claude_grep_row_names_pattern_scope_and_match_count() {
        let event = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "Grep",
                    "tool_use_id": "toolu_grep",
                    "tool_input": {"pattern": "TaskEventView", "path": "rust"},
                    "tool_response": {"stdout": "Found 2 matches"}
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(event.title, "Search code");
        assert_eq!(event.target.as_deref(), Some("TaskEventView in rust"));
        assert_eq!(
            event
                .presentation
                .as_ref()
                .and_then(|p| p.outcome.as_deref()),
            Some("2 matches")
        );
    }

    #[test]
    fn claude_pre_and_post_tool_use_share_one_action_id_for_pairing() {
        let call = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Bash",
                    "tool_use_id": "toolu_pair",
                    "tool_input": {"command": "cargo check -p oga-events"}
                }),
            ),
            Provider::Claude,
        );
        let result = event_view(
            &provider_event(
                2,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "Bash",
                    "tool_use_id": "toolu_pair",
                    "duration_ms": 900,
                    "tool_input": {"command": "cargo check -p oga-events"},
                    "tool_response": {"stdout": "Finished", "stderr": ""}
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(call.phase, EventPhase::Started);
        assert_eq!(call.complete, Some(false));
        assert_eq!(result.phase, EventPhase::Completed);
        assert_eq!(result.complete, Some(true));
        assert_eq!(call.action_id, result.action_id);
        assert_eq!(call.source_id, result.source_id);

        // The result also arrives on a separate agent.user tool_result event;
        // the catalogue says that one carries no actionable content and must
        // not render its own row (it would otherwise duplicate this pair).
        let tool_result_echo = event_view(
            &provider_event(
                3,
                "agent.user",
                serde_json::json!({
                    "type": "user",
                    "message": {
                        "role": "user",
                        "content": [{"type": "tool_result", "tool_use_id": "toolu_pair", "content": []}]
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(tool_result_echo.minor, Some(true));
    }

    #[test]
    fn claude_post_tool_use_failure_carries_the_error_text() {
        let event = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUseFailure",
                    "tool_name": "Bash",
                    "tool_use_id": "toolu_fail",
                    "duration_ms": 225,
                    "is_interrupt": false,
                    "error": "Exit code 2\nerror: store error: sqlite error: Invalid column type Real",
                    "tool_input": {"command": "sqlite3 ~/.oga/oga.db \"select ...\""}
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(event.phase, EventPhase::Failed);
        assert_eq!(
            event.detail.as_deref(),
            Some("Exit code 2\nerror: store error: sqlite error: Invalid column type Real")
        );
    }

    #[test]
    fn claude_agent_tool_row_names_only_the_description_not_the_prompt() {
        let event = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Agent",
                    "tool_use_id": "toolu_agent",
                    "tool_input": {
                        "description": "Audit sidebar & chrome parity",
                        "prompt": "You are auditing a React frontend rewrite of a Rust/Leptos UI, comparing them surface-by-surface for a parity report. This is READ-ONLY research.",
                        "subagent_type": "general-purpose"
                    }
                }),
            ),
            Provider::Claude,
        );
        let target = event.target.as_deref().unwrap_or_default();
        assert!(
            target.starts_with("Audit sidebar & chrome parity"),
            "expected the description to lead the row: {target}"
        );
        assert!(
            !target.contains("READ-ONLY"),
            "the full prompt leaked into the row: {:?}",
            event.target
        );
    }

    #[test]
    fn claude_subagent_lifecycle_hooks_get_a_named_row() {
        let start = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "SubagentStart",
                    "agent_id": "ad85449b7a1c40114",
                    "agent_type": "general-purpose"
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(start.title, SUBAGENT_STARTED_TITLE);
        assert_eq!(start.detail.as_deref(), Some("general-purpose"));
        assert_ne!(start.minor, Some(true));

        let stop = event_view(
            &provider_event(
                2,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "SubagentStop",
                    "agent_id": "ad85449b7a1c40114",
                    "agent_type": "general-purpose"
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(stop.title, SUBAGENT_FINISHED_TITLE);
        assert_eq!(stop.detail.as_deref(), Some("general-purpose"));
        assert_ne!(stop.minor, Some(true));

        let stop_failure = event_view(
            &provider_event(
                3,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "StopFailure",
                    "error": "server_error",
                    "last_assistant_message": "API Error: No response from API"
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(stop_failure.title, "Session error");
        assert_eq!(
            stop_failure.detail.as_deref(),
            Some("API Error: No response from API")
        );
        assert_eq!(stop_failure.phase, EventPhase::Failed);
    }

    /// SubagentStart/SubagentStop share agent_id and agent_type; tool uses
    /// between them stamp the same agent_id. Trace groups on agent_id, so
    /// boundaries must keep the SUBAGENT_*_TITLE pair with agent_id in payload.
    #[test]
    fn claude_hook_subagent_run_keeps_its_agent_id() {
        let agent_id = "a26c5ad830544e025";
        let views = [
            (
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "SubagentStart",
                    "agent_id": agent_id,
                    "agent_type": "general-purpose",
                    "session_id": "1e3e4355-3e49-49d3-b757-d81b16c7ddd5"
                }),
            ),
            (
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Read",
                    "tool_use_id": "toolu_013cJvbaTcGqQM6AU5Q7rQzn",
                    "agent_id": agent_id,
                    "agent_type": "general-purpose",
                    "tool_input": {"file_path": "pwa/src/lib/api.ts"}
                }),
            ),
            (
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PostToolUse",
                    "tool_name": "Read",
                    "tool_use_id": "toolu_013cJvbaTcGqQM6AU5Q7rQzn",
                    "agent_id": agent_id,
                    "agent_type": "general-purpose",
                    "tool_input": {"file_path": "pwa/src/lib/api.ts"}
                }),
            ),
            (
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "SubagentStop",
                    "agent_id": agent_id,
                    "agent_type": "general-purpose",
                    "session_id": "1e3e4355-3e49-49d3-b757-d81b16c7ddd5"
                }),
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (kind, payload))| {
            event_view(
                &provider_event(index as i64 + 1, kind, payload),
                Provider::Claude,
            )
        })
        .collect::<Vec<_>>();
        assert_eq!(views[0].title, SUBAGENT_STARTED_TITLE);
        assert_eq!(views[3].title, SUBAGENT_FINISHED_TITLE);
        for view in &views {
            let raw = view.raw_text.as_deref().unwrap_or_default();
            assert!(
                raw.contains(agent_id),
                "grouping reads agent_id off the raw payload: {raw:?}"
            );
        }
        assert_eq!(
            views[1].action_id.as_deref(),
            Some("toolu_013cJvbaTcGqQM6AU5Q7rQzn")
        );
        assert_eq!(
            views[2].action_id.as_deref(),
            Some("toolu_013cJvbaTcGqQM6AU5Q7rQzn")
        );
    }

    #[test]
    fn claude_thinking_only_message_is_not_an_agent_message() {
        let event = event_view(
            &provider_event(
                1,
                "agent.assistant",
                serde_json::json!({
                    "type": "assistant",
                    "message": {
                        "content": [{"type": "thinking", "thinking": "", "signature": "abc"}]
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(event.title, REASONING_TITLE);
        assert_eq!(event.kind, EventKind::Reasoning);
    }

    #[test]
    fn claude_agent_error_event_gets_a_real_row() {
        let event = event_view(
            &provider_event(
                1,
                "agent.error",
                serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "provider.invalid-request",
                        "message": "Provider request failed with HTTP 404",
                        "status": 404
                    }
                }),
            ),
            Provider::Claude,
        );
        assert_eq!(event.title, "Error");
        assert_eq!(event.kind, EventKind::Error);
        assert_eq!(event.phase, EventPhase::Failed);
        assert_eq!(
            event.detail.as_deref(),
            Some("Provider request failed with HTTP 404 (HTTP 404)")
        );
    }

    #[test]
    fn no_row_reaches_a_reader_without_a_title() {
        let payloads = [
            (
                "agent.tool_use",
                serde_json::json!({"type": "tool_use", "part": {"tool": "glob", "state": {"status": "completed", "title": "", "input": {"pattern": "**/*.rs"}}}}),
            ),
            (
                "agent.tool_use",
                serde_json::json!({"type": "tool_use", "part": {"tool": "", "state": {"status": "completed", "title": ""}}}),
            ),
            (
                "agent.tool_use",
                serde_json::json!({"type": "tool_use", "part": {"tool": "bash", "state": {"status": "running", "title": "", "input": {}}}}),
            ),
            (
                "agent.hook",
                serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "Glob", "tool_input": {"pattern": ""}}),
            ),
            ("agent.hook", serde_json::json!({"hook_event_name": "Stop"})),
            (
                "agent.item.completed",
                serde_json::json!({"type": "item.completed", "item": {"type": "web_search", "query": ""}}),
            ),
            (
                "agent.item.started",
                serde_json::json!({"type": "item.started", "item": {"type": "reasoning"}}),
            ),
            (
                "agent.assistant",
                serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t", "name": "TodoWrite", "input": {}}]}}),
            ),
            (
                "agent.system",
                serde_json::json!({"subtype": "compact_boundary"}),
            ),
            ("agent.user", serde_json::json!({"title": ""})),
            ("worktree_recreated", serde_json::json!({})),
            ("", serde_json::json!({"title": "  "})),
        ];
        for provider in [
            Provider::Claude,
            Provider::Codex,
            Provider::OpenCode,
            Provider::Antigravity,
            Provider::Pi,
        ] {
            for (kind, payload) in &payloads {
                let view = event_view(&provider_event(1, kind, payload.clone()), provider);
                assert!(
                    !view.title.trim().is_empty(),
                    "{provider:?} {kind} rendered a row with no title"
                );
                assert!(
                    !view.title.contains('.'),
                    "{provider:?} {kind} leaked a wire name: {}",
                    view.title
                );
            }
        }
    }

    #[test]
    fn needs_input_and_answered_carry_their_text_as_detail() {
        let question = TaskEvent {
            id: 1,
            task_id: "task".into(),
            kind: "needs_input".into(),
            state: TaskState::NeedsInput,
            payload: BTreeMap::from([(
                "question".into(),
                serde_json::json!("Which branch should this land on?"),
            )]),
            created_at: "2026-01-01T00:00:00Z".into(),
            turn_id: None,
        };
        assert_eq!(
            event_view(&question, Provider::Claude).detail.as_deref(),
            Some("Which branch should this land on?")
        );

        let answered = TaskEvent {
            id: 2,
            task_id: "task".into(),
            kind: "answered".into(),
            state: TaskState::Queued,
            payload: BTreeMap::from([("answer".into(), serde_json::json!("main is fine"))]),
            created_at: "2026-01-01T00:00:01Z".into(),
            turn_id: None,
        };
        assert_eq!(
            event_view(&answered, Provider::Claude).detail.as_deref(),
            Some("main is fine")
        );
    }

    #[test]
    fn bounds_largest_leaf_and_preserves_small_values() {
        let payload = serde_json::json!({"small":"kept", "large":"x".repeat(100_000)})
            .as_object()
            .unwrap()
            .clone();
        let bounded = bound_event_payload(&payload);
        assert!(serde_json::to_vec(&bounded).unwrap().len() <= MAX_EVENT_PAYLOAD_BYTES);
        assert_eq!(bounded["small"], "kept");
        assert!(
            bounded["large"]
                .as_str()
                .unwrap()
                .contains("…[truncated: kept")
        );
    }

    #[test]
    fn third_retry_becomes_error() {
        let view = |id| TaskEventView {
            id,
            task_id: "t".into(),
            source: EventSource::Broker,
            event_type: "agent.tool_use".into(),
            kind: EventKind::Retry,
            phase: EventPhase::Info,
            title: "Edit file".into(),
            detail: Some("miss".into()),
            verb: None,
            target: None,
            result: None,
            presentation: Some(oga_domain::TaskEventPresentation {
                kind: oga_domain::PresentationType::File,
                path: Some("a.rs".into()),
                change: None,
                command: None,
                status: None,
                exit_code: None,
                text: None,
                completed: None,
                total: None,
                cost_usd: None,
                tokens_in: None,
                tokens_out: None,
                tokens_cached: None,
                tokens_thinking: None,
                outcome: None,
                turns: None,
                duration_ms: None,
                level: Some(oga_domain::EventLevel::Warning),
            }),
            raw_text: None,
            created_at: "now".into(),
            parent_action_id: None,
            turn_id: None,
            action_id: None,
            source_id: None,
            complete: None,
            minor: None,
        };
        let views = mark_repeated_retries(vec![view(1), view(2), view(3)]);
        assert_eq!(views[2].kind, EventKind::Error);
        assert_eq!(views[2].phase, EventPhase::Failed);
        assert_eq!(
            views[2].detail.as_deref(),
            Some("miss · 3rd failed attempt")
        );
    }

    #[test]
    fn socket_batch_carries_capped_settlement_outcomes() {
        let task = Task {
            id: "task".into(),
            state: TaskState::Completed,
            tldr: Some("x".repeat(MAX_EVENT_OUTCOME + 1)),
            title: Some("A task".into()),
            ..Task::default()
        };
        let event = oga_domain::WaitedTaskEvent {
            id: 1,
            task_id: task.id.clone(),
            event_type: "completed".into(),
            state: TaskState::Completed,
            at: "now".into(),
            kind: EventKind::Lifecycle,
            summary: "completed".into(),
            minor: None,
        };

        let event = event_to_batch(&event, Some(&task));
        assert_eq!(
            event.outcome.as_deref().map(str::len),
            Some(MAX_EVENT_OUTCOME)
        );
        assert_eq!(event.truncated, Some(true));

        let snapshot = task_to_batch(&task);
        assert_eq!(
            snapshot.tldr.as_deref().map(str::len),
            Some(MAX_EVENT_OUTCOME)
        );
        assert_eq!(snapshot.truncated, Some(true));
    }

    fn lifecycle_event(
        kind: &str,
        state: TaskState,
        payload: BTreeMap<String, Value>,
    ) -> TaskEvent {
        TaskEvent {
            id: 1,
            task_id: "task".into(),
            kind: kind.into(),
            state,
            payload,
            created_at: "2026-01-01T00:00:00Z".into(),
            turn_id: None,
        }
    }

    #[test]
    fn never_show_lifecycle_events_stay_minor() {
        for event_type in [
            "heartbeat",
            "session_captured",
            "session_reused",
            "provider_retry",
            "broker_restarted",
            "worktree_recreated",
            "learn_routes",
            "learn_routes_duplicate",
            "learn_routes_invalid",
        ] {
            let event = lifecycle_event(event_type, TaskState::Running, BTreeMap::new());
            let view = event_view(&event, Provider::Claude);
            assert_eq!(
                view.minor,
                Some(true),
                "{event_type} should never reach the trace as a row"
            );
        }
    }

    #[test]
    fn scope_refusal_is_a_visible_critical_row() {
        let payload = BTreeMap::from([
            (
                "path".into(),
                serde_json::json!("/Users/malico/.oga/oga.db"),
            ),
            (
                "error".into(),
                serde_json::json!(
                    "outside the granted write scope; the sandbox refuses this write"
                ),
            ),
        ]);
        let event = lifecycle_event("scope_refusal", TaskState::Failed, payload);
        let view = event_view(&event, Provider::Claude);
        assert_eq!(view.title, "Write refused by scope");
        assert_eq!(
            view.detail.as_deref(),
            Some(
                "/Users/malico/.oga/oga.db — outside the granted write scope; the sandbox refuses this write"
            )
        );
        assert_eq!(view.kind, EventKind::Error);
        assert_ne!(view.minor, Some(true));
    }

    #[test]
    fn network_retry_exhausted_always_shows() {
        let payload = BTreeMap::from([("attempts".into(), serde_json::json!(12))]);
        let event = lifecycle_event("network_retry_exhausted", TaskState::Failed, payload);
        let view = event_view(&event, Provider::Claude);
        assert_eq!(view.title, "Network retries exhausted");
        assert_eq!(view.detail.as_deref(), Some("12 attempts, giving up"));
        assert_ne!(view.minor, Some(true));
    }

    #[test]
    fn an_unattended_wait_shows_but_a_scheduled_one_does_not() {
        let waiting = BTreeMap::from([
            (
                "note".into(),
                serde_json::json!("Waiting for usage to reset · resumes 14:00"),
            ),
            ("wait".into(), serde_json::json!("rate_limit")),
        ]);
        let view = event_view(
            &lifecycle_event("hold_armed", TaskState::Pending, waiting),
            Provider::Claude,
        );
        assert_eq!(view.title, "Waiting");
        assert_eq!(
            view.detail.as_deref(),
            Some("Waiting for usage to reset · resumes 14:00")
        );
        assert_ne!(view.minor, Some(true));

        let scheduled = BTreeMap::from([("note".into(), serde_json::json!("waiting for task 7"))]);
        let view = event_view(
            &lifecycle_event("hold_armed", TaskState::Pending, scheduled),
            Provider::Claude,
        );
        assert_eq!(view.minor, Some(true));
    }

    #[test]
    fn hold_expired_always_shows() {
        let payload = BTreeMap::from([
            (
                "reason".into(),
                serde_json::json!("network retry attempts exhausted"),
            ),
            ("attempt".into(), serde_json::json!(12)),
        ]);
        let event = lifecycle_event("hold_expired", TaskState::Blocked, payload);
        let view = event_view(&event, Provider::Claude);
        assert_eq!(view.title, "Wait timed out");
        assert_eq!(
            view.detail.as_deref(),
            Some("network retry attempts exhausted (attempt 12)")
        );
        assert_ne!(view.minor, Some(true));
    }

    #[test]
    fn line_dropped_shows_only_when_lines_were_actually_dropped() {
        let clean = lifecycle_event(
            "line_dropped",
            TaskState::Running,
            BTreeMap::from([
                ("malformed".into(), serde_json::json!(0)),
                ("oversized".into(), serde_json::json!(0)),
            ]),
        );
        assert_eq!(event_view(&clean, Provider::Claude).minor, Some(true));

        let lossy = lifecycle_event(
            "line_dropped",
            TaskState::Running,
            BTreeMap::from([
                ("malformed".into(), serde_json::json!(0)),
                ("oversized".into(), serde_json::json!(7)),
            ]),
        );
        let view = event_view(&lossy, Provider::Claude);
        assert_ne!(view.minor, Some(true));
        assert_eq!(view.title, ACTIVITY_SKIPPED_TITLE);
        assert_eq!(view.detail.as_deref(), Some("7 lines skipped"));
        assert_eq!(view.kind, EventKind::Lifecycle);
    }

    #[test]
    fn hold_dropped_shows_only_when_the_blocker_is_terminal() {
        let running = lifecycle_event(
            "hold_dropped",
            TaskState::Running,
            BTreeMap::from([
                (
                    "reason".into(),
                    serde_json::json!("blocker no longer needed"),
                ),
                ("blockerState".into(), serde_json::json!("running")),
            ]),
        );
        assert_eq!(event_view(&running, Provider::Claude).minor, Some(true));

        let failed = lifecycle_event(
            "hold_dropped",
            TaskState::Blocked,
            BTreeMap::from([
                ("reason".into(), serde_json::json!("the blocker failed")),
                ("blockerState".into(), serde_json::json!("failed")),
            ]),
        );
        let view = event_view(&failed, Provider::Claude);
        assert_ne!(view.minor, Some(true));
        assert_eq!(view.title, "Waiting ended");
        assert_eq!(view.detail.as_deref(), Some("the blocker failed"));

        // Real `hold_dropped` rows mostly carry no structured `blockerState`
        // at all — only a free-text reason ending "ended failed/blocked/cancelled".
        let reason_only = lifecycle_event(
            "hold_dropped",
            TaskState::Blocked,
            BTreeMap::from([(
                "reason".into(),
                serde_json::json!("prerequisite 02c8c49b ended failed"),
            )]),
        );
        assert_ne!(event_view(&reason_only, Provider::Claude).minor, Some(true));
    }

    #[test]
    fn worker_spawned_and_handed_off_read_like_a_run_log() {
        let spawned = lifecycle_event(
            "worker_spawned",
            TaskState::Running,
            BTreeMap::from([
                ("provider".into(), serde_json::json!("claude")),
                ("model".into(), serde_json::json!("haiku")),
                ("pid".into(), serde_json::json!(4242)),
            ]),
        );
        let view = event_view(&spawned, Provider::Claude);
        assert_eq!(view.title, "Worker started");
        assert_eq!(view.detail.as_deref(), Some("Claude Haiku"));

        let handed_off = lifecycle_event(
            "handed_off",
            TaskState::Running,
            BTreeMap::from([
                ("fromProfile".into(), serde_json::json!("claude")),
                ("toProfile".into(), serde_json::json!("claude")),
                ("fromModel".into(), serde_json::json!("Claude Opus")),
                ("model".into(), serde_json::json!("Claude Sonnet")),
                ("sessionPreserved".into(), serde_json::json!(true)),
            ]),
        );
        let view = event_view(&handed_off, Provider::Claude);
        assert_eq!(view.title, "Handed off");
        assert_eq!(
            view.detail.as_deref(),
            Some("Claude Opus → Claude Sonnet · conversation carried")
        );
    }

    #[test]
    fn pi_message_stream_collapses_into_one_open_row() {
        let start = event_view(
            &provider_event(
                1,
                "agent.message_start",
                serde_json::json!({
                    "type": "message_start",
                    "message": {"role": "assistant", "content": []}
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(start.kind, EventKind::Message);
        assert_eq!(start.title, "Agent message");
        assert_eq!(start.phase, EventPhase::Started);
        assert_eq!(start.complete, Some(false));
        assert!(start.action_id.is_some());

        // Shape observed in `~/.oga/oga.db` for pi tasks: a `message_update`
        // repeats the growing `message` alongside `assistantMessageEvent`,
        // thinking content first and the text block appended once the model
        // starts responding.
        let thinking_update = event_view(
            &provider_event(
                2,
                "agent.message_update",
                serde_json::json!({
                    "type": "message_update",
                    "assistantMessageEvent": {
                        "type": "thinking_delta",
                        "delta": "The",
                        "partial": {
                            "role": "assistant",
                            "content": [{"type": "thinking", "thinking": "The"}]
                        }
                    }
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(thinking_update.title, REASONING_TITLE);
        assert_eq!(thinking_update.detail.as_deref(), Some("The"));

        let text_update = event_view(
            &provider_event(
                3,
                "agent.message_update",
                serde_json::json!({
                    "type": "message_update",
                    "assistantMessageEvent": {
                        "type": "text_delta",
                        "delta": "## Resume",
                        "partial": {
                            "role": "assistant",
                            "content": [
                                {"type": "thinking", "thinking": "The"},
                                {"type": "text", "text": "## Resume"}
                            ]
                        }
                    }
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(text_update.detail.as_deref(), Some("## Resume"));
        assert_eq!(text_update.action_id, start.action_id);

        let text_update_again = event_view(
            &provider_event(
                4,
                "agent.message_update",
                serde_json::json!({
                    "type": "message_update",
                    "assistantMessageEvent": {
                        "type": "text_delta",
                        "delta": " flag",
                        "partial": {
                            "role": "assistant",
                            "content": [
                                {"type": "thinking", "thinking": "The"},
                                {"type": "text", "text": "## Resume flag"}
                            ]
                        }
                    }
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(text_update_again.detail.as_deref(), Some("## Resume flag"));
        assert_eq!(text_update_again.action_id, start.action_id);

        let end = event_view(
            &provider_event(
                5,
                "agent.message_end",
                serde_json::json!({
                    "type": "message_end",
                    "message": {
                        "role": "assistant",
                        "content": [{"type": "text", "text": "Hello there"}]
                    }
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(end.title, "Agent message");
        assert_eq!(end.phase, EventPhase::Completed);
        assert_eq!(end.complete, Some(true));
        assert_eq!(end.detail.as_deref(), Some("Hello there"));
        // The three events share the one correlation key `foldActions` (the
        // web activity domain) needs to settle them into a single row.
        assert_eq!(end.action_id, start.action_id);

        // A second message opens a fresh row under the same key rather than
        // reopening the one `message_end` already closed.
        let next_start = event_view(
            &provider_event(
                6,
                "agent.message_start",
                serde_json::json!({
                    "type": "message_start",
                    "message": {"role": "user", "content": [{"type": "text", "text": "and then?"}]}
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(next_start.title, "User message");
        assert_eq!(next_start.phase, EventPhase::Started);
        assert_eq!(next_start.action_id, start.action_id);
    }

    #[test]
    fn pi_tool_execution_start_without_matching_end_stays_open() {
        let view = event_view(
            &provider_event(
                1,
                "agent.tool_execution_start",
                serde_json::json!({
                    "type": "tool_execution_start",
                    "toolCallId": "call_pi_open",
                    "toolName": "bash",
                    "args": {"command": "sleep 30"}
                }),
            ),
            Provider::Pi,
        );
        assert_eq!(view.phase, EventPhase::Started);
        assert_eq!(view.complete, Some(false));
        assert_eq!(view.action_id.as_deref(), Some("call_pi_open"));
    }

    #[test]
    fn pi_turn_and_agent_boundaries_stay_minor() {
        for (kind, title) in [
            ("agent.turn_start", "Turn started"),
            ("agent.turn_end", "Turn ended"),
            ("agent.agent_start", "Agent started"),
            ("agent.agent_end", "Agent finished"),
            ("agent.agent_settled", "Agent settled"),
        ] {
            let event_type = kind.strip_prefix("agent.").unwrap();
            let view = event_view(
                &provider_event(1, kind, serde_json::json!({"type": event_type})),
                Provider::Pi,
            );
            assert_eq!(view.title, title, "for {kind}");
            assert_eq!(view.minor, Some(true), "for {kind}");
        }
    }

    #[test]
    fn antigravity_step_types_beyond_tool_cover_agent_response_and_user_input() {
        let response = event_view(
            &provider_event(
                1,
                "agent.event",
                serde_json::json!({
                    "event": "step_update",
                    "step_update": {
                        "step_type": "agent_response",
                        "conversation_id": "conv-1",
                        "step_index": 5,
                        "duration_seconds": 6.27,
                        "usage": {"input_tokens": 2900, "output_tokens": 818, "cache_read_tokens": 2400}
                    }
                }),
            ),
            Provider::Antigravity,
        );
        assert_eq!(response.kind, EventKind::Usage);
        assert_eq!(response.title, "Assistant responded");
        assert_eq!(response.phase, EventPhase::Completed);
        let detail = response.detail.expect("agent_response detail");
        assert!(detail.contains("6.3s"), "{detail}");
        assert!(detail.contains("2900 → 818 tokens"), "{detail}");
        assert!(detail.contains("cached 2400"), "{detail}");

        let user_input = event_view(
            &provider_event(
                2,
                "agent.event",
                serde_json::json!({
                    "event": "step_update",
                    "step_update": {
                        "step_type": "user_input",
                        "conversation_id": "conv-1",
                        "step_index": 0
                    }
                }),
            ),
            Provider::Antigravity,
        );
        assert_eq!(user_input.kind, EventKind::Message);
        assert_eq!(user_input.title, "User input");
        assert_eq!(user_input.detail.as_deref(), Some("step 0"));
    }

    #[test]
    fn antigravity_rate_limit_event_names_utilization_and_window() {
        let view = event_view(
            &provider_event(
                1,
                "agent.rate_limit_event",
                serde_json::json!({
                    "type": "rate_limit_event",
                    "rate_limit_info": {
                        "status": "allowed_warning",
                        "utilization": 0.91,
                        "rateLimitType": "five_hour",
                        "resetsAt": 1_786_115_400i64
                    }
                }),
            ),
            Provider::Antigravity,
        );
        assert_eq!(view.title, "Usage window");
        let detail = view.detail.expect("rate limit detail");
        assert!(detail.contains("91% used"), "{detail}");
        assert!(detail.contains("five hour window"), "{detail}");
    }

    #[test]
    fn antigravity_assistant_protocol_echo_is_suppressed() {
        // Antigravity reuses Claude Code's `agent.assistant` event name for its
        // own internal dialogue, but the tool calls and text it carries are
        // already rendered from `step_update.tool`/`agent_response` — showing
        // this too would duplicate every one of them.
        let view = event_view(
            &provider_event(
                1,
                "agent.assistant",
                serde_json::json!({
                    "type": "assistant",
                    "message": {"content": [{"type": "text", "text": "duplicate of agent_response"}]}
                }),
            ),
            Provider::Antigravity,
        );
        assert_eq!(view.minor, Some(true));
    }

    #[test]
    fn antigravity_run_command_and_edit_use_their_own_param_names() {
        let command = event_view(
            &provider_event(
                1,
                "agent.event",
                serde_json::json!({
                    "event": "step_update",
                    "step_update": {
                        "step_type": "tool",
                        "state": "DONE",
                        "tool_name": "run_command",
                        "tool_info": {
                            "parameters": {"CommandLine": "gh auth status"},
                            "output": "Logged in"
                        }
                    }
                }),
            ),
            Provider::Antigravity,
        );
        assert_eq!(command.kind, EventKind::Command);
        assert_eq!(
            command
                .presentation
                .as_ref()
                .and_then(|p| p.command.as_deref()),
            Some("gh auth status")
        );

        let edit = event_view(
            &provider_event(
                2,
                "agent.event",
                serde_json::json!({
                    "event": "step_update",
                    "step_update": {
                        "step_type": "tool",
                        "state": "ACTIVE",
                        "tool_name": "replace_file_content",
                        "tool_info": {"parameters": {"TargetFile": "/repo/src/ssh/pending.ts"}}
                    }
                }),
            ),
            Provider::Antigravity,
        );
        assert_eq!(edit.title, "Edit file");
        assert_eq!(edit.target.as_deref(), Some("/repo/src/ssh/pending.ts"));
    }

    #[test]
    fn command_summary_skips_setup_and_redirection() {
        assert_eq!(
            command_summary("cd /repo && rg -n \"block_on\" crates 2>/dev/null | head -20"),
            "rg -n \"block_on\" crates"
        );
        assert_eq!(
            command_summary("/bin/zsh -lc 'cd /repo && cargo test -p oga-http'"),
            "cargo test -p oga-http"
        );
    }

    #[test]
    fn command_summary_keeps_an_in_place_edit_whole() {
        // An in-place `sed` writes rather than reads; the summary must not
        // trim it down to something that reads as a lookup.
        let command = "sed -i '' -e 's/a/b/' src/main.rs";
        assert_eq!(command_role(command), None);
        assert_eq!(command_summary(command), command);
    }

    #[test]
    fn worker_helper_tools_read_as_lookups() {
        assert_eq!(
            command_role("oga query \"where is the token refresh handled\""),
            Some(CommandRole::Search)
        );
        assert_eq!(command_role("gh pr view 12"), Some(CommandRole::Inspect));
    }

    #[test]
    fn claude_bash_row_carries_a_short_summary_beside_the_full_command() {
        let command = "cd /repo && cargo check -p oga-events 2>&1 | tail -5";
        let view = event_view(
            &provider_event(
                1,
                "agent.hook",
                serde_json::json!({
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Bash",
                    "tool_use_id": "toolu_summary",
                    "tool_input": {"command": command}
                }),
            ),
            Provider::Claude,
        );
        let presentation = view.presentation.as_ref().expect("command presentation");
        assert_eq!(presentation.command.as_deref(), Some(command));
        assert_eq!(
            presentation.text.as_deref(),
            Some("cargo check -p oga-events")
        );
    }
}
