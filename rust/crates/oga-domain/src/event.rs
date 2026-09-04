//! Stored task events, their presentation views, and stream pointers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::task::{Provider, TaskState};

/// A stored task_events row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub id: i64,
    pub task_id: String,
    /// Broker-side type (`agent.message`, `oga.decision`, …).
    #[serde(rename = "type")]
    pub kind: String,
    /// The task state when the row was written; lets a subscriber tell a
    /// settlement from progress without reading the payload back.
    pub state: TaskState,
    #[serde(default)]
    pub payload: BTreeMap<String, Value>,
    pub created_at: String,
    /// Which round of work this row belongs to; absent before turns existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTurnStatus {
    Running,
    Completed,
    Failed,
    NeedsInput,
    Blocked,
    Cancelled,
    Interrupted,
}

/// One round of work: begins only where the broker actually spawns a worker —
/// dispatch, resume, delivered follow-up, restarting handoff. A steer or live
/// model switch folds into the open turn: no new process, no new turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskTurn {
    pub id: i64,
    pub task_id: String,
    /// 1-indexed, per task.
    pub ordinal: u64,
    pub status: TaskTurnStatus,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

/// The coarse classification every consumer tests first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Lifecycle,
    Message,
    Reasoning,
    Tool,
    Command,
    File,
    Error,
    Usage,
    Raw,
    Retry,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Lifecycle => "lifecycle",
            EventKind::Message => "message",
            EventKind::Reasoning => "reasoning",
            EventKind::Tool => "tool",
            EventKind::Command => "command",
            EventKind::File => "file",
            EventKind::Error => "error",
            EventKind::Usage => "usage",
            EventKind::Raw => "raw",
            EventKind::Retry => "retry",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventPhase {
    Info,
    Started,
    Completed,
    Failed,
}

/// Who wrote an event row: the broker itself, or the task's provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventSource {
    Broker,
    Provider(Provider),
}

impl EventSource {
    pub fn from_event_type(event_type: &str, provider: Provider) -> Self {
        if event_type.starts_with("agent.") {
            EventSource::Provider(provider)
        } else {
            EventSource::Broker
        }
    }
}

impl Serialize for EventSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            EventSource::Broker => serializer.serialize_str("broker"),
            EventSource::Provider(provider) => serializer.serialize_str(provider.as_str()),
        }
    }
}

impl<'de> Deserialize<'de> for EventSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "broker" => EventSource::Broker,
            other => {
                EventSource::Provider(Provider::deserialize(serde::de::value::StrDeserializer::<
                    D::Error,
                >::new(other))?)
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationType {
    File,
    Command,
    Message,
    Todo,
    Tool,
    Usage,
    Signal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLevel {
    Info,
    Warning,
    Error,
}

/// What one event looks like rendered: provider-neutral, ready to display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEventView {
    pub id: i64,
    pub task_id: String,
    pub source: EventSource,
    /// Broker-side type, so consumers can route on it — an oga decision row
    /// must never be mistaken for displayable activity.
    #[serde(rename = "type")]
    pub event_type: String,
    pub kind: EventKind,
    pub phase: EventPhase,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation: Option<TaskEventPresentation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
    pub created_at: String,
    /// Provider-declared parent action, when this event ran under another call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_action_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<i64>,
    /// Provider-issued id for the action this event describes. One call
    /// arrives as several rows — echo, pre-hook, post-hook, result — sharing
    /// it, so the trace folds them into one line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
    /// Same value as `actionId`, always present on an action row, so a result
    /// with an equal id patches the call in place rather than appending a near
    /// duplicate. Explicitly null when the provider gives no id — the caller
    /// then matches the newest incomplete row by title. Absent off actions.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::util::double_option"
    )]
    pub source_id: Option<Option<String>>,
    /// Whether the call this row describes has settled. An open row is the
    /// call; a complete row patches it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complete: Option<bool>,
    /// Plumbing the trace folds away by default: token tickers, step
    /// boundaries, duplicate tool results, hook bookkeeping, quiet heartbeats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minor: Option<bool>,
}

/// Structured facts an interface renders without re-parsing JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEventPresentation {
    #[serde(rename = "type")]
    pub kind: PresentationType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_cached: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_thinking: Option<u64>,
    /// What the action produced once its result arrived — lines read, lines
    /// changed, output emitted. Folded onto the calling row, so the result
    /// event itself never needs one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<EventLevel>,
}

/// An event row as a waiter returns it, plus what a reader needs to decide
/// whether the row is worth showing: `kind` and `minor`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitedTaskEvent {
    pub id: i64,
    pub task_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub state: TaskState,
    pub at: String,
    pub kind: EventKind,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minor: Option<bool>,
}

/// A pointer frame on the broker's SSE stream: cursor-ordered, no payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPointer {
    pub id: i64,
    pub cursor: i64,
    pub task_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub kind: EventKind,
    pub state: TaskState,
    pub at: String,
    pub title: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<i64>,
}
