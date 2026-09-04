//! Live instruction and model-switch validation.

use oga_domain::{Task, TaskControlState, TaskState};
use serde_json::json;

use crate::{
    ContinuationError, append_event_tx, dispatch::Dispatcher, lifecycle::now_iso, require_task,
    validate_model,
};

#[derive(Debug, Clone, Default)]
pub struct SteerRequest {
    pub task_id: String,
    pub instruction: Option<String>,
    pub model: Option<String>,
}

impl SteerRequest {
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

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

pub fn control_state(dispatcher: &Dispatcher, task: &Task) -> TaskControlState {
    if task.state != TaskState::Running {
        return TaskControlState {
            steerable: false,
            reason: Some(format!(
                "only a running task takes an instruction; this one is {}",
                task.state.as_str()
            )),
        };
    }
    if dispatcher.active_runs().get(&task.id).is_none() {
        return TaskControlState {
            steerable: false,
            reason: Some("this run has no open channel to the worker".into()),
        };
    }
    TaskControlState {
        steerable: false,
        reason: Some("this runner has no live stdin control channel".into()),
    }
}

pub async fn steer(
    dispatcher: &Dispatcher,
    request: SteerRequest,
) -> Result<Task, ContinuationError> {
    let task = require_task(dispatcher.store(), &request.task_id)?;
    let instruction = request
        .instruction
        .as_deref()
        .map(str::trim)
        .filter(|instruction| !instruction.is_empty())
        .map(str::to_owned);
    let model = request.model.as_deref().map(validate_model).transpose()?;
    if instruction.is_none() && model.is_none() {
        return Err(ContinuationError::Refusal(format!(
            "pass an instruction, a model, or both: {}",
            task.id
        )));
    }
    if task.state != TaskState::Running {
        record_rejection(
            dispatcher,
            &task,
            instruction.as_deref(),
            &format!(
                "only a running task takes an instruction; this one is {}. Resume it instead: {}",
                task.state.as_str(),
                task.id
            ),
        )?;
        return Err(ContinuationError::Refusal(format!(
            "only a running task takes an instruction; this one is {}. Resume it instead: {}",
            task.state.as_str(),
            task.id
        )));
    }
    let state = control_state(dispatcher, &task);
    if !state.steerable {
        let reason = state
            .reason
            .unwrap_or_else(|| "this run cannot be steered".into());
        record_rejection(dispatcher, &task, instruction.as_deref(), &reason)?;
        return Err(ContinuationError::Refusal(format!(
            "{reason}; wait for the task to finish and resume it, or use handoff to move it now: {}",
            task.id
        )));
    }
    Err(ContinuationError::Refusal(format!(
        "this run cannot be steered: {}",
        task.id
    )))
}

fn record_rejection(
    dispatcher: &Dispatcher,
    task: &Task,
    instruction: Option<&str>,
    reason: &str,
) -> Result<(), ContinuationError> {
    let Some(instruction) = instruction else {
        return Ok(());
    };
    let now = now_iso();
    dispatcher.store().transaction(|tx| {
        append_event_tx(
            tx,
            &task.id,
            "steer_rejected",
            task.state,
            json!({"instruction": instruction, "reason": reason}),
            &now,
        )?;
        Ok(())
    })?;
    Ok(())
}
