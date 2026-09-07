//! Selection goldens ported from the TypeScript behavioral suite: what lands
//! where for a given prompt, catalog, policy, and set of constraints.

use std::collections::BTreeMap;

use oga_config::{
    DirectoryModelSettings, LoveDestination, LoveRule, LoveRules, ResolvedModelSettings,
};
use oga_domain::{
    Difficulty, FailureCode, ModelCost, ModelInfo, ModelInfoSource, Profile, ProfileFailure,
    ProfileSuccess, ProfileUsage, Provider, RoutePreference, SelectionStage, TaskClass,
    UsageSource, UsageWindow, UsageWindowKind, WorkKind,
};
use oga_routing::{
    AllowedModel, CLAUDE_EFFORTS, NamedPair, NoEligibleModel, PolicyRoute, RouteError,
    RoutePreferences, RoutingPolicy, SelectionInputs, check_named_route, choose_model,
    classify_task, model_traits, normalize_profile_statuses,
};

fn profile(id: &str, provider: Provider, default_model: &str) -> Profile {
    Profile {
        id: id.into(),
        label: id.into(),
        provider,
        default_model: default_model.into(),
        enabled: true,
        env: BTreeMap::new(),
        capabilities: vec![],
        command: None,
    }
}

fn profiles() -> Vec<Profile> {
    vec![
        profile("claude", Provider::Claude, "sonnet"),
        profile("opencode", Provider::OpenCode, "opencode/big-pickle"),
    ]
}

fn info(
    id: &str,
    provider: Provider,
    profile_id: &str,
    source: ModelInfoSource,
    cost: Option<(f64, f64)>,
) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        label: id.into(),
        provider,
        profile_id: profile_id.into(),
        source,
        cost: cost.map(|(input, output)| ModelCost { input, output }),
        context_window: None,
        reasoning: None,
        efforts: None,
        default_effort: None,
        tool_call: None,
    }
}

fn models() -> Vec<ModelInfo> {
    vec![
        info(
            "haiku",
            Provider::Claude,
            "claude",
            ModelInfoSource::Alias,
            Some((0.2, 1.0)),
        ),
        info(
            "sonnet",
            Provider::Claude,
            "claude",
            ModelInfoSource::Configured,
            Some((3.0, 15.0)),
        ),
        info(
            "opus",
            Provider::Claude,
            "claude",
            ModelInfoSource::Alias,
            Some((15.0, 75.0)),
        ),
        info(
            "opencode/kimi-k3",
            Provider::OpenCode,
            "opencode",
            ModelInfoSource::Discovered,
            Some((0.0, 0.0)),
        ),
        info(
            "opencode/big-pickle",
            Provider::OpenCode,
            "opencode",
            ModelInfoSource::Configured,
            Some((0.0, 0.0)),
        ),
    ]
}

/// Every model in a catalog switched on for the account offering it: what an
/// account someone has actually set up looks like, since a model is
/// unavailable until it is turned on.
fn all_on(models: &[ModelInfo]) -> DirectoryModelSettings {
    let mut settings = DirectoryModelSettings::default();
    for model in models {
        settings
            .profiles
            .entry(model.profile_id.clone())
            .or_default()
            .model_enabled
            .insert(model.id.clone(), true);
    }
    settings
}

fn settings() -> ResolvedModelSettings {
    settings_with(all_on(&models()), None)
}

/// Nothing turned on anywhere: a fresh install, before anyone has chosen.
fn nothing_on() -> ResolvedModelSettings {
    settings_with(DirectoryModelSettings::default(), None)
}

fn spent(profile_id: &str, provider: Provider, used_percent: f64) -> ProfileUsage {
    usage_row(
        profile_id,
        provider,
        if provider == Provider::Claude {
            UsageSource::ClaudeCli
        } else {
            UsageSource::CodexSessionLog
        },
        vec![UsageWindow {
            label: "Current session".into(),
            kind: UsageWindowKind::Session,
            used_percent,
            window_minutes: None,
            resets_at: None,
            resets_text: None,
            model: None,
        }],
    )
}

/// A weekly window some hours from resetting, the shape the perishability
/// tiebreak reads.
fn weekly(
    profile_id: &str,
    provider: Provider,
    used_percent: f64,
    hours_to_reset: f64,
) -> ProfileUsage {
    let resets_at = oga_routing::now_ms() + (hours_to_reset * 3_600_000.0) as i64;
    usage_row(
        profile_id,
        provider,
        if provider == Provider::Claude {
            UsageSource::ClaudeCli
        } else {
            UsageSource::CodexSessionLog
        },
        vec![UsageWindow {
            label: "Current week".into(),
            kind: UsageWindowKind::Week,
            used_percent,
            window_minutes: None,
            resets_at: Some(oga_routing::format_rfc3339_ms(resets_at)),
            resets_text: None,
            model: None,
        }],
    )
}

fn usage_row(
    profile_id: &str,
    provider: Provider,
    source: UsageSource,
    windows: Vec<UsageWindow>,
) -> ProfileUsage {
    ProfileUsage {
        profile: profile_id.into(),
        provider,
        supported: true,
        source,
        windows,
        plan: None,
        observed_at: None,
        reason: None,
        account_failure: None,
        rate_limits_by_model: None,
        observed_rate_limits: None,
    }
}

fn silent(profile_id: &str, provider: Provider) -> ProfileUsage {
    let mut row = usage_row(profile_id, provider, UsageSource::None, vec![]);
    row.supported = false;
    row.reason = Some("usage tracking is not supported".into());
    row
}

fn unavailable(
    profile_id: &str,
    provider: Provider,
    model: &str,
    reason: &str,
    retry_at: Option<&str>,
) -> oga_routing::ProfileStatus {
    oga_routing::ProfileStatus {
        profile: profile_id.into(),
        provider,
        model: model.into(),
        state: oga_routing::AvailabilityState::Unavailable,
        source: oga_routing::AvailabilitySource::Task,
        reason: reason.into(),
        checked_at: "2026-08-05T00:00:00Z".into(),
        retry_at: retry_at.map(String::from),
    }
}

