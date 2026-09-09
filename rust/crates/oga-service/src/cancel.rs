//! Cancellation and timeout transitions.

use oga_domain::{CompletionCode, Task, TaskCompletion, TaskState};
use serde_json::json;

use crate::{
    ContinuationError, append_event_tx, dispatch::Dispatcher, lifecycle::now_iso, require_task,
};

#[derive(Debug, Clone, Default)]
pub struct CancelRequest {
    pub task_id: String,
    pub reason: Option<String>,
    pub timed_out: bool,
}

impl CancelRequest {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            reason: None,
            timed_out: false,
        }
    }

    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn timed_out(mut self) -> Self {
        self.timed_out = true;
        self
    }
}

pub async fn cancel(
    dispatcher: &Dispatcher,
    request: CancelRequest,
) -> Result<Task, ContinuationError> {
    let task = require_task(dispatcher.store(), &request.task_id)?;
    if task.state == TaskState::Cancelled {
        return Ok(task);
    }
    let reason = request
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or(if request.timed_out {
            "task timed out"
        } else {
            "cancelled by caller"
        })
        .to_owned();
    let code = if request.timed_out {
        CompletionCode::Timeout
    } else {
        CompletionCode::Cancelled
    };
    let state = if request.timed_out {
        TaskState::Failed
    } else {
        TaskState::Cancelled
    };
    let completion = TaskCompletion {
        exit_code: None,
        blocked: true,
        code,
        reason: Some(reason.clone()),
        suggested_scope: None,
        resets_at: None,
        asserted_completion: None,
        dependency_blocked: None,
    };

    if let Some(process) = dispatcher.active_runs().get(&request.task_id) {
        process.cancel().await;
    }

    let now = now_iso();
    dispatcher.store().transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state=?,error=?,completion_json=?,updated_at=? WHERE id=? AND state IN ('queued','pending','running','needs_input','answered','blocked')",
            rusqlite::params![
                state.as_str(),
                reason,
                serde_json::to_string(&completion)
                    .map_err(|error| oga_store::StoreError::Refusal(error.to_string()))?,
                now,
                request.task_id,
            ],
        )?;
        if changed != 1 {
            return Err(oga_store::StoreError::Refusal(format!(
                "task cannot be cancelled from state {}: {}",
                task.state.as_str(),
                request.task_id
            )));
        }
        tx.execute(
            "DELETE FROM task_holds WHERE task_id=?",
            [request.task_id.as_str()],
        )?;
        let waiting: i64 = tx.query_row(
            "SELECT COUNT(*) FROM task_follow_ups WHERE task_id=?",
            [request.task_id.as_str()],
            |row| row.get(0),
        )?;
        // Cancelling a task means abandoning its queue. A timeout does not:
        // it lands in `failed`, and the instructions waiting behind it are
        // still wanted, so a resume can send them.
        if waiting > 0 {
            if crate::follow_ups::settling_clears_follow_ups(state) {
                tx.execute(
                    "DELETE FROM task_follow_ups WHERE task_id=?",
                    [request.task_id.as_str()],
                )?;
                append_event_tx(
                    tx,
                    &request.task_id,
                    "follow_ups_dropped",
                    state,
                    json!({"dropped": waiting, "reason": reason}),
                    &now,
                )?;
            } else {
                append_event_tx(
                    tx,
                    &request.task_id,
                    "follow_ups_paused",
                    state,
                    json!({"waiting": waiting}),
                    &now,
                )?;
            }
        }
        append_event_tx(
            tx,
            &request.task_id,
            state.as_str(),
            state,
            json!({"error": reason, "completion": completion}),
            &now,
        )?;
        Ok(())
    })?;
    let cancelled = require_task(dispatcher.store(), &request.task_id)?;
    dispatcher.settle_dependents(&cancelled);
    Ok(cancelled)
}
