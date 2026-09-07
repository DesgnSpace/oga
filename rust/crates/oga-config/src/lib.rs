//! Profile discovery, YAML layers, cwd settings, and prompts.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use oga_domain::{Profile, Provider, TaskClass, TaskTopic, WorkKind};
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
const KIND_LIST_MESSAGE: &str = "must be a list of kinds of work: mechanical, context, build, reasoning, general, ui, ux, \
     backend, database, docs, tests, review, research, refactor";

pub const MODEL_SETTINGS_KEY: &str = "models";
pub const PROMPTS_KEY: &str = "prompts";
/// The worker prompt a fresh settings file starts from, editable in Settings
/// or overridden per project from `.oga.yaml`. It is a default, not a frame:
/// plain text that is sent as written once `{{brief}}`, `{{scope}}`,
/// `{{memories}}`, `{{attribution}}`, and `{{reporting}}`
/// are filled in per task, with the run itself as `{{task_id}}`,
/// `{{provider}}`, `{{model}}`, `{{effort}}`. A user who deletes everything
/// and writes one sentence gets one sentence sent. Code adds nothing except
/// the task slot itself, first, when it is missing.
pub const DEFAULT_WORKER_PROMPT: &str = concat!(
    "Worker mode: you are executing an assigned Oga task.\n",
    "Continue the assigned brief directly. Do not use Oga to delegate, resume, or manage another task, and do not create a child task for the same work.\n",
    "\n",
    "{{brief}}\n",
    "\n",
    "{{scope}}\n",
    "\n",
    "## Worker rules\n",
    "1. Blocked means stop. A command that will not run, a missing credential, an account or signup, a permission denial, a path outside your scope, a decision this brief does not answer — stop and report it, naming the blocker and the one decision you need.\n",
    "2. Do not work around a blocker. No retry loops, no second tool for the same job, no creating accounts, no linking or authenticating anything, no faking or stubbing the result. Stop, finish what does not depend on it, then report the exact command or path and say whether you need the caller to decide or to run it and return the output.\n",
    "3. Partial work is a valid result. Finish what is unblocked, then report what you stopped on.\n",
    "4. Never report a result you did not observe. If you could not run a check, say so and say why, instead of describing an outcome you did not see.\n",
    "5. Open your final report with `## TL;DR` — 1-3 plain-language sentences stating what was done or found and the outcome. Detail follows after; this applies to your final answer, not to intermediate messages.\n",
    "6. Write that TL;DR as bullets — one idea per line, never a paragraph — and make it stand alone: no bullet may need the detail below it to make sense.\n",
    "7. Keep it to roughly ten lines or fewer: the verdict; what the work did or decided, one meaningful line per decision; checks run and their results, quoting failures exactly; and what is left, broken, or uncertain, or \"nothing\".\n",
    "8. Describe meaning, not a file list. Name a file only when the file itself is the point, such as a moved file, deleted feature, or new entry point. Keep the branch line for worktree tasks.\n",
    "9. Clear local, reversible obstacles yourself — a stray generated file blocking a checkout, a stale lockfile, a missing directory, a tool needing a flag — decide, apply the fix, retry, and note it in the report. Stop only when the obstacle needs the caller: a credential, a scope or product decision, or an action that is irreversible or outside scope. A blocker is a decision you cannot make, not a step that failed once.\n",
    "10. If a clearly separate continuation is needed, state why it is separate and emit a compact caller-facing pointer with the child task ID and title, so the caller can start `oga watch <childTaskId>` and inspect after settlement. Do not include prompt or output in the pointer.\n",
    "11. Finding code starts with `oga query \"<what you need>\"`, every time, before any `find`, `rg`, `grep`, or glob. It is the project's own index: it takes a plain description, not just a name, and answers with the file, symbol, and line, kept in step with the working tree. Add `--code` when you want the code back instead of just the location, so you do not have to open the file. Fall back to `rg` or `find` only when query returns no match, or when the task needs every occurrence rather than the right place. Use the returned code before acting.\n",
    "12. Run the relevant checks before reporting completion, and say what you ran. Run JavaScript checks with `bun` or `bunx`; existing failures on the base branch do not block delivery.\n",
    "13. Commit the work, push the branch, and open a pull request with `gh pr create --base main`. After the pull request, run `oga relearn` once with symbols that exist in the diff; rejected route hints are a warning when the deliverable already exists.\n",
    "\n",
    "{{memories}}\n",
    "\n",
    "{{attribution}}\n",
    "\n",
    "{{reporting}}"
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
/// One place work can go: a worker, a model on it, and the thinking level it
/// uses. Written `worker:model:effort` with the model or the effort left out:
/// a missing model means the worker's own default, a missing effort means the
/// kind of work prices it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoveDestination {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl LoveDestination {
    /// Read one `worker:model:effort` destination, with the model or the
    /// effort left out. The worker always comes first, so a bare `opus` is
    /// the worker's default model, never a model id.
    pub fn parse(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err("name a destination, like 'claude:opus'".into());
        }
        let parts: Vec<&str> = trimmed.split(':').map(str::trim).collect();
        let worker = parts[0];
        if worker.is_empty() {
            return Err(format!(
                "name the worker first in '{trimmed}', like 'claude:opus'"
            ));
        }
        let destination = |model: Option<String>, effort: Option<String>| LoveDestination {
            profile_id: Some(worker.to_owned()),
            model,
            effort,
        };
        match parts.as_slice() {
            [_] => Ok(destination(None, None)),
            [_, second] => {
                if second.is_empty() {
                    return Err(format!(
                        "name a model or an effort after '{worker}:' in '{trimmed}'"
                    ));
                }
                if EFFORT_LEVELS.contains(second) {
                    Ok(destination(None, Some((*second).to_owned())))
                } else {
                    Ok(destination(Some((*second).to_owned()), None))
                }
            }
            [_, model, effort] => {
                if model.is_empty() {
                    return Err(format!("name a model in '{trimmed}', like '{worker}:opus'"));
                }
                if !EFFORT_LEVELS.contains(effort) {
                    return Err(format!(
                        "'{effort}' is not an effort in '{trimmed}'; choose one of {}",
                        EFFORT_LEVELS.join(", ")
                    ));
                }
                Ok(destination(
                    Some((*model).to_owned()),
                    Some((*effort).to_owned()),
                ))
            }
            _ => Err(format!(
                "a destination is worker:model:effort; '{trimmed}' has too many parts"
            )),
        }
    }

    /// Whether a catalog entry is this destination. A worker-scoped
    /// destination pins the account too; a bare model id matches wherever it
    /// is offered; a missing model means the worker's own default.
    pub fn matches(&self, profile_id: &str, model: &str, default_model: Option<&str>) -> bool {
        if self
            .profile_id
            .as_deref()
            .is_some_and(|named| named != profile_id)
        {
            return false;
        }
        match (&self.profile_id, &self.model) {
            (Some(_), None) => default_model == Some(model),
            (_, Some(wanted)) => wanted == model,
            (None, None) => false,
        }
    }

    /// How the destination is written and read back: `worker:model:effort`
    /// with the missing parts left out.
    pub fn triple(&self) -> String {
        let mut out = self
            .profile_id
            .clone()
            .or_else(|| self.model.clone())
            .unwrap_or_default();
        if let Some(model) = &self.model
            && self.profile_id.is_some()
        {
            out.push(':');
            out.push_str(model);
        }
        if let Some(effort) = &self.effort {
            out.push(':');
            out.push_str(effort);
        }
        out
    }

    /// How the destination reads in a sentence: `<worker>/<model>`, the bare
    /// model, or the worker alone when it stands for its default model.
    pub fn label(&self) -> String {
        match (&self.profile_id, &self.model) {
            (Some(profile_id), Some(model)) => format!("{profile_id}/{model}"),
            (Some(profile_id), None) => profile_id.clone(),
            (None, Some(model)) => model.clone(),
            (None, None) => String::new(),
        }
    }

    /// A chain of destinations, first tried to last, for listings and warnings.
    pub fn chain_label(all: &[LoveDestination]) -> String {
        all.iter()
            .map(LoveDestination::label)
            .collect::<Vec<_>>()
            .join(" → ")
    }
}