fn policy(routes: &[(&'static str, PolicyRoute)]) -> RoutingPolicy {
    let mut map = BTreeMap::new();
    for (class, route) in routes {
        let task_class = oga_routing::normalize_task_class(class).expect("valid class");
        map.insert(task_class, route.clone());
    }
    RoutingPolicy {
        path: "/project/.oga.yaml".into(),
        sources: vec!["/project/.oga.yaml".into()],
        routes: map,
    }
}

fn allow(provider: &str, model: &str) -> AllowedModel {
    AllowedModel {
        provider: provider.into(),
        model: model.into(),
    }
}

// One account, its models switched on, and no config file at all: what the
// classes are worth has to come out of the shipped tables, because there is
// nothing else to read.
#[test]
fn one_account_routes_every_class_from_defaults_alone() {
    let workers = [profile("claude", Provider::Claude, "sonnet")];
    // What `discover` builds for a claude profile: the configured model plus
    // the CLI's aliases, one shared effort ladder, and no published prices.
    let mut catalog = vec![info(
        "sonnet",
        Provider::Claude,
        "claude",
        ModelInfoSource::Configured,
        None,
    )];
    for alias in ["haiku", "opus", "fable"] {
        catalog.push(info(
            alias,
            Provider::Claude,
            "claude",
            ModelInfoSource::Alias,
            None,
        ));
    }
    for m in &mut catalog {
        m.efforts = Some(CLAUDE_EFFORTS.into_iter().map(String::from).collect());
    }
    let cases = [
        (
            "a rename",
            "Rename this symbol across the package.",
            TaskClass::Mechanical,
            Difficulty::Mechanical,
            "haiku",
        ),
        (
            "a probe",
            "Run these commands and report what you saw.",
            TaskClass::Mechanical,
            Difficulty::Mechanical,
            "haiku",
        ),
        (
            "a draft",
            "Draft the release notes from this changelog.",
            TaskClass::General,
            Difficulty::Standard,
            "sonnet",
        ),
        (
            "an implementation",
            "Implement the feature described in the plan.",
            TaskClass::Build,
            Difficulty::Standard,
            "sonnet",
        ),
        (
            "a code read",
            "Review this codebase and explain how auth works.",
            TaskClass::Context,
            Difficulty::Hard,
            "sonnet",
        ),
        (
            "a design",
            "Design the architecture and root-cause the race condition.",
            TaskClass::Reasoning,
            Difficulty::Critical,
            "fable",
        ),
    ];
    for (label, prompt, task_class, difficulty, model) in cases {
        let route = choose_model(
            prompt,
            &catalog,
            &workers,
            &RoutePreferences::default(),
            &SelectionInputs::new(&settings_with(all_on(&catalog), None)),
        )
        .unwrap();
        assert_eq!(route.task_class, task_class, "{label}");
        assert_eq!(route.difficulty, difficulty, "{label}");
        // No model here publishes a default effort, and nothing about how
        // hard the prompt reads infers one any more.
        assert_eq!(route.effort, None, "{label}");
        assert_eq!(route.model, model, "{label}");
        assert!(route.candidates[0].traits.quality >= 2, "{label}");
        // Nothing was set aside to get here: the defaults reach a real model on
        // their own rather than by giving up on a constraint.
        assert!(route.relaxed.is_empty(), "{label}");
    }

    // Cheap work and open work do not land on the same model.
    let cheap = choose_model(
        "Rename this symbol across the package.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings_with(all_on(&catalog), None)),
    )
    .unwrap();
    let open = choose_model(
        "Design the architecture of the scheduler.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings_with(all_on(&catalog), None)),
    )
    .unwrap();
    assert_ne!(cheap.model, open.model);
    assert!(cheap.candidates[0].traits.quality < open.candidates[0].traits.quality);
}

#[test]
fn the_capability_floor_the_prompt_buys() {
    let catalog = models();
    let workers = profiles();
    let opts = RoutePreferences::default();
    let mechanical = choose_model(
        "Apply this diff.",
        &catalog,
        &workers,
        &opts,
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    assert_eq!(mechanical.floor, 2);
    assert_eq!(mechanical.model, "haiku");

    let critical_prompt = "Design the architecture and root-cause the race condition.";
    let critical = choose_model(
        critical_prompt,
        &catalog,
        &workers,
        &opts,
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    assert_eq!(critical.floor, 5);
    assert_eq!(critical.model, "opencode/kimi-k3");
    let mut floored: Vec<&str> = critical
        .rejected
        .iter()
        .filter(|row| row.stage == SelectionStage::Floor)
        .map(|row| row.model.as_str())
        .collect();
    floored.sort_unstable();
    assert_eq!(floored, vec!["haiku", "opencode/big-pickle", "sonnet"]);
    let haiku = critical
        .rejected
        .iter()
        .find(|row| row.model == "haiku")
        .unwrap();
    assert!(haiku.reason.contains("below what critical work needs"));

    // The floor gives way rather than refusing when nothing clears it.
    let only_small = [info(
        "haiku",
        Provider::Claude,
        "claude",
        ModelInfoSource::Alias,
        Some((0.2, 1.0)),
    )];
    let workers_small = [profile("claude", Provider::Claude, "sonnet")];
    let route = choose_model(
        critical_prompt,
        &only_small,
        &workers_small,
        &opts,
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    assert_eq!(route.model, "haiku");
    assert_eq!(route.relaxed, vec![oga_domain::SelectionRelaxation::Floor]);
    assert_eq!(route.floor, 2);
    assert!(
        route
            .warnings
            .iter()
            .any(|w| w.contains("ran the strongest one available"))
    );

    // The project policy raises the floor above what the prompt buys.
    let raised = policy(&[(
        "mechanical",
        PolicyRoute {
            min_quality: Some(4),
            allow: vec![allow("claude", "*")],
            ..PolicyRoute::default()
        },
    )]);
    let route = choose_model(
        "Rename this symbol.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()).policy(&raised),
    )
    .unwrap();
    assert_eq!(route.floor, 4);
    assert_eq!(route.model, "sonnet");
}

// Difficulty is purely the router's own read of the prompt now: nothing a
// caller sends can declare it, so different prompts land on different tiers
// with no agreement bookkeeping to reconcile.
#[test]
fn difficulty_is_read_off_the_prompt_alone() {
    let catalog = models();
    let workers = profiles();
    let route = choose_model(
        "Architect a secure migration and analyze race conditions.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    assert_eq!(route.task_class, TaskClass::Reasoning);
    assert_eq!(route.difficulty, Difficulty::Critical);
    assert_eq!(route.model, "opencode/kimi-k3");

    let build = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    assert_eq!(build.difficulty, Difficulty::Standard);
}

#[test]
fn policy_is_the_authority_and_the_catalog_is_the_reality() {
    let catalog = models();
    let workers = profiles();

    // An allow entry no connected account offers is named; selection uses the
    // rest of the list.
    let unsatisfiable = policy(&[(
        "build",
        PolicyRoute {
            min_quality: Some(4),
            allow: vec![
                allow("claude", "opus-9"),
                allow("opencode", "opencode/kimi-k3"),
            ],
            ..PolicyRoute::default()
        },
    )]);
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()).policy(&unsatisfiable),
    )
    .unwrap();
    assert_eq!(route.model, "opencode/kimi-k3");
    assert!(
        route
            .warnings
            .iter()
            .any(|w| w.contains("claude model opus-9")
                && w.contains("no connected account offers it"))
    );

    // The allow list is set aside rather than leaving the class unroutable.
    let impossible = policy(&[(
        "build",
        PolicyRoute {
            allow: vec![allow("claude", "opus-9")],
            ..PolicyRoute::default()
        },
    )]);
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()).policy(&impossible),
    )
    .unwrap();
    assert_eq!(route.relaxed, vec![oga_domain::SelectionRelaxation::Policy]);
    assert!(
        route
            .warnings
            .iter()
            .any(|w| w.contains("no model this project allows for build work can run right now"))
    );
    assert!(!route.reason.contains("applied this project's rules"));

    // A model hint holds the allow list fixed instead of dodging it.
    let narrow = policy(&[(
        "build",
        PolicyRoute {
            allow: vec![allow("opencode", "opencode/kimi-k3")],
            ..PolicyRoute::default()
        },
    )]);
    let hinted = RoutePreferences {
        model_hint: Some("sonnet".into()),
        ..RoutePreferences::default()
    };
    let failure = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &hinted,
        &SelectionInputs::new(&settings()).policy(&narrow),
    )
    .unwrap_err();
    assert_eq!(failure.code(), NoEligibleModel::CODE);
    let RouteError::NoEligibleModel(no_eligible) = failure else {
        panic!("wrong error")
    };
    assert!(
        no_eligible
            .rejected
            .iter()
            .all(|row| row.stage == SelectionStage::Policy)
    );

    // The usage cutoff gives way only after the allow list has.
    let claude_only = policy(&[(
        "build",
        PolicyRoute {
            allow: vec![allow("claude", "*")],
            ..PolicyRoute::default()
        },
    )]);
    let spent_claude = [spent("claude", Provider::Claude, 99.0)];
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings())
            .policy(&claude_only)
            .usage(&spent_claude),
    )
    .unwrap();
    assert_eq!(route.profile_id, "opencode");
    assert_eq!(route.relaxed, vec![oga_domain::SelectionRelaxation::Policy]);

    let claude_trio = &catalog[..3];
    let solo_worker = vec![profiles()[0].clone()];
    let route = choose_model(
        "Implement the feature.",
        claude_trio,
        &solo_worker,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings())
            .policy(&claude_only)
            .usage(&spent_claude),
    )
    .unwrap();
    assert_eq!(route.profile_id, "claude");
    assert_eq!(route.relaxed, vec![oga_domain::SelectionRelaxation::Quota]);
    assert!(
        route
            .warnings
            .iter()
            .any(|w| w.contains("may stop part-way through"))
    );

    // Allow order decides the model when the caller named only the account.
    let ordered = |first: AllowedModel, second: AllowedModel| {
        policy(&[(
            "build",
            PolicyRoute {
                min_quality: Some(3),
                allow: vec![first, second],
                ..PolicyRoute::default()
            },
        )])
    };
    let kimi_first = ordered(
        allow("opencode", "opencode/kimi-k3"),
        allow("opencode", "opencode/big-pickle"),
    );
    let pickle_first = ordered(
        allow("opencode", "opencode/big-pickle"),
        allow("opencode", "opencode/kimi-k3"),
    );
    let named = RoutePreferences {
        profile_id: Some("opencode".into()),
        ..RoutePreferences::default()
    };
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &named,
        &SelectionInputs::new(&settings()).policy(&kimi_first),
    )
    .unwrap();
    assert_eq!(route.model, "opencode/kimi-k3");
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &named,
        &SelectionInputs::new(&settings()).policy(&pickle_first),
    )
    .unwrap();
    assert_eq!(route.model, "opencode/big-pickle");
}

