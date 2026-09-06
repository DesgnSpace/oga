//! Routing, model catalog, and usage types.

use serde::{Deserialize, Serialize};

use crate::task::Provider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutePreference {
    Balanced,
    Quality,
    Cost,
    Speed,
}

impl RoutePreference {
    pub fn as_str(self) -> &'static str {
        match self {
            RoutePreference::Balanced => "balanced",
            RoutePreference::Quality => "quality",
            RoutePreference::Cost => "cost",
            RoutePreference::Speed => "speed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    Mechanical,
    Context,
    Build,
    Reasoning,
    General,
}

impl TaskClass {
    pub const ALL: [TaskClass; 5] = [
        TaskClass::Mechanical,
        TaskClass::Context,
        TaskClass::Build,
        TaskClass::Reasoning,
        TaskClass::General,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TaskClass::Mechanical => "mechanical",
            TaskClass::Context => "context",
            TaskClass::Build => "build",
            TaskClass::Reasoning => "reasoning",
            TaskClass::General => "general",
        }
    }

    /// Read a class the way a config file writes it, ignoring case and padding.
    pub fn parse(value: &str) -> Option<TaskClass> {
        let normalized = value.trim().to_lowercase();
        TaskClass::ALL
            .into_iter()
            .find(|class| class.as_str() == normalized)
    }
}

/// What the work is about, orthogonal to how hard the router judges it. A
/// task has at most one: the strongest subject signal in the prompt wins, and
/// a prompt with none has no topic rather than a guessed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTopic {
    Ui,
    Backend,
    Database,
    Docs,
    Tests,
    Review,
    Research,
    Refactor,
}

impl TaskTopic {
    pub const ALL: [TaskTopic; 8] = [
        TaskTopic::Ui,
        TaskTopic::Backend,
        TaskTopic::Database,
        TaskTopic::Docs,
        TaskTopic::Tests,
        TaskTopic::Review,
        TaskTopic::Research,
        TaskTopic::Refactor,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TaskTopic::Ui => "ui",
            TaskTopic::Backend => "backend",
            TaskTopic::Database => "database",
            TaskTopic::Docs => "docs",
            TaskTopic::Tests => "tests",
            TaskTopic::Review => "review",
            TaskTopic::Research => "research",
            TaskTopic::Refactor => "refactor",
        }
    }

    /// Read a topic the way a config file writes it, ignoring case, padding,
    /// and the few aliases people reach for first. Anything else is not a
    /// topic, so a typo fails where it is written instead of routing quietly.
    pub fn parse(value: &str) -> Option<TaskTopic> {
        let normalized = value.trim().to_lowercase();
        match normalized.as_str() {
            "frontend" => return Some(TaskTopic::Ui),
            "db" => return Some(TaskTopic::Database),
            "doc" => return Some(TaskTopic::Docs),
            "test" => return Some(TaskTopic::Tests),
            _ => {}
        }
        TaskTopic::ALL
            .into_iter()
            .find(|topic| topic.as_str() == normalized)
    }
}

/// One entry of a love rule's `when` list: either a class of work or a topic.
/// The class names are the five the router always knew; the topic names are
/// the subjects callers actually ask about. Existing files naming only
/// classes read back unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    Mechanical,
    Context,
    Build,
    Reasoning,
    General,
    Ui,
    Backend,
    Database,
    Docs,
    Tests,
    Review,
    Research,
    Refactor,
}

