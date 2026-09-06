//! Model selection: from a prompt and the connected accounts to one profile,
//! model, effort, and the reasons that put it there.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use oga_config::{
    LoveDestination, LoveRule, ResolvedModelSettings, model_enabled, profile_enabled,
};
use oga_domain::{
    Difficulty, ModelInfo, Profile, ProfileUsage, RoutePreference, SelectionRejection,
    SelectionRelaxation, SelectionStage, TaskClass, TaskTopic, WorkKind,
};

use crate::classify::{TaskDemand, classify_task};
use crate::effort::{
    EFFORT_ORDER, difficulty_floor, difficulty_preference, heuristic_note, project_effort,
};
use crate::policy::{RoutingPolicy, unoffered_rule_message};
use crate::status::{AvailabilityState, ProfileStatus};
use crate::traits::{CostSource, ModelTraits, model_traits};
use crate::usage::{
    LOW_HEADROOM_PERCENT, NEAR_EXHAUSTED_PERCENT, now_ms, perishability, worst_window_used_percent,
};

/// Bumped whenever selection changes shape, so old records stay interpretable.
pub const ROUTER_VERSION: u32 = 3;

/// How many rejections ride a response and a task record. The catalogs run to
/// dozens of models per account, and a list of every model the policy does not
/// allow is noise in both places.
const MAX_REJECTED: usize = 12;

const STAGE_ORDER: [SelectionStage; 8] = [
    SelectionStage::Floor,
    SelectionStage::Quota,
    SelectionStage::Availability,
    SelectionStage::Catalog,
    SelectionStage::Policy,
    SelectionStage::Settings,
    SelectionStage::Capability,
    SelectionStage::Profile,
];

/// Where a candidate sits once the cheap gates have run over it.
struct Screened<'a> {
    model: &'a ModelInfo,
    allowed: bool,
    status: Option<&'a ProfileStatus>,
    used: Option<f64>,
}

/// One configuration of the two constraints selection is allowed to drop.
#[derive(Clone, Copy)]
struct Attempt {
    policy: bool,
    quota: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RoutePreferences {
    pub preference: Option<RoutePreference>,
    pub model_hint: Option<String>,
    pub difficulty: Option<Difficulty>,
    /// The subject of the work, named by the caller. Wins over whatever the
    /// prompt reads like; absent means the prompt's own signals decide.
    pub topic: Option<TaskTopic>,
    /// Restrict routing to one profile the caller already named, leaving only
    /// the model to choose. Within a named profile the policy allow order
    /// wins, since the caller picked the account and wants its best model for
    /// this class.
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelCandidate {
    pub profile_id: String,
    pub model: String,
    pub score: f64,
    pub traits: ModelTraits,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelRoute {
    pub profile_id: String,
    pub model: String,
    pub preference: RoutePreference,
    pub task_class: TaskClass,
    pub difficulty: Difficulty,
    /// Lowest capability tier this task may run on: the difficulty's tier, or
    /// the project policy's, whichever is higher. Lowered only when nothing
    /// clears it.
    pub floor: u8,
    /// Constraints dropped, in the order they may be dropped, to reach any
    /// destination at all. Empty on the ordinary path.
    pub relaxed: Vec<SelectionRelaxation>,
    /// False when the prompt read wanted a stronger tier than the declared
    /// difficulty allows. Never overrides the declaration; it only records it.
    pub heuristic_agreed: bool,
    pub effort: Option<String>,
    pub effort_reason: String,
    pub reason: String,
    pub candidates: Vec<ModelCandidate>,
    pub rejected: Vec<SelectionRejection>,
    pub rejected_count: usize,
    /// Worst usage window on the chosen account; null when its provider
    /// reports none.
    pub quota_used_percent: Option<f64>,
    pub warnings: Vec<String>,
}

/// Every candidate was filtered out. Carries the per-candidate reasons and the
/// earliest time any of them comes back, because a caller that knows an
/// account frees up in forty minutes can wait instead of guessing at another
/// one.
#[derive(Debug, Clone, PartialEq)]
pub struct NoEligibleModel {
    pub message: String,
    pub rejected: Vec<SelectionRejection>,
    pub earliest_retry_at: Option<String>,
}

impl NoEligibleModel {
    pub const CODE: &str = "no_eligible_model";
}

impl std::fmt::Display for NoEligibleModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RouteError {
    #[error(
        "No model is turned on for {0}, so this task was not sent anywhere. Open Settings, find \
         {0} under Workers, and switch on the models it may use."
    )]
    EmptyProfile(String),
    #[error("no model matches hint: {0}")]
    UnknownHint(String),
    #[error("{}", oga_config::model_not_enabled_message(profile_id, model))]
    ModelNotEnabled { profile_id: String, model: String },
    /// A resolved model the caller could reach, but has not turned on. Built
    /// with [`not_enabled_message`], which names what the caller typed, what
    /// it resolved to, and what is actually on — the extra context only an
    /// entry point holding the catalog and settings together can give.
    #[error("{0}")]
    ResolvedModelNotEnabled(String),
    /// The caller's name matches more than one model in this catalog, so
    /// guessing would silently run the wrong one.
    #[error("{}", ambiguous_message(profile_id, model, candidates))]
    AmbiguousModel {
        profile_id: String,
        model: String,
        candidates: Vec<String>,
    },
    #[error("{0}")]
    NoEligibleModel(NoEligibleModel),
}

impl RouteError {
    pub const MODEL_NOT_ENABLED: &str = "model_not_enabled";
    pub const AMBIGUOUS_MODEL: &str = "ambiguous_model";

    pub fn code(&self) -> &'static str {
        match self {
            RouteError::NoEligibleModel(_) => NoEligibleModel::CODE,
            RouteError::ModelNotEnabled { .. } | RouteError::ResolvedModelNotEnabled(_) => {
                Self::MODEL_NOT_ENABLED
            }
            RouteError::AmbiguousModel { .. } => Self::AMBIGUOUS_MODEL,
            _ => "route_error",
        }
    }
}

/// What resolving a caller's model name against one worker's catalog found.
/// The exact id Settings shows always wins; failing that, the short trailing
/// name Settings displays resolves if it names exactly one model.
pub enum ModelNameMatch<'a> {
    Resolved(&'a ModelInfo),
    Ambiguous(Vec<&'a ModelInfo>),
    Unknown,
}

/// The one resolution rule every entry point applies: a caller may name a
/// model the way Settings shows it, including the short trailing name, but
/// only when that names exactly one model here.
pub fn resolve_model_name<'a>(models: &[&'a ModelInfo], hint: &str) -> ModelNameMatch<'a> {
    match match_hint(models, hint) {
        matched if matched.is_empty() => ModelNameMatch::Unknown,
        matched if matched.len() == 1 => ModelNameMatch::Resolved(matched[0]),
        matched => ModelNameMatch::Ambiguous(matched),
    }
}