#[test]
fn remaining_usage_filters_automatic_routing_but_not_a_named_account() {
    let catalog = models();
    let workers = profiles();
    let spent_claude = [spent("claude", Provider::Claude, 99.0)];

    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()).usage(&spent_claude),
    )
    .unwrap();
    assert_eq!(route.profile_id, "opencode");
    assert!(
        route
            .rejected
            .iter()
            .any(|row| row.stage == SelectionStage::Quota && row.profile_id == "claude")
    );
    assert!(
        route
            .warnings
            .iter()
            .any(|w| w.starts_with("claude has 1% left on the window covering"))
    );

    let named = RoutePreferences {
        profile_id: Some("claude".into()),
        ..RoutePreferences::default()
    };
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &named,
        &SelectionInputs::new(&settings()).usage(&spent_claude),
    )
    .unwrap();
    assert_eq!(route.profile_id, "claude");
    assert_eq!(route.quota_used_percent, Some(99.0));
    assert!(
        !route
            .rejected
            .iter()
            .any(|row| row.stage == SelectionStage::Quota)
    );
    assert!(
        route
            .warnings
            .iter()
            .any(|w| w.contains("too little to finish a task"))
    );

    // A provider reporting no usage is unknown headroom, neither spent nor
    // free: it stays selectable and earns no score credit.
    let rows = [
        spent("claude", Provider::Claude, 99.0),
        silent("opencode", Provider::OpenCode),
    ];
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()).usage(&rows),
    )
    .unwrap();
    assert_eq!(route.profile_id, "opencode");
    assert_eq!(route.quota_used_percent, None);
    assert!(
        !route
            .rejected
            .iter()
            .any(|row| row.profile_id == "opencode")
    );
    let without_row = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()).usage(&spent_claude),
    )
    .unwrap();
    assert_eq!(route.candidates[0].score, without_row.candidates[0].score);
}

#[test]
fn perishable_headroom_breaks_score_ties() {
    let workers = [
        profile("claude", Provider::Claude, "sonnet"),
        profile("claude-work", Provider::Claude, "sonnet"),
    ];
    let catalog = [
        info(
            "sonnet",
            Provider::Claude,
            "claude",
            ModelInfoSource::Configured,
            Some((3.0, 15.0)),
        ),
        info(
            "sonnet",
            Provider::Claude,
            "claude-work",
            ModelInfoSource::Configured,
            Some((3.0, 15.0)),
        ),
    ];

    // Two identical accounts: the account is the only thing left to decide.
    let rows = [
        weekly("claude", Provider::Claude, 45.0, 29.0),
        weekly("claude-work", Provider::Claude, 12.0, 156.0),
    ];
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings_with(all_on(&catalog), None)).usage(&rows),
    )
    .unwrap();
    assert_eq!(route.profile_id, "claude");
    assert!(
        route
            .warnings
            .iter()
            .any(|w| w.contains("claude was preferred over claude-work")
                && w.contains("quota timing"))
    );

    // An account past the high-headroom threshold is not a target, however
    // soon it resets.
    let rows = [
        weekly("claude", Provider::Claude, 95.0, 1.0),
        weekly("claude-work", Provider::Claude, 50.0, 100.0),
    ];
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings_with(all_on(&catalog), None)).usage(&rows),
    )
    .unwrap();
    assert_eq!(route.profile_id, "claude-work");

    // A named profile wins regardless of which account is more perishable.
    let named = RoutePreferences {
        profile_id: Some("claude".into()),
        ..RoutePreferences::default()
    };
    let rows = [
        weekly("claude", Provider::Claude, 80.0, 50.0),
        weekly("claude-work", Provider::Claude, 5.0, 10.0),
    ];
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &named,
        &SelectionInputs::new(&settings_with(all_on(&catalog), None)).usage(&rows),
    )
    .unwrap();
    assert_eq!(route.profile_id, "claude");

    // A provider reporting no usage is unaffected by another account's
    // perishability: existing tiebreak order stands.
    let baseline = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings_with(all_on(&catalog), None)),
    )
    .unwrap();
    let rows = [
        weekly("claude-work", Provider::Claude, 20.0, 5.0),
        silent("claude", Provider::Claude),
    ];
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings_with(all_on(&catalog), None)).usage(&rows),
    )
    .unwrap();
    assert_eq!(route.profile_id, baseline.profile_id);
    assert!(!route.warnings.iter().any(|w| w.contains("quota timing")));

    // The quality floor still wins over a perishable account below it.
    let rows = [weekly("claude", Provider::Claude, 5.0, 1.0)];
    let route = choose_model(
        "Design the architecture and root-cause the race condition.",
        &models(),
        &profiles(),
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()).usage(&rows),
    )
    .unwrap();
    assert_eq!(route.model, "opencode/kimi-k3");
}

#[test]
fn when_every_candidate_is_filtered_out_the_failure_says_what_would_help() {
    let statuses = [
        unavailable(
            "claude",
            Provider::Claude,
            "haiku",
            "Observed rate limit",
            Some("2026-08-05T03:50:00.000Z"),
        ),
        unavailable(
            "claude",
            Provider::Claude,
            "sonnet",
            "Observed rate limit",
            Some("2026-08-05T03:20:00.000Z"),
        ),
        unavailable(
            "claude",
            Provider::Claude,
            "opus",
            "Observed rate limit",
            Some("2026-08-05T03:20:00.000Z"),
        ),
        unavailable(
            "opencode",
            Provider::OpenCode,
            "opencode/kimi-k3",
            "Observed billing failure",
            None,
        ),
        unavailable(
            "opencode",
            Provider::OpenCode,
            "opencode/big-pickle",
            "Observed billing failure",
            None,
        ),
    ];
    let failure = choose_model(
        "Implement the feature.",
        &models(),
        &profiles(),
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()).statuses(&statuses),
    )
    .unwrap_err();
    let RouteError::NoEligibleModel(no_eligible) = failure else {
        panic!("wrong error")
    };
    assert_eq!(
        no_eligible.earliest_retry_at.as_deref(),
        Some("2026-08-05T03:20:00.000Z")
    );
    assert_eq!(no_eligible.rejected.len(), 5);
    assert!(
        no_eligible
            .rejected
            .iter()
            .all(|row| row.stage == SelectionStage::Availability)
    );
    assert_eq!(
        no_eligible
            .rejected
            .iter()
            .find(|row| row.profile_id == "opencode")
            .map(|row| row.reason.as_str()),
        Some("Observed billing failure")
    );
    assert!(no_eligible.message.contains("2026-08-05T03:20:00.000Z"));
}

