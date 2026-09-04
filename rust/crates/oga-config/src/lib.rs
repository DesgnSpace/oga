//! Profile discovery, YAML layers, cwd settings, and prompts.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use oga_domain::{Profile, Provider, TaskClass};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const DEFAULT_MODELS: &[(Provider, &str)] = &[
    (Provider::Claude, "sonnet"),
    (Provider::Codex, "gpt-5"),
    (Provider::OpenCode, "opencode/big-pickle"),
    (Provider::OpenCode2, "opencode/x-preview-f-free"),
    (Provider::Antigravity, "gemini-3.6-flash-medium"),
    (Provider::Pi, "opencode-go/deepseek-v4-flash"),
];

pub const EFFORT_LEVELS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
const EFFORT_MESSAGE: &str = "must be one of minimal, low, medium, high, xhigh, max";
const KIND_LIST_MESSAGE: &str =
    "must be a list of kinds of work: mechanical, context, build, reasoning, general";

pub const MODEL_SETTINGS_KEY: &str = "models";
pub const PROMPTS_KEY: &str = "prompts";
pub const DEFAULT_WORKER_PROMPT: &str = concat!(
    "1. Blocked means stop. A command that will not run, a missing credential, an account or signup, a permission denial, a path outside your scope, a decision this brief does not answer — stop and report it, naming the blocker and the one decision you need.\n",
    "2. Do not work around a blocker. No retry loops, no second tool for the same job, no creating accounts, no linking or authenticating anything, no faking or stubbing the result. Stop, finish what does not depend on it, then report the exact command or path and say whether you need the caller to decide or to run it and return the output.\n",
    "3. Partial work is a valid result. Finish what is unblocked, then report what you stopped on.\n",
    "4. Never report a result you did not observe. If you could not run a check, say so and say why, instead of describing an outcome you did not see.\n",
    "5. Open your final report with `## TL;DR` — 1-3 plain-language sentences stating what was done or found and the outcome. Detail follows after; this applies to your final answer, not to intermediate messages.\n",
    "6. Write that TL;DR as bullets — one idea per line, never a paragraph — and make it stand alone: no bullet may need the detail below it to make sense.\n",
    "7. Keep it to roughly ten lines or fewer: the verdict; what the work did or decided, one meaningful line per decision; checks run and their results, quoting failures exactly; and what is left, broken, or uncertain, or \"nothing\".\n",
    "8. Describe meaning, not a file list. Name a file only when the file itself is the point, such as a moved file, deleted feature, or new entry point. Keep the branch line for worktree tasks."
);

fn yaml_key(value: &serde_yaml::Value) -> Option<&str> {
    value.as_str()
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid config {path} at {field}: {message}")]
    Invalid {
        path: PathBuf,
        field: String,
        message: String,
    },
    #[error("config changed outside Oga: {0}")]
    Conflict(PathBuf),
    #[error("I/O error at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid stored setting: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSource {
    pub source: String,
    pub sources: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedProfile {
    pub id: String,
    pub by: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProfiles {
    pub profiles: Vec<Profile>,
    pub sources: BTreeMap<String, ProfileSource>,
    pub excluded: Vec<ExcludedProfile>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub profiles: Vec<Profile>,
}
#[derive(Debug, Clone)]
pub struct ConfigLayer {
    pub path: PathBuf,
    pub root: serde_yaml::Mapping,
}
#[derive(Debug, Clone, Default)]
pub struct ConfigLayers {
    pub project: Option<ConfigLayer>,
    pub user: Option<ConfigLayer>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelOverride {
    pub enabled: Option<bool>,
    pub preferred: Option<bool>,
    pub loved: Option<bool>,
    pub effort: Option<String>,
    pub capabilities: Option<Vec<String>>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelOverrides {
    pub shared: BTreeMap<String, ModelOverride>,
    #[serde(rename = "byProfile")]
    pub by_profile: BTreeMap<String, BTreeMap<String, ModelOverride>>,
}
/// One standing answer to "where does work that names no model go": a model,
/// the kinds of work it takes, and the reasoning effort it takes them at. An
/// empty `when` is the catch-all, taking every kind no other rule claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoveRule {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub when: Vec<TaskClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub scope: String,
}

impl LoveRule {
    /// Whether a catalog entry is this rule's model. A worker-scoped rule pins
    /// the account too; a bare model id matches wherever it is offered.
    pub fn names_model(&self, profile_id: &str, model: &str) -> bool {
        self.model == model
            && self
                .profile_id
                .as_deref()
                .is_none_or(|named| named == profile_id)
    }

    /// How the rule is written and read back: `<worker>/<model>`, or the bare
    /// model.
    pub fn label(&self) -> String {
        match &self.profile_id {
            Some(profile_id) => format!("{profile_id}/{}", self.model),
            None => self.model.clone(),
        }
    }
}

/// The love rules of the scope that owns them: a project's list replaces the
/// global one whole, never merging the two.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LoveRules(pub Vec<LoveRule>);

impl LoveRules {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, LoveRule> {
        self.0.iter()
    }

    /// The rule that owns this kind of work: the one claiming it, else the
    /// catch-all, else nothing.
    pub fn for_class(&self, class: TaskClass) -> Option<&LoveRule> {
        self.0
            .iter()
            .find(|rule| rule.when.contains(&class))
            .or_else(|| self.0.iter().find(|rule| rule.when.is_empty()))
    }

    /// Whether any rule sends work to this model.
    pub fn names_model(&self, profile_id: &str, model: &str) -> bool {
        self.0
            .iter()
            .any(|rule| rule.names_model(profile_id, model))
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryModelSettings {
    pub profiles: BTreeMap<String, ProfileModelEnablement>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileModelEnablement {
    pub enabled: Option<bool>,
    #[serde(rename = "modelEnabled", default)]
    pub model_enabled: BTreeMap<String, bool>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModelSettings {
    pub global: DirectoryModelSettings,
    pub project: Option<DirectoryModelSettings>,
    pub overrides: Option<ModelOverrides>,
    pub love: LoveRules,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSnapshot {
    pub path: PathBuf,
    pub text: String,
    pub revision: String,
}

pub fn default_model(provider: Provider) -> &'static str {
    DEFAULT_MODELS
        .iter()
        .find(|(p, _)| *p == provider)
        .map_or("", |(_, model)| model)
}
pub fn canonical_cwd(cwd: impl AsRef<Path>) -> PathBuf {
    fs::canonicalize(cwd.as_ref()).unwrap_or_else(|_| cwd.as_ref().to_path_buf())
}
pub fn global_cwd() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"))
}
pub fn config_path(cwd: Option<&Path>) -> PathBuf {
    cwd.map_or_else(
        || global_cwd().join(".oga.yaml"),
        |p| canonical_cwd(p).join(".oga.yaml"),
    )
}

pub fn load_config_layers(cwd: Option<&Path>) -> Result<ConfigLayers, ConfigError> {
    let project = cwd
        .map(|p| config_path(Some(p)))
        .map(|p| read_layer(&p))
        .transpose()?
        .flatten();
    let user = read_layer(&config_path(None))?;
    Ok(ConfigLayers { project, user })
}
fn read_layer(path: &Path) -> Result<Option<ConfigLayer>, ConfigError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e)
            if e.kind() == io::ErrorKind::NotFound
                || e.kind() == io::ErrorKind::PermissionDenied =>
        {
            if e.kind() == io::ErrorKind::NotFound
                && path.extension().and_then(|e| e.to_str()) == Some("yaml")
            {
                let yml = path.with_extension("yml");
                if yml.exists() {
                    return read_layer(&yml);
                }
            }
            return Ok(None);
        }
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.into(),
                source,
            });
        }
    };
    let root = serde_yaml::from_str::<serde_yaml::Mapping>(&text)
        .map_err(|e| invalid(path, "syntax", &e.to_string()))?;
    Ok(Some(ConfigLayer {
        path: path.into(),
        root,
    }))
}

