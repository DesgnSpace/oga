//! Worker prompt assembly and completion-marker interpretation.

use std::{
    str::FromStr,
    time::{Duration, SystemTime},
};

use chrono::{Datelike, SecondsFormat, TimeZone, Utc};
use oga_domain::{CompletionCode, MemoryEntry, TaskCompletion, TaskScope, TaskState};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where a worker's stamp points back to.
pub const ATTRIBUTION_EMAIL: &str = "oga@desgn.space";

/// The run this prompt was built for. Rendered verbatim into the attribution
/// instruction below, so the stamp always names the worker that ran.
/// `provider` is the provider type (`claude`, `codex`, …), never the user's
/// own profile name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerAttribution {
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
}

impl WorkerAttribution {
    /// `on claude/opus, high effort`, or `on claude/opus` when no effort ran.
    pub fn summary(&self) -> String {
        match self
            .effort
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(effort) => format!("on {}/{}, {effort} effort", self.provider, self.model),
            None => format!("on {}/{}", self.provider, self.model),
        }
    }

    /// The footer stamped on pull request bodies the worker opens.
    pub fn footer(&self) -> String {
        format!(
            "Supervised by Oga ({}) — {ATTRIBUTION_EMAIL}",
            self.summary()
        )
    }

    /// The git trailer stamped on commits the worker creates.
    pub fn trailer(&self) -> String {
        format!(
            "Supervised-by: Oga ({}) — {ATTRIBUTION_EMAIL}",
            self.summary()
        )
    }
}

/// Stamp this run unless the project turned attribution off. A config that
/// cannot be read fails open: attribution stays on, the default.
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
    let layers = match oga_config::load_config_layers(Some(cwd)) {
        Ok(layers) => layers,
        Err(_) => return Some(attribution),
    };
    let project = oga_config::read_worker_attribution(layers.project.as_ref())
        .ok()
        .flatten();
    let user = oga_config::read_worker_attribution(layers.user.as_ref())
        .ok()
        .flatten();
    oga_config::resolve_worker_attribution(project, user).then_some(attribution)
}

/// Same as [`attribution_for`], but read from the project the task was
/// delegated against: a worktree task runs in a checkout whose own directory
/// holds no `.oga.yaml`, so the origin directory decides.
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

/// The attribution block appended to the worker prompt. Kept as one function
/// so commits and pull requests always stamp the same words.
fn attribution_lines(attribution: &WorkerAttribution) -> Vec<String> {
    vec![
        "Stamp what you ship so the tool stays visible. This is on by default; skip it only when the project's own worker rules forbid tool attribution, or when the commit already carries an Oga trailer.".into(),
        format!(
            "- Commits you create: end each message with the trailer `{}` on its own line.",
            attribution.trailer()
        ),
        "- Pull requests you open: end the body with a footer on its own lines:".into(),
        "  ---".into(),
        format!("  {}", attribution.footer()),
        "- Use these words exactly as written here. Never stamp the same commit twice, and never add attribution to anything the user wrote themselves.".into(),
    ]
}

/// Who is running, for `{{task_id}}`, `{{provider}}`, `{{model}}`, and
/// `{{effort}}` substitution. Always known on fresh launches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptIdentity {
    pub task_id: String,
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
}

impl PromptIdentity {
    pub fn new(
        task_id: &str,
        provider: oga_domain::Provider,
        model: &str,
        effort: Option<&str>,
    ) -> Self {
        Self {
            task_id: task_id.to_owned(),
            provider: provider.as_str().to_owned(),
            model: model.to_owned(),
            effort: effort.map(str::to_owned),
        }
    }
}

/// Inputs for the full prompt sent to a fresh provider session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerPromptInput {
    pub task: String,
    pub allow_questions: bool,
    pub scope: Option<TaskScope>,
    /// The template, resolved upstream with the brief slot ensured.
    pub worker_prompt: String,
    pub context_map: Option<String>,
    pub memories: Vec<MemoryEntry>,
    /// `None` silences the stamp: the project turned attribution off.
    pub attribution: Option<WorkerAttribution>,
    pub identity: PromptIdentity,
}

