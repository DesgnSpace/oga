//! Task lifecycle types: identity, state, scope, completion, holds, worktrees.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
    #[serde(rename = "opencode")]
    OpenCode,
    #[serde(rename = "opencode-2")]
    OpenCode2,
    Antigravity,
    Pi,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
            Provider::OpenCode => "opencode",
            Provider::OpenCode2 => "opencode-2",
            Provider::Antigravity => "antigravity",
            Provider::Pi => "pi",
        }
    }
}

/// Task states. `pending` is held: the broker knows when or on what condition
/// the task starts, and it is not before then; it survives restarts untouched.
/// `answered` has no writer — answering moves a row back to `queued` and
/// records an event instead — but the string is pinned by the live schema's
/// `CHECK(state IN (…))` and the desktop app, so it stays until a migration
/// removes it everywhere at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    #[default]
    Queued,
    Pending,
    Running,
    NeedsInput,
    Answered,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    /// The states a watcher stops following: the work can no longer move on
    /// its own. Deliberately wider than cleanup's deletable states, which
    /// exclude `blocked` and `needs_input` because that history still matters.
    pub fn settled(self) -> bool {
        matches!(
            self,
            TaskState::Completed
                | TaskState::Failed
                | TaskState::Cancelled
                | TaskState::Blocked
                | TaskState::NeedsInput
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Pending => "pending",
            TaskState::Running => "running",
            TaskState::NeedsInput => "needs_input",
            TaskState::Answered => "answered",
            TaskState::Blocked => "blocked",
            TaskState::Completed => "completed",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Delegated,
    Orchestrator,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskScope {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

/// How a run ended. `aborted` is a generation that died mid-turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionCode {
    Completed,
    PermissionDenied,
    NeedsAuthority,
    Aborted,
    Cancelled,
    Timeout,
    Auth,
    Billing,
    RateLimit,
    Network,
    WorkerError,
    /// Runs recorded before a clean exit counted as done. Nothing produces it
    /// any more; it stays so those stored completions still load.
    Unverified,
}

/// A caller's correction of a completion the worker never attested. The state
/// moves to `completed` while the original completion survives untouched, so
/// an asserted success stays visibly different from a verified one forever.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCompletionOverride {
    pub asserted_by: String,
    pub reason: String,
    pub asserted_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced_code: Option<CompletionCode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCompletion {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    pub blocked: bool,
    pub code: CompletionCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Scope that would have survived the run's sandbox denials; approved by
    /// resuming with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_scope: Option<TaskScope>,
    /// When the provider's rate-limit window clears, ISO. Only set on
    /// `rate_limit`: wait and resume for free, or hand off now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asserted_completion: Option<TaskCompletionOverride>,
    /// Set only when a failed/cancelled/blocked prerequisite is what dropped
    /// this task into `blocked`, so exactly those dependents go back to
    /// waiting when the prerequisite is resumed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_blocked: Option<bool>,
}

/// One worker run or terminal dependency outcome, kept for later runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAttempt {
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion: Option<TaskCompletion>,
    pub ended_at: String,
    /// Where this run actually ran: after a handoff moves the task's profile
    /// and model, only the attempt remembers which account did earlier work
    /// and which session holds it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldVerb {
    Resume,
    Delegate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnBlockerFailure {
    Hold,
    Run,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkHoldArgs {
    pub attempt: u32,
    pub max_attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_error: Option<String>,
}

/// Marks a hold armed because the run was cut short from outside — the broker
/// stopping, the machine sleeping. Its presence is what makes the park read as
/// picking a run back up rather than waiting on a condition, and it records
/// which pick-up this is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartHoldArgs {
    pub attempt: u32,
}

/// Arguments a hold release replays against its verb.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_blocker_failure: Option<OnBlockerFailure>,
    /// Present only on a network retry hold; carries attempt bookkeeping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkHoldArgs>,
    /// Present only on a hold armed by restart recovery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart: Option<RestartHoldArgs>,
    /// Present only when the caller named the start time. It is what separates
    /// a start somebody chose from a wait on a prerequisite or an account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled: Option<bool>,
}

/// Why a `pending` task has not started. One per task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskHold {
    pub task_id: String,
    /// What release calls.
    pub verb: HoldVerb,
    /// Arguments release replays.
    pub args: HoldArgs,
    /// Clock condition, ISO: not before this instant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<String>,
    /// Availability condition: hold until this profile+model is usable again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub await_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub await_model: Option<String>,
    /// When the sweep next evaluates this hold, ISO.
    pub next_check_at: String,
    /// Give-up time, ISO: past it the hold drops and the task lands blocked.
    pub expires_at: String,
    /// Releases that ran into a still-limited account and re-armed. Capped at 3.
    pub probe_count: u32,
    /// The one human line every surface shows, written at arm time.
    pub note: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldViewKind {
    Time,
    ProfileAvailable,
    Dependency,
    Network,
    /// The run was cut short by something outside it — the broker stopping,
    /// the machine sleeping — and is queued to pick up where it left off.
    Restart,
}

/// What a hold looks like to a caller: the condition, not the mechanics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskHoldView {
    pub kind: HoldViewKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<String>,
    pub note: String,
    pub expires_at: String,
}

