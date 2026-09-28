//! Changes to a task no worker has picked up yet, and requeueing one cancelled before it started.

use oga_domain::{HoldArgs, HoldVerb, OnBlockerFailure, Task, TaskHold, TaskKind, TaskState};
use oga_store::{StoreError, append_event};
use rusqlite::params;
use serde_json::{Map, Value, json};

use crate::{
    ContinuationError,
    dependencies::{self, DependencyBlocker, MAX_PREREQUISITES},
    dispatch::Dispatcher,
    holds,
    instruct::instruct_call,
    lifecycle::{self, now_iso},
    require_existing_worktree, require_profile, require_task, saved_instructions, validate_model,
    waiting,
};

const ADDED_INSTRUCTION_HEADING: &str = "## Added before this task started";

#[derive(Debug, Clone, Default)]
pub struct EditRequest {
    pub task_id: String,
    /// Replaces every prerequisite. `add_depends_on` and `remove_depends_on`
    /// apply on top of it.
    pub depends_on: Option<Vec<String>>,
    pub add_depends_on: Vec<String>,
    pub remove_depends_on: Vec<String>,
    pub on_blocker_failure: Option<OnBlockerFailure>,
    pub parent_task_id: Option<String>,
    pub timeout_ms: Option<u64>,
    pub effort: Option<String>,
    pub profile_id: Option<String>,
    pub model: Option<String>,
    pub instruction: Option<String>,
    pub requeue: bool,
}

impl EditRequest {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            ..Self::default()
        }
    }

    pub fn depends_on(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.depends_on = Some(ids.into_iter().map(Into::into).collect());
        self
    }

    pub fn add_depends_on(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.add_depends_on = ids.into_iter().map(Into::into).collect();
        self
    }

    pub fn remove_depends_on(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.remove_depends_on = ids.into_iter().map(Into::into).collect();
        self
    }

    pub fn on_blocker_failure(mut self, policy: OnBlockerFailure) -> Self {
        self.on_blocker_failure = Some(policy);
        self
    }

    pub fn instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instruction = Some(instruction.into());
        self
    }

    pub fn requeue(mut self) -> Self {
        self.requeue = true;
        self
    }

    fn changes_nothing(&self) -> bool {
        self.depends_on.is_none()
            && self.add_depends_on.is_empty()
            && self.remove_depends_on.is_empty()
            && self.on_blocker_failure.is_none()
            && self.parent_task_id.is_none()
            && self.timeout_ms.is_none()
            && self.effort.is_none()
            && self.profile_id.is_none()
            && self.model.is_none()
            && self.instruction.is_none()
    }
}

enum Wait {
    Start,
    Hold(Box<TaskHold>),
    StillBlocked,
}