pub fn load_profiles(
    base: Vec<Profile>,
    cwd: Option<&Path>,
) -> Result<ResolvedProfiles, ConfigError> {
    let layers = load_config_layers(cwd)?;
    resolve_profiles(base, layers.user.as_ref(), layers.project.as_ref())
}
pub fn resolve_profiles(
    base: Vec<Profile>,
    user: Option<&ConfigLayer>,
    project: Option<&ConfigLayer>,
) -> Result<ResolvedProfiles, ConfigError> {
    let user_table = profile_table(user)?;
    let project_table = profile_table(project)?;
    let mut ids: Vec<String> = base
        .iter()
        .map(|p| p.id.clone())
        .chain(user_table.entries.keys().cloned())
        .chain(project_table.entries.keys().cloned())
        .collect();
    ids.sort();
    ids.dedup();
    let mut allowed: BTreeSet<String> = ids.iter().cloned().collect();
    let mut excluded = Vec::new();
    for only in [user_table.only.as_ref(), project_table.only.as_ref()]
        .into_iter()
        .flatten()
    {
        for id in allowed
            .iter()
            .filter(|id| !only.ids.contains(*id))
            .cloned()
            .collect::<Vec<_>>()
        {
            excluded.push(ExcludedProfile {
                id: id.clone(),
                by: only.path.display().to_string(),
            });
            allowed.remove(&id);
        }
    }
    let base_map: BTreeMap<_, _> = base.into_iter().map(|p| (p.id.clone(), p)).collect();
    let mut profiles = Vec::new();
    let mut sources = BTreeMap::new();
    for id in ids.into_iter().filter(|id| allowed.contains(id)) {
        let profile = merge_profile(
            base_map.get(&id),
            user_table.entries.get(&id),
            project_table.entries.get(&id),
            &id,
        )?;
        let declaring: Vec<String> = [project_table.entries.get(&id), user_table.entries.get(&id)]
            .into_iter()
            .flatten()
            .map(|e| e.path.display().to_string())
            .collect();
        sources.insert(
            id.clone(),
            ProfileSource {
                source: declaring
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "defaults".into()),
                sources: if declaring.is_empty() {
                    vec!["defaults".into()]
                } else {
                    declaring
                },
            },
        );
        profiles.push(profile);
    }
    Ok(ResolvedProfiles {
        profiles,
        sources,
        excluded,
    })
}

#[derive(Default)]
struct ProfileTable {
    entries: BTreeMap<String, ProfileEntry>,
    only: Option<Only>,
}
struct Only {
    ids: BTreeSet<String>,
    path: PathBuf,
}
struct ProfileEntry {
    path: PathBuf,
    fields: ProfileFields,
}
#[derive(Default)]
struct ProfileFields {
    enabled: Option<bool>,
    label: Option<String>,
    provider: Option<Provider>,
    model: Option<String>,
    env: Option<BTreeMap<String, String>>,
    capabilities: Option<Vec<String>>,
    command: Option<Vec<String>>,
}

