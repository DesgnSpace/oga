//! Provider catalog parsing and the `/api/models` row shape.

use std::sync::LazyLock;

use oga_config::{
    ModelOverride, ModelOverrides, ResolvedModelSettings, model_enabled, model_override_for,
};
use oga_domain::{ModelCost, ModelInfo, ModelInfoSource, ModelQuery, ModelSettingsRow, Profile};
use regex::Regex;
use serde_json::Value;

/// What `discover` builds for a claude profile before any CLI call: the
/// configured model plus the CLI's aliases, sharing one session-level effort
/// ladder.
pub const CLAUDE_ALIASES: [&str; 4] = ["sonnet", "opus", "haiku", "fable"];
/// `claude --effort <level>` is a session flag, so the ladder is the same for
/// every model the CLI accepts rather than published per model.
pub const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// `pi --thinking <level>` is likewise a session flag. Its own ladder also has
/// `off`, which Oga has no effort for and therefore never sends.
pub const PI_EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Clone, Default)]
struct ModelMeta {
    cost: Option<ModelCost>,
    context_window: Option<u64>,
    reasoning: Option<bool>,
    tool_call: Option<bool>,
    efforts: Option<Vec<String>>,
    default_effort: Option<String>,
}

impl ModelMeta {
    fn into_info(
        self,
        profile: &Profile,
        id: String,
        label: String,
        source: ModelInfoSource,
    ) -> ModelInfo {
        ModelInfo {
            id,
            label,
            provider: profile.provider,
            profile_id: profile.id.clone(),
            source,
            cost: self.cost,
            context_window: self.context_window,
            reasoning: self.reasoning,
            efforts: self.efforts,
            default_effort: self.default_effort,
            tool_call: self.tool_call,
        }
    }
}

pub fn claude_models(profile: &Profile) -> Vec<ModelInfo> {
    let mut ids = vec![profile.default_model.clone()];
    for alias in CLAUDE_ALIASES {
        if !ids.iter().any(|id| id == alias) {
            ids.push(alias.to_owned());
        }
    }
    ids.into_iter()
        .map(|id| {
            let source = if id == profile.default_model {
                ModelInfoSource::Configured
            } else {
                ModelInfoSource::Alias
            };
            ModelMeta {
                efforts: Some(CLAUDE_EFFORTS.into_iter().map(String::from).collect()),
                ..ModelMeta::default()
            }
            .into_info(profile, id.clone(), id, source)
        })
        .collect()
}

pub fn claude_models_from_catalog(
    profile: &Profile,
    models: impl IntoIterator<Item = (String, String)>,
) -> Vec<ModelInfo> {
    let models: Vec<_> = models.into_iter().collect();
    if models.is_empty() {
        return claude_models(profile);
    }
    models
        .into_iter()
        .map(|(id, label)| {
            let source = if id == profile.default_model {
                ModelInfoSource::Configured
            } else {
                ModelInfoSource::Discovered
            };
            ModelMeta {
                efforts: Some(CLAUDE_EFFORTS.into_iter().map(String::from).collect()),
                ..ModelMeta::default()
            }
            .into_info(profile, id, label, source)
        })
        .collect()
}

/// Codex publishes `{ models: [...] }`; hidden slugs stay hidden, and each
/// model carries its own reasoning-effort ladder.
pub fn parse_codex_models(raw: &str, profile: &Profile) -> serde_json::Result<Vec<ModelInfo>> {
    let parsed: Value = serde_json::from_str(raw)?;
    Ok(parsed
        .get("models")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|item| {
            let slug = item.get("slug")?.as_str()?.to_owned();
            if item.get("visibility").and_then(Value::as_str) == Some("hidden") {
                return None;
            }
            let label = item
                .get("display_name")
                .and_then(Value::as_str)
                .filter(|label| !label.is_empty())
                .unwrap_or(&slug)
                .to_owned();
            // Entries arrive as objects carrying an effort id and a
            // description; only the id is needed to dispatch, so the
            // description is dropped rather than stored and never used.
            let mut efforts: Vec<String> = Vec::new();
            for level in item
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let id = match level {
                    Value::String(text) => Some(text.clone()),
                    other => other
                        .get("effort")
                        .and_then(Value::as_str)
                        .map(String::from),
                };
                if let Some(id) = id
                    && !efforts.contains(&id)
                {
                    efforts.push(id);
                }
            }
            Some(
                ModelMeta {
                    efforts: (!efforts.is_empty()).then_some(efforts),
                    default_effort: item
                        .get("default_reasoning_level")
                        .and_then(Value::as_str)
                        .map(String::from),
                    ..ModelMeta::default()
                }
                .into_info(profile, slug, label, ModelInfoSource::Discovered),
            )
        })
        .collect())
}

