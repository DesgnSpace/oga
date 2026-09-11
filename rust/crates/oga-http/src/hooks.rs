//! Provider hook events attached to task history.

use std::collections::BTreeMap;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    response::IntoResponse,
};
use oga_domain::TaskEvent;
use oga_events::bound_event_payload;
use serde_json::{Value, json};

use crate::{
    router::{HttpError, HttpState, parse_json, run_blocking},
    state,
};

pub async fn append(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let payload: Value = parse_json(&body)?;
    if !payload.is_object() {
        return Err(HttpError::bad_request("hook payload must be an object"));
    }
    let payload = bound_event_payload(payload.as_object().expect("hook payload is an object"))
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let store = state.store.clone();
    run_blocking(move || {
        let task =
            state::load_task(&store, &id)?.ok_or_else(|| HttpError::not_found("unknown task"))?;
        store.repositories().events().append(&TaskEvent {
            id: 0,
            task_id: id,
            kind: "agent.hook".into(),
            state: task.state,
            payload,
            created_at: oga_routing::format_rfc3339_ms(oga_routing::now_ms()),
            turn_id: None,
        })?;
        Ok(())
    })
    .await?;
    Ok(Json(json!({})))
}
