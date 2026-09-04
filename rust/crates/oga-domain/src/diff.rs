//! The real diff of a task's checkout, read from git rather than derived from
//! what a worker reported it did.

use serde::{Deserialize, Serialize};

/// Which two sides a task diff compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskDiffBasis {
    /// A worktree task: its branch plus anything uncommitted in the checkout,
    /// against the commit the branch was cut from.
    Branch,
    /// A task running in the caller's own checkout: the working tree against
    /// `HEAD`, narrowed to the directory and paths the task could write.
    WorkingTree,
}

/// What happened to one file between the two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskDiffFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    /// Present in the checkout and not in git's index.
    Untracked,
}

/// One file's entry in a task diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDiffFile {
    /// Relative to the task's own directory, matching the reported view.
    pub path: String,
    /// Where a renamed file came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: TaskDiffFileStatus,
    pub added: u32,
    pub removed: u32,
    /// The file's unified diff, absent when it was left out for size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// Set when `patch` was left out because the file's diff was too big.
    pub too_large: bool,
}

/// Everything one read of a task's checkout found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDiff {
    pub basis: TaskDiffBasis,
    /// The commit the diff was taken against, when git could name one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub files: Vec<TaskDiffFile>,
    /// Set when the checkout held more than one read carries.
    pub truncated: bool,
}
