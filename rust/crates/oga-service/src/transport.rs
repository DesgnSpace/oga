//! Which transport a run uses, and the per-profile preference behind it.
//!
//! The decision is made when a run opens a session and is written to the task
//! before any prompt leaves Oga. A task recorded on the command line stays
//! there; an ACP task keeps continuing its ACP conversation. Only a run that
//! opens a new session, on a profile set to `auto`, may move to the command
//! line, and only while ACP has yet to reach the agent its adapter was
//! verified against.

use std::collections::BTreeMap;

use oga_acp::Stage;
use oga_config::{canonical_cwd, global_cwd};
use oga_domain::{
    AcpRestore, Profile, Task, TaskTransport, Transport, TransportPreference, TransportReason,
};
use oga_providers::{AcpAdapter, AcpAdapters};
use oga_store::{Store, StoreError};
use serde::Serialize;

/// The settings key holding every profile's preference, under the global cwd.
pub const TRANSPORT_SETTINGS_KEY: &str = "transport";

pub fn transport_preference(
    store: &Store,
    profile_id: &str,
) -> Result<TransportPreference, StoreError> {
    Ok(saved_preferences(store)?
        .remove(profile_id)
        .unwrap_or_default())
}

pub fn set_transport_preference(
    store: &Store,
    profile_id: &str,
    preference: TransportPreference,
    now: &str,
) -> Result<(), StoreError> {
    let mut preferences = saved_preferences(store)?;
    if preference == TransportPreference::Auto {
        preferences.remove(profile_id);
    } else {
        preferences.insert(profile_id.to_owned(), preference);
    }
    let value = serde_json::to_string(&preferences)
        .map_err(|error| StoreError::Refusal(error.to_string()))?;
    store
        .repositories()
        .settings()
        .put(&settings_scope(), TRANSPORT_SETTINGS_KEY, &value, now)
}

fn saved_preferences(store: &Store) -> Result<BTreeMap<String, TransportPreference>, StoreError> {
    let Some(raw) = store
        .repositories()
        .settings()
        .get(&settings_scope(), TRANSPORT_SETTINGS_KEY)?
    else {
        return Ok(BTreeMap::new());
    };
    serde_json::from_str(&raw).map_err(|error| {
        StoreError::Refusal(format!("transport settings are not readable: {error}"))
    })
}

fn settings_scope() -> String {
    canonical_cwd(global_cwd()).display().to_string()
}

/// What a new session on this profile would run on, before anything starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveTransport {
    pub preference: TransportPreference,
    /// Absent when nothing can run: ACP is required and unavailable.
    pub transport: Option<Transport>,
    /// The adapter ACP would launch.
    pub adapter: Option<String>,
    /// Why the command line, or why nothing can run.
    pub reason: Option<TransportReason>,
}

pub fn effective_transport(
    profile: &Profile,
    preference: TransportPreference,
    adapters: &AcpAdapters,
) -> EffectiveTransport {
    let adapter = adapters
        .get(profile.provider)
        .map(|adapter| adapter.id.clone());
    let (transport, reason) = match plan_new_session(profile, preference, adapters) {
        NewSession::Acp { .. } => (Some(Transport::Acp), None),
        NewSession::Cli(reason) => (Some(Transport::Cli), Some(reason)),
        NewSession::Refuse => (None, Some(TransportReason::NoAdapter)),
    };
    EffectiveTransport {
        preference,
        transport,
        adapter,
        reason,
    }
}