/// What someone is told when the model they named names more than one entry
/// in this catalog: every candidate, so they can say exactly which one.
pub fn ambiguous_message(profile_id: &str, typed: &str, candidates: &[String]) -> String {
    format!(
        "{typed} names more than one model for {profile_id}: {}. Say which one, the way \
         Settings shows it.",
        candidates.join(", ")
    )
}

/// What someone is told when the model they named resolves to one that is
/// switched off: what they typed, what it resolved to (when that differs),
/// and what is actually on for this worker, so they do not have to open
/// Settings just to find that out.
pub fn not_enabled_message(profile_id: &str, typed: &str, resolved: &str, on: &[String]) -> String {
    let named = if typed == resolved {
        resolved.to_owned()
    } else {
        format!("{resolved} (you typed \"{typed}\")")
    };
    let on_list = if on.is_empty() {
        "No models are turned on for it.".to_owned()
    } else {
        format!("Models turned on for it: {}.", on.join(", "))
    };
    format!(
        "{named} is not turned on for {profile_id}, so this task was not sent anywhere. \
         {on_list} Open Settings, find {profile_id} under Workers, and switch {resolved} on."
    )
}

/// Everything selection reads about the world beyond catalogs and profiles.
/// The policy comes from the task's cwd, so it is the workspace's own files
/// that decide, never the directory the broker happened to be launched from.
pub struct SelectionInputs<'a> {
    pub statuses: &'a [ProfileStatus],
    pub policy: Option<&'a RoutingPolicy>,
    pub usage: &'a [ProfileUsage],
    pub settings: &'a ResolvedModelSettings,
}

impl<'a> SelectionInputs<'a> {
    pub fn new(settings: &'a ResolvedModelSettings) -> Self {
        SelectionInputs {
            statuses: &[],
            policy: None,
            usage: &[],
            settings,
        }
    }

    pub fn statuses(mut self, statuses: &'a [ProfileStatus]) -> Self {
        self.statuses = statuses;
        self
    }

    pub fn policy(mut self, policy: &'a RoutingPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    pub fn usage(mut self, usage: &'a [ProfileUsage]) -> Self {
        self.usage = usage;
        self
    }
}

