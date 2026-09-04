//! Catalog-parsing and `/api/models` row goldens ported from the TypeScript
//! behavioral suite.

use std::collections::BTreeMap;

use oga_config::{
    DirectoryModelSettings, ModelOverride, ModelOverrides, ProfileModelEnablement,
    ResolvedModelSettings,
};
use oga_domain::{ModelCost, ModelInfoSource, ModelQuery, Provider};
use oga_routing::{
    CLAUDE_EFFORTS, claude_models, model_capabilities, parse_antigravity_models,
    parse_codex_models, parse_opencode_models, parse_opencode_v2_models, parse_pi_models,
    select_model_rows,
};

fn profile(provider: Provider) -> oga_domain::Profile {
    use std::collections::BTreeMap;
    oga_domain::Profile {
        id: "codex-work".into(),
        label: "Codex work".into(),
        provider,
        default_model: "gpt-default".into(),
        enabled: true,
        env: BTreeMap::new(),
        capabilities: vec![],
        command: None,
    }
}

#[test]
fn normalizes_visible_codex_models() {
    let raw = r#"{"models":[
        {"slug":"gpt-a","display_name":"GPT A","visibility":"list"},
        {"slug":"gpt-hidden","visibility":"hidden"}
    ]}"#;
    let parsed = parse_codex_models(raw, &profile(Provider::Codex)).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "gpt-a");
    assert_eq!(parsed[0].label, "GPT A");
    assert_eq!(parsed[0].source, ModelInfoSource::Discovered);
}

#[test]
fn model_rows_expose_each_model_effort_ladder() {
    let model = oga_domain::ModelInfo {
        id: "sonnet".into(),
        label: "Sonnet".into(),
        provider: Provider::Claude,
        profile_id: "codex-work".into(),
        source: ModelInfoSource::Discovered,
        cost: None,
        context_window: None,
        reasoning: Some(true),
        efforts: Some(vec!["low".into(), "medium".into(), "high".into()]),
        default_effort: Some("medium".into()),
        tool_call: None,
    };
    let rows = select_model_rows(
        std::slice::from_ref(&model),
        &ModelOverrides::default(),
        &settings(std::slice::from_ref(&model)),
        &oga_domain::ModelQuery::default(),
    );
    assert_eq!(
        rows[0].efforts.as_deref(),
        Some(["low".into(), "medium".into(), "high".into()].as_slice())
    );
    assert_eq!(rows[0].default_effort.as_deref(), Some("medium"));
}

#[test]
fn keeps_the_codex_reasoning_effort_ladder_and_its_default() {
    let raw = r#"{"models":[{
        "slug":"gpt-5.6-luna",
        "display_name":"GPT-5.6-Luna",
        "visibility":"list",
        "default_reasoning_level":"medium",
        "supported_reasoning_levels":[
            {"effort":"low","description":"Fast responses with lighter reasoning"},
            {"effort":"medium","description":"Balances speed and reasoning depth"},
            {"effort":"high","description":"Greater reasoning depth"},
            {"effort":"xhigh","description":"Extra high reasoning depth"},
            {"effort":"max","description":"Maximum reasoning depth"}
        ]
    }]}"#;
    let parsed = parse_codex_models(raw, &profile(Provider::Codex)).unwrap();
    assert_eq!(
        parsed[0].efforts.as_deref().map(<[String]>::to_vec),
        Some(vec![
            "low".to_owned(),
            "medium".to_owned(),
            "high".to_owned(),
            "xhigh".to_owned(),
            "max".to_owned()
        ])
    );
    assert_eq!(parsed[0].default_effort.as_deref(), Some("medium"));
}

#[test]
fn omits_the_effort_ladder_when_the_provider_publishes_none() {
    let parsed = parse_codex_models(
        r#"{"models":[{"slug":"gpt-a","visibility":"list"}]}"#,
        &profile(Provider::Codex),
    )
    .unwrap();
    assert_eq!(parsed[0].efforts, None);
    assert_eq!(parsed[0].default_effort, None);
}

#[test]
fn normalizes_opencode_provider_model_lines_with_metadata() {
    let p = profile(Provider::OpenCode);
    let parsed = parse_opencode_models("openai/gpt-5\nbad line\nanthropic/sonnet\n", &p);
    let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["openai/gpt-5", "anthropic/sonnet"]);

    // ANSI coloring and pretty-printed metadata blocks survive the split.
    let rich = parse_opencode_models(
        "\x1b[1mopencode/kimi-k3\x1b[0m\n{\n  \"cost\": { \"input\": 0.4, \"output\": 2.0 },\n  \
         \"limit\": { \"context\": 262144 },\n  \"capabilities\": { \"reasoning\": true, \
         \"toolcall\": true }\n}\n",
        &p,
    );
    assert_eq!(rich.len(), 1);
    assert_eq!(rich[0].id, "opencode/kimi-k3");
    assert_eq!(rich[0].reasoning, Some(true));
    assert_eq!(rich[0].tool_call, Some(true));
    assert_eq!(
        rich[0].cost,
        Some(ModelCost {
            input: 0.4,
            output: 2.0
        })
    );
    assert_eq!(rich[0].context_window, Some(262144));
}

