//! Archive, restore, and explicit worktree removal actions.

use std::{collections::HashSet, fs, path::Path, time::Duration};

use oga_domain::{
    BranchOutcome, Task, TaskState, TaskWorktree, WorktreeDeleteBatchResult, WorktreeDeleteEntry,
    WorktreeDeleteResult, WorktreeDeleteSkipped,
};
use oga_store::append_event;
use serde_json::json;
use tokio::time::{sleep, timeout};

use crate::{
    ContinuationError,
    cancel::{self, CancelRequest},
    dispatch::Dispatcher,
    lifecycle::{self, now_iso},
    require_task,
};

const ARCHIVE_STOPS_REASON: &str = "archived while running — stopped first";
const ARCHIVE_CANCEL_TIMEOUT: Duration = Duration::from_secs(10);
const ARCHIVE_CANCEL_POLL: Duration = Duration::from_millis(10);
const CHECKOUT_KEPT_BRANCH_REASON: &str = "checkout was kept, so the branch was kept";

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
        if let Some(reason) = checkout_in_use_reason(dispatcher, &task)? {
            record_checkout_kept(dispatcher, &task, &reason)?;
            checkout = Some(reason);
            if request.delete_branch {
                branch = Some(BranchOutcome::Kept);
                branch_reason = Some(CHECKOUT_KEPT_BRANCH_REASON.into());
            }
        } else {
            if task.state != TaskState::RemovingCheckout {
                task = begin_checkout_removal(dispatcher, &task)?;
            }
            let removal = finish_checkout_removal(dispatcher, &task, request.delete_branch).await;
            task = require_task(dispatcher.store(), &task.id)?;
            checkout = Some(
                removal
                    .keep_reason
                    .clone()
                    .unwrap_or_else(|| "removed".into()),
            );
            if let Some(result) = removal.branch {
                branch = Some(result.outcome);
                branch_reason = result.reason;
            } else if let Some(error) = removal.branch_error {
                branch = Some(BranchOutcome::Kept);
                branch_reason = Some(error);
            } else if request.delete_branch {
                branch = Some(BranchOutcome::Kept);
                branch_reason = Some(CHECKOUT_KEPT_BRANCH_REASON.into());
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

/// The outcome of one checkout removal attempt. `keep_reason` is set when the
/// checkout stays behind.
#[derive(Debug, Clone, Default)]
pub struct CheckoutRemoval {
    pub keep_reason: Option<String>,
    pub branch: Option<oga_worktree::BranchRemoval>,
    pub branch_error: Option<String>,
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

/// The siblings on a checkout that a live worker could still write to. A
/// finished task does not hold its checkout, archived or not.
fn live_checkout_tasks(
    dispatcher: &Dispatcher,
    path: &str,
    exclude: &str,
) -> Result<Vec<String>, ContinuationError> {
    let rows = dispatcher.store().with_connection(|connection| {
        let mut statement =
            connection.prepare("SELECT id,state FROM tasks WHERE worktree_path=?")?;
        Ok(statement
            .query_map([path], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?)
    })?;
    Ok(rows
        .into_iter()
        .filter(|(id, _)| id != exclude)
        .filter(|(_, state)| state_holds_checkout(state))
        .map(|(id, _)| id)
        .collect())
}

/// Whether a state string holds a checkout. A state Oga does not know counts
/// as holding it, so an upgrade never removes live work.
fn state_holds_checkout(state: &str) -> bool {
    parse_task_state(state).is_none_or(oga_worktree::checkout_in_use)
}

fn parse_task_state(state: &str) -> Option<TaskState> {
    serde_json::from_str(&format!("\"{state}\"")).ok()
}

fn checkout_in_use_reason(
    dispatcher: &Dispatcher,
    task: &Task,
) -> Result<Option<String>, ContinuationError> {
    let Some(worktree) = &task.worktree else {
        return Ok(None);
    };
    let active = live_checkout_tasks(dispatcher, &worktree.path, &task.id)?;
    if active.is_empty() {
        return Ok(None);
    }
    let ids = active.join(", ");
    Ok(Some(format!(
        "kept: shared with {} still using this checkout ({ids})",
        if active.len() == 1 {
            "a live task"
        } else {
            "live tasks"
        }
    )))
}

/// Records, in the task's history, a checkout it kept in place.
fn record_checkout_kept(
    dispatcher: &Dispatcher,
    task: &Task,
    reason: &str,
) -> Result<(), ContinuationError> {
    let now = now_iso();
    dispatcher.store().transaction(|tx| {
        append_event(
            tx,
            &task.id,
            "checkout_kept",
            task.state,
            &json!({ "reason": reason }),
            &now,
            None,
        )?;
        Ok(())
    })?;
    Ok(())
}

fn begin_checkout_removal(dispatcher: &Dispatcher, task: &Task) -> Result<Task, ContinuationError> {
    let now = now_iso();
    dispatcher.store().transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='removing_checkout',checkout_state=?,updated_at=? WHERE id=? AND state<> 'removing_checkout'",
            rusqlite::params![task.state.as_str(), now, task.id],
        )?;
        if changed != 1 {
            return Err(oga_store::StoreError::Refusal(format!(
                "task cannot start checkout removal: {}",
                task.id
            )));
        }
        append_event(
            tx,
            &task.id,
            "checkout_removal_started",
            TaskState::RemovingCheckout,
            &json!({}),
            &now,
            None,
        )?;
        Ok(())
    })?;
    require_task(dispatcher.store(), &task.id)
}

enum CheckoutAttempt {
    Removed,
    /// Kept on purpose, with the sentence a person reads.
    Kept(String),
    /// Git refused or could not be read.
    Failed(String),
}

async fn finish_checkout_removal(
    dispatcher: &Dispatcher,
    task: &Task,
    delete_branch: bool,
) -> CheckoutRemoval {
    let Some(worktree) = task.worktree.as_ref() else {
        return CheckoutRemoval::default();
    };
    let attempt = if Path::new(&worktree.path).exists() {
        match oga_worktree::worktree_has_uncommitted_work(worktree).await {
            Ok(true) => CheckoutAttempt::Kept("kept: it still has uncommitted changes".into()),
            Ok(false) => match oga_worktree::remove_task_worktree(worktree).await {
                Ok(()) => CheckoutAttempt::Removed,
                Err(error) => {
                    CheckoutAttempt::Failed(format!("could not remove the checkout: {error}"))
                }
            },
            Err(error) => {
                CheckoutAttempt::Failed(format!("could not inspect the checkout: {error}"))
            }
        }
    } else {
        match oga_worktree::remove_task_worktree(worktree).await {
            Ok(()) => CheckoutAttempt::Removed,
            Err(error) => CheckoutAttempt::Failed(format!("could not prune the checkout: {error}")),
        }
    };
    let removed = matches!(attempt, CheckoutAttempt::Removed);
    if removed {
        forget_index(dispatcher, worktree).await;
    }
    let (branch, branch_error) = if removed && delete_branch {
        match oga_worktree::remove_task_branch_safely(worktree).await {
            Ok(result) => (Some(result), None),
            Err(error) => (None, Some(format!("could not check the branch: {error}"))),
        }
    } else {
        (None, None)
    };
    // `error` is for git failing, not for a checkout Oga chose to keep.
    let (event, error) = match &attempt {
        CheckoutAttempt::Removed => ("checkout_removed", None),
        CheckoutAttempt::Kept(_) => ("checkout_kept", None),
        CheckoutAttempt::Failed(error) => ("checkout_removal_failed", Some(error.clone())),
    };
    let keep_reason = match attempt {
        CheckoutAttempt::Kept(reason) => Some(reason),
        _ => None,
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
            .and_then(parse_task_state)
            .unwrap_or(TaskState::Cancelled);
        let changed = tx.execute(
            "UPDATE tasks SET state=?,checkout_state=NULL,error=?,updated_at=? WHERE id=? AND state='removing_checkout'",
            rusqlite::params![state.as_str(), error, now, task.id],
        )?;
        if changed == 1 {
            append_event(
                tx,
                &task.id,
                event,
                state,
                &json!({
                    "reason": keep_reason,
                    "error": error,
                    "branchOutcome": branch.as_ref().map(|result| result.outcome),
                    "branchReason": branch.as_ref().and_then(|result| result.reason.as_deref()),
                    "branchError": branch_error,
                }),
                &now,
                None,
            )?;
        }
        Ok(())
    });
    CheckoutRemoval {
        keep_reason,
        branch,
        branch_error,
    }
}

