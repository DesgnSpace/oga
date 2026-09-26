//! Settings and discovery routes backed by the shared store/config crates.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    Json,
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use oga_config::{
    ConfigLayer, ConfigLayers, LoveRules, MASKED_SECRET, ModelOverrides, ResolvedModelSettings,
    config_revision, global_cwd, load_config_layers, model_enabled, model_override_for,
    read_model_overrides, read_model_settings,
};
use oga_domain::{
    AdvisorSettings, AppearanceSettings, CleanupSettings, CleanupSnapshot, MemoryEntry, ModelInfo,
    ModelInfoSource, ModelQuery as DomainModelQuery, ModelSettingsRow, Profile, ProfileUsage,
    Provider, UsageSource, UsageWindow, UsageWindowKind, WaitSettings,
};
use oga_pricing::catalogue as pricing_catalogue;
use oga_providers::{
    CURSOR_LOGIN, cannot_start, codex_home, environment_for, opencode_executable,
    opencode2_executable, unset_environment_for,
};
use oga_routing::{
    claude_models, claude_models_from_catalog, cursor_models, format_rfc3339_ms, now_ms,
    parse_antigravity_models, parse_codex_models, parse_fx_models, parse_opencode_models,
    parse_opencode_v2_models, parse_pi_models, select_model_rows, summarize_usage,
};
use oga_runner::worker_path::worker_path;
use oga_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

use crate::router::{HttpError, HttpState, run_blocking};

const MODEL_SETTINGS_KEY: &str = "models";
const CALLER_PROMPTS_KEY: &str = "callerPrompts";
const CLEANUP_KEY: &str = "cleanup";
const ADVISOR_KEY: &str = "advisor";
const APPEARANCE_KEY: &str = "appearance";
const MAX_FONT_ID_LEN: usize = 64;
const MIN_CLEANUP_DAYS: u64 = 1;
const MAX_CLEANUP_DAYS: u64 = 3_650;
const MAX_WAIT_MINUTES: u64 = 24 * 60;
const MAX_WAIT_ATTEMPTS: u32 = 100;
const MAX_MEMORY_VALUE: usize = 16_000;
const MAX_MEMORY_ENTRIES: u64 = 100;
const MAX_MEMORY_CHARS: u64 = 64_000;