/// One standing answer to "where does work that names no model go": an
/// ordered list of destinations, and the kinds of work they take. A kind is
/// either a class of work or a subject; `general` and an empty `when` both
/// take every kind no other rule claims. The list reads top to bottom: the
/// first destination that can take the work runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoveRule {
    pub destinations: Vec<LoveDestination>,
    pub when: Vec<WorkKind>,
    pub scope: String,
}

impl LoveRule {
    /// The destination tried first. Rules always hold at least one: the
    /// reader refuses a rule that names none.
    pub fn primary(&self) -> &LoveDestination {
        self.destinations
            .first()
            .expect("a love rule names at least one destination")
    }

    /// Whether a catalog entry is one of this rule's destinations.
    pub fn names_model(&self, profile_id: &str, model: &str, default_model: Option<&str>) -> bool {
        self.destinations
            .iter()
            .any(|destination| destination.matches(profile_id, model, default_model))
    }

    /// How this rule came to own the task: the first kind it names that
    /// describes the work, its topic before its class, else the fallback it
    /// offers by naming `general` or by naming nothing at all.
    pub fn match_for(&self, class: TaskClass, topic: Option<TaskTopic>) -> LoveMatch {
        if let Some(topic) = topic
            && let Some(kind) = self.when.iter().find(|kind| kind.as_topic() == Some(topic))
        {
            return LoveMatch::Claimed(*kind);
        }
        if let Some(kind) = self.when.iter().find(|kind| kind.as_class() == Some(class)) {
            return LoveMatch::Claimed(*kind);
        }
        if self.when.contains(&WorkKind::General) {
            return LoveMatch::Fallback;
        }
        LoveMatch::CatchAll
    }