#[test]
fn deprioritizes_profiles_deep_into_a_rate_limit_window() {
    let near_limit = [spent("claude", Provider::Claude, 96.0)];
    let opts = RoutePreferences::default();
    let baseline = choose_model(
        "Rename this variable in two files.",
        &models(),
        &profiles(),
        &opts,
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    assert_eq!(baseline.model, "haiku");
    let route = choose_model(
        "Rename this variable in two files.",
        &models(),
        &profiles(),
        &opts,
        &SelectionInputs::new(&settings()).usage(&near_limit),
    )
    .unwrap();
    assert_eq!(route.profile_id, "opencode");
    assert!(route.warnings.iter().any(|w| {
        w.starts_with("claude is 96% into the rate-limit window covering")
            && w.ends_with("; deprioritized")
    }));
}

#[test]
fn an_exhausted_model_window_leaves_the_accounts_other_models_routable() {
    let opus_exhausted = [usage_row(
        "claude",
        Provider::Claude,
        UsageSource::ClaudeCli,
        vec![
            UsageWindow {
                label: "Current session".into(),
                kind: UsageWindowKind::Session,
                used_percent: 15.0,
                window_minutes: None,
                resets_at: None,
                resets_text: None,
                model: None,
            },
            UsageWindow {
                label: "Current week (Opus)".into(),
                kind: UsageWindowKind::Week,
                used_percent: 99.0,
                window_minutes: None,
                resets_at: None,
                resets_text: None,
                model: Some("opus".into()),
            },
        ],
    )];
    let opts = RoutePreferences::default();
    let route = choose_model(
        "Rename this variable in two files.",
        &models(),
        &profiles(),
        &opts,
        &SelectionInputs::new(&settings()).usage(&opus_exhausted),
    )
    .unwrap();
    assert_eq!(route.model, "haiku");
    assert_eq!(route.quota_used_percent, Some(15.0));
    assert!(
        route
            .rejected
            .iter()
            .any(|row| row.model == "opus" && row.stage == SelectionStage::Quota)
    );
    assert!(
        !route
            .rejected
            .iter()
            .any(|row| row.model == "haiku" && row.stage == SelectionStage::Quota)
    );
}

#[test]
fn model_hints_resolve_and_fail_with_named_reasons() {
    let route = choose_model(
        "Implement the fix.",
        &models(),
        &profiles(),
        &RoutePreferences {
            model_hint: Some("kimi3".into()),
            ..RoutePreferences::default()
        },
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    assert_eq!(route.model, "opencode/kimi-k3");
    assert_eq!(route.profile_id, "opencode");

    let statuses = [unavailable(
        "opencode",
        Provider::OpenCode,
        "opencode/kimi-k3",
        "Observed rate limit",
        Some("2026-07-30T12:10:00.000Z"),
    )];
    let failure = choose_model(
        "Implement the feature.",
        &models(),
        &profiles(),
        &RoutePreferences {
            model_hint: Some("kimi3".into()),
            ..RoutePreferences::default()
        },
        &SelectionInputs::new(&settings()).statuses(&statuses),
    )
    .unwrap_err();
    assert!(failure.to_string().contains(
        "model hint kimi3 has no eligible model for build; excluded model opencode/opencode/kimi-k3: Observed rate limit; retry at 2026-07-30T12:10:00.000Z"
    ));

    let build_policy = policy(&[(
        "build",
        PolicyRoute {
            preference: Some(RoutePreference::Quality),
            min_quality: Some(5),
            allow: vec![allow("claude", "opus"), allow("opencode", "opencode-go/*")],
        },
    )]);
    let failure = choose_model(
        "Implement the feature.",
        &models(),
        &profiles(),
        &RoutePreferences {
            model_hint: Some("sonnet".into()),
            ..RoutePreferences::default()
        },
        &SelectionInputs::new(&settings()).policy(&build_policy),
    )
    .unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("model hint sonnet has no eligible model for build")
    );
}

#[test]
fn local_status_picks_between_profiles_offering_the_same_allowed_model() {
    let mut low = profile("claude", Provider::Claude, "sonnet");
    low.id = "claude-low".into();
    let mut funded = profile("claude", Provider::Claude, "sonnet");
    funded.id = "claude-funded".into();
    let workers = [low, funded];
    let catalog: Vec<ModelInfo> = workers
        .iter()
        .map(|p| {
            info(
                "opus",
                Provider::Claude,
                &p.id,
                ModelInfoSource::Configured,
                Some((15.0, 75.0)),
            )
        })
        .collect();
    let statuses = [
        unavailable(
            "claude-low",
            Provider::Claude,
            "opus",
            "Observed billing failure",
            None,
        ),
        oga_routing::ProfileStatus {
            profile: "claude-funded".into(),
            provider: Provider::Claude,
            model: "opus".into(),
            state: oga_routing::AvailabilityState::Available,
            source: oga_routing::AvailabilitySource::Task,
            reason: "Observed successful generation".into(),
            checked_at: "2026-07-30T12:01:00Z".into(),
            retry_at: None,
        },
    ];
    let build_policy = policy(&[(
        "build",
        PolicyRoute {
            preference: Some(RoutePreference::Quality),
            min_quality: Some(5),
            allow: vec![allow("claude", "opus"), allow("opencode", "opencode-go/*")],
        },
    )]);
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings_with(all_on(&catalog), None))
            .statuses(&statuses)
            .policy(&build_policy),
    )
    .unwrap();
    assert_eq!(route.profile_id, "claude-funded");
    assert!(route.warnings[0].contains("claude-low/opus"));
}

#[test]
fn an_unknown_status_stays_eligible_with_a_warning() {
    let statuses = [oga_routing::ProfileStatus {
        profile: "claude".into(),
        provider: Provider::Claude,
        model: "opus".into(),
        state: oga_routing::AvailabilityState::Unknown,
        source: oga_routing::AvailabilitySource::Configuration,
        reason: "No observed generation outcome".into(),
        checked_at: "2026-07-30T12:00:00Z".into(),
        retry_at: None,
    }];
    let build_policy = policy(&[(
        "build",
        PolicyRoute {
            preference: Some(RoutePreference::Quality),
            min_quality: Some(5),
            allow: vec![allow("claude", "opus"), allow("opencode", "opencode-go/*")],
        },
    )]);
    let opus_only = [models()[2].clone()];
    let solo_worker = vec![profiles()[0].clone()];
    let route = choose_model(
        "Implement the feature.",
        &opus_only,
        &solo_worker,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings())
            .statuses(&statuses)
            .policy(&build_policy),
    )
    .unwrap();
    assert_eq!(route.model, "opus");
    assert!(route.warnings[0].contains("availability unknown"));
}

