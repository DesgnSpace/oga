//! What a parked task waits for: the account probe, the connectivity probe,
//! and the one line every surface shows while it waits.

use std::{
    collections::BTreeSet,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Local};
use oga_config::{ResolvedModelSettings, model_enabled};
use oga_domain::{ModelInfo, ModelInfoSource, Profile, ProfileFailure, Task, WaitSettings};
use oga_routing::{AvailabilityState, model_traits, normalize_profile_statuses, now_ms};
use oga_store::{Store, StoreError};

use crate::holds::{Availability, HoldProbe};

pub const WAIT_SETTINGS_KEY: &str = "waiting";

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Reads the project's waiting policy, falling back to the defaults whenever
/// nothing was ever written or what was written no longer parses.
pub fn wait_settings(store: &Store) -> WaitSettings {
    let cwd = oga_config::global_cwd().display().to_string();
    store
        .repositories()
        .settings()
        .get(&cwd, WAIT_SETTINGS_KEY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// The line a waiting task shows while the connection is gone.
pub const NETWORK_WAIT_NOTE: &str = "Waiting for network";

/// The line a rate-limited task shows, with the local clock time it expects to
/// pick up again.
pub fn rate_limit_wait_note(resets_at: &str) -> String {
    match DateTime::parse_from_rfc3339(resets_at) {
        Ok(instant) => format!(
            "Waiting for usage to reset · resumes {}",
            instant.with_timezone(&Local).format("%H:%M")
        ),
        Err(_) => "Waiting for usage to reset".into(),
    }
}

/// The account and connectivity probes the sweep runs against the real world.
pub struct StoreProbe {
    store: Arc<Store>,
}

impl StoreProbe {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

impl HoldProbe for StoreProbe {
    /// Availability is read from the same recorded outcomes the routing screen
    /// reads, so a hold releases on exactly what the rest of the app calls
    /// usable. An account whose retry time has passed but which nothing has
    /// exercised since reads `unknown`, and unknown is enough to try again —
    /// only a real generation can prove more.
    fn profile_available(
        &self,
        profile: &str,
        model: Option<&str>,
        _cwd: &str,
    ) -> Result<Availability, String> {
        let Some(row) = self
            .store
            .repositories()
            .profiles()
            .get(profile)
            .map_err(|error| error.to_string())?
        else {
            return Ok(Availability::available());
        };
        let model = model.unwrap_or(&row.default_model).to_owned();
        let failures = load_failures(&self.store, profile).map_err(|error| error.to_string())?;
        let catalog = [model_info(&row, &model)];
        let statuses =
            normalize_profile_statuses(&[row], &catalog, &failures, &[], now_ms(), false);
        Ok(match statuses.first() {
            Some(status) if status.state == AvailabilityState::Unavailable => {
                Availability::unavailable(status.retry_at.clone())
            }
            _ => Availability::available(),
        })
    }

    /// One provider host answering is enough to call the connection back: a
    /// completed TCP handshake proves a run can leave the machine again, which
    /// a name that merely resolves does not.
    fn network_available(&self, _cwd: &str) -> Result<bool, String> {
        Ok(PROBE_HOSTS.iter().any(|host| reachable(host)))
    }
}

/// Hosts the providers actually live behind, plus a resolver for the case where
/// the machine is back but a single provider is not.
const PROBE_HOSTS: [&str; 4] = [
    "api.anthropic.com",
    "api.openai.com",
    "opencode.ai",
    "one.one.one.one",
];

fn reachable(host: &str) -> bool {
    let Ok(addresses) = (host, 443).to_socket_addrs() else {
        return false;
    };
    addresses
        .collect::<Vec<SocketAddr>>()
        .iter()
        .any(|address| TcpStream::connect_timeout(address, PROBE_TIMEOUT).is_ok())
}

fn model_info(profile: &Profile, model: &str) -> ModelInfo {
    ModelInfo {
        id: model.to_owned(),
        label: model.to_owned(),
        provider: profile.provider,
        profile_id: profile.id.clone(),
        source: ModelInfoSource::Configured,
        cost: None,
        context_window: None,
        reasoning: None,
        efforts: None,
        default_effort: None,
        tool_call: None,
    }
}

/// Another worker that can take this task now: turned on for the project, at
/// least as capable as the model that ran out, and not itself out of usage.
/// Weakest qualifying model first, so a rate limit does not silently promote a
/// task to the most expensive account on the machine.
pub fn alternative_worker(
    store: &Store,
    task: &Task,
    probe: &StoreProbe,
) -> Option<(String, String)> {
    let settings = crate::authorization::resolved_model_settings(
        store,
        &crate::authorization::settings_cwd(task),
    )
    .ok()?;
    let profiles = store.repositories().profiles().list().ok()?;
    let wanted = quality(&profiles, &task.profile_id, &task.model);
    let mut candidates: Vec<(u8, String, String)> = Vec::new();
    for profile in profiles
        .iter()
        .filter(|profile| profile.enabled && profile.id != task.profile_id)
    {
        for model in enabled_models(&settings, &profile.id) {
            let tier = model_traits(&model_info(profile, &model)).quality;
            if tier < wanted {
                continue;
            }
            let usable = probe
                .profile_available(&profile.id, Some(&model), &task.cwd)
                .is_ok_and(|availability| availability.available);
            if usable {
                candidates.push((tier, profile.id.clone(), model));
            }
        }
    }
    candidates.sort();
    candidates
        .into_iter()
        .next()
        .map(|(_, profile, model)| (profile, model))
}

fn quality(profiles: &[Profile], profile_id: &str, model: &str) -> u8 {
    profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .map_or(3, |profile| {
            model_traits(&model_info(profile, model)).quality
        })
}

/// Every model the user turned on for one worker, across the global and
/// project layers.
fn enabled_models(settings: &ResolvedModelSettings, profile: &str) -> Vec<String> {
    let mut models: BTreeSet<String> = BTreeSet::new();
    for layer in [Some(&settings.global), settings.project.as_ref()]
        .into_iter()
        .flatten()
    {
        if let Some(enablement) = layer.profiles.get(profile) {
            models.extend(enablement.model_enabled.keys().cloned());
        }
    }
    models
        .into_iter()
        .filter(|model| model_enabled(settings, profile, model))
        .collect()
}

fn load_failures(store: &Store, profile: &str) -> Result<Vec<ProfileFailure>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT code,message,failed_at,consecutive_failures,retry_at,model FROM profile_failures WHERE profile_id=?",
        )?;
        Ok(statement
            .query_map([profile], |row| {
                let code: String = row.get(0)?;
                let model: String = row.get(5)?;
                let code = serde_json::from_str(&format!("\"{code}\"")).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(ProfileFailure {
                    profile_id: profile.to_owned(),
                    code,
                    message: row.get(1)?,
                    failed_at: row.get(2)?,
                    consecutive_failures: row.get(3)?,
                    retry_at: row.get(4)?,
                    model: (!model.is_empty()).then_some(model),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_limit_note_carries_the_local_resume_time() {
        let note = rate_limit_wait_note("2026-09-02T12:00:00.000Z");
        assert!(note.starts_with("Waiting for usage to reset · resumes "));
        let expected = DateTime::parse_from_rfc3339("2026-09-02T12:00:00.000Z")
            .expect("parsed")
            .with_timezone(&Local)
            .format("%H:%M")
            .to_string();
        assert!(note.ends_with(&expected), "unexpected note: {note}");
    }

    #[test]
    fn an_unreadable_reset_time_still_reads_as_a_wait() {
        assert_eq!(
            rate_limit_wait_note("whenever"),
            "Waiting for usage to reset"
        );
    }
}