#[derive(Debug, Deserialize)]
pub struct CwdQuery {
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ModelSettingsQuery {
    cwd: Option<String>,
    refresh: Option<bool>,
    /// Only the workers and models a task can move to.
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct MemoryWrite {
    cwd: String,
    key: String,
    value: String,
    #[serde(rename = "expectedVersion")]
    expected_version: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct PromptWrite {
    cwd: String,
    written: bool,
    value: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct ModelQuery {
    pub profile: Option<String>,
    pub provider: Option<String>,
    pub refresh: Option<bool>,
    pub cwd: Option<String>,
    #[serde(rename = "includeDisabled")]
    pub include_disabled: Option<bool>,
    #[serde(rename = "onlyPreferred")]
    pub only_preferred: Option<bool>,
    #[serde(rename = "onlyEnabled")]
    pub only_enabled: Option<bool>,
    pub query: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelSettingsWrite {
    cwd: String,
    #[serde(rename = "profileId")]
    profile_id: String,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    #[serde(rename = "expectedRevision")]
    expected_revision: Option<String>,
    enabled: Option<Option<bool>>,
    preferred: Option<Option<bool>>,
    capabilities: Option<Option<Vec<String>>>,
}

#[derive(Debug, Deserialize)]
pub struct CleanupSettingsWrite {
    pub enabled: bool,
    #[serde(rename = "olderThanDays")]
    pub older_than_days: u64,
    #[serde(rename = "archivedOnly")]
    pub archived_only: bool,
}

#[derive(Debug, Deserialize)]
pub struct WaitSettingsWrite {
    #[serde(rename = "networkMaxWaitMinutes")]
    pub network_max_wait_minutes: Option<u64>,
    #[serde(rename = "networkMaxAttempts")]
    pub network_max_attempts: Option<u32>,
    #[serde(rename = "moveOnRateLimit")]
    pub move_on_rate_limit: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct RevisionQuery {
    revision: Option<String>,
    cwd: Option<String>,
}

#[derive(Debug, Serialize)]
struct PromptConfig {
    cwd: String,
    scope: String,
    written: bool,
    value: String,
    inherited: String,
    #[serde(rename = "configPath", skip_serializing_if = "Option::is_none")]
    config_path: Option<String>,
}

pub async fn get_memories(
    State(state): State<HttpState>,
    Query(query): Query<CwdQuery>,
) -> Result<impl IntoResponse, HttpError> {
    let cwd = require_cwd(query.cwd.as_deref())?;
    let memories =
        run_blocking(move || Ok(state.store.repositories().memories().list(&cwd)?)).await?;
    Ok(Json(json!({ "memories": memories })))
}

pub async fn put_memory(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: MemoryWrite = parse_json(&body)?;
    let cwd = require_cwd(Some(&body.cwd))?;
    let key = validate_memory_key(&body.key)?;
    let value = body.value.trim().to_owned();
    if value.is_empty() {
        return Err(HttpError::bad_request("memory value must not be empty"));
    }
    if value.chars().count() > MAX_MEMORY_VALUE {
        return Err(HttpError::bad_request(
            "memory value exceeds 16000 characters",
        ));
    }
    let saved = run_blocking(move || {
        let existing = state
            .store
            .repositories()
            .memories()
            .list(&cwd)?
            .into_iter()
            .find(|memory| memory.key == key);
        let entries = state.store.repositories().memories().list(&cwd)?;
        if existing.is_none() && entries.len() as u64 >= MAX_MEMORY_ENTRIES {
            return Err(HttpError::bad_request(
                "project memory limit is 100 entries",
            ));
        }
        let total = entries
            .iter()
            .map(|memory| memory.value.chars().count() as u64)
            .sum::<u64>()
            .saturating_sub(
                existing
                    .as_ref()
                    .map_or(0, |memory| memory.value.chars().count() as u64),
            )
            + value.chars().count() as u64;
        if total > MAX_MEMORY_CHARS {
            return Err(HttpError::bad_request(
                "project memory exceeds 64000 characters",
            ));
        }
        let now = now_iso();
        let entry = MemoryEntry {
            cwd,
            key,
            value,
            version: existing.as_ref().map_or(1, |memory| memory.version),
            created_at: existing
                .as_ref()
                .map_or_else(|| now.clone(), |memory| memory.created_at.clone()),
            updated_at: now,
        };
        Ok(state
            .store
            .repositories()
            .memories()
            .upsert(&entry, body.expected_version)?)
    })
    .await?;
    Ok(Json(serde_json::to_value(saved).unwrap()))
}

pub async fn get_caller_prompt(
    State(state): State<HttpState>,
    Query(query): Query<CwdQuery>,
) -> Result<impl IntoResponse, HttpError> {
    run_blocking(move || read_prompt(&state, query)).await
}

pub async fn put_caller_prompt(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    run_blocking(move || write_prompt(&state, &body)).await
}

pub async fn delete_caller_prompt(
    State(state): State<HttpState>,
    Query(query): Query<CwdQuery>,
) -> Result<impl IntoResponse, HttpError> {
    run_blocking(move || clear_prompt(&state, query)).await
}

fn read_prompt(state: &HttpState, query: CwdQuery) -> Result<Json<Value>, HttpError> {
    let cwd = requested_cwd(query.cwd.as_deref());
    Ok(Json(
        serde_json::to_value(prompt_config(&state.store, &cwd)?).unwrap(),
    ))
}

fn write_prompt(state: &HttpState, body: &Bytes) -> Result<Json<Value>, HttpError> {
    let body: PromptWrite = parse_json(body)?;
    let cwd = canonical_cwd(&body.cwd);
    let prompt = prompt_config(&state.store, &cwd)?;
    if let Some(path) = prompt.config_path {
        return Err(HttpError::bad_request(format!(
            "These instructions come from {path}. Edit them there."
        )));
    }
    let value = body.value.trim().to_owned();
    if value.chars().count() > 8_000 {
        return Err(HttpError::bad_request(
            "invalid prompt config at caller_prompt: must be at most 8000 characters".to_owned(),
        ));
    }
    let written = body.written && !value.is_empty() && value != prompt.inherited;
    let stored =
        json!({ "written": written, "value": if written { value } else { String::new() } });
    state.store.repositories().settings().put(
        &cwd,
        CALLER_PROMPTS_KEY,
        &stored.to_string(),
        &now_iso(),
    )?;
    Ok(Json(
        serde_json::to_value(prompt_config(&state.store, &cwd)?).unwrap(),
    ))
}

fn clear_prompt(state: &HttpState, query: CwdQuery) -> Result<Json<Value>, HttpError> {
    let cwd = requested_cwd(query.cwd.as_deref());
    state.store.repositories().settings().put(
        &cwd,
        CALLER_PROMPTS_KEY,
        &json!({ "written": false, "value": "" }).to_string(),
        &now_iso(),
    )?;
    Ok(Json(
        serde_json::to_value(prompt_config(&state.store, &cwd)?).unwrap(),
    ))
}

fn requested_cwd(cwd: Option<&str>) -> String {
    canonical_cwd(cwd.unwrap_or(&global_cwd().display().to_string()))
}

pub async fn get_projects(State(state): State<HttpState>) -> Result<impl IntoResponse, HttpError> {
    let global = global_cwd().display().to_string();
    let seen = run_blocking(move || {
        Ok(state.store.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT cwd FROM (SELECT COALESCE(origin_cwd,cwd) AS cwd,MAX(updated_at) AS seen FROM tasks GROUP BY COALESCE(origin_cwd,cwd) UNION ALL SELECT cwd,MAX(updated_at) AS seen FROM memories GROUP BY cwd UNION ALL SELECT cwd,MAX(updated_at) AS seen FROM context_index GROUP BY cwd) GROUP BY cwd ORDER BY MAX(seen) DESC,cwd",
            )?;
            Ok(statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })?)
    })
    .await?;
    let projects = seen
        .into_iter()
        .filter(|cwd| cwd != &global)
        .collect::<Vec<_>>();
    Ok(Json(json!({ "global": global, "projects": projects })))
}

pub async fn get_cleanup(State(state): State<HttpState>) -> Result<impl IntoResponse, HttpError> {
    let snapshot = run_blocking(move || cleanup_snapshot(&state.store)).await?;
    Ok(Json(serde_json::to_value(snapshot).unwrap()))
}

pub async fn put_cleanup(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: CleanupSettingsWrite = parse_json(&body)?;
    validate_cleanup_days(body.older_than_days)?;
    let settings = CleanupSettings {
        enabled: body.enabled,
        older_than_days: body.older_than_days,
        archived_only: body.archived_only,
    };
    let cwd = global_cwd().display().to_string();
    let snapshot = run_blocking(move || {
        state.store.repositories().settings().put(
            &cwd,
            CLEANUP_KEY,
            &serde_json::to_string(&settings).unwrap(),
            &now_iso(),
        )?;
        cleanup_snapshot(&state.store)
    })
    .await?;
    Ok(Json(serde_json::to_value(snapshot).unwrap()))
}

pub async fn get_waiting(State(state): State<HttpState>) -> Result<impl IntoResponse, HttpError> {
    let settings =
        run_blocking(move || Ok(oga_service::waiting::wait_settings(&state.store))).await?;
    Ok(Json(serde_json::to_value(settings).unwrap()))
}

pub async fn put_waiting(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: WaitSettingsWrite = parse_json(&body)?;
    let defaults = WaitSettings::default();
    let settings = WaitSettings {
        network_max_wait_minutes: body
            .network_max_wait_minutes
            .unwrap_or(defaults.network_max_wait_minutes),
        network_max_attempts: body
            .network_max_attempts
            .unwrap_or(defaults.network_max_attempts),
        move_on_rate_limit: body
            .move_on_rate_limit
            .unwrap_or(defaults.move_on_rate_limit),
    };
    if !(1..=MAX_WAIT_MINUTES).contains(&settings.network_max_wait_minutes) {
        return Err(HttpError::bad_request(format!(
            "networkMaxWaitMinutes must be a whole number from 1 to {MAX_WAIT_MINUTES}"
        )));
    }
    if !(1..=MAX_WAIT_ATTEMPTS).contains(&settings.network_max_attempts) {
        return Err(HttpError::bad_request(format!(
            "networkMaxAttempts must be a whole number from 1 to {MAX_WAIT_ATTEMPTS}"
        )));
    }
    let store = state.store.clone();
    run_blocking(move || {
        Ok(store.repositories().settings().put(
            &global_cwd().display().to_string(),
            oga_service::waiting::WAIT_SETTINGS_KEY,
            &serde_json::to_string(&settings).unwrap(),
            &now_iso(),
        )?)
    })
    .await?;
    get_waiting(State(state)).await
}

/// The advisor's own settings, with the key masked. Everything else here
/// reads a project's settings; this one is global, because the key signs in
/// to one account whichever project a task starts from.
pub async fn get_advisor(State(state): State<HttpState>) -> Result<Json<Value>, HttpError> {
    let settings = run_blocking(move || advisor_settings(&state.store)).await?;
    Ok(Json(json!({
        "enabled": settings.enabled,
        "apiKey": if settings.api_key.is_empty() { "" } else { MASKED_SECRET },
    })))
}

pub async fn put_advisor(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<Json<Value>, HttpError> {
    let body: AdvisorSettings = parse_json(&body)?;
    let store = state.store.clone();
    run_blocking(move || {
        let stored = advisor_settings(&store)?;
        let api_key = if body.api_key == MASKED_SECRET {
            stored.api_key
        } else {
            body.api_key.trim().to_owned()
        };
        if body.enabled && api_key.is_empty() {
            return Err(HttpError::bad_request(
                "Paste your TypeSafe key before turning this on.",
            ));
        }
        Ok(store.repositories().settings().put(
            &global_cwd().display().to_string(),
            ADVISOR_KEY,
            &serde_json::to_string(&AdvisorSettings {
                enabled: body.enabled,
                api_key,
            })
            .unwrap(),
            &now_iso(),
        )?)
    })
    .await?;
    get_advisor(State(state)).await
}

/// The stored advisor settings, or the defaults when nothing was written or
/// what was written no longer parses. Carries the key, so it never reaches a
/// response body unmasked.
pub fn advisor_settings(store: &Store) -> Result<AdvisorSettings, HttpError> {
    Ok(store
        .repositories()
        .settings()
        .get(&global_cwd().display().to_string(), ADVISOR_KEY)?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default())
}

/// Global: the look belongs to the app, not to a project.
pub async fn get_appearance(State(state): State<HttpState>) -> Result<Json<Value>, HttpError> {
    let settings = run_blocking(move || appearance_settings(&state.store)).await?;
    Ok(Json(json!({ "font": settings.font })))
}

/// Saves the font id as given. The web app owns the list of fonts, so an id
/// it does not know yet is kept, and only its shape is checked here.
pub async fn put_appearance(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<Json<Value>, HttpError> {
    let body: AppearanceSettings = parse_json(&body)?;
    if let Some(font) = &body.font {
        let well_formed = !font.is_empty()
            && font.len() <= MAX_FONT_ID_LEN
            && font
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !well_formed {
            return Err(HttpError::bad_request(format!(
                "font must be 1 to {MAX_FONT_ID_LEN} lowercase letters, digits, or dashes"
            )));
        }
    }
    let store = state.store.clone();
    run_blocking(move || {
        Ok(store.repositories().settings().put(
            &global_cwd().display().to_string(),
            APPEARANCE_KEY,
            &serde_json::to_string(&body).unwrap(),
            &now_iso(),
        )?)
    })
    .await?;
    get_appearance(State(state)).await
}

/// The stored appearance, or the defaults when nothing was written or what
/// was written no longer parses.
pub fn appearance_settings(store: &Store) -> Result<AppearanceSettings, HttpError> {
    Ok(store
        .repositories()
        .settings()
        .get(&global_cwd().display().to_string(), APPEARANCE_KEY)?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default())
}

pub async fn preview_cleanup(
    State(state): State<HttpState>,
) -> Result<impl IntoResponse, HttpError> {
    get_cleanup(State(state)).await
}

pub async fn run_cleanup(State(state): State<HttpState>) -> Result<impl IntoResponse, HttpError> {
    let finished_at = now_iso();
    let result = tokio::time::timeout(
        Duration::from_secs(15 * 60),
        tokio::task::spawn_blocking(move || {
            let settings = cleanup_settings(&state.store)?;
            let cutoff = format_rfc3339_ms(
                now_ms().saturating_sub((settings.older_than_days * 86_400_000) as i64),
            );
            Ok::<_, HttpError>(state.store.cleanup(
                &cutoff,
                settings.archived_only,
                &finished_at,
            )?)
        }),
    )
    .await
    .map_err(|_| HttpError::timeout("cleanup is taking too long; try again later"))?
    .map_err(|error| HttpError::internal(format!("cleanup task failed: {error}")))??;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

fn cleanup_snapshot(store: &Store) -> Result<CleanupSnapshot, HttpError> {
    let settings = cleanup_settings(store)?;
    let cutoff =
        format_rfc3339_ms(now_ms().saturating_sub((settings.older_than_days * 86_400_000) as i64));
    let plan = store.cleanup_plan(&cutoff, settings.archived_only)?;
    Ok(CleanupSnapshot { settings, plan })
}

fn cleanup_settings(store: &Store) -> Result<CleanupSettings, HttpError> {
    let cwd = global_cwd().display().to_string();
    let Some(raw) = store.repositories().settings().get(&cwd, CLEANUP_KEY)? else {
        return Ok(CleanupSettings::default());
    };
    let settings: CleanupSettings = serde_json::from_str(&raw)
        .map_err(|error| HttpError::bad_request(format!("invalid cleanup settings: {error}")))?;
    validate_cleanup_days(settings.older_than_days)?;
    Ok(settings)
}

fn validate_cleanup_days(days: u64) -> Result<(), HttpError> {
    if (MIN_CLEANUP_DAYS..=MAX_CLEANUP_DAYS).contains(&days) {
        Ok(())
    } else {
        Err(HttpError::bad_request(format!(
            "olderThanDays must be a whole number from {MIN_CLEANUP_DAYS} to {MAX_CLEANUP_DAYS}"
        )))
    }
}

pub async fn get_models(
    State(state): State<HttpState>,
    Query(query): Query<ModelQuery>,
) -> Result<impl IntoResponse, HttpError> {
    let profiles = state.store.repositories().profiles().list()?;
    let provider = query.provider.as_deref().map(parse_provider).transpose()?;
    if let Some(profile_id) = &query.profile
        && !profiles
            .iter()
            .any(|profile| profile.id == *profile_id && profile.enabled)
    {
        return Err(HttpError::bad_request(format!(
            "unknown or disabled profile: {profile_id}"
        )));
    }
    let cwd = query
        .cwd
        .as_deref()
        .map(canonical_cwd)
        .unwrap_or_else(|| global_cwd().display().to_string());
    let settings = resolved_model_settings(&state.store, &cwd)?;
    let models = discover_catalog(&profiles, query.refresh == Some(true))
        .await
        .into_iter()
        .filter(|model| {
            query
                .profile
                .as_deref()
                .is_none_or(|id| id == model.profile_id)
        })
        .filter(|model| provider.is_none_or(|value| value == model.provider))
        .filter(|model| {
            query.include_disabled == Some(true)
                || (model_enabled(&settings, &model.profile_id, &model.id)
                    && profiles
                        .iter()
                        .find(|profile| profile.id == model.profile_id)
                        .is_some_and(|profile| profile.enabled))
        })
        .filter(|model| {
            query
                .query
                .as_deref()
                .is_none_or(|needle| model.id.to_lowercase().contains(&needle.to_lowercase()))
        })
        .collect::<Vec<_>>();
    Ok(Json(serde_json::to_value(models).unwrap()))
}

pub async fn model_rows(
    store: &Store,
    query: &ModelQuery,
    include_usage: bool,
) -> Result<Vec<ModelSettingsRow>, HttpError> {
    let profiles = store.repositories().profiles().list()?;
    let provider = query.provider.as_deref().map(parse_provider).transpose()?;
    if let Some(profile_id) = &query.profile
        && !profiles
            .iter()
            .any(|profile| profile.id == *profile_id && profile.enabled)
    {
        return Err(HttpError::bad_request(format!(
            "unknown or disabled profile: {profile_id}"
        )));
    }
    let cwd = query
        .cwd
        .as_deref()
        .map(canonical_cwd)
        .unwrap_or_else(|| global_cwd().display().to_string());
    let settings = resolved_model_settings(store, &cwd)?;
    let profiles = profiles
        .into_iter()
        .filter(|profile| profile.enabled)
        .collect::<Vec<_>>();
    let models = discover_catalog(&profiles, query.refresh == Some(true)).await;
    let empty_overrides = Default::default();
    let checked = profiles.clone();
    let unavailable = tokio::task::spawn_blocking(move || {
        checked
            .iter()
            .filter_map(|profile| cannot_start(profile).map(|reason| (profile.id.clone(), reason)))
            .collect::<HashMap<_, _>>()
    })
    .await
    .map_err(|error| HttpError::internal(format!("checking workers failed: {error}")))?;
    let rows = select_model_rows(
        &models,
        settings.overrides.as_ref().unwrap_or(&empty_overrides),
        &settings,
        &DomainModelQuery {
            profile: query.profile.clone(),
            provider,
            refresh: query.refresh,
            cwd: Some(cwd),
            include_disabled: query.include_disabled,
            only_preferred: query.only_preferred,
            only_enabled: query.only_enabled,
            query: query.query.clone(),
        },
        &profiles,
    )
    .into_iter()
    .map(|mut row| {
        row.unavailable = unavailable.get(&row.profile).cloned();
        row
    })
    .collect::<Vec<_>>();
    if !include_usage || rows.is_empty() {
        return Ok(rows);
    }
    let needed = rows
        .iter()
        .map(|row| row.profile.as_str())
        .collect::<HashSet<_>>();
    let usage_profiles = profiles
        .into_iter()
        .filter(|profile| needed.contains(profile.id.as_str()))
        .collect::<Vec<_>>();
    let usage = usage_for_profiles(&usage_profiles, query.refresh == Some(true), store).await?;
    let usage_by_profile: HashMap<&str, &ProfileUsage> = usage
        .iter()
        .map(|row| (row.profile.as_str(), row))
        .collect();
    Ok(rows
        .into_iter()
        .map(|mut row| {
            if let Some(usage) = usage_by_profile.get(row.profile.as_str()) {
                row.usage = Some(summarize_usage(usage, &row.model));
            }
            row
        })
        .collect())
}

/// Reads and merges usage for the profiles a query selects — shared by the
/// `/api/usage` route and the `models` listing's per-row summaries so both
/// see the same cache and the same rate-limit merge.
pub async fn usage_rows(store: &Store, query: &ModelQuery) -> Result<Vec<ProfileUsage>, HttpError> {
    let profiles = store.repositories().profiles().list()?;
    let provider = query.provider.as_deref().map(parse_provider).transpose()?;
    let selected = profiles
        .into_iter()
        .filter(|profile| profile.enabled)
        .filter(|profile| query.profile.as_deref().is_none_or(|id| id == profile.id))
        .filter(|profile| provider.is_none_or(|value| value == profile.provider))
        .collect::<Vec<_>>();
    if let Some(profile_id) = &query.profile
        && !selected.iter().any(|profile| profile.id == *profile_id)
    {
        return Err(HttpError::bad_request(format!(
            "unknown or disabled profile: {profile_id}"
        )));
    }
    usage_for_profiles(&selected, query.refresh == Some(true), store).await
}

pub async fn get_usage(
    State(state): State<HttpState>,
    Query(query): Query<ModelQuery>,
) -> Result<impl IntoResponse, HttpError> {
    let rows = usage_rows(&state.store, &query).await?;
    Ok(Json(serde_json::to_value(rows).unwrap()))
}

pub async fn delete_grant(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
) -> Result<impl IntoResponse, HttpError> {
    let changed = run_blocking(move || {
        Ok(state.store.transaction(|tx| {
            Ok(tx.execute("DELETE FROM scope_grants WHERE id=?", [id.as_str()])? != 0)
        })?)
    })
    .await?;
    if !changed {
        return Err(HttpError::not_found("unknown grant"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_model_settings(
    State(state): State<HttpState>,
    Query(query): Query<ModelSettingsQuery>,
) -> Result<impl IntoResponse, HttpError> {
    let cwd = canonical_cwd(
        query
            .cwd
            .as_deref()
            .unwrap_or(&global_cwd().display().to_string()),
    );
    let mut view = model_settings_view(&state.store, &cwd, query.refresh == Some(true)).await?;
    if query.enabled == Some(true) {
        keep_enabled(&mut view);
    }
    Ok(Json(view))
}

fn keep_enabled(view: &mut Value) {
    let Some(workers) = view["workers"].as_array_mut() else {
        return;
    };
    workers.retain_mut(|worker| {
        if worker["enabled"] != true {
            return false;
        }
        if let Some(models) = worker["models"].as_array_mut() {
            models.retain(|model| model["enabled"] == true);
        }
        true
    });
}

pub async fn put_model_settings(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: ModelSettingsWrite = parse_json(&body)?;
    let cwd = canonical_cwd(&body.cwd);
    let store = state.store.clone();
    let written = cwd.clone();
    run_blocking(move || save_model_setting(&store, &written, body)).await?;
    Ok(Json(model_settings_view(&state.store, &cwd, false).await?))
}

fn save_model_setting(store: &Store, cwd: &str, body: ModelSettingsWrite) -> Result<(), HttpError> {
    let current = model_settings_raw(store, cwd)?;
    check_revision(body.expected_revision.as_deref(), &current)?;
    let profiles = store.repositories().profiles().list()?;
    if !profiles.iter().any(|profile| profile.id == body.profile_id) {
        return Err(HttpError::bad_request(format!(
            "unknown worker: {}",
            body.profile_id
        )));
    }
    let mut root = current.value;
    let profiles_value = root
        .as_object_mut()
        .ok_or_else(|| HttpError::bad_request("invalid model settings"))?
        .entry("profiles")
        .or_insert_with(|| json!({}));
    let profile = profiles_value
        .as_object_mut()
        .ok_or_else(|| HttpError::bad_request("invalid model settings"))?
        .entry(body.profile_id.clone())
        .or_insert_with(|| json!({}));
    let profile = profile
        .as_object_mut()
        .ok_or_else(|| HttpError::bad_request("invalid model settings"))?;
    let Some(model_id) = body.model_id.as_deref() else {
        return Err(HttpError::bad_request(
            "Use the Workers toggle to change a worker’s availability.",
        ));
    };
    if model_id.is_empty() {
        return Err(HttpError::bad_request("modelId must not be empty"));
    }
    if body.enabled.is_none() && body.preferred.is_none() && body.capabilities.is_none() {
        return Err(HttpError::bad_request("model change is empty"));
    }
    let models = profile.entry("models").or_insert_with(|| json!({}));
    let model = models
        .as_object_mut()
        .ok_or_else(|| HttpError::bad_request("invalid model settings"))?
        .entry(model_id)
        .or_insert_with(|| json!({}));
    let model = model
        .as_object_mut()
        .ok_or_else(|| HttpError::bad_request("invalid model settings"))?;
    update_optional(model, "enabled", body.enabled);
    update_optional(model, "preferred", body.preferred);
    if let Some(capabilities) = body.capabilities {
        match capabilities {
            Some(values) => {
                if values
                    .iter()
                    .any(|value| value.is_empty() || value.len() > 80)
                {
                    return Err(HttpError::bad_request("invalid capabilities"));
                }
                model.insert("capabilities".into(), json!(values));
            }
            None => {
                model.remove("capabilities");
            }
        }
    }
    if model.is_empty() {
        models
            .as_object_mut()
            .expect("model settings object")
            .remove(model_id);
    }
    store
        .repositories()
        .settings()
        .put(cwd, MODEL_SETTINGS_KEY, &root.to_string(), &now_iso())?;
    Ok(())
}

pub async fn delete_model_settings(
    State(state): State<HttpState>,
    Query(query): Query<RevisionQuery>,
) -> Result<impl IntoResponse, HttpError> {
    let cwd = canonical_cwd(
        query
            .cwd
            .as_deref()
            .unwrap_or(&global_cwd().display().to_string()),
    );
    let store = state.store.clone();
    let removed = cwd.clone();
    run_blocking(move || {
        let current = model_settings_raw(&store, &removed)?;
        check_revision(query.revision.as_deref(), &current)?;
        Ok(store
            .repositories()
            .settings()
            .remove(&removed, MODEL_SETTINGS_KEY)?)
    })
    .await?;
    Ok(Json(model_settings_view(&state.store, &cwd, false).await?))
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, HttpError> {
    serde_json::from_slice(body).map_err(|_| HttpError::bad_request("invalid JSON body"))
}

fn require_cwd(cwd: Option<&str>) -> Result<String, HttpError> {
    let cwd = cwd.ok_or_else(|| HttpError::bad_request("cwd is required"))?;
    let path = Path::new(cwd);
    if !path.is_absolute() {
        return Err(HttpError::bad_request("cwd must be an absolute path"));
    }
    if !path.is_dir() {
        return Err(HttpError::bad_request("cwd does not exist"));
    }
    Ok(canonical_cwd(cwd))
}

fn canonical_cwd(cwd: &str) -> String {
    oga_config::canonical_cwd(cwd).display().to_string()
}

fn validate_memory_key(key: &str) -> Result<String, HttpError> {
    let valid = !key.is_empty()
        && key.len() <= 100
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'/' | b'_' | b'-')
        })
        && key.as_bytes()[0].is_ascii_alphanumeric();
    if !valid {
        return Err(HttpError::bad_request(
            "memory key must be 1-100 lowercase letters, numbers, dots, slashes, underscores, or hyphens",
        ));
    }
    Ok(key.to_owned())
}

/// The brief rules one scope's text resolves through, highest first: the
/// `.oga.yaml` sitting in that directory, then what Settings saved for it,
/// then what it inherits. The file is read here rather than copied into the
/// store, so editing it changes the next dispatch.
fn prompt_config(store: &Store, cwd: &str) -> Result<PromptConfig, HttpError> {
    let global = canonical_cwd(&global_cwd().display().to_string());
    let layers = load_config_layers(Some(Path::new(cwd)))
        .map_err(|error| HttpError::bad_request(error.to_string()))?;
    let default_text = oga_config::DEFAULT_CALLER_PROMPT.to_owned();
    let own_layer = layers.project.as_ref();
    let read_file = |layer: Option<&ConfigLayer>| {
        oga_config::read_caller_prompt(layer)
            .map_err(|error| HttpError::bad_request(error.to_string()))
    };
    let from_file =
        read_file(own_layer)?.zip(own_layer.map(|layer| layer.path.display().to_string()));
    let own = read_json_setting(store, cwd, CALLER_PROMPTS_KEY)?;
    let own_value = own
        .as_ref()
        .and_then(|value| value.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let written = own
        .as_ref()
        .and_then(|value| value.get("written"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let inherited = if cwd == global {
        default_text.clone()
    } else {
        read_file(layers.user.as_ref())?
            .or(read_json_setting(store, &global, CALLER_PROMPTS_KEY)?
                .filter(|value| value.get("written").and_then(Value::as_bool) == Some(true))
                .and_then(|value| {
                    value
                        .get("value")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }))
            .unwrap_or(default_text)
    };
    let saved = if written {
        own_value
    } else {
        inherited.clone()
    };
    let (value, config_path) = match from_file {
        Some((prompt, path)) => (prompt, Some(path)),
        None => (saved, None),
    };
    Ok(PromptConfig {
        cwd: cwd.to_owned(),
        scope: if cwd == global { "global" } else { "project" }.into(),
        written,
        value,
        inherited,
        config_path,
    })
}

/// The brief rules as the calling agent reads them, with `{{project}}` filled
/// in. `{{default}}` renders as nothing so older saved rules do not leak it;
/// every other `{{name}}` stays visible, so a typo reads as a typo.
pub fn caller_prompt(store: &Store, cwd: &str) -> Result<String, HttpError> {
    let cwd = canonical_cwd(cwd);
    let value = prompt_config(store, &cwd)?.value;
    Ok(oga_service::render_template(
        &value,
        &[("default", String::new()), ("project", cwd)],
    ))
}

fn read_json_setting(store: &Store, cwd: &str, key: &str) -> Result<Option<Value>, HttpError> {
    let raw = store.repositories().settings().get(cwd, key)?;
    match raw {
        Some(value) => serde_json::from_str(&value)
            .map(Some)
            .map_err(|error| HttpError::bad_request(error.to_string())),
        None => Ok(None),
    }
}

fn parse_provider(value: &str) -> Result<Provider, HttpError> {
    match value {
        "claude" => Ok(Provider::Claude),
        "codex" => Ok(Provider::Codex),
        "opencode" => Ok(Provider::OpenCode),
        "opencode-2" => Ok(Provider::OpenCode2),
        "antigravity" => Ok(Provider::Antigravity),
        "pi" => Ok(Provider::Pi),
        "fx" => Ok(Provider::Fx),
        "cursor" => Ok(Provider::Cursor),
        _ => Err(HttpError::bad_request("invalid provider")),
    }
}

/// How long a discovered catalog is trusted before a provider is asked again.
const CATALOG_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
/// How long one provider CLI gets to answer before its catalog call is given up on.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(150);

type CatalogCache = HashMap<String, (Instant, Vec<ModelInfo>)>;

static CATALOG_CACHE: OnceLock<Mutex<CatalogCache>> = OnceLock::new();
static CATALOG_REFRESHES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn catalog_cache() -> &'static Mutex<CatalogCache> {
    CATALOG_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Re-reads, off the caller's path, only the catalogs that are missing or
/// older than `CATALOG_CACHE_TTL`.
fn refresh_catalog_in_background(profiles: &[Profile]) {
    let pending = CATALOG_REFRESHES.get_or_init(|| Mutex::new(HashSet::new()));
    let profiles = {
        let cache = catalog_cache().lock().expect("catalog cache poisoned");
        profiles
            .iter()
            .filter(|profile| profile.provider != Provider::Claude)
            .filter(|profile| {
                cache
                    .get(&profile.id)
                    .is_none_or(|(at, _)| at.elapsed() >= CATALOG_CACHE_TTL)
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let mut pending_profiles = pending.lock().expect("catalog refresh lock poisoned");
    let profiles = profiles
        .into_iter()
        .filter(|profile| pending_profiles.insert(profile.id.clone()))
        .collect::<Vec<_>>();
    drop(pending_profiles);
    if profiles.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let _ = discover_catalog(&profiles, false).await;
        let mut pending = CATALOG_REFRESHES
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .expect("catalog refresh lock poisoned");
        for profile in profiles {
            pending.remove(&profile.id);
        }
    });
}

fn fallback_model(profile: &Profile) -> ModelInfo {
    ModelInfo {
        id: profile.default_model.clone(),
        label: profile.default_model.clone(),
        provider: profile.provider,
        profile_id: profile.id.clone(),
        source: ModelInfoSource::Configured,
        cost: None,
        context_window: None,
        reasoning: None,
        efforts: None,
        default_effort: None,
        tool_call: None,
    }
}

/// The last catalog `discover_catalog` found for each profile, without
/// spawning anything new. For callers that cannot await a provider CLI, such
/// as route planning on the hot dispatch path; a profile never yet
/// discovered falls back to its configured model.
pub fn cached_catalog(profiles: &[Profile]) -> Vec<ModelInfo> {
    let cache = catalog_cache().lock().expect("catalog cache poisoned");
    let mut models = Vec::new();
    for profile in profiles {
        match cache.get(&profile.id) {
            Some((_, cached)) => models.extend(cached.clone()),
            None if profile.provider == Provider::Claude => models.extend(claude_models(profile)),
            None => models.push(fallback_model(profile)),
        }
    }
    models.sort_by(|left, right| {
        left.profile_id
            .cmp(&right.profile_id)
            .then_with(|| left.id.cmp(&right.id))
    });
    models
}

/// Every model every profile currently offers, including models nobody has
/// turned on yet — callers narrow that down with `select_model_rows`. Claude's
/// catalog comes from models.dev; every other provider is asked live, with a
/// per-profile cache so a settings screen opening twice in a row doesn't
/// re-spawn a CLI each time.
pub(crate) async fn discover_catalog(profiles: &[Profile], refresh: bool) -> Vec<ModelInfo> {
    let mut handles = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let profile = profile.clone();
        handles.push(tokio::spawn(async move {
            models_for_profile(&profile, refresh).await
        }));
    }
    let mut models = Vec::new();
    for handle in handles {
        models.extend(handle.await.unwrap_or_default());
    }
    models.sort_by(|left, right| {
        left.profile_id
            .cmp(&right.profile_id)
            .then_with(|| left.id.cmp(&right.id))
    });
    models
}

async fn models_for_profile(profile: &Profile, refresh: bool) -> Vec<ModelInfo> {
    if profile.provider == Provider::Claude {
        let models = pricing_catalogue()
            .await
            .map(|catalogue| {
                catalogue
                    .models_for_provider("anthropic")
                    .iter()
                    .map(|model| (model.id.clone(), model.name.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let models = claude_models_from_catalog(profile, models);
        catalog_cache()
            .lock()
            .expect("catalog cache poisoned")
            .insert(profile.id.clone(), (Instant::now(), models.clone()));
        return models;
    }
    if !refresh
        && let Some((at, cached)) = catalog_cache()
            .lock()
            .expect("catalog cache poisoned")
            .get(&profile.id)
        && at.elapsed() < CATALOG_CACHE_TTL
    {
        return cached.clone();
    }
    let discovered = discover(profile).await.unwrap_or_default();
    let models = if profile.provider == Provider::OpenCode {
        let cached = cached_opencode_models(profile).await;
        let models = merge_model_catalogs(discovered, cached);
        if models.is_empty() {
            vec![fallback_model(profile)]
        } else {
            models
        }
    } else if !discovered.is_empty() {
        discovered
    } else {
        vec![fallback_model(profile)]
    };
    catalog_cache()
        .lock()
        .expect("catalog cache poisoned")
        .insert(profile.id.clone(), (Instant::now(), models.clone()));
    models
}

/// Gives a probe the same account the worker would run under: the profile's
/// own variables, and the removal of the ones the provider has to run without.
fn apply_profile_environment(command: &mut Command, profile: &Profile) {
    command.envs(environment_for(profile));
    for key in unset_environment_for(profile) {
        command.env_remove(key);
    }
}

/// Spawns the provider's own model-listing command in a throwaway directory
/// and parses its output. A provider that fails, times out, or is not
/// understood yields no models, so the caller falls back to the profile's
/// configured model rather than showing nothing at all.
async fn discover(profile: &Profile) -> Result<Vec<ModelInfo>, ()> {
    if profile.provider == Provider::Cursor {
        return discover_cursor_models(profile).await;
    }
    let dir = tempfile::tempdir().map_err(|_| ())?;
    let cwd = dir.path().display().to_string();
    let argv: Vec<String> = match profile.provider {
        Provider::Codex => vec!["codex".into(), "debug".into(), "models".into()],
        Provider::Antigravity => vec!["agy".into(), "models".into()],
        Provider::Pi => vec!["pi".into(), "--list-models".into()],
        Provider::Fx => vec!["fx".into(), "models".into()],
        Provider::OpenCode2 => vec![
            opencode2_executable(profile),
            "api".into(),
            "GET".into(),
            "/api/model".into(),
            "--param".into(),
            format!("location[directory]={cwd}"),
        ],
        Provider::OpenCode => vec![
            opencode_executable(profile).map_err(|_| ())?,
            "models".into(),
            "--verbose".into(),
            "--refresh".into(),
        ],
        Provider::Claude => unreachable!("claude models come from models.dev"),
        Provider::Cursor => unreachable!("cursor models come from an ACP session"),
    };
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(&cwd)
        .env("PATH", worker_path())
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    apply_profile_environment(&mut command, profile);
    // OpenCode 2 exits before a pipe drains, cutting a long answer short, so
    // its catalogue is written to a file instead.
    let answer_file = dir.path().join("models.json");
    if profile.provider == Provider::OpenCode2 {
        let file = std::fs::File::create(&answer_file).map_err(|_| ())?;
        command.stdout(file);
    }
    // `output` would replace that file with a pipe.
    let output = timeout(PROVIDER_TIMEOUT, async {
        command.spawn()?.wait_with_output().await
    })
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    let stdout = if profile.provider == Provider::OpenCode2 {
        std::fs::read(&answer_file).map_err(|_| ())?
    } else {
        output.stdout
    };
    let raw = String::from_utf8_lossy(&stdout);
    let parsed = match profile.provider {
        Provider::Codex => parse_codex_models(&raw, profile).map_err(|_| ())?,
        Provider::Antigravity => parse_antigravity_models(&raw, profile),
        Provider::Pi => parse_pi_models(&raw, profile),
        Provider::Fx => parse_fx_models(&raw, profile),
        Provider::OpenCode2 => parse_opencode_v2_models(&raw, profile).map_err(|_| ())?,
        Provider::OpenCode => parse_opencode_models(&raw, profile),
        Provider::Claude => unreachable!("claude models come from models.dev"),
        Provider::Cursor => unreachable!("cursor models come from an ACP session"),
    };
    Ok(parsed)
}

/// The models a Cursor account can reach, read from a session of its own.
///
/// Cursor offers its catalogue only to an ACP session: the ids carry the
/// thinking, context, and speed choices a run makes, and `cursor-agent models`
/// lists neither those ids nor those choices. So this opens a session in a
/// throwaway directory, keeps what opening it answered, and drops the session
/// without ever prompting it.
async fn discover_cursor_models(profile: &Profile) -> Result<Vec<ModelInfo>, ()> {
    let dir = tempfile::tempdir().map_err(|_| ())?;
    let cwd = dir.path().display().to_string();
    let mut command = Command::new("cursor-agent");
    command
        .arg("acp")
        .current_dir(&cwd)
        .env("PATH", worker_path())
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    apply_profile_environment(&mut command, profile);
    let mut agent = command.spawn().map_err(|_| ())?;
    let mut stdin = agent.stdin.take().ok_or(())?;
    let mut stdout = BufReader::new(agent.stdout.take().ok_or(())?).lines();
    let opened = timeout(PROVIDER_TIMEOUT, async {
        let handshake = [
            (
                1,
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": {}}),
            ),
            (2, "authenticate", json!({"methodId": CURSOR_LOGIN})),
        ];
        for (id, method, params) in handshake {
            ask(&mut stdin, &mut stdout, id, method, params).await?;
        }
        ask(
            &mut stdin,
            &mut stdout,
            3,
            "session/new",
            json!({"cwd": cwd, "mcpServers": []}),
        )
        .await
    })
    .await
    .ok()
    .flatten()
    .ok_or(())?;
    Ok(cursor_models(&opened, profile))
}

/// Sends one request and answers with the agent's `result` for it, skipping
/// everything it says on the way. An error, or a close before the answer,
/// ends the read.
async fn ask(
    stdin: &mut ChildStdin,
    lines: &mut Lines<BufReader<ChildStdout>>,
    id: u64,
    method: &str,
    params: Value,
) -> Option<Value> {
    let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    stdin
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .ok()?;
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message["id"] == json!(id) {
            return message.get("result").cloned();
        }
    }
    None
}

async fn cached_opencode_models(profile: &Profile) -> Vec<ModelInfo> {
    let home = global_cwd();
    let cache_roots = [
        environment_for(profile).remove("XDG_CACHE_HOME"),
        std::env::var("XDG_CACHE_HOME").ok(),
        Some(home.join(".cache").display().to_string()),
        Some(home.join("Library/Caches").display().to_string()),
    ];
    let mut raw = None;
    for root in cache_roots.into_iter().flatten() {
        if let Ok(value) =
            tokio::fs::read_to_string(Path::new(&root).join("opencode/models.json")).await
        {
            raw = Some(value);
            break;
        }
    }
    let Some(raw) = raw else {
        return Vec::new();
    };

    let data_roots = [
        environment_for(profile).remove("XDG_DATA_HOME"),
        std::env::var("XDG_DATA_HOME").ok(),
        Some(home.join(".local/share").display().to_string()),
        Some(
            home.join("Library/Application Support")
                .display()
                .to_string(),
        ),
    ];
    let mut providers = HashSet::new();
    for root in data_roots.into_iter().flatten() {
        let Ok(auth) = tokio::fs::read_to_string(Path::new(&root).join("opencode/auth.json")).await
        else {
            continue;
        };
        let Some(auth) = serde_json::from_str::<Value>(&auth).ok() else {
            continue;
        };
        if let Some(values) = auth.as_object() {
            providers.extend(values.keys().cloned());
        }
        break;
    }
    providers.insert("opencode".into());
    if let Some((provider, _)) = profile.default_model.split_once('/') {
        providers.insert(provider.into());
    }
    parse_opencode_cache_models(&raw, profile, &providers)
}

fn merge_model_catalogs(primary: Vec<ModelInfo>, secondary: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let mut models = HashMap::with_capacity(primary.len() + secondary.len());
    for model in primary.into_iter().chain(secondary) {
        models.entry(model.id.clone()).or_insert(model);
    }
    let mut models: Vec<_> = models.into_values().collect();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models
}

fn parse_opencode_cache_models(
    raw: &str,
    profile: &Profile,
    providers: &HashSet<String>,
) -> Vec<ModelInfo> {
    let Ok(root) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(root) = root.as_object() else {
        return Vec::new();
    };
    let mut models = Vec::new();
    for (provider_id, provider) in root {
        if !providers.contains(provider_id) {
            continue;
        }
        let Some(entries) = provider.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (model_id, model) in entries {
            let id = format!("{provider_id}/{model_id}");
            let label = model
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or(&id)
                .to_owned();
            let efforts = model
                .get("reasoning_options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|option| option.get("type").and_then(Value::as_str) == Some("effort"))
                .and_then(|option| option.get("values"))
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .filter(|values| !values.is_empty());
            let cost = match (
                model.pointer("/cost/input").and_then(Value::as_f64),
                model.pointer("/cost/output").and_then(Value::as_f64),
            ) {
                (Some(input), Some(output)) => Some(oga_domain::ModelCost { input, output }),
                _ => None,
            };
            models.push(ModelInfo {
                id,
                label,
                provider: profile.provider,
                profile_id: profile.id.clone(),
                source: ModelInfoSource::Discovered,
                cost,
                context_window: model.pointer("/limit/context").and_then(Value::as_u64),
                reasoning: model.get("reasoning").and_then(Value::as_bool),
                efforts,
                default_effort: None,
                tool_call: model.get("tool_call").and_then(Value::as_bool),
            });
        }
    }
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models
}

fn model_settings_raw(store: &Store, cwd: &str) -> Result<RawModelSettings, HttpError> {
    let value = read_json_setting(store, cwd, MODEL_SETTINGS_KEY)?.unwrap_or_else(|| json!({}));
    let raw = value.to_string();
    Ok(RawModelSettings {
        revision: config_revision(&raw),
        value,
    })
}

struct RawModelSettings {
    revision: String,
    value: Value,
}

fn check_revision(expected: Option<&str>, current: &RawModelSettings) -> Result<(), HttpError> {
    if let Some(expected) = expected
        && expected != current.revision
    {
        return Err(HttpError::conflict(
            "This file changed outside Oga. Reload it before saving.",
        ));
    }
    Ok(())
}

pub(crate) fn resolved_model_settings(
    store: &Store,
    cwd: &str,
) -> Result<ResolvedModelSettings, HttpError> {
    let global = canonical_cwd(&global_cwd().display().to_string());
    let global_raw = read_json_setting(store, &global, MODEL_SETTINGS_KEY)?;
    let project_raw = if cwd == global {
        None
    } else {
        read_json_setting(store, cwd, MODEL_SETTINGS_KEY)?
    };
    let global_raw = global_raw.unwrap_or_else(|| json!({}));
    let layers = load_config_layers((cwd != global).then_some(Path::new(cwd)))
        .map_err(|error| HttpError::bad_request(error.to_string()))?;
    let (overrides, love) =
        read_model_overrides(&layers).map_err(|error| HttpError::bad_request(error.to_string()))?;
    Ok(ResolvedModelSettings {
        global: read_model_settings(&global_raw),
        project: project_raw.as_ref().map(read_model_settings),
        overrides: Some(overrides),
        love,
    })
}

/// Where a directory sends work that names no model, rule by rule.
pub fn love_rules(cwd: &str) -> Result<LoveRules, HttpError> {
    let global = canonical_cwd(&global_cwd().display().to_string());
    let layers = load_config_layers((cwd != global).then_some(Path::new(cwd)))
        .map_err(|error| HttpError::bad_request(error.to_string()))?;
    let (_, love) =
        read_model_overrides(&layers).map_err(|error| HttpError::bad_request(error.to_string()))?;
    Ok(love)
}

async fn model_settings_view(store: &Store, cwd: &str, refresh: bool) -> Result<Value, HttpError> {
    let profiles = store.repositories().profiles().list()?;
    let global = canonical_cwd(&global_cwd().display().to_string());
    let current = model_settings_raw(store, cwd)?;
    let global_raw =
        read_json_setting(store, &global, MODEL_SETTINGS_KEY)?.unwrap_or_else(|| json!({}));
    let project_raw = if cwd == global {
        None
    } else {
        read_json_setting(store, cwd, MODEL_SETTINGS_KEY)?
    };
    let global_settings = read_model_settings(&global_raw);
    let project_settings = project_raw.as_ref().map(read_model_settings);
    let layers = load_config_layers((cwd != global).then_some(Path::new(cwd)))
        .map_err(|error| HttpError::bad_request(error.to_string()))?;
    let overrides = model_override_scopes(&layers)?;
    let global_model_settings = ResolvedModelSettings {
        global: global_settings.clone(),
        project: None,
        overrides: Some(overrides.global.clone()),
        love: LoveRules::default(),
    };
    let model_settings = ResolvedModelSettings {
        global: global_settings.clone(),
        project: project_settings.clone(),
        overrides: Some(overrides.current.clone()),
        love: overrides.love.clone(),
    };
    let models = if refresh {
        discover_catalog(&profiles, true).await
    } else {
        let models = cached_catalog(&profiles);
        refresh_catalog_in_background(&profiles);
        models
    };
    let workers = profiles
        .iter()
        .map(|profile| {
            let global_profile = global_settings.profiles.get(&profile.id);
            let project_profile = project_settings
                .as_ref()
                .and_then(|settings| settings.profiles.get(&profile.id));
            let inherited_enabled = global_profile
                .and_then(|setting| setting.enabled)
                .unwrap_or(profile.enabled);
            let enabled = project_profile
                .and_then(|setting| setting.enabled)
                .unwrap_or(inherited_enabled);
            let model_rows = models
                .iter()
                .filter(|model| model.profile_id == profile.id)
                .map(|model| {
                    let project_override = overrides
                        .project
                        .as_ref()
                        .and_then(|overrides| {
                            model_override_for(overrides, &profile.id, &model.id)
                        });
                    let global_model = model_setting_value(&global_raw, &profile.id, &model.id);
                    let project_model = project_raw.as_ref().and_then(|raw| {
                        model_setting_value(raw, &profile.id, &model.id)
                    });
                    let inherited_enabled =
                        model_enabled(&global_model_settings, &profile.id, &model.id);
                    let enabled = model_enabled(&model_settings, &profile.id, &model.id);
                    let inherited_preferred = global_model
                        .as_ref()
                        .and_then(|setting| setting.preferred)
                        .unwrap_or(false);
                    let preferred = project_model
                        .as_ref()
                        .and_then(|setting| setting.preferred)
                        .unwrap_or(inherited_preferred);
                    let inherited_capabilities = global_model
                        .as_ref()
                        .and_then(|setting| setting.capabilities.clone())
                        .unwrap_or_else(|| model_capabilities(model));
                    let capabilities = project_model
                        .as_ref()
                        .and_then(|setting| setting.capabilities.clone())
                        .unwrap_or_else(|| inherited_capabilities.clone());
                    json!({
                        "id": model.id,
                        "label": model.label,
                        "contextWindow": model.context_window,
                        "enabled": enabled,
                        "inheritedEnabled": inherited_enabled,
                        "hasEnabledOverride": project_model.as_ref().is_some_and(|setting| setting.enabled.is_some())
                            || project_override.as_ref().is_some_and(|setting| setting.enabled.is_some()),
                        "preferred": preferred,
                        "inheritedPreferred": inherited_preferred,
                        "hasPreferredOverride": project_model.as_ref().is_some_and(|setting| setting.preferred.is_some()),
                        "capabilities": capabilities,
                        "inheritedCapabilities": inherited_capabilities,
                        "hasCapabilitiesOverride": project_model.as_ref().is_some_and(|setting| setting.capabilities.is_some()),
                        "loved": overrides.love.names_model(
                            &profile.id,
                            &model.id,
                            Some(profile.default_model.as_str())
                        ),
                        "availableGlobally": inherited_enabled,
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "id": profile.id,
                "label": profile.label,
                "provider": profile.provider,
                "enabled": enabled,
                "inheritedEnabled": inherited_enabled,
                "hasEnabledOverride": project_profile.is_some_and(|setting| setting.enabled.is_some()),
                "availableGlobally": global == cwd || inherited_enabled,
                "configured": profile.enabled,
                "models": model_rows,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "cwd": cwd,
        "scope": if global == cwd { "global" } else { "project" },
        "revision": current.revision,
        "workers": workers,
        "love": overrides.love,
    }))
}

struct ModelOverrideScopes {
    current: ModelOverrides,
    global: ModelOverrides,
    project: Option<ModelOverrides>,
    love: LoveRules,
}

fn model_override_scopes(layers: &ConfigLayers) -> Result<ModelOverrideScopes, HttpError> {
    let (current, love) =
        read_model_overrides(layers).map_err(|error| HttpError::bad_request(error.to_string()))?;
    let (global, _) = read_model_overrides(&ConfigLayers {
        user: layers.user.clone(),
        project: None,
    })
    .map_err(|error| HttpError::bad_request(error.to_string()))?;
    let project = layers
        .project
        .clone()
        .map(|project| {
            read_model_overrides(&ConfigLayers {
                user: None,
                project: Some(project),
            })
            .map(|(overrides, _)| overrides)
            .map_err(|error| HttpError::bad_request(error.to_string()))
        })
        .transpose()?;
    Ok(ModelOverrideScopes {
        current,
        global,
        project,
        love,
    })
}

#[derive(Clone, Default)]
struct ModelSetting {
    enabled: Option<bool>,
    preferred: Option<bool>,
    capabilities: Option<Vec<String>>,
}

fn model_setting_value(value: &Value, profile_id: &str, model_id: &str) -> Option<ModelSetting> {
    let profile = value.get("profiles")?.get(profile_id)?.as_object()?;
    let setting = profile
        .get("modelEnabled")
        .or_else(|| profile.get("models"))?
        .get(model_id)?;
    match setting {
        Value::Bool(enabled) => Some(ModelSetting {
            enabled: Some(*enabled),
            ..ModelSetting::default()
        }),
        Value::Object(setting) => Some(ModelSetting {
            enabled: setting.get("enabled").and_then(Value::as_bool),
            preferred: setting.get("preferred").and_then(Value::as_bool),
            capabilities: setting
                .get("capabilities")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                }),
        }),
        _ => None,
    }
}

fn model_capabilities(model: &ModelInfo) -> Vec<String> {
    let mut capabilities = Vec::new();
    if model.reasoning == Some(true) {
        capabilities.push("reasoning".into());
    }
    if model.context_window.is_some_and(|window| window >= 128_000) {
        capabilities.push("long-context".into());
    }
    if model.tool_call == Some(true) {
        capabilities.push("tool-use".into());
    }
    if model
        .cost
        .as_ref()
        .is_some_and(|cost| cost.input == 0.0 && cost.output == 0.0)
    {
        capabilities.push("free".into());
    }
    capabilities
}

fn update_optional<T: Serialize>(
    object: &mut Map<String, Value>,
    key: &str,
    value: Option<Option<T>>,
) {
    if let Some(value) = value {
        match value {
            Some(value) => {
                object.insert(key.into(), serde_json::to_value(value).unwrap());
            }
            None => {
                object.remove(key);
            }
        }
    }
}

/// How long a usage read counts as fresh. A stale read is still served at once
/// while a new one is fetched behind it, so only a profile's first read waits.
const USAGE_CACHE_TTL: Duration = Duration::from_secs(60);
/// How long a usage-reading command gets before its read is given up on.
const USAGE_TIMEOUT: Duration = Duration::from_secs(30);

struct CachedUsage {
    read_at: tokio::time::Instant,
    usage: ProfileUsage,
    /// A newer read is already on its way, so a stale hit starts no other.
    refreshing: bool,
}

static USAGE_CACHE: OnceLock<Mutex<HashMap<String, CachedUsage>>> = OnceLock::new();

fn usage_cache() -> &'static Mutex<HashMap<String, CachedUsage>> {
    USAGE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The last usage read on file for each profile, without spawning anything
/// new — for callers that cannot await a provider CLI, such as route
/// planning on the hot dispatch path. A profile never yet read reports
/// unknown rather than blocking.
pub fn cached_usage(profiles: &[Profile]) -> Vec<ProfileUsage> {
    let cache = usage_cache().lock().expect("usage cache poisoned");
    profiles
        .iter()
        .map(|profile| {
            cache
                .get(&profile.id)
                .map(|cached| cached.usage.clone())
                .unwrap_or_else(|| unsupported_usage(profile, "usage not read yet"))
        })
        .collect()
}

async fn usage_for_profiles(
    profiles: &[Profile],
    refresh: bool,
    store: &Store,
) -> Result<Vec<ProfileUsage>, HttpError> {
    let failures = failures_for_profiles(store)?;
    let mut handles = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let profile = profile.clone();
        handles.push(tokio::spawn(async move {
            usage_for_profile(&profile, refresh).await
        }));
    }
    let mut rows = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(usage) = handle.await {
            rows.push(merge_observed_failures(usage, &failures));
        }
    }
    Ok(rows)
}

async fn usage_for_profile(profile: &Profile, refresh: bool) -> ProfileUsage {
    if !refresh
        && let Some(cached) = usage_cache()
            .lock()
            .expect("usage cache poisoned")
            .get_mut(&profile.id)
    {
        if cached.read_at.elapsed() >= USAGE_CACHE_TTL && !cached.refreshing {
            cached.refreshing = true;
            let profile = profile.clone();
            tokio::spawn(async move { read_usage_into_cache(&profile).await });
        }
        return cached.usage.clone();
    }
    read_usage_into_cache(profile).await
}

async fn read_usage_into_cache(profile: &Profile) -> ProfileUsage {
    let usage = fetch_usage(profile).await;
    usage_cache().lock().expect("usage cache poisoned").insert(
        profile.id.clone(),
        CachedUsage {
            read_at: tokio::time::Instant::now(),
            usage: usage.clone(),
            refreshing: false,
        },
    );
    usage
}

/// Reads usage straight from each provider, with no failure history merged
/// in yet — that happens once, in `merge_observed_failures`, after the read
/// lands in cache.
async fn fetch_usage(profile: &Profile) -> ProfileUsage {
    match profile.provider {
        Provider::Claude => claude_usage(profile).await,
        Provider::Codex => codex_usage(profile).await,
        Provider::OpenCode => {
            unsupported_usage(profile, "usage tracking is not supported for opencode")
        }
        Provider::OpenCode2 => {
            unsupported_usage(profile, "usage tracking is not supported for opencode-2")
        }
        Provider::Antigravity => {
            unsupported_usage(profile, "no usage source known for antigravity")
        }
        Provider::Pi => unsupported_usage(profile, "usage tracking is not supported for pi"),
        Provider::Cursor => {
            unsupported_usage(profile, "usage tracking is not supported for cursor")
        }
        Provider::Fx => unsupported_usage(profile, "usage tracking is not supported for fx"),
    }
}

fn unsupported_usage(profile: &Profile, reason: &str) -> ProfileUsage {
    ProfileUsage {
        profile: profile.id.clone(),
        provider: profile.provider,
        supported: false,
        source: UsageSource::None,
        windows: Vec::new(),
        plan: None,
        observed_at: None,
        reason: Some(reason.to_owned()),
        account_failure: None,
        rate_limits_by_model: None,
        observed_rate_limits: None,
    }
}

/// `claude -p "/usage" --output-format json` runs as a local command: zero
/// API turns, zero cost.
async fn claude_usage(profile: &Profile) -> ProfileUsage {
    claude_usage_with_program(profile, "claude").await
}

async fn claude_usage_with_program(profile: &Profile, program: &str) -> ProfileUsage {
    let Ok(dir) = tempfile::tempdir() else {
        return unsupported_usage(
            profile,
            "claude /usage failed: could not create scratch dir",
        );
    };
    let mut command = Command::new(program);
    command
        .args(["-p", "/usage", "--output-format", "json"])
        .current_dir(dir.path())
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    apply_profile_environment(&mut command, profile);
    let output = match timeout(USAGE_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return unsupported_usage(profile, &format!("claude /usage failed: {error}"));
        }
        Err(_) => return unsupported_usage(profile, "claude /usage timed out"),
    };
    if !output.status.success() {
        return unsupported_usage(
            profile,
            &format!("claude /usage exited with code {}", output.status),
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let windows = parse_claude_usage(&claude_result_text(&stdout));
    if windows.is_empty() {
        return unsupported_usage(profile, "claude /usage returned no limit lines");
    }
    ProfileUsage {
        profile: profile.id.clone(),
        provider: profile.provider,
        supported: true,
        source: UsageSource::ClaudeCli,
        windows,
        plan: None,
        observed_at: Some(format_rfc3339_ms(now_ms())),
        reason: None,
        account_failure: None,
        rate_limits_by_model: None,
        observed_rate_limits: None,
    }
}

/// `claude -p` wraps its answer in `{"result": "<text>"}`; anything else
/// (auth prompts, a stray warning) parses as empty text, which in turn parses
/// to no usage lines rather than a garbled one.
fn claude_result_text(stdout: &str) -> String {
    serde_json::from_str::<Value>(stdout.trim())
        .ok()
        .and_then(|value| {
            value
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// Parses lines like `Current week (Opus): 42% used · resets Aug 13 at
/// 11:59pm (Africa/Douala)`. `resets` is kept as prose, never resolved to an
/// instant here — Oga has no IANA timezone database in the Rust broker, and a
/// half-guessed instant is worse than none.
fn parse_claude_usage(text: &str) -> Vec<UsageWindow> {
    let mut windows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(colon) = line.find(": ") else {
            continue;
        };
        let label = line[..colon].trim();
        let rest = line[colon + 2..].trim();
        let Some(percent_end) = rest.find("% used") else {
            continue;
        };
        let Ok(used_percent) = rest[..percent_end].trim().parse::<f64>() else {
            continue;
        };
        let tail = rest[percent_end + "% used".len()..].trim();
        let resets_text = tail
            .strip_prefix('·')
            .map(str::trim)
            .and_then(|text| text.strip_prefix("resets"))
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
        let lower_label = label.to_lowercase();
        let kind = if lower_label.contains("session") {
            UsageWindowKind::Session
        } else if lower_label.contains("week") {
            UsageWindowKind::Week
        } else {
            UsageWindowKind::Other
        };
        // `Current week (Opus)` meters one model family; `Current week (all
        // models)` is the account's own window and qualifies nothing.
        let model = label
            .strip_suffix(')')
            .and_then(|prefix| prefix.rsplit_once('('))
            .map(|(_, scope)| scope.trim())
            .filter(|scope| !scope.eq_ignore_ascii_case("all models"))
            .map(str::to_lowercase);
        windows.push(UsageWindow {
            label: label.to_owned(),
            kind,
            used_percent,
            window_minutes: None,
            resets_at: None,
            resets_text,
            model,
        });
    }
    windows
}

/// Codex logs `token_count` events with `rate_limits` into every session
/// rollout; the newest few files are enough to find the latest one.
async fn codex_usage(profile: &Profile) -> ProfileUsage {
    let home = codex_home(profile);
    let sessions_dir = format!("{home}/sessions");
    let mut files = Vec::new();
    collect_rollout_files(Path::new(&sessions_dir), &mut files).await;
    files.sort();
    files.reverse();
    files.truncate(5);
    for path in &files {
        if let Some(found) = rate_limits_from_rollout(path).await {
            return ProfileUsage {
                profile: profile.id.clone(),
                provider: profile.provider,
                supported: true,
                source: UsageSource::CodexSessionLog,
                windows: found.windows,
                plan: found.plan,
                observed_at: found.observed_at,
                reason: None,
                account_failure: None,
                rate_limits_by_model: None,
                observed_rate_limits: None,
            };
        }
    }
    unsupported_usage(
        profile,
        &format!("no rate_limits events in recent session logs under {sessions_dir}"),
    )
}

async fn collect_rollout_files(dir: &Path, out: &mut Vec<String>) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let is_rollout = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"));
            if is_rollout && let Some(path) = path.to_str() {
                out.push(path.to_owned());
            }
        }
    }
}

struct CodexRateLimits {
    windows: Vec<UsageWindow>,
    plan: Option<String>,
    observed_at: Option<String>,
}

async fn rate_limits_from_rollout(path: &str) -> Option<CodexRateLimits> {
    let raw = tokio::fs::read_to_string(path).await.ok()?;
    raw.lines()
        .rev()
        .filter(|line| line.contains("\"rate_limits\""))
        .find_map(parse_codex_rate_limits)
}

fn parse_codex_rate_limits(line: &str) -> Option<CodexRateLimits> {
    let event: Value = serde_json::from_str(line).ok()?;
    let limits = event.get("payload")?.get("rate_limits")?;
    let windows = [
        codex_window("primary", limits.get("primary")),
        codex_window("secondary", limits.get("secondary")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if windows.is_empty() {
        return None;
    }
    Some(CodexRateLimits {
        windows,
        plan: limits
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        observed_at: event
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn codex_window(label: &str, window: Option<&Value>) -> Option<UsageWindow> {
    let window = window.filter(|value| !value.is_null())?;
    let used_percent = window.get("used_percent")?.as_f64()?;
    let window_minutes = window.get("window_minutes").and_then(Value::as_u64);
    let kind = match window_minutes {
        Some(minutes) if minutes >= 10_080 => UsageWindowKind::Week,
        Some(minutes) if minutes <= 720 => UsageWindowKind::Session,
        _ => UsageWindowKind::Other,
    };
    let resets_at = window
        .get("resets_at")
        .and_then(Value::as_f64)
        .map(|seconds| format_rfc3339_ms((seconds * 1000.0) as i64));
    Some(UsageWindow {
        label: label.to_owned(),
        kind,
        used_percent,
        window_minutes,
        resets_at,
        resets_text: None,
        model: None,
    })
}

/// Attaches the account-wide failure and per-model rate limits already on
/// file, so a usage read that failed to reach a provider still shows a
/// caller-visible reason it can't be dispatched to right now.
fn merge_observed_failures(usage: ProfileUsage, failures: &[FailureRow]) -> ProfileUsage {
    let own = failures
        .iter()
        .filter(|failure| failure.profile_id == usage.profile)
        .collect::<Vec<_>>();
    let account = own.iter().find(|failure| failure.model.is_none());
    let rate_limits = own
        .iter()
        .filter(|failure| failure.model.is_some())
        .map(|failure| oga_domain::RateLimitByModel {
            model: failure.model.clone().unwrap_or_default(),
            message: failure.message.clone(),
            failed_at: failure.failed_at.clone(),
            consecutive_failures: failure.consecutive_failures,
            retry_at: failure.retry_at.clone(),
        })
        .collect::<Vec<_>>();
    ProfileUsage {
        account_failure: account.map(|failure| oga_domain::AccountFailure {
            message: failure.message.clone(),
            failed_at: failure.failed_at.clone(),
            consecutive_failures: failure.consecutive_failures,
            retry_at: failure.retry_at.clone(),
        }),
        rate_limits_by_model: (!rate_limits.is_empty()).then_some(rate_limits),
        ..usage
    }
}

#[derive(Clone)]
struct FailureRow {
    profile_id: String,
    model: Option<String>,
    message: String,
    failed_at: String,
    consecutive_failures: u32,
    retry_at: Option<String>,
}

fn failures_for_profiles(store: &Store) -> Result<Vec<FailureRow>, HttpError> {
    store
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT profile_id,model,message,failed_at,consecutive_failures,retry_at FROM profile_failures ORDER BY profile_id,model",
            )?;
            Ok(statement
                .query_map([], |row| {
                    let model = row.get::<_, String>(1)?;
                    Ok(FailureRow {
                        profile_id: row.get(0)?,
                        model: (!model.is_empty()).then_some(model),
                        message: row.get(2)?,
                        failed_at: row.get(3)?,
                        consecutive_failures: row.get(4)?,
                        retry_at: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(HttpError::from)
}

fn now_iso() -> String {
    format_rfc3339_ms(now_ms())
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    fn profile(id: &str, provider: Provider) -> Profile {
        Profile {
            id: id.to_owned(),
            label: id.to_owned(),
            provider,
            default_model: "configured-default".to_owned(),
            enabled: true,
            env: Default::default(),
            capabilities: Vec::new(),
            command: None,
        }
    }

    #[test]
    fn usage_probes_read_account_directories_from_the_profile() {
        // SAFETY: test-only env mutation; nothing else in this crate reads these.
        unsafe {
            std::env::set_var("CLAUDE_CONFIG_DIR", "/broker/.claude-me");
            std::env::set_var("CODEX_HOME", "/broker/.codex-me");
        }
        let home = std::env::var("HOME").expect("HOME");

        let claude = profile("fixture-claude", Provider::Claude);
        assert_eq!(
            environment_for(&claude)["CLAUDE_CONFIG_DIR"],
            format!("{home}/.fixture-claude")
        );

        let mut work = profile("fixture-claude-work", Provider::Claude);
        work.env
            .insert("CLAUDE_CONFIG_DIR".into(), "$HOME/.claude-work".into());
        assert_eq!(
            environment_for(&work)["CLAUDE_CONFIG_DIR"],
            format!("{home}/.claude-work")
        );

        assert_eq!(
            codex_home(&profile("fixture-codex", Provider::Codex)),
            format!("{home}/.codex")
        );
    }

    #[tokio::test]
    async fn a_profile_never_discovered_falls_back_to_its_configured_model() {
        let opencode = profile("fixture-opencode", Provider::OpenCode);
        let models = cached_catalog(std::slice::from_ref(&opencode));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "configured-default");
        assert_eq!(models[0].source, ModelInfoSource::Configured);
    }

    #[tokio::test]
    async fn discover_catalog_reports_every_cached_model_including_disabled_ones() {
        let opencode = profile("fixture-cached-opencode", Provider::OpenCode);
        let seeded = vec![
            ModelInfo {
                id: "opencode-go/deepseek-v4-flash".into(),
                label: "opencode-go/deepseek-v4-flash".into(),
                provider: Provider::OpenCode,
                profile_id: opencode.id.clone(),
                source: ModelInfoSource::Discovered,
                cost: None,
                context_window: None,
                reasoning: None,
                efforts: None,
                default_effort: None,
                tool_call: None,
            },
            ModelInfo {
                id: "openai/gpt-5.6-luna".into(),
                label: "openai/gpt-5.6-luna".into(),
                provider: Provider::OpenCode,
                profile_id: opencode.id.clone(),
                source: ModelInfoSource::Discovered,
                cost: None,
                context_window: None,
                reasoning: None,
                efforts: None,
                default_effort: None,
                tool_call: None,
            },
        ];
        catalog_cache()
            .lock()
            .expect("catalog cache poisoned")
            .insert(opencode.id.clone(), (Instant::now(), seeded.clone()));

        // Not refreshing must reuse the cached catalog rather than collapsing
        // back to the single configured-model fallback.
        let models = discover_catalog(std::slice::from_ref(&opencode), false).await;
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"opencode-go/deepseek-v4-flash"));
        assert!(ids.contains(&"openai/gpt-5.6-luna"));

        // A disabled model must still be selectable, only dropped when the
        // caller explicitly asks for enabled-only rows.
        let settings = ResolvedModelSettings {
            global: Default::default(),
            project: None,
            overrides: None,
            love: LoveRules::default(),
        };
        let all_rows = select_model_rows(
            &models,
            &Default::default(),
            &settings,
            &DomainModelQuery {
                only_enabled: Some(false),
                only_preferred: Some(false),
                ..Default::default()
            },
            std::slice::from_ref(&opencode),
        );
        assert_eq!(all_rows.len(), 2);
        assert!(all_rows.iter().all(|row| !row.enabled));
    }

    #[test]
    fn cached_opencode_catalog_keeps_models_from_selected_providers() {
        let opencode = profile("fixture-opencode-cache", Provider::OpenCode);
        let providers = HashSet::from(["opencode".into(), "opencode-go".into()]);
        let raw = serde_json::json!({
            "opencode": {
                "models": {
                    "big-pickle": { "name": "Big Pickle", "reasoning": false, "tool_call": true }
                }
            },
            "opencode-go": {
                "models": {
                    "deepseek-v4-pro": { "name": "DeepSeek V4 Pro", "reasoning": true, "tool_call": true }
                }
            },
            "unselected": {
                "models": {
                    "hidden-model": { "name": "Hidden model" }
                }
            }
        })
        .to_string();

        let models = parse_opencode_cache_models(&raw, &opencode, &providers);
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();

        assert_eq!(ids, ["opencode-go/deepseek-v4-pro", "opencode/big-pickle"]);
        assert_eq!(models[0].label, "DeepSeek V4 Pro");
        assert!(models.iter().all(|model| model.profile_id == opencode.id));
    }

    #[test]
    fn opencode_catalog_merges_live_and_cached_models_without_duplicates() {
        let opencode = profile("fixture-opencode-merge", Provider::OpenCode);
        let live = ModelInfo {
            id: "opencode/live-model".into(),
            label: "Live model".into(),
            provider: Provider::OpenCode,
            profile_id: opencode.id.clone(),
            source: ModelInfoSource::Discovered,
            cost: None,
            context_window: None,
            reasoning: None,
            efforts: None,
            default_effort: None,
            tool_call: None,
        };
        let cached = ModelInfo {
            id: "opencode/cached-model".into(),
            label: "Cached model".into(),
            ..live.clone()
        };

        let models = merge_model_catalogs(vec![live], vec![cached.clone(), cached]);

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "opencode/cached-model");
        assert_eq!(models[1].id, "opencode/live-model");
    }

    #[tokio::test]
    async fn claude_profiles_use_the_catalog_and_cache_the_result() {
        let claude = profile("fixture-claude", Provider::Claude);
        let models = discover_catalog(&[claude], false).await;
        assert!(!models.is_empty());
        assert!(models.iter().all(|m| m.provider == Provider::Claude));
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn claude_usage_parses_scoped_and_account_windows_with_reset_prose() {
        let text = "Current session: 42% used · resets Aug 13 at 11:59pm (Africa/Douala)\n\
                     Current week (all models): 10% used\n\
                     Current week (Opus): 87% used · resets Aug 18 at 12:00am (Africa/Douala)\n\
                     not a usage line";
        let windows = parse_claude_usage(text);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].kind, UsageWindowKind::Session);
        assert_eq!(windows[0].used_percent, 42.0);
        assert_eq!(
            windows[0].resets_text.as_deref(),
            Some("Aug 13 at 11:59pm (Africa/Douala)")
        );
        assert_eq!(windows[0].model, None);
        assert_eq!(windows[1].kind, UsageWindowKind::Week);
        assert_eq!(
            windows[1].model, None,
            "an all-models window scopes nothing"
        );
        assert_eq!(windows[2].kind, UsageWindowKind::Week);
        assert_eq!(windows[2].used_percent, 87.0);
        assert_eq!(windows[2].model.as_deref(), Some("opus"));
    }

    #[test]
    fn claude_usage_skips_lines_that_do_not_match() {
        assert!(parse_claude_usage("").is_empty());
        assert!(parse_claude_usage("Current session: not a percent").is_empty());
        assert!(parse_claude_usage("no colon here at all").is_empty());
    }

    #[test]
    fn claude_result_text_reads_the_result_field_and_falls_back_to_empty() {
        assert_eq!(claude_result_text("{\"result\": \"line one\"}"), "line one");
        assert_eq!(claude_result_text("not json"), "");
        assert_eq!(claude_result_text("{\"other\": 1}"), "");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_usage_uses_the_profile_config_directory() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary directory");
        let program = directory.path().join("claude");
        fs::write(
            &program,
            "#!/bin/sh\ncase \"$CLAUDE_CONFIG_DIR\" in\n  */profile-config) printf '%s\\n' '{\"result\":\"Current session: 77% used\"}' ;;\n  *) printf '%s\\n' '{\"result\":\"no limit lines\"}' ;;\nesac\n",
        )
        .expect("fake claude");
        let mut permissions = fs::metadata(&program).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&program, permissions).expect("make fake claude executable");

        let mut profile = Profile {
            id: "claude-me".into(),
            label: "Claude me".into(),
            provider: Provider::Claude,
            default_model: "configured-default".into(),
            enabled: true,
            env: Default::default(),
            capabilities: Vec::new(),
            command: None,
        };
        profile
            .env
            .insert("CLAUDE_CONFIG_DIR".into(), "/tmp/profile-config".into());
        let usage =
            claude_usage_with_program(&profile, program.to_str().expect("utf-8 path")).await;

        assert!(usage.supported);
        assert_eq!(usage.windows[0].used_percent, 77.0);
    }

    #[test]
    fn codex_rate_limits_parse_primary_and_secondary_windows() {
        let line = r#"{"timestamp":"2026-08-25T00:00:00Z","payload":{"rate_limits":{"primary":{"used_percent":55.0,"window_minutes":300,"resets_at":1893456000},"secondary":{"used_percent":12.0,"window_minutes":10080},"plan_type":"pro"}}}"#;
        let found = parse_codex_rate_limits(line).expect("parses");
        assert_eq!(found.windows.len(), 2);
        assert_eq!(found.windows[0].label, "primary");
        assert_eq!(found.windows[0].kind, UsageWindowKind::Session);
        assert_eq!(found.windows[1].kind, UsageWindowKind::Week);
        assert_eq!(found.plan.as_deref(), Some("pro"));
        assert_eq!(found.observed_at.as_deref(), Some("2026-08-25T00:00:00Z"));
    }

    #[test]
    fn codex_rate_limits_are_none_without_a_rate_limits_payload() {
        assert!(parse_codex_rate_limits(r#"{"payload":{}}"#).is_none());
        assert!(parse_codex_rate_limits("not json").is_none());
    }
}
