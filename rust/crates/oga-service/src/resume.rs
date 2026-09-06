//! Resume and fresh-session reseed transitions.

use oga_domain::{HoldArgs, HoldVerb, Task, TaskHold, TaskScope, TaskState};
use serde_json::json;

use crate::{
    ContinuationError, append_event_tx, close_attempt,
    dispatch::Dispatcher,
    encode_store,
    handoff_brief::{FreshSessionCause, HandoffBriefOptions, handoff_brief},
    holds::{HOLD_EXPIRY, HoldSweep},
    lifecycle::now_iso,
    require_existing_worktree, require_profile, require_task, resume_prompt,
    schedule::{StartAt, parse_start_at},
    validate_model, waiting,
};

/// How long a `rate_limit` hold waits when the failed run named no reset time.
const RATE_LIMIT_FALLBACK_MS: i64 = 10 * 60 * 1_000;

#[derive(Debug, Clone, Default)]
pub struct ResumeRequest {
    pub task_id: String,
    pub instruction: Option<String>,
    pub scope: Option<TaskScope>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub timeout_ms: Option<u64>,
    pub allow_questions: Option<bool>,
    pub start_at: Option<String>,
}

impl ResumeRequest {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            ..Self::default()
        }
    }

    pub fn instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instruction = Some(instruction.into());
        self
    }

    pub fn scope(mut self, scope: TaskScope) -> Self {
        self.scope = Some(scope);
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

    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn allow_questions(mut self, allow_questions: bool) -> Self {
        self.allow_questions = Some(allow_questions);
        self
    }

    pub fn start_at(mut self, start_at: impl Into<String>) -> Self {
        self.start_at = Some(start_at.into());
        self
    }
}

pub async fn resume(
    dispatcher: &Dispatcher,
    request: ResumeRequest,
) -> Result<Task, ContinuationError> {
    let _worktree_guard = dispatcher.worktree_operations().lock().await;
    let old = require_task(dispatcher.store(), &request.task_id)?;
    if !matches!(
        old.state,
        TaskState::Failed
            | TaskState::Cancelled
            | TaskState::Blocked
            | TaskState::Completed
            | TaskState::Pending
    ) {
        return Err(ContinuationError::Refusal(format!(
            "task cannot be resumed from state {}: {}",
            old.state.as_str(),
            old.id
        )));
    }
    let instruction = request
        .instruction
        .as_deref()
        .map(str::trim)
        .filter(|instruction| !instruction.is_empty())
        .map(str::to_owned);
    if old.state == TaskState::Completed && instruction.is_none() {
        return Err(ContinuationError::Refusal(format!(
            "task cannot be resumed without an instruction: {}",
            old.id
        )));
    }
    let mut recreated_worktree = false;
    if let Some(worktree) = &old.worktree {
        if !std::path::Path::new(&worktree.path).join(".git").exists() {
            if old.archived_at.is_some() {
                oga_worktree::recreate_task_worktree(worktree).await?;
                recreated_worktree = true;
            } else {
                require_existing_worktree(&old)?;
            }
        } else {
            require_existing_worktree(&old)?;
        }
    }
    let profile = require_profile(dispatcher.store(), &old.profile_id)?;
    if old.session_id.is_some() && profile.command.is_some() {
        return Err(ContinuationError::Refusal(format!(
            "profile {} runs a custom command; provider sessions are never captured, so resume cannot continue this task",
            profile.id
        )));
    }
    let model = request
        .model
        .as_deref()
        .map(validate_model)
        .transpose()?
        .unwrap_or_else(|| old.model.clone());
    if let Some(timeout_ms) = request.timeout_ms
        && !(1..=86_400_000).contains(&timeout_ms)
    {
        return Err(ContinuationError::Refusal(
            "timeoutMs must be an integer between 1 and 86400000".into(),
        ));
    }
    let effort = request
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(str::to_owned)
        .or_else(|| old.effort.clone());
    if let Some(start_at) = request.start_at.as_deref() {
        // A held resume replays its instruction and nothing else, so settings
        // stated now would be dropped the moment the hold releases.
        let dropped: Vec<&str> = [
            request.model.is_some().then_some("model"),
            request.effort.is_some().then_some("effort"),
            request.timeout_ms.is_some().then_some("timeoutMs"),
            request.scope.is_some().then_some("scope"),
            request
                .allow_questions
                .is_some()
                .then_some("allowQuestions"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !dropped.is_empty() {
            return Err(ContinuationError::Refusal(format!(
                "a scheduled resume carries only its instruction: {} — {} would be lost when it starts; resume without startAt to change them",
                old.id,
                dropped.join(" and ")
            )));
        }
        return hold_until(dispatcher, &old, instruction, start_at);
    }
    let now = now_iso();
    let attempts = close_attempt(&old, &now, false);
    let attempts_json = encode_store(&attempts)?;
    let scope_updated = request.scope.is_some();
    let scope = request.scope.unwrap_or_else(|| old.scope.clone());
    let scope_json = encode_store(&scope)?;
    let allow_questions = request.allow_questions.unwrap_or(old.allow_questions);
    let was_archived = old.archived_at.is_some();
    let launch_instruction = instruction
        .clone()
        .unwrap_or_else(|| crate::dispatch::CONTINUE_INSTRUCTION.to_owned());
    dispatcher.store().transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='queued',output='',error=NULL,question=NULL,completion_json=NULL,attempts_json=?,timeout_ms=COALESCE(?,timeout_ms),scope_json=?,allow_questions=?,model=?,effort=?,archived_at=NULL,updated_at=? WHERE id=? AND state IN ('failed','cancelled','blocked','completed','pending')",
            rusqlite::params![
                attempts_json,
                request.timeout_ms,
                scope_json,
                i64::from(allow_questions),
                model,
                effort,
                now,
                old.id,
            ],
        )?;
        if changed != 1 {
            return Err(oga_store::StoreError::Refusal(format!(
                "task cannot be resumed: {}",
                old.id
            )));
        }
        if old.state == TaskState::Pending {
            tx.execute("DELETE FROM task_holds WHERE task_id=?", [old.id.as_str()])?;
            append_event_tx(
                tx,
                &old.id,
                "hold_released",
                TaskState::Pending,
                json!({"forced": true}),
                &now,
            )?;
        }
        if was_archived {
            append_event_tx(
                tx,
                &old.id,
                "unarchived",
                TaskState::Queued,
                json!({"reason": "resumed"}),
                &now,
            )?;
        }
        if recreated_worktree {
            append_event_tx(
                tx,
                &old.id,
                "worktree_recreated",
                TaskState::Queued,
                json!({
                    "worktree": old.worktree.as_ref().map(|value| value.path.clone()),
                    "branch": old.worktree.as_ref().map(|value| value.branch.clone()),
                }),
                &now,
            )?;
        }
        let mut resumed_payload = json!({
            "previousState": old.state,
            "attempt": attempts.len(),
        });
        if scope_updated {
            resumed_payload["scopeUpdated"] = json!(true);
        }
        if let Some(allow_questions) = request.allow_questions {
            resumed_payload["allowQuestions"] = json!(allow_questions);
        }
        if let Some(model) = &request.model {
            resumed_payload["model"] = json!(model);
        }
        if let Some(effort) = &request.effort {
            resumed_payload["effort"] = json!(effort);
        }
        if let Some(instruction) = &instruction {
            resumed_payload["instruction"] = json!(instruction);
        }
        append_event_tx(
            tx,
            &old.id,
            "resumed",
            TaskState::Queued,
            resumed_payload,
            &now,
        )?;
        Ok(())
    })?;
    let task = require_task(dispatcher.store(), &old.id)?;
    // The row has left its ending, so the dependents that ending dropped go
    // back to waiting. After the transition, never before: a dependent restored
    // while this task still read `failed` would be dropped again.
    dispatcher.restore_dropped_dependents(&task);
    // No session to reopen: a short "continue" instruction gives the worker
    // nothing to work from, so the full brief — the original task plus a
    // summary of the prior run — is rebuilt and shipped instead.
    let prompt = if old.session_id.is_none() {
        let events = dispatcher.store().repositories().events().list(&old.id)?;
        let fresh = handoff_brief(
            &Task {
                state: old.state,
                output: old.output.clone(),
                error: old.error.clone(),
                question: old.question.clone(),
                completion: old.completion.clone(),
                ..task.clone()
            },
            &events,
            profile.provider,
            &HandoffBriefOptions {
                same_account: true,
                instruction: Some(instruction.clone().unwrap_or_default()),
                fresh_session_cause: Some(
                    if old.state == oga_domain::TaskState::Pending
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
            },
        );
        let fallback_now = now_iso();
        dispatcher.store().transaction(|tx| {
            append_event_tx(
                tx,
                &task.id,
                "resume_fallback",
                task.state,
                json!({
                    "reason": "session_not_captured",
                    "tier": fresh.tier,
                    "chars": fresh.chars,
                }),
                &fallback_now,
            )?;
            Ok(())
        })?;
        fresh.prompt
    } else {
        resume_prompt(old.state, &launch_instruction, true)
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
        old.session_id,
    );
    Ok(task)
}

