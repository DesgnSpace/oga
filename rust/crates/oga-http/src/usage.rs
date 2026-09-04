use axum::{
    Json,
    extract::{Query, State},
};
use oga_routing::{format_rfc3339_ms, now_ms, parse_rfc3339_ms};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::router::{HttpError, HttpState};

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;

#[derive(Debug, Deserialize, Default)]
pub struct UsageQuery {
    #[serde(rename = "tzOffset")]
    pub tz_offset: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDay {
    pub date: String,
    pub cost_usd: f64,
    pub tokens: u64,
    pub tasks: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePeriod {
    pub id: &'static str,
    pub label: &'static str,
    pub cost_usd: f64,
    pub tokens: u64,
    pub tasks: u64,
    pub breakdown: Vec<UsageBreakdown>,
    pub active_days: u64,
    pub peak_hour: Option<u8>,
    pub favourite_model: Option<String>,
    pub days: Vec<UsageDay>,
    /// Local `YYYY-MM-DD` bounds of the period. The client builds the
    /// calendar grid from these plus `days`, which only holds active days.
    pub start: String,
    pub end: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBreakdown {
    pub provider: String,
    pub profile: String,
    pub model: String,
    pub cost_usd: f64,
    pub tokens: u64,
    pub tasks: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageResponse {
    pub periods: Vec<UsagePeriod>,
    pub current_streak_days: u64,
    pub longest_streak_days: u64,
}

#[derive(Debug, Clone, Copy)]
struct Period {
    id: &'static str,
    label: &'static str,
    start: i64,
    end: i64,
}

struct TaskRow {
    provider: String,
    profile: String,
    model: String,
    cost: f64,
    tokens: u64,
    timestamp: i64,
}

pub async fn get_usage(
    State(state): State<HttpState>,
    Query(query): Query<UsageQuery>,
) -> Result<Json<UsageResponse>, HttpError> {
    let offset = query.tz_offset.unwrap_or(0);
    if !(-840..=840).contains(&offset) {
        return Err(HttpError::bad_request(
            "tzOffset must be between -840 and 840 minutes",
        ));
    }
    let now = now_ms();
    let period_defs = periods(now, offset);
    let since = format_rfc3339_ms(
        period_defs
            .iter()
            .map(|period| period.start)
            .min()
            .unwrap_or(0),
    );
    let until = format_rfc3339_ms(
        period_defs
            .iter()
            .map(|period| period.end)
            .max()
            .unwrap_or(0),
    );
    let rows = state
        .store
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT COALESCE(p.provider, ''), t.profile_id, t.model, t.cost_usd,
                    COALESCE(t.tokens_in, 0) + COALESCE(t.tokens_out, 0), t.spend_at
               FROM tasks t LEFT JOIN profiles p ON p.id = t.profile_id
              WHERE t.spend_at >= ? AND t.spend_at <= ?",
            )?;
            Ok(statement
                .query_map(params![since, until], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                        row.get::<_, u64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(HttpError::from)?;

    let tasks = rows
        .into_iter()
        .filter_map(|(provider, profile, model, cost, tokens, at)| {
            parse_rfc3339_ms(&at).map(|timestamp| TaskRow {
                provider,
                profile,
                model,
                cost,
                tokens,
                timestamp,
            })
        })
        .collect::<Vec<_>>();

    let active_days = tasks
        .iter()
        .map(|row| local_day(row.timestamp, offset))
        .collect::<BTreeSet<_>>();
    let today = local_day(now, offset);
    let (current_streak_days, longest_streak_days) = streaks(&active_days, today);

    let periods = period_defs
        .into_iter()
        .map(|period| summarize(&tasks, &period, offset))
        .collect();
    Ok(Json(UsageResponse {
        periods,
        current_streak_days,
        longest_streak_days,
    }))
}

fn summarize(tasks: &[TaskRow], period: &Period, offset_minutes: i32) -> UsagePeriod {
    let mut breakdown = BTreeMap::<(String, String, String), UsageBreakdown>::new();
    let mut days = BTreeMap::<i64, UsageDay>::new();
    let mut hours = [0u64; 24];
    for row in tasks
        .iter()
        .filter(|row| row.timestamp >= period.start && row.timestamp < period.end)
    {
        let key = (row.provider.clone(), row.profile.clone(), row.model.clone());
        let entry = breakdown.entry(key).or_insert_with(|| UsageBreakdown {
            provider: row.provider.clone(),
            profile: row.profile.clone(),
            model: row.model.clone(),
            cost_usd: 0.0,
            tokens: 0,
            tasks: 0,
        });
        entry.cost_usd += row.cost;
        entry.tokens += row.tokens;
        entry.tasks += 1;

        let day = local_day(row.timestamp, offset_minutes);
        let bucket = days.entry(day).or_insert_with(|| UsageDay {
            date: format_local_date(day),
            cost_usd: 0.0,
            tokens: 0,
            tasks: 0,
        });
        bucket.cost_usd += row.cost;
        bucket.tokens += row.tokens;
        bucket.tasks += 1;

        hours[local_hour(row.timestamp, offset_minutes) as usize] += 1;
    }
    let mut breakdown = breakdown.into_values().collect::<Vec<_>>();
    breakdown.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let favourite_model = breakdown.first().map(|row| row.model.clone());
    let mut peak_hour: Option<u8> = None;
    let mut peak_count = 0u64;
    for (index, count) in hours.iter().enumerate() {
        if *count > peak_count {
            peak_count = *count;
            peak_hour = Some(index as u8);
        }
    }
    // Calendar bounds as local day indexes. `end` is exclusive, so the last
    // included instant is `end - 1`. `all` starts at the earliest active day
    // (or today when empty) instead of the epoch.
    let end_day = local_day(
        period.end.saturating_sub(1).max(period.start),
        offset_minutes,
    );
    let start_day = if period.id == "all" {
        days.keys().next().copied().unwrap_or(end_day)
    } else {
        local_day(period.start, offset_minutes)
    };
    UsagePeriod {
        id: period.id,
        label: period.label,
        cost_usd: breakdown.iter().map(|row| row.cost_usd).sum(),
        tokens: breakdown.iter().map(|row| row.tokens).sum(),
        tasks: breakdown.iter().map(|row| row.tasks).sum(),
        active_days: days.len() as u64,
        peak_hour,
        favourite_model,
        days: days.into_values().collect(),
        breakdown,
        start: format_local_date(start_day),
        end: format_local_date(end_day),
    }
}

/// Local calendar day index for an instant, shifted by the client's timezone.
fn local_day(timestamp_ms: i64, offset_minutes: i32) -> i64 {
    (timestamp_ms + i64::from(offset_minutes) * 60_000).div_euclid(DAY_MS)
}

/// Local hour of day (0-23) for an instant, shifted by the client's timezone.
fn local_hour(timestamp_ms: i64, offset_minutes: i32) -> u8 {
    ((timestamp_ms + i64::from(offset_minutes) * 60_000).rem_euclid(DAY_MS) / HOUR_MS) as u8
}

/// `YYYY-MM-DD` for a local day index. The index counts UTC days since the
/// epoch, so formatting it as a UTC date yields the local calendar date.
fn format_local_date(day: i64) -> String {
    let (year, month, date) = civil_from_days(day);
    format!("{year:04}-{month:02}-{date:02}")
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

/// Current and longest run of consecutive active days. A missing today falls
/// back to the run ending yesterday, so the streak survives until a full day
/// without work breaks it.
fn streaks(active_days: &BTreeSet<i64>, today: i64) -> (u64, u64) {
    if active_days.is_empty() {
        return (0, 0);
    }
    let mut longest = 0u64;
    let mut run = 0u64;
    let mut previous: Option<i64> = None;
    for day in active_days {
        if previous.is_some_and(|last| *day == last + 1) {
            run += 1;
        } else {
            run = 1;
        }
        longest = longest.max(run);
        previous = Some(*day);
    }
    let mut current = 0u64;
    let mut day = today;
    if !active_days.contains(&day) {
        day -= 1;
    }
    while active_days.contains(&day) {
        current += 1;
        day -= 1;
    }
    (current, longest)
}

fn periods(now: i64, offset_minutes: i32) -> [Period; 6] {
    let local_now = now + i64::from(offset_minutes) * 60_000;
    let local_midnight = local_now.div_euclid(DAY_MS) * DAY_MS - i64::from(offset_minutes) * 60_000;
    [
        Period {
            id: "all",
            label: "All",
            start: 0,
            end: now,
        },
        Period {
            id: "last30d",
            label: "30d",
            start: now - 30 * DAY_MS,
            end: now,
        },
        Period {
            id: "last7d",
            label: "7d",
            start: now - 7 * DAY_MS,
            end: now,
        },
        Period {
            id: "last24h",
            label: "24h",
            start: now - DAY_MS,
            end: now,
        },
        Period {
            id: "today",
            label: "Today",
            start: local_midnight,
            end: now,
        },
        Period {
            id: "yesterday",
            label: "Yesterday",
            start: local_midnight - DAY_MS,
            end: local_midnight,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        DAY_MS, HOUR_MS, Period, TaskRow, format_local_date, local_day, local_hour, periods,
        streaks, summarize,
    };
    use std::collections::BTreeSet;

    #[test]
    fn periods_use_local_calendar_midnight() {
        let now = 10 * DAY_MS + 2 * 60 * 60_000;
        let periods = periods(now, 120);
        assert_eq!(periods[4].end - periods[4].start, 4 * 60 * 60_000);
        assert_eq!(periods[5].end, periods[4].start);
        assert_eq!(periods[3].start, now - DAY_MS);
        assert_eq!(periods[2].start, now - 7 * DAY_MS);
        assert_eq!(periods[1].start, now - 30 * DAY_MS);
        assert_eq!(periods[0].start, 0);
    }

    #[test]
    fn day_bucketing_respects_timezone_offset() {
        // 02:30 UTC is still the previous day at UTC-4.
        let timestamp = 2 * DAY_MS + 150 * 60_000;
        assert_eq!(local_day(timestamp, 0), 2);
        assert_eq!(local_day(timestamp, -240), 1);
        assert_eq!(local_day(timestamp, 120), 2);
        assert_eq!(local_hour(timestamp, 0), 2);
        assert_eq!(local_hour(timestamp, -240), 22);
    }

    #[test]
    fn local_dates_format_as_calendar_days() {
        assert_eq!(format_local_date(0), "1970-01-01");
        assert_eq!(format_local_date(20_000), "2024-10-04");
    }

    #[test]
    fn streaks_count_current_and_longest_runs() {
        let days = BTreeSet::from([1, 2, 3, 5, 6]);
        assert_eq!(streaks(&days, 6), (2, 3));
    }

    #[test]
    fn summarize_reports_local_period_bounds() {
        // 2024-10-04 midday UTC (day 20_000, per `local_dates_format_as_calendar_days`).
        let noon = 20_000 * DAY_MS + 12 * HOUR_MS;
        let tasks = vec![TaskRow {
            provider: "test".to_string(),
            profile: "worker".to_string(),
            model: "model".to_string(),
            cost: 1.0,
            tokens: 5,
            timestamp: noon,
        }];
        let all = Period {
            id: "all",
            label: "All",
            start: 0,
            end: noon + 1,
        };
        let summary = summarize(&tasks, &all, 0);
        assert_eq!(summary.start, "2024-10-04");
        assert_eq!(summary.end, "2024-10-04");

        // An empty period still reports a day so the client grid can render.
        let today = Period {
            id: "today",
            label: "Today",
            start: noon,
            end: noon + 1,
        };
        let summary = summarize(&[], &today, 0);
        assert_eq!(summary.start, summary.end);
    }

    #[test]
    fn streaks_survive_a_quiet_today() {
        let days = BTreeSet::from([8, 9]);
        assert_eq!(streaks(&days, 10), (2, 2));
        assert_eq!(streaks(&BTreeSet::new(), 10), (0, 0));
    }
}
