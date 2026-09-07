//! Typed broker access shared by the CLI, Oga, the desktop shell, and the UI.
//!
//! This module holds the wire types every consumer needs. The HTTP and SSE
//! transport lives in [`loopback`] and is native only; the desktop web view
//! reaches the broker through the shell's [`bridge`] instead.

pub mod bridge;
#[cfg(not(target_arch = "wasm32"))]
mod loopback;

use std::{collections::BTreeMap, time::Duration};

use oga_domain::{
    ArchivedFilter, BranchOutcome, ConsumerDelivery, Difficulty, EventKind, EventPointer,
    MemoryEntry, MemoryProject, ProfileFailure, ProfileView, ScopeGrant, SpendTotals, Task,
    TaskCompletion, TaskEventView, TaskHoldView, TaskScope, TaskState, TaskSummary, TaskTurn,
    WorkKind, WorktreeOption,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

#[cfg(not(target_arch = "wasm32"))]
pub use loopback::{ClientError, EventStream, LoopbackClient};

/// Query parameters for the broker state snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateQuery {
    pub archived: Option<ArchivedFilter>,
    pub compact: bool,
    pub limit: Option<u64>,
}

impl StateQuery {
    pub fn archived(mut self, archived: ArchivedFilter) -> Self {
        self.archived = Some(archived);
        self
    }

    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// Query parameters for a task or agent event read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskEventsQuery {
    pub after: Option<i64>,
    pub wait_ms: Option<u64>,
    pub last: Option<u64>,
    pub before: Option<i64>,
    pub limit: Option<u64>,
}

impl TaskEventsQuery {
    pub fn after(mut self, after: i64) -> Self {
        self.after = Some(after);
        self
    }

    pub fn wait_ms(mut self, wait_ms: u64) -> Self {
        self.wait_ms = Some(wait_ms);
        self
    }

    pub fn last(mut self, last: u64) -> Self {
        self.last = Some(last);
        self
    }

    pub fn before(mut self, before: i64) -> Self {
        self.before = Some(before);
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// Filters for the cursor-ordered global event stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventStreamQuery {
    pub after: i64,
    pub tasks: Vec<String>,
    pub kinds: Vec<EventKind>,
    pub agents: bool,
}

impl EventStreamQuery {
    pub fn new(after: i64) -> Self {
        Self {
            after,
            ..Self::default()
        }
    }

    pub fn task(mut self, task_id: impl Into<String>) -> Self {
        self.tasks.push(task_id.into());
        self
    }

    pub fn kind(mut self, kind: EventKind) -> Self {
        self.kinds.push(kind);
        self
    }

    pub fn agents(mut self, agents: bool) -> Self {
        self.agents = agents;
        self
    }
}

/// The typed state returned by `GET /api/state`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerState {
    pub profiles: Vec<ProfileView>,
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub profile_failures: Option<Vec<ProfileFailure>>,
    #[serde(default)]
    pub grants: Option<Vec<ScopeGrant>>,
    #[serde(default)]
    pub memory_projects: Vec<MemoryProject>,
    #[serde(default)]
    pub spend: Option<SpendTotals>,
}

