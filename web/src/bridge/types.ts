// Wire types shared with the Rust broker. Mirrors `oga_client::bridge::BrokerCall`
// (rust/crates/oga-client/src/bridge.rs) and the domain types it carries.

// --- Shared domain enums --------------------------------------------------

export type Provider = "claude" | "codex" | "opencode" | "opencode-2" | "antigravity" | "pi";

export type TaskState =
  | "queued"
  | "pending"
  | "running"
  | "needs_input"
  | "answered"
  | "blocked"
  | "completed"
  | "failed"
  | "cancelled";

export type BranchOutcome = "kept" | "deleted" | "already_gone";

export interface ArchiveTaskResponse {
  id: string;
  state: TaskState;
  stopped?: boolean;
  checkout?: string;
  branchOutcome?: BranchOutcome;
  branchReason?: string;
}

export type ArchivedFilter = "active" | "only" | "include";

export type CompletionCode =
  | "completed"
  | "permission_denied"
  | "needs_authority"
  | "aborted"
  | "cancelled"
  | "timeout"
  | "auth"
  | "billing"
  | "rate_limit"
  | "network"
  | "worker_error";

export interface TaskScope {
  read: string[];
  write: string[];
}

export interface TaskCompletionOverride {
  assertedBy: string;
  reason: string;
  assertedAt: string;
  replacedCode?: CompletionCode;
}

export interface TaskCompletion {
  exitCode?: number;
  blocked: boolean;
  code: CompletionCode;
  reason?: string;
  suggestedScope?: TaskScope;
  resetsAt?: string;
  assertedCompletion?: TaskCompletionOverride;
  dependencyBlocked?: boolean;
}

export type HoldViewKind = "time" | "profile_available" | "dependency" | "network" | "restart";

export interface TaskHoldView {
  kind: HoldViewKind;
  until?: string;
  waitingOn?: string;
  note: string;
  expiresAt: string;
}

export type TaskKind = "delegated" | "orchestrator";

export interface TaskWorktree {
  originCwd: string;
  path: string;
  branch: string;
  links?: string[];
}

/** A listed task row. Timestamps are ISO strings kept verbatim from the wire. */
export interface Task {
  id: string;
  kind?: TaskKind;
  profileId: string;
  model: string;
  prompt: string;
  shippedPrompt?: string;
  cwd: string;
  branch?: string;
  worktree?: TaskWorktree;
  worktreeLabel?: string;
  state: TaskState;
  createdAt: string;
  updatedAt: string;
  output: string;
  error?: string;
  question?: string;
  parentTaskId?: string;
  orchestratorId?: string;
  scope: TaskScope;
  grantId?: string;
  allowQuestions: boolean;
  timeoutMs?: number;
  effort?: string;
  effortActual?: string;
  tldr?: string;
  title?: string;
  sessionId?: string;
  completion?: TaskCompletion;
  attempts?: TaskAttempt[];
  costUsd?: number;
  costUsdEstimated?: boolean;
  turns?: number;
  archivedAt?: string;
  queuedFollowUps?: number;
  queuedFollowUpItems?: string[];
  hold?: TaskHoldView;
  attachments?: string[];
}

export interface TaskAttempt {
  output: string;
  error?: string;
  question?: string;
  completion?: TaskCompletion;
  endedAt: string;
  profileId?: string;
  model?: string;
  sessionId?: string;
}

/** A task list row: enough to recognise and route on, never the prompt body. */
export interface TaskSummary {
  id: string;
  profileId: string;
  model: string;
  cwd: string;
  originCwd?: string;
  branch?: string;
  worktreeLabel?: string;
  state: TaskState;
  promptPreview: string;
  tldr?: string;
  title?: string;
  createdAt: string;
  updatedAt: string;
  error?: string;
  question?: string;
  parentTaskId?: string;
  orchestratorId?: string;
  grantId?: string;
  sessionId?: string;
  completion?: TaskCompletion;
  costUsd?: number;
  costUsdEstimated?: boolean;
  archivedAt?: string;
  hold?: TaskHoldView;
}

export type EventKind =
  | "lifecycle"
  | "message"
  | "reasoning"
  | "tool"
  | "command"
  | "file"
  | "error"
  | "usage"
  | "raw"
  | "retry";

export type EventPhase = "info" | "started" | "completed" | "failed";

/** Who wrote an event row: the broker itself, or the task's provider. */
export type EventSource = "broker" | Provider;

export type PresentationType = "file" | "command" | "message" | "todo" | "tool" | "usage" | "signal";

export type EventLevel = "info" | "warning" | "error";