/// Choose where work runs. Every filter stage records why it removed what it
/// removed; nothing is silently skipped.
pub fn choose_model(
    prompt: &str,
    models: &[ModelInfo],
    profiles: &[Profile],
    options: &RoutePreferences,
    extra: &SelectionInputs,
) -> Result<ModelRoute, RouteError> {
    let demand = classify_task(prompt);
    let difficulty = options.difficulty.unwrap_or(demand.difficulty);
    let topic = options.topic.or(demand.topic);
    let policy_route = extra
        .policy
        .and_then(|policy| policy.route_for_task(demand.task_class));
    let preference = options
        .preference
        .or(policy_route.and_then(|route| route.preference))
        .unwrap_or_else(|| difficulty_preference(difficulty));
    let declared_floor = difficulty_floor(difficulty);
    let floor = declared_floor.max(
        policy_route
            .and_then(|route| route.min_quality)
            .unwrap_or(0),
    );
    let heuristic_agreed = difficulty_floor(demand.difficulty) <= declared_floor;
    let mut warnings: Vec<String> = Vec::new();
    let mut rejected: Vec<SelectionRejection> = Vec::new();

    let enabled: HashSet<&str> = profiles
        .iter()
        .filter(|profile| profile.enabled)
        .map(|profile| profile.id.as_str())
        .collect();
    // Settings are the person's own choice, so they bound the whole catalog:
    // an entry no scope allows cannot make an allow-list finding either.
    let catalog: Vec<&ModelInfo> = models
        .iter()
        .filter(|model| {
            enabled.contains(model.profile_id.as_str())
                && model_enabled(extra.settings, &model.profile_id, &model.id)
        })
        .collect();
    let base: Vec<&ModelInfo> = models
        .iter()
        .filter(|model| {
            if let Some(profile_id) = &options.profile_id
                && model.profile_id != *profile_id
            {
                return false;
            }
            if !enabled.contains(model.profile_id.as_str()) {
                reject(
                    &mut rejected,
                    model,
                    SelectionStage::Profile,
                    "account is disabled",
                    None,
                );
                return false;
            }
            if !profile_enabled(extra.settings, &model.profile_id) {
                reject(
                    &mut rejected,
                    model,
                    SelectionStage::Settings,
                    "this worker is turned off in Settings",
                    None,
                );
                return false;
            }
            if !model_enabled(extra.settings, &model.profile_id, &model.id) {
                reject(
                    &mut rejected,
                    model,
                    SelectionStage::Settings,
                    "not turned on for this worker in Settings",
                    None,
                );
                return false;
            }
            if model.tool_call == Some(false) {
                reject(
                    &mut rejected,
                    model,
                    SelectionStage::Capability,
                    "model cannot call tools",
                    None,
                );
                return false;
            }
            if not_a_text_model(&model.id) {
                reject(
                    &mut rejected,
                    model,
                    SelectionStage::Capability,
                    "not a text model",
                    None,
                );
                return false;
            }
            true
        })
        .collect::<Vec<_>>();
    if base.is_empty()
        && let Some(profile_id) = &options.profile_id
    {
        return Err(RouteError::EmptyProfile(profile_id.clone()));
    }
    let requested: Vec<&ModelInfo> = match &options.model_hint {
        Some(hint) => {
            let matched = match_hint(&base, hint);
            if matched.is_empty() {
                // The hint may well name a real model the user simply never
                // turned on. Saying so beats "no such model", which sends them
                // looking for a typo that is not there.
                let offered: Vec<&ModelInfo> = models
                    .iter()
                    .filter(|model| {
                        enabled.contains(model.profile_id.as_str())
                            && options
                                .profile_id
                                .as_ref()
                                .is_none_or(|named| model.profile_id == *named)
                    })
                    .collect();
                if let Some(model) = match_hint(&offered, hint)
                    .into_iter()
                    .find(|model| !model_enabled(extra.settings, &model.profile_id, &model.id))
                {
                    return Err(RouteError::ModelNotEnabled {
                        profile_id: model.profile_id.clone(),
                        model: model.id.clone(),
                    });
                }
                return Err(RouteError::UnknownHint(hint.clone()));
            }
            matched
        }
        None => base,
    };

    // The allow list is the authority on what may run, so a rule naming a
    // model no account actually offers is a config problem the user has to
    // see: it silently shrinks the choice for this whole class of work.
    // Reported after the per-model findings, which are the ones that explain
    // this dispatch.
    let config_warnings: Vec<String> = policy_route
        .map(|route| {
            route
                .unoffered_rules(catalog.iter().copied())
                .iter()
                .map(|rule| unoffered_rule_message(demand.task_class, rule))
                .collect()
        })
        .unwrap_or_default();

    // A provider that reports no usage at all — opencode and pi report none —
    // is unknown headroom, so it is neither filtered nor penalised. Headroom
    // is read per model, because a provider that meters one model family
    // separately reports a window that governs that family and no other.
    let status_by_model: HashMap<(&str, &str), &ProfileStatus> = extra
        .statuses
        .iter()
        .map(|status| ((status.profile.as_str(), status.model.as_str()), status))
        .collect();
    let usage_by_profile: HashMap<&str, &ProfileUsage> = extra
        .usage
        .iter()
        .map(|row| (row.profile.as_str(), row))
        .collect();
    let screened: Vec<Screened> = requested
        .iter()
        .map(|model| {
            let used = usage_by_profile
                .get(model.profile_id.as_str())
                .and_then(|row| worst_window_used_percent(row, Some(model.id.as_str())));
            Screened {
                model,
                allowed: policy_route
                    .is_none_or(|route| route.model_allowed(model.provider.as_str(), &model.id)),
                status: status_by_model
                    .get(&(model.profile_id.as_str(), model.id.as_str()))
                    .copied(),
                used,
            }
        })
        .collect();
    let mut used_by_candidate: HashMap<(&str, &str), f64> = HashMap::new();
    for item in &screened {
        if let Some(used) = item.used {
            used_by_candidate.insert(
                (item.model.profile_id.as_str(), item.model.id.as_str()),
                used,
            );
        }
    }
    let used_by = |model: &ModelInfo| -> Option<f64> {
        used_by_candidate
            .get(&(model.profile_id.as_str(), model.id.as_str()))
            .copied()
    };

    // Only automatic routing may lose a candidate to quota. A caller that
    // named the account gets the warning and keeps the dispatch.
    let quota_binds = options.profile_id.is_none();
    let is_exhausted = |item: &Screened| item.used.unwrap_or(0.0) >= NEAR_EXHAUSTED_PERCENT;
    let passes = |item: &Screened, attempt: Attempt| -> bool {
        (!attempt.policy || item.allowed)
            && item
                .status
                .is_none_or(|status| status.state != AvailabilityState::Unavailable)
            && (!attempt.quota || !quota_binds || !is_exhausted(item))
    };

    // A love rule is the caller's standing answer to the question selection
    // would otherwise ask for this kind of work, so it stands in for the class
    // policy the way a named model does — the class still prices the effort
    // unless the rule set one. The chain reads top to bottom: the first
    // destination that can take the work runs it, and whatever was skipped
    // says so on the way past. It gives way the moment no destination can
    // take the work: the point of loving a model is to stop choosing, not to
    // buy a way for a dispatch to fail.
    if let Some(loved) = extra.settings.love.for_task(demand.task_class, topic)
        && options.model_hint.is_none()
        && options.profile_id.is_none()
    {
        let defaults: HashMap<&str, &str> = profiles
            .iter()
            .map(|profile| (profile.id.as_str(), profile.default_model.as_str()))
            .collect();
        let default_of = |profile_id: &str| defaults.get(profile_id).copied();
        let model_matches = |destination: &LoveDestination, profile_id: &str, model: &str| {
            destination.matches(profile_id, model, default_of(profile_id))
        };
        // Why one destination was not among the candidates. The screening
        // pass has already said this per model, so its wording is reused
        // rather than guessed at a second time; nothing said means no account
        // listed the model at all.
        let skip_reason = |destination: &LoveDestination| -> String {
            rejected
                .iter()
                .find(|row| {
                    destination.matches(&row.profile_id, &row.model, default_of(&row.profile_id))
                })
                .map(|row| row.reason.clone())
                .unwrap_or_else(|| "no connected account offers it".into())
        };
        let mut skipped: Vec<(String, String)> = Vec::new();
        let mut pick: Option<(&Screened, usize)> = None;
        for (index, destination) in loved.destinations.iter().enumerate() {
            let candidate = screened
                .iter()
                .filter(|item| model_matches(destination, &item.model.profile_id, &item.model.id))
                .min_by(|a, b| a.used.unwrap_or(0.0).total_cmp(&b.used.unwrap_or(0.0)));
            let Some(candidate) = candidate else {
                skipped.push((destination.label(), skip_reason(destination)));
                continue;
            };
            if candidate.status.map(|s| s.state) == Some(AvailabilityState::Unavailable) {
                skipped.push((
                    destination.label(),
                    candidate
                        .status
                        .map(|s| s.reason.clone())
                        .expect("state checked"),
                ));
                continue;
            }
            if is_exhausted(candidate) {
                skipped.push((
                    destination.label(),
                    format!(
                        "{}% of its usage window is spent",
                        candidate.used.expect("exhausted")
                    ),
                ));
                continue;
            }
            pick = Some((candidate, index));
            break;
        }
        if let Some((pick, index)) = pick {
            let ctx = LovedContext {
                demand: &demand,
                difficulty,
                preference,
                floor,
                heuristic_agreed,
                options,
                topic,
                index,
                skipped,
            };
            return Ok(finish_loved_route(
                pick,
                loved,
                ctx,
                &mut warnings,
                &rejected,
            ));
        }
        let matched = loved.matching_kind(demand.task_class, topic);
        warnings.push(loved_miss_warning(
            loved,
            matched,
            &skipped,
            demand.task_class,
        ));
    }

    // Nothing clearing every constraint is a reason to drop one, in a fixed
    // order: the project's own allow list before the usage cutoff, since
    // running outside the rules beats a run that dies part-way through. Each
    // is tried alone before both go, so neither is dropped when it was not the
    // thing in the way. A model hint is narrow enough that failing on it says
    // more than dodging the policy would, so a hint holds the allow list
    // fixed.
    let may_drop_policy = policy_route.is_some() && options.model_hint.is_none();
    let mut attempts = vec![Attempt {
        policy: true,
        quota: true,
    }];
    if may_drop_policy {
        attempts.push(Attempt {
            policy: false,
            quota: true,
        });
    }
    attempts.push(Attempt {
        policy: true,
        quota: false,
    });
    if may_drop_policy {
        attempts.push(Attempt {
            policy: false,
            quota: false,
        });
    }
    let attempt = attempts
        .iter()
        .find(|candidate| screened.iter().any(|item| passes(item, **candidate)));
    // Nothing survived any of them: report against the strictest, which is the
    // one whose rejections say what the caller would have to change.
    let applied = attempt.copied().unwrap_or(attempts[0]);

    let mut relaxed: Vec<SelectionRelaxation> = Vec::new();
    let mut usable: Vec<&ModelInfo> = Vec::new();
    let mut policy_excluded = 0usize;
    for item in &screened {
        if let Some(found) = attempt
            && passes(item, *found)
        {
            usable.push(item.model);
            continue;
        }
        if applied.policy && !item.allowed {
            policy_excluded += 1;
            reject(
                &mut rejected,
                item.model,
                SelectionStage::Policy,
                &format!(
                    "not allowed for {} work by the project's routing policy ([routes] in .oga.yaml)",
                    demand.task_class.as_str()
                ),
                None,
            );
            continue;
        }
        if let Some(status) = item.status
            && status.state == AvailabilityState::Unavailable
        {
            warnings.push(format!(
                "excluded model {}/{}: {}{}",
                item.model.profile_id,
                item.model.id,
                status.reason,
                status
                    .retry_at
                    .as_ref()
                    .map(|retry| format!("; retry at {retry}"))
                    .unwrap_or_default()
            ));
            reject(
                &mut rejected,
                item.model,
                SelectionStage::Availability,
                &status.reason,
                status.retry_at.clone(),
            );
            continue;
        }
        reject(
            &mut rejected,
            item.model,
            SelectionStage::Quota,
            &format!("{}% of the usage window is spent", item.used.unwrap_or(0.0)),
            None,
        );
    }
    if policy_excluded > 0 {
        warnings.push(format!(
            "{} models this project does not allow for {} work were left out",
            policy_excluded,
            demand.task_class.as_str()
        ));
    }
    if attempt.is_some() && !applied.policy && policy_route.is_some() {
        relaxed.push(SelectionRelaxation::Policy);
        warnings.push(format!(
            "no model this project allows for {} work can run right now, so this went to one \
             outside those rules; update the allow list in .oga.yaml, or connect an account \
             offering a model it already names",
            demand.task_class.as_str()
        ));
    }

    // Quota findings read per model but are said per account: a claude profile
    // whose session window covers four models would otherwise repeat one
    // number four times.
    let spent_and_reachable: Vec<&ModelInfo> = screened
        .iter()
        .filter(|item| {
            (!applied.policy || item.allowed)
                && item
                    .status
                    .is_none_or(|status| status.state != AvailabilityState::Unavailable)
                && is_exhausted(item)
        })
        .map(|item| item.model)
        .collect();
    for (profile_id, spent) in group_by_profile(&spent_and_reachable, &used_by) {
        warnings.push(format!(
            "{} has {}% left on the window covering {}, too little to finish a task",
            profile_id,
            100.0
                - spent
                    .iter()
                    .map(|(_, used)| *used)
                    .fold(f64::INFINITY, f64::min),
            spent
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if attempt.is_some() && !applied.quota && quota_binds {
        relaxed.push(SelectionRelaxation::Quota);
        warnings.push(
            "every account that could take this work has almost nothing left in its usage \
             window, so the run may stop part-way through; wait for a window to reset, or \
             connect another account"
                .to_owned(),
        );
    }
    let strained: Vec<&ModelInfo> = usable
        .iter()
        .copied()
        .filter(|model| {
            used_by(model).is_some_and(|used| (75.0..NEAR_EXHAUSTED_PERCENT).contains(&used))
        })
        .collect();
    for (profile_id, spent) in group_by_profile(&strained, &used_by) {
        warnings.push(format!(
            "{} is {}% into the rate-limit window covering {}; deprioritized",
            profile_id,
            spent
                .iter()
                .map(|(_, used)| *used)
                .fold(f64::NEG_INFINITY, f64::max),
            spent
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if usable.is_empty() {
        let mut all_warnings = warnings.clone();
        all_warnings.extend(config_warnings);
        return Err(RouteError::NoEligibleModel(no_eligible_model(
            options.model_hint.as_deref(),
            demand.task_class,
            &all_warnings,
            &rejected,
        )));
    }

    // The floor is what difficulty buys: a tier the work is known to need. It
    // gives way only when nothing clears it, since a run below the intended
    // tier beats no run and the record says which happened.
    let usable_traits: Vec<ModelTraits> = usable.iter().map(|model| model_traits(model)).collect();
    let quality_at = |index: usize| usable_traits[index].quality;
    let clearing_indices = |floor: u8| -> Vec<usize> {
        (0..usable.len())
            .filter(|index| quality_at(*index) >= floor)
            .collect()
    };
    let mut effective_floor = floor;
    let mut clearing = clearing_indices(effective_floor);
    while clearing.is_empty() && effective_floor > 1 {
        effective_floor -= 1;
        clearing = clearing_indices(effective_floor);
    }
    if effective_floor < floor {
        relaxed.insert(0, SelectionRelaxation::Floor);
        warnings.push(format!(
            "no connected account offers a model strong enough for {} work; ran the strongest \
             one available",
            difficulty.as_str()
        ));
    }
    for (index, model) in usable.iter().enumerate() {
        let quality = usable_traits[index].quality;
        if quality < effective_floor {
            reject(
                &mut rejected,
                model,
                SelectionStage::Floor,
                &format!(
                    "tier {quality} of 5 is below what {} work needs",
                    difficulty.as_str()
                ),
                None,
            );
        }
    }

    // Rank only bites when the caller named the profile. Left flat otherwise,
    // so automatic routing stays score-driven and the rate-limit penalty still
    // wins.
    let rank_of = |candidate: &ModelCandidate| -> usize {
        match (options.profile_id.as_deref(), policy_route) {
            (Some(_), Some(route)) => route
                .allow_rank(candidate.profile_id.as_str(), candidate.model.as_str())
                .unwrap_or(usize::MAX),
            _ => 0,
        }
    };
    // A last-resort tiebreak among candidates the score already called even:
    // the one whose weekly headroom is about to reset unspent is worth
    // spending before an equally-good account that still has days of runway.
    // Never read for a candidate whose usage is unknown, so silence keeps
    // breaking ties the way it always has.
    let clock = now_ms();
    let perishability_of = |candidate: &ModelCandidate| -> Option<crate::usage::Perishable> {
        perishability(
            usage_by_profile.get(candidate.profile_id.as_str()).copied(),
            find_model(models, &candidate.profile_id, &candidate.model)?,
            clock,
        )
    };

    let mut candidates: Vec<ModelCandidate> = clearing
        .iter()
        .map(|index| {
            let model = usable[*index];
            let traits = usable_traits[*index].clone();
            let scored =
                score(&traits, effective_floor, preference) - usage_penalty(used_by(model));
            ModelCandidate {
                profile_id: model.profile_id.clone(),
                model: model.id.clone(),
                score: scored,
                traits,
            }
        })
        .collect();
    candidates.sort_by(|a, b| {
        rank_of(a)
            .cmp(&rank_of(b))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| match (perishability_of(a), perishability_of(b)) {
                (Some(pa), Some(pb)) => pb.score.total_cmp(&pa.score),
                _ => Ordering::Equal,
            })
            .then_with(|| a.profile_id.cmp(&b.profile_id))
            .then_with(|| a.model.cmp(&b.model))
    });
    let selected = &candidates[0];
    if let Some(status) = screened
        .iter()
        .find(|item| {
            item.model.profile_id == selected.profile_id && item.model.id == selected.model
        })
        .and_then(|item| item.status)
        && status.state == AvailabilityState::Unknown
    {
        warnings.push(format!(
            "availability unknown for {}/{}: {}",
            selected.profile_id, selected.model, status.reason
        ));
    }
    if let Some(runner_up) = candidates.get(1)
        && rank_of(selected) == rank_of(runner_up)
        && selected.score == runner_up.score
        && let (Some(winner), Some(loser)) =
            (perishability_of(selected), perishability_of(runner_up))
        && winner.score != loser.score
    {
        warnings.push(format!(
            "{} was preferred over {} on quota timing: its week window has {}% left and \
                     resets at {}, worth spending before that headroom is lost",
            selected.profile_id,
            runner_up.profile_id,
            100.0 - winner.window_used_percent,
            winner.resets_at
        ));
    }
    let selected_profile_id = selected.profile_id.clone();
    let selected_model_id = selected.model.clone();
    let selected_traits = selected.traits.clone();
    let chosen = clearing
        .iter()
        .map(|index| usable[*index])
        .find(|model| model.profile_id == selected_profile_id && model.id == selected_model_id)
        .expect("selected came from the clearing models");
    let projected = project_effort(chosen, difficulty);
    let used = used_by(chosen);
    let mut reason = demand.reason.clone();
    if policy_route.is_some() && applied.policy {
        reason.push_str(&format!(
            "; applied this project's rules for {} work",
            demand.task_class.as_str()
        ));
    }
    reason.push_str(&format!(
        "; selected quality {}/5, cost {}/5, speed {}/5",
        selected_traits.quality, selected_traits.cost, selected_traits.speed
    ));

    if options.difficulty.is_some() && !heuristic_agreed {
        warnings.push(heuristic_note(&demand.reason, difficulty));
    }
    warnings.extend(config_warnings);
    if candidates
        .iter()
        .any(|candidate| candidate.traits.cost_source == CostSource::Unknown)
    {
        warnings
            .push("some candidates have unknown price data; cost was scored neutrally".to_owned());
    }

    Ok(ModelRoute {
        profile_id: selected_profile_id,
        model: selected_model_id,
        preference,
        task_class: demand.task_class,
        difficulty,
        floor: effective_floor,
        relaxed,
        heuristic_agreed,
        effort: projected.effort,
        effort_reason: projected.reason,
        reason,
        candidates: diverse_candidates(candidates, 3),
        rejected: top_rejections(&rejected),
        rejected_count: rejected.len(),
        quota_used_percent: used,
        warnings,
    })
}

/// What the loved path carries from selection into its own route build.
struct LovedContext<'a> {
    demand: &'a TaskDemand,
    difficulty: Difficulty,
    preference: RoutePreference,
    floor: u8,
    heuristic_agreed: bool,
    options: &'a RoutePreferences,
    topic: Option<TaskTopic>,
    /// Which destination in the rule's chain ran, and the earlier ones it
    /// passed on the way there.
    index: usize,
    skipped: Vec<(String, String)>,
}

/// Build the loved-model route once a candidate cleared everything. The class
/// prices the effort unless the rule set one; only the destination was never in
/// question.
fn finish_loved_route<'a>(
    pick: &Screened,
    loved: &LoveRule,
    ctx: LovedContext<'a>,
    warnings: &mut Vec<String>,
    rejected: &[SelectionRejection],
) -> ModelRoute {
    let LovedContext {
        demand,
        difficulty,
        preference,
        floor,
        heuristic_agreed,
        options,
        topic,
        index,
        skipped,
    } = ctx;
    let destination = &loved.destinations[index];
    let traits = model_traits(pick.model);
    let rule_effort = loved_effort(pick.model, destination.effort.as_deref());
    let projected = rule_effort
        .clone()
        .map(|effort| crate::effort::ProjectedEffort {
            effort: Some(effort),
            reason: "the loved model's configured reasoning effort".into(),
        })
        .unwrap_or_else(|| project_effort(pick.model, difficulty));
    if traits.quality < floor {
        warnings.push(format!(
            "the loved model is tier {} of 5, under what {} work usually asks for; it ran \
             anyway because it is the default here",
            traits.quality,
            difficulty.as_str()
        ));
    }
    if let Some(used) = pick.used.filter(|used| *used >= 75.0) {
        warnings.push(format!(
            "{} is {}% into the rate-limit window covering {}; the run may stop part-way through",
            pick.model.profile_id, used, pick.model.id
        ));
    }
    let mut route_warnings = warnings.clone();
    if !skipped.is_empty() {
        let details = skipped
            .iter()
            .map(|(label, reason)| format!("{label} ({reason})"))
            .collect::<Vec<_>>()
            .join("; ");
        route_warnings.push(format!(
            "{} {} loved before {} but could not take this task: {details}; sent here instead",
            skipped
                .iter()
                .map(|(label, _)| label.as_str())
                .collect::<Vec<_>>()
                .join(" and "),
            if skipped.len() == 1 { "is" } else { "are" },
            loved.destinations[index].label(),
        ));
    }
    if options.difficulty.is_some() && !heuristic_agreed {
        route_warnings.push(heuristic_note(&demand.reason, difficulty));
    }
    ModelRoute {
        profile_id: pick.model.profile_id.clone(),
        model: pick.model.id.clone(),
        preference,
        task_class: demand.task_class,
        difficulty,
        floor,
        relaxed: Vec::new(),
        heuristic_agreed,
        effort: projected.effort,
        effort_reason: projected.reason,
        reason: format!(
            "{}; {}",
            demand.reason,
            love_route_reason(
                loved,
                loved.matching_kind(demand.task_class, topic),
                rule_effort.as_deref(),
                index,
            )
        ),
        candidates: vec![ModelCandidate {
            profile_id: pick.model.profile_id.clone(),
            model: pick.model.id.clone(),
            score: score(&traits, floor, preference),
            traits,
        }],
        rejected: top_rejections(rejected),
        rejected_count: rejected.len(),
        quota_used_percent: pick.used,
        warnings: route_warnings,
    }
}

/// Why the route ended here, naming the rule that decided it: the kind of work
/// it claims, or the whole scope when it is the catch-all. A destination past
/// the first says its place in the chain, so the record shows the task did
/// not run where it was first meant to.
fn love_route_reason(
    loved: &LoveRule,
    matched: Option<WorkKind>,
    effort: Option<&str>,
    index: usize,
) -> String {
    let effort = effort.map_or_else(String::new, |effort| format!(" at {effort} effort"));
    let chosen = &loved.destinations[index];
    if index == 0 {
        return match matched {
            None => format!(
                "sent to the loved model, the default {}{effort}",
                if loved.scope == "project" {
                    "for this project"
                } else {
                    "everywhere"
                }
            ),
            Some(kind) => format!(
                "sent to {}, loved for {} work{effort}",
                loved.label(),
                kind.as_str()
            ),
        };
    }
    let place = ordinal(index);
    match matched {
        None => format!(
            "sent to {}, the {place} loved model, the default {}{effort}",
            chosen.label(),
            if loved.scope == "project" {
                "for this project"
            } else {
                "everywhere"
            }
        ),
        Some(kind) => format!(
            "sent to {}, {place} loved choice for {} work{effort}",
            chosen.label(),
            kind.as_str()
        ),
    }
}

/// Second, third, fourth: the place of a destination past the first.
fn ordinal(index: usize) -> String {
    match index {
        1 => "second".into(),
        2 => "third".into(),
        3 => "fourth".into(),
        4 => "fifth".into(),
        _ => format!("{}th", index + 1),
    }
}

/// What the fallback path says when no destination could take the work: the
/// whole chain and why each link failed, so the usual choice it fell back to
/// does not look like the plan.
fn loved_miss_warning(
    loved: &LoveRule,
    matched: Option<WorkKind>,
    skipped: &[(String, String)],
    task_class: TaskClass,
) -> String {
    let usual = format!(
        "this went to the usual choice for {} work instead",
        matched.map_or(task_class.as_str(), WorkKind::as_str)
    );
    if skipped.len() <= 1 {
        let reason = skipped
            .first()
            .map(|(_, reason)| reason.as_str())
            .unwrap_or("no connected account offers it");
        return format!(
            "{} is loved here{} but could not take this task: {reason}; {usual}",
            loved.label(),
            love_kind_suffix(matched),
        );
    }
    let details = skipped
        .iter()
        .map(|(label, reason)| format!("{label} ({reason})"))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "{} are loved here{} but none could take this task: {details}; {usual}",
        loved.chain_label(),
        love_kind_suffix(matched),
    )
}

/// How a warning names the rule that was skipped: the kind of work it claims,
/// or nothing when it takes everything else.
fn love_kind_suffix(matched: Option<WorkKind>) -> String {
    match matched {
        None => String::new(),
        Some(kind) => format!(" for {} work", kind.as_str()),
    }
}

fn loved_effort(model: &ModelInfo, requested: Option<&str>) -> Option<String> {
    let requested = requested?;
    let levels = model.efforts.as_deref()?;
    if levels.is_empty() {
        return None;
    }
    let requested_index = EFFORT_ORDER.iter().position(|level| *level == requested)?;
    levels
        .iter()
        .filter_map(|level| {
            EFFORT_ORDER
                .iter()
                .position(|known| known == &level.as_str())
                .filter(|index| *index <= requested_index)
                .map(|index| (index, level))
        })
        .max_by_key(|(index, _)| *index)
        .map(|(_, level)| level.clone())
        .or_else(|| levels.first().cloned())
}

/// The caller-named pair under audit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NamedPair<'a> {
    pub profile_id: &'a str,
    pub model: &'a str,
    pub difficulty: Option<Difficulty>,
    pub effort: Option<&'a str>,
    pub preference: Option<RoutePreference>,
}

/// What the caller-named path reports without ever blocking it. The same
/// filters automatic routing applies, run as advice: naming a profile and a
/// model is the caller's call, but sending work to an account with revoked
/// credentials should not be silent about it.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedRouteAudit {
    pub task_class: TaskClass,
    pub preference: RoutePreference,
    pub difficulty: Difficulty,
    pub floor: u8,
    pub heuristic_agreed: bool,
    pub effort: Option<String>,
    pub effort_reason: String,
    pub quota_used_percent: Option<f64>,
    pub rejected: Vec<SelectionRejection>,
    pub warnings: Vec<String>,
    /// The id the catalog knows this model by. Equal to the caller's own
    /// string when nothing resolved it to something else — dispatch must use
    /// this, never the caller's raw string, so the model that runs is the one
    /// Settings actually has an opinion about.
    pub resolved_model: String,
}

