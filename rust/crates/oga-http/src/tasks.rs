//! Task dispatch and mutation routes.

use std::{path::PathBuf, time::Duration};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use oga_domain::{OnBlockerFailure, Task, TaskKind, TaskScope, WorktreeOption};
use oga_service::{
    ArchiveRequest, CompletionAssertion, DispatchRequest, FollowUpQueue, HandoffRequest,
    ReplyRequest, ResumeRequest, SteerRequest, WorktreeRemoveRequest,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    router::{HttpError, HttpState, latest_event_id, parse_json, parse_optional_json},
    routing::{self, RouteInput},
    settings, state,
};

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DispatchBody {
    pub profile: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub cwd: Option<String>,
    pub parent: Option<String>,
    pub scope: Option<TaskScope>,
    pub allow_questions: Option<bool>,
    pub can_delegate: Option<bool>,
    pub effort: Option<String>,
    /// The kind of work, when the caller names it, in the same vocabulary
    /// `oga love --when` accepts. Wins over whatever the prompt reads like
    /// for love-rule matching.
    pub kind: Option<oga_domain::WorkKind>,
    pub tldr: Option<String>,
    pub title: Option<String>,
    pub timeout_ms: Option<u64>,
    pub worktree: Option<WorktreeOption>,
    pub depends_on: Option<Vec<String>>,
    pub on_blocker_failure: Option<OnBlockerFailure>,
    pub start_at: Option<String>,
    pub attachments: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveBody {
    archived: bool,
    #[serde(default)]
    delete_branch: bool,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ResumeBody {
    instruction: Option<String>,
    timeout_ms: Option<u64>,
    scope: Option<TaskScope>,
    allow_questions: Option<bool>,
    start_at: Option<String>,
    queue: Option<QueueAction>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum QueueAction {
    Add,
    Clear,
}

#[derive(Debug, Deserialize)]
struct ReplyBody {
    answer: Option<String>,
    scope: Option<TaskScope>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct SteerBody {
    instruction: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct HandoffBody {
    profile: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    scope: Option<TaskScope>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CompletionBody {
    asserted_by: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct CancelQuery {
    reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeQuery {
    delete_branch: Option<String>,
}

pub async fn dispatch(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body = parse_dispatch_body(&body)?;
    let task = dispatch_body(&state, body, TaskKind::Delegated, None).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(started_task(&state, &task, true)?),
    ))
}

pub(crate) async fn dispatch_body(
    state: &HttpState,
    body: DispatchBody,
    kind: TaskKind,
    orchestrator_id: Option<String>,
) -> Result<Task, HttpError> {
    let prompt = required_prompt(body.prompt)?;
    let cwd = required_cwd(body.cwd)?;
    let tldr = body.tldr.ok_or_else(|| missing_string_field("tldr"))?;
    if prompt.chars().count() > 64_000 {
        return Err(HttpError::bad_request("prompt exceeds 64000 characters"));
    }
    if let Some(model) = &body.model
        && (model.is_empty() || model.chars().count() > 200)
    {
        return Err(HttpError::bad_request(
            "model must be between 1 and 200 characters",
        ));
    }
    if body
        .timeout_ms
        .is_some_and(|timeout| !(1..=86_400_000).contains(&timeout))
    {
        return Err(HttpError::bad_request(
            "timeoutMs must be between 1 and 86400000",
        ));
    }
    if let Some(effort) = &body.effort {
        validate_effort(effort)?;
    }
    if let Some(depends_on) = &body.depends_on
        && (depends_on.len() > 16 || depends_on.iter().any(String::is_empty))
    {
        return Err(HttpError::bad_request(
            "dependsOn must contain at most 16 non-empty task ids",
        ));
    }
    if let Some(title) = &body.title
        && !(1..=60).contains(&title.chars().count())
    {
        return Err(HttpError::bad_request(
            "title must be between 1 and 60 characters",
        ));
    }
    if let Some(attachments) = &body.attachments
        && (attachments.len() > 20 || attachments.iter().any(String::is_empty))
    {
        return Err(HttpError::bad_request(
            "attachments must contain at most 20 non-empty paths",
        ));
    }
    if !(1..=200).contains(&tldr.chars().count()) {
        return Err(HttpError::bad_request(
            "tldr must be between 1 and 200 characters",
        ));
    }
    let profile = body.profile.filter(|profile| !profile.is_empty());
    let route = routing::plan(
        state,
        RouteInput {
            prompt: prompt.clone(),
            cwd: cwd.display().to_string(),
            profile,
            model: body.model,
            kind: body.kind,
            effort: body.effort,
            default_profile_shortcut: true,
        },
    )?;
    let workspace = cwd.display().to_string();
    let worker_prompt = settings::worker_prompt(&state.store, &workspace)?;
    let (scope, grant_id, remember_scope) = match body.scope {
        Some(scope) => (scope, None, true),
        None => {
            let grant = state
                .store
                .repositories()
                .grants()
                .for_cwd(&workspace)?
                .into_iter()
                .find(|grant| grant.profile_id == route.profile_id);
            if let Some(grant) = grant {
                state.store.repositories().grants().touch(
                    &grant.id,
                    &oga_routing::format_rfc3339_ms(oga_routing::now_ms()),
                )?;
                (grant.scope, Some(grant.id), false)
            } else {
                (default_scope(), None, false)
            }
        }
    };
    let mut request = DispatchRequest::new(route.profile_id, prompt, cwd);
    request.model = Some(route.model);
    request.scope = scope;
    request.grant_id = grant_id;
    request.remember_scope = remember_scope;
    request.allow_questions = body.allow_questions.unwrap_or(true);
    request.can_delegate = body.can_delegate.unwrap_or(false);
    request.timeout = body.timeout_ms.map(Duration::from_millis);
    request.parent_task_id = body.parent;
    request.orchestrator_id = orchestrator_id;
    request.kind = kind;
    request.effort = route.effort;
    request.tldr = Some(tldr);
    request.title = body.title;
    request.worktree = body.worktree;
    request.depends_on = body.depends_on.unwrap_or_default();
    request.on_blocker_failure = body.on_blocker_failure.unwrap_or(OnBlockerFailure::Hold);
    request.selection = Some(route.decision);
    request.worker_prompt = Some(worker_prompt);
    request.start_at = body.start_at;
    request.attachments = body.attachments.unwrap_or_default();
    Ok(state.dispatcher.dispatch(request).await?.task)
}

pub(crate) fn parse_dispatch_body(body: &[u8]) -> Result<DispatchBody, HttpError> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| HttpError::bad_request("invalid JSON body"))?;
    routing::reject_difficulty(&value)?;
    if let Some(prompt) = value.get("prompt")
        && !prompt.is_string()
    {
        return Err(HttpError::bad_request(format!(
            "prompt: Invalid input: expected string, received {}",
            json_type(prompt)
        )));
    }
    if !value.get("tldr").is_some_and(Value::is_string) {
        return Err(missing_string_field("tldr"));
    }
    serde_json::from_value(value).map_err(|_| HttpError::bad_request("invalid JSON body"))
}

/// The body schema is checked before any route-specific rule, so a request
/// missing a required string hears about that field rather than the next gate.
fn missing_string_field(field: &str) -> HttpError {
    HttpError::bad_request(format!(
        "{field}: Invalid input: expected string, received undefined"
    ))
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub async fn archive(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: ArchiveBody = parse_json(&body)?;
    let mut request = ArchiveRequest::new(id, body.archived);
    if body.delete_branch {
        request = request.delete_branch();
    }
    let result = state.dispatcher.archive(request).await?;
    let mut response = response_view(&state, &result.task, false)?;
    if let Some(object) = response.as_object_mut() {
        if result.stopped {
            object.insert("stopped".into(), json!(true));
        }
        if let Some(checkout) = result.checkout {
            object.insert("checkout".into(), json!(checkout));
        }
        if let Some(branch) = result.branch {
            object.insert("branchOutcome".into(), json!(branch));
        }
        if let Some(reason) = result.branch_reason {
            object.insert("branchReason".into(), json!(reason));
        }
    }
    Ok(Json(response))
}

pub async fn cancel(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    Query(query): Query<CancelQuery>,
) -> Result<impl IntoResponse, HttpError> {
    if state::load_task(&state.store, &id)?
        .is_some_and(|task| task.kind == Some(TaskKind::Orchestrator))
    {
        return Err(HttpError::bad_request(format!("unknown task: {id}")));
    }
    let mut request = oga_service::CancelRequest::new(id);
    if let Some(reason) = query.reason {
        request = request.reason(reason);
    }
    let task = state.dispatcher.cancel(request).await?;
    Ok(Json(response_view(&state, &task, false)?))
}

pub async fn resume(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: ResumeBody = parse_optional_json(&body)?;
    let current = state.dispatcher.task(&id)?;
    if matches!(body.queue, Some(QueueAction::Clear)) {
        FollowUpQueue::new(state.store.clone()).clear(&id, current.state, "removed on request")?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(started_task(&state, &state.dispatcher.task(&id)?, false)?),
        ));
    }
    if matches!(body.queue, Some(QueueAction::Add))
        && matches!(
            current.state,
            oga_domain::TaskState::Queued
                | oga_domain::TaskState::Pending
                | oga_domain::TaskState::Running
                | oga_domain::TaskState::Answered
        )
    {
        let instruction = body
            .instruction
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                HttpError::bad_request(format!(
                    "a queued follow-up needs an instruction: {}",
                    current.id
                ))
            })?;
        if body.timeout_ms.is_some()
            || body.scope.is_some()
            || body.allow_questions.is_some()
            || body.start_at.is_some()
        {
            return Err(HttpError::bad_request(format!(
                "a queued follow-up takes only an instruction: {}",
                current.id
            )));
        }
        FollowUpQueue::new(state.store.clone()).queue(&id, current.state, instruction)?;
        return Ok((
            StatusCode::ACCEPTED,
            Json(started_task(&state, &state.dispatcher.task(&id)?, false)?),
        ));
    }
    let mut request = ResumeRequest::new(id);
    if let Some(instruction) = body.instruction {
        request = request.instruction(instruction);
    }
    if let Some(scope) = body.scope {
        request = request.scope(scope);
    }
    if let Some(timeout_ms) = body.timeout_ms {
        request = request.timeout_ms(timeout_ms);
    }
    if let Some(allow_questions) = body.allow_questions {
        request = request.allow_questions(allow_questions);
    }
    if let Some(start_at) = body.start_at {
        request = request.start_at(start_at);
    }
    let task = state.dispatcher.resume(request).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(started_task(&state, &task, false)?),
    ))
}

