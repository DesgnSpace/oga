//! Difficulty tiers and the model's own default reasoning effort.

use oga_domain::{Difficulty, ModelInfo, RoutePreference};

/// The capability tier each difficulty demands. Declaring too high buys one
/// over-priced success; declaring too low buys a cheap retry, so the default
/// sits low deliberately.
pub fn difficulty_floor(difficulty: Difficulty) -> u8 {
    match difficulty {
        Difficulty::Mechanical => 2,
        Difficulty::Standard => 3,
        Difficulty::Hard => 4,
        Difficulty::Critical => 5,
    }
}

/// How the survivors are ranked, tracking the difficulty in force rather than
/// the class. `balanced` is already cost-led; only the top rung stops caring.
pub fn difficulty_preference(difficulty: Difficulty) -> RoutePreference {
    match difficulty {
        Difficulty::Mechanical | Difficulty::Standard | Difficulty::Hard => {
            RoutePreference::Balanced
        }
        Difficulty::Critical => RoutePreference::Quality,
    }
}

/// Oga's own effort vocabulary, weakest first. Claude's and pi's ladders are
/// built in this order; a provider that publishes its ladder in some other
/// order is re-sorted against this before a loved rule's effort is matched to it.
pub const EFFORT_ORDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// One resolved rung: always a value the provider published, with the wording
/// that says where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedEffort {
    pub effort: Option<String>,
    pub reason: String,
}

/// The model's own resting effort, when the provider publishes one — never
/// inferred from how hard the work reads. This is the last stop once neither
/// the caller nor a loved rule named one.
pub fn default_effort(model: &ModelInfo) -> ProjectedEffort {
    match model.default_effort.clone() {
        Some(effort) => ProjectedEffort {
            reason: format!(
                "{}'s own default reasoning effort for this model",
                model.provider.as_str()
            ),
            effort: Some(effort),
        },
        None => ProjectedEffort {
            effort: None,
            reason: format!(
                "{} publishes no default reasoning effort for this model",
                model.provider.as_str()
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ModelInfoFields, model};
    use oga_domain::{ModelCost, Provider};

    #[test]
    fn reads_the_providers_own_default_when_it_publishes_one() {
        let claude = model(
            "model",
            Provider::Claude,
            "p",
            ModelInfoFields {
                default_effort: Some("medium".into()),
                ..ModelInfoFields::default()
            },
        );
        let projected = default_effort(&claude);
        assert_eq!(projected.effort.as_deref(), Some("medium"));
        assert!(projected.reason.contains("default"));
    }

    #[test]
    fn no_published_default_means_no_effort_flag() {
        let bare = model("m", Provider::OpenCode, "oc", ModelInfoFields::default());
        let projected = default_effort(&bare);
        assert_eq!(projected.effort, None);
        assert!(projected.reason.contains("no default reasoning effort"));
    }

    #[test]
    fn catalog_prices_do_not_affect_the_default() {
        let priced = model(
            "m",
            Provider::Claude,
            "c",
            ModelInfoFields {
                cost: Some(ModelCost {
                    input: 3.0,
                    output: 15.0,
                }),
                default_effort: Some("high".into()),
                ..ModelInfoFields::default()
            },
        );
        assert_eq!(default_effort(&priced).effort.as_deref(), Some("high"));
    }
}