/// OpenCode prints one `provider/model` line per entry followed optionally by
/// a JSON metadata block; anything between two model lines belongs to the one
/// before it.
pub fn parse_opencode_models(raw: &str, profile: &Profile) -> Vec<ModelInfo> {
    let clean = strip_ansi(raw);
    let mut out: Vec<ModelInfo> = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in clean.lines() {
        if is_model_line(line.trim()) {
            if let Some((id, metadata)) = current.take() {
                out.push(opencode_entry(profile, id, &metadata));
            }
            current = Some((line.trim().to_owned(), Vec::new()));
        } else if let Some((_, metadata)) = &mut current {
            metadata.push(line);
        }
    }
    if let Some((id, metadata)) = current.take() {
        out.push(opencode_entry(profile, id, &metadata));
    }
    out
}

fn opencode_entry(profile: &Profile, id: String, metadata: &[&str]) -> ModelInfo {
    let meta = opencode_metadata(metadata.join("\n").trim());
    meta.into_info(profile, id.clone(), id, ModelInfoSource::Discovered)
}

fn is_model_line(line: &str) -> bool {
    let Some((left, right)) = line.split_once('/') else {
        return false;
    };
    let plain = |part: &str| !part.is_empty() && part.chars().all(|c| !c.is_whitespace());
    plain(left) && !right.is_empty() && !right.contains('/') && plain(right)
}

fn opencode_metadata(raw: &str) -> ModelMeta {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return ModelMeta::default();
    };
    let input = non_negative_number(value.pointer("/cost/input"));
    let output = non_negative_number(value.pointer("/cost/output"));
    let bool_of = |pointer: &str| value.pointer(pointer).and_then(Value::as_bool);
    ModelMeta {
        cost: match (input, output) {
            (Some(input), Some(output)) => Some(ModelCost { input, output }),
            _ => None,
        },
        context_window: non_negative_number(value.pointer("/limit/context")).map(|n| n as u64),
        reasoning: bool_of("/capabilities/reasoning").or_else(|| bool_of("/reasoning")),
        tool_call: bool_of("/capabilities/toolcall").or_else(|| bool_of("/tool_call")),
        ..ModelMeta::default()
    }
}

/// The v2 server answers `GET /api/model` with `{location, data: [Model.Info]}`.
/// A row's addressable id is `providerID/modelID` — the spelling `run --model`
/// takes — and a model's published `variants` are its reasoning-effort ladder,
/// each variant carrying one `reasoningEffort`.
pub fn parse_opencode_v2_models(
    raw: &str,
    profile: &Profile,
) -> serde_json::Result<Vec<ModelInfo>> {
    let parsed: Value = serde_json::from_str(raw)?;
    let Some(rows) = parsed.get("data").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    Ok(rows
        .iter()
        .filter_map(|item| {
            let provider_id = item.get("providerID")?.as_str()?;
            let model_id = item.get("modelID")?.as_str()?;
            if provider_id.is_empty() || model_id.is_empty() {
                return None;
            }
            if item.get("enabled").and_then(Value::as_bool) == Some(false) {
                return None;
            }
            let id = format!("{provider_id}/{model_id}");
            let label = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or(&id)
                .to_owned();
            let input = non_negative_number(item.pointer("/cost/0/input"));
            let output = non_negative_number(item.pointer("/cost/0/output"));
            let tool_call = item.pointer("/capabilities/tools").and_then(Value::as_bool);
            // A model with variants reasons; the variant ids are the ladder it
            // accepts.
            let mut efforts: Vec<String> = Vec::new();
            for variant in item
                .get("variants")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(rung) = variant
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|rung| !rung.is_empty())
                    && !efforts.iter().any(|known| known == rung)
                {
                    efforts.push(rung.to_owned());
                }
            }
            Some(
                ModelMeta {
                    cost: match (input, output) {
                        (Some(input), Some(output)) => Some(ModelCost { input, output }),
                        _ => None,
                    },
                    context_window: non_negative_number(item.pointer("/limit/context"))
                        .map(|n| n as u64),
                    tool_call,
                    reasoning: (!efforts.is_empty()).then_some(true),
                    efforts: (!efforts.is_empty()).then_some(efforts),
                    ..ModelMeta::default()
                }
                .into_info(profile, id, label, ModelInfoSource::Discovered),
            )
        })
        .collect())
}

pub fn parse_antigravity_models(raw: &str, profile: &Profile) -> Vec<ModelInfo> {
    raw.lines()
        .filter_map(|line| {
            let (id, label) = line.split_once('\t')?;
            let id = id.trim();
            let label = label.trim();
            if !ANTIGRAVITY_ID.is_match(id) || label.is_empty() {
                return None;
            }
            Some((id.to_owned(), label.to_owned()))
        })
        .map(|(id, label)| {
            ModelMeta::default().into_info(profile, id, label, ModelInfoSource::Discovered)
        })
        .collect()
}

