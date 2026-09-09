//! Per-model account availability, read from recorded task outcomes.

use oga_domain::{FailureCode, ModelInfo, Profile, ProfileFailure, ProfileSuccess, Provider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvailabilityState {
    Available,
    Unavailable,
    Unknown,
}

impl AvailabilityState {
    pub fn as_str(self) -> &'static str {
        match self {
            AvailabilityState::Available => "available",
            AvailabilityState::Unavailable => "unavailable",
            AvailabilityState::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvailabilitySource {
    Task,
    Catalog,
    Configuration,
}

impl AvailabilitySource {
    pub fn as_str(self) -> &'static str {
        match self {
            AvailabilitySource::Task => "task",
            AvailabilitySource::Catalog => "catalog",
            AvailabilitySource::Configuration => "configuration",
        }
    }
}

/// What is known about one model on one account.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileStatus {
    pub profile: String,
    pub provider: Provider,
    pub model: String,
    pub state: AvailabilityState,
    pub source: AvailabilitySource,
    pub reason: String,
    pub checked_at: String,
    pub retry_at: Option<String>,
}

/// One status row per catalog entry, sorted by profile then model. `now_ms`
/// decides whether a network or rate-limit retry time has passed; `refreshed`
/// only changes the wording of an unknown state's provenance.
pub fn normalize_profile_statuses(
    profiles: &[Profile],
    models: &[ModelInfo],
    failures: &[ProfileFailure],
    successes: &[ProfileSuccess],
    now_ms: i64,
    refreshed: bool,
) -> Vec<ProfileStatus> {
    let enabled_profiles: Vec<&Profile> =
        profiles.iter().filter(|profile| profile.enabled).collect();
    let mut rows: Vec<ProfileStatus> = models
        .iter()
        .filter_map(|model| {
            let profile = enabled_profiles.iter().find(|p| p.id == model.profile_id)?;
            let success = successes
                .iter()
                .find(|success| success.profile_id == profile.id);
            let failure = failures
                .iter()
                .find(|failure| failure.profile_id == profile.id)
                .and_then(|failure| applicable_failure(failure, &model.id, success));
            Some(availability(failure, success, now_ms, refreshed, model))
        })
        .collect();
    rows.sort_by(|a, b| {
        a.profile
            .cmp(&b.profile)
            .then_with(|| a.model.cmp(&b.model))
    });
    rows
}

/// Which recorded failure still describes this model. Two things disqualify
/// one.
///
/// A run that succeeded after the failure was recorded is later evidence about
/// the same account, so the failure no longer describes it: the settle path
/// clears the row on a normal completion, but a task completed any other way —
/// asserted, force-settled, completed while the broker was down — leaves it
/// behind, and without this the profile reads as broken forever.
///
/// A rate limit belongs to the model that hit it. Providers meter per model,
/// so one exhausted model says nothing about the account's other models; auth
/// and billing are credentials, and network is the host, so both stay
/// account-wide.
fn applicable_failure<'a>(
    failure: &'a ProfileFailure,
    model_id: &str,
    success: Option<&ProfileSuccess>,
) -> Option<&'a ProfileFailure> {
    if let Some(success) = success {
        // An unreadable timestamp on either side leaves the finding standing,
        // matching a NaN comparison never clearing anything.
        if let (Some(succeeded), Some(failed)) = (
            crate::usage::parse_rfc3339_ms(&success.succeeded_at),
            crate::usage::parse_rfc3339_ms(&failure.failed_at),
        ) && succeeded > failed
        {
            return None;
        }
    }
    match (&failure.code, &failure.model) {
        (FailureCode::RateLimit, Some(limited)) if limited == model_id => Some(failure),
        (FailureCode::RateLimit, _) => None,
        _ => Some(failure),
    }
}

