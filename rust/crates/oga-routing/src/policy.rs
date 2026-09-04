//! The project routing policy: `routes` in `.oga.yaml`, validated, merged
//! across layers, and matched against catalogs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use oga_config::{ConfigError, ConfigLayers, load_config_layers};
use oga_domain::{ModelInfo, ModelInfoSource, RoutePreference, TaskClass};
use thiserror::Error;

const TASK_CLASSES: [TaskClass; 5] = [
    TaskClass::Mechanical,
    TaskClass::Context,
    TaskClass::Build,
    TaskClass::Reasoning,
    TaskClass::General,
];
const PREFERENCES: [RoutePreference; 4] = [
    RoutePreference::Balanced,
    RoutePreference::Quality,
    RoutePreference::Cost,
    RoutePreference::Speed,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedModel {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PolicyRoute {
    pub preference: Option<RoutePreference>,
    pub min_quality: Option<u8>,
    pub allow: Vec<AllowedModel>,
}

/// A version 1 policy. `path` is the highest file that contributed a route and
/// `sources` lists every contributor, highest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingPolicy {
    pub path: String,
    pub sources: Vec<String>,
    pub routes: BTreeMap<TaskClass, PolicyRoute>,
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("invalid routing policy {path} at {field}: {message}")]
    Invalid {
        path: String,
        field: String,
        message: String,
    },
    #[error(transparent)]
    Config(#[from] ConfigError),
}

impl PolicyError {
    fn invalid(path: &str, field: &str, message: &str) -> Self {
        PolicyError::Invalid {
            path: path.to_owned(),
            field: field.to_owned(),
            message: message.to_owned(),
        }
    }

    /// The file and exact invalid field, for callers that name them.
    pub fn parts(&self) -> Option<(&str, &str)> {
        match self {
            PolicyError::Invalid { path, field, .. } => Some((path, field)),
            PolicyError::Config(_) => None,
        }
    }
}

fn normalize_task_class_value(value: &str) -> Option<TaskClass> {
    let normalized = value.trim().to_lowercase();
    TASK_CLASSES.into_iter().find(|c| c.as_str() == normalized)
}

/// Normalizes only supported task classes; anything else is not a class.
pub fn normalize_task_class(value: &str) -> Option<TaskClass> {
    normalize_task_class_value(value)
}

/// The effective policy for a cwd: the project file's routes merged over the
/// user file's, which in turn stands where the project says nothing.
pub fn load_routing_policy(cwd: &Path) -> Result<Option<RoutingPolicy>, PolicyError> {
    let layers = load_config_layers(Some(cwd)).map_err(policy_layer_error)?;
    routing_policy_from_layers(&layers)
}

/// A broken YAML layer fails the load as a policy error naming the file, so
/// callers see one policy-shaped failure whatever broke first.
fn policy_layer_error(error: ConfigError) -> PolicyError {
    match error {
        ConfigError::Invalid {
            path,
            field,
            message,
        } => PolicyError::Invalid {
            path: path.display().to_string(),
            field,
            message,
        },
        other => PolicyError::Config(other),
    }
}

/// Validate both layers and merge them.
pub fn routing_policy_from_layers(
    layers: &ConfigLayers,
) -> Result<Option<RoutingPolicy>, PolicyError> {
    let project = match layers.project.as_ref() {
        Some(layer) => validate_policy(&layer.root, &layer.path.display().to_string())?,
        None => None,
    };
    let user = match layers.user.as_ref() {
        Some(layer) => validate_policy(&layer.root, &layer.path.display().to_string())?,
        None => None,
    };
    Ok(merge_policies(project, user))
}

/// Per class, scalar fields override and `allow` replaces whole — an allow
/// list is written best-first, so merging two lists would scramble its
/// meaning.
pub fn merge_policies(
    project: Option<RoutingPolicy>,
    user: Option<RoutingPolicy>,
) -> Option<RoutingPolicy> {
    if project.is_none() && user.is_none() {
        return None;
    }
    let mut classes: BTreeSet<TaskClass> = BTreeSet::new();
    for policy in [&project, &user].into_iter().flatten() {
        classes.extend(policy.routes.keys().copied());
    }
    let mut routes = BTreeMap::new();
    for task_class in classes {
        let p = project
            .as_ref()
            .and_then(|policy| policy.routes.get(&task_class));
        let u = user
            .as_ref()
            .and_then(|policy| policy.routes.get(&task_class));
        // Neither can both be missing: the class came from one of them.
        let Some(allow) = p.or(u).map(|route| route.allow.clone()) else {
            continue;
        };
        routes.insert(
            task_class,
            PolicyRoute {
                preference: p
                    .and_then(|r| r.preference)
                    .or(u.and_then(|r| r.preference)),
                min_quality: p
                    .and_then(|r| r.min_quality)
                    .or(u.and_then(|r| r.min_quality)),
                allow,
            },
        );
    }
    let sources: Vec<String> = [project.as_ref(), user.as_ref()]
        .into_iter()
        .flatten()
        .map(|policy| policy.path.clone())
        .collect();
    Some(RoutingPolicy {
        path: sources[0].clone(),
        sources,
        routes,
    })
}

impl RoutingPolicy {
    pub fn route_for_task(&self, task_class: TaskClass) -> Option<&PolicyRoute> {
        self.routes.get(&task_class)
    }
}

impl PolicyRoute {
    pub fn model_allowed(&self, provider_id: &str, model_id: &str) -> bool {
        self.allow_rank(provider_id, model_id).is_some()
    }

    /// Whether one allow rule covers this model. Needed per rule, not per
    /// route: a rule matching nothing any account offers is a config mistake
    /// worth naming, and ranking reports only the first rule that matched.
    pub fn rule_matches(rule: &AllowedModel, provider_id: &str, model_id: &str) -> bool {
        rule.provider == normalize_id(provider_id)
            && glob_matches(&rule.model, &normalize_id(model_id))
    }

    /// Position of the first allow rule matching this model. Entries are
    /// written best-first, so this is the preference order to use when a
    /// caller has already named the profile and only the model is left to
    /// choose.
    pub fn allow_rank(&self, provider_id: &str, model_id: &str) -> Option<usize> {
        self.allow
            .iter()
            .position(|rule| Self::rule_matches(rule, provider_id, model_id))
    }

    /// Allow entries no connected account can satisfy. A rule matching nothing
    /// silently shrinks the choice for a whole class of work, so it is named
    /// rather than left for a dispatch to trip over.
    ///
    /// A provider whose discovery failed falls back to one configured entry,
    /// and that list is not evidence about anything else the account offers —
    /// so a rule naming such a provider is left alone. A rule naming a
    /// provider with no profile at all is reported, since nothing there could
    /// ever match.
    pub fn unoffered_rules<'m>(
        &self,
        offered: impl IntoIterator<Item = &'m ModelInfo>,
    ) -> Vec<AllowedModel> {
        let offered: Vec<&ModelInfo> = offered.into_iter().collect();
        let connected: BTreeSet<&str> = offered.iter().map(|m| m.provider.as_str()).collect();
        let enumerated: BTreeSet<&str> = offered
            .iter()
            .filter(|model| model.source != ModelInfoSource::Configured)
            .map(|model| model.provider.as_str())
            .collect();
        self.allow
            .iter()
            .filter(|rule| {
                !offered
                    .iter()
                    .any(|model| Self::rule_matches(rule, model.provider.as_str(), &model.id))
                    && (!connected.contains(rule.provider.as_str())
                        || enumerated.contains(rule.provider.as_str()))
            })
            .cloned()
            .collect()
    }
}

