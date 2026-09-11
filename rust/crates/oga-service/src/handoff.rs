//! Moving a task between profiles or models.

use oga_domain::{Task, TaskScope, TaskState};
use serde_json::json;

use crate::{
    ContinuationError, append_event_tx, close_attempt,
    dispatch::Dispatcher,
    encode_store,
    handoff_brief::{FreshSessionCause, HandoffBriefOptions, handoff_brief},
    lifecycle::now_iso,
    require_existing_worktree, require_profile, require_task, validate_model,
};

#[derive(Debug, Clone, Default)]
pub struct HandoffRequest {
    pub task_id: String,
    pub profile_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub scope: Option<TaskScope>,
}

impl HandoffRequest {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            ..Self::default()
        }
    }

    pub fn profile(mut self, profile_id: impl Into<String>) -> Self {
        self.profile_id = Some(profile_id.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = Some(effort.into());
        self
    }

    pub fn scope(mut self, scope: TaskScope) -> Self {
        self.scope = Some(scope);
        self
    }
}

pub async fn handoff(
    dispatcher: &Dispatcher,
    request: HandoffRequest,
) -> Result<Task, ContinuationError> {
    let old = require_task(dispatcher.store(), &request.task_id)?;
    if !matches!(
        old.state,
        TaskState::Failed
            | TaskState::Cancelled
            | TaskState::Blocked
            | TaskState::Queued
            | TaskState::Pending
            | TaskState::Running
            | TaskState::NeedsInput
            | TaskState::Answered
    ) {
        return Err(ContinuationError::Refusal(format!(
            "task cannot be handed off from state {}: {}",
            old.state.as_str(),
            old.id
        )));
    }
    require_existing_worktree(&old)?;
    let profile_id = request
        .profile_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&old.profile_id)
        .to_owned();
    let same_profile = profile_id == old.profile_id;
    let requested_model = request.model.as_deref().map(validate_model).transpose()?;
    if same_profile && requested_model.is_none() {
        return Err(ContinuationError::Refusal(format!(
            "handoff needs a destination: task {} is already on {} — pass model to switch its model, or name a different profile to move it to",
            old.id, profile_id
        )));
    }
    if same_profile && requested_model.as_deref() == Some(old.model.as_str()) {
        return Err(ContinuationError::Refusal(format!(
            "nothing to move: task {} already runs {}/{} — name a different model or profile",
            old.id, profile_id, old.model
        )));
    }
    let profile = require_profile(dispatcher.store(), &profile_id)?;
    if !profile.enabled {
        return Err(ContinuationError::Refusal(format!(
            "profile disabled: {profile_id}"
        )));
    }
    let model = validate_model(
        requested_model
            .as_deref()
            .unwrap_or(profile.default_model.as_str()),
    )?;
    let effort = request
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| old.effort.clone());
    let scope_updated = request.scope.is_some();
    let scope = request.scope.unwrap_or_else(|| old.scope.clone());
    let scope_json = encode_store(&scope)?;
    // A hold that names the old profile is that account's wait — a rate limit,
    // an account with no usage left. Moving the task off that account is what
    // resolves it, so the move drops the hold and runs instead of inheriting a
    // reset time the destination never had to wait for. Every other hold — a
    // prerequisite, a scheduled start — outlives the move.
    let held_on_old_account = crate::holds::get_hold(dispatcher.store(), &old.id)?
        .is_some_and(|hold| hold.await_profile.as_deref() == Some(old.profile_id.as_str()));
    if matches!(old.state, TaskState::Queued | TaskState::Pending)
        && (same_profile || !held_on_old_account)
    {
        let now = now_iso();
        // A provider session belongs to one account, so a row that changes
        // profile leaves its session behind even while it keeps waiting.
        let session_id = old.session_id.clone().filter(|_| same_profile);
        dispatcher.store().transaction(|tx| {
            let changed = tx.execute(
                "UPDATE tasks SET profile_id=?,model=?,effort=?,session_id=?,scope_json=?,updated_at=? WHERE id=? AND state IN ('queued','pending')",
                rusqlite::params![profile_id, model, effort, session_id, scope_json, now, old.id],
            )?;
            if changed != 1 {
                return Err(oga_store::StoreError::Refusal(format!(
                    "task cannot be handed off: {}",
                    old.id
                )));
            }
            append_event_tx(
                tx,
                &old.id,
                "handed_off",
                old.state,
                json!({
                    "previousState": old.state,
                    "fromProfile": old.profile_id,
                    "toProfile": profile_id,
                    "fromModel": old.model,
                    "model": model,
                    "sessionPreserved": same_profile,
                    "scopeUpdated": scope_updated,
                }),
                &now,
            )?;
            if !same_profile && !scope_updated {
                append_event_tx(
                    tx,
                    &old.id,
                    "scope_inherited",
                    old.state,
                    json!({
                        "scope": scope,
                        "approvedFor": old.profile_id,
                        "usedBy": profile_id,
                    }),
                    &now,
                )?;
            }
            Ok(())
        })?;
        return require_task(dispatcher.store(), &old.id);
    }
    let preserve_session = same_profile && old.session_id.is_some() && profile.command.is_none();

    if old.state == TaskState::Running {
        let Some(process) = dispatcher.active_runs().get(&old.id) else {
            return Err(ContinuationError::Refusal(format!(
                "task {} is running but this broker has no worker to stop",
                old.id
            )));
        };
        process.cancel().await;
    }

    let now = now_iso();
    let attempts = close_attempt(&old, &now, old.state == TaskState::Running);
    let attempts_json = encode_store(&attempts)?;
    let session_id = if preserve_session {
        old.session_id.clone()
    } else {
        None
    };
    // Built before the row moves: `old` still names the profile whose work
    // this is, and still carries the failure the brief has to explain. A
    // provider session belongs to one account, so any move off it — same
    // profile without a usable session, or a different profile entirely —
    // needs the original task and the prior run reproduced from Oga's own
    // record instead of a session that the destination cannot reopen.
    let brief = if preserve_session {
        None
    } else {
        let events = dispatcher.store().repositories().events().list(&old.id)?;
        Some(handoff_brief(
            &old,
            &events,
            profile.provider,
            &HandoffBriefOptions {
                same_account: same_profile,
                fresh_session_cause: same_profile.then_some(
                    if old.state == TaskState::Pending
                        || old
                            .completion
                            .as_ref()
                            .and_then(|completion| completion.dependency_blocked)
                            .unwrap_or(false)
                    {
                        FreshSessionCause::NotStarted
                    } else {
                        FreshSessionCause::SessionNotCaptured
                    },
                ),
                ..HandoffBriefOptions::default()
            },
        ))
    };
    dispatcher.store().transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='queued',output='',error=NULL,question=NULL,completion_json=NULL,attempts_json=?,profile_id=?,model=?,effort=?,session_id=?,scope_json=?,updated_at=? WHERE id=? AND state IN ('failed','cancelled','blocked','queued','pending','running','needs_input','answered')",
            rusqlite::params![
                attempts_json,
                profile_id,
                model,
                effort,
                session_id,
                scope_json,
                now,
                old.id,
            ],
        )?;
        if changed != 1 {
            return Err(oga_store::StoreError::Refusal(format!(
                "task cannot be handed off: {}",
                old.id
            )));
        }
        tx.execute(
            "UPDATE task_turns SET status='interrupted',ended_at=? WHERE task_id=? AND status='running'",
            rusqlite::params![now, old.id],
        )?;
        tx.execute("DELETE FROM task_holds WHERE task_id=?", [old.id.as_str()])?;
        if held_on_old_account {
            append_event_tx(
                tx,
                &old.id,
                "hold_released",
                TaskState::Queued,
                json!({
                    "note": format!("moved to {profile_id}, no longer waiting for {} to have usage again", old.profile_id),
                    "wait": "rate_limit",
                }),
                &now,
            )?;
        }
        append_event_tx(
            tx,
            &old.id,
            "handed_off",
            TaskState::Queued,
            json!({
                "previousState": old.state,
                "fromProfile": old.profile_id,
                "toProfile": profile_id,
                "fromModel": old.model,
                "model": model,
                "attempt": attempts.len(),
                "sessionPreserved": preserve_session,
                "scopeUpdated": scope_updated,
            }),
            &now,
        )?;
        if !same_profile && !scope_updated {
            append_event_tx(
                tx,
                &old.id,
                "scope_inherited",
                TaskState::Queued,
                json!({
                    "scope": scope,
                    "approvedFor": old.profile_id,
                    "usedBy": profile_id,
                }),
                &now,
            )?;
        }
        Ok(())
    })?;
    let task = require_task(dispatcher.store(), &old.id)?;
    // The row has left its ending, so the dependents that ending dropped go
    // back to waiting. After the transition, never before: a dependent restored
    // while this task still read `failed` would be dropped again.
    dispatcher.restore_dropped_dependents(&task);
    let prompt = match &brief {
        Some(brief) => {
            dispatcher.store().transaction(|tx| {
                append_event_tx(
                    tx,
                    &task.id,
                    "handoff_brief",
                    task.state,
                    json!({
                        "tier": brief.tier,
                        "chars": brief.chars,
                        "omittedMessages": brief.omitted_messages,
                    }),
                    &now_iso(),
                )?;
                Ok(())
            })?;
            brief.prompt.clone()
        }
        None => format!(
            "This task now runs model `{}` in the same provider session. Continue without repeating completed work.",
            task.model
        ),
    };
    dispatcher.launch_continuation(
        task.clone(),
        profile,
        crate::prompt::WorkerPromptInput {
            task: prompt,
            allow_questions: task.allow_questions,
            scope: Some(task.scope.clone()),
            ..crate::prompt::WorkerPromptInput::default()
        },
        session_id,
    );
    Ok(task)
}
