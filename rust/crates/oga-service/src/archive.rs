//! Archive, restore, and explicit worktree removal actions.

use std::{path::Path, time::Duration};

use oga_domain::{
    BranchOutcome, Task, TaskState, WorktreeDeleteBatchResult, WorktreeDeleteEntry,
    WorktreeDeleteResult, WorktreeDeleteSkipped,
};
use serde_json::json;
use tokio::time::{sleep, timeout};

use crate::{
    ContinuationError, append_event_tx,
    cancel::{self, CancelRequest},
    dispatch::Dispatcher,
    lifecycle::{self, now_iso},
    require_task,
};

const ARCHIVE_STOPS_REASON: &str = "archived while running — stopped first";
const ARCHIVE_CANCEL_TIMEOUT: Duration = Duration::from_secs(10);
const ARCHIVE_CANCEL_POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone)]
pub struct ArchiveRequest {
    pub task_id: String,
    pub archived: bool,
    pub delete_branch: bool,
}

impl ArchiveRequest {
    pub fn new(task_id: impl Into<String>, archived: bool) -> Self {
        Self {
            task_id: task_id.into(),
            archived,
            delete_branch: false,
        }
    }

    pub fn delete_branch(mut self) -> Self {
        self.delete_branch = true;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveResult {
    pub task: Task,
    pub stopped: bool,
    pub checkout: Option<String>,
    pub branch: Option<BranchOutcome>,
    pub branch_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorktreeRemoveRequest {
    pub task_id: String,
    pub delete_branch: bool,
}

impl WorktreeRemoveRequest {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            delete_branch: false,
        }
    }

    pub fn delete_branch(mut self) -> Self {
        self.delete_branch = true;
        self
    }
}

pub async fn archive(
    dispatcher: &Dispatcher,
    request: ArchiveRequest,
) -> Result<ArchiveResult, ContinuationError> {
    if request.delete_branch && !request.archived {
        return Err(ContinuationError::Refusal(
            "deleteBranch only applies when archiving".into(),
        ));
    }
    let original = require_task(dispatcher.store(), &request.task_id)?;
    if request.delete_branch && original.worktree.is_none() {
        return Err(ContinuationError::Refusal(
            "deleteBranch only applies to worktree tasks".into(),
        ));
    }
    let mut stopped = false;
    if request.archived && archive_stops_state(original.state) {
        cancel::cancel(
            dispatcher,
            CancelRequest::new(&original.id).reason(ARCHIVE_STOPS_REASON),
        )
        .await?;
        wait_for_cancellation(dispatcher, &original.id).await?;
        stopped = true;
    }
    let task = require_task(dispatcher.store(), &request.task_id)?;
    let mut task = set_archived(dispatcher, &task, request.archived)?;
    let mut checkout = None;
    let mut branch = None;
    let mut branch_reason = None;
    if request.archived && task.worktree.is_some() {
        if task.state == TaskState::RemovingCheckout {
            checkout = Some("removal in progress".into());
            if request.delete_branch {
                branch = Some(BranchOutcome::Kept);
                branch_reason = Some(
                    "checkout removal is still in progress; the branch is kept until it finishes"
                        .into(),
                );
            }
        } else if let Some(reason) = checkout_in_use_reason(dispatcher, &task)? {
            checkout = Some(reason);
            if request.delete_branch {
                branch = Some(BranchOutcome::Kept);
                branch_reason = Some("checkout was kept, so the branch was kept".into());
            }
        } else {
            task = begin_checkout_removal(dispatcher, &task)?;
            let cleanup = task.clone();
            let cleanup_dispatcher = dispatcher.clone();
            tokio::spawn(async move {
                finish_checkout_removal(cleanup_dispatcher, cleanup, request.delete_branch).await;
            });
            checkout = Some("removal in progress".into());
            if request.delete_branch {
                branch = Some(BranchOutcome::Kept);
                branch_reason = Some(
                    "checkout removal is in progress; the branch is kept until it finishes".into(),
                );
            }
        }
    }
    Ok(ArchiveResult {
        task,
        stopped,
        checkout,
        branch,
        branch_reason,
    })
}

async fn wait_for_cancellation(
    dispatcher: &Dispatcher,
    task_id: &str,
) -> Result<(), ContinuationError> {
    timeout(ARCHIVE_CANCEL_TIMEOUT, async {
        loop {
            let task = require_task(dispatcher.store(), task_id)?;
            if task.state == TaskState::Cancelled
                && dispatcher.active_runs().get(task_id).is_none()
                && !dispatcher.active_runs().is_starting(task_id)
            {
                return Ok(());
            }
            sleep(ARCHIVE_CANCEL_POLL).await;
        }
    })
    .await
    .map_err(|_| {
        ContinuationError::Refusal(format!(
            "could not confirm the worker stopped before archiving: {task_id} — retry after it settles"
        ))
    })?
}

fn archive_stops_state(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Queued
            | TaskState::PreparingCheckout
            | TaskState::Pending
            | TaskState::Running
            | TaskState::NeedsInput
            | TaskState::Answered
            | TaskState::Blocked
    )
}

