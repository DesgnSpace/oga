//! What a model is worth: quality tier, price band, and speed, read from its
//! own id and the account's published prices.

use std::sync::LazyLock;

use oga_domain::ModelInfo;
use regex::Regex;

/// Where the cost read came from, which says how much to trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostSource {
    /// The account published input/output prices.
    Catalog,
    /// The name marks the band (free, small, frontier).
    Heuristic,
    /// No signal either way; scored neutrally.
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelTraits {
    pub quality: u8,
    pub cost: u8,
    pub speed: u8,
    pub cost_source: CostSource,
    pub estimated_input_usd_per_million: Option<f64>,
    pub estimated_output_usd_per_million: Option<f64>,
}

static QUALITY_TIERS: LazyLock<Vec<(Regex, u8)>> = LazyLock::new(|| {
    vec![
        // `flash` in a model name almost always marks a small tier, but
        // deepseek-v4-flash is a full-strength everyday model. This test runs
        // first and would otherwise floor it at 2, under the min_quality of
        // every route class — so policy could never select it.
        (Regex::new("deepseek-v4-flash").expect("pattern"), 4),
        (
            Regex::new("(?:haiku|nano|mini|flash|spark|lite|20b)").expect("pattern"),
            2,
        ),
        (
            Regex::new("(?:opus|fable|(?:^|[-/])sol|pro|max|ultra|reasoning|kimi-k3|kimi-k2\\.7-code|gpt-5\\.6)")
                .expect("pattern"),
            5,
        ),
        (
            Regex::new(
                "(?:sonnet|luna|terra|kimi-k2\\.[56]|large|gpt-5\\.[45]|glm-5|qwen3\\.[67]|minimax-m3)",
            )
            .expect("pattern"),
            4,
        ),
    ]
});

static SPEED_TIERS: LazyLock<Vec<(Regex, u8)>> = LazyLock::new(|| {
    vec![
        (
            Regex::new("(?:fast|flash|spark|haiku|nano|lite)").expect("pattern"),
            5,
        ),
        (Regex::new("mini").expect("pattern"), 4),
        (
            Regex::new("(?:opus|(?:^|[-/])sol|max|ultra|reasoning)").expect("pattern"),
            1,
        ),
    ]
});

static PRICY_NAMES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(?:free|haiku|nano|mini|flash|spark|lite|opus|fable|(?:^|[-/])sol|pro|max|ultra)")
        .expect("pattern")
});
static SMALL_NAMES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?:haiku|nano|mini|flash|spark|lite)").expect("pattern"));
static FREE_NAMES: LazyLock<Regex> = LazyLock::new(|| Regex::new("free").expect("pattern"));

fn tier(regexes: &[(Regex, u8)], id: &str) -> u8 {
    regexes
        .iter()
        .find(|(pattern, _)| pattern.is_match(id))
        .map_or(3, |(_, tier)| *tier)
}

pub fn model_traits(model_info: &ModelInfo) -> ModelTraits {
    let id = model_info.id.to_lowercase();
    let quality = tier(&QUALITY_TIERS, &id);
    let speed = tier(&SPEED_TIERS, &id);
    if let Some(cost) = &model_info.cost {
        let blended = cost.input * 0.8 + cost.output * 0.2;
        return ModelTraits {
            quality,
            cost: if blended == 0.0 {
                0
            } else if blended <= 0.5 {
                1
            } else if blended <= 2.0 {
                2
            } else if blended <= 8.0 {
                3
            } else if blended <= 20.0 {
                4
            } else {
                5
            },
            speed,
            cost_source: CostSource::Catalog,
            estimated_input_usd_per_million: Some(cost.input),
            estimated_output_usd_per_million: Some(cost.output),
        };
    }
    if PRICY_NAMES.is_match(&id) {
        return ModelTraits {
            quality,
            cost: if FREE_NAMES.is_match(&id) {
                0
            } else if SMALL_NAMES.is_match(&id) {
                1
            } else {
                5
            },
            speed,
            cost_source: CostSource::Heuristic,
            estimated_input_usd_per_million: None,
            estimated_output_usd_per_million: None,
        };
    }
    ModelTraits {
        quality,
        cost: 2,
        speed,
        cost_source: CostSource::Unknown,
        estimated_input_usd_per_million: None,
        estimated_output_usd_per_million: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ModelInfoFields, model};
    use oga_domain::ModelCost;

    fn traits_for(id: &str) -> ModelTraits {
        model_traits(&model(
            id,
            oga_domain::Provider::OpenCode,
            "oc",
            ModelInfoFields::default(),
        ))
    }

    #[test]
    fn a_flash_name_marks_a_small_tier() {
        assert_eq!(traits_for("opencode-go/gemini-3.6-flash-low").quality, 2);
        assert_eq!(traits_for("haiku").quality, 2);
    }

    #[test]
    fn deepseek_v4_flash_is_an_everyday_model_despite_the_flash_name() {
        let traits = traits_for("opencode-go/deepseek-v4-flash");
        assert_eq!(traits.quality, 4);
        assert_eq!(traits.speed, 5);
    }

    #[test]
    fn frontier_and_mid_tiers_read_from_the_name() {
        for id in ["opus", "fable", "openai/sol", "gpt-5.6-sol", "kimi-k3"] {
            assert_eq!(traits_for(id).quality, 5, "{id}");
        }
        // minimax-m3 is listed in the mid-tier names upstream but its own
        // "mini" substring shadows that tier there too, so parity keeps both
        // behaviors identical rather than fixing one side.
        for id in ["sonnet", "gpt-5.5-large"] {
            assert_eq!(traits_for(id).quality, 4, "{id}");
        }
        assert_eq!(traits_for("anonymous-model").quality, 3);
    }

    #[test]
    fn speed_reads_fast_names_high_and_frontier_names_low() {
        assert_eq!(traits_for("spark").speed, 5);
        assert_eq!(traits_for("mini").speed, 4);
        assert_eq!(traits_for("max").speed, 1);
        assert_eq!(traits_for("sonnet").speed, 3);
    }

    #[test]
    fn published_prices_band_the_cost() {
        let priced = |input: f64, output: f64| {
            model_traits(&model(
                "m",
                oga_domain::Provider::Claude,
                "c",
                ModelInfoFields {
                    cost: Some(ModelCost { input, output }),
                    ..ModelInfoFields::default()
                },
            ))
        };
        assert_eq!(priced(0.0, 0.0).cost, 0);
        assert_eq!(priced(0.2, 1.0).cost, 1);
        assert_eq!(priced(3.0, 15.0).cost, 3);
        assert_eq!(priced(15.0, 75.0).cost, 5);
        assert_eq!(priced(3.0, 15.0).cost_source, CostSource::Catalog);
    }

    #[test]
    fn unnamed_prices_fall_back_to_the_name() {
        assert_eq!(traits_for("something-free").cost, 0);
        assert_eq!(traits_for("nano").cost, 1);
        assert_eq!(traits_for("pro").cost, 5);
        assert_eq!(traits_for("mystery").cost_source, CostSource::Unknown);
        assert_eq!(traits_for("mystery").cost, 2);
    }
}
