//! Shared domain types and the JSON casing used by every surface.

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
    AccountFailure, AdvisedRoute, ChosenRoute, DecidedBy, Difficulty, EffortSource, ModelCost,
    ModelInfo, ModelInfoSource, ModelQuery, ModelSettingsRow, ModelUsageSummary, ObservedRateLimit,
    ProfileUsage, RateLimitByModel, RoutePreference, RoutingRecord, RunnerUp, SelectionDecision,
    SelectionRejection, SelectionRelaxation, SelectionStage, TaskClass, TaskSelection, TaskTopic,
    UsageQuery, UsageSource, UsageWindow, UsageWindowKind, WorkKind,
};
pub use socket::{
    BatchEvent, BatchFrame, BatchTask, ErrorFrame, HelloFrame, HelloPayload, MAX_EVENT_OUTCOME,
    MAX_EVENT_TITLE, OUTCOME_STATES, SubscribeFrame, WireTaskOutcome,
};
pub use store::{
    AdvisorSettings, CleanupPlan, CleanupRecord, CleanupResult, CleanupSettings, CleanupSnapshot,
    CleanupStateCount, FailureCode, ProfileFailure, ProfileSuccess, SpendTotals, WaitSettings,
};
pub use task::{
    AcpAgentIdentity, AcpRestore, AcpSteering, ActivityCounts, ArchivedFilter, BranchOutcome,
    CheckoutOutcome, CompletionCode, HoldArgs, HoldVerb, HoldViewKind, InFlightTask, ListOrder,
    NetworkHoldArgs, OnBlockerFailure, Provider, RestartHoldArgs, StateFilter, Task, TaskAttempt,
    TaskCompletion, TaskCompletionOverride, TaskControlState, TaskHold, TaskHoldView, TaskKind,
    TaskListQuery, TaskMatch, TaskScope, TaskState, TaskSummary, TaskTransport, TaskWorker,
    TaskWorktree, Transport, TransportPreference, TransportReason, WorktreeDeleteBatchResult,
    WorktreeDeleteEntry, WorktreeDeleteResult, WorktreeDeleteSkipped, WorktreeOption,
    WorktreeRequest,
};