/// The lighter state returned by `GET /api/state?view=summary`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerSummaryState {
    pub profiles: Vec<ProfileView>,
    pub tasks: Vec<TaskSummary>,
    #[serde(default)]
    pub tasks_has_more: Option<bool>,
    #[serde(default)]
    pub memory_projects: Vec<MemoryProject>,
    #[serde(default)]
    pub spend: Option<SpendTotals>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBreakdown {
    pub provider: String,
    pub profile: String,
    pub model: String,
    pub cost_usd: f64,
    pub tokens: u64,
    pub tasks: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDay {
    pub date: String,
    pub cost_usd: f64,
    pub tokens: u64,
    pub tasks: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePeriod {
    pub id: String,
    pub label: String,
    pub cost_usd: f64,
    pub tokens: u64,
    pub tasks: u64,
    pub breakdown: Vec<UsageBreakdown>,
    #[serde(default)]
    pub active_days: u64,
    #[serde(default)]
    pub peak_hour: Option<u8>,
    #[serde(default)]
    pub favourite_model: Option<String>,
    #[serde(default)]
    pub days: Vec<UsageDay>,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub end: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageResponse {
    pub periods: Vec<UsagePeriod>,
    #[serde(default)]
    pub current_streak_days: u64,
    #[serde(default)]
    pub longest_streak_days: u64,
}

/// A task event read, normalized across the bare-array and paged responses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEventPage {
    pub events: Vec<TaskEventView>,
    #[serde(default)]
    pub cursor: Option<i64>,
    #[serde(default)]
    pub has_more: Option<bool>,
    #[serde(default)]
    pub oldest_id: Option<i64>,
    #[serde(default)]
    pub has_earlier: Option<bool>,
}

/// The small response shared by task mutation routes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskActionResponse {
    pub id: String,
    pub state: TaskState,
    #[serde(default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub effort_actual: Option<String>,
    #[serde(default)]
    pub cursor: Option<i64>,
    #[serde(default)]
    pub attempt_count: Option<u64>,
    #[serde(default)]
    pub queued_follow_ups: Option<u64>,
    #[serde(default)]
    pub archived_at: Option<String>,
    #[serde(default)]
    pub hold: Option<TaskHoldView>,
    #[serde(default)]
    pub completion: Option<TaskCompletion>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub stopped: Option<bool>,
    #[serde(default)]
    pub checkout: Option<String>,
    #[serde(default)]
    pub branch_outcome: Option<BranchOutcome>,
    #[serde(default)]
    pub branch_reason: Option<String>,
    #[serde(default)]
    pub unarchived: Option<bool>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub worktree_recreated: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TurnsResponse {
    pub turns: Vec<TaskTurn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerInbox {
    pub consumer_id: String,
    pub channel: String,
    pub cursor: i64,
    pub updated_at: String,
    #[serde(default)]
    pub rebaselined: Option<bool>,
    pub deliveries: Vec<ConsumerDelivery>,
}

/// Delivery data returned after an event is acknowledged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenDelivery {
    pub consumer_id: String,
    pub event_id: i64,
    pub channel: String,
    pub status: oga_domain::DeliveryStatus,
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub sent_at: Option<String>,
    #[serde(default)]
    pub seen_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingPreview {
    pub profile_id: String,
    pub model: String,
    pub difficulty: Difficulty,
    pub task_class: oga_domain::TaskClass,
    pub reason: String,
    #[serde(default)]
    pub effort: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MemoryList {
    pub memories: Vec<MemoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectList {
    pub global: String,
    pub projects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptConfig {
    pub cwd: String,
    pub scope: String,
    pub written: bool,
    pub value: String,
    pub inherited: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSettingsSnapshot {
    pub cwd: String,
    pub scope: String,
    pub revision: String,
    pub workers: Vec<WorkerSettings>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerSettings {
    pub id: String,
    pub label: String,
    pub provider: oga_domain::Provider,
    pub enabled: bool,
    pub inherited_enabled: bool,
    pub has_enabled_override: bool,
    pub available_globally: bool,
    pub configured: bool,
    pub models: Vec<ModelSettingsModel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSettingsModel {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub inherited_enabled: bool,
    pub has_enabled_override: bool,
    pub preferred: bool,
    pub inherited_preferred: bool,
    pub has_preferred_override: bool,
    pub capabilities: Vec<String>,
    pub inherited_capabilities: Vec<String>,
    pub has_capabilities_override: bool,
    pub available_globally: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryInitResponse {
    pub file_count: u64,
    pub symbol_count: u64,
    pub partial: bool,
    pub changed: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentStopped {
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentRemoved {
    pub removed: bool,
}

/// Body shared by task and agent dispatch routes.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub prompt: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<TaskScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_questions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_delegate: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The kind of work, when the caller names it, in the same vocabulary
    /// `oga love --when` accepts: a class of work or a subject. Lets a love
    /// rule for it apply even when the prompt never reads that way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<WorkKind>,
    pub tldr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_blocker_failure: Option<oga_domain::OnBlockerFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
}

impl DispatchRequest {
    pub fn new(prompt: impl Into<String>, cwd: impl Into<String>, tldr: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            cwd: cwd.into(),
            tldr: tldr.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResumeRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<TaskScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_questions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue: Option<QueueAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueAction {
    Add,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyRequest {
    pub answer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<TaskScope>,
}

impl ReplyRequest {
    pub fn new(answer: impl Into<String>) -> Self {
        Self {
            answer: answer.into(),
            scope: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteerRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<TaskScope>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompletionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asserted_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryWrite {
    pub cwd: String,
    pub key: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptWrite {
    pub cwd: String,
    pub written: bool,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingPreviewRequest {
    pub cwd: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<WorkKind>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelSettingsUpdate {
    pub cwd: String,
    pub profile_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<Option<bool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred: Option<Option<bool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Option<Vec<String>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCreate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub label: String,
    pub provider: oga_domain::Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProfilePatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<oga_domain::Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    pub cwd: String,
    pub question: String,
    /// The task whose read scope the answer must stay inside, when the lookup
    /// runs for one.
    pub task: Option<String>,
    pub limit: Option<u64>,
    pub code: bool,
}

impl QueryRequest {
    pub fn new(cwd: impl Into<String>, question: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            question: question.into(),
            task: None,
            limit: None,
            code: false,
        }
    }

    pub fn task(mut self, task: impl Into<String>) -> Self {
        self.task = Some(task.into());
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn code(mut self, code: bool) -> Self {
        self.code = code;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryInitRequest {
    pub cwd: String,
    pub force: bool,
}

impl QueryInitRequest {
    pub fn new(cwd: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            force: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventStreamOptions {
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventHead {
    pub cursor: i64,
}

impl Default for EventStreamOptions {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(5),
        }
    }
}

/// A frame whose name is known but whose body does not match its shape.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct FrameError(pub String);

/// The four broker SSE frames, plus a forwards-compatible unknown frame.
#[derive(Debug, Clone, PartialEq)]
pub enum EventFrame {
    Ready(ReadyFrame),
    Task(EventPointer),
    Cursor(CursorFrame),
    Keepalive(CursorFrame),
    Unknown { event: String, data: Value },
}

impl EventFrame {
    /// Reads one frame from the event name and JSON body the broker sent.
    pub fn from_parts(event: &str, data: Value) -> Result<Self, FrameError> {
        fn read<T: DeserializeOwned>(event: &str, data: Value) -> Result<T, FrameError> {
            serde_json::from_value(data)
                .map_err(|error| FrameError(format!("invalid {event} frame: {error}")))
        }

        Ok(match event {
            "ready" => Self::Ready(read(event, data)?),
            "task" => Self::Task(read(event, data)?),
            "cursor" => Self::Cursor(read(event, data)?),
            "keepalive" => Self::Keepalive(read(event, data)?),
            _ => Self::Unknown {
                event: event.to_owned(),
                data,
            },
        })
    }

    /// Splits the frame back into the event name and JSON body it came from.
    pub fn into_parts(self) -> (String, Value) {
        fn write<T: Serialize>(value: &T) -> Value {
            serde_json::to_value(value).expect("frame bodies are plain JSON")
        }

        match self {
            Self::Ready(frame) => ("ready".to_owned(), write(&frame)),
            Self::Task(pointer) => ("task".to_owned(), write(&pointer)),
            Self::Cursor(frame) => ("cursor".to_owned(), write(&frame)),
            Self::Keepalive(frame) => ("keepalive".to_owned(), write(&frame)),
            Self::Unknown { event, data } => (event, data),
        }
    }

    /// The cursor the frame advances the stream to, when it carries one.
    pub fn cursor(&self) -> Option<i64> {
        match self {
            Self::Ready(frame) => Some(frame.cursor),
            Self::Task(pointer) => Some(pointer.cursor),
            Self::Cursor(frame) | Self::Keepalive(frame) => Some(frame.cursor),
            Self::Unknown { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyFrame {
    pub version: u32,
    pub cursor: i64,
    pub stream_floor: i64,
    #[serde(default)]
    pub tasks: Vec<String>,
    #[serde(default)]
    pub kinds: Vec<EventKind>,
    #[serde(default)]
    pub agents: bool,
    #[serde(default)]
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorFrame {
    pub cursor: i64,
}