/// Audit a caller-named profile/model pair. Most findings advise, never
/// block, since naming a profile and a model is the caller's call — but a
/// model this worker has not turned on is refused the same as automatic
/// routing refuses one, because the enabled check is authoritative on every
/// path a task can be dispatched by.
pub fn check_named_route(
    prompt: &str,
    pair: NamedPair,
    models: &[ModelInfo],
    profiles: &[Profile],
    extra: &SelectionInputs,
) -> Result<NamedRouteAudit, RouteError> {
    let statuses = extra.statuses;
    let policy = extra.policy;
    let usage = extra.usage;
    let settings = extra.settings;
    let NamedPair {
        profile_id,
        model: model_id,
        difficulty: declared,
        effort: wanted_effort,
        preference: named_preference,
    } = pair;
    let demand = classify_task(prompt);
    let difficulty = declared.unwrap_or(demand.difficulty);
    let policy_route = policy.and_then(|policy| policy.route_for_task(demand.task_class));
    let declared_floor = difficulty_floor(difficulty);
    let heuristic_agreed = difficulty_floor(demand.difficulty) <= declared_floor;
    let mut warnings: Vec<String> = Vec::new();
    let mut rejected: Vec<SelectionRejection> = Vec::new();
    let mut add = |stage: SelectionStage, reason: String, retry_at: Option<String>| {
        rejected.push(SelectionRejection {
            profile_id: profile_id.to_owned(),
            model: model_id.to_owned(),
            stage,
            reason,
            retry_at,
        });
    };

    let provider = profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .map(|p| p.provider);
    let offered: Vec<&ModelInfo> = models
        .iter()
        .filter(|m| m.profile_id == profile_id)
        .collect();
    // One resolution rule everywhere: the exact id, or its short trailing name
    // when that names exactly one model here. Two or more candidates is a
    // reason to stop rather than guess which one runs.
    let chosen = match resolve_model_name(&offered, model_id) {
        ModelNameMatch::Resolved(model) => Some(model),
        ModelNameMatch::Ambiguous(candidates) => {
            return Err(RouteError::AmbiguousModel {
                profile_id: profile_id.to_owned(),
                model: model_id.to_owned(),
                candidates: candidates.into_iter().map(|m| m.id.clone()).collect(),
            });
        }
        ModelNameMatch::Unknown => None,
    };
    // Everything past here reasons about the id the catalog actually knows,
    // not the name the caller happened to type — the two only differ when a
    // short name resolved to something else, and only the resolved id says
    // anything true about availability, usage, or whether it is turned on.
    let resolved_id: &str = chosen.map_or(model_id, |model| model.id.as_str());
    // Discovery that fails falls back to the one model the profile is
    // configured with, and that list is not evidence about anything else the
    // account offers. Only a list that was actually enumerated can say a model
    // is missing from it.
    let enumerated = offered.len() > 1
        || offered
            .iter()
            .any(|m| m.source == oga_domain::ModelInfoSource::Discovered);
    if chosen.is_none() && enumerated {
        add(
            SelectionStage::Catalog,
            "the account does not list this model".into(),
            None,
        );
        warnings.push(format!(
            "{profile_id} does not list a model called {model_id}. Check what it offers before \
             dispatching, or the run may fail at start."
        ));
    }
    // A model this worker has not turned on is refused here exactly as
    // automatic routing refuses one — the caller named the pair, but that
    // never authorised spending on a model nobody switched on.
    if let Some(model) = chosen
        && !model_enabled(settings, profile_id, &model.id)
    {
        let on: Vec<String> = offered
            .iter()
            .filter(|candidate| model_enabled(settings, profile_id, &candidate.id))
            .map(|candidate| candidate.id.clone())
            .collect();
        return Err(RouteError::ResolvedModelNotEnabled(not_enabled_message(
            profile_id, model_id, &model.id, &on,
        )));
    }

    let status = statuses
        .iter()
        .find(|item| item.profile == profile_id && item.model == resolved_id);
    if let Some(status) = status
        && status.state == AvailabilityState::Unavailable
    {
        add(
            SelectionStage::Availability,
            status.reason.clone(),
            status.retry_at.clone(),
        );
        warnings.push(format!(
            "{profile_id} is unavailable. {}{}. Dispatching anyway.",
            status.reason,
            status
                .retry_at
                .as_ref()
                .map(|retry| format!("; it should answer again after {retry}"))
                .unwrap_or_default()
        ));
    }

    let measured = usage.iter().find(|row| row.profile == profile_id);
    let used = measured.and_then(|row| worst_window_used_percent(row, Some(resolved_id)));
    if let Some(used) = used
        && used >= LOW_HEADROOM_PERCENT
    {
        if used >= NEAR_EXHAUSTED_PERCENT {
            add(
                SelectionStage::Quota,
                format!("{used}% of the usage window is spent"),
                None,
            );
        }
        warnings.push(format!(
            "{profile_id} has {}% left on the window covering {resolved_id}; the run may stop \
             part-way through.",
            100.0 - used
        ));
    }

    if let (Some(route), Some(provider)) = (policy_route, provider)
        && !route.model_allowed(provider.as_str(), resolved_id)
    {
        add(
            SelectionStage::Policy,
            format!(
                "not allowed for {} work by the project's routing policy ([routes] in .oga.yaml)",
                demand.task_class.as_str()
            ),
            None,
        );
        warnings.push(format!(
            "{} model {resolved_id} is not one this project allows for {} work; the explicit \
             choice overrode the project's routing policy ([routes] in .oga.yaml).",
            provider.as_str(),
            demand.task_class.as_str()
        ));
    }

    let projected = chosen.map(|chosen| project_effort(chosen, difficulty));
    let (projected_effort, effort_reason) =
        projected.map(|p| (p.effort, p.reason)).unwrap_or_else(|| {
            (
                None,
                "the account's model list does not cover this model, so its effort levels are \
                 unknown"
                    .to_owned(),
            )
        });
    if let Some(wanted) = wanted_effort
        && let Some(chosen) = chosen
        && let Some(levels) = chosen.efforts.as_deref()
        && !levels.iter().any(|level| level == wanted)
    {
        warnings.push(format!(
            "{resolved_id} accepts {} as reasoning levels, not {wanted}; passing it through as \
             asked.",
            levels.join(", ")
        ));
    }
    if declared.is_some() && !heuristic_agreed {
        warnings.push(heuristic_note(&demand.reason, difficulty));
    }

    Ok(NamedRouteAudit {
        task_class: demand.task_class,
        preference: named_preference
            .or(policy_route.and_then(|route| route.preference))
            .unwrap_or_else(|| difficulty_preference(difficulty)),
        difficulty,
        floor: declared_floor.max(
            policy_route
                .and_then(|route| route.min_quality)
                .unwrap_or(0),
        ),
        heuristic_agreed,
        effort: projected_effort,
        effort_reason,
        quota_used_percent: used,
        rejected: top_rejections(&rejected),
        warnings,
        resolved_model: resolved_id.to_owned(),
    })
}