export interface TaskEventPresentation {
  type: PresentationType;
  path?: string;
  change?: string;
  command?: string;
  status?: string;
  exitCode?: number;
  text?: string;
  completed?: number;
  total?: number;
  costUsd?: number;
  tokensIn?: number;
  tokensOut?: number;
  tokensCached?: number;
  tokensThinking?: number;
  outcome?: string;
  turns?: number;
  durationMs?: number;
  level?: EventLevel;
}

/** What one event looks like rendered: provider-neutral, ready to display. */
export interface TaskEventView {
  id: number;
  taskId: string;
  source: EventSource;
  type: string;
  kind: EventKind;
  phase: EventPhase;
  title: string;
  detail?: string;
  verb?: string;
  target?: string;
  result?: string;
  presentation?: TaskEventPresentation;
  rawText?: string;
  createdAt: string;
  parentActionId?: string;
  turnId?: number;
  actionId?: string;
  /** Explicitly `null` when the provider gives no id; absent off actions. */
  sourceId?: string | null;
  complete?: boolean;
  minor?: boolean;
}

/** Which two sides a task diff compared. */
export type TaskDiffBasis = "branch" | "working_tree";

export type TaskDiffFileStatus = "added" | "modified" | "deleted" | "renamed" | "untracked";

export interface TaskDiffFile {
  path: string;
  oldPath?: string;
  status: TaskDiffFileStatus;
  added: number;
  removed: number;
  /** The file's unified diff, absent when it was left out for size. */
  patch?: string;
  tooLarge: boolean;
}

/** The real diff of a task's checkout, as git reports it. */
export interface TaskDiff {
  basis: TaskDiffBasis;
  base?: string;
  files: TaskDiffFile[];
  truncated: boolean;
}

export interface EventPointer {
  id: number;
  cursor: number;
  taskId: string;
  type: string;
  kind: EventKind;
  state: TaskState;
  at: string;
  title: string;
  summary: string;
  turnId?: number;
}

export interface ProfileView {
  id: string;
  label: string;
  provider: Provider;
  model: string;
  enabled: boolean;
  env: Record<string, string>;
  capabilities: string[];
  command?: string[];
}

export interface MemoryEntry {
  cwd: string;
  key: string;
  value: string;
  version: number;
  createdAt: string;
  updatedAt: string;
}

export interface MemoryProject {
  cwd: string;
  count: number;
  chars: number;
  updatedAt: string;
}

export interface SpendTotals {
  costUsd: number;
  tokens: number;
  since: string;
  windowMs: number;
  unpricedTasks: number;
}

export interface CleanupSettings {
  enabled: boolean;
  olderThanDays: number;
  archivedOnly: boolean;
}

export interface CleanupStateCount {
  state: TaskState;
  tasks: number;
}

export interface CleanupPlan {
  cutoff: string;
  tasks: number;
  byState: CleanupStateCount[];
  events: number;
  bytes: number;
  heldBack: number;
}

export interface CleanupSnapshot {
  settings: CleanupSettings;
  plan: CleanupPlan;
}

export interface CleanupResult extends CleanupSnapshot {
  finishedAt: string;
  fileBytesBefore: number;
  fileBytesAfter: number;
}

/** How long a task waits out a lost connection, and what an exhausted account does. */
export interface WaitSettings {
  networkMaxWaitMinutes: number;
  networkMaxAttempts: number;
  moveOnRateLimit: boolean;
}

export interface HealthReport {
  status: string;
  version: string;
  mcpContractVersion: number;
  build: string;
  stale: boolean;
  currentSha?: string;
  hint?: string;
}

// --- Query and request bodies ---------------------------------------------

export interface StateQuery {
  archived?: ArchivedFilter;
  compact: boolean;
  limit?: number;
  skipSummaryAggregates?: boolean;
}

export interface TaskEventsQuery {
  after?: number;
  waitMs?: number;
  last?: number;
  before?: number;
  limit?: number;
}

export type QueueAction = "add" | "clear";

export interface ResumeRequest {
  instruction?: string;
  timeoutMs?: number;
  scope?: TaskScope;
  allowQuestions?: boolean;
  queue?: QueueAction;
}

export interface ReplyRequest {
  answer: string;
  scope?: TaskScope;
}

export interface SteerRequest {
  instruction?: string;
  model?: string;
}

export interface HandoffRequest {
  profile?: string;
  model?: string;
  effort?: string;
  scope?: TaskScope;
}

export interface CompletionRequest {
  assertedBy?: string;
  reason?: string;
}

export interface PromptWrite {
  cwd: string;
  written: boolean;
  value: string;
}

/**
 * `enabled`/`preferred`/`capabilities` are double options on the Rust side
 * (`Option<Option<T>>`): omit the field to leave it untouched, or send `null`
 * to clear an override back to the inherited value.
 */
