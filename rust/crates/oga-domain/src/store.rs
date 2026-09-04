//! Store-level records: profile failures, spend, cleanup.

use serde::{Deserialize, Serialize};

use crate::task::TaskState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    Auth,
    Billing,
    RateLimit,
    Network,
}

/// A recorded account failure. A rate limit belongs to the model the run was
/// using, not to the whole account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileFailure {
    pub profile_id: String,
    pub code: FailureCode,
    pub message: String,
    pub failed_at: String,
    pub consecutive_failures: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSuccess {
    pub profile_id: String,
    pub succeeded_at: String,
}

/// What tasks have cost and used, summed over a trailing window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendTotals {
    pub cost_usd: f64,
    pub tokens: u64,
    /// Start of the window this was summed over.
    pub since: String,
    /// How long the window is, so a reader knows what period the sum covers.
    pub window_ms: u64,
    /// Settled tasks whose provider reported no price, so the total is a floor.
    pub unpriced_tasks: u64,
}

/// What a cleanup would remove, or did remove. Counts a person reads back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPlan {
    /// A task must have stopped changing before this to qualify.
    pub cutoff: String,
    pub tasks: u64,
    /// Eligible task counts per final state, largest group first.
    pub by_state: Vec<CleanupStateCount>,
    /// Rows of recorded activity — every step of every run that qualifies.
    pub events: u64,
    /// Roughly what those rows hold, before index and row overhead.
    pub bytes: u64,
    /// Finished, archived and old enough, kept because a task under them is not.
    pub held_back: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupStateCount {
    pub state: TaskState,
    pub tasks: u64,
}

/// A cleanup that already ran, kept so an unattended pass stays answerable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupRecord {
    #[serde(flatten)]
    pub plan: CleanupPlan,
    pub finished_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    #[serde(flatten)]
    pub record: CleanupRecord,
    pub file_bytes_before: u64,
    pub file_bytes_after: u64,
}

/// The conservative automatic-cleanup policy stored with workspace settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupSettings {
    pub enabled: bool,
    pub older_than_days: u64,
    pub archived_only: bool,
}

impl Default for CleanupSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            older_than_days: 30,
            archived_only: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupSnapshot {
    pub settings: CleanupSettings,
    pub plan: CleanupPlan,
}

/// How long a task waits out a lost connection or an exhausted account, and
/// whether an exhausted account may move the task to another worker instead of
/// waiting for the reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitSettings {
    pub network_max_wait_minutes: u64,
    pub network_max_attempts: u32,
    pub move_on_rate_limit: bool,
}

impl Default for WaitSettings {
    fn default() -> Self {
        Self {
            network_max_wait_minutes: 60,
            network_max_attempts: 12,
            move_on_rate_limit: false,
        }
    }
}
