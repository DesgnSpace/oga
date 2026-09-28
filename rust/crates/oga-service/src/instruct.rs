//! One instruction for a task in any state, routed to edit, steer, the
//! follow-up queue, or the task's saved instructions.

use oga_domain::{Task, TaskKind, TaskState};
use serde::Serialize;
use serde_json::json;

use crate::{
    ContinuationError, dispatch::Dispatcher, edit::EditRequest, follow_ups::queue_follow_up,
    lifecycle::now_iso, require_task, resume::ResumeRequest, saved_instructions,
    steer::SteerRequest,
};

#[derive(Debug, Clone, Default)]
pub struct InstructRequest {
    pub task_id: String,
    pub instruction: String,
    /// A stopped task runs with the instruction now instead of keeping it.
    pub now: bool,
}

impl InstructRequest {
    pub fn new(task_id: impl Into<String>, instruction: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            instruction: instruction.into(),
            now: false,
        }
    }

    pub fn now(mut self) -> Self {
        self.now = true;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Instructed {
    /// Added to the brief of a task that has not started.
    Appended,
    /// Handed to the running worker mid-run.
    Delivered,
    /// Waits behind the current or starting run and runs after it finishes clean.
    Queued,
    /// Kept on a stopped task; its next run carries it.
    Saved,
    /// A stopped task started again with it.
    Resumed,
}

impl Instructed {
    pub fn note(self) -> &'static str {
        match self {
            Self::Appended => "Added to its brief; the worker starts from it.",
            Self::Delivered => "The worker has it and is carrying on with the same run.",
            Self::Queued => "Queued as a follow-up. It runs after the current run finishes clean.",
            Self::Saved => "Saved for its next run; nothing started. Its next resume carries it.",
            Self::Resumed => "Started again with the instruction.",
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstructOutcome {
    pub task: Task,
    pub instructed: Instructed,
}

pub async fn instruct(
    dispatcher: &Dispatcher,
    request: InstructRequest,
) -> Result<InstructOutcome, ContinuationError> {
    let task = require_task(dispatcher.store(), &request.task_id)?;
    if task.kind == Some(TaskKind::Orchestrator) {
        return Err(ContinuationError::Refusal(format!(
            "unknown task: {}",
            task.id
        )));
    }
    let instruction = request.instruction.trim();
    if instruction.is_empty() {
        return Err(ContinuationError::Refusal(format!(
            "an instruction is required: {}",
            task.id
        )));
    }
    let never_started = task.shipped_prompt.is_none();
    let (task, instructed) = match task.state {
        TaskState::Pending | TaskState::Blocked if never_started => {
            let task = dispatcher
                .edit(EditRequest::new(&task.id).instruction(instruction))
                .await?;
            (task, Instructed::Appended)
        }
        TaskState::Running => {
            let outcome = dispatcher
                .steer(SteerRequest::new(&task.id).instruction(instruction))
                .await?;
            let instructed = if outcome.queued {
                Instructed::Queued
            } else {
                Instructed::Delivered
            };
            (outcome.task, instructed)
        }
        TaskState::Queued | TaskState::PreparingCheckout | TaskState::Answered => {
            queue_follow_up(
                dispatcher.store(),
                &task.id,
                task.state,
                instruction,
                None,
                &now_iso(),
            )?;
            (
                require_task(dispatcher.store(), &task.id)?,
                Instructed::Queued,
            )
        }
        TaskState::NeedsInput => {
            return Err(ContinuationError::Refusal(needs_input_refusal(&task.id)));
        }
        TaskState::Pending
        | TaskState::Blocked
        | TaskState::Completed
        | TaskState::Failed
        | TaskState::Cancelled
        | TaskState::RemovingCheckout => {
            if request.now {
                let task = dispatcher
                    .resume(ResumeRequest::new(&task.id).instruction(instruction))
                    .await?;
                (task, Instructed::Resumed)
            } else {
                saved_instructions::save(
                    dispatcher.store(),
                    &task.id,
                    task.state,
                    instruction,
                    &now_iso(),
                )?;
                (
                    require_task(dispatcher.store(), &task.id)?,
                    Instructed::Saved,
                )
            }
        }
    };
    Ok(InstructOutcome { task, instructed })
}

/// The call that adds `instruction` to `task_id` from any state, for a refusal
/// to name in place of the one refused.
pub fn instruct_call(task_id: &str, instruction: Option<&str>) -> String {
    format!(
        "instruct {}",
        json!({"taskId": task_id, "instruction": instruction.unwrap_or("<instruction>")})
    )
}

fn needs_input_refusal(task_id: &str) -> String {
    format!(
        "task {task_id} is waiting on an answer to its question, so it takes no instruction until it has one; answer with reply {}, which can carry the instruction too",
        json!({"taskId": task_id, "answer": "<answer>"})
    )
}
