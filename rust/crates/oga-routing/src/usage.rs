//! Usage-window math and the perishable-headroom tiebreak.

use oga_domain::{ModelInfo, ModelUsageSummary, ProfileUsage, UsageWindow, UsageWindowKind};

/// At this much of a window spent the run dies part-way through.
pub(crate) const NEAR_EXHAUSTED_PERCENT: f64 = 98.0;
/// Worth telling a caller that named the account itself.
pub(crate) const LOW_HEADROOM_PERCENT: f64 = 90.0;

/// A window scoped to one model family governs that family alone, matched by
/// the model id containing the family name; an account-wide window governs
/// everything.
fn scope_covers(window: &UsageWindow, model: Option<&str>) -> bool {
    match &window.model {
        None => true,
        Some(scope) => model.is_some_and(|model| model.to_lowercase().contains(scope)),
    }
}

/// How much of the capacity that governs `model` is already spent. Windows the
/// provider scoped to a different model family are excluded: an Opus week at
/// 99% is not a reason to stop dispatching Sonnet. Asked without a model, only
/// the account-wide windows count, so the number never passes a single model's
/// limit off as the account's.
pub fn worst_window_used_percent(usage: &ProfileUsage, model: Option<&str>) -> Option<f64> {
    usage
        .windows
        .iter()
        .filter(|window| scope_covers(window, model))
        .map(|window| window.used_percent)
        .reduce(f64::max)
}

/// The weekly window governing `model`, when the provider reports one. Scoped
/// the same way `worst_window_used_percent` scopes any window. Multiple
/// matches pick the one with the most used, since that is the one actually
/// constraining this model.
pub fn weekly_window<'a>(usage: &'a ProfileUsage, model: Option<&str>) -> Option<&'a UsageWindow> {
    usage
        .windows
        .iter()
        .filter(|window| window.kind == UsageWindowKind::Week && scope_covers(window, model))
        .reduce(|worst, window| {
            if window.used_percent > worst.used_percent {
                window
            } else {
                worst
            }
        })
}

/// A compact usage read for one model row, joining the account's worst
/// covering window with the rate-limit and account-failure signals already
/// on file. `known` is `false` exactly when neither is available, and then
/// `reason` always says why — an unknown read is never silent.
pub fn summarize_usage(usage: &ProfileUsage, model_id: &str) -> ModelUsageSummary {
    let worst = usage
        .windows
        .iter()
        .filter(|window| scope_covers(window, Some(model_id)))
        .reduce(|worst, window| {
            if window.used_percent > worst.used_percent {
                window
            } else {
                worst
            }
        });
    let rate_limited = usage.rate_limits_by_model.as_ref().is_some_and(|rows| {
        rows.iter()
            .any(|row| row.model == model_id || row.model == "unknown")
    });
    let out_of_credits = usage.account_failure.is_some();
    let known = worst.is_some() || rate_limited || out_of_credits;
    ModelUsageSummary {
        known,
        percent_used: worst.map(|window| window.used_percent),
        resets_text: worst.and_then(|window| window.resets_text.clone()),
        resets_at: worst.and_then(|window| window.resets_at.clone()),
        rate_limited,
        out_of_credits,
        reason: (!known).then(|| {
            usage
                .reason
                .clone()
                .unwrap_or_else(|| "usage unknown".into())
        }),
    }
}

/// How urgent it is to spend this account's headroom before the window closes
/// it out unused. Reads the weekly window rather than the session one: a
/// session recycles every few hours, so headroom lost there comes back before
/// it matters, but a week resets once and unused share in it is actually gone.
#[derive(Debug, Clone, PartialEq)]
pub struct Perishable {
    pub score: f64,
    pub window_used_percent: f64,
    pub resets_at: String,
}

/// `None` means no signal either way — no weekly window, or one with only a
/// human-rendered reset time — and a candidate far into its window
/// (`LOW_HEADROOM_PERCENT` or more) scores zero rather than unknown, since it
/// is known to be a bad target, not an unread one.
pub fn perishability(
    usage: Option<&ProfileUsage>,
    model_info: &ModelInfo,
    now_ms: i64,
) -> Option<Perishable> {
    let window = weekly_window(usage?, Some(&model_info.id))?;
    let resets_at = window.resets_at.clone().unwrap_or_default();
    if window.used_percent >= LOW_HEADROOM_PERCENT {
        return Some(Perishable {
            score: 0.0,
            window_used_percent: window.used_percent,
            resets_at,
        });
    }
    let hours_until_reset = hours_until_reset(window.resets_at.as_deref(), now_ms)?;
    Some(Perishable {
        score: (100.0 - window.used_percent) / (hours_until_reset + 1.0),
        window_used_percent: window.used_percent,
        resets_at,
    })
}