fn profile_table(layer: Option<&ConfigLayer>) -> Result<ProfileTable, ConfigError> {
    let Some(layer) = layer else {
        return Ok(ProfileTable::default());
    };
    let Some(value) = layer.root.get("profiles") else {
        return Ok(ProfileTable::default());
    };
    let table = value
        .as_mapping()
        .ok_or_else(|| invalid(&layer.path, "profiles", "must be a table"))?;
    let mut out = ProfileTable::default();
    for (id, value) in table {
        let Some(id) = id.as_str() else { continue };
        if id == "only" && !value.is_mapping() {
            let values = value.as_sequence().ok_or_else(|| {
                invalid(
                    &layer.path,
                    "profiles.only",
                    "must be a list of profile ids",
                )
            })?;
            let mut ids = BTreeSet::new();
            for v in values {
                let id = v.as_str().filter(|s| valid_id(s)).ok_or_else(|| {
                    invalid(
                        &layer.path,
                        "profiles.only",
                        "must be a list of profile ids",
                    )
                })?;
                ids.insert(id.to_string());
            }
            out.only = Some(Only {
                ids,
                path: layer.path.clone(),
            });
            continue;
        }
        if !valid_id(id) {
            return Err(invalid(
                &layer.path,
                &format!("profiles.{id}"),
                "profile ids allow letters, digits, _ and - only (no dots)",
            ));
        }
        out.entries.insert(
            id.to_owned(),
            ProfileEntry {
                path: layer.path.clone(),
                fields: parse_fields(value, &layer.path, &format!("profiles.{id}"))?,
            },
        );
    }
    Ok(out)
}
fn parse_fields(
    value: &serde_yaml::Value,
    path: &Path,
    field: &str,
) -> Result<ProfileFields, ConfigError> {
    let table = value
        .as_mapping()
        .ok_or_else(|| invalid(path, field, "must be a table"))?;
    let allowed = [
        "enabled",
        "label",
        "provider",
        "model",
        "env",
        "capabilities",
        "command",
    ];
    if let Some(key) = table.keys().find_map(|k| {
        let key = yaml_key(k)?;
        (!allowed.contains(&key)).then_some(key)
    }) {
        return Err(invalid(path, &format!("{field}.{key}"), "unknown field"));
    }
    let enabled = table
        .get("enabled")
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| invalid(path, &format!("{field}.enabled"), "must be true or false"))
        })
        .transpose()?;
    let label = table
        .get("label")
        .map(|v| {
            v.as_str()
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
                .ok_or_else(|| {
                    invalid(
                        path,
                        &format!("{field}.label"),
                        "must be a non-empty string",
                    )
                })
        })
        .transpose()?;
    let provider = table
        .get("provider")
        .map(|v| provider_value(v.as_str().unwrap_or(""), path, &format!("{field}.provider")))
        .transpose()?;
    let model = table
        .get("model")
        .map(|v| {
            v.as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 200)
                .map(|s| s.trim().to_string())
                .ok_or_else(|| {
                    invalid(
                        path,
                        &format!("{field}.model"),
                        "must be a non-empty string of at most 200 characters",
                    )
                })
        })
        .transpose()?;
    let env = table
        .get("env")
        .map(|v| {
            v.as_mapping()
                .ok_or_else(|| invalid(path, &format!("{field}.env"), "must be a table"))
                .map(|t| {
                    t.iter()
                        .filter_map(|(k, v)| {
                            let k = yaml_key(k)?;
                            Some((
                                k.trim().to_string(),
                                v.as_str().map_or_else(
                                    || serde_yaml::to_string(v).unwrap_or_default(),
                                    String::from,
                                ),
                            ))
                        })
                        .collect()
                })
        })
        .transpose()?;
    let capabilities = table
        .get("capabilities")
        .map(|v| {
            v.as_sequence()
                .ok_or_else(|| {
                    invalid(
                        path,
                        &format!("{field}.capabilities"),
                        "must be an array of strings",
                    )
                })
                .and_then(|a| {
                    a.iter()
                        .map(|v| {
                            v.as_str().map(String::from).ok_or_else(|| {
                                invalid(
                                    path,
                                    &format!("{field}.capabilities"),
                                    "must be an array of strings",
                                )
                            })
                        })
                        .collect()
                })
        })
        .transpose()?;
    let command = table
        .get("command")
        .map(|v| {
            v.as_sequence()
                .filter(|a| !a.is_empty())
                .ok_or_else(|| {
                    invalid(
                        path,
                        &format!("{field}.command"),
                        "must be a non-empty array of strings",
                    )
                })
                .and_then(|a| {
                    a.iter()
                        .enumerate()
                        .map(|(i, v)| {
                            v.as_str()
                                .filter(|s| !s.trim().is_empty())
                                .map(String::from)
                                .ok_or_else(|| {
                                    invalid(
                                        path,
                                        &format!("{field}.command[{i}]"),
                                        "must be a non-empty string",
                                    )
                                })
                        })
                        .collect()
                })
        })
        .transpose()?;
    Ok(ProfileFields {
        enabled,
        label,
        provider,
        model,
        env,
        capabilities,
        command,
    })
}
fn merge_profile(
    base: Option<&Profile>,
    user: Option<&ProfileEntry>,
    project: Option<&ProfileEntry>,
    id: &str,
) -> Result<Profile, ConfigError> {
    let mut profile = base.cloned().unwrap_or_else(|| Profile {
        id: id.into(),
        label: id.into(),
        provider: Provider::Claude,
        default_model: String::new(),
        enabled: true,
        env: BTreeMap::new(),
        capabilities: Vec::new(),
        command: None,
    });
    if base.is_none()
        && user.and_then(|e| e.fields.provider).is_none()
        && project.and_then(|e| e.fields.provider).is_none()
    {
        return Err(invalid(
            &project.or(user).unwrap().path,
            &format!("profiles.{id}.provider"),
            &format!("no profile named {id} exists below this file, so provider is required"),
        ));
    }
    for entry in [user, project].into_iter().flatten() {
        let f = &entry.fields;
        if let Some(v) = f.enabled {
            profile.enabled = v
        }
        if let Some(v) = &f.label {
            profile.label = v.clone()
        }
        if let Some(v) = f.provider {
            profile.provider = v
        }
        if let Some(v) = &f.model {
            profile.default_model = v.clone()
        }
        if let Some(v) = &f.env {
            profile.env = v.clone()
        }
        if let Some(v) = &f.capabilities {
            profile.capabilities = v.clone()
        }
        if let Some(v) = &f.command {
            profile.command = Some(v.clone())
        }
    }
    if profile.default_model.is_empty() {
        profile.default_model = default_model(profile.provider).into();
    }
    Ok(profile)
}
fn provider_value(value: &str, path: &Path, field: &str) -> Result<Provider, ConfigError> {
    match value {
        "claude" => Ok(Provider::Claude),
        "codex" => Ok(Provider::Codex),
        "opencode" => Ok(Provider::OpenCode),
        "opencode-2" => Ok(Provider::OpenCode2),
        "antigravity" => Ok(Provider::Antigravity),
        "pi" => Ok(Provider::Pi),
        _ => Err(invalid(
            path,
            field,
            "must be one of claude, codex, opencode, opencode-2, antigravity, pi",
        )),
    }
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && value.as_bytes()[0].is_ascii_alphanumeric()
}
fn invalid(path: &Path, field: &str, message: &str) -> ConfigError {
    ConfigError::Invalid {
        path: path.into(),
        field: field.into(),
        message: message.into(),
    }
}