export interface ModelSettingsUpdate {
  cwd: string;
  profileId: string;
  modelId?: string;
  expectedRevision?: string;
  enabled?: boolean | null;
  preferred?: boolean | null;
  capabilities?: string[] | null;
}

export interface ProfileCreate {
  id?: string;
  label: string;
  provider: Provider;
  model?: string;
  enabled?: boolean;
  env?: Record<string, unknown>;
  capabilities?: string[];
  command?: string[];
}

export interface ProfilePatch {
  enabled?: boolean;
  label?: string;
  provider?: Provider;
  model?: string;
  capabilities?: string[];
  env?: Record<string, unknown>;
}

// --- Response bodies --------------------------------------------------

export interface BrokerSummaryState {
  profiles: ProfileView[];
  tasks: TaskSummary[];
  tasksHasMore?: boolean;
  memoryProjects: MemoryProject[];
  spend?: SpendTotals;
}

export interface TaskEventPage {
  events: TaskEventView[];
  cursor?: number;
  hasMore?: boolean;
  oldestId?: number;
  hasEarlier?: boolean;
}

export interface ProjectList {
  global: string;
  projects: string[];
}

export interface PromptConfig {
  cwd: string;
  scope: string;
  written: boolean;
  value: string;
  inherited: string;
  /** Set when the instructions come from a project file, which owns them. */
  configPath?: string;
}

export interface ModelSettingsModel {
  id: string;
  label: string;
  /** The model's published window, when the catalog knows one. Absent means unknown, never zero. */
  contextWindow?: number;
  enabled: boolean;
  inheritedEnabled: boolean;
  hasEnabledOverride: boolean;
  preferred: boolean;
  inheritedPreferred: boolean;
  hasPreferredOverride: boolean;
  capabilities: string[];
  inheritedCapabilities: string[];
  hasCapabilitiesOverride: boolean;
  availableGlobally: boolean;
}

export interface WorkerSettings {
  id: string;
  label: string;
  provider: Provider;
  enabled: boolean;
  inheritedEnabled: boolean;
  hasEnabledOverride: boolean;
  availableGlobally: boolean;
  configured: boolean;
  models: ModelSettingsModel[];
}

export type WorkKind =
  | "mechanical"
  | "context"
  | "build"
  | "reasoning"
  | "general"
  | "ui"
  | "ux"
  | "backend"
  | "database"
  | "docs"
  | "tests"
  | "review"
  | "research"
  | "refactor";

/**
 * One place work can go: a worker, a model on it, and the thinking level.
 * A missing model means the worker's default; a missing effort means the
 * kind of work prices it.
 */
export interface LoveDestination {
  profileId?: string;
  model?: string;
  effort?: string;
}

/**
 * One standing rule for work that names no model. `when` lists the kinds of
 * work the rule takes; an empty list takes every kind no other rule claims.
 * `models` is the ordered chain that is tried first to last; the flat
 * `model`/`profileId`/`effort` describe its first destination.
 */
export interface LoveRule {
  model: string;
  profileId?: string;
  when: WorkKind[];
  effort?: string;
  /** Absent on payloads from before the chain; read the flat fields then. */
  models?: LoveDestination[];
  scope: string;
}

export interface ModelSettingsSnapshot {
  cwd: string;
  scope: string;
  revision: string;
  workers: WorkerSettings[];
  love: LoveRule[];
}

export interface UsageBreakdown {
  provider: string;
  profile: string;
  model: string;
  costUsd: number;
  tokens: number;
  tasks: number;
}

export interface UsageDay {
  date: string;
  costUsd: number;
  tokens: number;
  tasks: number;
}

export interface UsagePeriod {
  id: string;
  label: string;
  costUsd: number;
  tokens: number;
  tasks: number;
  breakdown: UsageBreakdown[];
  activeDays: number;
  peakHour: number | null;
  favouriteModel: string | null;
  days: UsageDay[];
  /** Local `YYYY-MM-DD` bounds of the period; absent on older payloads. */
  start?: string;
  end?: string;
}

export interface UsageResponse {
  periods: UsagePeriod[];
  currentStreakDays: number;
  longestStreakDays: number;
}

/** Everything the web view needs to show one task, read in a single call. */
export interface TaskSnapshot {
  task: Task;
  events: TaskEventView[];
  cursor: number;
  oldestId?: number;
  hasEarlier: boolean;
}

/** What moved on a watched task, and nothing else. */
export interface TaskDelta {
  taskId: string;
  fromCursor: number;
  cursor: number;
  task?: Task;
  events: TaskEventView[];
}