fn reject(
    rejected: &mut Vec<SelectionRejection>,
    model: &ModelInfo,
    stage: SelectionStage,
    reason: &str,
    retry_at: Option<String>,
) {
    rejected.push(SelectionRejection {
        profile_id: model.profile_id.clone(),
        model: model.id.clone(),
        stage,
        reason: reason.to_owned(),
        retry_at,
    });
}

/// Most-informative first, then capped: a candidate dropped at the floor says
/// more about the decision than the dozens a provider's catalog contributes
/// which the policy was never going to allow.
fn top_rejections(rejected: &[SelectionRejection]) -> Vec<SelectionRejection> {
    let mut sorted: Vec<&SelectionRejection> = rejected.iter().collect();
    sorted.sort_by_key(|row| STAGE_ORDER.iter().position(|stage| stage == &row.stage));
    sorted.into_iter().take(MAX_REJECTED).cloned().collect()
}

fn no_eligible_model(
    model_hint: Option<&str>,
    task_class: TaskClass,
    warnings: &[String],
    rejected: &[SelectionRejection],
) -> NoEligibleModel {
    let mut retry_times: Vec<&str> = rejected
        .iter()
        .filter_map(|row| row.retry_at.as_deref())
        .collect();
    retry_times.sort_unstable();
    let earliest_retry_at = retry_times.first().map(|retry| (*retry).to_owned());
    let detail = if warnings.is_empty() {
        String::new()
    } else {
        format!("; {}", warnings.join("; "))
    };
    let wait = earliest_retry_at
        .as_ref()
        .map(|earliest| format!("; the earliest is available again at {earliest}"))
        .unwrap_or_default();
    // Nothing survived and every rejection was the user's own switch: that is
    // a settings answer, not a routing one, so it gets said plainly instead of
    // buried in a list of per-model reasons.
    let all_turned_off = !rejected.is_empty()
        && rejected
            .iter()
            .all(|row| row.stage == SelectionStage::Settings);
    let message = match model_hint {
        Some(hint) => format!(
            "model hint {hint} has no eligible model for {}{detail}{wait}",
            task_class.as_str()
        ),
        None if all_turned_off => oga_config::NO_MODEL_ENABLED_MESSAGE.to_owned(),
        None => format!("no routable models are available{detail}{wait}"),
    };
    NoEligibleModel {
        message,
        rejected: top_rejections(rejected),
        earliest_retry_at,
    }
}