    /// How the rule is written and read back: its first destination.
    pub fn label(&self) -> String {
        self.primary().label()
    }

    /// The whole chain, first tried to last, for listings and warnings.
    pub fn chain_label(&self) -> String {
        LoveDestination::chain_label(&self.destinations)
    }

    /// Every destination in file syntax, for `models` lists.
    pub fn triples(&self) -> Vec<String> {
        self.destinations
            .iter()
            .map(LoveDestination::triple)
            .collect()
    }
}

impl Serialize for LoveRule {
    /// Flat legacy fields describe the first destination, so readers written
    /// before the list keep working; `models` carries the whole chain.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let primary = self.primary();
        let mut out = serializer.serialize_struct("LoveRule", 6)?;
        out.serialize_field("model", primary.model.as_deref().unwrap_or(""))?;
        if let Some(profile_id) = &primary.profile_id {
            out.serialize_field("profileId", profile_id)?;
        }
        if let Some(effort) = &primary.effort {
            out.serialize_field("effort", effort)?;
        }
        out.serialize_field("models", &self.destinations)?;
        out.serialize_field(
            "when",
            &self
                .when
                .iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<_>>(),
        )?;
        out.serialize_field("scope", &self.scope)?;
        out.end()
    }
}

impl<'de> Deserialize<'de> for LoveRule {
    /// Reads the file shape: `model` with an optional `effort`, or a
    /// `models` list of `worker:model:effort` destinations, never both.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct LoveRuleIn {
            #[serde(default)]
            model: Option<String>,
            #[serde(default)]
            models: Option<Vec<String>>,
            #[serde(default)]
            when: Vec<String>,
            #[serde(default)]
            effort: Option<String>,
            #[serde(default)]
            scope: String,
        }
        let raw = LoveRuleIn::deserialize(deserializer)?;
        if raw.model.is_some() && raw.models.is_some() {
            return Err(serde::de::Error::custom(
                "write model or models, not both; a list of destinations goes under models",
            ));
        }
        let when = raw
            .when
            .iter()
            .map(|kind| {
                WorkKind::parse(kind).ok_or_else(|| {
                    serde::de::Error::custom(format!("there is no kind of work called '{kind}'"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(models) = raw.models {
            if models.is_empty() {
                return Err(serde::de::Error::custom(
                    "models must name at least one destination, like 'claude:opus'",
                ));
            }
            let destinations = models
                .iter()
                .map(|destination| {
                    LoveDestination::parse(destination).map_err(serde::de::Error::custom)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(duplicated) = destinations
                .iter()
                .enumerate()
                .find(|(index, destination)| destinations[..*index].contains(destination))
                .map(|(_, destination)| destination.triple())
            {
                return Err(serde::de::Error::custom(format!(
                    "'{duplicated}' names the same destination twice"
                )));
            }
            if raw.effort.is_some() {
                return Err(serde::de::Error::custom(
                    "write the effort with each destination, like 'claude:opus:low'",
                ));
            }
            return Ok(LoveRule {
                destinations,
                when,
                scope: raw.scope,
            });
        }
        let target = raw
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| {
                serde::de::Error::custom(
                    "must name a model, as <worker>:<model> or a bare model id",
                )
            })?;
        let (profile_id, model) = match target.split_once(':') {
            Some((profile_id, model)) if !profile_id.is_empty() && !model.is_empty() => {
                (Some(profile_id.to_owned()), model.to_owned())
            }
            Some(_) => {
                return Err(serde::de::Error::custom(
                    "must name a model, as <worker>:<model> or a bare model id",
                ));
            }
            None => (None, target.to_owned()),
        };
        let effort = raw
            .effort
            .map(|effort| {
                EFFORT_LEVELS
                    .contains(&effort.as_str())
                    .then_some(effort)
                    .ok_or_else(|| serde::de::Error::custom(EFFORT_MESSAGE))
            })
            .transpose()?;
        Ok(LoveRule {
            destinations: vec![LoveDestination {
                profile_id,
                model: Some(model),
                effort,
            }],
            when,
            scope: raw.scope,
        })
    }
}

/// Why a love rule owns a task, for the record the caller reads back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoveMatch {
    /// The rule names this task's subject or its class.
    Claimed(WorkKind),
    /// The rule names `general`, so it takes what nothing else claims.
    Fallback,
    /// The rule names no kind, so it takes what nothing else claims.
    CatchAll,
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

    /// The rule that owns this task: the one naming its subject first, then
    /// the one naming its class, then `general`, else the catch-all, else
    /// nothing. `general` is the word for work with nothing particular about
    /// it, so a rule naming it takes whatever no other rule claimed — and it
    /// goes before the catch-all, since writing the word is a choice and
    /// leaving `when` off is not. Within one tier the file order wins, so the
    /// fleet reads top to bottom.
    pub fn for_task(&self, class: TaskClass, topic: Option<TaskTopic>) -> Option<&LoveRule> {
        if let Some(topic) = topic
            && let Some(rule) = self
                .0
                .iter()
                .find(|rule| rule.when.iter().any(|kind| kind.as_topic() == Some(topic)))
        {
            return Some(rule);
        }
        self.0
            .iter()
            .find(|rule| rule.when.iter().any(|kind| kind.as_class() == Some(class)))
            .or_else(|| {
                self.0
                    .iter()
                    .find(|rule| rule.when.contains(&WorkKind::General))
            })
            .or_else(|| self.0.iter().find(|rule| rule.when.is_empty()))
    }

    /// The rule that owns this kind of work: the one claiming it, else the
    /// catch-all, else nothing.
    pub fn for_class(&self, class: TaskClass) -> Option<&LoveRule> {
        self.for_task(class, None)
    }

    /// Whether any rule sends work to this model.
    pub fn names_model(&self, profile_id: &str, model: &str, default_model: Option<&str>) -> bool {
        self.0
            .iter()
            .any(|rule| rule.names_model(profile_id, model, default_model))
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
                                destinations: vec![LoveDestination {
                                    profile_id: Some(key.to_owned()),
                                    model: Some(model.to_owned()),
                                    effort: parsed.effort.clone(),
                                }],
                                when: Vec::new(),
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
                            destinations: vec![LoveDestination {
                                profile_id: None,
                                model: Some(key.to_owned()),
                                effort: parsed.effort.clone(),
                            }],
                            when: Vec::new(),
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

const LOVE_SHAPE: &str = "each rule takes model or a models list of destinations, an optional when list of kinds, and an optional effort";

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
            (!["model", "models", "when", "effort"].contains(&key)).then_some(key)
        }) {
            return Err(invalid(
                &layer.path,
                &format!("{field}.{key}"),
                &format!("unknown field; {LOVE_SHAPE}"),
            ));
        }
        let has_model = table.contains_key("model");
        let has_models = table.contains_key("models");
        if has_model && has_models {
            return Err(invalid(
                &layer.path,
                &field,
                "write model or models, not both; a list of destinations goes under models",
            ));
        }
        let destinations = if has_models {
            if table.contains_key("effort") {
                return Err(invalid(
                    &layer.path,
                    &format!("{field}.effort"),
                    "write the effort with each destination, like 'claude:opus:low'",
                ));
            }
            let listed = table
                .get("models")
                .and_then(serde_yaml::Value::as_sequence)
                .ok_or_else(|| {
                    invalid(
                        &layer.path,
                        &format!("{field}.models"),
                        "must be a list of destinations, like [opencode:luna:max, claude:opus:low]",
                    )
                })?;
            if listed.is_empty() {
                return Err(invalid(
                    &layer.path,
                    &format!("{field}.models"),
                    "must name at least one destination, like 'claude:opus'",
                ));
            }
            listed
                .iter()
                .enumerate()
                .map(|(position, value)| {
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|destination| !destination.is_empty())
                        .ok_or_else(|| {
                            invalid(
                                &layer.path,
                                &format!("{field}.models[{position}]"),
                                "must name a destination, like 'claude:opus'",
                            )
                        })
                        .and_then(|destination| {
                            LoveDestination::parse(destination).map_err(|message| {
                                invalid(
                                    &layer.path,
                                    &format!("{field}.models[{position}]"),
                                    &message,
                                )
                            })
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
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
            let effort = table
                .get("effort")
                .map(|effort| {
                    effort
                        .as_str()
                        .filter(|effort| EFFORT_LEVELS.contains(effort))
                        .map(String::from)
                        .ok_or_else(|| {
                            invalid(&layer.path, &format!("{field}.effort"), EFFORT_MESSAGE)
                        })
                })
                .transpose()?;
            vec![LoveDestination {
                profile_id,
                model: Some(model),
                effort,
            }]
        };
        if let Some(duplicated) = destinations
            .iter()
            .enumerate()
            .find(|(index, destination)| destinations[..*index].contains(destination))
            .map(|(_, destination)| destination.triple())
        {
            return Err(invalid(
                &layer.path,
                &field,
                &format!("'{duplicated}' names the same destination twice"),
            ));
        }
        let when = match table.get("when") {
            None => Vec::new(),
            Some(when) => {
                let listed = when.as_sequence().ok_or_else(|| {
                    invalid(&layer.path, &format!("{field}.when"), KIND_LIST_MESSAGE)
                })?;
                listed
                    .iter()
                    .map(|value| {
                        value.as_str().and_then(WorkKind::parse).ok_or_else(|| {
                            invalid(&layer.path, &format!("{field}.when"), KIND_LIST_MESSAGE)
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        if when.is_empty()
            && let Some(other) = rules.iter().find(|rule| rule.when.is_empty())
        {
            let label = LoveRule {
                destinations: destinations.clone(),
                when: Vec::new(),
                scope: scope.into(),
            }
            .label();
            return Err(invalid(
                &layer.path,
                "love",
                &format!(
                    "{} and {} both take every other kind of work; only one rule may leave when \
                     out",
                    other.label(),
                    label
                ),
            ));
        }
        if let Some(kind) = when
            .iter()
            .find(|kind| rules.iter().any(|rule| rule.when.contains(kind)))
        {
            return Err(invalid(
                &layer.path,
                "love",
                &format!(
                    "two rules claim {} work; a kind of work belongs to one rule",
                    kind.as_str()
                ),
            ));
        }
        rules.push(LoveRule {
            destinations,
            when,
            scope: scope.into(),
        });
    }
    Ok(Some(rules))
}

/// The worker prompt one `.oga.yaml` writes for its own scope: plain text
/// that is sent as written, with `{{brief}}` marking where the task lands,
/// alongside `{{scope}}`, `{{memories}}`,
/// `{{attribution}}`, `{{reporting}}`, and the run itself as `{{task_id}}`,
/// `{{provider}}`, `{{model}}`, `{{effort}}`. A value without `{{brief}}`
/// keeps working: resolution gives it the slot first through
/// [`ensure_brief_slot`], leaving its words and order untouched.
/// `attribution` is the only other key: `false` turns off the supervision
/// line workers stamp on commits and pull requests, `true` (or leaving it
/// out) leaves it on. Any other key is a rule the writer expects Oga to
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
        (key != "prompt" && key != "attribution").then_some(key)
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

const WORKER_SHAPE: &str =
    "worker takes prompt, holding the rules text, and an optional attribution flag";

/// The single structural guarantee: the task slot. A template holding
/// `{{brief}}` is sent as written, nothing added. Anything older — plain
/// rules from before templates existed — gets the slot first, where the task
/// always landed, so existing prompts keep working with their words and
/// order untouched.
pub fn ensure_brief_slot(raw: &str) -> String {
    if raw.contains("{{brief}}") {
        raw.to_owned()
    } else {
        format!("{{{{brief}}}}\n\n{raw}")
    }
}

/// Whether this scope stamps worker output with the Done-with-Oga line.
/// `None` means the file says nothing and the next scope up decides.
pub fn read_worker_attribution(layer: Option<&ConfigLayer>) -> Result<Option<bool>, ConfigError> {
    let Some(layer) = layer else {
        return Ok(None);
    };
    let Some(worker) = layer.root.get("worker") else {
        return Ok(None);
    };
    let table = worker
        .as_mapping()
        .ok_or_else(|| invalid(&layer.path, "worker", WORKER_SHAPE))?;
    table
        .get("attribution")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| invalid(&layer.path, "worker.attribution", "must be true or false"))
        })
        .transpose()
}

/// Attribution is on unless somebody turns it off. The project file wins over
/// the all-projects file; either `false` silences the stamp.
pub fn resolve_worker_attribution(project: Option<bool>, user: Option<bool>) -> bool {
    project.or(user).unwrap_or(true)
}

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
                destinations: vec![LoveDestination {
                    profile_id: Some("opencode".into()),
                    model: Some("openai/gpt-5.6-luna".into()),
                    effort: None,
                }],
                when: Vec::new(),
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
        assert_eq!(
            context.primary().model.as_deref(),
            Some("openai/gpt-5.6-luna")
        );
        assert_eq!(context.primary().effort.as_deref(), Some("low"));
        assert_eq!(
            love.for_class(TaskClass::Reasoning)
                .unwrap()
                .primary()
                .effort
                .as_deref(),
            Some("max")
        );
        assert_eq!(
            love.for_class(TaskClass::General)
                .unwrap()
                .primary()
                .model
                .as_deref(),
            Some("opus")
        );
        assert!(love.names_model("claude", "opus", None));
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

        assert_eq!(
            love.for_class(TaskClass::Context)
                .unwrap()
                .primary()
                .model
                .as_deref(),
            Some("kimi")
        );
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
                "invalid config /work/.oga.yaml at love[0].kind: unknown field; each rule takes model or a models list of destinations, an optional when list of kinds, and an optional effort",
            ),
            (
                "love:\n  - model: a\n    when: [refactoring]\n",
                "invalid config /work/.oga.yaml at love[0].when: must be a list of kinds of work: mechanical, context, build, reasoning, general, ui, backend, database, docs, tests, review, research, refactor",
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
    fn love_rules_read_subjects_and_aliases() {
        let project = layer(
            "/work/.oga.yaml",
            r#"
            love:
              - model: opencode:muse
                when: [frontend]
              - model: codex:beast
                when: [backend]
              - model: claude:opus
        "#,
        );
        let (_, love) = read_model_overrides(&ConfigLayers {
            user: None,
            project: Some(project),
        })
        .unwrap();

        // `frontend` is how people write it; the rule still answers to `ui`.
        let ui = love
            .for_task(TaskClass::Build, Some(TaskTopic::Ui))
            .unwrap();
        assert_eq!(ui.primary().model.as_deref(), Some("muse"));
        assert_eq!(
            ui.match_for(TaskClass::Build, Some(TaskTopic::Ui)),
            LoveMatch::Claimed(WorkKind::Ui)
        );
        let backend = love
            .for_task(TaskClass::Reasoning, Some(TaskTopic::Backend))
            .unwrap();
        assert_eq!(backend.primary().model.as_deref(), Some("beast"));
        // No subject: the class-only task falls to the catch-all.
        assert_eq!(
            love.for_task(TaskClass::Build, None)
                .unwrap()
                .primary()
                .model
                .as_deref(),
            Some("opus")
        );
    }

    #[test]
    fn love_rules_prefer_a_subject_over_a_class_over_the_catch_all() {
        let project = layer(
            "/work/.oga.yaml",
            r#"
            love:
              - model: a
                when: [build]
              - model: b
                when: [refactor]
              - model: c
        "#,
        );
        let (_, love) = read_model_overrides(&ConfigLayers {
            user: None,
            project: Some(project),
        })
        .unwrap();

        // One task, two claims: the refactor rule wins over the build one.
        assert_eq!(
            love.for_task(TaskClass::Build, Some(TaskTopic::Refactor))
                .unwrap()
                .primary()
                .model
                .as_deref(),
            Some("b")
        );
        // Class alone still lands on the class rule, not the catch-all.
        assert_eq!(
            love.for_task(TaskClass::Build, None)
                .unwrap()
                .primary()
                .model
                .as_deref(),
            Some("a")
        );
        // A subject no rule names falls back to the class, then the catch-all.
        assert_eq!(
            love.for_task(TaskClass::Context, Some(TaskTopic::Ui))
                .unwrap()
                .primary()
                .model
                .as_deref(),
            Some("c")
        );
    }

    #[test]
    fn a_general_rule_takes_the_work_no_other_rule_claims() {
        let project = layer(
            "/work/.oga.yaml",
            r#"
            love:
              - model: claude:claude-opus-5
                when: [general]
                effort: low
        "#,
        );
        let (_, love) = read_model_overrides(&ConfigLayers {
            user: None,
            project: Some(project),
        })
        .unwrap();

        // Docs work under a build class: no rule names either, so the general
        // rule takes it.
        let rule = love
            .for_task(TaskClass::Build, Some(TaskTopic::Docs))
            .unwrap();
        assert_eq!(rule.primary().model.as_deref(), Some("claude-opus-5"));
        assert_eq!(
            rule.match_for(TaskClass::Build, Some(TaskTopic::Docs)),
            LoveMatch::Fallback
        );
        // Work that genuinely reads as general still matches the word itself.
        assert_eq!(
            love.for_class(TaskClass::General)
                .unwrap()
                .match_for(TaskClass::General, None),
            LoveMatch::Claimed(WorkKind::General)
        );
    }

    #[test]
    fn a_claimed_kind_wins_over_the_general_rule_and_general_over_the_catch_all() {
        let project = layer(
            "/work/.oga.yaml",
            r#"
            love:
              - model: a
                when: [ui]
              - model: b
                when: [build]
              - model: c
                when: [general]
              - model: d
        "#,
        );
        let (_, love) = read_model_overrides(&ConfigLayers {
            user: None,
            project: Some(project),
        })
        .unwrap();
        let model = |class, topic| {
            love.for_task(class, topic)
                .unwrap()
                .primary()
                .model
                .clone()
                .unwrap()
        };

        // A rule naming the subject beats the general rule.
        assert_eq!(model(TaskClass::Reasoning, Some(TaskTopic::Ui)), "a");
        // So does a rule naming the class.
        assert_eq!(model(TaskClass::Build, Some(TaskTopic::Database)), "b");
        // Nothing claims docs work under a context class: the general rule
        // takes it, ahead of the bare catch-all.
        assert_eq!(model(TaskClass::Context, Some(TaskTopic::Docs)), "c");
        assert_eq!(model(TaskClass::General, None), "c");
    }

    #[test]
    fn the_catch_all_still_takes_everything_when_no_rule_names_general() {
        let project = layer(
            "/work/.oga.yaml",
            "love:\n  - model: a\n    when: [ui]\n  - model: b\n",
        );
        let (_, love) = read_model_overrides(&ConfigLayers {
            user: None,
            project: Some(project),
        })
        .unwrap();

        let rule = love
            .for_task(TaskClass::Context, Some(TaskTopic::Docs))
            .unwrap();
        assert_eq!(rule.primary().model.as_deref(), Some("b"));
        assert_eq!(
            rule.match_for(TaskClass::Context, Some(TaskTopic::Docs)),
            LoveMatch::CatchAll
        );
    }

    #[test]
    fn love_rules_reject_a_subject_claimed_twice() {
        let twice = layer(
            "/work/.oga.yaml",
            "love:\n  - model: a\n    when: [ui]\n  - model: b\n    when: [frontend]\n",
        );
        assert_eq!(
            read_model_overrides(&ConfigLayers {
                user: None,
                project: Some(twice),
            })
            .unwrap_err()
            .to_string(),
            "invalid config /work/.oga.yaml at love: two rules claim ui work; a kind of work belongs to one rule"
        );
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
    fn love_models_lists_read_an_ordered_chain_with_omitted_parts() {
        let project = layer(
            "/work/.oga.yaml",
            "love:\n  - models: [opencode:luna:max, claude:opus:low, claude]\n    when: [ui]\n",
        );
        let (_, love) = read_model_overrides(&ConfigLayers {
            user: None,
            project: Some(project),
        })
        .unwrap();

        let rule = love
            .for_task(TaskClass::Build, Some(TaskTopic::Ui))
            .unwrap();
        assert_eq!(
            rule.triples(),
            vec!["opencode:luna:max", "claude:opus:low", "claude"]
        );
        assert_eq!(rule.chain_label(), "opencode/luna → claude/opus → claude");
        // A worker alone stands for its default model.
        assert!(rule.names_model("claude", "sonnet", Some("sonnet")));
        assert!(!rule.names_model("claude", "haiku", Some("sonnet")));
        assert!(rule.names_model("opencode", "luna", None));
    }

    #[test]
    fn love_destinations_parse_worker_model_and_effort_pieces() {
        let parsed = LoveDestination::parse("opencode:luna:max").unwrap();
        assert_eq!(parsed.profile_id.as_deref(), Some("opencode"));
        assert_eq!(parsed.model.as_deref(), Some("luna"));
        assert_eq!(parsed.effort.as_deref(), Some("max"));
        assert_eq!(parsed.triple(), "opencode:luna:max");

        let worker = LoveDestination::parse("claude").unwrap();
        assert_eq!(worker.profile_id.as_deref(), Some("claude"));
        assert_eq!(worker.model, None);
        assert_eq!(worker.triple(), "claude");

        let effort_only = LoveDestination::parse("claude:high").unwrap();
        assert_eq!(effort_only.model, None);
        assert_eq!(effort_only.effort.as_deref(), Some("high"));
        assert_eq!(effort_only.triple(), "claude:high");

        assert_eq!(
            LoveDestination::parse("claude:opus:enormous").unwrap_err(),
            "'enormous' is not an effort in 'claude:opus:enormous'; choose one of minimal, low, medium, high, xhigh, max"
        );
        assert!(LoveDestination::parse(":opus").is_err());
        assert!(LoveDestination::parse("a:b:c:d").is_err());
    }

    #[test]
    fn love_models_lists_reject_bad_destinations_where_they_are_written() {
        for (source, message) in [
            (
                "love:\n  - models: [claude:opus, claude:opus]\n",
                "invalid config /work/.oga.yaml at love[0]: 'claude:opus' names the same destination twice",
            ),
            (
                "love:\n  - models: []\n",
                "invalid config /work/.oga.yaml at love[0].models: must name at least one destination, like 'claude:opus'",
            ),
            (
                "love:\n  - model: claude:opus\n    models: [opencode:luna]\n",
                "invalid config /work/.oga.yaml at love[0]: write model or models, not both; a list of destinations goes under models",
            ),
            (
                "love:\n  - models: [claude:opus]\n    effort: low\n",
                "invalid config /work/.oga.yaml at love[0].effort: write the effort with each destination, like 'claude:opus:low'",
            ),
            (
                "love:\n  - models: [claude:opus:enormous]\n",
                "invalid config /work/.oga.yaml at love[0].models[0]: 'enormous' is not an effort in 'claude:opus:enormous'; choose one of minimal, low, medium, high, xhigh, max",
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
    fn love_rules_serialize_the_first_destination_flat_and_the_chain_whole() {
        let project = layer(
            "/work/.oga.yaml",
            "love:\n  - models: [opencode:luna:max, claude:opus:low]\n    when: [ui]\n",
        );
        let (_, love) = read_model_overrides(&ConfigLayers {
            user: None,
            project: Some(project),
        })
        .unwrap();

        let rule = &love.0[0];
        let json = serde_json::to_value(rule).expect("love rule serializes");
        assert_eq!(json["model"], "luna");
        assert_eq!(json["profileId"], "opencode");
        assert_eq!(json["effort"], "max");
        assert_eq!(json["models"].as_array().expect("models").len(), 2);
        // A single destination keeps the legacy shape, with the chain alongside.
        let single = layer("/work/.oga.yaml", "love:\n  - model: claude:opus\n");
        let (_, love) = read_model_overrides(&ConfigLayers {
            user: None,
            project: Some(single),
        })
        .unwrap();
        let json = serde_json::to_value(&love.0[0]).expect("love rule serializes");
        assert_eq!(json["model"], "opus");
        assert_eq!(json["when"].as_array().expect("when").len(), 0);
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
            "invalid config /work/.oga.yaml at worker.tldr_sentences: unknown key; worker takes prompt, holding the rules text, and an optional attribution flag"
        );
    }

    #[test]
    fn worker_attribution_defaults_on_and_resolves_project_first() {
        assert!(read_worker_attribution(None).unwrap().is_none());
        let on = layer("/work/.oga.yaml", "worker:\n  attribution: true\n");
        let off = layer("/work/.oga.yaml", "worker:\n  attribution: false\n");
        let prompt_only = layer("/work/.oga.yaml", "worker:\n  prompt: rules\n");
        assert_eq!(read_worker_attribution(Some(&on)).unwrap(), Some(true));
        assert_eq!(read_worker_attribution(Some(&off)).unwrap(), Some(false));
        assert_eq!(read_worker_attribution(Some(&prompt_only)).unwrap(), None);
        let bad = layer("/work/.oga.yaml", "worker:\n  attribution: sometimes\n");
        assert!(read_worker_attribution(Some(&bad)).is_err());
        assert!(resolve_worker_attribution(None, None));
        assert!(!resolve_worker_attribution(Some(false), Some(true)));
        assert!(!resolve_worker_attribution(None, Some(false)));
        assert!(resolve_worker_attribution(Some(true), Some(false)));
    }

    #[test]
    fn brief_slot_is_added_first_only_when_missing() {
        assert_eq!(
            ensure_brief_slot("{{brief}}\n\nBe terse."),
            "{{brief}}\n\nBe terse."
        );
        assert_eq!(ensure_brief_slot("Be terse."), "{{brief}}\n\nBe terse.");
        assert_eq!(ensure_brief_slot(""), "{{brief}}\n\n");
    }

    #[test]
    fn editable_default_is_a_template_carrying_the_house_style() {
        assert!(DEFAULT_WORKER_PROMPT.contains("{{brief}}"));
        assert!(DEFAULT_WORKER_PROMPT.contains("{{scope}}"));
        assert!(DEFAULT_WORKER_PROMPT.contains("{{memories}}"));
        assert!(DEFAULT_WORKER_PROMPT.contains("{{attribution}}"));
        assert!(DEFAULT_WORKER_PROMPT.contains("{{reporting}}"));
        assert!(DEFAULT_WORKER_PROMPT.contains("Clear local, reversible obstacles yourself"));
        assert!(DEFAULT_WORKER_PROMPT.contains("oga query"));
        assert!(
            DEFAULT_WORKER_PROMPT
                .contains("Add `--code` when you want the code back instead of just the location")
        );
        assert!(DEFAULT_WORKER_PROMPT.contains("gh pr create"));
        assert!(DEFAULT_WORKER_PROMPT.contains("oga relearn"));
        assert!(DEFAULT_WORKER_PROMPT.contains("Do not use Oga to delegate")); // default text, deletable
    }

    #[test]
    fn worker_prompt_rejects_a_non_text_block() {
        let project = layer("/work/.oga.yaml", "worker:\n  prompt:\n    - one\n");

        assert!(read_worker_prompt(Some(&project)).is_err());
    }
}
