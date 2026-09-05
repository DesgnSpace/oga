//! The caller's own start time, resolved into the clock field a hold waits on.

use oga_routing::{format_rfc3339_ms, parse_rfc3339_ms};

/// What a caller's `startAt` asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartAt {
    /// A resolved instant, ISO.
    At(String),
    /// Wait for the account that ran out to have usage again.
    WhenUsageResets,
}

const RATE_LIMIT: &str = "rate_limit";

/// Reads `startAt` as the literal `rate_limit`, an ISO instant, or a duration
/// like `30m`, `4h`, `2d`. A time that has already passed is refused: it would
/// release on the next sweep, which is not what naming a time means.
pub fn parse_start_at(value: &str, now_ms: i64) -> Result<StartAt, String> {
    let text = value.trim();
    if text == RATE_LIMIT {
        return Ok(StartAt::WhenUsageResets);
    }
    if let Some(delay) = duration_ms(text) {
        return Ok(StartAt::At(format_rfc3339_ms(now_ms.saturating_add(delay))));
    }
    let instant = parse_rfc3339_ms(text).ok_or_else(|| {
        format!(
            "startAt must be \"rate_limit\", an ISO timestamp, or a duration like \"30m\", \"4h\" or \"2d\", got: {value}"
        )
    })?;
    if instant <= now_ms {
        return Err(format!("startAt is already past: {value}"));
    }
    Ok(StartAt::At(format_rfc3339_ms(instant)))
}

fn duration_ms(text: &str) -> Option<i64> {
    let unit = match text.as_bytes().last()? {
        b'm' => 60_000,
        b'h' => 3_600_000,
        b'd' => 86_400_000,
        _ => return None,
    };
    let digits = &text[..text.len() - 1];
    if digits.is_empty() || digits.bytes().any(|byte| !byte.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<i64>()
        .ok()
        .filter(|amount| *amount > 0)
        .map(|amount| amount.saturating_mul(unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_767_225_600_000;

    #[test]
    fn an_instant_is_kept_as_the_start_time() {
        assert_eq!(
            parse_start_at("2026-01-01T01:00:00.000Z", NOW),
            Ok(StartAt::At("2026-01-01T01:00:00.000Z".into()))
        );
    }

    #[test]
    fn a_duration_counts_from_now() {
        assert_eq!(
            parse_start_at("30m", NOW),
            Ok(StartAt::At(format_rfc3339_ms(NOW + 1_800_000)))
        );
        assert_eq!(
            parse_start_at("4h", NOW),
            Ok(StartAt::At(format_rfc3339_ms(NOW + 14_400_000)))
        );
        assert_eq!(
            parse_start_at("2d", NOW),
            Ok(StartAt::At(format_rfc3339_ms(NOW + 172_800_000)))
        );
    }

    #[test]
    fn the_rate_limit_literal_waits_on_the_account_instead() {
        assert_eq!(
            parse_start_at("rate_limit", NOW),
            Ok(StartAt::WhenUsageResets)
        );
    }

    #[test]
    fn a_time_that_has_passed_is_refused() {
        let error = parse_start_at("2020-01-01T00:00:00.000Z", NOW).expect_err("refused");
        assert!(error.contains("already past"), "unexpected error: {error}");
    }

    #[test]
    fn a_value_that_reads_as_neither_is_refused() {
        for value in ["soon", "0m", "45", "m", "-1h", ""] {
            let error = parse_start_at(value, NOW).expect_err("refused");
            assert!(
                error.starts_with("startAt must be"),
                "unexpected error for {value}: {error}"
            );
        }
    }
}