#[test]
fn missing_price_keeps_candidates_visible_and_scores_cost_neutrally() {
    let unknown = info(
        "sonnet",
        Provider::Claude,
        "claude",
        ModelInfoSource::Configured,
        None,
    );
    let mut catalog = vec![unknown];
    catalog.extend(models().into_iter().skip(3));
    let opts = RoutePreferences {
        preference: Some(RoutePreference::Quality),
        ..RoutePreferences::default()
    };
    let route = choose_model(
        "Review this codebase.",
        &catalog,
        &profiles(),
        &opts,
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    assert!(route.candidates.iter().any(|c| c.profile_id == "claude"));
    assert!(route.candidates.iter().any(|c| c.profile_id == "opencode"));
    assert_eq!(
        route
            .candidates
            .iter()
            .find(|c| c.profile_id == "claude")
            .map(|c| c.traits.cost_source),
        Some(oga_routing::CostSource::Unknown)
    );
    assert!(
        route
            .warnings
            .iter()
            .any(|w| w.contains("unknown price data"))
    );
}

// The model's own default effort applies no matter how hard the prompt
// reads — nothing about the work's difficulty ever picks a rung any more.
#[test]
fn effort_is_read_off_the_chosen_models_own_default() {
    let mut catalog = models();
    for model in &mut catalog {
        if model.profile_id == "claude" {
            model.default_effort = Some("high".into());
        }
    }
    let solo_worker = vec![profiles()[0].clone()];
    let cheap = choose_model(
        "Rename this variable in two files.",
        &catalog,
        &solo_worker,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    let hard = choose_model(
        "Implement the feature.",
        &catalog,
        &solo_worker,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    // Whichever model the floor and preference pick for each prompt, both
    // land on the same default effort — the model's own, not the prompt's.
    assert_eq!(cheap.effort.as_deref(), Some("high"));
    assert_eq!(hard.effort.as_deref(), Some("high"));
    assert!(cheap.effort_reason.contains("default"));
}

fn named_pair<'a>(profile_id: &'a str, model: &'a str) -> NamedPair<'a> {
    NamedPair {
        profile_id,
        model,
        effort: None,
        preference: None,
    }
}

#[test]
fn the_caller_named_pair_is_advised_never_blocked() {
    let catalog = models();
    let workers = profiles();

    // An account with a billing failure warns and still returns a plan.
    let statuses = [unavailable(
        "claude",
        Provider::Claude,
        "opus",
        "Observed billing failure",
        None,
    )];
    let audit = check_named_route(
        "Implement the feature.",
        named_pair("claude", "opus"),
        &catalog,
        &workers,
        &SelectionInputs::new(&settings()).statuses(&statuses),
    )
    .expect("named route");
    assert_eq!(audit.rejected.len(), 1);
    assert_eq!(audit.rejected[0].stage, SelectionStage::Availability);
    assert_eq!(audit.rejected[0].reason, "Observed billing failure");
    assert!(
        audit
            .warnings
            .iter()
            .any(|w| w.contains("claude is unavailable") && w.contains("Dispatching anyway"))
    );

    // A model the account does not list warns without guessing another one.
    let audit = check_named_route(
        "Implement the feature.",
        named_pair("claude", "opus-9"),
        &catalog,
        &workers,
        &SelectionInputs::new(&settings()),
    )
    .expect("named route");
    assert_eq!(
        audit.rejected.iter().map(|r| r.stage).collect::<Vec<_>>(),
        vec![SelectionStage::Catalog]
    );
    assert!(
        audit
            .warnings
            .iter()
            .any(|w| w.contains("does not list a model called opus-9"))
    );
    assert_eq!(audit.effort, None);

    // A catalog that is only the configured fallback claims nothing about the
    // model.
    let fallback = [info(
        "sonnet",
        Provider::Claude,
        "claude",
        ModelInfoSource::Configured,
        None,
    )];
    let audit = check_named_route(
        "Implement the feature.",
        named_pair("claude", "opus"),
        &fallback,
        &workers,
        &SelectionInputs::new(&settings()),
    )
    .expect("named route");
    assert!(audit.rejected.is_empty());
    assert!(audit.warnings.is_empty());

    // A pair outside project policy warns that the choice overrode it.
    let narrow = policy(&[(
        "build",
        PolicyRoute {
            allow: vec![allow("opencode", "opencode/kimi-k3")],
            ..PolicyRoute::default()
        },
    )]);
    let audit = check_named_route(
        "Implement the feature.",
        named_pair("claude", "opus"),
        &catalog,
        &workers,
        &SelectionInputs::new(&settings()).policy(&narrow),
    )
    .expect("named route");
    assert_eq!(
        audit.rejected.iter().map(|r| r.stage).collect::<Vec<_>>(),
        vec![SelectionStage::Policy]
    );
    assert!(
        audit
            .warnings
            .iter()
            .any(|w| w.contains("overrode the project's routing policy"))
    );

    // An account with almost nothing left warns and is still dispatched to.
    let spent_claude = [spent("claude", Provider::Claude, 99.0)];
    let audit = check_named_route(
        "Implement the feature.",
        named_pair("claude", "opus"),
        &catalog,
        &workers,
        &SelectionInputs::new(&settings()).usage(&spent_claude),
    )
    .expect("named route");
    assert_eq!(audit.quota_used_percent, Some(99.0));
    assert_eq!(
        audit.rejected.iter().map(|r| r.stage).collect::<Vec<_>>(),
        vec![SelectionStage::Quota]
    );
    assert!(
        audit
            .warnings
            .iter()
            .any(|w| w.contains("1% left on the window covering opus"))
    );

    // A caller effort the model does not accept is warned about, not dropped.
    let laddered = models()
        .into_iter()
        .map(|mut m| {
            if m.id == "opus" {
                m.efforts = Some(
                    ["low", "medium", "high"]
                        .into_iter()
                        .map(String::from)
                        .collect(),
                );
            }
            m
        })
        .collect::<Vec<_>>();
    let pair = NamedPair {
        effort: Some("max"),
        ..named_pair("claude", "opus")
    };
    let audit = check_named_route(
        "Implement the feature.",
        pair,
        &laddered,
        &workers,
        &SelectionInputs::new(&settings()),
    )
    .expect("named route");
    assert!(
        audit
            .warnings
            .iter()
            .any(|w| w.contains("accepts low, medium, high") && w.contains("passing it through"))
    );

    // A clean named pair produces no findings and still reads the model's
    // own default effort — nothing here comes from how hard the work reads.
    let defaulted = models()
        .into_iter()
        .map(|mut m| {
            if m.id == "opus" {
                m.default_effort = Some("low".into());
            }
            m
        })
        .collect::<Vec<_>>();
    let pair = named_pair("claude", "opus");
    let audit = check_named_route(
        "Rename this symbol in two files.",
        pair,
        &defaulted,
        &workers,
        &SelectionInputs::new(&settings()),
    )
    .expect("named route");
    assert!(audit.rejected.is_empty());
    assert!(audit.warnings.is_empty());
    assert_eq!(audit.effort.as_deref(), Some("low"));
}

#[test]
fn a_switched_off_model_is_refused_even_when_named_explicitly() {
    let catalog = models();
    let workers = profiles();

    let refusal = check_named_route(
        "Implement the feature.",
        named_pair("claude", "opus"),
        &catalog,
        &workers,
        &SelectionInputs::new(&nothing_on()),
    )
    .expect_err("refused");
    assert!(
        refusal
            .to_string()
            .contains("opus is not turned on for claude"),
        "{refusal}"
    );
    assert!(refusal.to_string().contains("Open Settings"), "{refusal}");
}

#[test]
fn a_short_name_resolves_and_switches_on_the_right_model() {
    let catalog = models();
    let workers = profiles();

    let audit = check_named_route(
        "Implement the feature.",
        named_pair("opencode", "big-pickle"),
        &catalog,
        &workers,
        &SelectionInputs::new(&settings()),
    )
    .expect("named route");
    assert_eq!(audit.resolved_model, "opencode/big-pickle");
}

#[test]
fn an_ambiguous_name_is_refused_with_candidates() {
    let catalog = [
        info(
            "opencode/big-pickle",
            Provider::OpenCode,
            "opencode",
            ModelInfoSource::Discovered,
            None,
        ),
        info(
            "opencode-go/big-pickle",
            Provider::OpenCode,
            "opencode",
            ModelInfoSource::Discovered,
            None,
        ),
    ];
    let workers = profiles();

    let refusal = check_named_route(
        "Implement the feature.",
        named_pair("opencode", "big-pickle"),
        &catalog,
        &workers,
        &SelectionInputs::new(&settings()),
    )
    .expect_err("ambiguous");
    assert!(
        refusal.to_string().contains("opencode/big-pickle"),
        "{refusal}"
    );
    assert!(
        refusal.to_string().contains("opencode-go/big-pickle"),
        "{refusal}"
    );
}

fn settings_with(
    global: DirectoryModelSettings,
    project: Option<DirectoryModelSettings>,
) -> ResolvedModelSettings {
    ResolvedModelSettings {
        global,
        project,
        overrides: None,
        love: LoveRules::default(),
    }
}

/// The same settings with named models switched back off.
fn without(
    mut settings: DirectoryModelSettings,
    profile_id: &str,
    models: &[&str],
) -> DirectoryModelSettings {
    let profile = settings.profiles.entry(profile_id.to_owned()).or_default();
    for model in models {
        profile.model_enabled.insert((*model).to_owned(), false);
    }
    settings
}

// Settings are the person's own choice, and the choice has to be made: a model
// runs where it was switched on and nowhere else, whatever it scores.
#[test]
fn model_settings_bound_selection() {
    let catalog = models();
    let workers = profiles();
    let quality = RoutePreferences {
        preference: Some(RoutePreference::Quality),
        ..RoutePreferences::default()
    };

    // Nothing switched on: routing refuses instead of reaching for something.
    let refused = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&nothing_on()),
    )
    .unwrap_err();
    assert_eq!(refused.code(), NoEligibleModel::CODE);
    assert!(refused.to_string().contains("Open Settings"));

    // A model switched off is not a candidate.
    let opus_off = settings_with(without(all_on(&catalog), "claude", &["opus"]), None);
    let route = choose_model(
        "Design the architecture and the migration.",
        &catalog,
        &workers,
        &quality,
        &SelectionInputs::new(&opus_off),
    )
    .unwrap();
    assert_ne!(route.model, "opus");
    assert!(
        route
            .rejected
            .iter()
            .any(|row| row.stage == SelectionStage::Settings && row.model == "opus")
    );

    // Naming one that is off refuses by name; it never lands somewhere else.
    let named = choose_model(
        "Design the architecture and the migration.",
        &catalog,
        &workers,
        &RoutePreferences {
            model_hint: Some("opus".into()),
            ..RoutePreferences::default()
        },
        &SelectionInputs::new(&opus_off),
    )
    .unwrap_err();
    assert_eq!(named.code(), RouteError::MODEL_NOT_ENABLED);
    assert!(
        named
            .to_string()
            .contains("opus is not turned on for claude")
    );

    // Switching it back on makes the same request land on it.
    let route = choose_model(
        "Design the architecture and the migration.",
        &catalog,
        &workers,
        &RoutePreferences {
            model_hint: Some("opus".into()),
            ..RoutePreferences::default()
        },
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    assert_eq!(route.model, "opus");

    // A project's own switch removes a model the global scope allows.
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings_with(
            all_on(&catalog),
            Some(without(
                DirectoryModelSettings::default(),
                "claude",
                &["opus", "sonnet", "haiku"],
            )),
        )),
    )
    .unwrap();
    assert_eq!(route.profile_id, "opencode");

    // Turning a worker off removes every model it offers.
    let mut worker_off = all_on(&catalog);
    worker_off
        .profiles
        .entry("claude".into())
        .or_default()
        .enabled = Some(false);
    let route = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings_with(worker_off, None)),
    )
    .unwrap();
    assert_eq!(route.profile_id, "opencode");

    // Nothing left enabled reports no eligible model rather than picking one
    // anyway.
    let failure = choose_model(
        "Implement the feature.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings_with(
            without(
                without(all_on(&catalog), "claude", &["haiku", "sonnet", "opus"]),
                "opencode",
                &["opencode/kimi-k3", "opencode/big-pickle"],
            ),
            None,
        )),
    )
    .unwrap_err();
    assert_eq!(failure.code(), NoEligibleModel::CODE);
}

