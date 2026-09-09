//! `/health` response.

use axum::{extract::State, response::IntoResponse};
use oga_domain::HealthReport;

use crate::router::HttpState;

pub async fn get(State(state): State<HttpState>) -> impl IntoResponse {
    axum::Json(HealthReport::ok(state.build, &state.staleness))
}