pub async fn reply(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: ReplyBody = parse_json(&body)?;
    let answer = body
        .answer
        .filter(|answer| !answer.is_empty())
        .ok_or_else(|| HttpError::bad_request("answer is required"))?;
    let mut request = ReplyRequest::new(id, answer);
    if let Some(scope) = body.scope {
        request = request.scope(scope);
    }
    let task = state.dispatcher.reply(request).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(started_task(&state, &task, false)?),
    ))
}

pub async fn remove_follow_up(
    State(state): State<HttpState>,
    Path((id, index)): Path<(String, String)>,
) -> Result<impl IntoResponse, HttpError> {
    let index = index
        .parse::<usize>()
        .map_err(|_| HttpError::bad_request("invalid follow-up index"))?;
    let task =
        state::load_task(&state.store, &id)?.ok_or_else(|| HttpError::not_found("unknown task"))?;
    let removed = FollowUpQueue::new(state.store.clone()).remove_at(&id, task.state, index)?;
    if !removed {
        return Err(HttpError::not_found("follow-up not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn steer(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: SteerBody = parse_optional_json(&body)?;
    let task = steer_task(&state, id, body).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(response_view(&state, &task, false)?),
    ))
}

pub(crate) async fn steer_task(
    state: &HttpState,
    id: String,
    body: SteerBody,
) -> Result<Task, HttpError> {
    let mut request = SteerRequest::new(id);
    if let Some(instruction) = body.instruction {
        request = request.instruction(instruction);
    }
    if let Some(model) = body.model {
        request = request.model(model);
    }
    Ok(state.dispatcher.steer(request).await?.task)
}

pub async fn handoff(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: HandoffBody = parse_json(&body)?;
    if let Some(profile) = &body.profile
        && profile.is_empty()
    {
        return Err(HttpError::bad_request("profile must not be empty"));
    }
    if let Some(model) = &body.model
        && (model.is_empty() || model.chars().count() > 200)
    {
        return Err(HttpError::bad_request(
            "model must be between 1 and 200 characters",
        ));
    }
    if let Some(effort) = &body.effort {
        validate_effort(effort)?;
    }
    let mut request = HandoffRequest::new(id);
    if let Some(profile) = body.profile {
        request = request.profile(profile);
    }
    if let Some(model) = body.model {
        request = request.model(model);
    }
    if let Some(effort) = body.effort {
        request = request.effort(effort);
    }
    if let Some(scope) = body.scope {
        request = request.scope(scope);
    }
    let task = state.dispatcher.handoff(request).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(started_task(&state, &task, true)?),
    ))
}

