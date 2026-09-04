//! The Unix event socket's NDJSON frames. Unknown keys are ignored by both
//! sides, so every frame deserializes with `#[serde(default)]`-style leniency.

use serde::{Deserialize, Serialize};

use crate::event::EventKind;
use crate::task::{CompletionCode, TaskState};

/// The wire's own caps, shared by the server and `oga watch` — the only
/// consumer — so neither can drift. A field cut at its cap is flagged.
pub const MAX_EVENT_TITLE: usize = 80;
pub const MAX_EVENT_OUTCOME: usize = 200;

/// client→server, one NDJSON line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeFrame {
    pub v: u8,
    #[serde(default)]
    pub watch: Vec<String>,
    pub after_cursor: i64,
}

/// server→client: build identity plus where to resume from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloFrame {
    pub hello: HelloPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloPayload {
    pub version: String,
    pub mcp_contract_version: u32,
    /// Max event cursor for the subscribed set, so a client can skip history
    /// without opening the store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_cursor: Option<i64>,
    /// Oldest surviving event id for the subscribed set; 0 when none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_floor: Option<i64>,
    /// True when the subscribe's afterCursor predates surviving history: a
    /// saved cursor is rebasing, not resuming.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
}

/// The settle and needs_input states whose outcome the wire carries.
pub const OUTCOME_STATES: &[TaskState] = &[
    TaskState::NeedsInput,
    TaskState::Completed,
    TaskState::Failed,
    TaskState::Blocked,
    TaskState::Cancelled,
];

/// The one-line outcome a settle carries: `completed` reads the TLDR,
/// `needs_input` the question, the rest the completion reason.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireTaskOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The completion code, for failed and blocked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<CompletionCode>,
    /// A carried field was cut at its cap.
    pub truncated: bool,
    /// The expected outcome is not on the wire; only the record has it.
    pub more: bool,
}

/// One batched event line for `oga watch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchEvent {
    pub id: i64,
    pub task_id: String,
    pub kind: EventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minor: Option<bool>,
    pub state: TaskState,
    pub at: String,
    pub summary: String,
    /// Task title, capped at [`MAX_EVENT_TITLE`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// For a settle or needs_input: the one-line outcome, capped at
    /// [`MAX_EVENT_OUTCOME`]; `truncated` says when it was cut.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// For failed and blocked: the completion code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<CompletionCode>,
    /// A variable-length field was cut at its cap; the record holds it all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// The outcome is not in this event — inspect to learn it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more: Option<bool>,
}

/// One batched task line for `oga watch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchTask {
    pub id: String,
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    /// The TLDR, carried on a completed task. Capped; `truncated` says when.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tldr: Option<String>,
    /// For failed and blocked: the completion code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<CompletionCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more: Option<bool>,
}

/// server→client: events and tasks changed since the last batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchFrame {
    pub events: Vec<BatchEvent>,
    pub tasks: Vec<BatchTask>,
    pub cursor: i64,
    pub has_more: bool,
}

/// server→client, then the connection closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorFrame {
    pub error: String,
}
