//! Worker prompt assembly and completion-marker interpretation.

use std::{
    str::FromStr,
    time::{Duration, SystemTime},
};

use chrono::{Datelike, SecondsFormat, TimeZone, Utc};
use oga_domain::{CompletionCode, MemoryEntry, TaskCompletion, TaskScope, TaskState};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const PREAMBLE: [&str; 5] = [
    "Worker mode: you are executing an assigned Oga task.",
    "Continue the assigned brief directly. Do not use Oga to delegate, resume, or manage another task, and do not create a child task for the same work.",
    "If a clearly separate continuation is needed, state why it is separate, emit a compact caller-facing pointer with the child task ID and title, and let the caller start `oga watch <childTaskId>` immediately. The caller uses `inspect` after settlement. Do not include prompt or output in the pointer.",
    "Clear obstacles yourself. When the thing in the way is local, reversible, inside scope, and does not change what the task delivers — a stray generated file blocking a checkout, a stale lockfile, a missing directory, a tool needing a flag — decide, apply the fix, retry, and note it in the report.",
    "Stop only when the obstacle needs the caller: a credential, a scope or product decision, or an action that is irreversible or outside scope. A blocker is a decision you cannot make, not a step that failed once.",
];

const DELIVERY_RULES: [&str; 3] = [
    "Run JavaScript checks with `bun` or `bunx`; existing failures on the base branch do not block delivery.",
    "Commit the work without an AI attribution trailer, push the branch, and open a pull request with `gh pr create --base main`.",
    "After the pull request, run `oga relearn` once with symbols that exist in the diff; rejected route hints are a warning when the deliverable already exists.",
];

const DISCOVERY_POINTER: &str = "Finding code starts with `oga query \"<what you are looking for>\"`, every time, before any `find`, `rg`, `grep`, or glob. It is the project's own index: it takes a plain description, not just a name, and answers with the file, symbol, and line, kept in step with the working tree. Fall back to `rg` or `find` only when query returns no match, or when the task needs every occurrence rather than the right place. Read the source it names before acting.";

/// Inputs for the full prompt sent to a fresh provider session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerPromptInput {
    pub task: String,
    pub allow_questions: bool,
    pub scope: Option<TaskScope>,
    pub worker_prompt: String,
    pub context_map: Option<String>,
    pub memories: Vec<MemoryEntry>,
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

/// Assemble the stable worker document used for a new provider session.
pub fn assemble_worker_prompt(input: &WorkerPromptInput) -> String {
    let mut parts = PREAMBLE
        .iter()
        .map(|line| (*line).to_owned())
        .collect::<Vec<_>>();
    parts.extend([
        String::new(),
        DISCOVERY_POINTER.to_owned(),
        String::new(),
        input.task.clone(),
    ]);
    if let Some(scope) = &input.scope {
        parts.extend([String::new(), scope_line(scope)]);
    }
    if !input.worker_prompt.trim().is_empty() {
        parts.extend([
            String::new(),
            "## Worker rules".into(),
            input.worker_prompt.trim().into(),
        ]);
    }
    if let Some(context_map) = input
        .context_map
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.extend([String::new(), context_map.trim().to_owned()]);
    }
    if !input.memories.is_empty() {
        let facts = input
            .memories
            .iter()
            .map(|memory| format!("- {}: {}", memory.key, memory.value))
            .collect::<Vec<_>>()
            .join("\n");
        parts.extend([
            String::new(),
            "## Memories".into(),
            "Treat these project facts as shared context. If one conflicts with the task or current files, report the conflict.".into(),
            facts,
        ]);
    }
    parts.extend([String::new(), "## Delivery".into()]);
    parts.extend(DELIVERY_RULES.iter().map(|rule| (*rule).to_owned()));
    parts.extend([
        String::new(),
        "## Reporting".into(),
        if input.allow_questions {
            "If a product choice, secret, destructive action, or new authority is required, stop and end with: OGA_NEEDS_INPUT: <one clear question>".into()
        } else {
            "Do not ask questions. If required information or authority is missing, report a blocked result.".into()
        },
        "Before signing off, verify the work: run relevant checks available in your environment and report each check and result in TL;DR; quote failures exactly.".into(),
        "Before signing off, run `oga relearn '<json>'` exactly once, where `<json>` is an array of `{\"hints\":[...],\"path\":\"<file you actually read>\",\"symbol\":\"<optional symbol in it>\"}`. `hints` are the words that identify each location — order does not matter. Pass every reusable source route learned this run, or `[]` if none. Never pass the placeholder shape itself.".into(),
        "If work cannot be completed, end with: OGA_BLOCKED: <permission_denied|needs_authority|worker_error> | <short reason>".into(),
    ]);
    parts.join("\n")
}