fn availability(
    failure: Option<&ProfileFailure>,
    success: Option<&ProfileSuccess>,
    now_ms: i64,
    refreshed: bool,
    model: &ModelInfo,
) -> ProfileStatus {
    let base = |state, source, reason: String, checked_at: String, retry_at| ProfileStatus {
        profile: model.profile_id.clone(),
        provider: model.provider,
        model: model.id.clone(),
        state,
        source,
        reason,
        checked_at,
        retry_at,
    };
    let Some(failure) = failure else {
        return if let Some(success) = success {
            base(
                AvailabilityState::Available,
                AvailabilitySource::Task,
                "Observed successful generation".into(),
                success.succeeded_at.clone(),
                None,
            )
        } else {
            base(
                AvailabilityState::Unknown,
                if refreshed {
                    AvailabilitySource::Catalog
                } else {
                    AvailabilitySource::Configuration
                },
                if refreshed {
                    "Catalog access does not confirm generation or billing availability".into()
                } else {
                    "No observed generation outcome".into()
                },
                format_rfc3339_ms(now_ms),
                None,
            )
        };
    };
    let checked_at = failure.failed_at.clone();
    match failure.code {
        FailureCode::Auth => base(
            AvailabilityState::Unavailable,
            AvailabilitySource::Task,
            "Observed authentication failure".into(),
            checked_at,
            None,
        ),
        FailureCode::Billing => base(
            AvailabilityState::Unavailable,
            AvailabilitySource::Task,
            "Observed billing failure".into(),
            checked_at,
            None,
        ),
        FailureCode::Network => {
            let retry_at = retry_or_default(failure, 5 * 60_000);
            if retry_after(&retry_at, now_ms) {
                base(
                    AvailabilityState::Unavailable,
                    AvailabilitySource::Task,
                    format!("Observed network failure: {}", failure.message),
                    checked_at,
                    Some(retry_at),
                )
            } else {
                base(
                    AvailabilityState::Unknown,
                    AvailabilitySource::Task,
                    format!(
                        "Network retry time passed; availability has not been rechecked \
                         (was: {})",
                        failure.message
                    ),
                    checked_at,
                    Some(retry_at),
                )
            }
        }
        FailureCode::RateLimit => {
            let retry_at = retry_or_default(failure, 10 * 60_000);
            if retry_after(&retry_at, now_ms) {
                base(
                    AvailabilityState::Unavailable,
                    AvailabilitySource::Task,
                    "Observed rate limit".into(),
                    checked_at,
                    Some(retry_at),
                )
            } else {
                base(
                    AvailabilityState::Unknown,
                    AvailabilitySource::Task,
                    "Rate-limit retry time passed; availability has not been rechecked".into(),
                    checked_at,
                    Some(retry_at),
                )
            }
        }
    }
}

/// A network failure retries after five minutes by default and a rate limit
/// after ten, when the provider did not say when.
fn retry_or_default(failure: &ProfileFailure, default_ms: i64) -> String {
    failure.retry_at.clone().unwrap_or_else(|| {
        format_rfc3339_ms(
            crate::usage::parse_rfc3339_ms(&failure.failed_at).unwrap_or(0) + default_ms,
        )
    })
}

fn retry_after(retry_at: &str, now_ms: i64) -> bool {
    crate::usage::parse_rfc3339_ms(retry_at).is_some_and(|retry| retry > now_ms)
}