#[test]
fn normalizes_antigravity_model_lines() {
    let p = profile(Provider::Antigravity);
    let parsed = parse_antigravity_models(
        "Fetching available models...\n\
         gemini-3.7-flash-high\tGemini 3.7 Flash (High)\n\
         gemini-3.7-flash-medium\tGemini 3.7 Flash (Medium)\n\
         gemini-3.7-flash-low\tGemini 3.7 Flash (Low)\n\
         gemini-3.6-flash-high\tGemini 3.6 Flash (High)\n\
         gemini-3.6-flash-medium\tGemini 3.6 Flash (Medium)\n\
         gemini-3.6-flash-low\tGemini 3.6 Flash (Low)\n\
         gemini-3.1-pro-high\tGemini 3.1 Pro (High)\n\
         gemini-3.1-pro-low\tGemini 3.1 Pro (Low)\n\
         claude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\n\
         claude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)\n\
         gpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n\
         Available agents:\n",
        &p,
    );
    let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(parsed.len(), 11);
    assert_eq!(ids[0], "gemini-3.7-flash-high");
    assert_eq!(parsed[0].label, "Gemini 3.7 Flash (High)");
    assert_eq!(ids[10], "gpt-oss-120b-medium");
    assert_eq!(parsed[10].label, "GPT-OSS 120B (Medium)");
}

#[test]
fn parses_pi_padded_tables_and_drops_prose_outputs() {
    let p = profile(Provider::Pi);
    let table = [
        "provider  model            context  max-out  thinking  images",
        "anthropic  claude-sonnet-4-6  200K     64K      yes       yes",
        "opencode   nemotron-3-super-free  1M   32K      no        no",
    ]
    .join("\n");
    let parsed = parse_pi_models(&table, &p);
    let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "anthropic/claude-sonnet-4-6",
            "opencode/nemotron-3-super-free"
        ]
    );
    assert_eq!(
        parsed[0].efforts.as_deref().map(<[String]>::to_vec),
        Some(
            ["minimal", "low", "medium", "high", "xhigh", "max"]
                .into_iter()
                .map(String::from)
                .collect()
        )
    );
    assert_eq!(parsed[1].efforts, None);
    assert_eq!(parsed[1].reasoning, Some(false));

    // A search that matched nothing is prose, not a catalog.
    assert!(parse_pi_models("No models matching \"sonnet\"\n", &p).is_empty());
}

#[test]
fn normalizes_opencode2_server_catalog_rows() {
    let mut p = profile(Provider::OpenCode2);
    p.id = "oc2".into();
    p.default_model = "opencode/x-preview-f-free".into();
    let raw = r#"{"location":{"directory":"/tmp"},"data":[
        {
          "providerID":"opencode","modelID":"x-preview-f-free","name":"Ox Alpha Free (Unlimited)",
          "enabled":true,
          "capabilities":{"tools":true,"input":["text"],"output":["text"]},
          "variants":[],
          "cost":[{"input":0,"output":0}],
          "limit":{"context":256000,"output":32000}
        },
        {
          "providerID":"opencode","modelID":"deepseek-v4-flash-free","name":"DeepSeek V4 Flash Free",
          "enabled":true,
          "capabilities":{"tools":true,"input":["text"],"output":["text"]},
          "variants":[{"id":"low"},{"id":"high"},{"id":"max"}],
          "cost":[{"input":0,"output":0}],
          "limit":{"context":200000,"output":128000}
        }
    ]}"#;
    let parsed = parse_opencode_v2_models(raw, &p).unwrap();
    let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "opencode/x-preview-f-free",
            "opencode/deepseek-v4-flash-free"
        ]
    );
    assert_eq!(parsed[0].label, "Ox Alpha Free (Unlimited)");
    assert_eq!(parsed[0].tool_call, Some(true));
    assert_eq!(parsed[0].context_window, Some(256000));
    // A model with variants reasons, and the variant ids are its ladder.
    assert_eq!(parsed[1].reasoning, Some(true));
    assert_eq!(
        parsed[1].efforts.as_deref().map(<[String]>::to_vec),
        Some(vec!["low".to_owned(), "high".to_owned(), "max".to_owned()])
    );
    assert_eq!(parsed[0].efforts, None);

    // Disabled rows and malformed ids are skipped.
    let bad = r#"{"data":[
        {"providerID":"opencode","modelID":"retired","name":"Retired","enabled":false,"variants":[],"cost":[],"limit":{}},
        {"providerID":"opencode","modelID":"","name":"No id","variants":[],"cost":[],"limit":{}}
    ]}"#;
    assert!(parse_opencode_v2_models(bad, &p).unwrap().is_empty());
}