/// The same check across every class a policy defines, for reporting at load.
pub fn unoffered_policy_rules(
    policy: &RoutingPolicy,
    offered: &[ModelInfo],
) -> Vec<(TaskClass, AllowedModel)> {
    policy
        .routes
        .iter()
        .flat_map(|(task_class, route)| {
            route
                .unoffered_rules(offered.iter())
                .into_iter()
                .map(move |rule| (*task_class, rule))
        })
        .collect()
}

/// One wording for the finding, wherever it is reported from.
pub fn unoffered_rule_message(task_class: TaskClass, rule: &AllowedModel) -> String {
    format!(
        "this project allows {} model {} for {} work, but no connected account offers it; \
         remove the entry or connect that account",
        rule.provider,
        rule.model,
        task_class.as_str()
    )
}

fn validate_policy(
    root: &serde_yaml::Mapping,
    path: &str,
) -> Result<Option<RoutingPolicy>, PolicyError> {
    const ROOT_FIELDS: [&str; 5] = ["version", "routes", "worker", "profiles", "models"];
    if let Some(unknown) = root.keys().find_map(|key| {
        let key = key.as_str()?;
        (!ROOT_FIELDS.contains(&key)).then_some(key)
    }) {
        return Err(PolicyError::invalid(path, unknown, "unknown field"));
    }
    // `[worker]` alone is a complete file: prompt rules are read by the config
    // crate, and a project with no `[routes]` has no policy to version or
    // validate.
    let Some(raw_routes) = root.get("routes") else {
        return Ok(None);
    };
    if root.get("version").and_then(serde_yaml::Value::as_i64) != Some(1) {
        return Err(PolicyError::invalid(path, "version", "must be 1"));
    }
    let raw_routes = raw_routes.as_mapping().ok_or_else(|| {
        PolicyError::invalid(path, "routes", "must define at least one task class")
    })?;
    let mut routes = BTreeMap::new();
    for (raw_key, value) in raw_routes {
        let Some(key) = raw_key.as_str() else {
            continue;
        };
        let Some(task_class) = normalize_task_class(key) else {
            return Err(PolicyError::invalid(
                path,
                &format!("routes.{key}"),
                &format!(
                    "unsupported task class; expected {}",
                    TASK_CLASSES
                        .iter()
                        .map(|c| c.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        };
        if routes.contains_key(&task_class) {
            return Err(PolicyError::invalid(
                path,
                &format!("routes.{key}"),
                &format!("duplicates normalized task class {}", task_class.as_str()),
            ));
        }
        routes.insert(
            task_class,
            validate_route(value, path, &format!("routes.{key}"))?,
        );
    }
    if routes.is_empty() {
        return Err(PolicyError::invalid(
            path,
            "routes",
            "must define at least one task class",
        ));
    }
    Ok(Some(RoutingPolicy {
        path: path.to_owned(),
        sources: vec![path.to_owned()],
        routes,
    }))
}

fn validate_route(
    value: &serde_yaml::Value,
    path: &str,
    field: &str,
) -> Result<PolicyRoute, PolicyError> {
    const ROUTE_FIELDS: [&str; 3] = ["preference", "min_quality", "allow"];
    let route = expect_table(value, path, field)?;
    if let Some(unknown) = route.keys().find_map(|key| {
        let key = key.as_str()?;
        (!ROUTE_FIELDS.contains(&key)).then_some(key)
    }) {
        return Err(PolicyError::invalid(
            path,
            &format!("{field}.{unknown}"),
            "unknown field",
        ));
    }
    let preference = match route.get("preference") {
        Some(value) => {
            let text = value.as_str().ok_or_else(|| {
                PolicyError::invalid(path, &format!("{field}.preference"), &preference_message())
            })?;
            let found = PREFERENCES.into_iter().find(|p| p.as_str() == text);
            Some(found.ok_or_else(|| {
                PolicyError::invalid(path, &format!("{field}.preference"), &preference_message())
            })?)
        }
        None => None,
    };
    let min_quality = match route.get("min_quality") {
        Some(value) => {
            let quality = value.as_i64();
            if !matches!(quality, Some(q) if (1..=5).contains(&q)) {
                return Err(PolicyError::invalid(
                    path,
                    &format!("{field}.min_quality"),
                    "must be an integer from 1 to 5",
                ));
            }
            Some(quality.unwrap() as u8)
        }
        None => None,
    };
    let raw_allow = route.get("allow").and_then(serde_yaml::Value::as_sequence);
    let Some(raw_allow) = raw_allow.filter(|allow| !allow.is_empty()) else {
        return Err(PolicyError::invalid(
            path,
            &format!("{field}.allow"),
            "must be a non-empty array",
        ));
    };
    let allow = raw_allow
        .iter()
        .enumerate()
        .map(|(index, rule)| validate_allowed_model(rule, path, &format!("{field}.allow[{index}]")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PolicyRoute {
        preference,
        min_quality,
        allow,
    })
}

fn preference_message() -> String {
    format!(
        "must be one of {}",
        PREFERENCES
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn validate_allowed_model(
    value: &serde_yaml::Value,
    path: &str,
    field: &str,
) -> Result<AllowedModel, PolicyError> {
    const RULE_FIELDS: [&str; 2] = ["provider", "model"];
    let rule = expect_table(value, path, field)?;
    if let Some(unknown) = rule.keys().find_map(|key| {
        let key = key.as_str()?;
        (!RULE_FIELDS.contains(&key)).then_some(key)
    }) {
        return Err(PolicyError::invalid(
            path,
            &format!("{field}.{unknown}"),
            "unknown field",
        ));
    }
    let provider = rule
        .get("provider")
        .and_then(serde_yaml::Value::as_str)
        .filter(|text| PROVIDER_ID.is_match(text))
        .ok_or_else(|| {
            PolicyError::invalid(
                path,
                &format!("{field}.provider"),
                "must be a provider ID without globs",
            )
        })?;
    let model = rule
        .get("model")
        .and_then(serde_yaml::Value::as_str)
        .filter(|text| MODEL_GLOB.is_match(text))
        .ok_or_else(|| {
            PolicyError::invalid(
                path,
                &format!("{field}.model"),
                "must be a model ID glob using only safe ID characters and *",
            )
        })?;
    Ok(AllowedModel {
        provider: normalize_id(provider),
        model: normalize_id(model),
    })
}

use regex::Regex;
use std::sync::LazyLock;

static PROVIDER_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)^[a-z0-9._-]+$").expect("pattern"));
static MODEL_GLOB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)^[a-z0-9._:/@*-]+$").expect("pattern"));

fn expect_table<'a>(
    value: &'a serde_yaml::Value,
    path: &str,
    field: &str,
) -> Result<&'a serde_yaml::Mapping, PolicyError> {
    value
        .as_mapping()
        .ok_or_else(|| PolicyError::invalid(path, field, "must be a table"))
}

fn normalize_id(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Anchored glob over safe ID characters: every literal segment must appear in
/// order between any number of wildcard runs.
fn glob_matches(pattern: &str, value: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return value == pattern;
    }
    let head = parts[0];
    let tail = parts[parts.len() - 1];
    let Some(after_head) = value.strip_prefix(head) else {
        return false;
    };
    let Some(mut middle) = after_head.strip_suffix(tail) else {
        return false;
    };
    for part in &parts[1..parts.len() - 1] {
        match middle.find(part) {
            Some(index) => middle = &middle[index + part.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ModelInfoFields, model};
    use oga_domain::Provider;

    fn parse(source: &str) -> serde_yaml::Mapping {
        serde_yaml::from_str(source).expect("valid yaml")
    }

    fn validated(source: &str, path: &str) -> Result<Option<RoutingPolicy>, PolicyError> {
        validate_policy(&parse(source), path)
    }

    #[test]
    fn loads_and_normalizes_a_version_one_policy() {
        let policy = validated(
            "version: 1\nroutes:\n  BUILD:\n    preference: quality\n    min_quality: 5\n    allow:\n      - provider: Claude\n        model: OPUS\n      - provider: OpenCode\n        model: opencode-go/*\n",
            "/project/.oga.yaml",
        )
        .unwrap()
        .unwrap();
        assert_eq!(policy.path, "/project/.oga.yaml");
        let route = policy.route_for_task(TaskClass::Build).unwrap();
        assert_eq!(route.preference, Some(RoutePreference::Quality));
        assert_eq!(route.min_quality, Some(5));
        assert_eq!(
            route.allow,
            vec![
                AllowedModel {
                    provider: "claude".into(),
                    model: "opus".into()
                },
                AllowedModel {
                    provider: "opencode".into(),
                    model: "opencode-go/*".into()
                },
            ]
        );
    }

    #[test]
    fn a_worker_only_file_has_no_routing_policy() {
        assert_eq!(
            validated("worker:\n  checklist: true\n", "/p").unwrap(),
            None
        );
    }

    #[test]
    fn reports_the_file_and_exact_invalid_field() {
        let error = validated(
            "version: 1\nroutes:\n  build:\n    allow:\n      - provider: claude\n        model: opus\n        profile: personal\n",
            "/project/.oga.yaml",
        )
        .unwrap_err();
        assert!(error.to_string().contains("/project/.oga.yaml"));
        let (path, field) = error.parts().unwrap();
        assert_eq!(path, "/project/.oga.yaml");
        assert_eq!(field, "routes.build.allow[0].profile");
    }

    #[test]
    fn rejects_invalid_fields_with_their_paths() {
        for (source, field) in [
            (
                "version: 2\nroutes:\n  build:\n    allow: [{ provider: claude, model: opus }]",
                "version",
            ),
            (
                "version: 1\nroutes:\n  deploy:\n    allow: [{ provider: claude, model: opus }]",
                "routes.deploy",
            ),
            (
                "version: 1\nroutes:\n  build:\n    allow: [{ provider: cl*, model: opus }]",
                "routes.build.allow[0].provider",
            ),
            (
                "version: 1\nroutes:\n  build:\n    allow: [{ provider: claude, model: opus[0-9] }]",
                "routes.build.allow[0].model",
            ),
            (
                "version: 1\nextra: true\nroutes:\n  build:\n    allow: [{ provider: claude, model: o }]",
                "extra",
            ),
            (
                "version: 1\nroutes:\n  build:\n    allow: []",
                "routes.build.allow",
            ),
            (
                "version: 1\nroutes:\n  build:\n    min_quality: 9\n    allow: [{ provider: claude, model: o }]",
                "routes.build.min_quality",
            ),
            (
                "version: 1\nroutes:\n  build:\n    preference: cheapest\n    allow: [{ provider: claude, model: o }]",
                "routes.build.preference",
            ),
        ] {
            let error = validated(source, "/p").unwrap_err();
            assert_eq!(error.parts().map(|(_, f)| f), Some(field), "{source}");
        }
    }

    #[test]
    fn duplicate_normalized_classes_are_refused() {
        let error = validated(
            "version: 1\nroutes:\n  BUILD: { allow: [{ provider: c, model: o }] }\n  build: { allow: [{ provider: c, model: p }] }",
            "/p",
        )
        .unwrap_err();
        assert_eq!(error.parts().unwrap().1, "routes.build");
    }

    #[test]
    fn empty_routes_are_refused() {
        let error = validated("version: 1\nroutes: {}\n", "/p").unwrap_err();
        assert_eq!(error.parts().unwrap().1, "routes");
    }

    #[test]
    fn matches_normalized_ids_with_anchored_globs() {
        let policy = validated(
            r#"
version = 1
[routes.reasoning]
allow = [
  { provider = "claude", model = "*" },
  { provider = "opencode", model = "opencode-go/*" },
]
"#,
            "/p",
        )
        .unwrap()
        .unwrap();
        let route = policy.route_for_task(TaskClass::Reasoning).unwrap();
        assert!(route.model_allowed("CLAUDE", "OPUS"));
        assert!(route.model_allowed("opencode", "opencode-go/kimi-k2"));
        assert!(!route.model_allowed("opencode", "prefix/opencode-go/kimi-k2"));
        assert!(!route.model_allowed("other", "opencode-go/kimi-k2"));
    }

    #[test]
    fn normalizes_only_supported_task_classes() {
        assert_eq!(normalize_task_class(" Build "), Some(TaskClass::Build));
        assert_eq!(normalize_task_class("profile-name"), None);
    }

    fn offered(provider: Provider, id: &str, configured_only: bool) -> ModelInfo {
        model(
            id,
            provider,
            provider.as_str(),
            ModelInfoFields {
                configured_only,
                ..ModelInfoFields::default()
            },
        )
    }

    #[test]
    fn names_unsatisfiable_entries_and_stays_quiet_on_fallback_catalogs() {
        let policy = validated(
            r#"
version = 1
[routes.build]
allow = [
  { provider = "pi", model = "opencode-go/deepseek-v4-flash" },
  { provider = "claude", model = "sonnet" },
]
[routes.reasoning]
allow = [{ provider = "claude", model = "opus" }]
"#,
            "/p",
        )
        .unwrap()
        .unwrap();
        let offered_catalog = [
            offered(Provider::Claude, "sonnet", true),
            offered(Provider::Claude, "opus", false),
        ];
        let findings = unoffered_policy_rules(&policy, &offered_catalog);
        assert_eq!(
            findings,
            vec![(
                TaskClass::Build,
                AllowedModel {
                    provider: "pi".into(),
                    model: "opencode-go/deepseek-v4-flash".into()
                }
            )]
        );
        // The message formats whatever rule it is handed; load-time findings
        // carry full ids, and a bare id reads back bare.
        assert_eq!(
            unoffered_rule_message(
                TaskClass::Build,
                &AllowedModel {
                    provider: "pi".into(),
                    model: "deepseek-v4-flash".into()
                }
            ),
            "this project allows pi model deepseek-v4-flash for build work, but no connected \
             account offers it; remove the entry or connect that account"
        );

        // Discovery that failed leaves one configured entry behind, which says
        // nothing about the rest of the account — so it cannot convict a rule.
        let fallback = [offered(Provider::OpenCode, "opencode/big-pickle", true)];
        let narrow = validated(
            "version = 1\n[routes.build]\nallow = [{ provider = \"opencode\", model = \"opencode/kimi-k3\" }]",
            "/p",
        )
        .unwrap()
        .unwrap();
        assert!(unoffered_policy_rules(&narrow, &fallback).is_empty());
        let discovered = [offered(Provider::OpenCode, "opencode/big-pickle", false)];
        assert_eq!(unoffered_policy_rules(&narrow, &discovered).len(), 1);
    }

    #[test]
    fn merges_project_over_user_with_whole_allow_replacement() {
        let user = validated(
            r#"
version = 1
[routes.build]
preference = "cost"
min_quality = 3
allow = [{ provider = "pi", model = "*" }]
[routes.context]
allow = [{ provider = "pi", model = "*" }]
"#,
            "/home/.oga.yaml",
        )
        .unwrap()
        .unwrap();
        let project = validated(
            r#"
version = 1
[routes.build]
preference = "quality"
allow = [{ provider = "claude", model = "*" }]
"#,
            "/work/.oga.yaml",
        )
        .unwrap()
        .unwrap();
        let merged = merge_policies(Some(project), Some(user)).unwrap();
        assert_eq!(merged.path, "/work/.oga.yaml");
        assert_eq!(
            merged.sources,
            vec!["/work/.oga.yaml".to_owned(), "/home/.oga.yaml".to_owned()]
        );
        let build = merged.route_for_task(TaskClass::Build).unwrap();
        assert_eq!(build.preference, Some(RoutePreference::Quality));
        assert_eq!(build.min_quality, Some(3));
        assert_eq!(
            build.allow,
            vec![AllowedModel {
                provider: "claude".into(),
                model: "*".into()
            }]
        );
        let context = merged.route_for_task(TaskClass::Context).unwrap();
        assert_eq!(
            context.allow,
            vec![AllowedModel {
                provider: "pi".into(),
                model: "*".into()
            }]
        );
    }

    #[test]
    fn globs_match_segment_order_not_just_containment() {
        assert!(glob_matches("*kimi*", "opencode/kimi-k3"));
        assert!(glob_matches("a*b*c", "axxbyyc"));
        assert!(!glob_matches("a*c*b", "axxbyyc"));
        assert!(glob_matches("exact", "exact"));
        assert!(!glob_matches("exact", "other"));
        assert!(glob_matches("", ""));
    }
}
