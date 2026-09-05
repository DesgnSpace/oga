//! Task dispatch, lifecycle orchestration, and task actions.

use oga_domain::{Profile, Task, TaskAttempt, TaskState};
use oga_store::{Store, StoreError};
use rusqlite::Transaction;
use serde::Serialize;
use thiserror::Error;

pub mod archive;
pub mod authorization;
pub mod cancel;
pub mod complete;
pub mod deliveries;
pub mod dependencies;
pub mod dispatch;
pub mod follow_ups;
pub mod handoff;
pub mod handoff_brief;
pub mod holds;
pub mod lifecycle;
pub mod prompt;
pub mod reconcile;
pub mod reply;
pub mod resume;
pub mod schedule;
pub mod steer;
pub mod waiting;

const MAX_ATTEMPTS: usize = 10;

/// Errors returned when a task action cannot be applied to the current row.
#[derive(Debug, Error)]
pub enum ContinuationError {
    #[error("task action refused: {0}")]
    Refusal(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Worktree(#[from] oga_worktree::WorktreeError),
    #[error(transparent)]
    Provider(#[from] oga_providers::ProviderError),
}

impl From<holds::HoldError> for ContinuationError {
    fn from(error: holds::HoldError) -> Self {
        match error {
            holds::HoldError::Store(error) => Self::Store(error),
        }
    }
}

impl ContinuationError {
    pub fn message(&self) -> String {
        match self {
            Self::Refusal(reason) => format!("Error: {reason}"),
            _ => format!("Error: {self}"),
        }
    }
}

pub(crate) fn require_task(store: &Store, task_id: &str) -> Result<Task, ContinuationError> {
    lifecycle::load_task(store, task_id)?
        .ok_or_else(|| ContinuationError::Refusal(format!("unknown task: {task_id}")))
}

pub(crate) fn require_profile(
    store: &Store,
    profile_id: &str,
) -> Result<Profile, ContinuationError> {
    store
        .repositories()
        .profiles()
        .get(profile_id)?
        .ok_or_else(|| ContinuationError::Refusal(format!("unknown profile: {profile_id}")))
}

pub(crate) fn require_existing_worktree(task: &Task) -> Result<(), ContinuationError> {
    if let Some(worktree) = &task.worktree {
        oga_worktree::require_worktree_paths(worktree)?;
    }
    Ok(())
}

pub(crate) fn validate_model(model: &str) -> Result<String, ContinuationError> {
    let model = model.trim();
    if model.is_empty() {
        return Err(ContinuationError::Refusal("model must not be empty".into()));
    }
    if model.len() > 200 {
        return Err(ContinuationError::Refusal(
            "model exceeds 200 characters".into(),
        ));
    }
    Ok(model.to_owned())
}

pub(crate) fn encode_store<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value)
        .map_err(|error| StoreError::Refusal(format!("invalid JSON: {error}")))
}

pub(crate) fn close_attempt(task: &Task, ended_at: &str, always_close: bool) -> Vec<TaskAttempt> {
    if !always_close && task.output.is_empty() && task.error.is_none() && task.question.is_none() {
        return task.attempts.clone();
    }
    let mut attempts = task.attempts.clone();
    while attempts.len() >= MAX_ATTEMPTS {
        attempts.remove(0);
    }
    attempts.push(TaskAttempt {
        output: task.output.clone(),
        error: task.error.clone(),
        question: task.question.clone(),
        completion: task.completion.clone(),
        ended_at: ended_at.to_owned(),
        profile_id: Some(task.profile_id.clone()),
        model: Some(task.model.clone()),
        session_id: task.session_id.clone(),
    });
    attempts
}

pub(crate) fn append_event_tx(
    tx: &Transaction<'_>,
    task_id: &str,
    kind: &str,
    state: oga_domain::TaskState,
    payload: serde_json::Value,
    at: &str,
) -> Result<i64, rusqlite::Error> {
    tx.execute(
        "INSERT INTO task_events(task_id,event_type,state,payload,created_at) VALUES(?,?,?,?,?)",
        rusqlite::params![task_id, kind, state.as_str(), payload.to_string(), at],
    )?;
    Ok(tx.last_insert_rowid())
}

pub(crate) fn continuation_prompt(task: &str, instruction: &str) -> String {
    format!(
        "# Continuation instruction\n{instruction}\n\nContinue the original task below without repeating completed work.\n\n{task}"
    )
}

pub(crate) fn resume_prompt(
    previous_state: TaskState,
    instruction: &str,
    has_session: bool,
) -> String {
    let context = if previous_state == TaskState::Completed {
        "Your earlier run on this task finished. This is a follow-up in that same session: keep what you already read and decided, and do the work above without re-deriving the project from scratch.".to_owned()
    } else if previous_state == TaskState::Pending {
        format!(
            "The task never ran; it was waiting and now starts{}.",
            if has_session {
                " in its existing provider session"
            } else {
                ""
            }
        )
    } else {
        format!(
            "The previous Oga run ended in state `{}`. Continue the existing provider session without repeating completed work.",
            previous_state.as_str()
        )
    };
    let heading = if previous_state == TaskState::Completed {
        "# Follow-up instruction"
    } else {
        "# Resume instruction"
    };
    format!("{heading}\n{instruction}\n\n{context}")
}

pub use archive::{
    ArchiveRequest, ArchiveResult, WorktreeRemoveRequest, archive, remove_project_worktrees,
    remove_worktree,
};
pub use cancel::{CancelRequest, cancel};
pub use complete::{CompletionAssertion, assert_completion, force_complete};
pub use deliveries::{
    DeliveryInbox, DeliveryService, Rebaselined, advance_cursor, baseline_consumer, get_cursor,
    mark_deliveries_pending, read_inbox, register_consumer, remember_consumer_scope,
    retire_sent_deliveries,
};
pub use dependencies::{
    DependencyBlocker, add_dependencies, dependencies_of, dependency_closure, dependency_hold,
    dependents_of, restore_dependency_hold, unsettled_blockers,
};
pub use dispatch::{
    DelegateRequest, DispatchError, DispatchPlan, DispatchRequest, DispatchResult, Dispatcher,
    TaskService,
};
pub use follow_ups::{
    FollowUpFeed, FollowUpQueue, FollowUpService, QueuedFollowUp, clear_follow_ups,
    count_follow_ups, feed_follow_up, list_follow_ups, queue_follow_up, remove_follow_up_at,
    take_next_follow_up, take_next_instruction,
};
pub use handoff::{HandoffRequest, handoff};
pub use handoff_brief::{
    DIGEST_CAP, FreshSessionCause, HandoffBrief, HandoffBriefOptions, VERBATIM_CAP, handoff_brief,
};
pub use holds::{
    AlwaysAvailable, Availability, Clock, FixedClock, HOLD_EXPIRY, HoldError, HoldProbe,
    HoldService, HoldSweep, HoldSweepReport, MAX_RECHECK, MIN_RECHECK, arm_hold, drop_hold,
    due_holds, get_hold, touch_hold,
};
pub use lifecycle::{
    ActiveRuns, LifecycleError, MapFoldHook, NoopMapFoldHook, RunOutcome, run_task,
    run_task_and_release, run_task_with_hook,
};
pub use prompt::{
    WorkerOutcome, WorkerPromptInput, assemble_worker_prompt, classify_failure,
    interpret_worker_outcome, scope_line,
};
pub use reply::{ReplyRequest, reply};
pub use resume::{ResumeRequest, resume};
pub use schedule::{StartAt, parse_start_at};
pub use steer::{SteerRequest, steer};