pub async fn edit(
    dispatcher: &Dispatcher,
    request: EditRequest,
) -> Result<Task, ContinuationError> {
    let store = dispatcher.store();
    let old = require_task(store, &request.task_id)?;
    require_editable(&old, &request)?;
    require_existing_worktree(&old)?;

    let current_ids = dependencies::dependency_ids(store, &old.id)?;
    let prerequisite_ids = prerequisite_ids(&old.id, &current_ids, &request)?;
    let prerequisites = prerequisite_ids
        .iter()
        .map(|id| {
            lifecycle::load_task(store, id)?.ok_or_else(|| {
                ContinuationError::Refusal(format!("unknown prerequisite task: {id}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let policy = match request.on_blocker_failure {
        Some(policy) => policy,
        None => dependencies::blocker_policy(store, &old.id)?,
    };
    let (profile_id, model) = destination(dispatcher, &old, &request)?;
    let parent_task_id = match request.parent_task_id.as_deref().map(str::trim) {
        Some(parent) => Some(require_parent(dispatcher, &old.id, parent)?),
        None => old.parent_task_id.clone(),
    };
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
    let instruction = request
        .instruction
        .as_deref()
        .map(str::trim)
        .filter(|instruction| !instruction.is_empty());
    // A task cancelled before it started keeps what was saved on it in its brief.
    let saved = saved_instructions::waiting(store, &old.id)?;
    let added = saved_instructions::with_saved(instruction.map(str::to_owned), &saved);
    let prompt = match &added {
        Some(added) => format!(
            "{}\n\n{ADDED_INSTRUCTION_HEADING}\n\n{added}",
            old.prompt.trim_end()
        ),
        None => old.prompt.clone(),
    };

    let now = now_iso();
    let previous_hold = holds::get_hold(store, &old.id)?;
    let wait = plan_wait(&old, &prerequisites, policy, previous_hold.as_ref(), &now)?;
    let state = match &wait {
        Wait::Start => TaskState::Queued,
        Wait::Hold(_) => TaskState::Pending,
        Wait::StillBlocked => TaskState::Blocked,
    };
    let mut changes = Map::new();
    if prerequisite_ids != current_ids {
        changes.insert("dependsOn".into(), json!(prerequisite_ids));
    }
    if request.on_blocker_failure.is_some() {
        changes.insert("onBlockerFailure".into(), json!(policy));
    }
    if parent_task_id != old.parent_task_id {
        changes.insert("parent".into(), json!(parent_task_id));
    }
    if let Some(timeout_ms) = request.timeout_ms {
        changes.insert("timeoutMs".into(), json!(timeout_ms));
    }
    if effort != old.effort {
        changes.insert("effort".into(), json!(effort));
    }
    if profile_id != old.profile_id {
        changes.insert("fromProfile".into(), json!(old.profile_id));
        changes.insert("toProfile".into(), json!(profile_id));
    }
    if model != old.model {
        changes.insert("fromModel".into(), json!(old.model));
        changes.insert("model".into(), json!(model));
    }
    let requeued = old.state == TaskState::Cancelled;

    store.transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state=?,prompt=?,profile_id=?,model=?,effort=?,timeout_ms=COALESCE(?,timeout_ms),parent_task_id=?,on_blocker_failure=?,error=CASE WHEN ?='blocked' THEN error ELSE NULL END,completion_json=CASE WHEN ?='blocked' THEN completion_json ELSE NULL END,archived_at=CASE WHEN ? THEN NULL ELSE archived_at END,updated_at=? WHERE id=? AND state=? AND shipped_prompt IS NULL",
            params![
                state.as_str(),
                prompt,
                profile_id,
                model,
                effort,
                request.timeout_ms,
                parent_task_id,
                policy_column(policy),
                state.as_str(),
                state.as_str(),
                requeued,
                now,
                old.id,
                old.state.as_str(),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::Refusal(format!(
                "task changed while it was being edited; read it again and retry: {}",
                old.id
            )));
        }
        dependencies::replace_dependencies(tx, &old.id, &prerequisite_ids, &now)?;
        saved_instructions::mark_sent(tx, &old.id, state, saved.len(), &now)?;
        tx.execute("DELETE FROM task_holds WHERE task_id=?", [old.id.as_str()])?;
        if requeued {
            append_event(
                tx,
                &old.id,
                "requeued",
                state,
                &json!({"previousState": old.state}),
                &now,
                None,
            )?;
            if old.archived_at.is_some() {
                append_event(
                    tx,
                    &old.id,
                    "unarchived",
                    state,
                    &json!({"reason": "requeued"}),
                    &now,
                    None,
                )?;
            }
        }
        if !changes.is_empty() {
            append_event(tx, &old.id, "edited", state, &Value::Object(changes.clone()), &now, None)?;
        }
        if let Some(instruction) = instruction {
            append_event(
                tx,
                &old.id,
                "instruction_added",
                state,
                &json!({"instruction": instruction}),
                &now,
                None,
            )?;
        }
        match &wait {
            Wait::Hold(hold) => holds::insert_hold(tx, hold)?,
            Wait::Start => {
                append_event(tx, &old.id, "queued", TaskState::Queued, &json!({}), &now, None)?;
            }
            Wait::StillBlocked => {}
        }
        Ok(())
    })?;

    let task = require_task(store, &old.id)?;
    // Leaving `cancelled` or `blocked` lifts the reason its dependents were dropped.
    if matches!(old.state, TaskState::Cancelled | TaskState::Blocked) && state != TaskState::Blocked
    {
        dispatcher.restore_dropped_dependents(&task);
    }
    if state == TaskState::Queued {
        dispatcher.launch_waiting(task.clone());
    }
    Ok(task)
}

/// Only a task no worker has picked up can be edited: its brief has not been
/// sent, so every change still reaches the run.
fn require_editable(task: &Task, request: &EditRequest) -> Result<(), ContinuationError> {
    if task.kind == Some(TaskKind::Orchestrator) {
        return Err(ContinuationError::Refusal(format!(
            "unknown task: {}",
            task.id
        )));
    }
    if task.shipped_prompt.is_some() {
        return Err(ContinuationError::Refusal(format!(
            "task {} has already run, so its brief and wait are fixed; to give it an instruction in any state, call {}",
            task.id,
            instruct_call(&task.id, request.instruction.as_deref())
        )));
    }
    match task.state {
        TaskState::Pending | TaskState::Blocked => {}
        TaskState::Cancelled if request.requeue => {}
        TaskState::Cancelled => {
            return Err(ContinuationError::Refusal(format!(
                "task {} is cancelled; pass requeue to put it back to waiting with these changes",
                task.id
            )));
        }
        TaskState::Queued | TaskState::PreparingCheckout => {
            return Err(ContinuationError::Refusal(format!(
                "task {} is starting now, so it can no longer be edited; to give it an instruction, call {}",
                task.id,
                instruct_call(&task.id, request.instruction.as_deref())
            )));
        }
        state => {
            return Err(ContinuationError::Refusal(format!(
                "only a task that has not started can be edited; this one is {}: {}. To give it an instruction, call {}",
                state.as_str(),
                task.id,
                instruct_call(&task.id, request.instruction.as_deref())
            )));
        }
    }
    if request.changes_nothing() && !request.requeue {
        return Err(ContinuationError::Refusal(format!(
            "nothing to change: pass dependsOn, addDependsOn, removeDependsOn, onBlockerFailure, parent, timeoutMs, effort, profile, model, instruction, or requeue: {}",
            task.id
        )));
    }
    Ok(())
}

fn prerequisite_ids(
    task_id: &str,
    current: &[String],
    request: &EditRequest,
) -> Result<Vec<String>, ContinuationError> {
    let mut ids = Vec::new();
    for id in request
        .depends_on
        .as_deref()
        .unwrap_or(current)
        .iter()
        .chain(&request.add_depends_on)
    {
        let id = id.trim();
        if !id.is_empty() && !ids.iter().any(|kept| kept == id) {
            ids.push(id.to_owned());
        }
    }
    for id in &request.remove_depends_on {
        let id = id.trim();
        let Some(position) = ids.iter().position(|kept| kept == id) else {
            return Err(ContinuationError::Refusal(format!(
                "{id} is not a prerequisite of task {task_id}"
            )));
        };
        ids.remove(position);
    }
    if ids.iter().any(|id| id == task_id) {
        return Err(ContinuationError::Refusal(format!(
            "task cannot depend on itself: {task_id}"
        )));
    }
    if ids.len() > MAX_PREREQUISITES {
        return Err(ContinuationError::Refusal(format!(
            "too many prerequisites: {} > {MAX_PREREQUISITES}",
            ids.len()
        )));
    }
    Ok(ids)
}

/// A new account without a named model takes that account's default.
fn destination(
    dispatcher: &Dispatcher,
    task: &Task,
    request: &EditRequest,
) -> Result<(String, String), ContinuationError> {
    if request.profile_id.is_none() && request.model.is_none() {
        return Ok((task.profile_id.clone(), task.model.clone()));
    }
    let profile_id = request
        .profile_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&task.profile_id)
        .to_owned();
    let profile = require_profile(dispatcher.store(), &profile_id)?;
    if !profile.enabled {
        return Err(ContinuationError::Refusal(format!(
            "profile disabled: {profile_id}"
        )));
    }
    let model = match request.model.as_deref() {
        Some(model) => validate_model(model)?,
        None if profile_id == task.profile_id => task.model.clone(),
        None => profile.default_model.clone(),
    };
    Ok((profile_id, model))
}

fn require_parent(
    dispatcher: &Dispatcher,
    task_id: &str,
    parent_id: &str,
) -> Result<String, ContinuationError> {
    if parent_id == task_id {
        return Err(ContinuationError::Refusal(format!(
            "task cannot be its own parent: {task_id}"
        )));
    }
    let mut ancestor = Some(parent_id.to_owned());
    let mut depth = 0;
    while let Some(id) = ancestor {
        let task = require_task(dispatcher.store(), &id)
            .map_err(|_| ContinuationError::Refusal(format!("unknown parent task: {id}")))?;
        if id == parent_id && task.kind == Some(TaskKind::Orchestrator) {
            return Err(ContinuationError::Refusal(format!(
                "parent must name a delegated task: {parent_id}"
            )));
        }
        if id == task_id {
            return Err(ContinuationError::Refusal(format!(
                "parent {parent_id} is itself under task {task_id}"
            )));
        }
        depth += 1;
        if depth > dependencies::MAX_DEPENDENCY_DEPTH {
            break;
        }
        ancestor = task.parent_task_id;
    }
    Ok(parent_id.to_owned())
}

/// A new task's prerequisite rules, except an edit never lands a waiting task
/// `blocked`: that would drop everything waiting on it.
fn plan_wait(
    task: &Task,
    prerequisites: &[Task],
    policy: OnBlockerFailure,
    previous: Option<&TaskHold>,
    now: &str,
) -> Result<Wait, ContinuationError> {
    let blockers = prerequisites
        .iter()
        .map(DependencyBlocker::of)
        .collect::<Vec<_>>();
    if let Some(stopped) = prerequisites
        .iter()
        .zip(&blockers)
        .find(|(_, blocker)| blocker.stops_dependents(policy))
        .map(|(prerequisite, _)| prerequisite)
    {
        if task.state == TaskState::Blocked {
            return Ok(Wait::StillBlocked);
        }
        return Err(ContinuationError::Refusal(format!(
            "prerequisite {} is {}, so task {} could not start; requeue or resume {} first, or drop it with removeDependsOn",
            stopped.id,
            stopped.state.as_str(),
            task.id,
            stopped.id
        )));
    }
    let start_at = previous
        .and_then(|hold| hold.start_at.clone())
        .filter(|start| start.as_str() > now);
    if dependencies::prerequisites_met(policy, &blockers) {
        return Ok(match start_at {
            Some(start) => Wait::Hold(Box::new(scheduled_hold(&task.id, &start, previous, now))),
            None => Wait::Start,
        });
    }
    let waiting_on = prerequisites
        .iter()
        .filter(|prerequisite| prerequisite.state != TaskState::Completed)
        .cloned()
        .collect::<Vec<_>>();
    let mut hold = dependencies::dependency_hold(&task.id, &waiting_on, policy, None, now);
    if let Some(start) = start_at {
        hold.note = format!("{}; {}", hold.note, waiting::scheduled_start_note(&start));
        hold.next_check_at = start.clone();
        hold.start_at = Some(start);
    }
    Ok(Wait::Hold(Box::new(hold)))
}

/// A start time the caller chose survives an edit that clears every prerequisite.
fn scheduled_hold(task_id: &str, start: &str, previous: Option<&TaskHold>, now: &str) -> TaskHold {
    TaskHold {
        task_id: task_id.into(),
        verb: HoldVerb::Delegate,
        args: HoldArgs {
            scheduled: Some(true),
            ..HoldArgs::default()
        },
        start_at: Some(start.into()),
        await_profile: None,
        await_model: None,
        next_check_at: start.into(),
        expires_at: previous
            .map(|hold| hold.expires_at.clone())
            .unwrap_or_else(|| now.into()),
        probe_count: 0,
        note: waiting::scheduled_start_note(start),
        created_at: now.into(),
        updated_at: now.into(),
    }
}

fn policy_column(policy: OnBlockerFailure) -> &'static str {
    match policy {
        OnBlockerFailure::Hold => "hold",
        OnBlockerFailure::Run => "run",
    }
}