impl WorkKind {
    pub const ALL: [WorkKind; 13] = [
        WorkKind::Mechanical,
        WorkKind::Context,
        WorkKind::Build,
        WorkKind::Reasoning,
        WorkKind::General,
        WorkKind::Ui,
        WorkKind::Backend,
        WorkKind::Database,
        WorkKind::Docs,
        WorkKind::Tests,
        WorkKind::Review,
        WorkKind::Research,
        WorkKind::Refactor,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            WorkKind::Mechanical => "mechanical",
            WorkKind::Context => "context",
            WorkKind::Build => "build",
            WorkKind::Reasoning => "reasoning",
            WorkKind::General => "general",
            WorkKind::Ui => "ui",
            WorkKind::Backend => "backend",
            WorkKind::Database => "database",
            WorkKind::Docs => "docs",
            WorkKind::Tests => "tests",
            WorkKind::Review => "review",
            WorkKind::Research => "research",
            WorkKind::Refactor => "refactor",
        }
    }

    /// Read a kind the way a config file or `--when` writes it: class names,
    /// topic names, and the topic aliases, ignoring case and padding.
    pub fn parse(value: &str) -> Option<WorkKind> {
        if let Some(class) = TaskClass::parse(value) {
            return Some(match class {
                TaskClass::Mechanical => WorkKind::Mechanical,
                TaskClass::Context => WorkKind::Context,
                TaskClass::Build => WorkKind::Build,
                TaskClass::Reasoning => WorkKind::Reasoning,
                TaskClass::General => WorkKind::General,
            });
        }
        TaskTopic::parse(value).map(|topic| match topic {
            TaskTopic::Ui => WorkKind::Ui,
            TaskTopic::Backend => WorkKind::Backend,
            TaskTopic::Database => WorkKind::Database,
            TaskTopic::Docs => WorkKind::Docs,
            TaskTopic::Tests => WorkKind::Tests,
            TaskTopic::Review => WorkKind::Review,
            TaskTopic::Research => WorkKind::Research,
            TaskTopic::Refactor => WorkKind::Refactor,
        })
    }

    /// The class this kind names, if it names one.
    pub fn as_class(self) -> Option<TaskClass> {
        match self {
            WorkKind::Mechanical => Some(TaskClass::Mechanical),
            WorkKind::Context => Some(TaskClass::Context),
            WorkKind::Build => Some(TaskClass::Build),
            WorkKind::Reasoning => Some(TaskClass::Reasoning),
            WorkKind::General => Some(TaskClass::General),
            _ => None,
        }
    }

    /// The topic this kind names, if it names one.
    pub fn as_topic(self) -> Option<TaskTopic> {
        match self {
            WorkKind::Ui => Some(TaskTopic::Ui),
            WorkKind::Backend => Some(TaskTopic::Backend),
            WorkKind::Database => Some(TaskTopic::Database),
            WorkKind::Docs => Some(TaskTopic::Docs),
            WorkKind::Tests => Some(TaskTopic::Tests),
            WorkKind::Review => Some(TaskTopic::Review),
            WorkKind::Research => Some(TaskTopic::Research),
            WorkKind::Refactor => Some(TaskTopic::Refactor),
            _ => None,
        }
    }

    /// Whether this kind names a topic rather than a class. A topic match
    /// outranks a class match when both claim one task.
    pub fn is_topic(self) -> bool {
        self.as_topic().is_some()
    }
}

/// How hard the caller judges the work: the one routing input the caller
/// knows better than Oga does, since it wrote the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Mechanical,
    Standard,
    Hard,
    Critical,
}

impl Difficulty {
    pub fn as_str(self) -> &'static str {
        match self {
            Difficulty::Mechanical => "mechanical",
            Difficulty::Standard => "standard",
            Difficulty::Hard => "hard",
            Difficulty::Critical => "critical",
        }
    }
}

/// Where a candidate fell out of selection, most-informative first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionStage {
    /// Cleared every other filter but sits below the difficulty's tier.
    Floor,
    /// The account has effectively no quota left in its current window.
    Quota,
    /// A recorded auth, billing, or rate-limit failure on the account.
    Availability,
    /// The account's own catalog does not list the model.
    Catalog,
    /// Outside the allow list the project policy sets for this work class.
    Policy,
    /// Turned off for this project, or everywhere, in Settings.
    Settings,
    /// No tool calling, or not a text model.
    Capability,
    Profile,
}