/// A task's own checkout of its repository, when it was delegated with one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWorktree {
    /// The repository directory the caller delegated against.
    pub origin_cwd: String,
    /// Root of the checkout the worker runs in.
    pub path: String,
    /// Branch the worker commits on. It survives cleanup; the checkout may not.
    pub branch: String,
    /// Untracked paths the checkout was seeded from, relative to `originCwd`.
    /// Absent when nothing was seeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<String>>,
}

/// Every worktree choice a caller can make in one value. `true` is shorthand
/// for `{}`; absent means the task runs in the caller's own directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorktreeOption {
    Bare(bool),
    Request(WorktreeRequest),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRequest {
    /// Commit, branch or tag the checkout starts from; defaults to HEAD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Branch the work lands on; defaults to `oga/<slug-of-title>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Untracked paths, relative to cwd, linked back to the original directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<Vec<String>>,
    /// Existing task id whose checkout and branch this task joins. Refused
    /// alongside any of the other fields: the joined checkout decided them all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join: Option<String>,
}

/// A listed task row. Timestamps are ISO strings kept verbatim from the wire.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<TaskKind>,
    pub profile_id: String,
    pub model: String,
    pub prompt: String,
    /// The prompt actually sent to the worker: caller text plus memories and
    /// protocol wrapper.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shipped_prompt: Option<String>,
    pub cwd: String,
    /// Branch of this task's repository when dispatch captured it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Present when the run happens in a dedicated checkout instead of the
    /// caller's directory; project-keyed state stays on `originCwd`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<TaskWorktree>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_label: Option<String>,
    pub state: TaskState,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    /// The short-lived orchestrator run that created this delegated task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestrator_id: Option<String>,
    pub scope: TaskScope,
    /// Grant the scope came from; absent means none was stated or on file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    pub allow_questions: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Reasoning effort requested for this run; resume reuses it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// What the provider session actually ran at, read back afterwards; never
    /// guessed from the requested effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_actual: Option<String>,
    /// Caller's one-line handle, what a human reads instead of the prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tldr: Option<String>,
    /// Short label, what a sidebar reads at a glance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion: Option<TaskCompletion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<TaskAttempt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// `cost_usd` was priced from public model rates because the provider
    /// itself never reported an amount.
    #[serde(default)]
    pub cost_usd_estimated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    /// Follow-up instructions waiting to feed into this session once the
    /// current run lands clean. Counted on the single-task read only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued_follow_ups: Option<u64>,
    /// The waiting instructions themselves, oldest first. Single-task read only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued_follow_up_items: Option<Vec<String>>,
    /// Present only while pending: what this task is waiting on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<TaskHoldView>,
    /// Paths handed to the worker alongside the prompt at dispatch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
}

impl Task {
    /// The branch the work ran on: the captured one, or the worktree's own.
    pub fn effective_branch(&self) -> Option<&str> {
        self.branch
            .as_deref()
            .or_else(|| self.worktree.as_ref().map(|w| w.branch.as_str()))
    }
}

/// A task list row: enough to recognise and route on, never the prompt body.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub id: String,
    pub profile_id: String,
    pub model: String,
    pub cwd: String,
    /// The project directory a worktree task was delegated against. Present
    /// only with a worktree, where `cwd` is inside the checkout instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_label: Option<String>,
    pub state: TaskState,
    pub prompt_preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tldr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestrator_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion: Option<TaskCompletion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// `cost_usd` was priced from public model rates because the provider
    /// itself never reported an amount.
    #[serde(default)]
    pub cost_usd_estimated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<TaskHoldView>,
}

/// Whether a mid-run instruction can reach the live worker right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskControlState {
    pub steerable: bool,
    /// Why not, when not. Provider-level wording; clients render their own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What archiving removed or kept of one task's worktree checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeDeleteResult {
    pub task_id: String,
    pub checkout: CheckoutOutcome,
    pub branch: BranchOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutOutcome {
    Removed,
    AlreadyGone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchOutcome {
    Kept,
    Deleted,
    AlreadyGone,
}

/// A worktree removal that did not apply, with the reason it was refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeDeleteSkipped {
    pub task_id: String,
    pub skipped: bool,
    pub state: TaskState,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeDeleteBatchResult {
    pub project: String,
    pub results: Vec<WorktreeDeleteEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorktreeDeleteEntry {
    Applied(WorktreeDeleteResult),
    Skipped(WorktreeDeleteSkipped),
}

/// The worker process behind a `running` row, written where it is spawned so a
/// later broker can still find it. Workers are detached, so they outlive the
/// broker that started them and recovery has to be able to tell a survivor
/// from a corpse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWorker {
    pub pid: u32,
    pub pgid: i32,
    /// The broker that spawned it. A live broker other than this one still
    /// owns its worker, and recovery leaves it alone.
    pub broker_pid: u32,
    pub started_at: String,
}

/// A task that would lose its worker if the broker restarted right now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InFlightTask {
    pub id: String,
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Absent when the task is queued and never spawned: nothing to kill.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListOrder {
    Newest,
    Oldest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchivedFilter {
    Active,
    Only,
    Include,
}

/// Which task field matched a task-list search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskMatch {
    Title,
    Tldr,
    Prompt,
}

/// One-or-many state filter, as the query string accepts it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StateFilter {
    One(TaskState),
    Many(Vec<TaskState>),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskListQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<StateFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<ListOrder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<ArchivedFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// The number of running tasks, counted in SQL so the poll behind it never has
/// to load the task list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityCounts {
    pub running: usize,
}

impl TaskState {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Pending | Self::Running)
    }
}
