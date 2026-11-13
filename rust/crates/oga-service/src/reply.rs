//! Answering a worker question and reopening its provider session.

use oga_domain::{Task, TaskScope, TaskState};
use oga_store::append_event;
use serde_json::json;

use crate::{
    ContinuationError, SteerRequest,
    acp_question::Answer,
    close_attempt, continuation_prompt,
    dispatch::Dispatcher,
    encode_store,
    lifecycle::{ActiveRun, now_iso},
    require_existing_worktree, require_profile, require_task,
};

#[derive(Debug, Clone)]
pub struct ReplyRequest {
    pub task_id: String,
    pub answer: String,
    pub scope: Option<TaskScope>,
}

impl ReplyRequest {
    pub fn new(task_id: impl Into<String>, answer: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            answer: answer.into(),
            scope: None,
        }
    }

    pub fn scope(mut self, scope: TaskScope) -> Self {
        self.scope = Some(scope);
        self
    }
}

pub async fn reply(
    dispatcher: &Dispatcher,
    request: ReplyRequest,
) -> Result<Task, ContinuationError> {
    let old = require_task(dispatcher.store(), &request.task_id)?;
    if old.state != TaskState::NeedsInput {
        let message = if old.state == TaskState::Blocked {
            format!(
                "task does not need input: {} — state is blocked; use resume with your answer as the instruction",
                old.id
            )
        } else {
            format!(
                "task does not need input: {} (state: {})",
                old.id,
                old.state.as_str()
            )
        };
        return Err(ContinuationError::Refusal(message));
    }
    let answer = request.answer.trim();
    if answer.is_empty() {
        return Err(ContinuationError::Refusal(format!(
            "reply needs an answer: {}",
            old.id
        )));
    }
    if let Some((run, waiting)) = dispatcher
        .active_runs()
        .get(&old.id)
        .and_then(|run| run.take_question().map(|waiting| (run, waiting)))
    {
        let reply = LiveReply {
            answer: Answer::read(answer),
            scope: request.scope,
        };
        return answer_live_question(dispatcher, &old, &run, waiting, reply).await;
    }
    require_existing_worktree(&old)?;
    let profile = require_profile(dispatcher.store(), &old.profile_id)?;
    if profile.command.is_some() {
        return Err(ContinuationError::Refusal(format!(
            "profile {} runs a custom command; provider sessions are never captured, so reply cannot continue this task",
            profile.id
        )));
    }
    let Some(session_id) = old.session_id.clone() else {
        return Err(ContinuationError::Refusal(format!(
            "task has no captured session to reply to: {}",
            old.id
        )));
    };
    let now = now_iso();
    let attempts = close_attempt(&old, &now, false);
    let scope_updated = request.scope.is_some();
    let scope = request.scope.unwrap_or_else(|| old.scope.clone());
    let attempts_json = encode_store(&attempts)?;
    let scope_json = encode_store(&scope)?;
    dispatcher.store().transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='queued',output='',error=NULL,question=NULL,completion_json=NULL,attempts_json=?,scope_json=?,updated_at=? WHERE id=? AND state='needs_input'",
            rusqlite::params![attempts_json, scope_json, now, old.id],
        )?;
        if changed != 1 {
            return Err(oga_store::StoreError::Refusal(format!(
                "task does not need input: {}",
                old.id
            )));
        }
        append_event(
            tx,
            &old.id,
            "answered",
            TaskState::Queued,
            &json!({
                "attempt": attempts.len(),
                "answer": answer,
                "scopeUpdated": scope_updated,
            }),
            &now,
            None,
        )?;
        Ok(())
    })?;
    let task = require_task(dispatcher.store(), &old.id)?;
    dispatcher.launch_continuation(
        task.clone(),
        profile,
        crate::prompt::WorkerPromptInput {
            task: continuation_prompt(
                &old.prompt,
                &format!(
                    "The worker asked: {}\nYour answer: {answer}",
                    old.question.as_deref().unwrap_or("What input is required?")
                ),
            ),
            ..crate::prompt::WorkerPromptInput::default()
        },
        Some(session_id),
    );
    Ok(task)
}

/// What a person said to a question the worker is parked on.
struct LiveReply {
    answer: Answer,
    scope: Option<TaskScope>,
}

/// The task runs again before the worker hears the answer, so the worker's
/// next question opens only after this one has closed.
async fn answer_live_question(
    dispatcher: &Dispatcher,
    old: &Task,
    run: &ActiveRun,
    waiting: tokio::sync::oneshot::Sender<Answer>,
    LiveReply { answer, scope }: LiveReply,
) -> Result<Task, ContinuationError> {
    let now = now_iso();
    let scope_json = scope.as_ref().map(encode_store).transpose()?;
    let instruction = match &answer {
        Answer::Instead(instruction) => Some(instruction.clone()),
        Answer::Allow | Answer::Refuse => None,
    };
    let mut payload = json!({"answer": answer.name(), "scopeUpdated": scope.is_some()});
    if let Some(instruction) = &instruction {
        payload["instruction"] = json!(instruction);
    }
    dispatcher.store().transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='running',question=NULL,scope_json=COALESCE(?,scope_json),updated_at=? WHERE id=? AND state='needs_input'",
            rusqlite::params![scope_json, now, old.id],
        )?;
        if changed != 1 {
            return Err(oga_store::StoreError::Refusal(format!(
                "task does not need input: {}",
                old.id
            )));
        }
        append_event(
            tx,
            &old.id,
            "permission_replied",
            TaskState::Running,
            &payload,
            &now,
            None,
        )?;
        Ok(())
    })?;
    // Set before the worker hears the answer, so its next request sees it.
    if let Some(scope) = scope {
        run.set_scope(scope);
    }
    if waiting.send(answer).is_err() {
        return Err(ContinuationError::Refusal(format!(
            "the worker stopped before your answer reached it: {}",
            old.id
        )));
    }
    if let Some(instruction) = instruction {
        crate::steer::steer(
            dispatcher,
            SteerRequest::new(&old.id).instruction(instruction),
        )
        .await?;
    }
    require_task(dispatcher.store(), &old.id)
}