/// Parks the resume behind a hold instead of running it. `rate_limit` keys the
/// release on the account rather than the clock alone — the reset time is only
/// the first place to look, and the sweep will not start a run into an account
/// still reporting no usage.
fn hold_until(
    dispatcher: &Dispatcher,
    old: &Task,
    instruction: Option<String>,
    start_at: &str,
) -> Result<Task, ContinuationError> {
    let now_ms = oga_routing::now_ms();
    let start_at = parse_start_at(start_at, now_ms).map_err(ContinuationError::Refusal)?;
    let (until, await_profile, note) = match start_at {
        StartAt::At(instant) => {
            let note = waiting::scheduled_start_note(&instant);
            (instant, None, note)
        }
        StartAt::WhenUsageResets => {
            let instant = old
                .completion
                .as_ref()
                .and_then(|completion| completion.resets_at.clone())
                .unwrap_or_else(|| {
                    oga_routing::format_rfc3339_ms(now_ms.saturating_add(RATE_LIMIT_FALLBACK_MS))
                });
            let note = waiting::rate_limit_wait_note(&instant);
            (instant, Some(old.profile_id.clone()), note)
        }
    };
    let scheduled = await_profile.is_none();
    let now = now_iso();
    let hold = TaskHold {
        task_id: old.id.clone(),
        verb: HoldVerb::Resume,
        args: HoldArgs {
            instruction,
            scheduled: scheduled.then_some(true),
            ..HoldArgs::default()
        },
        start_at: Some(until.clone()),
        await_model: await_profile.as_ref().map(|_| old.model.clone()),
        await_profile,
        next_check_at: until,
        expires_at: oga_routing::format_rfc3339_ms(
            now_ms.saturating_add(HOLD_EXPIRY.as_millis() as i64),
        ),
        probe_count: 0,
        note,
        created_at: now.clone(),
        updated_at: now,
    };
    Ok(HoldSweep::new(dispatcher.store().clone()).arm(&hold)?)
}