fn score(traits: &ModelTraits, required_quality: u8, preference: RoutePreference) -> f64 {
    let quality_gap = f64::from(traits.quality) - f64::from(required_quality);
    let fit = if quality_gap < 0.0 {
        50.0 + quality_gap * 30.0
    } else {
        50.0
    };
    let weights = match preference {
        RoutePreference::Quality => (1.0, 1.0, 5.0),
        RoutePreference::Cost => (8.0, 2.0, 0.0),
        RoutePreference::Speed => (2.0, 6.0, 0.0),
        RoutePreference::Balanced => (4.0, 2.0, 0.0),
    };
    fit - f64::from(traits.cost) * weights.0
        + f64::from(traits.speed) * weights.1
        + f64::from(traits.quality) * weights.2
}

fn usage_penalty(used_percent: Option<f64>) -> f64 {
    match used_percent {
        None => 0.0,
        Some(used) if used >= 95.0 => 60.0,
        Some(used) if used >= 90.0 => 40.0,
        Some(used) if used >= 75.0 => 15.0,
        Some(_) => 0.0,
    }
}

/// Keep at most `limit` picks and keep them on distinct accounts where the
/// ranked list offers that, so a caller sees real alternatives rather than one
/// account three times.
fn diverse_candidates(candidates: Vec<ModelCandidate>, limit: usize) -> Vec<ModelCandidate> {
    let mut selected: Vec<ModelCandidate> = Vec::new();
    let mut seen_profiles: HashSet<String> = HashSet::new();
    for candidate in &candidates {
        if !seen_profiles.insert(candidate.profile_id.clone()) {
            continue;
        }
        selected.push(candidate.clone());
        if selected.len() == limit {
            return selected;
        }
    }
    for candidate in &candidates {
        if selected.iter().any(|picked| {
            picked.profile_id == candidate.profile_id && picked.model == candidate.model
        }) {
            continue;
        }
        selected.push(candidate.clone());
        if selected.len() == limit {
            break;
        }
    }
    selected
}