/// `pi --list-models` prints a padded table — a header row, then one row per
/// model with columns joined by at least two spaces. Widths are recomputed per
/// invocation, so the split has to be on the gap, never on an offset. Rows are
/// keyed off the header rather than assumed: the failure outputs (no auth
/// anywhere, or a search that matched nothing) are prose, and matching no
/// header drops them to the caller's configured-model fallback.
pub fn parse_pi_models(raw: &str, profile: &Profile) -> Vec<ModelInfo> {
    let clean = strip_ansi(raw);
    let rows: Vec<Vec<&str>> = clean
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| PI_COLUMN_SPLIT.split(line).collect())
        .collect();
    let header_index = rows.iter().position(|columns| {
        columns.first().copied() == Some("provider") && columns.get(1).copied() == Some("model")
    });
    let Some(header_index) = header_index else {
        return Vec::new();
    };
    let thinking_column = rows[header_index]
        .iter()
        .position(|column| *column == "thinking");
    rows[header_index + 1..]
        .iter()
        .filter_map(|columns| {
            let [provider, model, ..] = columns.as_slice() else {
                return None;
            };
            if provider.is_empty() || model.is_empty() {
                return None;
            }
            let id = format!("{provider}/{model}");
            // pi takes --thinking as a session flag rather than publishing a
            // ladder per model, so a reasoning model gets the whole ladder,
            // like Claude's.
            let reasoning =
                thinking_column.is_some_and(|column| columns.get(column).copied() == Some("yes"));
            Some(
                ModelMeta {
                    reasoning: Some(reasoning),
                    efforts: reasoning.then(|| PI_EFFORTS.into_iter().map(String::from).collect()),
                    ..ModelMeta::default()
                }
                .into_info(profile, id.clone(), id, ModelInfoSource::Discovered),
            )
        })
        .collect()
}

static ANTIGRAVITY_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)^[a-z0-9][a-z0-9._-]*$").expect("pattern"));
static PI_COLUMN_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}").expect("pattern"));
static ANSI_ESCAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").expect("pattern"));

fn strip_ansi(raw: &str) -> String {
    ANSI_ESCAPE.replace_all(raw, "").into_owned()
}

fn non_negative_number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && *number >= 0.0)
}

/// Capabilities a model advertises unless this project overrides them.
pub fn model_capabilities(
    model_info: &ModelInfo,
    override_for_model: Option<&ModelOverride>,
) -> Vec<String> {
    if let Some(capabilities) = override_for_model.and_then(|o| o.capabilities.as_ref()) {
        return capabilities.clone();
    }
    let mut capabilities = Vec::new();
    if model_info.reasoning == Some(true) {
        capabilities.push("reasoning".into());
    }
    if model_info
        .context_window
        .is_some_and(|window| window >= 128_000)
    {
        capabilities.push("long-context".into());
    }
    if model_info.tool_call == Some(true) {
        capabilities.push("tool-use".into());
    }
    if model_info
        .cost
        .as_ref()
        .is_some_and(|cost| cost.input == 0.0 && cost.output == 0.0)
    {
        capabilities.push("free".into());
    }
    capabilities
}

/// One `/api/models` row: the catalog entry joined with per-project settings.
/// Profiles ride along so a rule naming a worker alone still marks that
/// worker's default model as loved.
pub fn select_model_rows(
    models: &[ModelInfo],
    overrides: &ModelOverrides,
    settings: &ResolvedModelSettings,
    query: &ModelQuery,
    profiles: &[Profile],
) -> Vec<ModelSettingsRow> {
    let rows: Vec<ModelSettingsRow> = models
        .iter()
        .map(|model| {
            let override_for_model = model_override_for(overrides, &model.profile_id, &model.id);
            let enabled = override_for_model
                .as_ref()
                .and_then(|o| o.enabled)
                .unwrap_or_else(|| model_enabled(settings, &model.profile_id, &model.id));
            let default_model = profiles
                .iter()
                .find(|profile| profile.id == model.profile_id)
                .map(|profile| profile.default_model.as_str());
            ModelSettingsRow {
                profile: model.profile_id.clone(),
                model: model.id.clone(),
                capabilities: model_capabilities(model, override_for_model.as_ref()),
                enabled,
                preferred: override_for_model.as_ref().and_then(|o| o.preferred) == Some(true),
                loved: settings
                    .love
                    .names_model(&model.profile_id, &model.id, default_model),
                efforts: model.efforts.clone(),
                default_effort: model.default_effort.clone(),
                usage: None,
            }
        })
        .collect();
    // The loved model is where an unnamed dispatch lands, so a narrowed view
    // that hid it would answer "what runs here" with the one row that is
    // wrong.
    let has_preferred = rows.iter().any(|row| row.preferred && row.enabled);
    rows.into_iter()
        .filter(|row| {
            query
                .profile
                .as_deref()
                .is_none_or(|profile| row.profile == profile)
        })
        .filter(|row| {
            query.provider.is_none_or(|provider| {
                models.iter().any(|model| {
                    model.profile_id == row.profile
                        && model.id == row.model
                        && model.provider == provider
                })
            })
        })
        // A disabled row survives only when the caller asked for everything.
        .filter(|row| query.only_enabled == Some(false) || row.enabled)
        .filter(|row| {
            if query.only_preferred == Some(false) {
                true
            } else if has_preferred {
                row.preferred || row.loved
            } else {
                true
            }
        })
        .filter(|row| {
            query
                .query
                .as_deref()
                .is_none_or(|needle| row.model.to_lowercase().contains(&needle.to_lowercase()))
        })
        .map(|mut row| {
            if !has_preferred {
                row.preferred = false;
            }
            row
        })
        .collect()
}