pub fn mask_secret_env(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .map(|(k, v)| {
            (
                k.clone(),
                if ["KEY", "TOKEN", "SECRET", "PASS"]
                    .iter()
                    .any(|needle| k.to_ascii_uppercase().contains(needle))
                {
                    "••••••••".into()
                } else {
                    v.clone()
                },
            )
        })
        .collect()
}
pub fn config_revision(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
pub fn read_config_file(cwd: Option<&Path>) -> Result<ConfigSnapshot, ConfigError> {
    let path = config_path(cwd);
    let text = fs::read_to_string(&path).unwrap_or_default();
    Ok(ConfigSnapshot {
        path,
        revision: config_revision(&text),
        text,
    })
}
pub fn atomic_write(path: &Path, text: &str) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.into(),
            source,
        })?;
    }
    let temp = path.with_extension(format!("yaml.tmp.{}", std::process::id()));
    fs::write(&temp, text).map_err(|source| ConfigError::Io {
        path: temp.clone(),
        source,
    })?;
    fs::rename(&temp, path).map_err(|source| ConfigError::Io {
        path: path.into(),
        source,
    })
}
pub fn update_config_file(
    cwd: Option<&Path>,
    expected_revision: &str,
    text: &str,
) -> Result<ConfigSnapshot, ConfigError> {
    let current = read_config_file(cwd)?;
    if current.revision != expected_revision {
        return Err(ConfigError::Conflict(current.path));
    }
    if text != current.text {
        atomic_write(&current.path, text)?;
    }
    read_config_file(cwd)
}

/// One directory's saved model settings. A model is written either as a bare
/// boolean or as an object carrying `enabled`, and files written before the
/// rename keep the map under `models`.
pub fn read_model_settings(value: &Value) -> DirectoryModelSettings {
    let mut settings = DirectoryModelSettings::default();
    let Some(profiles) = value.get("profiles").and_then(Value::as_object) else {
        return settings;
    };
    for (profile_id, value) in profiles {
        let Some(profile) = value.as_object() else {
            continue;
        };
        let models = profile
            .get("modelEnabled")
            .or_else(|| profile.get("models"))
            .and_then(Value::as_object);
        let mut model_enabled = BTreeMap::new();
        for (model_id, value) in models.into_iter().flatten() {
            if let Some(enabled) = value
                .as_bool()
                .or_else(|| value.get("enabled").and_then(Value::as_bool))
            {
                model_enabled.insert(model_id.clone(), enabled);
            }
        }
        settings.profiles.insert(
            profile_id.clone(),
            ProfileModelEnablement {
                enabled: profile.get("enabled").and_then(Value::as_bool),
                model_enabled,
            },
        );
    }
    settings
}

/// Read model choices from the YAML layers. Project choices replace the same
/// user choice, while a project's love rules replace the user's whole list.
pub fn read_model_overrides(
    layers: &ConfigLayers,
) -> Result<(ModelOverrides, LoveRules), ConfigError> {
    let mut overrides = ModelOverrides::default();
    let mut love = LoveRules::default();
    for (layer, scope) in [
        (layers.user.as_ref(), "global"),
        (layers.project.as_ref(), "project"),
    ] {
        let Some(layer) = layer else { continue };
        let mut layer_loved = Vec::new();
        if let Some(models) = layer.root.get("models") {
            let table = models
                .as_mapping()
                .ok_or_else(|| invalid(&layer.path, "models", "must be a table"))?;
            for (raw_key, value) in table {
                let Some(key) = yaml_key(raw_key) else {
                    continue;
                };
                if let Some(profile_models) = value.as_mapping()
                    && profile_models.values().any(serde_yaml::Value::is_mapping)
                {
                    for (raw_model, value) in profile_models {
                        let Some(model) = yaml_key(raw_model) else {
                            continue;
                        };
                        let parsed = parse_model_override(
                            value,
                            &layer.path,
                            &format!("models.{key}.{model}"),
                        )?;
                        if parsed.loved == Some(true) {
                            layer_loved.push(LoveRule {
                                model: model.to_owned(),
                                profile_id: Some(key.to_owned()),
                                when: Vec::new(),
                                effort: parsed.effort.clone(),
                                scope: scope.into(),
                            });
                        }
                        overrides
                            .by_profile
                            .entry(key.to_owned())
                            .or_default()
                            .insert(model.to_owned(), parsed);
                    }
                } else {
                    let parsed =
                        parse_model_override(value, &layer.path, &format!("models.{key}"))?;
                    if parsed.loved == Some(true) {
                        layer_loved.push(LoveRule {
                            model: key.to_owned(),
                            profile_id: None,
                            when: Vec::new(),
                            effort: parsed.effort.clone(),
                            scope: scope.into(),
                        });
                    }
                    overrides.shared.insert(key.to_owned(), parsed);
                }
            }
        }
        if layer_loved.len() > 1 {
            let names = layer_loved
                .iter()
                .map(LoveRule::label)
                .collect::<Vec<_>>()
                .join(" and ");
            return Err(invalid(
                &layer.path,
                "models",
                &format!("only one model can be loved at a time; {names} are both marked"),
            ));
        }
        let listed = read_love_list(layer, scope)?;
        match (listed, layer_loved.into_iter().next()) {
            (Some(_), Some(marked)) => {
                return Err(invalid(
                    &layer.path,
                    "love",
                    &format!(
                        "{} is also marked loved under models; write the love list or the flag, \
                         not both",
                        marked.label()
                    ),
                ));
            }
            (Some(rules), None) => love = LoveRules(rules),
            (None, Some(marked)) => love = LoveRules(vec![marked]),
            (None, None) => {}
        }
    }
    Ok((overrides, love))
}

