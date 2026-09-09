//! Profile CRUD routes backed by the store layer.

use std::collections::BTreeMap;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use oga_config::{default_model, mask_secret_env};
use oga_domain::{Profile, ProfileView, Provider};
use oga_routing::{format_rfc3339_ms, now_ms};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::router::{HttpError, HttpState, parse_json};

const MASKED_SECRET: &str = "••••••••";

pub async fn list_not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(
            "content-type",
            HeaderValue::from_static("text/plain;charset=utf-8"),
        )
        .body("Not found".into())
        .expect("valid not-found response")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileCreate {
    id: Option<String>,
    label: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    enabled: Option<bool>,
    env: Option<BTreeMap<String, Value>>,
    capabilities: Option<Vec<String>>,
    command: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfilePatch {
    enabled: Option<bool>,
    label: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    capabilities: Option<Vec<String>>,
    env: Option<BTreeMap<String, Value>>,
}

pub async fn create(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: ProfileCreate = parse_json(&body)?;
    let profile = normalize_create(body, &state)?;
    state
        .store
        .repositories()
        .profiles()
        .insert(&profile, &now_iso())?;
    Ok((StatusCode::CREATED, Json(public_profile(&profile))))
}

pub async fn update(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: ProfilePatch = parse_json(&body)?;
    let mut profile = state
        .store
        .repositories()
        .profiles()
        .get(&id)?
        .ok_or_else(|| HttpError::not_found("unknown profile"))?;
    let expected = profile.clone();

    apply_patch(&mut profile, body)?;
    if !state.store.repositories().profiles().update_if_unchanged(
        &id,
        &expected,
        &profile,
        &now_iso(),
    )? {
        return Err(HttpError::conflict(
            "profile changed outside Oga. Reload it before saving",
        ));
    }
    Ok(Json(public_profile(&profile)))
}

pub async fn remove(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, HttpError> {
    if !state
        .store
        .repositories()
        .profiles()
        .remove(&id, &now_iso())?
    {
        return Err(HttpError::not_found("unknown profile"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn public_profile(profile: &Profile) -> ProfileView {
    let mut view = ProfileView::from(profile);
    view.env = mask_secret_env(&profile.env);
    view
}

fn normalize_create(body: ProfileCreate, state: &HttpState) -> Result<Profile, HttpError> {
    let label = body
        .label
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| HttpError::bad_request("label is required"))?;
    let provider = parse_provider(body.provider.as_deref())?;
    let id = body
        .id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            let slug = slug(&label);
            if slug.is_empty() {
                Uuid::new_v4().simple().to_string()
            } else {
                slug
            }
        });
    let id = unique_id(state, id)?;
    let model = normalize_model(body.model, provider)?;

    Ok(Profile {
        id,
        label,
        provider,
        default_model: model,
        enabled: body.enabled.unwrap_or(true),
        env: normalize_env(body.env, None),
        capabilities: normalize_strings(body.capabilities.unwrap_or_default()),
        command: normalize_command(body.command)?,
    })
}

fn apply_patch(profile: &mut Profile, body: ProfilePatch) -> Result<(), HttpError> {
    if let Some(enabled) = body.enabled {
        profile.enabled = enabled;
    }
    if let Some(label) = body.label {
        let label = label.trim();
        if label.is_empty() {
            return Err(HttpError::bad_request("label is required"));
        }
        profile.label = label.to_owned();
    }
    let provider_changed = if let Some(provider) = body.provider {
        let provider = parse_provider(Some(&provider))?;
        let changed = profile.provider != provider;
        profile.provider = provider;
        changed
    } else {
        false
    };
    if let Some(model) = body.model {
        profile.default_model = normalize_model(Some(model), profile.provider)?;
    } else if provider_changed {
        profile.default_model = default_model(profile.provider).to_owned();
    }
    if let Some(capabilities) = body.capabilities {
        profile.capabilities = normalize_strings(capabilities);
    }
    if let Some(env) = body.env {
        profile.env = normalize_env(Some(env), Some(&profile.env));
    }
    Ok(())
}

fn normalize_model(model: Option<String>, provider: Provider) -> Result<String, HttpError> {
    let model = model.unwrap_or_default();
    if model.chars().count() > 200 {
        return Err(HttpError::bad_request(
            "model must be at most 200 characters",
        ));
    }
    let model = model.trim();
    Ok(if model.is_empty() {
        default_model(provider).to_owned()
    } else {
        model.to_owned()
    })
}

fn parse_provider(value: Option<&str>) -> Result<Provider, HttpError> {
    match value {
        Some("claude") => Ok(Provider::Claude),
        Some("codex") => Ok(Provider::Codex),
        Some("opencode") => Ok(Provider::OpenCode),
        Some("opencode-2") => Ok(Provider::OpenCode2),
        Some("antigravity") => Ok(Provider::Antigravity),
        Some("pi") => Ok(Provider::Pi),
        _ => Err(HttpError::bad_request("invalid provider")),
    }
}

fn normalize_env(
    values: Option<BTreeMap<String, Value>>,
    existing: Option<&BTreeMap<String, String>>,
) -> BTreeMap<String, String> {
    values
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.trim().to_owned();
            if key.is_empty() {
                return None;
            }
            let value = value_to_string(value);
            let value = if value == MASKED_SECRET {
                existing
                    .and_then(|env| env.get(&key))
                    .cloned()
                    .unwrap_or(value)
            } else {
                value
            };
            Some((key, value))
        })
        .collect()
}

fn value_to_string(value: Value) -> String {
    match value {
        Value::String(value) => value,
        value => value.to_string(),
    }
}

fn normalize_strings(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .collect()
}

fn normalize_command(command: Option<Vec<String>>) -> Result<Option<Vec<String>>, HttpError> {
    command
        .map(|values| {
            if values.is_empty() || values.iter().any(|value| value.trim().is_empty()) {
                return Err(HttpError::bad_request(
                    "command must be a non-empty array of strings",
                ));
            }
            Ok(values)
        })
        .transpose()
}

fn slug(label: &str) -> String {
    let mut value = String::new();
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            value.push(character.to_ascii_lowercase());
        } else if !value.ends_with('-') {
            value.push('-');
        }
    }
    value.trim_matches('-').to_owned()
}

fn unique_id(state: &HttpState, requested: String) -> Result<String, HttpError> {
    let profiles = state.store.repositories().profiles();
    if !profiles.exists(&requested)? {
        return Ok(requested);
    }
    loop {
        let candidate = format!("{}-{}", requested, Uuid::new_v4().simple());
        if !profiles.exists(&candidate)? {
            return Ok(candidate);
        }
    }
}

fn now_iso() -> String {
    format_rfc3339_ms(now_ms())
}
