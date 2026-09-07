//! Task classification, the internal difficulty heuristic and the model's
//! own default effort, routing policy, model selection with quota and
//! availability awareness, provider catalog parsing, and the `/api/models`
//! row shape — the decision half of dispatch.

pub mod catalog;
pub mod classify;
pub mod effort;
pub mod policy;
pub mod selection;
pub mod status;
pub mod traits;
pub mod usage;

pub use catalog::{
    CLAUDE_ALIASES, CLAUDE_EFFORTS, PI_EFFORTS, claude_models, claude_models_from_catalog,
    model_capabilities, parse_antigravity_models, parse_codex_models, parse_opencode_models,
    parse_opencode_v2_models, parse_pi_models, select_model_rows,
};
pub use classify::{TaskDemand, classify_task};
pub use effort::{
    EFFORT_ORDER, ProjectedEffort, default_effort, difficulty_floor, difficulty_preference,
};
pub use policy::{
    AllowedModel, PolicyError, PolicyRoute, RoutingPolicy, load_routing_policy, merge_policies,
    normalize_task_class, routing_policy_from_layers, unoffered_policy_rules,
    unoffered_rule_message,
};
pub use selection::{
    ModelCandidate, ModelNameMatch, ModelRoute, NamedPair, NamedRouteAudit, NoEligibleModel,
    ROUTER_VERSION, RouteError, RoutePreferences, SelectionInputs, ambiguous_message,
    check_named_route, choose_model, not_enabled_message, resolve_model_name,
};
pub use status::format_rfc3339_ms;
pub use status::{
    AvailabilitySource, AvailabilityState, ProfileStatus, normalize_profile_statuses,
};
pub use traits::{CostSource, ModelTraits, model_traits};
pub use usage::{
    Perishable, now_ms, parse_rfc3339_ms, perishability, summarize_usage, weekly_window,
    worst_window_used_percent,
};

#[cfg(test)]
pub(crate) mod test_support {
    use oga_domain::{ModelCost, ModelInfo, ModelInfoSource, Provider};

    /// Optional fields for building a `ModelInfo` in tests; everything absent
    /// stays `None`, and `configured_only` picks a source.
    #[derive(Default)]
    pub struct ModelInfoFields {
        pub cost: Option<ModelCost>,
        pub context_window: Option<u64>,
        pub reasoning: Option<bool>,
        pub efforts: Option<Vec<String>>,
        pub default_effort: Option<String>,
        pub tool_call: Option<bool>,
        pub configured_only: bool,
    }

    pub fn model(
        id: &str,
        provider: Provider,
        profile_id: &str,
        fields: ModelInfoFields,
    ) -> ModelInfo {
        ModelInfo {
            id: id.to_owned(),
            label: id.to_owned(),
            provider,
            profile_id: profile_id.to_owned(),
            source: if fields.configured_only {
                ModelInfoSource::Configured
            } else {
                ModelInfoSource::Discovered
            },
            cost: fields.cost,
            context_window: fields.context_window,
            reasoning: fields.reasoning,
            efforts: fields.efforts,
            default_effort: fields.default_effort,
            tool_call: fields.tool_call,
        }
    }
}
