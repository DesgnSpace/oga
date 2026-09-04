//! Archive, restore, and explicit worktree removal actions.

use std::path::Path;

use oga_domain::{
    Task, TaskState, WorktreeDeleteBatchResult, WorktreeDeleteEntry, WorktreeDeleteResult,
    WorktreeDeleteSkipped,
};
use serde_json::json;

use crate::{
    ContinuationError, append_event_tx,
    cancel::{self, CancelRequest},
    dispatch::Dispatcher,
    lifecycle::{self, now_iso},
    require_task,
};

const ARCHIVE_STOPS_REASON: &str = "archived while running — stopped first";

#[derive(Debug, Clone)]
pub struct ArchiveRequest {
    pub task_id: String,
    pub archived: bool,
}

impl ArchiveRequest {
    pub fn new(task_id: impl Into<String>, archived: bool) -> Self {
        Self {
            task_id: task_id.into(),
            archived,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveResult {
    pub task: Task,
    pub stopped: bool,
    pub checkout: Option<String>,
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
    let original = require_task(dispatcher.store(), &request.task_id)?;
    let mut stopped = false;
    if request.archived && archive_stops_state(original.state) {
        cancel::cancel(
            dispatcher,
            CancelRequest::new(&original.id).reason(ARCHIVE_STOPS_REASON),
        )
        .await?;
        stopped = true;
    }
    let task = require_task(dispatcher.store(), &request.task_id)?;
    let checkout = if request.archived {
        checkout_on_archive(dispatcher, &task).await
    } else {
        None
    };
    let task = set_archived(dispatcher, &task, request.archived)?;
    Ok(ArchiveResult {
        task,
        stopped,
        checkout,
    })
}

fn archive_stops_state(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Queued
            | TaskState::Pending
            | TaskState::Running
            | TaskState::NeedsInput
            | TaskState::Answered
            | TaskState::Blocked
    )
}

async fn checkout_on_archive(dispatcher: &Dispatcher, task: &Task) -> Option<String> {
    let Some(worktree) = &task.worktree else {
        return None;
    };
    if !Path::new(&worktree.path).exists() {
        return Some("nothing to remove".into());
    }
    let active = match checkout_tasks(dispatcher, &worktree.path, Some(&task.id)) {
        Ok(active) => active,
        Err(error) => return Some(format!("could not inspect the checkout: {error}")),
    };
    if !active.is_empty() {
        let ids = active
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Some(format!(
            "kept because {ids} {} still using it",
            if active.len() == 1 { "is" } else { "are" }
        ));
    }
    match oga_worktree::worktree_has_uncommitted_work(worktree).await {
        Ok(true) => Some(format!(
            "kept because it has uncommitted work: {}",
            worktree.path
        )),
        Ok(false) => match oga_worktree::remove_task_worktree(worktree).await {
            Ok(()) => Some("removed".into()),
            Err(error) => Some(format!("could not remove the checkout: {error}")),
        },
        Err(error) => Some(format!("could not inspect the checkout: {error}")),
    }
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
        oga_domain::CheckoutOutcome::AlreadyGone
    };
    let branch = if request.delete_branch {
        oga_worktree::remove_task_branch(&worktree).await?
    } else {
        oga_domain::BranchOutcome::Kept
    };
    Ok(WorktreeDeleteEntry::Applied(WorktreeDeleteResult {
        task_id: task.id,
        checkout,
        branch,
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
        let mut statement = connection.prepare("SELECT id FROM tasks WHERE worktree_path=?")?;
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
    Ok(tasks
        .into_iter()
        .filter(|task| oga_worktree::worktree_active(task.state))
        .collect())
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
