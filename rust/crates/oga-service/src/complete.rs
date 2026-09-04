//! Caller-asserted completion transitions.

use oga_domain::{CompletionCode, Task, TaskCompletion, TaskCompletionOverride, TaskState};
use serde_json::json;

use crate::{
    ContinuationError, append_event_tx, dispatch::Dispatcher, lifecycle::now_iso, require_task,
};

#[derive(Debug, Clone)]
pub struct CompletionAssertion {
    pub task_id: String,
    pub asserted_by: String,
    pub reason: String,
}

impl CompletionAssertion {
    pub fn new(
        task_id: impl Into<String>,
        asserted_by: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            asserted_by: asserted_by.into(),
            reason: reason.into(),
        }
    }
}

pub fn assert_completion(
    dispatcher: &Dispatcher,
    assertion: CompletionAssertion,
) -> Result<Task, ContinuationError> {
    let (asserted_by, reason) = validate_assertion(&assertion)?;
    let task = require_task(dispatcher.store(), &assertion.task_id)?;
    if !matches!(task.state, TaskState::Blocked | TaskState::Failed) {
        return Err(ContinuationError::Refusal(format!(
            "task cannot be asserted completed from state {}: {}",
            task.state.as_str(),
            task.id
        )));
    }
    write_completion(dispatcher, &task, &asserted_by, &reason, false)
}

pub fn force_complete(
    dispatcher: &Dispatcher,
    assertion: CompletionAssertion,
) -> Result<Task, ContinuationError> {
    let (asserted_by, reason) = validate_assertion(&assertion)?;
    let task = require_task(dispatcher.store(), &assertion.task_id)?;
    if task.state == TaskState::Completed {
        return Ok(task);
    }
    if let Some(process) = dispatcher.active_runs().get(&task.id) {
        process.cancel_now();
    }
    write_completion(dispatcher, &task, &asserted_by, &reason, true)
}

fn validate_assertion(
    assertion: &CompletionAssertion,
) -> Result<(String, String), ContinuationError> {
    let asserted_by = assertion.asserted_by.trim();
    if asserted_by.is_empty() {
        return Err(ContinuationError::Refusal(format!(
            "asserted completion needs who asserted it: {}",
            assertion.task_id
        )));
    }
    if asserted_by.len() > 200 {
        return Err(ContinuationError::Refusal(
            "assertedBy exceeds 200 characters".into(),
        ));
    }
    let reason = assertion.reason.trim();
    if reason.is_empty() {
        return Err(ContinuationError::Refusal(format!(
            "asserted completion needs a reason: {}",
            assertion.task_id
        )));
    }
    if reason.len() > 500 {
        return Err(ContinuationError::Refusal(
            "reason exceeds 500 characters".into(),
        ));
    }
    Ok((asserted_by.to_owned(), reason.to_owned()))
}

fn write_completion(
    dispatcher: &Dispatcher,
    task: &Task,
    asserted_by: &str,
    reason: &str,
    force: bool,
) -> Result<Task, ContinuationError> {
    let now = now_iso();
    let mut completion = task.completion.clone().unwrap_or(TaskCompletion {
        exit_code: None,
        blocked: true,
        code: CompletionCode::WorkerError,
        reason: None,
        suggested_scope: None,
        resets_at: None,
        asserted_completion: None,
        dependency_blocked: None,
    });
    completion.asserted_completion = Some(TaskCompletionOverride {
        asserted_by: asserted_by.to_owned(),
        reason: reason.to_owned(),
        asserted_at: now.clone(),
        replaced_code: task.completion.as_ref().map(|value| value.code),
    });
    let completion_json = serde_json::to_string(&completion)
        .map_err(|error| ContinuationError::Refusal(format!("invalid completion JSON: {error}")))?;
    dispatcher.store().transaction(|tx| {
        let states = if force {
            "'queued','running','needs_input','answered','blocked','failed','cancelled','pending'"
        } else {
            "'blocked','failed'"
        };
        let changed = tx.execute(
            &format!(
                "UPDATE tasks SET state='completed',completion_json=?,updated_at=? WHERE id=? AND state IN ({states})"
            ),
            rusqlite::params![completion_json, now, task.id],
        )?;
        if changed != 1 {
            return Err(oga_store::StoreError::Refusal(format!(
                "task cannot be marked completed from state {}: {}",
                task.state.as_str(),
                task.id
            )));
        }
        append_event_tx(
            tx,
            &task.id,
            "completion_asserted",
            TaskState::Completed,
            json!({
                "assertedBy": asserted_by,
                "reason": reason,
                "previousState": task.state,
                "replacedCode": task.completion.as_ref().map(|value| value.code),
            }),
            &now,
        )?;
        Ok(())
    })?;
    let completed = require_task(dispatcher.store(), &task.id)?;
    dispatcher.settle_dependents(&completed);
    Ok(completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assertion_rejects_blank_identity_and_reason() {
        let blank = CompletionAssertion::new("task", " ", "why");
        assert!(validate_assertion(&blank).is_err());
        let blank_reason = CompletionAssertion::new("task", "person", " ");
        assert!(validate_assertion(&blank_reason).is_err());
    }
}