const LOVE_SHAPE: &str =
    "each rule takes model, an optional when list of kinds, and an optional effort";

/// The `love` list of one layer, or nothing when the layer does not write one.
fn read_love_list(layer: &ConfigLayer, scope: &str) -> Result<Option<Vec<LoveRule>>, ConfigError> {
    let Some(value) = layer.root.get("love") else {
        return Ok(None);
    };
    let entries = value.as_sequence().ok_or_else(|| {
        invalid(
            &layer.path,
            "love",
            &format!("must be a list; {LOVE_SHAPE}"),
        )
    })?;
    let mut rules: Vec<LoveRule> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let field = format!("love[{index}]");
        let table = entry.as_mapping().ok_or_else(|| {
            invalid(
                &layer.path,
                &field,
                &format!("must be a table; {LOVE_SHAPE}"),
            )
        })?;
        if let Some(key) = table.keys().find_map(|key| {
            let key = yaml_key(key)?;
            (!["model", "when", "effort"].contains(&key)).then_some(key)
        }) {
            return Err(invalid(
                &layer.path,
                &format!("{field}.{key}"),
                &format!("unknown field; {LOVE_SHAPE}"),
            ));
        }
        let target = table
            .get("model")
            .and_then(serde_yaml::Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| {
                invalid(
                    &layer.path,
                    &format!("{field}.model"),
                    "must name a model, as <worker>:<model> or a bare model id",
                )
            })?;
        let (profile_id, model) = match target.split_once(':') {
            Some((profile_id, model)) if !profile_id.is_empty() && !model.is_empty() => {
                (Some(profile_id.to_owned()), model.to_owned())
            }
            Some(_) => {
                return Err(invalid(
                    &layer.path,
                    &format!("{field}.model"),
                    "must name a model, as <worker>:<model> or a bare model id",
                ));
            }
            None => (None, target.to_owned()),
        };
        let when = match table.get("when") {
            None => Vec::new(),
            Some(when) => {
                let listed = when.as_sequence().ok_or_else(|| {
                    invalid(&layer.path, &format!("{field}.when"), KIND_LIST_MESSAGE)
                })?;
                listed
                    .iter()
                    .map(|value| {
                        value.as_str().and_then(TaskClass::parse).ok_or_else(|| {
                            invalid(&layer.path, &format!("{field}.when"), KIND_LIST_MESSAGE)
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        let effort = table
            .get("effort")
            .map(|effort| {
                effort
                    .as_str()
                    .filter(|effort| EFFORT_LEVELS.contains(effort))
                    .map(String::from)
                    .ok_or_else(|| invalid(&layer.path, &format!("{field}.effort"), EFFORT_MESSAGE))
            })
            .transpose()?;
        if when.is_empty()
            && let Some(other) = rules.iter().find(|rule| rule.when.is_empty())
        {
            return Err(invalid(
                &layer.path,
                "love",
                &format!(
                    "{} and {} both take every other kind of work; only one rule may leave when \
                     out",
                    other.label(),
                    LoveRule {
                        model: model.clone(),
                        profile_id: profile_id.clone(),
                        when: Vec::new(),
                        effort: None,
                        scope: scope.into(),
                    }
                    .label()
                ),
            ));
        }
        if let Some(class) = when
            .iter()
            .find(|class| rules.iter().any(|rule| rule.when.contains(class)))
        {
            return Err(invalid(
                &layer.path,
                "love",
                &format!(
                    "two rules claim {} work; a kind of work belongs to one rule",
                    class.as_str()
                ),
            ));
        }
        rules.push(LoveRule {
            model,
            profile_id,
            when,
            effort,
            scope: scope.into(),
        });
    }
    Ok(Some(rules))
}

/// The worker rules one `.oga.yaml` writes for its own scope. `prompt` is the
/// whole table: whatever it holds ships verbatim under `## Worker rules`, read
/// again on every dispatch. Any other key is a rule the writer expects Oga to
/// honour and Oga would silently drop, so it fails the read instead.
pub fn read_worker_prompt(layer: Option<&ConfigLayer>) -> Result<Option<String>, ConfigError> {
    let Some(layer) = layer else {
        return Ok(None);
    };
    let Some(worker) = layer.root.get("worker") else {
        return Ok(None);
    };
    let table = worker
        .as_mapping()
        .ok_or_else(|| invalid(&layer.path, "worker", WORKER_SHAPE))?;
    if let Some(key) = table.keys().find_map(|key| {
        let key = yaml_key(key)?;
        (key != "prompt").then_some(key)
    }) {
        return Err(invalid(
            &layer.path,
            &format!("worker.{key}"),
            &format!("unknown key; {WORKER_SHAPE}"),
        ));
    }
    table
        .get("prompt")
        .map(|prompt| {
            prompt
                .as_str()
                .map(|prompt| prompt.trim().to_owned())
                .ok_or_else(|| {
                    invalid(
                        &layer.path,
                        "worker.prompt",
                        "must be text; write it as a block with `prompt: |`",
                    )
                })
        })
        .transpose()
}

const WORKER_SHAPE: &str = "worker takes one key, prompt, holding the rules text";

fn parse_model_override(
    value: &serde_yaml::Value,
    path: &Path,
    field: &str,
) -> Result<ModelOverride, ConfigError> {
    let table = value
        .as_mapping()
        .ok_or_else(|| invalid(path, field, "must be a table"))?;
    let allowed = ["enabled", "preferred", "loved", "effort", "capabilities"];
    if let Some(key) = table.keys().find_map(|key| {
        let key = yaml_key(key)?;
        (!allowed.contains(&key)).then_some(key)
    }) {
        return Err(invalid(path, &format!("{field}.{key}"), "unknown field"));
    }
    let boolean = |name: &str| {
        table
            .get(name)
            .map(|value| {
                value.as_bool().ok_or_else(|| {
                    invalid(path, &format!("{field}.{name}"), "must be true or false")
                })
            })
            .transpose()
    };
    if let Some(effort) = table.get("effort")
        && effort
            .as_str()
            .is_none_or(|effort| !EFFORT_LEVELS.contains(&effort))
    {
        return Err(invalid(path, &format!("{field}.effort"), EFFORT_MESSAGE));
    }
    let capabilities = table
        .get("capabilities")
        .map(|value| {
            value
                .as_sequence()
                .ok_or_else(|| {
                    invalid(
                        path,
                        &format!("{field}.capabilities"),
                        "must be an array of strings",
                    )
                })?
                .iter()
                .map(|value| {
                    value.as_str().map(String::from).ok_or_else(|| {
                        invalid(
                            path,
                            &format!("{field}.capabilities"),
                            "must be an array of strings",
                        )
                    })
                })
                .collect()
        })
        .transpose()?;
    Ok(ModelOverride {
        enabled: boolean("enabled")?,
        preferred: boolean("preferred")?,
        loved: boolean("loved")?,
        effort: table
            .get("effort")
            .and_then(serde_yaml::Value::as_str)
            .map(String::from),
        capabilities,
    })
}
/// Whether a worker may run in this directory at all. Both scopes have to
/// agree, and silence is consent: a worker nobody has ruled on stays
/// available, which is what makes an untouched install behave as before.
pub fn profile_enabled(settings: &ResolvedModelSettings, profile: &str) -> bool {
    if settings
        .global
        .profiles
        .get(profile)
        .and_then(|p| p.enabled)
        == Some(false)
    {
        return false;
    }
    settings
        .project
        .as_ref()
        .and_then(|s| s.profiles.get(profile))
        .and_then(|p| p.enabled)
        != Some(false)
}

/// Whether the user has turned this model on for this worker. Silence is a no:
/// a model nobody has ruled on cannot run, so discovering one never authorises
/// it to spend anything.
pub fn model_enabled(settings: &ResolvedModelSettings, profile: &str, model: &str) -> bool {
    if settings
        .global
        .profiles
        .get(profile)
        .and_then(|p| p.enabled)
        == Some(false)
        || settings
            .project
            .as_ref()
            .and_then(|s| s.profiles.get(profile))
            .and_then(|p| p.enabled)
            == Some(false)
    {
        return false;
    }
    if let Some(value) = settings
        .overrides
        .as_ref()
        .and_then(|o| model_override_for(o, profile, model))
        .and_then(|o| o.enabled)
    {
        return value;
    }
    if let Some(value) = settings
        .project
        .as_ref()
        .and_then(|s| s.profiles.get(profile))
        .and_then(|p| p.model_enabled.get(model))
    {
        return *value;
    }
    settings
        .global
        .profiles
        .get(profile)
        .and_then(|p| p.model_enabled.get(model))
        .copied()
        == Some(true)
}

/// What someone is told when their task never reached a provider because the
/// model it wanted has not been turned on.
pub fn model_not_enabled_message(profile: &str, model: &str) -> String {
    format!(
        "{model} is not turned on for {profile}, so this task was not sent anywhere. Open \
         Settings, find {profile} under Workers, and switch {model} on."
    )
}

/// What someone is told when nothing at all is available to take their work.
pub const NO_MODEL_ENABLED_MESSAGE: &str = "No models are turned on, so this task was not sent anywhere. Open Settings, pick a worker, \
     and switch on the models it may use.";

pub fn model_override_for(
    overrides: &ModelOverrides,
    profile: &str,
    model: &str,
) -> Option<ModelOverride> {
    let shared = overrides.shared.get(model);
    let scoped = overrides.by_profile.get(profile).and_then(|p| p.get(model));
    if shared.is_none() && scoped.is_none() {
        return None;
    }
    Some(ModelOverride {
        enabled: scoped
            .and_then(|v| v.enabled)
            .or_else(|| shared.and_then(|v| v.enabled)),
        preferred: scoped
            .and_then(|v| v.preferred)
            .or_else(|| shared.and_then(|v| v.preferred)),
        loved: scoped
            .and_then(|v| v.loved)
            .or_else(|| shared.and_then(|v| v.loved)),
        effort: scoped
            .and_then(|v| v.effort.clone())
            .or_else(|| shared.and_then(|v| v.effort.clone())),
        capabilities: scoped
            .and_then(|v| v.capabilities.clone())
            .or_else(|| shared.and_then(|v| v.capabilities.clone())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile {
            id: "main".into(),
            label: "Main".into(),
            provider: Provider::Claude,
            default_model: "sonnet".into(),
            enabled: true,
            env: BTreeMap::from([("API_KEY".into(), "secret".into())]),
            capabilities: vec!["tools".into()],
            command: None,
        }
    }

    fn layer(path: &str, source: &str) -> ConfigLayer {
        ConfigLayer {
            path: path.into(),
            root: serde_yaml::from_str(source).unwrap(),
        }
    }

    #[test]
    fn project_fields_override_user_and_preserve_unmentioned_fields() {
        let user = layer(
            "/home/user/.oga.yaml",
            r#"
            profiles:
              main:
                label: User
                model: opus
        "#,
        );
        let project = layer(
            "/work/.oga.yaml",
            r#"
            profiles:
              main:
                enabled: false
        "#,
        );
        let resolved = resolve_profiles(vec![profile()], Some(&user), Some(&project)).unwrap();
        assert_eq!(resolved.profiles[0].label, "User");
        assert_eq!(resolved.profiles[0].default_model, "opus");
        assert!(!resolved.profiles[0].enabled);
        assert_eq!(resolved.sources["main"].source, "/work/.oga.yaml");
        assert_eq!(
            resolved.sources["main"].sources,
            ["/work/.oga.yaml", "/home/user/.oga.yaml"]
        );
    }

    #[test]
    fn new_profiles_require_provider_and_use_provider_default_model() {
        let config_layer = layer(
            "/home/user/.oga.yaml",
            r#"
            profiles:
              codex:
                provider: codex
        "#,
        );
        let resolved = resolve_profiles(vec![], Some(&config_layer), None).unwrap();
        assert_eq!(resolved.profiles[0].default_model, "gpt-5");
        let invalid_layer = layer("/bad", "profiles:\n  new:\n    label: x");
        assert!(resolve_profiles(vec![], Some(&invalid_layer), None).is_err());
    }

    #[test]
    fn only_narrows_profiles_and_masks_secret_environment_values() {
        let layer = layer(
            "/work/.oga.yaml",
            r#"
            profiles:
              only: [main]
              other:
                provider: pi
        "#,
        );
        let resolved = resolve_profiles(vec![profile()], None, Some(&layer)).unwrap();
        assert_eq!(resolved.profiles.len(), 1);
        assert_eq!(resolved.excluded[0].id, "other");
        assert_eq!(mask_secret_env(&profile().env)["API_KEY"], "••••••••");
    }

    #[test]
    fn model_settings_are_project_first_and_revision_is_stable() {
        let global = DirectoryModelSettings {
            profiles: BTreeMap::from([(
                "main".into(),
                ProfileModelEnablement {
                    enabled: None,
                    model_enabled: BTreeMap::from([("opus".into(), false)]),
                },
            )]),
        };
        let project = DirectoryModelSettings::default();
        let settings = ResolvedModelSettings {
            global,
            project: Some(project),
            overrides: None,
            love: LoveRules::default(),
        };
        assert!(!model_enabled(&settings, "main", "opus"));
        assert_eq!(config_revision("same"), config_revision("same"));
        assert_ne!(config_revision("same"), config_revision("changed"));
    }

    fn resolved(
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

    fn switched(profile: &str, model: &str, on: bool) -> DirectoryModelSettings {
        DirectoryModelSettings {
            profiles: BTreeMap::from([(
                profile.into(),
                ProfileModelEnablement {
                    enabled: None,
                    model_enabled: BTreeMap::from([(model.into(), on)]),
                },
            )]),
        }
    }

    #[test]
    fn a_model_runs_only_where_someone_switched_it_on() {
        // Nobody has ruled on it, so it cannot spend anything.
        assert!(!model_enabled(
            &resolved(DirectoryModelSettings::default(), None),
            "main",
            "opus"
        ));

        // On globally, then off again at either scope.
        let on = switched("main", "opus", true);
        assert!(model_enabled(&resolved(on.clone(), None), "main", "opus"));
        assert!(!model_enabled(
            &resolved(on.clone(), Some(switched("main", "opus", false))),
            "main",
            "opus"
        ));
        assert!(!model_enabled(
            &resolved(switched("main", "opus", false), None),
            "main",
            "opus"
        ));

        // A project may switch one on that the global scope never mentions.
        assert!(model_enabled(
            &resolved(DirectoryModelSettings::default(), Some(on)),
            "main",
            "opus"
        ));

        // Turning the worker off overrides any model switch under it.
        let mut worker_off = switched("main", "opus", true);
        worker_off
            .profiles
            .get_mut("main")
            .expect("main is present")
            .enabled = Some(false);
        assert!(!model_enabled(&resolved(worker_off, None), "main", "opus"));
    }

    #[test]
    fn saved_settings_read_both_shapes_a_model_switch_is_written_in() {
        let settings = read_model_settings(&serde_json::json!({
            "profiles": {
                "main": {
                    "enabled": true,
                    "models": { "opus": { "enabled": true }, "haiku": false }
                }
            }
        }));
        let main = settings.profiles.get("main").expect("main is present");

        assert_eq!(main.enabled, Some(true));
        assert_eq!(main.model_enabled.get("opus"), Some(&true));
        assert_eq!(main.model_enabled.get("haiku"), Some(&false));
        assert!(
            read_model_settings(&serde_json::json!({}))
                .profiles
                .is_empty()
        );
    }

    #[test]
    fn model_overrides_resolve_profiled_and_bare_loved_entries_by_scope() {
        let user = layer(
            "/home/user/.oga.yaml",
            "models:\n  openai/gpt-5.6-luna:\n    loved: true\n",
        );
        let project = layer(
            "/work/.oga.yaml",
            "models:\n  opencode:\n    openai/gpt-5.6-luna:\n      loved: true\n",
        );
        let (overrides, loved) = read_model_overrides(&ConfigLayers {
            user: Some(user),
            project: Some(project),
        })
        .unwrap();

        assert_eq!(overrides.shared["openai/gpt-5.6-luna"].loved, Some(true));
        assert_eq!(
            overrides.by_profile["opencode"]["openai/gpt-5.6-luna"].loved,
            Some(true)
        );
        assert_eq!(
            loved,
            LoveRules(vec![LoveRule {
                model: "openai/gpt-5.6-luna".into(),
                profile_id: Some("opencode".into()),
                when: Vec::new(),
                effort: None,
                scope: "project".into(),
            }])
        );
    }

    #[test]
    fn love_rules_read_kinds_and_effort_per_rule() {
        let project = layer(
            "/work/.oga.yaml",
            r#"
            love:
              - model: opencode:openai/gpt-5.6-luna
                when: [context, mechanical]
                effort: low
              - model: opencode:openai/gpt-5.6-luna
                when: [reasoning, build]
                effort: max
              - model: claude:opus
        "#,
        );
        let (_, love) = read_model_overrides(&ConfigLayers {
            user: None,
            project: Some(project),
        })
        .unwrap();

        let context = love.for_class(TaskClass::Context).unwrap();
        assert_eq!(context.model, "openai/gpt-5.6-luna");
        assert_eq!(context.effort.as_deref(), Some("low"));
        assert_eq!(
            love.for_class(TaskClass::Reasoning)
                .unwrap()
                .effort
                .as_deref(),
            Some("max")
        );
        assert_eq!(love.for_class(TaskClass::General).unwrap().model, "opus");
        assert!(love.names_model("claude", "opus"));
    }

    #[test]
    fn love_rules_of_a_project_replace_the_global_list() {
        let user = layer("/home/user/.oga.yaml", "love:\n  - model: claude:opus\n");
        let project = layer(
            "/work/.oga.yaml",
            "love:\n  - model: opencode:kimi\n    when: [context]\n",
        );
        let (_, love) = read_model_overrides(&ConfigLayers {
            user: Some(user),
            project: Some(project),
        })
        .unwrap();

        assert_eq!(love.for_class(TaskClass::Context).unwrap().model, "kimi");
        assert!(love.for_class(TaskClass::Reasoning).is_none());
    }

    #[test]
    fn love_rules_reject_a_kind_claimed_twice_and_a_second_catch_all() {
        let twice = layer(
            "/work/.oga.yaml",
            "love:\n  - model: a\n    when: [build]\n  - model: b\n    when: [build]\n",
        );
        assert_eq!(
            read_model_overrides(&ConfigLayers {
                user: None,
                project: Some(twice),
            })
            .unwrap_err()
            .to_string(),
            "invalid config /work/.oga.yaml at love: two rules claim build work; a kind of work belongs to one rule"
        );

        let two_catch_alls = layer("/work/.oga.yaml", "love:\n  - model: a\n  - model: b\n");
        assert!(
            read_model_overrides(&ConfigLayers {
                user: None,
                project: Some(two_catch_alls),
            })
            .unwrap_err()
            .to_string()
            .contains("both take every other kind of work")
        );
    }

    #[test]
    fn love_rules_reject_unknown_fields_kinds_and_efforts() {
        for (source, message) in [
            (
                "love:\n  - model: a\n    kind: [build]\n",
                "invalid config /work/.oga.yaml at love[0].kind: unknown field; each rule takes model, an optional when list of kinds, and an optional effort",
            ),
            (
                "love:\n  - model: a\n    when: [refactor]\n",
                "invalid config /work/.oga.yaml at love[0].when: must be a list of kinds of work: mechanical, context, build, reasoning, general",
            ),
            (
                "love:\n  - model: a\n    effort: enormous\n",
                "invalid config /work/.oga.yaml at love[0].effort: must be one of minimal, low, medium, high, xhigh, max",
            ),
            (
                "love:\n  - when: [build]\n",
                "invalid config /work/.oga.yaml at love[0].model: must name a model, as <worker>:<model> or a bare model id",
            ),
        ] {
            let error = read_model_overrides(&ConfigLayers {
                user: None,
                project: Some(layer("/work/.oga.yaml", source)),
            })
            .unwrap_err();
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn love_rules_and_the_loved_flag_cannot_share_a_layer() {
        let project = layer(
            "/work/.oga.yaml",
            "love:\n  - model: claude:opus\nmodels:\n  claude:\n    haiku:\n      loved: true\n",
        );
        let error = read_model_overrides(&ConfigLayers {
            user: None,
            project: Some(project),
        })
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "invalid config /work/.oga.yaml at love: claude/haiku is also marked loved under models; write the love list or the flag, not both"
        );
    }

    #[test]
    fn worker_prompt_reads_the_block_verbatim() {
        let project = layer(
            "/work/.oga.yaml",
            "version: 1\nworker:\n  prompt: |\n    1. Blocked means stop.\n    2. Report what you ran.\n",
        );

        assert_eq!(
            read_worker_prompt(Some(&project)).unwrap().as_deref(),
            Some("1. Blocked means stop.\n2. Report what you ran.")
        );
    }

    #[test]
    fn worker_prompt_is_absent_without_a_table() {
        assert!(read_worker_prompt(None).unwrap().is_none());
        assert!(
            read_worker_prompt(Some(&layer("/work/.oga.yaml", "version: 1\n")))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn worker_prompt_rejects_every_other_key() {
        let project = layer("/work/.oga.yaml", "worker:\n  tldr_sentences: 1-3\n");
        let error = read_worker_prompt(Some(&project)).unwrap_err();

        assert_eq!(
            error.to_string(),
            "invalid config /work/.oga.yaml at worker.tldr_sentences: unknown key; worker takes one key, prompt, holding the rules text"
        );
    }

    #[test]
    fn worker_prompt_rejects_a_non_text_block() {
        let project = layer("/work/.oga.yaml", "worker:\n  prompt:\n    - one\n");

        assert!(read_worker_prompt(Some(&project)).is_err());
    }
}
