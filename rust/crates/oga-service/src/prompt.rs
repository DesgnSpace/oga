//! Worker prompt assembly and completion-marker interpretation.

use std::{
    str::FromStr,
    time::{Duration, SystemTime},
};

use chrono::{Datelike, SecondsFormat, TimeZone, Utc};
use oga_domain::{CompletionCode, MemoryEntry, TaskCompletion, TaskState};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Inputs for the full prompt sent to a fresh provider session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerPromptInput {
    pub task: String,
    pub memories: Vec<MemoryEntry>,
    pub attribution: Option<WorkerAttribution>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerAttribution {
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
}

impl WorkerAttribution {
    fn trailer(&self) -> String {
        let run = match self.effort.as_deref().filter(|effort| !effort.is_empty()) {
            Some(effort) => format!("{}/{}, {effort}", self.provider, self.model),
            None => format!("{}/{}", self.provider, self.model),
        };
        format!("Co-Authored-By: Oga ({run}) <oga@desgn.space>")
    }
}

/// The provider's final text and Oga's interpretation of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerOutcome {
    pub state: TaskState,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub completion: TaskCompletion,
}

/// Asks the worker to teach `oga query` the places it worked in, so the
/// next lookup lands there. Settlement records whether it did.
const RELEARN_SECTION: &str = r#"## Before you finish
When the work is done and before your final answer, run `oga relearn` once with every file you found or changed that a later search should land on: `oga relearn '[{"hints":["<words someone would search>"],"path":"<file>","symbol":"<optional symbol>"}]'`."#;

/// Assemble the brief with its project memories and optional attribution.
pub fn assemble_worker_message(input: &WorkerPromptInput) -> String {
    let mut sections = vec![input.task.clone()];
    if !input.memories.is_empty() {
        sections.push(memories_section(&input.memories));
    }
    sections.push(RELEARN_SECTION.to_owned());
    if let Some(attribution) = &input.attribution {
        sections.push(format!(
            "## Attribution\nStamp what you ship so Oga stays visible. End each commit you create with `{}` on its own line. Never stamp the same commit twice or add attribution to work the user wrote themselves.",
            attribution.trailer()
        ));
    }
    sections.join("\n\n")
}

pub fn attribution_for(
    cwd: &std::path::Path,
    provider: &str,
    model: &str,
    effort: Option<&str>,
) -> Option<WorkerAttribution> {
    let attribution = WorkerAttribution {
        provider: provider.to_owned(),
        model: model.to_owned(),
        effort: effort.map(str::to_owned),
    };
    let Ok(layers) = oga_config::load_config_layers(Some(cwd)) else {
        return Some(attribution);
    };
    let project = oga_config::read_worker_attribution(layers.project.as_ref())
        .ok()
        .flatten();
    let user = oga_config::read_worker_attribution(layers.user.as_ref())
        .ok()
        .flatten();
    oga_config::resolve_worker_attribution(project, user).then_some(attribution)
}

pub fn attribution_for_task(
    task: &oga_domain::Task,
    provider: oga_domain::Provider,
) -> Option<WorkerAttribution> {
    let cwd = task
        .worktree
        .as_ref()
        .map(|worktree| worktree.origin_cwd.as_str())
        .unwrap_or(task.cwd.as_str());
    attribution_for(
        std::path::Path::new(cwd),
        provider.as_str(),
        &task.model,
        task.effort.as_deref(),
    )
}