fn loved(model_name: &str, profile_id: Option<&str>) -> ResolvedModelSettings {
    love_rules(vec![rule(model_name, profile_id, &[], None)])
}

fn rule(
    model_name: &str,
    profile_id: Option<&str>,
    when: &[WorkKind],
    effort: Option<&str>,
) -> LoveRule {
    LoveRule {
        destinations: vec![LoveDestination {
            profile_id: profile_id.map(String::from),
            model: Some(model_name.into()),
            effort: effort.map(String::from),
        }],
        when: when.to_vec(),
        scope: "project".into(),
    }
}

fn chain(rules: Vec<(&str, Option<&str>, Option<&str>)>, when: &[WorkKind]) -> LoveRule {
    LoveRule {
        destinations: rules
            .into_iter()
            .map(|(model_name, profile_id, effort)| LoveDestination {
                profile_id: profile_id.map(String::from),
                model: Some(model_name.into()),
                effort: effort.map(String::from),
            })
            .collect(),
        when: when.to_vec(),
        scope: "project".into(),
    }
}

fn love_rules(rules: Vec<LoveRule>) -> ResolvedModelSettings {
    ResolvedModelSettings {
        global: all_on(&models()),
        project: None,
        overrides: None,
        love: LoveRules(rules),
    }
}