fn checkout_in_use_reason(
    dispatcher: &Dispatcher,
    task: &Task,
) -> Result<Option<String>, ContinuationError> {
    let Some(worktree) = &task.worktree else {
        return Ok(None);
    };
    let active = checkout_tasks(dispatcher, &worktree.path, Some(&task.id))?;
    if !active.is_empty() {
        let ids = active
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(Some(format!(
            "kept because {ids} {} still using it",
            if active.len() == 1 { "is" } else { "are" }
        )));
    }
    Ok(None)
}

fn begin_checkout_removal(dispatcher: &Dispatcher, task: &Task) -> Result<Task, ContinuationError> {
    let now = now_iso();
    dispatcher.store().transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='removing_checkout',checkout_state=?,updated_at=? WHERE id=? AND archived_at IS NOT NULL AND state<> 'removing_checkout'",
            rusqlite::params![task.state.as_str(), now, task.id],
        )?;
        if changed != 1 {
            return Err(oga_store::StoreError::Refusal(format!(
                "task cannot start checkout removal: {}",
                task.id
            )));
        }
        append_event_tx(
            tx,
            &task.id,
            "checkout_removal_started",
            TaskState::RemovingCheckout,
            json!({}),
            &now,
        )?;
        Ok(())
    })?;
    require_task(dispatcher.store(), &task.id)
}

async fn finish_checkout_removal(dispatcher: Dispatcher, task: Task, delete_branch: bool) {
    let Some(worktree) = &task.worktree else {
        return;
    };
    let outcome = if Path::new(&worktree.path).exists() {
        match oga_worktree::worktree_has_uncommitted_work(worktree).await {
            Ok(true) => Err(format!(
                "kept because it has uncommitted work: {}",
                worktree.path
            )),
            Ok(false) => oga_worktree::remove_task_worktree(worktree)
                .await
                .map_err(|error| format!("could not remove the checkout: {error}")),
            Err(error) => Err(format!("could not inspect the checkout: {error}")),
        }
    } else {
        oga_worktree::remove_task_worktree(worktree)
            .await
            .map_err(|error| format!("could not prune the checkout: {error}"))
    };
    let mut error = outcome.err();
    let branch = if error.is_none() && delete_branch {
        match oga_worktree::remove_task_branch_safely(worktree).await {
            Ok(result) => Some(result),
            Err(branch_error) => {
                error = Some(format!(
                    "branch safety check failed, so the branch was kept: {branch_error}"
                ));
                None
            }
        }
    } else {
        None
    };
    let now = now_iso();
    let _ = dispatcher.store().transaction(|tx| {
        let previous: Option<String> = tx.query_row(
            "SELECT checkout_state FROM tasks WHERE id=?",
            [task.id.as_str()],
            |row| row.get(0),
        )?;
        let state = previous
            .as_deref()
            .and_then(|state| serde_json::from_str(&format!("\"{state}\"")).ok())
            .unwrap_or(TaskState::Cancelled);
        let changed = tx.execute(
            "UPDATE tasks SET state=?,checkout_state=NULL,error=?,updated_at=? WHERE id=? AND state='removing_checkout'",
            rusqlite::params![state.as_str(), error, now, task.id],
        )?;
        if changed == 1 {
            append_event_tx(
                tx,
                &task.id,
                if error.is_some() { "checkout_removal_failed" } else { "checkout_removed" },
                state,
                json!({
                    "error": error,
                    "branchOutcome": branch.as_ref().map(|result| result.outcome),
                    "branchReason": branch.as_ref().and_then(|result| result.reason.as_deref()),
                }),
                &now,
            )?;
        }
        Ok(())
    });
}