/// A removed checkout's code index goes with it. Failing to drop it leaves
/// rows the next broker start prunes, so it never fails the removal.
async fn forget_index(dispatcher: &Dispatcher, worktree: &TaskWorktree) {
    let store = dispatcher.store().clone();
    let path = worktree.path.clone();
    let forgotten = tokio::task::spawn_blocking(move || {
        oga_context::ContextIndex::new(&store)
            .forget(&path)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result);
    if let Err(error) = forgotten {
        eprintln!(
            "could not drop the code index for {}: {error}",
            worktree.path
        );
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
        append_event(
            tx,
            &task.id,
            if archived { "archived" } else { "unarchived" },
            task.state,
            &json!({}),
            &now,
            None,
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
    let active = live_checkout_tasks(dispatcher, &worktree.path, &task.id)?;
    if !active.is_empty() {
        return Ok(WorktreeDeleteEntry::Skipped(WorktreeDeleteSkipped {
            task_id: task.id,
            skipped: true,
            state: task.state,
            reason: format!("shared with a live task ({})", active.join(", ")),
        }));
    }
    let checkout = if Path::new(&worktree.path).exists() {
        oga_worktree::remove_task_worktree(&worktree).await?;
        oga_domain::CheckoutOutcome::Removed
    } else {
        oga_worktree::remove_task_worktree(&worktree).await?;
        oga_domain::CheckoutOutcome::AlreadyGone
    };
    forget_index(dispatcher, &worktree).await;
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

/// The result of one automatic checkout collection pass.
#[derive(Debug, Clone, Default)]
pub struct CheckoutSweep {
    /// Tasks whose checkout was removed.
    pub removed: Vec<String>,
    /// A task or an unowned checkout path, and why it stayed.
    pub kept: Vec<CheckoutKept>,
    /// Unowned checkouts under the worktrees root that were removed.
    pub orphans_removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutKept {
    /// The task id, or the checkout path when no task row owns it.
    pub subject: String,
    pub reason: String,
}

/// Removes checkouts no live task needs: unfinished removals from a previous
/// broker, settled work the cleanup settings include, and unowned checkouts
/// under the worktrees root. Settled work waits out the retention the settings
/// name; a dirty checkout or one a live task uses stays.
pub async fn sweep_checkouts(
    dispatcher: &Dispatcher,
    cutoff: &str,
    archived_only: bool,
) -> Result<CheckoutSweep, ContinuationError> {
    let mut sweep = CheckoutSweep::default();
    // So a task the recovery pass already restored is not attempted again below.
    let mut handled = HashSet::new();

    // A removal a previous broker began but never finished: the row still reads
    // `removing_checkout`, which is neither live nor eligible below.
    for task in tasks_matching(dispatcher, "state='removing_checkout'")? {
        if let Some(worktree) = task.worktree.as_ref() {
            handled.insert(worktree.path.clone());
        }
        let removal = finish_checkout_removal(dispatcher, &task, false).await;
        record_removal(&mut sweep, &task.id, removal);
    }

    // Finished work the cleanup settings include and the retention has aged.
    for task in eligible_checkout_tasks(dispatcher, cutoff, archived_only)? {
        let Some(worktree) = task.worktree.as_ref() else {
            continue;
        };
        if !Path::new(&worktree.path).exists() || !handled.insert(worktree.path.clone()) {
            continue;
        }
        if let Some(reason) = checkout_in_use_reason(dispatcher, &task)? {
            record_checkout_kept(dispatcher, &task, &reason)?;
            push_kept(&mut sweep, &task.id, reason);
            continue;
        }
        let started = match begin_checkout_removal(dispatcher, &task) {
            Ok(started) => started,
            Err(error) => {
                push_kept(
                    &mut sweep,
                    &task.id,
                    format!("could not start removal: {error}"),
                );
                continue;
            }
        };
        let removal = finish_checkout_removal(dispatcher, &started, false).await;
        record_removal(&mut sweep, &task.id, removal);
    }

    sweep_orphan_checkouts(dispatcher, &mut sweep).await;
    Ok(sweep)
}

fn record_removal(sweep: &mut CheckoutSweep, subject: &str, removal: CheckoutRemoval) {
    match removal.keep_reason {
        Some(reason) => push_kept(sweep, subject, reason),
        None => sweep.removed.push(subject.to_owned()),
    }
}

fn push_kept(sweep: &mut CheckoutSweep, subject: &str, reason: String) {
    if !sweep.kept.iter().any(|kept| kept.subject == subject) {
        sweep.kept.push(CheckoutKept {
            subject: subject.to_owned(),
            reason,
        });
    }
}

fn tasks_matching(dispatcher: &Dispatcher, clause: &str) -> Result<Vec<Task>, ContinuationError> {
    let query = format!("SELECT id FROM tasks WHERE {clause}");
    load_tasks(dispatcher, task_ids(dispatcher, &query, None)?)
}

fn eligible_checkout_tasks(
    dispatcher: &Dispatcher,
    cutoff: &str,
    archived_only: bool,
) -> Result<Vec<Task>, ContinuationError> {
    let archive = if archived_only {
        " AND archived_at IS NOT NULL"
    } else {
        ""
    };
    let query = format!(
        "SELECT id FROM tasks WHERE worktree_path IS NOT NULL \
         AND state IN ('completed','failed','cancelled') AND updated_at < ?{archive} ORDER BY updated_at"
    );
    load_tasks(dispatcher, task_ids(dispatcher, &query, Some(cutoff))?)
}

fn task_ids(
    dispatcher: &Dispatcher,
    query: &str,
    cutoff: Option<&str>,
) -> Result<Vec<String>, ContinuationError> {
    let ids = dispatcher.store().with_connection(|connection| {
        let mut statement = connection.prepare(query)?;
        let ids = statement
            .query_map(rusqlite::params_from_iter(cutoff), |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    })?;
    Ok(ids)
}

fn load_tasks(dispatcher: &Dispatcher, ids: Vec<String>) -> Result<Vec<Task>, ContinuationError> {
    ids.into_iter()
        .map(|id| {
            lifecycle::load_task(dispatcher.store(), &id)?.ok_or_else(|| {
                ContinuationError::Refusal(format!("task disappeared from checkout: {id}"))
            })
        })
        .collect()
}

/// Removes checkouts under the worktrees root that no task row references.
/// Only a clean git checkout goes; anything else is left where it is.
async fn sweep_orphan_checkouts(dispatcher: &Dispatcher, sweep: &mut CheckoutSweep) {
    let root = dispatcher.worktrees_root().to_path_buf();
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let path = path.to_string_lossy().into_owned();
        if path_is_owned(dispatcher, &path).unwrap_or(true) {
            continue;
        }
        let Some(worktree) = orphan_worktree(&path).await else {
            continue;
        };
        match oga_worktree::worktree_has_uncommitted_work(&worktree).await {
            Ok(false) => match oga_worktree::remove_task_worktree(&worktree).await {
                Ok(()) => sweep.orphans_removed.push(path),
                Err(error) => push_kept(sweep, &path, format!("could not remove it: {error}")),
            },
            Ok(true) => push_kept(sweep, &path, "it still has uncommitted changes".into()),
            Err(error) => push_kept(sweep, &path, format!("could not inspect it: {error}")),
        }
    }
}

/// Builds the worktree record an unowned checkout needs, resolving the
/// repository git records for it. `None` leaves a path git cannot place.
async fn orphan_worktree(path: &str) -> Option<TaskWorktree> {
    let root = oga_worktree::checkout_repository_root(Path::new(path))
        .await
        .ok()
        .flatten()?;
    Some(TaskWorktree {
        origin_cwd: root.to_string_lossy().into_owned(),
        path: path.to_owned(),
        branch: String::new(),
        links: None,
        from: None,
        base: None,
    })
}

fn path_is_owned(dispatcher: &Dispatcher, path: &str) -> Result<bool, ContinuationError> {
    let count = dispatcher.store().with_connection(|connection| {
        Ok(connection.query_row(
            "SELECT COUNT(*) FROM tasks WHERE worktree_path=?",
            [path],
            |row| row.get::<_, u64>(0),
        )?)
    })?;
    Ok(count > 0)
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