// A loved model takes work of every class, gives the class the effort, and
// gives way the moment it cannot take the work.
#[test]
fn a_loved_model_changes_the_destination_and_nothing_else() {
    let catalog = models();
    let workers = profiles();
    let architecture = "Architect a secure migration and analyze race conditions.";
    let loved_settings = loved("opencode/big-pickle", Some("opencode"));

    for prompt in [
        architecture,
        "Rename this variable in two files.",
        "Implement the fix.",
        "Read these files and understand how auth works.",
        "Draft the release note.",
    ] {
        let route = choose_model(
            prompt,
            &catalog,
            &workers,
            &RoutePreferences::default(),
            &SelectionInputs::new(&loved_settings),
        )
        .unwrap();
        assert_eq!(route.model, "opencode/big-pickle", "{prompt}");
        assert!(route.reason.contains("loved model"), "{prompt}");
    }

    // With no rule effort, the model's own default sets the thinking level —
    // the same value whether the prompt reads hard or cheap.
    let defaulted = catalog
        .iter()
        .map(|m| {
            let mut m = m.clone();
            if m.id == "opencode/big-pickle" {
                m.default_effort = Some("medium".into());
            }
            m
        })
        .collect::<Vec<_>>();
    let hard = choose_model(
        architecture,
        &defaulted,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&loved_settings),
    )
    .unwrap();
    let cheap = choose_model(
        "Rename this variable in two files.",
        &defaulted,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&loved_settings),
    )
    .unwrap();
    assert_eq!(hard.model, cheap.model);
    assert_eq!(hard.effort.as_deref(), Some("medium"));
    assert_eq!(cheap.effort.as_deref(), Some("medium"));

    // It loses to a model or account the caller named.
    let hinted = RoutePreferences {
        model_hint: Some("haiku".into()),
        ..RoutePreferences::default()
    };
    let route = choose_model(
        architecture,
        &catalog,
        &workers,
        &hinted,
        &SelectionInputs::new(&loved_settings),
    )
    .unwrap();
    assert_eq!(route.model, "haiku");
    assert!(!route.reason.contains("loved"));
    let named = RoutePreferences {
        profile_id: Some("claude".into()),
        ..RoutePreferences::default()
    };
    let route = choose_model(
        architecture,
        &catalog,
        &workers,
        &named,
        &SelectionInputs::new(&loved_settings),
    )
    .unwrap();
    assert_eq!(route.profile_id, "claude");
    assert!(!route.reason.contains("loved"));

    // It gives way when its account is unavailable, naming it and why.
    let statuses = [unavailable(
        "opencode",
        Provider::OpenCode,
        "opencode/big-pickle",
        "the account is out of credits",
        None,
    )];
    let route = choose_model(
        architecture,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&loved_settings).statuses(&statuses),
    )
    .unwrap();
    assert_ne!(route.model, "opencode/big-pickle");
    assert!(!route.reason.contains("loved model"));
    assert!(route.warnings.iter().any(|w| {
        w.contains("opencode/opencode/big-pickle is loved here")
            && w.contains("the account is out of credits")
            && w.contains("usual choice for reasoning work")
    }));

    // It gives way when its usage window is spent.
    let spent_week = [usage_row(
        "opencode",
        Provider::OpenCode,
        UsageSource::None,
        vec![UsageWindow {
            label: "Current week".into(),
            kind: UsageWindowKind::Week,
            used_percent: 99.0,
            window_minutes: None,
            resets_at: None,
            resets_text: None,
            model: None,
        }],
    )];
    let route = choose_model(
        architecture,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&loved_settings).usage(&spent_week),
    )
    .unwrap();
    assert_eq!(route.profile_id, "claude");
    assert!(
        route
            .warnings
            .iter()
            .any(|w| w.contains("99% of its usage window is spent"))
    );

    // It gives way when no connected account offers it.
    let retired = loved("opencode/retired", Some("opencode"));
    let bare = choose_model(
        architecture,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    let route = choose_model(
        architecture,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&retired),
    )
    .unwrap();
    assert_eq!(route.model, bare.model);
    assert!(route.warnings.iter().any(|w| {
        w.contains("opencode/opencode/retired is loved here")
            && w.contains("no connected account offers it")
    }));

    // It says out loud when it runs under the tier the work asks for.
    let route = choose_model(
        architecture,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&loved_settings),
    )
    .unwrap();
    assert_eq!(route.floor, 5);
    assert!(
        route
            .warnings
            .iter()
            .any(|w| w.contains("the loved model is tier 3 of 5"))
    );

    // An entry naming no account matches wherever the model is offered.
    let bare_entry = loved("opencode/kimi-k3", None);
    let route = choose_model(
        architecture,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&bare_entry),
    )
    .unwrap();
    assert_eq!(route.profile_id, "opencode");
    assert_eq!(route.model, "opencode/kimi-k3");

    // It overrides the project's allow list for the class.
    let build_policy = policy(&[(
        "build",
        PolicyRoute {
            preference: Some(RoutePreference::Quality),
            min_quality: Some(5),
            allow: vec![allow("claude", "opus"), allow("opencode", "opencode-go/*")],
        },
    )]);
    let haiku_loved = loved("haiku", Some("claude"));
    let route = choose_model(
        "Implement the fix.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&haiku_loved).policy(&build_policy),
    )
    .unwrap();
    assert_eq!(route.model, "haiku");
    assert!(route.relaxed.is_empty());

    // With none set, routing is exactly what it was.
    let with_empty = choose_model(
        architecture,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings()),
    )
    .unwrap();
    assert_eq!(with_empty.model, bare.model);
    assert_eq!(with_empty.reason, bare.reason);
    assert_eq!(with_empty.warnings, bare.warnings);
    assert!(!bare.reason.contains("loved"));
}

// Love rules split the standing answer by kind of work: the rule claiming the
// class wins, the catch-all takes the rest, and each rule prices its own
// effort.
#[test]
fn love_rules_route_each_kind_of_work_to_its_own_model() {
    let catalog = models()
        .into_iter()
        .map(|mut model| {
            if model.id == "opencode/big-pickle" {
                model.efforts = Some(
                    ["low", "medium", "high", "xhigh", "max"]
                        .into_iter()
                        .map(String::from)
                        .collect(),
                );
            }
            model
        })
        .collect::<Vec<_>>();
    let workers = profiles();
    let architecture = "Architect a secure migration and analyze race conditions.";
    let reading = "Read these files and understand how auth works.";
    let settings = love_rules(vec![
        rule(
            "opencode/big-pickle",
            Some("opencode"),
            &[WorkKind::Context, WorkKind::Mechanical],
            Some("low"),
        ),
        rule(
            "opencode/big-pickle",
            Some("opencode"),
            &[WorkKind::Reasoning, WorkKind::Build],
            Some("max"),
        ),
        rule("haiku", Some("claude"), &[], None),
    ]);

    // The rule that claims the class decides the effort, not the difficulty.
    let cheap = choose_model(
        reading,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(cheap.model, "opencode/big-pickle");
    assert_eq!(cheap.effort.as_deref(), Some("low"));
    assert!(
        cheap
            .reason
            .contains("loved for context work at low effort"),
        "{}",
        cheap.reason
    );

    let thinking = choose_model(
        architecture,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(thinking.model, "opencode/big-pickle");
    assert_eq!(thinking.effort.as_deref(), Some("max"));

    // Work no rule claims falls to the catch-all.
    let other = choose_model(
        "Draft the release note.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(other.model, "haiku");

    // A rule that cannot take the work names itself and the kind it claims.
    let statuses = [unavailable(
        "opencode",
        Provider::OpenCode,
        "opencode/big-pickle",
        "the account is out of credits",
        None,
    )];
    let skipped = choose_model(
        reading,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings).statuses(&statuses),
    )
    .unwrap();
    assert_ne!(skipped.model, "opencode/big-pickle");
    assert!(skipped.warnings.iter().any(|warning| {
        warning.contains("opencode/opencode/big-pickle is loved here for context work")
            && warning.contains("the account is out of credits")
    }));
}

// A rule holds an ordered chain of destinations: the first one that can take
// the work runs it, each with its own effort, and the record says which one
// that was.
#[test]
fn a_love_chain_falls_to_the_next_destination_that_can_take_the_work() {
    let catalog = models()
        .into_iter()
        .map(|mut model| {
            if model.id == "opencode/big-pickle" || model.id == "opus" {
                model.efforts = Some(
                    ["low", "medium", "high", "xhigh", "max"]
                        .into_iter()
                        .map(String::from)
                        .collect(),
                );
            }
            model
        })
        .collect::<Vec<_>>();
    let workers = profiles();
    let reading = "Read these files and understand how auth works.";
    let settings = love_rules(vec![chain(
        vec![
            ("opencode/big-pickle", Some("opencode"), Some("max")),
            ("opus", Some("claude"), Some("low")),
        ],
        &[WorkKind::Context],
    )]);

    // Both up: the first runs, priced by its own effort, with nothing to say.
    let route = choose_model(
        reading,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(route.model, "opencode/big-pickle");
    assert_eq!(route.effort.as_deref(), Some("max"));
    assert!(
        route
            .reason
            .contains("loved for context work at max effort"),
        "{}",
        route.reason
    );
    assert!(
        !route.warnings.iter().any(|w| w.contains("loved before")),
        "{:?}",
        route.warnings
    );

    // The first out of credits: the second runs, and the move is visible.
    let statuses = [unavailable(
        "opencode",
        Provider::OpenCode,
        "opencode/big-pickle",
        "the account is out of credits",
        None,
    )];
    let route = choose_model(
        reading,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings).statuses(&statuses),
    )
    .unwrap();
    assert_eq!(route.profile_id, "claude");
    assert_eq!(route.model, "opus");
    assert_eq!(route.effort.as_deref(), Some("low"));
    assert!(
        route
            .reason
            .contains("second loved choice for context work at low effort"),
        "{}",
        route.reason
    );
    assert!(
        route.warnings.iter().any(|w| {
            w.contains("opencode/opencode/big-pickle is loved before claude/opus")
                && w.contains("the account is out of credits")
        }),
        "{:?}",
        route.warnings
    );

    // A spent usage window moves the work the same way.
    let spent_week = [usage_row(
        "opencode",
        Provider::OpenCode,
        UsageSource::None,
        vec![UsageWindow {
            label: "Current week".into(),
            kind: UsageWindowKind::Week,
            used_percent: 99.0,
            window_minutes: None,
            resets_at: None,
            resets_text: None,
            model: None,
        }],
    )];
    let route = choose_model(
        reading,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings).usage(&spent_week),
    )
    .unwrap();
    assert_eq!(route.model, "opus");
    assert!(
        route
            .reason
            .contains("second loved choice for context work"),
        "{}",
        route.reason
    );

    // Neither can: the usual choice runs, and the warning names the chain.
    let both_down = [
        unavailable(
            "opencode",
            Provider::OpenCode,
            "opencode/big-pickle",
            "the account is out of credits",
            None,
        ),
        unavailable(
            "claude",
            Provider::Claude,
            "opus",
            "Observed rate limit",
            None,
        ),
    ];
    let route = choose_model(
        reading,
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings).statuses(&both_down),
    )
    .unwrap();
    assert_ne!(route.model, "opencode/big-pickle");
    assert_ne!(route.model, "opus");
    assert!(
        route.warnings.iter().any(|w| {
            w.contains("opencode/opencode/big-pickle → claude/opus are loved here for context work")
                && w.contains("none could take this task")
                && w.contains("the account is out of credits")
                && w.contains("Observed rate limit")
        }),
        "{:?}",
        route.warnings
    );
}