fn set_archived(
    dispatcher: &Dispatcher,
    task: &Task,
    archived: bool,
) -> Result<Task, ContinuationError> {
    if task.archived_at.is_some() == archived {
        return Ok(task.clone());
    }
    let now = now_iso();
    dispatcher.store().transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET archived_at=?,updated_at=? WHERE id=?",
            rusqlite::params![archived.then_some(now.as_str()), now, task.id],
        )?;
        if changed != 1 {
            return Err(oga_store::StoreError::Refusal(format!(
                "unknown task: {}",
                task.id
            )));
        }
        append_event_tx(
            tx,
            &task.id,
            if archived { "archived" } else { "unarchived" },
            task.state,
            json!({}),
            &now,
        )?;
        Ok(())
    })?;
    require_task(dispatcher.store(), &task.id)
}

pub async fn remove_worktree(
    dispatcher: &Dispatcher,
    request: WorktreeRemoveRequest,
) -> Result<WorktreeDeleteEntry, ContinuationError> {
    let task = require_task(dispatcher.store(), &request.task_id)?;
    let Some(worktree) = task.worktree.clone() else {
        return Err(ContinuationError::Refusal(format!(
            "task did not run in a worktree: {} — only tasks delegated with a worktree have a checkout to remove",
            task.id
        )));
    };
    if oga_worktree::worktree_active(task.state) {
        return Ok(WorktreeDeleteEntry::Skipped(WorktreeDeleteSkipped {
            task_id: task.id,
            skipped: true,
            state: task.state,
            reason: "task is not settled".into(),
        }));
    }
    let active = checkout_tasks(dispatcher, &worktree.path, Some(&task.id))?;
    if !active.is_empty() {
        let ids = active
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(WorktreeDeleteEntry::Skipped(WorktreeDeleteSkipped {
            task_id: task.id,
            skipped: true,
            state: task.state,
            reason: format!("still in use by {ids}"),
        }));
    }
    let checkout = if Path::new(&worktree.path).exists() {
        oga_worktree::remove_task_worktree(&worktree).await?;
        oga_domain::CheckoutOutcome::Removed
    } else {
        oga_worktree::remove_task_worktree(&worktree).await?;
        oga_domain::CheckoutOutcome::AlreadyGone
    };
    let (branch, branch_reason) = if request.delete_branch {
        let result = oga_worktree::remove_task_branch_safely(&worktree).await?;
        (result.outcome, result.reason)
    } else {
        (oga_domain::BranchOutcome::Kept, None)
    };
    Ok(WorktreeDeleteEntry::Applied(WorktreeDeleteResult {
        task_id: task.id,
        checkout,
        branch,
        branch_reason,
    }))
}

pub async fn remove_project_worktrees(
    dispatcher: &Dispatcher,
    project: impl Into<String>,
    delete_branch: bool,
) -> Result<WorktreeDeleteBatchResult, ContinuationError> {
    let project = project.into();
    let ids = dispatcher
        .store()
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id FROM tasks WHERE origin_cwd=? AND worktree_path IS NOT NULL ORDER BY created_at",
            )?;
            statement
                .query_map([project.as_str()], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(oga_store::StoreError::from)
        })?;
    let mut results = Vec::with_capacity(ids.len());
    for task_id in ids {
        results.push(
            remove_worktree(
                dispatcher,
                WorktreeRemoveRequest {
                    task_id,
                    delete_branch,
                },
            )
            .await?,
        );
    }
    Ok(WorktreeDeleteBatchResult { project, results })
}

fn checkout_tasks(
    dispatcher: &Dispatcher,
    path: &str,
    exclude: Option<&str>,
) -> Result<Vec<Task>, ContinuationError> {
    let ids = dispatcher.store().with_connection(|connection| {
        let mut statement = connection
            .prepare("SELECT id FROM tasks WHERE worktree_path=? AND archived_at IS NULL")?;
        statement
            .query_map([path], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(oga_store::StoreError::from)
    })?;
    let tasks = ids
        .into_iter()
        .filter(|id| exclude != Some(id.as_str()))
        .map(|id| {
            lifecycle::load_task(dispatcher.store(), &id)?.ok_or_else(|| {
                ContinuationError::Refusal(format!("task disappeared from checkout: {id}"))
            })
        })
        .collect::<Result<Vec<_>, ContinuationError>>()?;
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::archive_stops_state;
    use oga_domain::TaskState;

    #[test]
    fn archive_stops_every_state_that_can_hold_a_worker() {
        for state in [
            TaskState::Queued,
            TaskState::Pending,
            TaskState::Running,
            TaskState::NeedsInput,
            TaskState::Answered,
            TaskState::Blocked,
        ] {
            assert!(archive_stops_state(state));
        }
        for state in [
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Cancelled,
        ] {
            assert!(!archive_stops_state(state));
        }
    }
}
