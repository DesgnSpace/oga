//! Difficulty tiers, effort projection, and the disagreement note.

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

/// Where on a model's own effort ladder each difficulty lands, normalised so
/// ladders of different lengths stay comparable across providers.
fn effort_target(difficulty: Difficulty) -> f64 {
    match difficulty {
        Difficulty::Mechanical => 0.1,
        Difficulty::Standard => 0.3,
        Difficulty::Hard => 0.6,
        Difficulty::Critical => 0.9,
    }
}

/// Oga's own effort vocabulary, weakest first. Claude's and pi's ladders are
/// built in this order; a provider that publishes its ladder in some other
/// order is re-sorted against this before anything is projected onto it.
pub const EFFORT_ORDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// One projected rung: always a value the provider published, with the wording
/// that says how it was read off the ladder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedEffort {
    pub effort: Option<String>,
    pub reason: String,
}

/// The rung of this model's own ladder that the difficulty asks for. Only ever
/// a value the provider published, so a projected effort cannot be one the CLI
/// rejects; a provider with no ladder gets no effort flag rather than an
/// invented rung.
pub fn project_effort(model: &ModelInfo, difficulty: Difficulty) -> ProjectedEffort {
    let Some(published) = model.efforts.as_deref().filter(|levels| !levels.is_empty()) else {
        return ProjectedEffort {
            effort: None,
            reason: format!(
                "{} publishes no reasoning levels for this model",
                model.provider.as_str()
            ),
        };
    };
    let known = published
        .iter()
        .all(|level| EFFORT_ORDER.contains(&level.as_str()));
    // Claude's and pi's ladders are ours and already weakest-first; codex sends
    // its own and the order it uses is not part of its contract. Sorting by
    // Oga's vocabulary makes the projection independent of that order, and a
    // ladder using words outside the vocabulary keeps the published order.
    let mut ladder: Vec<&str> = published.iter().map(String::as_str).collect();
    if known {
        ladder.sort_by_key(|level| EFFORT_ORDER.iter().position(|known| known == level));
    }
    let index = (effort_target(difficulty) * (ladder.len() - 1) as f64).round() as usize;
    let rung = ladder[index];
    ProjectedEffort {
        effort: Some(rung.to_owned()),
        reason: if known {
            format!(
                "{} work sits at {} on this model's {} levels",
                difficulty.as_str(),
                rung,
                ladder.len()
            )
        } else {
            format!(
                "{} work sits at {} in the order {} published",
                difficulty.as_str(),
                rung,
                model.provider.as_str()
            )
        },
    }
}

/// The declaration is never overridden, so the disagreement is said out loud
/// instead: the caller sees its own optimism, and so does whoever reads the
/// task. Only a declaration earns this — warning about the default on every
/// prompt containing "implement" is noise.
pub fn heuristic_note(heuristic_reason: &str, difficulty: Difficulty) -> String {
    format!(
        "this reads like {} but was sent as {} work; raise difficulty if the result comes back thin",
        heuristic_reason,
        difficulty.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ModelInfoFields, model};
    use oga_domain::{ModelCost, Provider};

    fn ladder(efforts: &[&str], provider: Provider) -> ModelInfo {
        model(
            "model",
            provider,
            "p",
            ModelInfoFields {
                efforts: Some(efforts.iter().map(|s| s.to_string()).collect()),
                ..ModelInfoFields::default()
            },
        )
    }

    #[test]
    fn projects_onto_five_and_six_rung_ladders_weakest_first() {
        for efforts in [
            ["low", "medium", "high", "xhigh", "max"].as_slice(),
            ["minimal", "low", "medium", "high", "xhigh", "max"].as_slice(),
        ] {
            let claude = ladder(efforts, Provider::Claude);
            assert_eq!(
                project_effort(&claude, Difficulty::Mechanical)
                    .effort
                    .as_deref(),
                Some("low")
            );
            assert_eq!(
                project_effort(&claude, Difficulty::Standard)
                    .effort
                    .as_deref(),
                Some("medium")
            );
            assert_eq!(
                project_effort(&claude, Difficulty::Hard).effort.as_deref(),
                Some("high")
            );
            assert_eq!(
                project_effort(&claude, Difficulty::Critical)
                    .effort
                    .as_deref(),
                Some("max")
            );
        }
    }

    #[test]
    fn a_published_out_of_order_ladder_is_sorted_before_projecting() {
        let codex = ladder(&["high", "minimal", "medium"], Provider::Codex);
        assert_eq!(
            project_effort(&codex, Difficulty::Mechanical)
                .effort
                .as_deref(),
            Some("minimal")
        );
        assert_eq!(
            project_effort(&codex, Difficulty::Hard).effort.as_deref(),
            Some("medium")
        );
    }

    #[test]
    fn unknown_rung_names_keep_the_published_order() {
        let odd = ladder(&["thinky", "thinkier"], Provider::Codex);
        let projected = project_effort(&odd, Difficulty::Hard);
        assert_eq!(projected.effort.as_deref(), Some("thinkier"));
        assert!(projected.reason.contains("published"));
    }

    #[test]
    fn no_ladder_means_no_effort_flag() {
        let bare = model("m", Provider::OpenCode, "oc", ModelInfoFields::default());
        let projected = project_effort(&bare, Difficulty::Hard);
        assert_eq!(projected.effort, None);
        assert!(projected.reason.contains("no reasoning levels"));
    }

    #[test]
    fn catalog_prices_do_not_affect_the_projection() {
        let priced = model(
            "m",
            Provider::Claude,
            "c",
            ModelInfoFields {
                cost: Some(ModelCost {
                    input: 3.0,
                    output: 15.0,
                }),
                ..ModelInfoFields::default()
            },
        );
        assert!(
            project_effort(&priced, Difficulty::Critical)
                .effort
                .is_none()
        );
    }
}