// A destination may name a worker alone, standing for its default model.
#[test]
fn a_love_chain_may_leave_the_model_to_the_worker() {
    let catalog = models();
    let workers = profiles();
    let settings = love_rules(vec![LoveRule {
        destinations: vec![LoveDestination {
            profile_id: Some("opencode".into()),
            model: None,
            effort: None,
        }],
        when: vec![],
        scope: "project".into(),
    }]);

    let route = choose_model(
        "Implement the fix.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(route.profile_id, "opencode");
    assert_eq!(route.model, "opencode/big-pickle");
}

// A subject rule outranks a class rule for the same task; the caller's named
// subject outranks the prompt's own signals.
#[test]
fn love_rules_prefer_the_subject_over_the_class() {
    let catalog = models();
    let workers = profiles();
    let settings = love_rules(vec![
        rule(
            "opencode/big-pickle",
            Some("opencode"),
            &[WorkKind::Build],
            None,
        ),
        rule("sonnet", Some("claude"), &[WorkKind::Ui], None),
        rule("haiku", Some("claude"), &[], None),
    ]);

    // Build work about the UI goes to the UI rule, not the build rule.
    let ui = choose_model(
        "Implement the login UI component.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(ui.model, "sonnet");
    assert!(ui.reason.contains("loved for ui work"), "{}", ui.reason);

    // A subject the caller names applies even when the prompt never says so.
    let named = choose_model(
        "Implement the fix.",
        &catalog,
        &workers,
        &RoutePreferences {
            kind: Some(WorkKind::Ui),
            ..RoutePreferences::default()
        },
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(named.model, "sonnet", "{}", named.reason);

    // And it replaces the prompt's own subject rather than adding to it: UI
    // words with a named refactor fall back to the class rule.
    let replaced = choose_model(
        "Implement the login UI component.",
        &catalog,
        &workers,
        &RoutePreferences {
            kind: Some(WorkKind::Refactor),
            ..RoutePreferences::default()
        },
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(replaced.model, "opencode/big-pickle", "{}", replaced.reason);

    // No subject anywhere: the class rule still decides.
    let plain = choose_model(
        "Implement the fix.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(plain.model, "opencode/big-pickle");
}

// A caller that states the class of work reaches its loved model even when
// the prompt's own words would have read as a different class, and the
// reason names the kind as the caller's own word rather than the prompt's.
#[test]
fn a_caller_named_class_reaches_its_loved_model_over_the_prompts_own_read() {
    let catalog = models();
    let workers = profiles();
    let settings = love_rules(vec![
        rule("sonnet", Some("claude"), &[WorkKind::Mechanical], None),
        rule(
            "opencode/big-pickle",
            Some("opencode"),
            &[WorkKind::Build],
            None,
        ),
        rule("haiku", Some("claude"), &[], None),
    ]);

    // The prompt itself reads as build work, so the build rule would win.
    let guessed = choose_model(
        "Implement the thing described in the plan.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(guessed.model, "opencode/big-pickle");

    // The caller says this is mechanical work instead; that rule wins, and
    // the reason credits the caller, not the prompt.
    let named = choose_model(
        "Implement the thing described in the plan.",
        &catalog,
        &workers,
        &RoutePreferences {
            kind: Some(WorkKind::Mechanical),
            ..RoutePreferences::default()
        },
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(named.model, "sonnet", "{}", named.reason);
    assert!(
        named
            .reason
            .contains("loved for mechanical work, the kind you named"),
        "{}",
        named.reason
    );
}

// Nobody named a kind, and the prompt reads as nothing in particular: the
// rule pinned to general is what runs, not the catch-all — general is the
// floor, not one class among equals. A prompt that does read confidently
// still outranks it, with no kind named either way.
#[test]
fn a_blank_kind_over_an_illegible_prompt_floors_at_general() {
    let catalog = models();
    let workers = profiles();
    let settings = love_rules(vec![
        rule("sonnet", Some("claude"), &[WorkKind::General], None),
        rule(
            "opencode/big-pickle",
            Some("opencode"),
            &[WorkKind::Build],
            None,
        ),
        rule("haiku", Some("claude"), &[], None),
    ]);

    // Nothing about this prompt reads as any class in particular.
    let floored = choose_model(
        "Draft the release notes from this changelog.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(floored.model, "sonnet", "{}", floored.reason);
    assert!(
        floored.reason.contains("loved for general work"),
        "{}",
        floored.reason
    );
    assert!(
        !floored.reason.contains("the kind you named"),
        "{}",
        floored.reason
    );

    // The same prompt, still with no stated kind, reading as build work:
    // the classifier's own confident read still wins over the general floor.
    let read = choose_model(
        "Implement the thing described in the plan.",
        &catalog,
        &workers,
        &RoutePreferences::default(),
        &SelectionInputs::new(&settings),
    )
    .unwrap();
    assert_eq!(read.model, "opencode/big-pickle", "{}", read.reason);
}

#[test]
fn classification_matches_the_shipped_signal_table() {
    assert_eq!(
        classify_task("Write a commit message for this diff.").difficulty,
        Difficulty::Mechanical
    );
    let traits = model_traits(&info(
        "opencode-go/deepseek-v4-flash",
        Provider::OpenCode,
        "p",
        ModelInfoSource::Discovered,
        None,
    ));
    assert_eq!(traits.quality, 4);
    assert_eq!(traits.speed, 5);
}

#[test]
fn availability_normalization_feeds_selection_shapes() {
    let p = profile("p", Provider::Claude, "m");
    let m = info(
        "m1",
        Provider::Claude,
        "p",
        ModelInfoSource::Discovered,
        None,
    );
    let failure = ProfileFailure {
        profile_id: "p".into(),
        code: FailureCode::RateLimit,
        message: "limited".into(),
        failed_at: "2026-08-05T03:16:00Z".into(),
        consecutive_failures: 1,
        retry_at: None,
        model: Some("m1".into()),
    };
    let rows = normalize_profile_statuses(
        &[p],
        std::slice::from_ref(&m),
        std::slice::from_ref(&failure),
        &[],
        1_785_900_000_000,
        false,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, oga_routing::AvailabilityState::Unavailable);
    assert_eq!(rows[0].reason, "Observed rate limit");

    let cleared = ProfileSuccess {
        profile_id: "p".into(),
        succeeded_at: "2026-08-06T00:00:00Z".into(),
    };
    let p2 = profile("p", Provider::Claude, "m");
    let rows = normalize_profile_statuses(
        &[p2],
        std::slice::from_ref(&m),
        &[failure],
        &[cleared],
        1_785_900_000_000,
        false,
    );
    // A run that succeeded after the failure was recorded clears it.
    assert_eq!(rows[0].state, oga_routing::AvailabilityState::Available);
    assert_eq!(rows[0].reason, "Observed successful generation");
}
