//! Live instruction and model-switch validation.

use oga_domain::{Task, TaskControlState, TaskState};
use serde_json::json;

use crate::{
    ContinuationError, acp_run::Delivered, append_event_tx, dispatch::Dispatcher,
    follow_ups::queue_follow_up, lifecycle::now_iso, require_task, validate_model,
};

/// Why no instruction travels with a model change, whether or not the run
/// takes one at all.
const MODEL_IS_FIXED: &str = "a model cannot change while a run is under way";

/// What became of the instruction: delivered to a live run, or left waiting for
/// the current one to finish.
#[derive(Debug, Clone)]
pub struct SteerOutcome {
    pub task: Task,
    pub queued: bool,
}

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
    if task
        .transport
        .as_ref()
        .and_then(|transport| transport.steering)
        .is_none()
    {
        return TaskControlState {
            steerable: false,
            reason: Some("this worker only takes an instruction between runs".into()),
        };
    }
    TaskControlState {
        steerable: true,
        reason: None,
    }
}

pub async fn steer(
    dispatcher: &Dispatcher,
    request: SteerRequest,
) -> Result<SteerOutcome, ContinuationError> {
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
    let Some(instruction) = instruction else {
        let reason = state.reason.unwrap_or_else(|| MODEL_IS_FIXED.into());
        return Err(ContinuationError::Refusal(format!(
            "{reason}; use handoff to change the model now: {}",
            task.id
        )));
    };
    if model.is_some() {
        let reason = state.reason.unwrap_or_else(|| MODEL_IS_FIXED.into());
        record_rejection(dispatcher, &task, Some(&instruction), &reason)?;
        return Err(ContinuationError::Refusal(format!(
            "{reason}; send the instruction on its own, or use handoff to change the model now: {}",
            task.id
        )));
    }
    if state.steerable {
        return deliver_now(dispatcher, task, &instruction).await;
    }
    queue_for_later(dispatcher, task, &instruction, None)
}

/// Hands the instruction to the turn that is running. A worker that will not
/// take it keeps the behaviour of one that never could: the instruction waits
/// its turn, and the record says why it had to.
async fn deliver_now(
    dispatcher: &Dispatcher,
    task: Task,
    instruction: &str,
) -> Result<SteerOutcome, ContinuationError> {
    let Some(run) = dispatcher.active_runs().get(&task.id) else {
        return queue_for_later(dispatcher, task, instruction, None);
    };
    match run.steer(instruction).await {
        Delivered::InTurn => {
            record_delivery(dispatcher, &task, instruction)?;
            Ok(SteerOutcome {
                task,
                queued: false,
            })
        }
        Delivered::Missed(reason) => queue_for_later(dispatcher, task, instruction, Some(&reason)),
    }
}

/// The instruction could not reach the running turn, so it waits behind it
/// instead of costing the caller the work done so far. `reason` is what the
/// worker said when it was offered the instruction and did not take it.
fn queue_for_later(
    dispatcher: &Dispatcher,
    mut task: Task,
    instruction: &str,
    reason: Option<&str>,
) -> Result<SteerOutcome, ContinuationError> {
    let now = now_iso();
    let waiting = queue_follow_up(
        dispatcher.store(),
        &task.id,
        task.state,
        instruction,
        reason,
        &now,
    )?;
    task.queued_follow_ups = Some(waiting as u64);
    Ok(SteerOutcome { task, queued: true })
}

fn record_delivery(
    dispatcher: &Dispatcher,
    task: &Task,
    instruction: &str,
) -> Result<(), ContinuationError> {
    let now = now_iso();
    dispatcher.store().transaction(|tx| {
        append_event_tx(
            tx,
            &task.id,
            "steered",
            task.state,
            json!({"instruction": instruction}),
            &now,
        )?;
        Ok(())
    })?;
    Ok(())
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