/** A paced set of task changes from the shell's broker stream. */
export interface EventBatch {
  cursor: number;
  streamFloor: number;
  stale: boolean;
  pointers: EventPointer[];
}

/** What the shell's single broker connection is doing right now. */
export interface StreamStatus {
  connected: boolean;
  cursor: number;
  streamFloor: number;
  stale: boolean;
  error?: string;
}

export const EVENT_BATCH_EVENT = "oga-broker-batch";
export const STATUS_EVENT = "oga-broker-status";
export const TASK_DELTA_EVENT = "oga-task-delta";
export const MENU_EVENT = "oga-menu-command";

/** Only the menu items the shell hands to the page: opening the window,
 * quitting, and the help links are answered natively and never arrive here. */
export type MenuCommand =
  | "open-settings"
  | "check-for-updates"
  | "find-task"
  | "toggle-sidebar"
  | "toggle-inspector"
  | "refresh-tasks"
  | "clear-selection"
  | "show-activity"
  | "show-request"
  | "show-response"
  | "zoom-in"
  | "zoom-out"
  | "zoom-reset"
  | "history-back"
  | "history-forward";

/** One shell-installed MCP config target and whether the write landed. */
export interface McpInstallResult {
  client: string;
  path: string;
  success: boolean;
  message: string;
}

// --- The call union ---------------------------------------------------

/**
 * One read or write the web view asks the shell to make. Mirrors
 * `oga_client::bridge::BrokerCall`'s `#[serde(tag = "call", rename_all = "camelCase")]`
 * shape: `{call: "summary", query: {...}}`.
 */
export type BrokerCall =
  | { call: "health" }
  | { call: "summary"; query: StateQuery }
  | { call: "usage"; tzOffset?: number }
  | { call: "task"; taskId: string }
  | { call: "taskEvents"; taskId: string; query: TaskEventsQuery }
  | { call: "taskDiff"; taskId: string }
  | { call: "projects" }
  | { call: "memories"; cwd: string }
  | { call: "prompt"; cwd?: string }
  | { call: "modelSettings"; cwd?: string; refresh?: boolean }
  | { call: "cleanup" }
  | { call: "putCleanup"; settings: CleanupSettings }
  | { call: "runCleanup" }
  | { call: "waiting" }
  | { call: "putWaiting"; settings: WaitSettings }
  | { call: "archiveTask"; taskId: string; archived: boolean; deleteBranch?: boolean }
  | { call: "cancelTask"; taskId: string }
  | { call: "resumeTask"; taskId: string; request: ResumeRequest }
  | { call: "replyTask"; taskId: string; request: ReplyRequest }
  | { call: "steerTask"; taskId: string; request: SteerRequest }
  | { call: "handoffTask"; taskId: string; request: HandoffRequest }
  | { call: "completeTask"; taskId: string; request: CompletionRequest }
  | { call: "removeFollowUp"; taskId: string; index: number }
  | { call: "putPrompt"; request: PromptWrite }
  | { call: "putModelSettings"; request: ModelSettingsUpdate }
  | { call: "resetModelSettings"; cwd?: string; revision?: string }
  | { call: "createProfile"; profile: ProfileCreate }
  | { call: "updateProfile"; profileId: string; patch: ProfilePatch }
  | { call: "deleteProfile"; profileId: string };

/** A call's response shape, keyed by its `call` discriminant. */
export interface BrokerCallResult {
  health: HealthReport;
  summary: BrokerSummaryState;
  usage: UsageResponse;
  task: Task;
  taskEvents: TaskEventPage;
  taskDiff: TaskDiff;
  projects: ProjectList;
  memories: MemoryEntry[];
  prompt: PromptConfig;
  modelSettings: ModelSettingsSnapshot;
  cleanup: CleanupSnapshot;
  putCleanup: CleanupSnapshot;
  runCleanup: CleanupResult;
  waiting: WaitSettings;
  putWaiting: WaitSettings;
  archiveTask: ArchiveTaskResponse;
  cancelTask: void;
  resumeTask: void;
  replyTask: void;
  steerTask: void;
  handoffTask: void;
  completeTask: void;
  removeFollowUp: void;
  putPrompt: PromptConfig;
  putModelSettings: ModelSettingsSnapshot;
  resetModelSettings: ModelSettingsSnapshot;
  createProfile: ProfileView;
  updateProfile: ProfileView;
  deleteProfile: void;
}

/** Why a call failed, in the shape the web view can act on. */
export interface BridgeError {
  message: string;
  /** The broker's HTTP status, when the broker itself answered with one. */
  status?: number;
}

export type BridgeResult<T> = { ok: true; value: T } | { ok: false; error: BridgeError };