/// Format epoch milliseconds as the UTC instant string every timestamp in the
/// system is written as.
pub fn format_rfc3339_ms(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let time = ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        time / 3_600_000,
        time % 3_600_000 / 60_000,
        time % 60_000 / 1000,
        millis = time % 1000
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ModelInfoFields, model};

    const NOW: i64 = 1_785_900_000_000;

    fn profile(id: &str, enabled: bool) -> Profile {
        use std::collections::BTreeMap;
        Profile {
            id: id.into(),
            label: id.into(),
            provider: Provider::Claude,
            default_model: "m".into(),
            enabled,
            env: BTreeMap::new(),
            capabilities: vec![],
            command: None,
        }
    }

    fn failure(code: FailureCode, at: &str, extra: Extra) -> ProfileFailure {
        ProfileFailure {
            profile_id: "p".into(),
            code,
            message: "it broke".into(),
            failed_at: at.into(),
            consecutive_failures: 1,
            retry_at: extra.retry_at.map(String::from),
            model: extra.model.map(String::from),
        }
    }

    #[derive(Default)]
    struct Extra<'a> {
        retry_at: Option<&'a str>,
        model: Option<&'a str>,
    }

    fn normalize(
        failures: Vec<ProfileFailure>,
        successes: Vec<ProfileSuccess>,
    ) -> Vec<ProfileStatus> {
        let profiles = [profile("p", true)];
        let models = [model(
            "m1",
            Provider::Claude,
            "p",
            ModelInfoFields::default(),
        )];
        normalize_profile_statuses(&profiles, &models, &failures, &successes, NOW, false)
    }

    #[test]
    fn auth_and_billing_failures_make_the_model_unavailable() {
        let rows = normalize(
            vec![failure(
                FailureCode::Auth,
                "2026-08-01T00:00:00Z",
                Extra::default(),
            )],
            vec![],
        );
        assert_eq!(rows[0].state, AvailabilityState::Unavailable);
        assert_eq!(rows[0].reason, "Observed authentication failure");
        assert_eq!(rows[0].checked_at, "2026-08-01T00:00:00Z");
        assert_eq!(rows[0].retry_at, None);

        let rows = normalize(
            vec![failure(
                FailureCode::Billing,
                "2026-08-01T00:00:00Z",
                Extra::default(),
            )],
            vec![],
        );
        assert_eq!(rows[0].reason, "Observed billing failure");
    }

    #[test]
    fn network_failures_retry_then_read_unknown() {
        let pending = normalize(
            vec![failure(
                FailureCode::Network,
                "2026-08-05T03:16:00Z",
                Extra::default(),
            )],
            vec![],
        );
        assert_eq!(pending[0].state, AvailabilityState::Unavailable);
        assert!(pending[0].reason.contains("Observed network failure"));
        assert_eq!(
            pending[0].retry_at.as_deref(),
            Some("2026-08-05T03:21:00.000Z")
        );

        let passed = normalize(
            vec![failure(
                FailureCode::Network,
                "2026-08-01T00:00:00Z",
                Extra::default(),
            )],
            vec![],
        );
        assert_eq!(passed[0].state, AvailabilityState::Unknown);
        assert!(passed[0].reason.starts_with("Network retry time passed"));

        let explicit = normalize(
            vec![failure(
                FailureCode::Network,
                "2026-08-01T00:00:00Z",
                Extra {
                    retry_at: Some("2999-01-01T00:00:00Z"),
                    ..Extra::default()
                },
            )],
            vec![],
        );
        assert_eq!(
            explicit[0].retry_at.as_deref(),
            Some("2999-01-01T00:00:00Z")
        );
        assert_eq!(explicit[0].state, AvailabilityState::Unavailable);
    }

    #[test]
    fn rate_limits_are_scoped_to_the_model_that_hit_them_and_expire() {
        let profiles = [profile("p", true)];
        let models = [
            model("opus", Provider::Claude, "p", ModelInfoFields::default()),
            model("sonnet", Provider::Claude, "p", ModelInfoFields::default()),
        ];
        let failures = vec![failure(
            FailureCode::RateLimit,
            "2026-08-05T03:15:00Z",
            Extra {
                model: Some("opus"),
                ..Extra::default()
            },
        )];
        let rows = normalize_profile_statuses(&profiles, &models, &failures, &[], NOW, true);
        assert_eq!(rows.len(), 2);
        let opus = rows.iter().find(|r| r.model == "opus").unwrap();
        assert_eq!(opus.state, AvailabilityState::Unavailable);
        assert_eq!(opus.reason, "Observed rate limit");
        let sonnet = rows.iter().find(|r| r.model == "sonnet").unwrap();
        assert_eq!(sonnet.state, AvailabilityState::Unknown);
        assert_eq!(sonnet.source, AvailabilitySource::Catalog);

        let expired = normalize_profile_statuses(
            &profiles,
            &models,
            &[failure(
                FailureCode::RateLimit,
                "2026-08-01T00:00:00Z",
                Extra {
                    model: Some("opus"),
                    ..Extra::default()
                },
            )],
            &[],
            NOW,
            false,
        );
        let opus = expired.iter().find(|r| r.model == "opus").unwrap();
        assert_eq!(opus.state, AvailabilityState::Unknown);
        assert!(opus.reason.starts_with("Rate-limit retry time passed"));
    }

    #[test]
    fn a_later_success_clears_a_stale_failure() {
        let rows = normalize(
            vec![failure(
                FailureCode::Billing,
                "2026-08-01T00:00:00Z",
                Extra::default(),
            )],
            vec![ProfileSuccess {
                profile_id: "p".into(),
                succeeded_at: "2026-08-02T00:00:00Z".into(),
            }],
        );
        assert_eq!(rows[0].state, AvailabilityState::Available);
        assert_eq!(rows[0].reason, "Observed successful generation");

        let earlier = normalize(
            vec![failure(
                FailureCode::Billing,
                "2026-08-03T00:00:00Z",
                Extra::default(),
            )],
            vec![ProfileSuccess {
                profile_id: "p".into(),
                succeeded_at: "2026-08-02T00:00:00Z".into(),
            }],
        );
        assert_eq!(earlier[0].state, AvailabilityState::Unavailable);
    }

    #[test]
    fn disabled_profiles_contribute_no_rows() {
        let profiles = [profile("p", false)];
        let models = [model(
            "m1",
            Provider::Claude,
            "p",
            ModelInfoFields::default(),
        )];
        let rows = normalize_profile_statuses(&profiles, &models, &[], &[], NOW, false);
        assert!(rows.is_empty());
    }
}