pub async fn remove_worktree(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    Query(query): Query<WorktreeQuery>,
) -> Result<impl IntoResponse, HttpError> {
    let request = WorktreeRemoveRequest {
        task_id: id,
        delete_branch: query.delete_branch.as_deref() == Some("true"),
    };
    Ok(Json(state.dispatcher.remove_worktree(request).await?))
}

pub async fn complete(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: CompletionBody = parse_optional_json(&body)?;
    let assertion = CompletionAssertion::new(
        id,
        body.asserted_by.unwrap_or_else(|| "Oga app".to_owned()),
        body.reason
            .unwrap_or_else(|| "marked completed from the sidebar".to_owned()),
    );
    let task = state.dispatcher.force_complete(assertion)?;
    Ok(Json(response_view(&state, &task, false)?))
}

pub(crate) fn response_view(
    state: &HttpState,
    task: &Task,
    routing: bool,
) -> Result<Value, HttpError> {
    let task = state::load_task(&state.store, &task.id)?
        .ok_or_else(|| HttpError::not_found("unknown task"))?;
    Ok(task_view(&task, routing))
}

pub(crate) fn task_view(task: &Task, routing: bool) -> Value {
    let mut view = Map::from_iter([
        ("id".into(), json!(task.id)),
        ("state".into(), json!(task.state)),
    ]);
    if !task.attempts.is_empty() {
        view.insert("attemptCount".into(), json!(task.attempts.len()));
    }
    if let Some(count) = task.queued_follow_ups.filter(|count| *count > 0) {
        view.insert("queuedFollowUps".into(), json!(count));
    }
    if let Some(archived_at) = &task.archived_at {
        view.insert("archivedAt".into(), json!(archived_at));
    }
    if let Some(hold) = &task.hold {
        view.insert("hold".into(), json!(hold));
    }
    if routing {
        view.insert("profileId".into(), json!(task.profile_id));
        view.insert("model".into(), json!(task.model));
        if let Some(effort) = &task.effort {
            view.insert("effort".into(), json!(effort));
        }
        if let Some(effort) = &task.effort_actual {
            view.insert("effortActual".into(), json!(effort));
        }
    }
    Value::Object(view)
}

