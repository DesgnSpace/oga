//! HTTP routing preview and dispatch selection.

use std::path::Path;

use axum::{Json, body::Bytes, extract::State, response::IntoResponse};
use oga_advisor::{Advisor, Choice};
use oga_config::{ModelOverrides, model_override_for};
use oga_domain::{AdvisedRoute, DecidedBy, EffortSource, ModelInfo, Profile, SelectionDecision};
use oga_routing::{
    AdvisedModel, ModelRoute, NamedPair, ROUTER_VERSION, RoutePreferences, SelectionInputs,
    check_named_route, choose_model, load_routing_policy, model_capabilities, offered_models,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    router::{HttpError, HttpState, parse_json},
    settings, state,
};

/// The message a caller sending `difficulty` gets on every route-starting
/// surface: it names what replaced the option instead of dropping it silently.
const DIFFICULTY_REMOVED_MESSAGE: &str = "difficulty is no longer an option; name the kind of \
     work with kind, and set how hard the model thinks with effort";

pub(crate) fn reject_difficulty(value: &Value) -> Result<(), HttpError> {
    if value.get("difficulty").is_some() {
        return Err(HttpError::bad_request(DIFFICULTY_REMOVED_MESSAGE));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewBody {
    pub cwd: String,
    pub prompt: String,
    pub kind: Option<oga_domain::WorkKind>,
}

/// What a caller wants when a task starts: a prompt to read and any of the
/// caller's own overrides. Shared by the HTTP dispatch and preview routes and
/// by MCP's `delegate`, so every entry point that starts a task resolves a
/// destination the same way.
#[derive(Debug, Clone)]
pub struct RouteInput {
    pub prompt: String,
    pub cwd: String,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub kind: Option<oga_domain::WorkKind>,
    pub effort: Option<String>,
    pub default_profile_shortcut: bool,
}

#[derive(Debug, Clone)]
pub struct RoutePlan {
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
    let raw: Value =
        serde_json::from_slice(&body).map_err(|_| HttpError::bad_request("invalid JSON body"))?;
    reject_difficulty(&raw)?;
    let body: PreviewBody = parse_json(&body)?;
    validate_preview(&body)?;
    let route = plan(
        &state,
        RouteInput {
            prompt: body.prompt,
            cwd: body.cwd,
            profile: None,
            model: None,
            kind: body.kind,
            effort: None,
            default_profile_shortcut: false,
        },
    )
    .await?;
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

/// Everything a route is chosen against, read once per dispatch: the
/// accounts, their catalogs, this project's settings and policy, and the
/// cached availability and usage behind them.
pub(crate) struct RoutingInputs {
    profiles: Vec<Profile>,
    models: Vec<ModelInfo>,
    settings: oga_config::ResolvedModelSettings,
    policy: Option<oga_routing::RoutingPolicy>,
    statuses: Vec<oga_routing::ProfileStatus>,
    usage: Vec<oga_domain::ProfileUsage>,
}

impl RoutingInputs {
    pub(crate) fn read(state: &HttpState, cwd: &Path) -> Result<RoutingInputs, HttpError> {
        let profiles = state.store.repositories().profiles().list()?;
        let models = settings::cached_catalog(&profiles);
        let settings = settings::resolved_model_settings(&state.store, &cwd.display().to_string())?;
        let policy =
            load_routing_policy(cwd).map_err(|error| HttpError::bad_request(error.to_string()))?;
        let failures = state::list_profile_failures(&state.store)?;
        let statuses = oga_routing::normalize_profile_statuses(
            &profiles,
            &models,
            &failures,
            &[],
            oga_routing::now_ms(),
            false,
        );
        // Cache-only: a live provider read here would put a CLI spawn on the
        // hot dispatch path. A profile with no cached read yet scores as
        // unknown rather than blocking route selection on it.
        let usage = settings::cached_usage(&profiles);
        Ok(RoutingInputs {
            profiles,
            models,
            settings,
            policy,
            statuses,
            usage,
        })
    }

    fn selection(&self) -> SelectionInputs<'_> {
        let inputs = SelectionInputs::new(&self.settings)
            .statuses(&self.statuses)
            .usage(&self.usage);
        match self.policy.as_ref() {
            Some(policy) => inputs.policy(policy),
            None => inputs,
        }
    }
}

pub async fn plan(state: &HttpState, input: RouteInput) -> Result<RoutePlan, HttpError> {
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

    let world = RoutingInputs::read(state, &oga_config::canonical_cwd(cwd))?;
    let RoutingInputs {
        profiles,
        models,
        settings,
        policy,
        ..
    } = &world;
    let extra = world.selection();

    match (&input.profile, &input.model) {
        (Some(profile_id), Some(model)) => {
            // Availability and usage stay unread here, same as before: a
            // caller naming both profile and model gets no quota or
            // availability advice, only whether the pair itself is reachable.
            let named_extra = match policy.as_ref() {
                Some(policy) => SelectionInputs::new(settings).policy(policy),
                None => SelectionInputs::new(settings),
            };
            let audit = check_named_route(
                &input.prompt,
                NamedPair {
                    profile_id,
                    model,
                    effort: input.effort.as_deref(),
                    preference: None,
                },
                models,
                profiles,
                &named_extra,
            )
            .map_err(|error| HttpError::bad_request(error.to_string()))?;
            Ok(RoutePlan {
                profile_id: profile_id.clone(),
                model: audit.resolved_model.clone(),
                effort: input.effort.clone().or(audit.effort.clone()),
                decision: decision_from_audit(&audit, input.effort.is_some()),
                warnings: audit.warnings,
                reason: format!("caller chose {profile_id}/{}", audit.resolved_model),
            })
        }
        _ => {
            let default_route = input.default_profile_shortcut
                && input.profile.is_none()
                && input.model.is_none()
                && input.kind.is_none()
                && default_profile(profiles).is_some();
            let profile_id = if default_route {
                default_profile(profiles).map(|profile| profile.id.clone())
            } else {
                input.profile.clone()
            };
            let preferences = RoutePreferences {
                model_hint: input.model.clone(),
                kind: input.kind,
                profile_id,
                ..RoutePreferences::default()
            };
            let route = choose_model(&input.prompt, models, profiles, &preferences, &extra)
                .map_err(|error| HttpError::bad_request(error.to_string()))?;
            // Advice only stands in for a choice the router made on its own.
            // A caller who named the account or the model already answered
            // the question, so nothing is asked on their behalf.
            let (route, advice) = if input.profile.is_some() || input.model.is_some() {
                (route, None)
            } else {
                match advisor(state)? {
                    Some(advisor) => {
                        let (route, advice) =
                            take_advice(&advisor, &input, &world, &preferences, route).await;
                        (route, Some(advice))
                    }
                    None => (route, Some(Err("the advisor is switched off".to_owned()))),
                }
            };
            let (advised, advisor_unanswered) = match advice {
                Some(Ok(advised)) => (Some(advised), None),
                Some(Err(reason)) => (None, Some(reason)),
                None => (None, None),
            };
            let effort = input
                .effort
                .clone()
                .or_else(|| route.effort.clone())
                .or_else(|| default_route.then(|| "low".to_owned()));
            let decided_by = if advised.as_ref().is_some_and(|advised| advised.used) {
                DecidedBy::Advisor
            } else if input.profile.is_some() {
                DecidedBy::CallerProfile
            } else {
                DecidedBy::Router
            };
            Ok(RoutePlan {
                profile_id: route.profile_id.clone(),
                model: route.model.clone(),
                effort,
                decision: decision_from_route(
                    &route,
                    decided_by,
                    input.effort.is_some(),
                    advised,
                    advisor_unanswered,
                ),
                warnings: route.warnings,
                reason: route.reason,
            })
        }
    }
}

/// Ask the advisor where this brief should run and take its answer. Anything
/// else leaves the route the rules built exactly as it was: switched off, no
/// key, a call that failed or ran long, or a destination selection will not
/// send this work to. What was advised is recorded either way, so a task that ran
/// on the rules still shows what it was weighed against, and a call with no
/// pick comes back as the reason.
async fn take_advice(
    advisor: &Advisor,
    input: &RouteInput,
    world: &RoutingInputs,
    preferences: &RoutePreferences,
    route: ModelRoute,
) -> (ModelRoute, Result<AdvisedRoute, String>) {
    let destinations = describe_destinations(world);
    match advisor.choose(&advisor_brief(input), &destinations).await {
        Ok(choice) => {
            let (route, advised) = apply_advice(input, world, preferences, route, choice);
            (route, Ok(advised))
        }
        Err(reason) => (route, Err(reason.to_string())),
    }
}

/// The advisor to ask, when one is switched on and signed in. The key it
/// holds goes nowhere but the request header.
fn advisor(state: &HttpState) -> Result<Option<Advisor>, HttpError> {
    let settings = settings::advisor_settings(&state.store)?;
    Ok((settings.enabled && !settings.api_key.is_empty()).then(|| Advisor::new(settings.api_key)))
}

/// Follow one answer in place of the route the rules built. Advice naming a
/// destination selection will not send this work to is recorded and passed
/// up rather than followed.
fn apply_advice(
    input: &RouteInput,
    world: &RoutingInputs,
    preferences: &RoutePreferences,
    route: ModelRoute,
    choice: Choice,
) -> (ModelRoute, AdvisedRoute) {
    let mut advised = AdvisedRoute {
        profile_id: choice.profile_id.clone(),
        model: choice.model.clone(),
        confidence: choice.confidence,
        used: false,
        effort: choice.effort.clone(),
        ignored_because: None,
    };
    let preferences = RoutePreferences {
        advised: Some(AdvisedModel {
            profile_id: choice.profile_id.clone(),
            model: choice.model.clone(),
        }),
        ..preferences.clone()
    };
    match choose_model(
        &input.prompt,
        &world.models,
        &world.profiles,
        &preferences,
        &world.selection(),
    ) {
        Ok(mut picked)
            if picked.profile_id == choice.profile_id && picked.model == choice.model =>
        {
            advised.used = true;
            if let Some(effort) = choice.effort {
                picked.effort = Some(effort);
                picked.effort_reason = "the advisor matched it to the brief".into();
            }
            (picked, advised)
        }
        _ => {
            advised.ignored_because = Some("that worker could not take this task".into());
            (route, advised)
        }
    }
}

/// Every destination this task could go to, each described by what it is good
/// at, so the advisor weighs strengths rather than names.
fn describe_destinations(world: &RoutingInputs) -> Vec<oga_advisor::Destination> {
    let no_overrides = ModelOverrides::default();
    let overrides = world.settings.overrides.as_ref().unwrap_or(&no_overrides);
    let now_ms = oga_routing::now_ms();
    offered_models(&world.models, &world.profiles, &world.selection())
        .into_iter()
        .map(|model| {
            let capabilities = model_capabilities(
                model,
                model_override_for(overrides, &model.profile_id, &model.id).as_ref(),
            );
            let mut facts = capabilities;
            if let Some(window) = model.context_window {
                facts.push(format!("{}k-token context", window / 1000));
            }
            if let Some(cost) = model
                .cost
                .as_ref()
                .filter(|cost| cost.input > 0.0 || cost.output > 0.0)
            {
                facts.push(format!(
                    "${} in / ${} out per million tokens",
                    cost.input, cost.output
                ));
            }
            if let Some(usage) = world
                .usage
                .iter()
                .find(|usage| usage.profile == model.profile_id)
            {
                facts.extend(usage_facts(usage, &model.id, now_ms));
            }
            let efforts = model.efforts.clone().unwrap_or_default();
            if !efforts.is_empty() {
                facts.push(format!("effort levels: {}", efforts.join(", ")));
            }
            oga_advisor::Destination {
                profile_id: model.profile_id.clone(),
                model: model.id.clone(),
                description: if facts.is_empty() {
                    model.label.clone()
                } else {
                    format!("{}: {}", model.label, facts.join("; "))
                },
                efforts,
            }
        })
        .collect()
}

/// How much of this account is left for `model` and when it comes back, so
/// the advisor can spend an allowance that resets soon before it is lost and
/// steer clear of one about to run out.
fn usage_facts(usage: &oga_domain::ProfileUsage, model: &str, now_ms: i64) -> Vec<String> {
    let summary = oga_routing::summarize_usage(usage, model);
    let mut facts = Vec::new();
    if summary.out_of_credits {
        facts.push("out of credits".to_owned());
    }
    if summary.rate_limited {
        facts.push("rate-limited right now".to_owned());
    }
    let week = oga_routing::weekly_window(usage, Some(model));
    if let Some(week) = week {
        let resets = week
            .resets_at
            .as_deref()
            .and_then(oga_routing::parse_rfc3339_ms)
            .map(|at| format!(", resets in {}h", (at - now_ms).max(0) / 3_600_000))
            .unwrap_or_default();
        facts.push(format!(
            "{:.0}% of its weekly allowance used{resets}",
            week.used_percent
        ));
    }
    if let Some(used) = summary.percent_used
        && week.is_none_or(|week| week.used_percent != used)
    {
        facts.push(format!("{used:.0}% of its tightest limit used"));
    }
    facts
}

/// What the advisor reads: the brief the caller typed and the kind of work
/// they named with it. Never memories, never files — only what was already
/// said out loud when the task was handed over.
fn advisor_brief(input: &RouteInput) -> String {
    match input.kind {
        Some(kind) => format!("Kind of work: {}\n\n{}", kind.as_str(), input.prompt),
        None => input.prompt.clone(),
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
    caller_effort: bool,
    advised: Option<AdvisedRoute>,
    advisor_unanswered: Option<String>,
) -> SelectionDecision {
    SelectionDecision {
        record: oga_domain::RoutingRecord {
            decided_by,
            router_version: ROUTER_VERSION,
            difficulty: route.difficulty,
            heuristic_class: route.task_class,
            floor: route.floor,
            relaxed: route.relaxed.clone(),
            preference: route.preference,
            effort_source: if caller_effort {
                EffortSource::Caller
            } else if advised
                .as_ref()
                .is_some_and(|advised| advised.used && advised.effort.is_some())
            {
                EffortSource::Advisor
            } else if route.effort_reason == "the loved model's configured reasoning effort" {
                EffortSource::Loved
            } else if route.effort.is_some() {
                EffortSource::Default
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
            advised,
            advisor_unanswered,
        },
        chosen: None,
    }
}

fn decision_from_audit(
    audit: &oga_routing::NamedRouteAudit,
    caller_effort: bool,
) -> SelectionDecision {
    SelectionDecision {
        record: oga_domain::RoutingRecord {
            decided_by: DecidedBy::CallerExplicit,
            router_version: ROUTER_VERSION,
            difficulty: audit.difficulty,
            heuristic_class: audit.task_class,
            floor: audit.floor,
            relaxed: Vec::new(),
            preference: audit.preference,
            effort_source: if caller_effort {
                EffortSource::Caller
            } else if audit.effort.is_some() {
                EffortSource::Default
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
            advised: None,
            advisor_unanswered: None,
        },
        chosen: None,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use axum::extract::State;
    use oga_domain::{Profile, Provider};
    use oga_store::Store;

    use super::*;

    const MODEL: &str = "opencode-go/ox-alpha-free";
    const FAST_MODEL: &str = "opencode-go/deepseek-v4-flash";
    const DEEP_MODEL: &str = "openai/gpt-5.6-luna";

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

    async fn route_to_model(store: Arc<Store>, cwd: &Path) -> Result<RoutePlan, HttpError> {
        plan(
            &HttpState::new(store),
            RouteInput {
                prompt: "inspect the fixture".into(),
                cwd: cwd.display().to_string(),
                profile: None,
                model: Some(MODEL.into()),
                kind: None,
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .await
    }

    #[tokio::test]
    async fn model_hint_can_route_without_a_profile() {
        let (directory, store) = fixture();
        switch_model(&store, directory.path(), "profile", MODEL, true);

        let route = route_to_model(store, directory.path())
            .await
            .expect("route");

        assert_eq!(route.profile_id, "profile");
        assert_eq!(route.model, MODEL);
    }

    #[tokio::test]
    async fn project_loved_model_reaches_unnamed_route() {
        let (directory, store) = fixture();
        switch_model(&store, directory.path(), "profile", MODEL, true);
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
                kind: None,
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .await
        .expect("route");

        assert_eq!(route.profile_id, "profile");
        assert_eq!(route.model, MODEL);
    }

    #[tokio::test]
    async fn a_love_rule_for_the_kind_of_work_reaches_an_unnamed_route() {
        let (directory, store) = fixture();
        switch_model(&store, directory.path(), "profile", MODEL, true);
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
                kind: None,
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .await
        .expect("route");

        assert_eq!(route.model, MODEL);
        assert_eq!(route.profile_id, "profile");
    }

    #[tokio::test]
    async fn a_love_rule_for_a_subject_beats_one_for_the_class() {
        use oga_domain::WorkKind;

        let (directory, store) = fixture();
        switch_model(&store, directory.path(), "profile", MODEL, true);
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
                kind: None,
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .await
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
                kind: Some(WorkKind::Ui),
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .await
        .expect("route");

        assert_eq!(route.model, MODEL);
    }

    #[tokio::test]
    async fn a_named_class_kind_reaches_its_loved_model_over_the_prompts_own_read() {
        use oga_domain::WorkKind;

        let (directory, store) = fixture();
        switch_model(&store, directory.path(), "profile", MODEL, true);
        std::fs::write(
            directory.path().join(".oga.yaml"),
            format!("love:\n  - model: profile:{MODEL}\n    when: [mechanical]\n"),
        )
        .expect("project config");

        // The prompt itself reads as build work, which no rule claims here,
        // so it runs on the ordinary path rather than the loved one.
        let unnamed = plan(
            &HttpState::new(store.clone()),
            RouteInput {
                prompt: "Implement the thing described in the plan.".into(),
                cwd: directory.path().display().to_string(),
                profile: None,
                model: None,
                kind: None,
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .await
        .expect("route");
        assert!(!unnamed.reason.contains("loved"), "{}", unnamed.reason);

        // Naming the class at dispatch reaches the rule even though the
        // prompt itself never reads as mechanical work.
        let route = plan(
            &HttpState::new(store),
            RouteInput {
                prompt: "Implement the thing described in the plan.".into(),
                cwd: directory.path().display().to_string(),
                profile: None,
                model: None,
                kind: Some(WorkKind::Mechanical),
                effort: None,
                default_profile_shortcut: false,
            },
        )
        .await
        .expect("route");

        assert_eq!(route.model, MODEL);
        assert!(
            route
                .reason
                .contains("loved for mechanical work, the kind you named"),
            "{}",
            route.reason
        );
    }

    #[tokio::test]
    async fn naming_a_model_that_is_off_refuses_instead_of_substituting() {
        let (directory, store) = fixture();

        let refusal = route_to_model(store.clone(), directory.path())
            .await
            .expect_err("refused");
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
        switch_model(&store, directory.path(), "profile", MODEL, true);
        assert!(
            route_to_model(store.clone(), directory.path())
                .await
                .is_ok()
        );
        switch_model(&store, directory.path(), "profile", MODEL, false);
        assert!(route_to_model(store, directory.path()).await.is_err());
    }

    /// Two accounts with one model each, so advice has something to choose
    /// between and the loved rule has something to be overridden.
    fn two_workers() -> (tempfile::TempDir, Arc<Store>) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        for (id, model) in [("fast", FAST_MODEL), ("deep", DEEP_MODEL)] {
            store
                .repositories()
                .profiles()
                .insert(
                    &Profile {
                        id: id.into(),
                        label: id.into(),
                        provider: Provider::OpenCode,
                        default_model: model.into(),
                        enabled: true,
                        env: BTreeMap::new(),
                        capabilities: Vec::new(),
                        command: None,
                    },
                    "2026-01-01T00:00:00.000Z",
                )
                .expect("profile insert");
            switch_model(&store, directory.path(), id, model, true);
        }
        (directory, store)
    }

    fn switch_model(store: &Store, cwd: &Path, profile: &str, model: &str, on: bool) {
        let cwd = oga_config::canonical_cwd(cwd).display().to_string();
        let existing: Value = store
            .repositories()
            .settings()
            .get(&cwd, oga_config::MODEL_SETTINGS_KEY)
            .expect("read")
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| json!({ "profiles": {} }));
        let mut settings = existing;
        settings["profiles"][profile]["modelEnabled"][model] = json!(on);
        store
            .repositories()
            .settings()
            .put(
                &cwd,
                oga_config::MODEL_SETTINGS_KEY,
                &settings.to_string(),
                "2026-01-01T00:00:00.000Z",
            )
            .expect("model setting");
    }

    fn love(directory: &Path, destination: &str) {
        std::fs::write(
            directory.join(".oga.yaml"),
            format!("love:\n  - model: {destination}\n"),
        )
        .expect("project config");
    }

    fn unnamed(cwd: &Path) -> RouteInput {
        RouteInput {
            prompt: "Implement the thing described in the plan.".into(),
            cwd: cwd.display().to_string(),
            profile: None,
            model: None,
            kind: None,
            effort: None,
            default_profile_shortcut: false,
        }
    }

    /// Everything a route is chosen against for this cwd, as `plan` reads it.
    fn world_of(state: &HttpState, input: &RouteInput) -> RoutingInputs {
        RoutingInputs::read(state, &oga_config::canonical_cwd(Path::new(&input.cwd)))
            .expect("world")
    }

    /// The route the rules alone build — what advice is weighed against.
    fn rules_route(world: &RoutingInputs, input: &RouteInput) -> ModelRoute {
        choose_model(
            &input.prompt,
            &world.models,
            &world.profiles,
            &RoutePreferences::default(),
            &world.selection(),
        )
        .expect("route")
    }

    async fn store_advisor(state: &HttpState, settings: Value) {
        let _ = settings::put_advisor(State(state.clone()), Bytes::from(settings.to_string()))
            .await
            .expect("advisor settings");
    }

    fn advice(profile_id: &str, model: &str, confidence: f64) -> Choice {
        Choice {
            profile_id: profile_id.into(),
            model: model.into(),
            confidence,
            effort: None,
        }
    }

    #[tokio::test]
    async fn a_stored_key_decides_nothing_until_the_switch_is_on() {
        let (directory, store) = two_workers();
        love(directory.path(), &format!("fast:{FAST_MODEL}"));
        let state = HttpState::new(store.clone());
        store_advisor(&state, json!({ "enabled": false, "apiKey": "secret" })).await;

        let route = plan(&state, unnamed(directory.path()))
            .await
            .expect("route");

        assert_eq!(route.profile_id, "fast");
        assert_eq!(route.decision.record.decided_by, DecidedBy::Router);
        assert_eq!(route.decision.record.advised, None);
        assert_eq!(
            route.decision.record.advisor_unanswered.as_deref(),
            Some("the advisor is switched off")
        );
        assert!(route.reason.contains("loved"), "{}", route.reason);
    }

    #[tokio::test]
    async fn the_key_never_comes_back_out_of_the_api() {
        let (_directory, store) = two_workers();
        let state = HttpState::new(store);
        store_advisor(
            &state,
            json!({ "enabled": true, "apiKey": "ts-live-secret" }),
        )
        .await;

        let shown = settings::get_advisor(State(state.clone()))
            .await
            .expect("read")
            .0;
        assert_eq!(shown["apiKey"], "••••••••");
        assert_eq!(shown["enabled"], true);

        // Writing the mask back leaves the stored key where it was, so
        // turning the switch off and on again does not wipe it.
        store_advisor(&state, json!({ "enabled": false, "apiKey": "••••••••" })).await;
        assert_eq!(
            settings::advisor_settings(&state.store)
                .expect("read")
                .api_key,
            "ts-live-secret"
        );
    }

    #[tokio::test]
    async fn turning_the_switch_on_without_a_key_is_refused() {
        let (_directory, store) = two_workers();
        let state = HttpState::new(store);

        let refusal = settings::put_advisor(
            State(state),
            Bytes::from(json!({ "enabled": true, "apiKey": "" }).to_string()),
        )
        .await
        .expect_err("refused");

        assert!(
            refusal.message.contains("TypeSafe key"),
            "{}",
            refusal.message
        );
    }

    #[tokio::test]
    async fn a_confident_answer_takes_the_task_off_the_loved_model() {
        let (directory, store) = two_workers();
        love(directory.path(), &format!("fast:{FAST_MODEL}"));
        let state = HttpState::new(store);
        let input = unnamed(directory.path());
        let world = world_of(&state, &input);
        let rules = rules_route(&world, &input);
        assert_eq!(rules.profile_id, "fast");

        let (route, advised) = apply_advice(
            &input,
            &world,
            &RoutePreferences::default(),
            rules,
            advice("deep", DEEP_MODEL, 0.91),
        );

        assert_eq!(route.profile_id, "deep");
        assert_eq!(route.model, DEEP_MODEL);

        assert!(advised.used);
        assert_eq!(advised.confidence, 0.91);
        assert_eq!(advised.ignored_because, None);
        assert!(
            route.reason.contains("picked for this brief"),
            "{}",
            route.reason
        );
    }

    #[tokio::test]
    async fn an_unsure_answer_still_decides_the_route() {
        let (directory, store) = two_workers();
        love(directory.path(), &format!("fast:{FAST_MODEL}"));
        let state = HttpState::new(store);
        let input = unnamed(directory.path());
        let world = world_of(&state, &input);
        let rules = rules_route(&world, &input);

        let (route, advised) = apply_advice(
            &input,
            &world,
            &RoutePreferences::default(),
            rules,
            advice("deep", DEEP_MODEL, 0.2),
        );

        assert_eq!(route.profile_id, "deep");
        assert!(advised.used);
    }

    #[tokio::test]
    async fn an_answer_naming_a_worker_that_cannot_run_it_leaves_the_rules_in_charge() {
        let (directory, store) = two_workers();
        love(directory.path(), &format!("fast:{FAST_MODEL}"));
        switch_model(&store, directory.path(), "deep", DEEP_MODEL, false);
        let state = HttpState::new(store);
        let input = unnamed(directory.path());
        let world = world_of(&state, &input);
        let rules = rules_route(&world, &input);

        let (route, advised) = apply_advice(
            &input,
            &world,
            &RoutePreferences::default(),
            rules,
            advice("deep", DEEP_MODEL, 0.99),
        );

        assert_eq!(route.profile_id, "fast");

        assert!(!advised.used);
        assert_eq!(
            advised.ignored_because.as_deref(),
            Some("that worker could not take this task")
        );
    }

    #[tokio::test]
    async fn a_switched_off_worker_is_never_offered_as_a_destination() {
        let (directory, store) = two_workers();
        switch_model(&store, directory.path(), "deep", DEEP_MODEL, false);
        let state = HttpState::new(store);
        let world = world_of(&state, &unnamed(directory.path()));

        let destinations = describe_destinations(&world);

        assert_eq!(destinations.len(), 1);
        assert_eq!(destinations[0].profile_id, "fast");
    }

    #[test]
    fn the_advisor_reads_the_brief_and_the_kind_and_nothing_else() {
        let mut input = unnamed(Path::new("/tmp"));
        assert_eq!(advisor_brief(&input), input.prompt);

        input.kind = Some(oga_domain::WorkKind::Ui);
        assert_eq!(
            advisor_brief(&input),
            "Kind of work: ui\n\nImplement the thing described in the plan."
        );
    }

    #[test]
    fn a_caller_still_sending_difficulty_gets_a_clear_message() {
        let refusal = reject_difficulty(&json!({ "difficulty": "hard" })).unwrap_err();
        assert!(refusal.message.contains("kind"), "{}", refusal.message);
        assert!(refusal.message.contains("effort"), "{}", refusal.message);
        assert!(reject_difficulty(&json!({ "kind": "ui" })).is_ok());
    }
}