#[test]
fn the_claude_catalog_uses_models_dev_names_and_efforts() {
    let mut p = profile(Provider::Claude);
    p.id = "claude".into();
    p.default_model = "sonnet".into();
    let parsed = oga_routing::claude_models_from_catalog(
        &p,
        [
            ("claude-sonnet-4-6".into(), "Claude Sonnet 4.6".into()),
            ("claude-opus-4-6".into(), "Claude Opus 4.6".into()),
        ],
    );
    let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["claude-opus-4-6", "claude-sonnet-4-6"]);
    assert_eq!(parsed[0].label, "Claude Opus 4.6");
    assert_eq!(parsed[1].source, ModelInfoSource::Configured);
    for model in &parsed {
        assert_eq!(
            model.efforts.as_deref().map(<[String]>::to_vec),
            Some(CLAUDE_EFFORTS.into_iter().map(String::from).collect())
        );
    }
}

#[test]
fn the_claude_catalog_falls_back_to_built_in_models_when_empty() {
    let mut p = profile(Provider::Claude);
    p.default_model = "sonnet".into();
    let parsed = oga_routing::claude_models_from_catalog(&p, Vec::new());
    let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["sonnet", "opus", "haiku", "fable"]);
    assert_eq!(parsed[1].source, ModelInfoSource::Alias);
}

/// Every catalog row switched on for this account. The join and narrowing
/// goldens are about what a row carries, not about what is on by default.
fn settings(models: &[oga_domain::ModelInfo]) -> ResolvedModelSettings {
    ResolvedModelSettings {
        global: DirectoryModelSettings {
            profiles: BTreeMap::from([(
                "codex-work".to_owned(),
                ProfileModelEnablement {
                    enabled: None,
                    model_enabled: models
                        .iter()
                        .map(|model| (model.id.clone(), true))
                        .collect(),
                },
            )]),
        },
        project: None,
        overrides: None,
        loved: None,
    }
}

#[test]
fn model_capabilities_derive_from_the_catalog_entry_unless_overridden() {
    let mut model = claude_models(&profile(Provider::Claude))[0].clone();
    model.reasoning = Some(true);
    model.context_window = Some(200_000);
    model.tool_call = Some(true);
    model.cost = Some(ModelCost {
        input: 0.0,
        output: 0.0,
    });
    assert_eq!(
        model_capabilities(&model, None),
        vec!["reasoning", "long-context", "tool-use", "free"]
    );
    // An override replaces the derived list whole.
    let over = ModelOverride {
        capabilities: Some(vec!["vision".into()]),
        ..ModelOverride::default()
    };
    assert_eq!(model_capabilities(&model, Some(&over)), vec!["vision"]);
}

#[test]
fn model_rows_join_catalog_entries_with_settings_and_narrowing() {
    let catalog = oga_routing::claude_models(&{
        let mut p = profile(Provider::Claude);
        p.default_model = "sonnet".into();
        p
    });
    let mut overrides = ModelOverrides::default();
    overrides.shared.insert(
        "haiku".into(),
        ModelOverride {
            enabled: None,
            preferred: Some(true),
            loved: None,
            effort: None,
            capabilities: Some(vec!["fast".into()]),
        },
    );
    let rows_all = select_model_rows(
        &catalog,
        &overrides,
        &settings(&catalog),
        &ModelQuery::default(),
    );
    let haiku = rows_all.iter().find(|row| row.model == "haiku").unwrap();
    assert!(haiku.preferred);
    assert_eq!(haiku.capabilities, vec!["fast"]);

    // With a preferred row present, the narrowed default view keeps preferred
    // and loved rows only.
    assert!(rows_all.iter().all(|row| row.preferred || row.loved));
    let widened = select_model_rows(
        &catalog,
        &overrides,
        &settings(&catalog),
        &ModelQuery {
            only_preferred: Some(false),
            ..ModelQuery::default()
        },
    );
    assert_eq!(widened.len(), 4);

    // Without any preferred row, nothing claims preference.
    let empty = ModelOverrides::default();
    let rows = select_model_rows(
        &catalog,
        &empty,
        &settings(&catalog),
        &ModelQuery::default(),
    );
    assert!(rows.iter().all(|row| !row.preferred));
    assert_eq!(rows.len(), 4);

    // The query narrows by case-insensitive substring of the model id.
    let rows = select_model_rows(
        &catalog,
        &empty,
        &settings(&catalog),
        &ModelQuery {
            query: Some("OPUS".into()),
            ..ModelQuery::default()
        },
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model, "opus");
}

#[test]
fn a_model_nobody_switched_on_is_reported_unavailable() {
    let catalog = claude_models(&profile(Provider::Claude));
    let nothing_on = ResolvedModelSettings {
        global: DirectoryModelSettings::default(),
        project: None,
        overrides: None,
        loved: None,
    };
    let rows = select_model_rows(
        &catalog,
        &ModelOverrides::default(),
        &nothing_on,
        &ModelQuery {
            only_enabled: Some(false),
            only_preferred: Some(false),
            ..ModelQuery::default()
        },
    );

    assert_eq!(rows.len(), catalog.len());
    assert!(rows.iter().all(|row| !row.enabled));
    // The default view offers nothing, so a caller sees the gap before it
    // dispatches into it.
    assert!(
        select_model_rows(
            &catalog,
            &ModelOverrides::default(),
            &nothing_on,
            &ModelQuery::default(),
        )
        .is_empty()
    );
}