fn match_hint<'a>(models: &[&'a ModelInfo], hint: &str) -> Vec<&'a ModelInfo> {
    let exact: Vec<&ModelInfo> = models.iter().copied().filter(|m| m.id == hint).collect();
    if !exact.is_empty() {
        return exact;
    }
    let wanted = canonical(hint);
    let by_name: Vec<&ModelInfo> = models
        .iter()
        .copied()
        .filter(|m| canonical(m.id.rsplit('/').next().unwrap_or(&m.id)) == wanted)
        .collect();
    if !by_name.is_empty() {
        return by_name;
    }
    models
        .iter()
        .copied()
        .filter(|m| canonical(&m.id).contains(&wanted))
        .collect()
}

fn canonical(value: &str) -> String {
    let stripped: String = value
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if let Some(rest) = stripped.strip_prefix("kimik")
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        return format!("kimi{rest}");
    }
    stripped
}

fn not_a_text_model(id: &str) -> bool {
    const NON_TEXT: [&str; 5] = ["image", "video", "audio", "embedding", "tts"];
    let lowered = id.to_lowercase();
    for word in NON_TEXT {
        let mut from = 0usize;
        while let Some(found) = lowered[from..].find(word) {
            let end = from + found + word.len();
            let next = &lowered[end..];
            if next.is_empty()
                || next.starts_with('-')
                || next.starts_with('/')
                || next.starts_with(':')
            {
                return true;
            }
            from += found + 1;
        }
    }
    false
}

fn find_model<'a>(
    models: &'a [ModelInfo],
    profile_id: &str,
    model_id: &str,
) -> Option<&'a ModelInfo> {
    models
        .iter()
        .find(|model| model.profile_id == profile_id && model.id == model_id)
}

/// Group the models by account in the order the accounts were first seen, so
/// the per-account wording follows the screening order.
fn group_by_profile<'m>(
    models: &[&'m ModelInfo],
    used_by: &dyn Fn(&ModelInfo) -> Option<f64>,
) -> Vec<(String, Vec<(String, f64)>)> {
    let mut groups: Vec<(String, Vec<(String, f64)>)> = Vec::new();
    for model in models {
        let Some(used) = used_by(model) else { continue };
        match groups
            .iter_mut()
            .find(|(profile, _)| *profile == model.profile_id)
        {
            Some((_, spent)) => spent.push((model.id.clone(), used)),
            None => groups.push((model.profile_id.clone(), vec![(model.id.clone(), used)])),
        }
    }
    groups
}