/// A constraint selection dropped to reach any destination at all. Settings,
/// tool-calling capability, and recorded unavailability are never dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionRelaxation {
    Floor,
    Policy,
    Quota,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRejection {
    pub profile_id: String,
    pub model: String,
    pub stage: SelectionStage,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChosenRoute {
    pub profile_id: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerUp {
    pub profile_id: String,
    pub model: String,
}

/// Everything a routing decision records except where it landed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingRecord {
    pub decided_by: DecidedBy,
    pub router_version: u32,
    pub difficulty: Difficulty,
    pub difficulty_source: DifficultySource,
    /// What the prompt heuristic made of the work, as a check on the caller's
    /// declaration.
    pub heuristic_class: TaskClass,
    /// False when the heuristic wanted a stronger tier than declared allows.
    pub heuristic_agreed: bool,
    pub floor: u8,
    /// Constraints dropped to reach a destination; empty when every one held.
    pub relaxed: Vec<SelectionRelaxation>,
    pub preference: RoutePreference,
    pub effort_source: EffortSource,
    pub effort_reason: String,
    /// Worst usage window on the chosen account; null when its provider
    /// reports no usage at all — unknown headroom, not full and not empty.
    pub quota_used_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runners_up: Option<Vec<RunnerUp>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected: Option<Vec<SelectionRejection>>,
    /// Total rejections; `rejected` carries only the most informative few.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
}

/// Why a task landed on the profile, model, and effort it did. `decidedBy`
/// separates a routing call from a lucky caller guess. The chosen pair is
/// filled in from the task row itself, so the record can never claim a
/// destination the task did not actually run on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSelection {
    #[serde(flatten)]
    pub record: RoutingRecord,
    pub chosen: ChosenRoute,
}

/// A selection with the chosen pair still to be filled in from the task row;
/// serializes without a `chosen` key until then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionDecision {
    #[serde(flatten)]
    pub record: RoutingRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen: Option<ChosenRoute>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecidedBy {
    Router,
    CallerProfile,
    CallerExplicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DifficultySource {
    Caller,
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortSource {
    Caller,
    Loved,
    Projected,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelInfoSource {
    Discovered,
    Alias,
    Configured,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub provider: Provider,
    pub profile_id: String,
    pub source: ModelInfoSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<ModelCost>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    /// Effort levels this model accepts, weakest first, as published by the
    /// provider. Absent means no published ladder, not that none exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<bool>,
}

/// One row of `/api/models`: the catalog entry joined with per-project settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSettingsRow {
    pub profile: String,
    pub model: String,
    pub capabilities: Vec<String>,
    pub enabled: bool,
    pub preferred: bool,
    /// A love rule sends work here when the caller names no model.
    pub loved: bool,
    /// Effort levels this model accepts, weakest first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// The profile's usage against the window that governs this model, when a
    /// caller asked for it. `None` means the caller opted out, not that usage
    /// is unknown — an included-but-unknown read still reports `known: false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelUsageSummary>,
}

/// A compact usage read for one model row: enough to weigh budget against
/// availability without dragging the full per-window detail `ProfileUsage`
/// carries into every row of a listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageSummary {
    /// `false` means the provider has no usage source, or none has been read
    /// yet — never omitted silently, always explained by `reason`.
    pub known: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent_used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    pub rate_limited: bool,
    pub out_of_credits: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh: Option<bool>,
    /// Project scoping the profile set; omitted reads user-level profiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_preferred: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_enabled: Option<bool>,
    /// Case-insensitive substring matched against model ids.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// One metered window on a provider account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub label: String,
    pub kind: UsageWindowKind,
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_text: Option<String>,
    /// The model family this window meters, when scoped to one. An account
    /// window says nothing about any single model's own limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageWindowKind {
    Session,
    Week,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountFailure {
    pub message: String,
    pub failed_at: String,
    pub consecutive_failures: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitByModel {
    pub model: String,
    pub message: String,
    pub failed_at: String,
    pub consecutive_failures: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedRateLimit {
    /// The upstream account the limit belongs to (`openai/…`, `opencode/…`).
    pub upstream: String,
    pub model: String,
    pub failed_at: String,
    pub hits: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUsage {
    pub profile: String,
    pub provider: Provider,
    pub supported: bool,
    pub source: UsageSource,
    pub windows: Vec<UsageWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Account-wide failure on file — auth, billing, or network.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_failure: Option<AccountFailure>,
    /// Every rate limit on file for this profile, one per model: providers
    /// that meter models separately can hold several live at once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limits_by_model: Option<Vec<RateLimitByModel>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_rate_limits: Option<Vec<ObservedRateLimit>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageSource {
    ClaudeCli,
    CodexSessionLog,
    None,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}