impl Default for WorkerPromptInput {
    /// A bare input still renders the default template: continuations that
    /// rebuild without a session have no resolved prompt of their own.
    fn default() -> Self {
        Self {
            task: String::new(),
            allow_questions: false,
            scope: None,
            worker_prompt: oga_config::DEFAULT_WORKER_PROMPT.to_owned(),
            context_map: None,
            memories: Vec::new(),
            attribution: None,
            identity: PromptIdentity::default(),
        }
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

/// Assemble the worker document sent to a new provider session. The prompt is
/// the user's plain text, top to bottom: code substitutes the values it
/// knows and sends what they wrote — no imposed sections, no mandatory
/// headings, no reordering, nothing appended. The one guarantee is the task
/// slot itself, ensured upstream: without `{{brief}}` there is no delegation
/// to perform.
pub fn assemble_worker_prompt(input: &WorkerPromptInput) -> String {
    render_template(&input.worker_prompt, &template_values(input))
}

/// One substitution pass over the template. Known `{{names}}` become the
/// run's values; anything else is left verbatim, so a typo degrades to
/// visible text rather than a silent drop.
fn render_template(template: &str, values: &[(&str, String)]) -> String {
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

/// Every value a template may name. `{{memories}}`, `{{context_map}}`, and
/// `{{attribution}}` expand to a whole section or nothing: a placeholder
/// language with no conditionals cannot skip a heading, so the section goes
/// down with the placeholder.
fn template_values(input: &WorkerPromptInput) -> Vec<(&str, String)> {
    let scope = input.scope.as_ref().map(scope_line).unwrap_or_default();
    let context_map = input
        .context_map
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    let memories = if input.memories.is_empty() {
        String::new()
    } else {
        memories_section(&input.memories)
    };
    let attribution = input
        .attribution
        .as_ref()
        .map(|attribution| {
            let mut lines = vec!["## Attribution".to_owned()];
            lines.extend(attribution_lines(attribution));
            lines.join("\n")
        })
        .unwrap_or_default();
    let mut reporting = vec!["## Reporting".to_owned()];
    reporting.extend(reporting_lines(input.allow_questions));
    vec![
        ("brief", input.task.clone()),
        ("scope", scope),
        ("task_id", input.identity.task_id.clone()),
        ("provider", input.identity.provider.clone()),
        ("model", input.identity.model.clone()),
        ("effort", input.identity.effort.clone().unwrap_or_default()),
        ("context_map", context_map),
        ("memories", memories),
        ("attribution", attribution),
        ("reporting", reporting.join("\n")),
    ]
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

fn reporting_lines(allow_questions: bool) -> Vec<String> {
    vec![
        if allow_questions {
            "If a product choice, secret, destructive action, or new authority is required, stop and end with: OGA_NEEDS_INPUT: <one clear question>".into()
        } else {
            "Do not ask questions. If required information or authority is missing, report a blocked result.".into()
        },
        "Before signing off, run `oga relearn '<json>'` exactly once, where `<json>` is an array of `{\"hints\":[...],\"path\":\"<file you actually read>\",\"symbol\":\"<optional symbol in it>\"}`. `hints` are the words that identify each location — order does not matter. Pass every reusable source route learned this run, or `[]` if none. Never pass the placeholder shape itself.".into(),
        "If work cannot be completed, end with: OGA_BLOCKED: <permission_denied|needs_authority|worker_error> | <short reason>".into(),
    ]
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
    fn prompt_substitutes_values_through_placeholders() {
        let prompt = assemble_worker_prompt(&WorkerPromptInput {
            task: "do the thing".into(),
            allow_questions: true,
            scope: Some(TaskScope {
                read: vec!["src/**".into()],
                write: vec!["src/**".into()],
            }),
            worker_prompt: "{{brief}}\n\n{{scope}}\n\n{{reporting}}".into(),
            ..WorkerPromptInput::default()
        });
        assert!(prompt.contains("do the thing"));
        assert!(prompt.contains("src/**"));
        assert!(prompt.contains("OGA_NEEDS_INPUT"));
    }

    #[test]
    fn one_sentence_in_one_sentence_out() {
        let prompt = assemble_worker_prompt(&WorkerPromptInput {
            task: "do the thing".into(),
            worker_prompt: "Be terse.".into(),
            ..WorkerPromptInput::default()
        });
        assert_eq!(prompt, "Be terse.");
    }

    #[test]
    fn template_substitutes_values_and_keeps_user_order() {
        let prompt = assemble_worker_prompt(&WorkerPromptInput {
            task: "do the thing".into(),
            allow_questions: true,
            scope: Some(TaskScope {
                read: vec!["src/**".into()],
                write: vec!["src/**".into()],
            }),
            worker_prompt:
                "{{reporting}}\n\n{{brief}}\n\n{{scope}}\n\n{{memories}}\n\n{{attribution}}".into(),
            context_map: None,
            memories: vec![MemoryEntry {
                cwd: "/work".into(),
                key: "a".into(),
                value: "b".into(),
                version: 1,
                created_at: "now".into(),
                updated_at: "now".into(),
            }],
            attribution: Some(WorkerAttribution {
                provider: "claude".into(),
                model: "opus".into(),
                effort: None,
            }),
            identity: PromptIdentity {
                task_id: "t-1".into(),
                provider: "claude".into(),
                model: "opus".into(),
                effort: None,
            },
        });
        let reporting_at = prompt.find("## Reporting").expect("reporting");
        let brief_at = prompt.find("do the thing").expect("brief");
        let scope_at = prompt.find("src/**").expect("scope");
        let memories_at = prompt.find("## Memories").expect("memories");
        let attribution_at = prompt.find("## Attribution").expect("attribution");
        assert!(reporting_at < brief_at && brief_at < scope_at);
        assert!(scope_at < memories_at && memories_at < attribution_at);
        assert!(prompt.contains("- a: b"));
        assert!(!prompt.contains("{{"));
        assert_eq!(
            prompt.matches("## Reporting").count(),
            1,
            "reporting is placed, not appended twice"
        );
    }

    #[test]
    fn dropped_sections_stay_dropped() {
        let prompt = assemble_worker_prompt(&WorkerPromptInput {
            task: "do the thing".into(),
            worker_prompt: "{{brief}}".into(),
            attribution: Some(WorkerAttribution {
                provider: "claude".into(),
                model: "opus".into(),
                effort: Some("high".into()),
            }),
            ..WorkerPromptInput::default()
        });
        assert_eq!(prompt, "do the thing");
    }

    #[test]
    fn template_leaves_unknown_placeholders_verbatim() {
        let prompt = assemble_worker_prompt(&WorkerPromptInput {
            task: "do the thing".into(),
            worker_prompt: "{{brief}}\n\n{{typo}}".into(),
            ..WorkerPromptInput::default()
        });
        assert!(prompt.contains("{{typo}}"));
    }

    #[test]
    fn prompt_stamps_the_real_destination_and_nothing_else() {
        let prompt = assemble_worker_prompt(&WorkerPromptInput {
            task: "do the thing".into(),
            worker_prompt: "{{brief}}\n\n{{attribution}}".into(),
            attribution: Some(WorkerAttribution {
                provider: "claude".into(),
                model: "opus".into(),
                effort: Some("high".into()),
            }),
            ..WorkerPromptInput::default()
        });
        assert!(prompt.contains("## Attribution"));
        assert!(
            prompt.contains("Supervised-by: Oga (on claude/opus, high effort) — oga@desgn.space")
        );
        assert!(
            prompt.contains("Supervised by Oga (on claude/opus, high effort) — oga@desgn.space")
        );
        assert!(prompt.contains(ATTRIBUTION_EMAIL));
        assert!(prompt.contains("Never stamp the same commit twice"));
        assert!(!prompt.contains("without an AI attribution trailer"));
    }

    #[test]
    fn prompt_leaves_effort_off_and_stays_silent_when_opted_out() {
        let prompt = assemble_worker_prompt(&WorkerPromptInput {
            task: "do the thing".into(),
            worker_prompt: "{{brief}}\n\n{{attribution}}".into(),
            attribution: Some(WorkerAttribution {
                provider: "claude".into(),
                model: "opus".into(),
                effort: None,
            }),
            ..WorkerPromptInput::default()
        });
        assert!(prompt.contains("Supervised-by: Oga (on claude/opus) — oga@desgn.space"));
        assert!(!prompt.contains("effort)"));

        let silent = assemble_worker_prompt(&WorkerPromptInput {
            task: "do the thing".into(),
            worker_prompt: "{{brief}}\n\n{{attribution}}".into(),
            ..WorkerPromptInput::default()
        });
        assert!(!silent.contains("## Attribution"));
        assert!(!silent.contains("upervised by Oga"));
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
