//! Per-provider ACP adapter registrations.
//!
//! An adapter is what turns a profile into an ACP agent process: the argv that
//! starts it and the two facts the task lifecycle cannot learn from the
//! handshake. Account directories and the rest of a profile's environment come
//! from the same place the command line gets them, so a profile reaches the
//! same account whichever transport runs it.

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

/// How Oga starts one provider's ACP agent.
#[derive(Clone)]
pub struct AcpAdapter {
    /// Stable name recorded on every task this adapter runs.
    pub id: String,
    argv: Arc<Argv>,
    /// The provider's command line gives the worker Oga's own tools, so the
    /// agent must accept Oga's HTTP MCP server. An agent that cannot is
    /// incompatible rather than quietly left without them.
    pub oga_tools: bool,
    /// The agent's ACP session id is also the id the provider's command line
    /// resumes, so a terminal can pick the conversation up.
    pub native_session: bool,
}

impl fmt::Debug for AcpAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcpAdapter")
            .field("id", &self.id)
            .field("oga_tools", &self.oga_tools)
            .field("native_session", &self.native_session)
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
            oga_tools: false,
            native_session: false,
        }
    }

    pub fn oga_tools(mut self, required: bool) -> Self {
        self.oga_tools = required;
        self
    }

    pub fn native_session(mut self, native: bool) -> Self {
        self.native_session = native;
        self
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
        Self::default()
    }

    pub fn register(mut self, provider: Provider, adapter: AcpAdapter) -> Self {
        self.adapters.insert(provider, adapter);
        self
    }

    pub fn get(&self, provider: Provider) -> Option<&AcpAdapter> {
        self.adapters.get(&provider)
    }
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
        for provider in [
            Provider::Claude,
            Provider::Codex,
            Provider::OpenCode,
            Provider::OpenCode2,
            Provider::Antigravity,
            Provider::Pi,
        ] {
            assert!(adapters.get(provider).is_none(), "{provider:?}");
        }
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