/// Render the scope sentence shared by fresh prompts and continuation prompts.
pub fn scope_line(scope: &TaskScope) -> String {
    let mut all = scope.read.clone();
    all.extend(scope.write.iter().cloned());
    all.sort();
    all.dedup();
    format!(
        "Scope for this task, which also shapes its context map — read: {}; write: {}. That is what the caller approved, not a reading list; what to open is your call.",
        describe_rules(&all),
        describe_rules(&scope.write),
    )
}

fn describe_rules(rules: &[String]) -> String {
    if rules.is_empty() {
        return "nothing".into();
    }
    if rules.iter().any(|rule| rule == "**") {
        return "the whole working directory".into();
    }
    let shown = rules.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
    if rules.len() > 8 {
        format!("{shown} and {} more", rules.len() - 8)
    } else {
        shown
    }
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
    if let Some(blocked) = marker_value(&output, "OGA_BLOCKED") {
        let (raw_code, reason) = blocked.split_once('|').map_or(
            (blocked.as_str(), "worker reported blocked"),
            |(code, reason)| (code.trim(), reason.trim()),
        );
        let code = match raw_code.trim().to_ascii_lowercase().as_str() {
            "permission_denied" => CompletionCode::PermissionDenied,
            "needs_authority" => CompletionCode::NeedsAuthority,
            _ => CompletionCode::WorkerError,
        };
        return WorkerOutcome {
            state: TaskState::Blocked,
            output,
            question: None,
            error: Some(reason.to_owned()),
            completion: TaskCompletion {
                exit_code: exit,
                blocked: true,
                code,
                reason: Some(reason.to_owned()),
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
    let lower = output.to_ascii_lowercase();
    if !output.trim().is_empty() && !permission_block(&lower) {
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
    let code = if permission_block(&lower) {
        CompletionCode::PermissionDenied
    } else {
        CompletionCode::WorkerError
    };
    let reason = if code == CompletionCode::WorkerError {
        "worker exited without output".into()
    } else {
        compact(&output)
    };
    WorkerOutcome {
        state: TaskState::Blocked,
        output,
        question: None,
        error: Some(reason.clone()),
        completion: TaskCompletion {
            exit_code: exit,
            blocked: true,
            code,
            reason: Some(reason),
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

fn permission_block(value: &str) -> bool {
    contains_any(
        value,
        &[
            "awaiting permission",
            "awaiting approval",
            "need permission",
            "need approval",
            "needs permission",
            "needs approval",
            "requires permission",
            "requires approval",
        ],
    ) || (value.contains("cannot proceed") && value.contains("permission"))
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
    fn prompt_contains_scope_and_reporting() {
        let prompt = assemble_worker_prompt(&WorkerPromptInput {
            task: "do the thing".into(),
            allow_questions: true,
            scope: Some(TaskScope {
                read: vec!["src/**".into()],
                write: vec!["src/**".into()],
            }),
            ..WorkerPromptInput::default()
        });
        assert!(prompt.contains("do the thing"));
        assert!(prompt.contains("src/**"));
        assert!(prompt.contains("OGA_NEEDS_INPUT"));
    }

    #[test]
    fn prompt_tells_the_worker_to_clear_recoverable_obstacles() {
        let prompt = assemble_worker_prompt(&WorkerPromptInput {
            task: "do the thing".into(),
            ..WorkerPromptInput::default()
        });
        assert!(prompt.contains("Clear obstacles yourself"));
        assert!(prompt.contains("stray generated file blocking a checkout"));
        assert!(
            prompt
                .contains("A blocker is a decision you cannot make, not a step that failed once.")
        );
    }

    #[test]
    fn outcome_markers_drive_terminal_state() {
        let completed =
            interpret_worker_outcome(Some(0), "Shipped it\nOGA_RESULT: completed\n", "", None);
        assert_eq!(completed.state, TaskState::Completed);
        assert_eq!(completed.output, "Shipped it");

        let blocked = interpret_worker_outcome(
            Some(0),
            "OGA_BLOCKED: permission_denied | no access",
            "",
            None,
        );
        assert_eq!(blocked.completion.code, CompletionCode::PermissionDenied);
        assert_eq!(blocked.state, TaskState::Blocked);
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
