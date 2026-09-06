//! HTTP routing preview and dispatch selection.

use std::path::Path;

use axum::{Json, body::Bytes, extract::State, response::IntoResponse};
use oga_domain::{DecidedBy, Difficulty, DifficultySource, EffortSource, SelectionDecision};
use oga_routing::{
    NamedPair, ROUTER_VERSION, RoutePreferences, SelectionInputs, check_named_route, choose_model,
    load_routing_policy,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    router::{HttpError, HttpState, parse_json},
    settings, state,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewBody {
    pub cwd: String,
    pub prompt: String,
    pub difficulty: Option<Difficulty>,
    pub kind: Option<oga_domain::TaskTopic>,
}

#[derive(Debug, Clone)]
pub(crate) struct RouteInput {
    pub prompt: String,
    pub cwd: String,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub difficulty: Option<Difficulty>,
    pub topic: Option<oga_domain::TaskTopic>,
    pub effort: Option<String>,
    pub default_profile_shortcut: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct RoutePlan {
    pub profile_id: String,
    pub model: String,
    pub effort: Option<String>,
    pub decision: SelectionDecision,
    pub warnings: Vec<String>,
    pub reason: String,
}

pub async fn preview(
    State(state): State<HttpState>,
    body: Bytes,
) -> Result<impl IntoResponse, HttpError> {
    let body: PreviewBody = parse_json(&body)?;
    validate_preview(&body)?;
    let route = plan(
        &state,
        RouteInput {
            prompt: body.prompt,
            cwd: body.cwd,
            profile: None,
            model: None,
            difficulty: body.difficulty,
            topic: body.kind,
            effort: None,
            default_profile_shortcut: false,
        },
    )?;
    let record = &route.decision.record;
    let mut response = json!({
        "profileId": route.profile_id,
        "model": route.model,
        "difficulty": record.difficulty,
        "taskClass": record.heuristic_class,
        "reason": route.reason,
        "warnings": route.warnings,
    });
    if let Some(effort) = route.effort {
        response["effort"] = json!(effort);
    }
    response["effortReason"] = json!(route.decision.record.effort_reason);
    Ok(Json(response))
}

pub(crate) fn plan(state: &HttpState, input: RouteInput) -> Result<RoutePlan, HttpError> {
    let cwd = Path::new(&input.cwd);
    if !cwd.is_absolute() {
        return Err(HttpError::bad_request("cwd must be an absolute path"));
    }
    if !cwd.is_dir() {
        return Err(HttpError::bad_request("cwd does not exist"));
    }
    if input.prompt.trim().is_empty() {
        return Err(HttpError::bad_request("prompt must not be empty"));
    }

    let profiles = state.store.repositories().profiles().list()?;
    let models = settings::cached_catalog(&profiles);
    let cwd = oga_config::canonical_cwd(cwd);
    let cwd_string = cwd.display().to_string();
    let settings = settings::resolved_model_settings(&state.store, &cwd_string)?;
    let policy =
        load_routing_policy(&cwd).map_err(|error| HttpError::bad_request(error.to_string()))?;
    let failures = state::list_profile_failures(&state.store)?;
    let statuses = oga_routing::normalize_profile_statuses(
        &profiles,
        &models,
        &failures,
        &[],
        oga_routing::now_ms(),
        false,
    );
    // Cache-only: a live provider read here would put a CLI spawn on the hot
    // dispatch path. A profile with no cached read yet scores as unknown
    // rather than blocking route selection on it.
    let usage = settings::cached_usage(&profiles);
    let extra = match policy.as_ref() {
        Some(policy) => SelectionInputs::new(&settings)
            .statuses(&statuses)
            .usage(&usage)
            .policy(policy),
        None => SelectionInputs::new(&settings)
            .statuses(&statuses)
            .usage(&usage),
    };

    match (&input.profile, &input.model) {
        (Some(profile_id), Some(model)) => {
            // Availability and usage stay unread here, same as before: a
            // caller naming both profile and model gets no quota or
            // availability advice, only whether the pair itself is reachable.
            let named_extra = match policy.as_ref() {
                Some(policy) => SelectionInputs::new(&settings).policy(policy),
                None => SelectionInputs::new(&settings),
            };
            let audit = check_named_route(
                &input.prompt,
                NamedPair {
                    profile_id,
                    model,
                    difficulty: input.difficulty,
                    effort: input.effort.as_deref(),
                    preference: None,
                },
                &models,
                &profiles,
                &named_extra,
            )
            .map_err(|error| HttpError::bad_request(error.to_string()))?;
            Ok(RoutePlan {
                profile_id: profile_id.clone(),
                model: audit.resolved_model.clone(),
                effort: input.effort.clone().or(audit.effort.clone()),
                decision: decision_from_audit(
                    &audit,
                    if input.difficulty.is_some() {
                        DifficultySource::Caller
                    } else {
                        DifficultySource::Default
                    },
                    input.effort.is_some(),
                ),
                warnings: audit.warnings,
                reason: format!("caller chose {profile_id}/{}", audit.resolved_model),
            })
        }
        _ => {
            let default_route = input.default_profile_shortcut
                && input.profile.is_none()
                && input.model.is_none()
                && input.difficulty.is_none()
                && input.topic.is_none()
                && default_profile(&profiles).is_some();
            let profile_id = if default_route {
                default_profile(&profiles).map(|profile| profile.id.clone())
            } else {
                input.profile.clone()
            };
            let difficulty = input.difficulty;
            let route = choose_model(
                &input.prompt,
                &models,
                &profiles,
                &RoutePreferences {
                    model_hint: input.model.clone(),
                    difficulty,
                    topic: input.topic,
                    profile_id,
                    ..RoutePreferences::default()
                },
                &extra,
            )
            .map_err(|error| HttpError::bad_request(error.to_string()))?;
            let effort = input
                .effort
                .clone()
                .or_else(|| route.effort.clone())
                .or_else(|| default_route.then(|| "low".to_owned()));
            Ok(RoutePlan {
                profile_id: route.profile_id.clone(),
                model: route.model.clone(),
                effort,
                decision: decision_from_route(
                    &route,
                    if input.profile.is_some() {
                        DecidedBy::CallerProfile
                    } else {
                        DecidedBy::Router
                    },
                    if input.difficulty.is_some() {
                        DifficultySource::Caller
                    } else {
                        DifficultySource::Default
                    },
                    input.effort.is_some(),
                ),
                warnings: route.warnings,
                reason: route.reason,
            })
        }
    }
}

fn default_profile(profiles: &[oga_domain::Profile]) -> Option<&oga_domain::Profile> {
    profiles
        .iter()
        .find(|profile| profile.id == "default" && profile.enabled)
        .or_else(|| profiles.iter().find(|profile| profile.enabled))
}

fn validate_preview(body: &PreviewBody) -> Result<(), HttpError> {
    if body.prompt.trim().is_empty() {
        return Err(HttpError::bad_request("prompt must not be empty"));
    }
    Ok(())
}

fn decision_from_route(
    route: &oga_routing::ModelRoute,
    decided_by: DecidedBy,
    difficulty_source: DifficultySource,
    caller_effort: bool,
) -> SelectionDecision {
    SelectionDecision {
        record: oga_domain::RoutingRecord {
            decided_by,
            router_version: ROUTER_VERSION,
            difficulty: route.difficulty,
            difficulty_source,
            heuristic_class: route.task_class,
            heuristic_agreed: route.heuristic_agreed,
            floor: route.floor,
            relaxed: route.relaxed.clone(),
            preference: route.preference,
            effort_source: if caller_effort {
                EffortSource::Caller
            } else if route.effort_reason == "the loved model's configured reasoning effort" {
                EffortSource::Loved
            } else if route.effort.is_some() {
                EffortSource::Projected
            } else {
                EffortSource::None
            },
            effort_reason: if caller_effort {
                "the caller set it".into()
            } else {
                route.effort_reason.clone()
            },
            quota_used_percent: route.quota_used_percent,
            runners_up: route.candidates.get(1..).map(|candidates| {
                candidates
                    .iter()
                    .map(|candidate| oga_domain::RunnerUp {
                        profile_id: candidate.profile_id.clone(),
                        model: candidate.model.clone(),
                    })
                    .collect()
            }),
            rejected: (!route.rejected.is_empty()).then(|| route.rejected.clone()),
            rejected_count: (!route.rejected.is_empty()).then_some(route.rejected_count as u64),
            warnings: (!route.warnings.is_empty()).then(|| route.warnings.clone()),
        },
        chosen: None,
    }
}

fn decision_from_audit(
    audit: &oga_routing::NamedRouteAudit,
    difficulty_source: DifficultySource,
    caller_effort: bool,
) -> SelectionDecision {
    SelectionDecision {
        record: oga_domain::RoutingRecord {
            decided_by: DecidedBy::CallerExplicit,
            router_version: ROUTER_VERSION,
            difficulty: audit.difficulty,
            difficulty_source,
            heuristic_class: audit.task_class,
            heuristic_agreed: audit.heuristic_agreed,
            floor: audit.floor,
            relaxed: Vec::new(),
            preference: audit.preference,
            effort_source: if caller_effort {
                EffortSource::Caller
            } else if audit.effort.is_some() {
                EffortSource::Projected
            } else {
                EffortSource::None
            },
            effort_reason: if caller_effort {
                "the caller set it".into()
            } else {
                audit.effort_reason.clone()
            },
            quota_used_percent: audit.quota_used_percent,
            runners_up: None,
            rejected: (!audit.rejected.is_empty()).then(|| audit.rejected.clone()),
            rejected_count: (!audit.rejected.is_empty()).then_some(audit.rejected.len() as u64),
            warnings: (!audit.warnings.is_empty()).then(|| audit.warnings.clone()),
        },
        chosen: None,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use oga_domain::{Profile, Provider};
    use oga_store::Store;

    use super::*;

    const MODEL: &str = "opencode-go/ox-alpha-free";

    fn fixture() -> (tempfile::TempDir, Arc<Store>) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        store
            .repositories()
            .profiles()
            .insert(
                &Profile {
                    id: "profile".into(),
                    label: "Test Profile".into(),
                    provider: Provider::OpenCode,
                    default_model: MODEL.into(),
                    enabled: true,
                    env: BTreeMap::new(),
                    capabilities: Vec::new(),
                    command: None,
                },
                "2026-01-01T00:00:00.000Z",
            )
            .expect("profile insert");
        (directory, store)
    }

    fn switch(store: &Store, cwd: &Path, on: bool) {
        store
            .repositories()
            .settings()
            .put(
                &oga_config::canonical_cwd(cwd).display().to_string(),
                oga_config::MODEL_SETTINGS_KEY,
                &json!({ "profiles": { "profile": { "modelEnabled": { MODEL: on } } } })
                    .to_string(),
                "2026-01-01T00:00:00.000Z",
            )
            .expect("model setting");
    }

    fn route_to_model(store: Arc<Store>, cwd: &Path) -> Result<RoutePlan, HttpError> {
        plan(
            &HttpState::new(store),
            RouteInput {
                prompt: "inspect the fixture".into(),
                cwd: cwd.display().to_string(),
                profile: None,
                model: Some(MODEL.into()),
                difficulty: None,
                topic: None,
                effort: None,
                default_profile_shortcut: false,
            },
        )
    }

    #[test]
    fn model_hint_can_route_without_a_profile() {
        let (directory, store) = fixture();
        switch(&store, directory.path(), true);

        let route = route_to_model(store, directory.path()).expect("route");

        assert_eq!(route.profile_id, "profile");
        assert_eq!(route.model, MODEL);
    }

    #[test]
    fn project_loved_model_reaches_unnamed_route() {
        let (directory, store) = fixture();
        switch(&store, directory.path(), true);
        std::fs::write(
            directory.path().join(".oga.yaml"),
            format!("models:\n  profile:\n    {MODEL}:\n      loved: true\n"),
        )
        .expect("project config");

        let route = plan(
            &HttpState::new(store),
            RouteInput {
                prompt: "inspect the fixture".into(),
                cwd: directory.path().display().to_string(),
                profile: None,
                model: None,
                difficulty: None,
                topic: None,
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .expect("route");

        assert_eq!(route.profile_id, "profile");
        assert_eq!(route.model, MODEL);
    }

    #[test]
    fn a_love_rule_for_the_kind_of_work_reaches_an_unnamed_route() {
        let (directory, store) = fixture();
        switch(&store, directory.path(), true);
        std::fs::write(
            directory.path().join(".oga.yaml"),
            format!("love:\n  - model: profile:{MODEL}\n    when: [context]\n    effort: low\n"),
        )
        .expect("project config");

        let route = plan(
            &HttpState::new(store),
            RouteInput {
                prompt: "Read these files and understand how auth works.".into(),
                cwd: directory.path().display().to_string(),
                profile: None,
                model: None,
                difficulty: None,
                topic: None,
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .expect("route");

        assert_eq!(route.model, MODEL);
        assert_eq!(route.profile_id, "profile");
    }

    #[test]
    fn a_love_rule_for_a_subject_beats_one_for_the_class() {
        use oga_domain::TaskTopic;

        let (directory, store) = fixture();
        switch(&store, directory.path(), true);
        std::fs::write(
            directory.path().join(".oga.yaml"),
            format!("love:\n  - model: profile:{MODEL}\n    when: [ui]\n"),
        )
        .expect("project config");

        // The prompt reads as build work about a UI subject; the ui rule wins.
        let route = plan(
            &HttpState::new(store.clone()),
            RouteInput {
                prompt: "Implement the login UI component.".into(),
                cwd: directory.path().display().to_string(),
                profile: None,
                model: None,
                difficulty: None,
                topic: None,
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .expect("route");

        assert_eq!(route.model, MODEL);
        assert!(
            route.reason.contains("loved for ui work"),
            "{}",
            route.reason
        );

        // A named subject at dispatch applies even when the prompt never says so.
        let route = plan(
            &HttpState::new(store),
            RouteInput {
                prompt: "Implement the thing described in the plan.".into(),
                cwd: directory.path().display().to_string(),
                profile: None,
                model: None,
                difficulty: None,
                topic: Some(TaskTopic::Ui),
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .expect("route");

        assert_eq!(route.model, MODEL);
    }

    #[test]
    fn naming_a_model_that_is_off_refuses_instead_of_substituting() {
        let (directory, store) = fixture();

        let refusal = route_to_model(store.clone(), directory.path()).expect_err("refused");
        assert!(
            refusal
                .message
                .contains("opencode-go/ox-alpha-free is not turned on for profile"),
            "{}",
            refusal.message
        );
        assert!(
            refusal.message.contains("Open Settings"),
            "{}",
            refusal.message
        );

        // Switched on, then off again: the switch is what decides, both ways.
        switch(&store, directory.path(), true);
        assert!(route_to_model(store.clone(), directory.path()).is_ok());
        switch(&store, directory.path(), false);
        assert!(route_to_model(store, directory.path()).is_err());
    }
}