/// How one run reaches its provider.
#[derive(Debug, Clone)]
pub(crate) enum TransportPlan {
    /// Run the command line. `decision` is what to record when this run is the
    /// one that decides.
    Cli { decision: Option<TaskTransport> },
    Acp {
        adapter: AcpAdapter,
        start: AcpStart,
        /// Whether this run is one that may hand the work to the command
        /// line at all: only a new session on `auto` is.
        may_fall_back: bool,
    },
    /// Nothing may run: ACP is required and there is no way to reach it.
    Refuse { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AcpStart {
    New,
    Restore { session_id: String, how: AcpRestore },
}

enum NewSession {
    Acp {
        adapter: Box<AcpAdapter>,
        may_fall_back: bool,
    },
    Cli(TransportReason),
    Refuse,
}

fn plan_new_session(
    profile: &Profile,
    preference: TransportPreference,
    adapters: &AcpAdapters,
) -> NewSession {
    if profile.command.is_some() {
        return NewSession::Cli(TransportReason::CustomCommand);
    }
    let adapter = adapters.get(profile.provider).cloned().map(Box::new);
    match (preference, adapter) {
        (TransportPreference::Cli, _) => NewSession::Cli(TransportReason::Preference),
        (TransportPreference::Auto, Some(adapter)) => NewSession::Acp {
            may_fall_back: !adapter.acp_only,
            adapter,
        },
        (TransportPreference::Auto, None) => NewSession::Cli(TransportReason::NoAdapter),
        (TransportPreference::Acp, Some(adapter)) => NewSession::Acp {
            adapter,
            may_fall_back: false,
        },
        (TransportPreference::Acp, None) => NewSession::Refuse,
    }
}

/// Whether ACP gave up before it reached the agent its adapter was verified
/// against. The command line is what keeps a provider usable where that agent
/// cannot run at all: nothing installed to start, an account ACP cannot reach,
/// a protocol or capability the adapter needs, or an agent that turns out to
/// be another release. Once it has answered `initialize`, the run holds the
/// agent Oga verified, and a failure after that is reported where a person can
/// see it rather than run again on a transport nobody chose.
pub(crate) fn failed_before_a_verified_agent(stage: Stage) -> bool {
    match stage {
        Stage::Spawn | Stage::Initialize => true,
        Stage::Session | Stage::Configure => false,
    }
}

/// Decides one run. `continuing` is the session the run was asked to reopen.
pub(crate) fn plan(
    task: &Task,
    profile: &Profile,
    preference: TransportPreference,
    adapters: &AcpAdapters,
    continuing: Option<&str>,
    now: &str,
) -> TransportPlan {
    match (&task.transport, continuing) {
        (Some(recorded), _) if recorded.kind == Transport::Cli => {
            return TransportPlan::Cli { decision: None };
        }
        // A session captured before transports were recorded is a CLI one.
        (None, Some(_)) => {
            return TransportPlan::Cli {
                decision: Some(TaskTransport::cli(TransportReason::Legacy, None, now)),
            };
        }
        (Some(recorded), Some(session_id)) => {
            let how = recorded.restore.unwrap_or(AcpRestore::Load);
            return match adapters.get(profile.provider) {
                Some(adapter) if profile.command.is_none() => TransportPlan::Acp {
                    adapter: adapter.clone(),
                    start: AcpStart::Restore {
                        session_id: session_id.to_owned(),
                        how,
                    },
                    may_fall_back: false,
                },
                _ => TransportPlan::Refuse {
                    reason: format!(
                        "{} can no longer connect over ACP, and this task's conversation needs it. Move the task to another worker to continue from a summary.",
                        profile.id
                    ),
                },
            };
        }
        _ => {}
    }
    match plan_new_session(profile, preference, adapters) {
        NewSession::Acp {
            adapter,
            may_fall_back,
        } => TransportPlan::Acp {
            adapter: *adapter,
            start: AcpStart::New,
            may_fall_back,
        },
        NewSession::Cli(reason) => TransportPlan::Cli {
            decision: Some(TaskTransport::cli(reason, None, now)),
        },
        NewSession::Refuse => TransportPlan::Refuse {
            reason: format!(
                "{} is set to use ACP, which Oga can't run for {} yet. Set the worker to use its command line, or move the task to another worker.",
                profile.id,
                profile.provider.as_str()
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use oga_domain::Provider;

    use super::*;

    const NOW: &str = "2026-01-01T00:00:00.000Z";

    fn profile(command: Option<Vec<String>>) -> Profile {
        Profile {
            id: "work".into(),
            label: "Work".into(),
            provider: Provider::Claude,
            default_model: "model".into(),
            enabled: true,
            env: BTreeMap::new(),
            capabilities: vec![],
            command,
        }
    }

    fn adapters() -> AcpAdapters {
        AcpAdapters::builtin().register(
            Provider::Claude,
            AcpAdapter::new("test-agent", |_| vec!["agent".into()]),
        )
    }

    fn acp_task(session: Option<&str>) -> Task {
        Task {
            transport: Some(TaskTransport {
                kind: Transport::Acp,
                reason: None,
                detail: None,
                acp_session_id: session.map(str::to_owned),
                restore: Some(AcpRestore::Resume),
                agent: None,
                steering: None,
                decided_at: NOW.into(),
            }),
            ..Task::default()
        }
    }

    #[test]
    fn a_new_session_on_auto_prefers_acp_and_may_fall_back() {
        let plan = plan(
            &Task::default(),
            &profile(None),
            TransportPreference::Auto,
            &adapters(),
            None,
            NOW,
        );
        assert!(matches!(
            plan,
            TransportPlan::Acp {
                start: AcpStart::New,
                may_fall_back: true,
                ..
            }
        ));
    }

    #[test]
    fn a_run_falls_back_only_until_it_reaches_the_agent_it_verified() {
        assert!(failed_before_a_verified_agent(Stage::Spawn));
        assert!(failed_before_a_verified_agent(Stage::Initialize));
        assert!(!failed_before_a_verified_agent(Stage::Session));
        assert!(!failed_before_a_verified_agent(Stage::Configure));
    }

    #[test]
    fn a_custom_command_stays_on_the_command_line() {
        let plan = plan(
            &Task::default(),
            &profile(Some(vec!["sh".into()])),
            TransportPreference::Acp,
            &adapters(),
            None,
            NOW,
        );
        let TransportPlan::Cli {
            decision: Some(decision),
        } = plan
        else {
            panic!("expected the command line, got {plan:?}");
        };
        assert_eq!(decision.reason, Some(TransportReason::CustomCommand));
    }

    #[test]
    fn explicit_acp_without_an_adapter_refuses_instead_of_falling_back() {
        let plan = plan(
            &Task::default(),
            &profile(None),
            TransportPreference::Acp,
            &AcpAdapters::default(),
            None,
            NOW,
        );
        assert!(matches!(plan, TransportPlan::Refuse { .. }), "{plan:?}");
    }

    #[test]
    fn auto_without_an_adapter_records_why_it_used_the_command_line() {
        let plan = plan(
            &Task::default(),
            &profile(None),
            TransportPreference::Auto,
            &AcpAdapters::default(),
            None,
            NOW,
        );
        let TransportPlan::Cli {
            decision: Some(decision),
        } = plan
        else {
            panic!("expected the command line, got {plan:?}");
        };
        assert_eq!(decision.reason, Some(TransportReason::NoAdapter));
    }

    #[test]
    fn a_command_line_task_never_moves_to_acp() {
        let task = Task {
            transport: Some(TaskTransport::cli(TransportReason::Unavailable, None, NOW)),
            session_id: Some("native".into()),
            ..Task::default()
        };
        for continuing in [None, Some("native")] {
            let plan = plan(
                &task,
                &profile(None),
                TransportPreference::Acp,
                &adapters(),
                continuing,
                NOW,
            );
            assert!(
                matches!(plan, TransportPlan::Cli { decision: None }),
                "{plan:?}"
            );
        }
    }

    #[test]
    fn a_session_from_before_transports_continues_on_the_command_line() {
        let task = Task {
            session_id: Some("native".into()),
            ..Task::default()
        };
        let plan = plan(
            &task,
            &profile(None),
            TransportPreference::Auto,
            &adapters(),
            Some("native"),
            NOW,
        );
        let TransportPlan::Cli {
            decision: Some(decision),
        } = plan
        else {
            panic!("expected the command line, got {plan:?}");
        };
        assert_eq!(decision.reason, Some(TransportReason::Legacy));
    }

    #[test]
    fn an_acp_conversation_continues_on_acp_and_never_falls_back() {
        let continued = plan(
            &acp_task(Some("acp-1")),
            &profile(None),
            TransportPreference::Cli,
            &adapters(),
            Some("acp-1"),
            NOW,
        );
        let TransportPlan::Acp {
            start,
            may_fall_back,
            ..
        } = continued
        else {
            panic!("expected ACP, got {continued:?}");
        };
        assert_eq!(
            start,
            AcpStart::Restore {
                session_id: "acp-1".into(),
                how: AcpRestore::Resume,
            }
        );
        assert!(!may_fall_back);

        let orphaned = plan(
            &acp_task(Some("acp-1")),
            &profile(None),
            TransportPreference::Auto,
            &AcpAdapters::default(),
            Some("acp-1"),
            NOW,
        );
        assert!(
            matches!(orphaned, TransportPlan::Refuse { .. }),
            "{orphaned:?}"
        );
    }
}
