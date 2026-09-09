//! The separate Oga orchestrator lane.

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use oga_domain::{Task, TaskKind};
use oga_service::CancelRequest;
use serde_json::json;

use crate::{
    router::{HttpError, HttpState, parse_optional_json},
    state,
    tasks::{self, SteerBody},
};

pub async fn dispatch(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body = tasks::parse_dispatch_body(&body)?;
    if body.profile.is_none() {
        return Err(HttpError::bad_request(
            "profile is required for an orchestrator",
        ));
    }
    let task = tasks::dispatch_body(&state, body, TaskKind::Orchestrator, None).await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

pub async fn dispatch_task(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    state::load_orchestrator(&state.store, &id)?;
    let body = tasks::parse_dispatch_body(&body)?;
    let task = tasks::dispatch_body(&state, body, TaskKind::Delegated, Some(id)).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(tasks::started_task(&state, &task, true)?),
    ))
}

pub async fn get(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, HttpError> {
    Ok(Json(state::load_orchestrator(&state.store, &id)?))
}

pub async fn stop(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, HttpError> {
    load_orchestrator_for_mutation(&state, &id)?;
    state.dispatcher.cancel(CancelRequest::new(id)).await?;
    Ok(Json(json!({ "stopped": true })))
}

pub async fn remove(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, HttpError> {
    let task = load_orchestrator_for_mutation(&state, &id)?;
    if !task.state.settled() {
        state
            .dispatcher
            .cancel(CancelRequest::new(id.clone()))
            .await?;
    }
    let changed = state
        .store
        .transaction(|tx| Ok(tx.execute("DELETE FROM tasks WHERE id=?", [id.as_str()])? != 0))?;
    if !changed {
        return Err(HttpError::not_found("unknown orchestrator"));
    }
    Ok(Json(json!({ "removed": true })))
}

pub async fn steer(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    load_orchestrator_for_mutation(&state, &id)?;
    let body: SteerBody = parse_optional_json(&body)?;
    let task = tasks::steer_task(&state, id, body).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(tasks::response_view(&state, &task, false)?),
    ))
}

/// Mutating the oga lane reports a missing orchestrator the way every other
/// task action reports a refusal: a 400 carrying the stringified error.
fn load_orchestrator_for_mutation(state: &HttpState, id: &str) -> Result<Task, HttpError> {
    state::load_orchestrator(&state.store, id).map_err(|error| {
        if error.status == StatusCode::NOT_FOUND {
            HttpError::bad_request(format!("Error: unknown orchestrator: {id}"))
        } else {
            error
        }
    })
}
