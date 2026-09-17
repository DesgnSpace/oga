//! Per-provider ACP adapter registrations.
//!
//! An adapter is what turns a profile into an ACP agent process: the argv that
//! starts it, the session settings that carry a run's model and effort, and
//! the two facts the task lifecycle cannot learn from the handshake. Account
//! directories and the rest of a profile's environment come from the same
//! place the command line gets them, so a profile reaches the same account
//! whichever transport runs it.

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    path::PathBuf,
    sync::Arc,
};

use oga_domain::{Profile, Provider};

use crate::{ProviderCommand, environment_for, skills_dir, unset_environment_for};

/// What an adapter needs to know to start one agent for one run.
#[derive(Debug, Clone, Copy)]
pub struct AcpLaunch<'a> {
    pub profile: &'a Profile,
    pub model: &'a str,
    pub effort: Option<&'a str>,
    pub cwd: &'a str,
}

type Argv = dyn Fn(&AcpLaunch<'_>) -> Vec<String> + Send + Sync;
type Settings = dyn Fn(&AcpLaunch<'_>) -> Vec<AcpSetting> + Send + Sync;
type Environment = dyn Fn(&AcpLaunch<'_>) -> BTreeMap<String, String> + Send + Sync;
type Directories = dyn Fn(&AcpLaunch<'_>) -> Vec<PathBuf> + Send + Sync;
type Incompatibility = dyn Fn(&AcpLaunch<'_>) -> Option<String> + Send + Sync;

/// A session setting the agent has to hold before a run's prompt, named the
/// way the agent names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpSetting {
    pub id: String,
    pub value: String,
    /// Whether the run cannot go ahead over ACP when the agent does not offer
    /// this value. A setting that is not required is left to the agent.
    pub required: bool,
}

/// The released agent an adapter was verified against: the name it reports
/// and the `major.minor` line whose patch releases Oga accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpRelease {
    pub agent: String,
    pub line: String,
}

/// How Oga starts one provider's ACP agent.
#[derive(Clone)]
pub struct AcpAdapter {
    /// Stable name recorded on every task this adapter runs.
    pub id: String,
    argv: Arc<Argv>,
    settings: Arc<Settings>,
    environment: Arc<Environment>,
    directories: Arc<Directories>,
    incompatibility: Arc<Incompatibility>,
    /// The only agent allowed to answer this adapter's command. Any other is
    /// turned away before a session opens.
    pub release: Option<AcpRelease>,
    /// The provider's command line gives the worker Oga's own tools, so the
    /// agent must accept Oga's HTTP MCP server. An agent that cannot is
    /// incompatible rather than quietly left without them.
    pub oga_tools: bool,
    /// The agent, by the name it reports, whose ACP session id is also the id
    /// the provider's command line resumes, so a terminal can pick the
    /// conversation up. Any other agent answering the same command gets no
    /// such id recorded.
    pub native_sessions_from: Option<String>,
}

impl fmt::Debug for AcpAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcpAdapter")
            .field("id", &self.id)
            .field("oga_tools", &self.oga_tools)
            .field("native_sessions_from", &self.native_sessions_from)
            .field("release", &self.release)
            .finish_non_exhaustive()
    }
}

impl AcpAdapter {
    pub fn new(
        id: impl Into<String>,
        argv: impl Fn(&AcpLaunch<'_>) -> Vec<String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            argv: Arc::new(argv),
            settings: Arc::new(|_| Vec::new()),
            environment: Arc::new(|_| BTreeMap::new()),
            directories: Arc::new(|_| Vec::new()),
            incompatibility: Arc::new(|_| None),
            release: None,
            oga_tools: false,
            native_sessions_from: None,
        }
    }

    pub fn release(mut self, agent: impl Into<String>, line: impl Into<String>) -> Self {
        self.release = Some(AcpRelease {
            agent: agent.into(),
            line: line.into(),
        });
        self
    }

    /// Why a launch cannot reach its account over ACP at all, known from the
    /// profile before any agent starts.
    pub fn incompatibility(
        mut self,
        incompatibility: impl Fn(&AcpLaunch<'_>) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.incompatibility = Arc::new(incompatibility);
        self
    }

    pub fn oga_tools(mut self, required: bool) -> Self {
        self.oga_tools = required;
        self
    }

    pub fn native_sessions_from(mut self, agent: impl Into<String>) -> Self {
        self.native_sessions_from = Some(agent.into());
        self
    }

    pub fn settings(
        mut self,
        settings: impl Fn(&AcpLaunch<'_>) -> Vec<AcpSetting> + Send + Sync + 'static,
    ) -> Self {
        self.settings = Arc::new(settings);
        self
    }

    /// Variables this run needs on top of the profile's own, for an agent that
    /// takes a launch's choices as environment rather than as flags.
    pub fn environment(
        mut self,
        environment: impl Fn(&AcpLaunch<'_>) -> BTreeMap<String, String> + Send + Sync + 'static,
    ) -> Self {
        self.environment = Arc::new(environment);
        self
    }

    pub fn directories(
        mut self,
        directories: impl Fn(&AcpLaunch<'_>) -> Vec<PathBuf> + Send + Sync + 'static,
    ) -> Self {
        self.directories = Arc::new(directories);
        self
    }

    pub fn incompatibility_for(&self, launch: &AcpLaunch<'_>) -> Option<String> {
        (self.incompatibility)(launch)
    }

    /// The session settings one run selects before its prompt, in order.
    pub fn settings_for(&self, launch: &AcpLaunch<'_>) -> Vec<AcpSetting> {
        (self.settings)(launch)
    }

    /// Workspace roots the session opens beyond the task's own directory, the
    /// ones the provider's command line reaches with its own flag.
    pub fn directories_for(&self, launch: &AcpLaunch<'_>) -> Vec<PathBuf> {
        (self.directories)(launch)
    }

    /// The agent process for one run, with the profile's account environment.
    /// The launch's own variables are written over that account, the way a
    /// command-line flag wins over the same choice made in the environment.
    pub fn command(&self, launch: &AcpLaunch<'_>) -> ProviderCommand {
        let mut env = environment_for(launch.profile);
        env.extend((self.environment)(launch));
        ProviderCommand {
            argv: (self.argv)(launch),
            env,
            env_remove: unset_environment_for(launch.profile),
        }
    }
}

/// The ACP adapters a broker can launch, one per provider.
#[derive(Debug, Clone, Default)]
pub struct AcpAdapters {
    adapters: HashMap<Provider, AcpAdapter>,
}

impl AcpAdapters {
    /// The adapters Oga ships. A provider joins this list when its adapter is
    /// wired and verified; until then it runs on its command line, and no
    /// provider claims ACP support it does not have.
    pub fn builtin() -> Self {
        Self::default()
            .register(Provider::Claude, claude())
            .register(Provider::Codex, codex())
            .register(Provider::OpenCode, opencode())
    }

    pub fn register(mut self, provider: Provider, adapter: AcpAdapter) -> Self {
        self.adapters.insert(provider, adapter);
        self
    }

    pub fn get(&self, provider: Provider) -> Option<&AcpAdapter> {
        self.adapters.get(&provider)
    }
}

/// Claude's released ACP adapter, `@agentclientprotocol/claude-agent-acp`,
/// which runs the Claude Agent SDK, verified against 0.78.0. Oga starts the
/// installed binary; an account without one falls back before any prompt.
///
/// The adapter drives the same `claude` executable the command line does, and
/// reads the user, project, and local settings that executable reads, so the
/// account, project instructions, hooks, and configured MCP servers are the
/// ones `CLAUDE_CONFIG_DIR` already names. Oga's own tools ride the session's
/// HTTP MCP servers, where `--mcp-config` carries them on the command line,
/// and the skills directory rides the session's workspace roots, where
/// `--add-dir` carries it. The command line's Oga hooks are not installed
/// here: an ACP session reports its own tool calls and subagents, so the same
/// work would arrive twice.
///
/// The model is the launch's own `ANTHROPIC_MODEL`, which both the adapter and
/// the executable behind it resolve the way `--model` resolves a name, since
/// the session's model choices are the CLI's short aliases rather than the
/// catalogue ids a task carries. The effort is a session setting, taking the
/// same level `--effort` does; a run that names none leaves the setting alone,
/// as a command line without the flag does.
///
/// A session it opens is a Claude Code session, created under the id it
/// answers with, so `claude --resume` reopens the conversation in a terminal.
/// The Claude Code behind it is the build the adapter ships rather than the
/// one on the account's path, so the two versions can differ; they write the
/// same transcripts into the same account, which is what a terminal reopens.
fn claude() -> AcpAdapter {
    AcpAdapter::new("claude-agent-acp", |_| vec!["claude-agent-acp".to_owned()])
        .oga_tools(true)
        .native_sessions_from("@agentclientprotocol/claude-agent-acp")
        .environment(|launch| {
            BTreeMap::from([("ANTHROPIC_MODEL".to_owned(), launch.model.to_owned())])
        })
        .directories(|launch| vec![PathBuf::from(skills_dir(launch.profile))])
        .settings(|launch| {
            launch
                .effort
                .map(|effort| AcpSetting {
                    id: "effort".into(),
                    value: effort.to_owned(),
                    required: true,
                })
                .into_iter()
                .collect()
        })
}

/// Codex's released ACP adapter, `@agentclientprotocol/codex-acp`, which drives
/// Codex's own app server, verified against 1.12.0. Oga starts the installed
/// binary and accepts only that release line's patch releases; any other agent
/// is turned away before a session opens, and an account without the adapter
/// falls back before any prompt.
///
/// The adapter runs the Codex build it ships (0.154.0 for 1.12.0) rather than
/// the `codex` on the account's path, and never changes that one. Both keep
/// their threads under the profile's `CODEX_HOME`, which is what the command
/// line and a terminal resume from.
///
/// The model and effort ride `CODEX_CONFIG`, which the adapter hands to Codex
/// as the same config overrides `--model` and `-c model_reasoning_effort` are,
/// so Codex resolves them the way the command line would. The session settings
/// then confirm the session holds them, and choose `agent-full-access`: no
/// approvals and no sandbox of Codex's own, which is what
/// `--dangerously-bypass-approvals-and-sandbox` asks for, leaving confinement
/// to the runner.
///
/// Oga's own tools ride the session's HTTP MCP servers, where `-c
/// mcp_servers.oga.*` carries them on the command line. The adapter drops a
/// session server named like one the account already configures, so
/// `DISABLE_MCP_CONFIG_FILTERING` merges it into that one instead, the way `-c`
/// does, rather than losing the header that binds the tools to this task.
///
/// A session it opens is a Codex thread under the id it answers with, the
/// thread id `codex exec --json` reports and `codex exec resume` takes.
///
/// `codex exec` signs in with `CODEX_API_KEY` ahead of the account's saved
/// login, and the app server never reads it, so an account that sets it keeps
/// to its command line.
fn codex() -> AcpAdapter {
    const AGENT: &str = "@agentclientprotocol/codex-acp";
    AcpAdapter::new("codex-acp", |_| vec!["codex-acp".to_owned()])
        .release(AGENT, "1.12")
        .oga_tools(true)
        .native_sessions_from(AGENT)
        .incompatibility(|launch| {
            signs_in_with_codex_api_key(launch.profile).then(|| {
                "this account signs in with CODEX_API_KEY, which only Codex's command line reads"
                    .to_owned()
            })
        })
        .environment(|launch| {
            let mut config = serde_json::json!({ "model": launch.model });
            if let Some(effort) = launch.effort {
                config["model_reasoning_effort"] = effort.into();
            }
            BTreeMap::from([
                ("CODEX_CONFIG".to_owned(), config.to_string()),
                ("DISABLE_MCP_CONFIG_FILTERING".to_owned(), "true".to_owned()),
            ])
        })
        .settings(|launch| {
            let setting = |id: &str, value: &str| AcpSetting {
                id: id.into(),
                value: value.into(),
                required: true,
            };
            let mut settings = vec![setting("model", launch.model)];
            if let Some(effort) = launch.effort {
                settings.push(setting("reasoning_effort", effort));
            }
            settings.push(setting("mode", "agent-full-access"));
            settings
        })
}

/// Whether a Codex started for this profile sees a `CODEX_API_KEY`: the
/// profile's own value, or else the broker's, which a worker inherits.
fn signs_in_with_codex_api_key(profile: &Profile) -> bool {
    environment_for(profile)
        .get("CODEX_API_KEY")
        .cloned()
        .or_else(|| std::env::var("CODEX_API_KEY").ok())
        .is_some_and(|key| !key.trim().is_empty())
}

/// OpenCode's own `opencode acp` server, verified against OpenCode 1.18.31.
///
/// The session it opens is the OpenCode session itself, so its id is the one
/// `opencode --session` continues. Its command line runs without Oga's tools,
/// and so does this. The model and the effort are session settings rather than
/// flags: `model` takes the same `provider/model` id `--model` does, and
/// `effort` takes the same variant `--variant` does. A run that names no
/// effort asks for `default`, the variant the command line uses when it is
/// given none, rather than letting the agent pick one.
fn opencode() -> AcpAdapter {
    AcpAdapter::new("opencode-acp", |launch| {
        ["opencode", "acp", "--cwd", launch.cwd]
            .map(str::to_owned)
            .to_vec()
    })
    .native_sessions_from("OpenCode")
    .settings(|launch| {
        vec![
            AcpSetting {
                id: "model".into(),
                value: launch.model.to_owned(),
                required: true,
            },
            AcpSetting {
                id: "effort".into(),
                value: launch.effort.unwrap_or("default").to_owned(),
                required: launch.effort.is_some(),
            },
        ]
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn profile(provider: Provider, env: BTreeMap<String, String>) -> Profile {
        Profile {
            id: "work".into(),
            label: "Work".into(),
            provider,
            default_model: "model".into(),
            enabled: true,
            env,
            capabilities: vec![],
            command: None,
        }
    }

    #[test]
    fn no_provider_claims_an_adapter_it_does_not_ship() {
        let adapters = AcpAdapters::builtin();
        for provider in [Provider::Claude, Provider::Codex, Provider::OpenCode] {
            assert!(adapters.get(provider).is_some(), "{provider:?}");
        }
        for provider in [Provider::OpenCode2, Provider::Antigravity, Provider::Pi] {
            assert!(adapters.get(provider).is_none(), "{provider:?}");
        }
    }

    #[test]
    fn claude_starts_its_adapter_in_the_profiles_account_on_the_tasks_model() {
        let adapters = AcpAdapters::builtin();
        let adapter = adapters.get(Provider::Claude).expect("claude adapter");
        let profile = profile(
            Provider::Claude,
            BTreeMap::from([
                ("CLAUDE_CONFIG_DIR".into(), "~/.claude-work".into()),
                ("ANTHROPIC_MODEL".into(), "an older pin".into()),
            ]),
        );
        let launch = AcpLaunch {
            profile: &profile,
            model: "claude-opus-4-5",
            effort: Some("high"),
            cwd: "/repo",
        };

        let command = adapter.command(&launch);
        let home = crate::home();

        assert_eq!(command.argv, ["claude-agent-acp"]);
        assert_eq!(
            command.env.get("CLAUDE_CONFIG_DIR"),
            Some(&format!("{home}/.claude-work"))
        );
        assert_eq!(
            command.env.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("claude-opus-4-5"),
            "the task's model wins over the account's own pin, as --model does"
        );
        assert_eq!(
            adapter.directories_for(&launch),
            [PathBuf::from(format!("{home}/.claude-work/skills"))],
            "the directory --add-dir names is opened as a workspace root"
        );
        assert!(
            adapter.oga_tools,
            "the command line gives Claude Oga's tools with --mcp-config, so this must too"
        );
        assert_eq!(
            adapter.native_sessions_from.as_deref(),
            Some("@agentclientprotocol/claude-agent-acp")
        );
    }

    #[test]
    fn claude_asks_for_an_effort_only_when_the_run_names_one() {
        let adapters = AcpAdapters::builtin();
        let adapter = adapters.get(Provider::Claude).expect("claude adapter");
        let profile = profile(Provider::Claude, BTreeMap::new());
        let launch = |effort| AcpLaunch {
            profile: &profile,
            model: "opus",
            effort,
            cwd: "/repo",
        };

        assert_eq!(
            adapter.settings_for(&launch(Some("xhigh"))),
            [AcpSetting {
                id: "effort".into(),
                value: "xhigh".into(),
                required: true,
            }]
        );
        assert_eq!(adapter.settings_for(&launch(None)), []);
    }

    #[test]
    fn codex_starts_its_adapter_in_the_profiles_account_on_the_tasks_model() {
        let adapters = AcpAdapters::builtin();
        let adapter = adapters.get(Provider::Codex).expect("codex adapter");
        let profile = profile(
            Provider::Codex,
            BTreeMap::from([
                ("CODEX_HOME".into(), "~/.codex-work".into()),
                ("CODEX_API_KEY".into(), String::new()),
            ]),
        );
        let launch = AcpLaunch {
            profile: &profile,
            model: "gpt-5.5",
            effort: Some("xhigh"),
            cwd: "/repo",
        };

        let command = adapter.command(&launch);
        let cli = crate::command_for(&profile, "", "/repo", Some("gpt-5.5"), Some("xhigh"), None);

        assert_eq!(command.argv, ["codex-acp"]);
        assert_eq!(
            command.env.get("CODEX_HOME"),
            Some(&format!("{}/.codex-work", crate::home()))
        );
        assert_eq!(command.env_remove, cli.env_remove);
        let config: serde_json::Value =
            serde_json::from_str(&command.env["CODEX_CONFIG"]).expect("config overrides");
        assert_eq!(
            config,
            serde_json::json!({"model": "gpt-5.5", "model_reasoning_effort": "xhigh"}),
            "the same overrides --model and -c model_reasoning_effort make"
        );
        assert_eq!(
            command
                .env
                .get("DISABLE_MCP_CONFIG_FILTERING")
                .map(String::as_str),
            Some("true"),
            "Oga's server merges into a same-named one, as -c does, instead of being dropped"
        );
        assert!(
            adapter.oga_tools,
            "the command line gives Codex Oga's tools with -c mcp_servers.oga, so this must too"
        );
        assert_eq!(
            adapter.native_sessions_from.as_deref(),
            Some("@agentclientprotocol/codex-acp")
        );
        assert_eq!(
            adapter.release,
            Some(AcpRelease {
                agent: "@agentclientprotocol/codex-acp".into(),
                line: "1.12".into(),
            })
        );
        assert_eq!(adapter.incompatibility_for(&launch), None);
    }

    #[test]
    fn codex_holds_the_model_effort_and_full_access_the_command_line_asks_for() {
        let adapters = AcpAdapters::builtin();
        let adapter = adapters.get(Provider::Codex).expect("codex adapter");
        let profile = profile(Provider::Codex, BTreeMap::new());
        let launch = |effort| AcpLaunch {
            profile: &profile,
            model: "gpt-5.3-codex",
            effort,
            cwd: "/repo",
        };
        let setting = |id: &str, value: &str| AcpSetting {
            id: id.into(),
            value: value.into(),
            required: true,
        };

        assert_eq!(
            adapter.settings_for(&launch(Some("high"))),
            [
                setting("model", "gpt-5.3-codex"),
                setting("reasoning_effort", "high"),
                setting("mode", "agent-full-access"),
            ]
        );
        assert_eq!(
            adapter.settings_for(&launch(None)),
            [
                setting("model", "gpt-5.3-codex"),
                setting("mode", "agent-full-access"),
            ]
        );
        let config: serde_json::Value =
            serde_json::from_str(&adapter.command(&launch(None)).env["CODEX_CONFIG"])
                .expect("config overrides");
        assert_eq!(
            config,
            serde_json::json!({"model": "gpt-5.3-codex"}),
            "a run with no effort leaves it to Codex, as the command line does"
        );
    }

    #[test]
    fn a_codex_account_signed_in_with_an_api_key_cannot_use_acp() {
        let adapters = AcpAdapters::builtin();
        let adapter = adapters.get(Provider::Codex).expect("codex adapter");
        let profile = profile(
            Provider::Codex,
            BTreeMap::from([("CODEX_API_KEY".into(), "sk-test".into())]),
        );

        let reason = adapter.incompatibility_for(&AcpLaunch {
            profile: &profile,
            model: "gpt-5.5",
            effort: None,
            cwd: "/repo",
        });

        assert!(
            reason
                .as_deref()
                .is_some_and(|reason| reason.contains("CODEX_API_KEY")),
            "{reason:?}"
        );
    }

    #[test]
    fn opencode_starts_its_own_acp_server_in_the_profiles_account() {
        let adapters = AcpAdapters::builtin();
        let adapter = adapters.get(Provider::OpenCode).expect("opencode adapter");
        let profile = profile(
            Provider::OpenCode,
            BTreeMap::from([("XDG_DATA_HOME".into(), "~/.opencode-work/data".into())]),
        );
        let launch = AcpLaunch {
            profile: &profile,
            model: "anthropic/claude-sonnet-4",
            effort: Some("high"),
            cwd: "/repo",
        };

        let command = adapter.command(&launch);

        assert_eq!(command.argv, ["opencode", "acp", "--cwd", "/repo"]);
        let cli = crate::command_for(
            &profile,
            "",
            "/repo",
            Some(launch.model),
            Some("high"),
            None,
        );
        assert_eq!(
            (&command.env, &command.env_remove),
            (&cli.env, &cli.env_remove)
        );
        assert_eq!(
            command.env.get("XDG_DATA_HOME"),
            Some(&format!("{}/.opencode-work/data", crate::home()))
        );
        assert_eq!(adapter.native_sessions_from.as_deref(), Some("OpenCode"));
        assert!(
            !adapter.oga_tools,
            "the command line gives OpenCode no Oga tools either"
        );
    }

    #[test]
    fn opencode_selects_the_model_and_effort_the_command_line_would() {
        let adapters = AcpAdapters::builtin();
        let adapter = adapters.get(Provider::OpenCode).expect("opencode adapter");
        let profile = profile(Provider::OpenCode, BTreeMap::new());
        let launch = |effort| AcpLaunch {
            profile: &profile,
            model: "opencode/big-pickle",
            effort,
            cwd: "/repo",
        };
        let setting = |id: &str, value: &str, required| AcpSetting {
            id: id.into(),
            value: value.into(),
            required,
        };

        assert_eq!(
            adapter.settings_for(&launch(Some("max"))),
            [
                setting("model", "opencode/big-pickle", true),
                setting("effort", "max", true),
            ]
        );
        assert_eq!(
            adapter.settings_for(&launch(None)),
            [
                setting("model", "opencode/big-pickle", true),
                setting("effort", "default", false),
            ]
        );
    }

    #[test]
    fn an_adapter_launches_into_the_profiles_own_account() {
        let adapter = AcpAdapter::new("claude-test", |launch| {
            vec!["agent".into(), "--model".into(), launch.model.into()]
        });
        let profile = profile(
            Provider::Claude,
            BTreeMap::from([("CLAUDE_CONFIG_DIR".into(), "/accounts/work".into())]),
        );

        let command = adapter.command(&AcpLaunch {
            profile: &profile,
            model: "opus",
            effort: None,
            cwd: "/repo",
        });

        assert_eq!(command.argv, ["agent", "--model", "opus"]);
        assert_eq!(
            command.env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some("/accounts/work")
        );
        assert_eq!(command, {
            let cli = crate::command_for(&profile, "", "/repo", Some("opus"), None, None);
            ProviderCommand {
                argv: command.argv.clone(),
                ..cli
            }
        });
    }
}
