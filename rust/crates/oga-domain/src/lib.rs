//! Shared domain types for the Rust rewrite: tasks, profiles, events,
//! scopes, worktrees, holds, completions — and the JSON casing every surface
//! must agree on.
//!
//! Casing rules, defined once here:
//!
//! - Struct field names are camelCase (`#[serde(rename_all = "camelCase")]`),
//!   matching the broker's JSON output exactly.
//! - Enum values keep their TypeScript literal spellings: snake_case state
//!   and code names (`needs_input`, `permission_denied`), kebab-case for
//!   provider-adjacent sources (`caller-profile`, `claude-cli`), and the one
//!   hyphenated provider id `opencode-2`.
//! - Optional fields are omitted, never null; nullable non-optional fields
//!   serialize as explicit `null`. That distinction is load-bearing: clients
//!   treat a present-but-null key differently from an absent one.

pub mod context;
pub mod delivery;
pub mod diff;
pub mod event;
pub mod health;
pub mod profile;
pub mod routing;
pub mod socket;
pub mod store;
pub mod task;
pub mod util;

pub use context::SymbolKind;
pub use delivery::{ConsumerCursor, ConsumerDelivery, DeliveryStatus};
pub use diff::{TaskDiff, TaskDiffBasis, TaskDiffFile, TaskDiffFileStatus};
pub use event::{
    EventKind, EventLevel, EventPhase, EventPointer, EventSource, PresentationType, TaskEvent,
    TaskEventPresentation, TaskEventView, TaskTurn, TaskTurnStatus, WaitedTaskEvent,
};
pub use health::{HealthReport, MCP_CONTRACT_VERSION, Staleness, VERSION};
pub use profile::{Config, MemoryEntry, MemoryProject, Profile, ProfileView, ScopeGrant};
pub use routing::{
    AccountFailure, ChosenRoute, DecidedBy, Difficulty, EffortSource, ModelCost, ModelInfo,
    ModelInfoSource, ModelQuery, ModelSettingsRow, ModelUsageSummary, ObservedRateLimit,
    ProfileUsage, RateLimitByModel, RoutePreference, RoutingRecord, RunnerUp, SelectionDecision,
    SelectionRejection, SelectionRelaxation, SelectionStage, TaskClass, TaskSelection, TaskTopic,
    UsageQuery, UsageSource, UsageWindow, UsageWindowKind, WorkKind,
};
pub use socket::{
    BatchEvent, BatchFrame, BatchTask, ErrorFrame, HelloFrame, HelloPayload, MAX_EVENT_OUTCOME,
    MAX_EVENT_TITLE, OUTCOME_STATES, SubscribeFrame, WireTaskOutcome,
};
pub use store::{
    CleanupPlan, CleanupRecord, CleanupResult, CleanupSettings, CleanupSnapshot, CleanupStateCount,
    FailureCode, ProfileFailure, ProfileSuccess, SpendTotals, WaitSettings,
};
pub use task::{
    ActivityCounts, ArchivedFilter, BranchOutcome, CheckoutOutcome, CompletionCode, HoldArgs,
    HoldVerb, HoldViewKind, InFlightTask, ListOrder, NetworkHoldArgs, OnBlockerFailure, Provider,
    RestartHoldArgs, StateFilter, Task, TaskAttempt, TaskCompletion, TaskCompletionOverride,
    TaskControlState, TaskHold, TaskHoldView, TaskKind, TaskListQuery, TaskMatch, TaskScope,
    TaskState, TaskSummary, TaskWorker, TaskWorktree, WorktreeDeleteBatchResult,
    WorktreeDeleteEntry, WorktreeDeleteResult, WorktreeDeleteSkipped, WorktreeOption,
    WorktreeRequest,
};
