//! The gate between a chosen model and a provider process. Work reaches a
//! model only where the user turned that model on, and every path that can end
//! in a provider call passes through here.

use oga_config::{
    MODEL_SETTINGS_KEY, ResolvedModelSettings, canonical_cwd, global_cwd, load_config_layers,
    model_enabled, model_not_enabled_message, read_model_overrides, read_model_settings,
};
use oga_domain::Task;
use oga_store::{Store, StoreError};
use serde_json::{Value, json};

/// The directory whose settings govern a task. A worktree run answers to the
/// project it was cut from, not to the checkout it happens to live in.
pub(crate) fn settings_cwd(task: &Task) -> String {
    task.worktree
        .as_ref()
        .map_or_else(|| task.cwd.clone(), |worktree| worktree.origin_cwd.clone())
}

pub(crate) fn resolved_model_settings(
    store: &Store,
    cwd: &str,
) -> Result<ResolvedModelSettings, StoreError> {
    let global = canonical_cwd(global_cwd()).display().to_string();
    let cwd = canonical_cwd(cwd).display().to_string();
    let project = if cwd == global {
        None
    } else {
        saved(store, &cwd)?.as_ref().map(read_model_settings)
    };
    let layers = load_config_layers((cwd != global).then_some(std::path::Path::new(&cwd)))
        .map_err(|error| StoreError::Refusal(error.to_string()))?;
    let (overrides, love) =
        read_model_overrides(&layers).map_err(|error| StoreError::Refusal(error.to_string()))?;
    Ok(ResolvedModelSettings {
        global: read_model_settings(&saved(store, &global)?.unwrap_or_else(|| json!({}))),
        project,
        overrides: Some(overrides),
        love,
    })
}

/// Refuse a model the user has not turned on for this worker, in this project.
pub fn check_model_enabled(
    store: &Store,
    cwd: &str,
    profile_id: &str,
    model: &str,
) -> Result<(), String> {
    let settings = resolved_model_settings(store, cwd).map_err(|error| error.to_string())?;
    if model_enabled(&settings, profile_id, model) {
        Ok(())
    } else {
        Err(model_not_enabled_message(profile_id, model))
    }
}

fn saved(store: &Store, cwd: &str) -> Result<Option<Value>, StoreError> {
    Ok(store
        .repositories()
        .settings()
        .get(cwd, MODEL_SETTINGS_KEY)?
        .and_then(|raw| serde_json::from_str(&raw).ok()))
}
