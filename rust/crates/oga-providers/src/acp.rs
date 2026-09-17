//! Per-provider ACP adapter registrations.
//!
//! An adapter is what turns a profile into an ACP agent process: the argv that
//! starts it, the session settings that carry a run's model and effort, and
//! the two facts the task lifecycle cannot learn from the handshake. Account
//! directories and the rest of a profile's environment come from the same
//! place the command line gets them, so a profile reaches the same account
//! whichever transport runs it.

use std::{collections::HashMap, fmt, sync::Arc};

use oga_domain::{Profile, Provider};

use crate::{ProviderCommand, environment_for, unset_environment_for};

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

/// How Oga starts one provider's ACP agent.
#[derive(Clone)]
pub struct AcpAdapter {
    /// Stable name recorded on every task this adapter runs.
    pub id: String,
    argv: Arc<Argv>,
    settings: Arc<Settings>,
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
            oga_tools: false,
            native_sessions_from: None,
        }
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

    /// The session settings one run selects before its prompt, in order.
    pub fn settings_for(&self, launch: &AcpLaunch<'_>) -> Vec<AcpSetting> {
        (self.settings)(launch)
    }

    /// The agent process for one run, with the profile's account environment.
    pub fn command(&self, launch: &AcpLaunch<'_>) -> ProviderCommand {
        ProviderCommand {
            argv: (self.argv)(launch),
            env: environment_for(launch.profile),
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
        Self::default().register(Provider::OpenCode, opencode())
    }

    pub fn register(mut self, provider: Provider, adapter: AcpAdapter) -> Self {
        self.adapters.insert(provider, adapter);
        self
    }

    pub fn get(&self, provider: Provider) -> Option<&AcpAdapter> {
        self.adapters.get(&provider)
    }
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
        assert!(adapters.get(Provider::OpenCode).is_some());
        for provider in [
            Provider::Claude,
            Provider::Codex,
            Provider::OpenCode2,
            Provider::Antigravity,
            Provider::Pi,
        ] {
            assert!(adapters.get(provider).is_none(), "{provider:?}");
        }
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