pub(crate) fn started_task(
    state: &HttpState,
    task: &Task,
    routing: bool,
) -> Result<Value, HttpError> {
    let mut view = response_view(state, task, routing)?;
    let cursor = latest_event_id(&state.store, &task.id)?;
    view.as_object_mut()
        .expect("task view is an object")
        .insert("cursor".into(), json!(cursor));
    Ok(view)
}

fn required_prompt(prompt: Option<String>) -> Result<String, HttpError> {
    let prompt = prompt.ok_or_else(|| HttpError::bad_request("prompt is required"))?;
    if prompt.trim().is_empty() {
        return Err(HttpError::bad_request("prompt must not be empty"));
    }
    Ok(prompt)
}

fn required_cwd(cwd: Option<String>) -> Result<PathBuf, HttpError> {
    let cwd = cwd.ok_or_else(|| HttpError::bad_request("cwd is required"))?;
    let path = PathBuf::from(cwd);
    if !path.is_absolute() {
        return Err(HttpError::bad_request("cwd must be an absolute path"));
    }
    if !path.is_dir() {
        return Err(HttpError::bad_request("cwd does not exist"));
    }
    std::fs::canonicalize(path).map_err(|error| HttpError::bad_request(error.to_string()))
}

fn validate_effort(effort: &str) -> Result<(), HttpError> {
    if matches!(
        effort,
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
    ) {
        Ok(())
    } else {
        Err(HttpError::bad_request("invalid effort"))
    }
}

fn default_scope() -> TaskScope {
    TaskScope {
        read: vec!["**".into()],
        write: vec!["**".into()],
    }
}