/// `resetsAt` is the only reset field read here. A provider's own localized
/// rendering is not a value this can compare against a duration, so a window
/// carrying only that contributes no perishability signal.
fn hours_until_reset(resets_at: Option<&str>, now_ms: i64) -> Option<f64> {
    let target = parse_rfc3339_ms(resets_at?)?;
    Some((target - now_ms).max(0) as f64 / 3_600_000.0)
}

/// Parse an RFC 3339 instant into epoch milliseconds. Every `resetsAt` value
/// Oga writes or providers publish carries an explicit UTC designator or
/// numeric offset; anything else is unread and yields `None`.
pub fn parse_rfc3339_ms(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let slice = value.get(range)?;
        if slice.bytes().any(|byte| !byte.is_ascii_digit()) {
            return None;
        }
        slice.parse().ok()
    };
    let year = digits(0..4)?;
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T' && bytes[10] != b't' && bytes[10] != b' '
    {
        return None;
    }
    let month = digits(5..7)?;
    let day = digits(8..10)?;
    let hour = digits(11..13)?;
    let minute = digits(14..16)?;
    let second = digits(17..19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut index = 19usize;
    let mut fraction_ms = 0i64;
    if bytes.get(index) == Some(&b'.') {
        let start = index + 1;
        let mut end = start;
        while matches!(bytes.get(end), Some(byte) if byte.is_ascii_digit()) {
            end += 1;
        }
        if end == start {
            return None;
        }
        // Millisecond precision; further digits are truncated away.
        let kept: String = value[start..end].chars().take(3).collect();
        let padded = format!("{kept:0<3}");
        fraction_ms = padded.parse().ok()?;
        index = end;
    }
    let offset_minutes = match bytes.get(index)? {
        b'Z' | b'z' => {
            if index + 1 != bytes.len() {
                return None;
            }
            0i64
        }
        sign @ (b'+' | b'-') => {
            let oh = digits(index + 1..index + 3)?;
            let om = digits(index + 4..index + 6)?;
            if bytes.get(index + 3) != Some(&b':') || index + 6 != bytes.len() {
                return None;
            }
            let magnitude = oh * 60 + om;
            if *sign == b'+' { -magnitude } else { magnitude }
        }
        _ => return None,
    };
    let days = days_from_civil(year, month as u32, day as u32);
    let millis =
        days * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1000 + fraction_ms;
    Some(millis + offset_minutes * 60_000)
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = ((month + 9) % 12) as i64;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Milliseconds since the Unix epoch right now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ModelInfoFields, model};
    use oga_domain::{Provider, UsageSource};

    fn usage_row(windows: Vec<UsageWindow>) -> ProfileUsage {
        ProfileUsage {
            profile: "p".into(),
            provider: Provider::Claude,
            supported: true,
            source: UsageSource::ClaudeCli,
            windows,
            plan: None,
            observed_at: None,
            reason: None,
            account_failure: None,
            rate_limits_by_model: None,
            observed_rate_limits: None,
        }
    }

    fn window(
        kind: UsageWindowKind,
        label: &str,
        used: f64,
        model_scope: Option<&str>,
        resets_at: Option<&str>,
    ) -> UsageWindow {
        UsageWindow {
            label: label.to_owned(),
            kind,
            used_percent: used,
            window_minutes: None,
            resets_at: resets_at.map(String::from),
            resets_text: None,
            model: model_scope.map(String::from),
        }
    }

    #[test]
    fn scoped_windows_exclude_other_model_families() {
        let row = usage_row(vec![
            window(UsageWindowKind::Session, "session", 15.0, None, None),
            window(UsageWindowKind::Week, "opus week", 99.0, Some("opus"), None),
        ]);
        assert_eq!(worst_window_used_percent(&row, Some("opus")), Some(99.0));
        assert_eq!(worst_window_used_percent(&row, Some("sonnet")), Some(15.0));
        assert_eq!(worst_window_used_percent(&row, None), Some(15.0));
    }

    #[test]
    fn the_weekly_window_is_the_most_spent_matching_one() {
        let row = usage_row(vec![
            window(UsageWindowKind::Week, "week", 20.0, None, None),
            window(UsageWindowKind::Week, "week", 45.0, None, None),
            window(UsageWindowKind::Session, "session", 90.0, None, None),
        ]);
        assert_eq!(
            weekly_window(&row, Some("sonnet")).unwrap().used_percent,
            45.0
        );
    }

    #[test]
    fn perishability_prefers_headroom_that_resets_soonest() {
        let soon = usage_row(vec![window(
            UsageWindowKind::Week,
            "week",
            45.0,
            None,
            Some("2026-08-25T12:00:00Z"),
        )]);
        let m = model("m", Provider::Claude, "p", ModelInfoFields::default());
        let now = parse_rfc3339_ms("2026-08-25T00:00:00Z").unwrap();
        let found = perishability(Some(&soon), &m, now).unwrap();
        assert!((found.score - 55.0 / 13.0).abs() < 1e-9);
        assert_eq!(found.window_used_percent, 45.0);
        assert_eq!(found.resets_at, "2026-08-25T12:00:00Z");
    }

    #[test]
    fn a_far_spent_window_scores_zero_rather_than_unknown() {
        let spent = usage_row(vec![window(
            UsageWindowKind::Week,
            "week",
            95.0,
            None,
            Some("2026-08-25T12:00:00Z"),
        )]);
        let m = model("m", Provider::Claude, "p", ModelInfoFields::default());
        assert_eq!(perishability(Some(&spent), &m, 0).unwrap().score, 0.0);
        // No weekly window at all is a different thing: no signal either way.
        let session_only = usage_row(vec![window(
            UsageWindowKind::Session,
            "s",
            10.0,
            None,
            None,
        )]);
        assert_eq!(perishability(Some(&session_only), &m, 0), None);
        // A week whose reset is prose-only cannot be compared either.
        let text_only = usage_row(vec![window(UsageWindowKind::Week, "w", 10.0, None, None)]);
        assert_eq!(perishability(Some(&text_only), &m, 0), None);
    }

    #[test]
    fn summarize_reports_the_worst_covering_window_as_known() {
        let row = usage_row(vec![
            window(UsageWindowKind::Session, "session", 15.0, None, None),
            window(
                UsageWindowKind::Week,
                "opus week",
                82.0,
                Some("opus"),
                Some("2026-08-25T12:00:00Z"),
            ),
        ]);
        let summary = summarize_usage(&row, "claude-opus");
        assert!(summary.known);
        assert_eq!(summary.percent_used, Some(82.0));
        assert_eq!(summary.resets_at.as_deref(), Some("2026-08-25T12:00:00Z"));
        assert!(!summary.rate_limited);
        assert!(!summary.out_of_credits);
        assert_eq!(summary.reason, None);
    }

    #[test]
    fn summarize_is_unknown_with_a_reason_when_nothing_is_on_file() {
        let mut row = usage_row(vec![]);
        row.supported = false;
        row.reason = Some("no usage source known for antigravity".into());
        let summary = summarize_usage(&row, "some-model");
        assert!(!summary.known);
        assert_eq!(summary.percent_used, None);
        assert_eq!(
            summary.reason.as_deref(),
            Some("no usage source known for antigravity")
        );
    }

    #[test]
    fn summarize_flags_rate_limits_and_account_failures_as_known() {
        let mut row = usage_row(vec![]);
        row.rate_limits_by_model = Some(vec![oga_domain::RateLimitByModel {
            model: "claude-opus".into(),
            message: "rate limited".into(),
            failed_at: "2026-08-25T00:00:00Z".into(),
            consecutive_failures: 1,
            retry_at: None,
        }]);
        let summary = summarize_usage(&row, "claude-opus");
        assert!(summary.known);
        assert!(summary.rate_limited);
        assert!(!summary.out_of_credits);

        let mut billing_row = usage_row(vec![]);
        billing_row.account_failure = Some(oga_domain::AccountFailure {
            message: "billing issue".into(),
            failed_at: "2026-08-25T00:00:00Z".into(),
            consecutive_failures: 1,
            retry_at: None,
        });
        let billing_summary = summarize_usage(&billing_row, "claude-opus");
        assert!(billing_summary.known);
        assert!(billing_summary.out_of_credits);
        assert!(!billing_summary.rate_limited);
    }

    #[test]
    fn rfc3339_parsing_handles_offsets_and_fractions() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00.500Z"), Some(500));
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00.5Z"), Some(500));
        assert_eq!(parse_rfc3339_ms("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("1970-01-01T00:30:00+01:00"),
            Some(-1_800_000)
        );
        assert_eq!(
            parse_rfc3339_ms("1970-01-01T00:30:00-01:30"),
            Some(7_200_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-08-05T03:20:00.000Z"),
            Some(1_785_900_000_000)
        );
        assert_eq!(parse_rfc3339_ms("not-a-time"), None);
        assert_eq!(parse_rfc3339_ms("2026-08-05T03:20:00"), None);
        assert_eq!(parse_rfc3339_ms("2026-13-05T03:20:00Z"), None);
    }
}