/// One substitution pass over the template. Known `{{names}}` become the
/// run's values; anything else is left verbatim, so a typo degrades to
/// visible text rather than a silent drop.
pub fn render_template(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        match after.find("}}") {
            Some(close) => {
                let name = after[..close].trim();
                match values.iter().find(|(key, _)| *key == name) {
                    Some((_, value)) => out.push_str(value),
                    None => out.push_str(&rest[open..open + 2 + close + 2]),
                }
                rest = &after[close + 2..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn memories_section(memories: &[MemoryEntry]) -> String {
    let facts = memories
        .iter()
        .map(|memory| format!("- {}: {}", memory.key, memory.value))
        .collect::<Vec<_>>()
        .join("\n");
    [
        "## Memories".to_owned(),
        "Treat these project facts as shared context. If one conflicts with the task or current files, report the conflict.".to_owned(),
        facts,
    ]
    .join("\n")
}

/// Classify provider text using the same broad failure classes as the broker.
pub fn classify_failure(value: &str) -> CompletionCode {
    let lower = value.to_ascii_lowercase();
    if contains_any(
        &lower,
        &[
            "insufficient balance",
            "creditserror",
            "billing",
            "payment required",
        ],
    ) {
        return CompletionCode::Billing;
    }
    if contains_any(
        &lower,
        &[
            "connection refused",
            "connection reset",
            "connection timed out",
            "timed out",
            "timeout",
            "network",
            "no route to host",
            "temporary failure",
        ],
    ) {
        return CompletionCode::Network;
    }
    if contains_any(
        &lower,
        &[
            "unauthorized",
            "invalid api key",
            "authentication",
            "not logged in",
            "statuscode:401",
        ],
    ) {
        return CompletionCode::Auth;
    }
    if contains_any(
        &lower,
        &[
            "rate limit",
            "rate-limit",
            "too many requests",
            "session limit",
            "usage limit",
            "statuscode:429",
        ],
    ) {
        return CompletionCode::RateLimit;
    }
    if contains_any(
        &lower,
        &["permission denied", "operation not permitted", "sandbox"],
    ) {
        return CompletionCode::PermissionDenied;
    }
    CompletionCode::WorkerError
}

/// Whether provider text says the session it was asked to reopen does not
/// exist. Every provider phrases it differently and none of them give it a
/// status code, so the text is all there is to go on.
pub(crate) fn session_rejected(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    contains_any(
        &lower,
        &[
            "no conversation found",
            "conversation not found",
            "no session found",
            "session not found",
            "no such session",
            "unknown session",
            "invalid session id",
        ],
    )
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

/// Interpret a provider's final text and process result.
pub fn interpret_worker_outcome(
    exit_code: Option<i32>,
    output: impl Into<String>,
    stderr: impl Into<String>,
    aborted: Option<String>,
) -> WorkerOutcome {
    let output = output.into();
    let stderr = stderr.into();
    let exit = exit_code.map(i64::from);
    if let Some(question) =
        marker_value(&output, "OGA_NEEDS_INPUT").or_else(|| marker_value(&output, "NEEDS_INPUT"))
    {
        return WorkerOutcome {
            state: TaskState::NeedsInput,
            output,
            question: Some(question.clone()),
            error: None,
            completion: TaskCompletion {
                exit_code: exit,
                blocked: true,
                code: CompletionCode::NeedsAuthority,
                reason: Some(question),
                ..empty_completion()
            },
        };
    }
    if exit_code.is_some_and(|code| code != 0) {
        let error = if !stderr.trim().is_empty() {
            compact(&stderr)
        } else if !output.trim().is_empty() {
            compact(&output)
        } else {
            format!("exit {}", exit_code.unwrap_or_default())
        };
        let code = classify_failure(&format!("{stderr}\n{output}"));
        let resets_at = (code == CompletionCode::RateLimit)
            .then(|| rate_limit_reset_at(&format!("{stderr}\n{output}")))
            .flatten();
        return WorkerOutcome {
            state: TaskState::Failed,
            output,
            question: None,
            error: Some(error.clone()),
            completion: TaskCompletion {
                exit_code: exit,
                blocked: true,
                code,
                reason: Some(error),
                resets_at,
                ..empty_completion()
            },
        };
    }
    if let Some(line) = completed_marker(&output) {
        let output = output
            .lines()
            .filter(|line_value| *line_value != line)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned();
        return WorkerOutcome {
            state: TaskState::Completed,
            output,
            question: None,
            error: None,
            completion: TaskCompletion {
                exit_code: exit,
                blocked: false,
                code: CompletionCode::Completed,
                ..empty_completion()
            },
        };
    }
    if let Some((status, error)) = provider_error(&output) {
        let reason = compact(&format!(
            "provider returned {status}: {}",
            provider_error_message(&error).unwrap_or_else(|| error.to_string())
        ));
        let code = status_failure(status, &output);
        let resets_at = (code == CompletionCode::RateLimit)
            .then(|| rate_limit_reset_at(&output))
            .flatten();
        return WorkerOutcome {
            state: TaskState::Failed,
            output,
            question: None,
            error: Some(reason.clone()),
            completion: TaskCompletion {
                exit_code: exit,
                blocked: true,
                code,
                reason: Some(reason),
                resets_at,
                ..empty_completion()
            },
        };
    }
    if let Some(aborted) = aborted {
        return WorkerOutcome {
            state: TaskState::Failed,
            output,
            question: None,
            error: Some(aborted.clone()),
            completion: TaskCompletion {
                exit_code: exit,
                blocked: true,
                code: CompletionCode::Aborted,
                reason: Some(aborted),
                ..empty_completion()
            },
        };
    }
    if let Some(question) = prose_question(&output) {
        return WorkerOutcome {
            state: TaskState::NeedsInput,
            output,
            question: Some(question.clone()),
            error: None,
            completion: TaskCompletion {
                exit_code: exit,
                blocked: true,
                code: CompletionCode::NeedsAuthority,
                reason: Some(question),
                ..empty_completion()
            },
        };
    }
    if !output.trim().is_empty() {
        return WorkerOutcome {
            state: TaskState::Completed,
            output,
            question: None,
            error: None,
            completion: TaskCompletion {
                exit_code: exit,
                blocked: false,
                code: CompletionCode::Completed,
                ..empty_completion()
            },
        };
    }
    WorkerOutcome {
        state: TaskState::Blocked,
        output,
        question: None,
        error: Some("worker exited without output".to_owned()),
        completion: TaskCompletion {
            exit_code: exit,
            blocked: true,
            code: CompletionCode::WorkerError,
            reason: Some("worker exited without output".to_owned()),
            ..empty_completion()
        },
    }
}

fn marker_value(output: &str, marker: &str) -> Option<String> {
    let line = output.lines().rev().find(|line| !line.trim().is_empty())?;
    let trimmed = line
        .trim()
        .trim_matches(|character| "#>*-_~`".contains(character));
    let prefix = trimmed.get(..marker.len())?;
    if !prefix.eq_ignore_ascii_case(marker) {
        return None;
    }
    let value = trimmed[marker.len()..]
        .trim_start()
        .strip_prefix(':')?
        .trim();
    (!value.is_empty()).then(|| {
        value
            .trim_matches(|character| "*_~`\"'".contains(character))
            .trim()
            .to_owned()
    })
}

fn completed_marker(output: &str) -> Option<&str> {
    output.lines().find(|line| {
        let trimmed = line
            .trim()
            .trim_matches(|character| "#>*-_~`".contains(character));
        let prefix = trimmed.get(.."OGA_RESULT".len());
        prefix.is_some_and(|prefix| prefix.eq_ignore_ascii_case("OGA_RESULT"))
            && trimmed["OGA_RESULT".len()..]
                .trim_start()
                .strip_prefix(':')
                .is_some_and(|value| {
                    value
                        .trim()
                        .trim_matches(|character| "*_~`.".contains(character))
                        .eq_ignore_ascii_case("completed")
                })
    })
}

fn prose_question(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .take(8)
        .map(|line| line.trim_matches(|character| "#>*-_~`*_\"'()[]}".contains(character)))
        .find(|line| line.ends_with('?'))
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
}

fn compact(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect()
}

fn provider_error(output: &str) -> Option<(i32, Value)> {
    for line in output.lines() {
        let bytes = line.as_bytes();
        for index in 0..bytes.len().saturating_sub(2) {
            if !bytes[index..index + 3].iter().all(u8::is_ascii_digit) {
                continue;
            }
            let status = line[index..index + 3].parse::<i32>().ok()?;
            if !(400..=599).contains(&status) {
                continue;
            }
            let json = line[index + 3..].trim_start();
            if !json.starts_with('{') || !json.contains("\"error\"") {
                continue;
            }
            if let Ok(value) = serde_json::from_str(json) {
                return Some((status, value));
            }
        }
    }
    None
}

fn provider_error_message(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .map(str::to_owned)
}

fn status_failure(status: i32, output: &str) -> CompletionCode {
    match status {
        401 | 403 => CompletionCode::Auth,
        402 => CompletionCode::Billing,
        429 => CompletionCode::RateLimit,
        _ => classify_failure(output),
    }
}

pub fn rate_limit_reset_at(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    if let Some(index) = lower.find("limit reached") {
        let rest = &value[index..];
        if let Some(epoch) = digit_run_after(rest, '|') {
            let millis = if epoch < 100_000_000_000 {
                epoch.checked_mul(1_000)?
            } else {
                epoch
            };
            return crate::lifecycle::iso_from_unix_millis(millis);
        }
    }
    if let Some(index) = lower.find("resets in") {
        let mut total = 0_u64;
        let tokens = lower[index + "resets in".len()..]
            .split_whitespace()
            .take(8)
            .collect::<Vec<_>>();
        let mut token_index = 0;
        while token_index < tokens.len() {
            let token = tokens[token_index];
            let token = token.trim_matches(|character: char| !character.is_ascii_alphanumeric());
            let split = token
                .find(|character: char| !character.is_ascii_digit())
                .unwrap_or(token.len());
            if split == 0 {
                token_index += 1;
                continue;
            }
            let Some(amount) = token[..split].parse::<u64>().ok() else {
                break;
            };
            let unit = if split == token.len() {
                token_index += 1;
                tokens
                    .get(token_index)
                    .map(|unit| {
                        unit.trim_matches(|character: char| !character.is_ascii_alphabetic())
                    })
                    .unwrap_or("")
            } else {
                &token[split..]
            };
            let multiplier = if unit.starts_with('h') {
                3_600_000
            } else if unit.starts_with('m') {
                60_000
            } else if unit.starts_with('s') {
                1_000
            } else {
                token_index += 1;
                continue;
            };
            let Some(added) = amount
                .checked_mul(multiplier)
                .and_then(|value| total.checked_add(value))
            else {
                break;
            };
            total = added;
            token_index += 1;
        }
        if total > 0 {
            return Some(crate::lifecycle::iso_from_system_time(
                SystemTime::now() + Duration::from_millis(total),
            ));
        }
    }
    clock_reset_at(value, &lower)
}

/// Parses "resets 2:40am (Africa/Douala)" style clock-and-zone reset hints
/// into the next UTC instant that local time occurs.
fn clock_reset_at(value: &str, lower: &str) -> Option<String> {
    let index = lower.find("resets")?;
    let rest = &value[index..];
    let paren_start = rest.find('(')?;
    let paren_end = paren_start + rest[paren_start..].find(')')?;
    let zone_name = rest[paren_start + 1..paren_end].trim();
    let zone = chrono_tz::Tz::from_str(zone_name).ok()?;
    let time_token = rest[..paren_start]
        .split_whitespace()
        .find(|token| token.contains(':'))?;
    let time_token = time_token
        .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != ':');
    let (clock, meridiem) = match time_token.len().checked_sub(2) {
        Some(split) if time_token[split..].eq_ignore_ascii_case("am") => {
            (&time_token[..split], Some(false))
        }
        Some(split) if time_token[split..].eq_ignore_ascii_case("pm") => {
            (&time_token[..split], Some(true))
        }
        _ => (time_token, None),
    };
    let (hour_token, minute_token) = clock.split_once(':')?;
    let hour: u32 = hour_token.parse().ok()?;
    let minute: u32 = minute_token.parse().ok()?;
    let hour24 = match meridiem {
        Some(false) if hour == 12 => 0,
        Some(true) if hour != 12 => hour + 12,
        _ => hour,
    };
    if hour24 > 23 || minute > 59 {
        return None;
    }
    let now = Utc::now().with_timezone(&zone);
    let mut candidate = zone
        .with_ymd_and_hms(now.year(), now.month(), now.day(), hour24, minute, 0)
        .single()?;
    if candidate <= now {
        candidate += chrono::Duration::days(1);
    }
    Some(
        candidate
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

fn digit_run_after(value: &str, delimiter: char) -> Option<u64> {
    let start = value.find(delimiter)? + delimiter.len_utf8();
    let digits = value[start..].trim_start();
    let length = digits
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(digits.len());
    let value = &digits[..length];
    (9..=13)
        .contains(&value.len())
        .then(|| value.parse().ok())?
}

pub(crate) fn empty_completion() -> TaskCompletion {
    TaskCompletion {
        exit_code: None,
        blocked: false,
        code: CompletionCode::WorkerError,
        reason: None,
        stop_reason: None,
        suggested_scope: None,
        resets_at: None,
        asserted_completion: None,
        dependency_blocked: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembled_message_is_the_brief_then_memories_and_attribution() {
        let prompt = assemble_worker_message(&WorkerPromptInput {
            task: "do the thing".into(),
            attribution: Some(WorkerAttribution {
                provider: "claude".into(),
                model: "opus".into(),
                effort: None,
            }),
            memories: vec![MemoryEntry {
                cwd: "/work".into(),
                key: "a".into(),
                value: "b".into(),
                version: 1,
                created_at: "now".into(),
                updated_at: "now".into(),
            }],
        });
        assert_eq!(
            prompt,
            format!(
                "do the thing\n\n## Memories\nTreat these project facts as shared context. If one conflicts with the task or current files, report the conflict.\n- a: b\n\n{RELEARN_SECTION}\n\n## Attribution\nStamp what you ship so Oga stays visible. End each commit you create with `Co-Authored-By: Oga (claude/opus) <oga@desgn.space>` on its own line. Never stamp the same commit twice or add attribution to work the user wrote themselves."
            )
        );
    }

    #[test]
    fn a_bare_brief_gains_only_the_relearn_step() {
        let prompt = assemble_worker_message(&WorkerPromptInput {
            task: "do the thing".into(),
            ..WorkerPromptInput::default()
        });
        assert_eq!(prompt, format!("do the thing\n\n{RELEARN_SECTION}"));
    }

    #[test]
    fn outcome_markers_drive_terminal_state() {
        let completed =
            interpret_worker_outcome(Some(0), "Shipped it\nOGA_RESULT: completed\n", "", None);
        assert_eq!(completed.state, TaskState::Completed);
        assert_eq!(completed.output, "Shipped it");
    }

    #[test]
    fn a_worker_calling_itself_blocked_still_completes() {
        let outcome = interpret_worker_outcome(
            Some(0),
            "bun test timed out\nOGA_BLOCKED: worker_error | tests timed out",
            "",
            None,
        );
        assert_eq!(outcome.state, TaskState::Completed);
        assert_eq!(outcome.completion.code, CompletionCode::Completed);
    }

    #[test]
    fn clean_exit_with_output_completes_without_marker() {
        let outcome = interpret_worker_outcome(Some(0), "Shipped it", "", None);
        assert_eq!(outcome.state, TaskState::Completed);
        assert_eq!(outcome.completion.code, CompletionCode::Completed);
        assert!(!outcome.completion.blocked);
    }

    #[test]
    fn clean_exit_without_output_is_an_explicit_worker_error() {
        let outcome = interpret_worker_outcome(Some(0), "  \n", "", None);
        assert_eq!(outcome.state, TaskState::Blocked);
        assert_eq!(outcome.completion.code, CompletionCode::WorkerError);
        assert_eq!(
            outcome.completion.reason.as_deref(),
            Some("worker exited without output")
        );
    }

    #[test]
    fn nonzero_provider_output_keeps_failure_class() {
        let outcome = interpret_worker_outcome(Some(1), "", "rate limit reached", None);
        assert_eq!(outcome.state, TaskState::Failed);
        assert_eq!(outcome.completion.code, CompletionCode::RateLimit);
    }

    #[test]
    fn session_limit_message_carries_a_parsed_reset_time() {
        let outcome = interpret_worker_outcome(
            Some(1),
            "",
            "You've hit your session limit \u{b7} resets 2:40am (Africa/Douala)",
            None,
        );
        assert_eq!(outcome.state, TaskState::Failed);
        assert_eq!(outcome.completion.code, CompletionCode::RateLimit);
        assert!(
            outcome
                .completion
                .resets_at
                .as_deref()
                .is_some_and(|value| value.ends_with("T01:40:00.000Z")),
            "expected a parsed resetsAt, got {:?}",
            outcome.completion.resets_at
        );
    }

    #[test]
    fn zero_exit_provider_error_is_still_a_failure() {
        let outcome = interpret_worker_outcome(
            Some(0),
            r#"429 {"error":{"message":"rate limit reached"}}"#,
            "",
            None,
        );
        assert_eq!(outcome.state, TaskState::Failed);
        assert_eq!(outcome.completion.code, CompletionCode::RateLimit);
        assert!(outcome.completion.resets_at.is_none());
    }

    #[test]
    fn markers_accept_case_and_markdown_decoration() {
        let completed =
            interpret_worker_outcome(Some(0), "finished\n**oga_result: completed.**\n", "", None);
        assert_eq!(completed.state, TaskState::Completed);

        let question =
            interpret_worker_outcome(Some(0), "OGA_NEEDS_INPUT : choose a path", "", None);
        assert_eq!(question.state, TaskState::NeedsInput);
        assert_eq!(question.question.as_deref(), Some("choose a path"));
    }

    #[test]
    fn rate_limit_epoch_becomes_iso_reset_time() {
        let reset = rate_limit_reset_at("limit reached | 1760000000").expect("reset time");
        assert_eq!(reset, "2025-10-09T08:53:20.000Z");
    }

    #[test]
    fn rate_limit_duration_becomes_iso_reset_time() {
        assert!(rate_limit_reset_at("resets in 2 hours 30 minutes").is_some());
    }

    #[test]
    fn rate_limit_clock_and_zone_becomes_iso_reset_time() {
        let reset =
            rate_limit_reset_at("You've hit your session limit · resets 2:40am (Africa/Douala)")
                .expect("reset time");
        // Africa/Douala is a fixed UTC+1 offset, so 2:40am local is always 01:40 UTC.
        assert!(
            reset.ends_with("T01:40:00.000Z"),
            "unexpected reset: {reset}"
        );
    }

    #[test]
    fn rate_limit_clock_with_unknown_zone_yields_no_reset_time() {
        assert!(rate_limit_reset_at("resets 2:40am (Nowhere/Fake)").is_none());
    }
}
