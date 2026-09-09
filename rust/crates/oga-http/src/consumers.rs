//! Durable consumer inbox, cursor, and acknowledgement routes.

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use oga_service::DeliveryService;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::router::{HttpError, HttpState, parse_json};

#[derive(Debug, Deserialize, Default)]
pub struct InboxQuery {
    channel: Option<String>,
    limit: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CursorBody {
    cursor: Option<Value>,
}

pub async fn inbox(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    Query(query): Query<InboxQuery>,
) -> Result<impl IntoResponse, HttpError> {
    validate_consumer_id(&id)?;
    let channel = channel(query.channel.as_deref())?;
    let limit = inbox_limit(query.limit.as_deref());
    let inbox = DeliveryService::new(state.store.clone()).read(&id, &channel, limit, None)?;
    let mut response = serde_json::Map::from_iter([
        ("consumerId".into(), json!(id)),
        ("channel".into(), json!(channel)),
        ("cursor".into(), json!(inbox.cursor.cursor)),
        ("updatedAt".into(), json!(inbox.cursor.updated_at)),
        ("deliveries".into(), json!(inbox.deliveries)),
    ]);
    if let Some(rebaselined) = inbox.rebaselined {
        response.insert("rebaselined".into(), json!(rebaselined));
    }
    Ok(Json(Value::Object(response)))
}

pub async fn advance_cursor(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    validate_consumer_id(&id)?;
    let body: CursorBody = parse_json(&body)?;
    let cursor = body
        .cursor
        .as_ref()
        .and_then(parse_cursor)
        .ok_or_else(|| HttpError::bad_request("cursor must be a non-negative integer"))?;
    Ok(Json(
        DeliveryService::new(state.store.clone()).advance(&id, cursor)?,
    ))
}

fn validate_consumer_id(id: &str) -> Result<(), HttpError> {
    if id.is_empty() || id.chars().count() > 200 || id.contains('/') {
        return Err(HttpError::bad_request("invalid consumer id"));
    }
    Ok(())
}

fn channel(value: Option<&str>) -> Result<String, HttpError> {
    let value = value.unwrap_or("app");
    if value.is_empty()
        || value.chars().count() > 64
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.:-".contains(character))
    {
        return Err(HttpError::bad_request("invalid delivery channel"));
    }
    Ok(value.to_owned())
}

fn inbox_limit(value: Option<&str>) -> i64 {
    let value = value
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value != 0)
        .unwrap_or(100);
    value.clamp(1, 100)
}

fn parse_cursor(value: &Value) -> Option<i64> {
    let value = match value {
        Value::Number(value) => value.as_u64(),
        Value::String(value) if value.chars().all(|character| character.is_ascii_digit()) => {
            value.parse::<u64>().ok()
        }
        _ => None,
    }?;
    (value <= 9_007_199_254_740_991 && value <= i64::MAX as u64).then_some(value as i64)
}
