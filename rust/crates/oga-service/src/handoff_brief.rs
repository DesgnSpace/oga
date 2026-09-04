//! Rebuilds a dead run's context from Oga's own event rows so a fresh
//! provider session — same account or a different one — can pick a task up
//! without the original brief. Port of `src/handoff-brief.ts`; keep the two
//! in step.

use oga_domain::{EventPhase, Provider, Task, TaskEvent, TaskState};
use oga_events::event_view;
use oga_providers::write_targets_from;

/// How much of the previous run's transcript may be carried verbatim.
pub const VERBATIM_CAP: usize = 24_000;
/// The lossy tier's ceiling.
pub const DIGEST_CAP: usize = 8_000;
const MAX_WRITTEN_PATHS: usize = 20;

/// Why a same-account fresh session is needed when the prior run has no
/// session to reopen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshSessionCause {
    NotStarted,
    SessionNotCaptured,
}

#[derive(Debug, Clone, Default)]
pub struct HandoffBriefOptions {
    /// The instruction in flight, from the caller or a stored resume event.
    pub instruction: Option<String>,
    /// A fresh session on the same account, instead of a handoff to a new one.
    pub same_account: bool,
    pub fresh_session_cause: Option<FreshSessionCause>,
}

pub struct HandoffBrief {
    pub prompt: String,
    pub tier: &'static str,
    pub omitted_messages: usize,
    pub chars: usize,
}

struct TranscriptLine {
    kind: LineKind,
    text: String,
}

#[derive(PartialEq)]
enum LineKind {
    Message,
    Tool,
    Error,
}

pub fn handoff_brief(
    task: &Task,
    events: &[TaskEvent],
    provider: Provider,
    options: &HandoffBriefOptions,
) -> HandoffBrief {
    let lines = transcript(events, provider);
    let verbatim = render(&lines);
    let use_verbatim = verbatim.len() <= VERBATIM_CAP;
    let (carried_text, omitted_messages) = if use_verbatim {
        (verbatim, 0)
    } else {
        digest(&lines)
    };
    let run = task.attempts.len() + 1;
    // An explicit instruction — including an explicit empty string, which
    // suppresses an older resume instruction found in the event trace — wins
    // outright; only a genuinely absent option falls back to the trace.
    let current = match &options.instruction {
        Some(raw) => {
            let trimmed = raw.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        None => in_flight_instruction(events),
    };

    let continue_tail = "Do not repeat work the trace above shows as already finished, and do not re-read files whose contents it already reports unless you need to verify them. Anything the previous run left unwritten is still yours to produce.";

    let mut sections: Vec<String> = Vec::new();
    if let Some(current) = &current {
        sections.push("# Current instruction".into());
        sections.push(current.clone());
        sections.push(String::new());
        sections.push("A previous run was working on this instruction when it stopped. The original brief below is background — it may already be finished; this instruction is the current job.".into());
        sections.push(String::new());
    }
    sections.push("# Original task".into());
    sections.push(if current.is_some() {
        "The brief that started this task, kept for context. The instruction above is the work.".into()
    } else {
        "This is the task, verbatim, exactly as it was given to the previous worker. It is the contract; nothing below replaces it.".into()
    });
    sections.push(String::new());
    sections.push(task.prompt.clone());
    sections.push(String::new());
    sections.push(if options.same_account {
        format!("# Fresh session: run {run} of this task")
    } else {
        format!("# Handoff: run {run} of this task, on a new account")
    });
    sections.push(fresh_session_line(task, options));
    sections.push(String::new());
    sections.push("## Why the previous run ended".into());
    sections.extend(ending_lines(task));
    sections.push(String::new());
    sections.extend(written_section(task, events));
    sections.push(if use_verbatim {
        "## What the previous worker said and did, verbatim".into()
    } else {
        format!(
            "## What the previous worker said and did (condensed — the full transcript exceeded {VERBATIM_CAP} characters)"
        )
    });
    sections.push(if carried_text.is_empty() {
        "_The previous run produced no readable trace._".into()
    } else {
        carried_text
    });
    sections.push(String::new());
    sections.push("# Your instruction".into());
    sections.push(if current.is_some() {
        format!("Continue the current instruction above. {continue_tail}")
    } else {
        format!("Continue this task from where the previous run stopped. {continue_tail}")
    });

    let prompt = sections.join("\n");
    HandoffBrief {
        chars: prompt.len(),
        tier: if use_verbatim { "verbatim" } else { "digest" },
        omitted_messages,
        prompt,
    }
}

fn fresh_session_line(task: &Task, options: &HandoffBriefOptions) -> String {
    if options.same_account && options.fresh_session_cause == Some(FreshSessionCause::NotStarted) {
        let reason = task
            .error
            .clone()
            .or_else(|| task.completion.as_ref().and_then(|c| c.reason.clone()))
            .or_else(|| task.hold.as_ref().map(|h| h.note.clone()))
            .unwrap_or_else(|| "it was waiting to start".into());
        return format!(
            "The previous run never started; Oga recorded: {reason}. A fresh provider session starts now."
        );
    }
    if options.same_account
        && options.fresh_session_cause == Some(FreshSessionCause::SessionNotCaptured)
    {
        return "The previous run did not capture a provider session, so it cannot be reopened. A fresh provider session starts now.".into();
    }
    if options.same_account {
        format!(
            "A previous run of this task on profile `{}` could not be continued: its provider session is unusable (a hard stop left its history unable to reopen), so its work is reproduced below from Oga's own record of the run. A fresh session starts now.",
            task.profile_id
        )
    } else {
        format!(
            "A previous worker on profile `{}` already worked on this task and could not finish. Its provider session cannot be opened from here, so its work is reproduced below from Oga's own record of the run.",
            task.profile_id
        )
    }
}

/// The instruction the dead run was executing, when it was a resume. Latest
/// `resumed` event wins; a first run never has one.
fn in_flight_instruction(events: &[TaskEvent]) -> Option<String> {
    let mut instruction = None;
    for event in events {
        if event.kind != "resumed" {
            continue;
        }
        if let Some(value) = event.payload.get("instruction").and_then(|v| v.as_str()) {
            instruction = Some(value.to_owned());
        }
    }
    instruction
}

fn ending_lines(task: &Task) -> Vec<String> {
    let completion = task.completion.as_ref();
    let moved_mid_run =
        completion.is_none() && matches!(task.state, TaskState::Running | TaskState::Queued);
    let mut lines = Vec::new();
    if moved_mid_run {
        lines.push("- It was still working when it was stopped so the task could move.".into());
    } else {
        lines.push(format!("- Oga state: `{}`", task.state.as_str()));
    }
    if let Some(completion) = completion {
        lines.push(format!(
            "- Completion code: `{}`",
            completion_code_str(completion.code)
        ));
        if let Some(reason) = &completion.reason {
            lines.push(format!("- Reason: {reason}"));
        }
    }
    if !moved_mid_run
        && let Some(error) = &task.error
        && Some(error.as_str()) != completion.and_then(|c| c.reason.as_deref())
    {
        lines.push(format!("- Error: {}", clip(error, 1_000)));
    }
    if let Some(resets_at) = completion.and_then(|c| c.resets_at.as_deref()) {
        lines.push(format!(
            "- The previous account's rate limit resets at {resets_at}."
        ));
    }
    if let Some(question) = &task.question {
        lines.push(format!("- It was asking: {question}"));
    }
    let output = task.output.trim();
    if !output.is_empty() {
        lines.push(String::new());
        lines.push("Its final message was:".into());
        lines.push(String::new());
        lines.push(quote(&clip(output, 2_000)));
    }
    lines
}

fn written_section(task: &Task, events: &[TaskEvent]) -> Vec<String> {
    let mut written: Vec<String> = Vec::new();
    for event in events {
        if !event.kind.starts_with("agent.") {
            continue;
        }
        let payload = serde_json::Value::Object(
            event
                .payload
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        for target in write_targets_from(&payload) {
            let path = relativize(&target, &task.cwd);
            if !written.contains(&path) {
                written.push(path);
            }
        }
    }
    if written.is_empty() {
        return Vec::new();
    }
    let shown: Vec<&String> = written.iter().take(MAX_WRITTEN_PATHS).collect();
    let mut lines = vec!["## Files the previous run wrote or tried to write".to_owned()];
    lines.extend(shown.iter().map(|path| format!("- {path}")));
    if written.len() > shown.len() {
        lines.push(format!("- ({} more)", written.len() - shown.len()));
    }
    lines.push("Check these on disk before rewriting them; a run that died mid-write may have left one partial.".into());
    lines.push(String::new());
    lines
}

fn transcript(events: &[TaskEvent], provider: Provider) -> Vec<TranscriptLine> {
    let mut lines: Vec<TranscriptLine> = Vec::new();
    let mut seen_calls: std::collections::HashSet<String> = std::collections::HashSet::new();
    for event in events {
        if !event.kind.starts_with("agent.") && event.kind != "scope_refusal" {
            continue;
        }
        let view = event_view(event, provider);
        if !is_work_view(event, &view) {
            continue;
        }
        let detail = view.detail.as_deref().map(str::trim);
        match view.kind {
            oga_domain::EventKind::Message => {
                let Some(text) = detail.filter(|text| !text.is_empty()) else {
                    continue;
                };
                lines.push(TranscriptLine {
                    kind: LineKind::Message,
                    text: text.to_owned(),
                });
            }
            oga_domain::EventKind::Error => {
                lines.push(TranscriptLine {
                    kind: LineKind::Error,
                    text: join(&view.title, detail),
                });
            }
            oga_domain::EventKind::Tool
            | oga_domain::EventKind::File
            | oga_domain::EventKind::Command => {
                let text = join(&view.title, detail);
                if repeats_call(&view, &text, &seen_calls, lines.last()) {
                    continue;
                }
                if let Some(action_id) = &view.action_id {
                    seen_calls.insert(action_id.clone());
                }
                lines.push(TranscriptLine {
                    kind: LineKind::Tool,
                    text,
                });
            }
            _ => {}
        }
    }
    lines
}

fn is_work_view(event: &TaskEvent, view: &oga_domain::TaskEventView) -> bool {
    if event.kind == "agent.hook" {
        let hook = event
            .payload
            .get("hook_event_name")
            .or_else(|| event.payload.get("hookEventName"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !hook.contains("ToolUse") && !hook.contains("Failure") {
            return false;
        }
    }
    if matches!(
        view.kind,
        oga_domain::EventKind::Message | oga_domain::EventKind::Error
    ) || view.kind == oga_domain::EventKind::Retry
    {
        return true;
    }
    view.minor != Some(true)
        && matches!(
            view.kind,
            oga_domain::EventKind::Tool
                | oga_domain::EventKind::File
                | oga_domain::EventKind::Command
        )
}

fn repeats_call(
    view: &oga_domain::TaskEventView,
    text: &str,
    seen: &std::collections::HashSet<String>,
    last: Option<&TranscriptLine>,
) -> bool {
    if view.phase == EventPhase::Failed {
        return false;
    }
    match &view.action_id {
        Some(action_id) => seen.contains(action_id),
        None => last.is_some_and(|line| line.kind == LineKind::Tool && line.text == text),
    }
}

fn render(lines: &[TranscriptLine]) -> String {
    lines.iter().map(label).collect::<Vec<_>>().join("\n")
}

fn label(line: &TranscriptLine) -> String {
    match line.kind {
        LineKind::Message => format!("[assistant] {}", line.text.trim()),
        LineKind::Error => format!("[error] {}", line.text),
        LineKind::Tool => format!("[tool] {}", line.text),
    }
}

/// The lossy tier. Tool calls collapse to a deduplicated list; the assistant's
/// own words are kept verbatim from the end backwards, since a run's
/// conclusions are the last thing it says.
fn digest(lines: &[TranscriptLine]) -> (String, usize) {
    let digest_message_budget = (DIGEST_CAP as f64 * 0.6).round() as usize;
    let messages: Vec<&TranscriptLine> = lines
        .iter()
        .filter(|line| line.kind == LineKind::Message)
        .collect();
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0usize;
    for message in messages.iter().rev() {
        let text = label(message);
        if used + text.len() > digest_message_budget && !kept.is_empty() {
            break;
        }
        let trimmed = if text.len() > digest_message_budget {
            let tail: String = text
                .chars()
                .rev()
                .take(digest_message_budget)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("[assistant] …{tail}")
        } else {
            text
        };
        used += trimmed.len();
        kept.insert(0, trimmed);
    }
    let omitted_messages = messages.len() - kept.len();

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut tools: Vec<String> = Vec::new();
    let mut tool_budget = DIGEST_CAP.saturating_sub(used);
    let mut omitted_tools = 0usize;
    for line in lines {
        if line.kind == LineKind::Message {
            continue;
        }
        let text = label(line);
        if seen.contains(&text) {
            continue;
        }
        seen.insert(text.clone());
        if text.len() + 1 > tool_budget {
            omitted_tools += 1;
            continue;
        }
        tool_budget -= text.len() + 1;
        tools.push(text);
    }

    let mut out: Vec<String> = vec!["### What it did (deduplicated)".into()];
    if tools.is_empty() {
        out.push("_No tool calls recorded._".into());
    } else {
        out.extend(tools);
    }
    if omitted_tools > 0 {
        out.push(format!(
            "({omitted_tools} further distinct tool calls omitted)"
        ));
    }
    out.push(String::new());
    out.push("### What it said, last messages verbatim".into());
    if omitted_messages > 0 {
        out.push(format!(
            "({omitted_messages} earlier message{} omitted from the middle of the run; the tail below is kept in full)",
            if omitted_messages == 1 { "" } else { "s" }
        ));
    }
    if kept.is_empty() {
        out.push("_No assistant messages recorded._".into());
    } else {
        out.extend(kept);
    }
    (out.join("\n"), omitted_messages)
}

fn completion_code_str(code: oga_domain::CompletionCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn relativize(path: &str, cwd: &str) -> String {
    let prefix = format!("{cwd}/");
    path.strip_prefix(prefix.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_owned())
}

fn join(title: &str, detail: Option<&str>) -> String {
    match detail {
        Some(detail) => format!("{title}: {detail}"),
        None => title.to_owned(),
    }
}

fn quote(value: &str) -> String {
    value
        .split('\n')
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn clip(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.to_owned()
    } else {
        let clipped: String = value.chars().take(limit).collect();
        format!("{clipped}…")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use oga_domain::TaskState;

    use super::*;

    fn base_task() -> Task {
        Task {
            id: "task-1".into(),
            profile_id: "one".into(),
            prompt: "MARKER: original brief text".into(),
            cwd: "/repo".into(),
            state: TaskState::Failed,
            ..Task::default()
        }
    }

    fn message_event(id: i64, text: &str) -> TaskEvent {
        TaskEvent {
            id,
            task_id: "task-1".into(),
            kind: "agent.text".into(),
            state: TaskState::Running,
            payload: BTreeMap::from([("text".into(), serde_json::json!(text))]),
            created_at: "2026-01-01T00:00:00Z".into(),
            turn_id: None,
        }
    }

    #[test]
    fn carries_the_original_task_and_the_short_transcript_verbatim() {
        let task = base_task();
        let events = vec![message_event(1, "reading the config file now")];
        let brief = handoff_brief(
            &task,
            &events,
            Provider::Claude,
            &HandoffBriefOptions {
                same_account: true,
                fresh_session_cause: Some(FreshSessionCause::SessionNotCaptured),
                ..HandoffBriefOptions::default()
            },
        );
        assert_eq!(brief.tier, "verbatim");
        assert_eq!(brief.omitted_messages, 0);
        assert!(brief.prompt.contains("MARKER: original brief text"));
        assert!(brief.prompt.contains("reading the config file now"));
        assert!(brief.prompt.contains("did not capture a provider session"));
    }

    #[test]
    fn condenses_to_the_digest_tier_once_the_transcript_exceeds_the_verbatim_cap() {
        let task = base_task();
        let long_message = "x".repeat(2_000);
        let events: Vec<TaskEvent> = (0..20)
            .map(|index| message_event(index, &format!("{long_message}-{index}")))
            .collect();
        let brief = handoff_brief(
            &task,
            &events,
            Provider::Claude,
            &HandoffBriefOptions {
                same_account: true,
                fresh_session_cause: Some(FreshSessionCause::SessionNotCaptured),
                ..HandoffBriefOptions::default()
            },
        );
        assert_eq!(brief.tier, "digest");
        assert!(brief.omitted_messages > 0);
        assert!(brief.prompt.contains("MARKER: original brief text"));
        // The tail is what a digest keeps, since a run's conclusions are the
        // last thing it says.
        assert!(brief.prompt.contains("x-19"));
        assert!(brief.chars < VERBATIM_CAP + task.prompt.len());
    }

    #[test]
    fn an_explicit_empty_instruction_suppresses_the_stored_resume_instruction() {
        let task = base_task();
        let events = vec![TaskEvent {
            id: 1,
            task_id: "task-1".into(),
            kind: "resumed".into(),
            state: TaskState::Queued,
            payload: BTreeMap::from([(
                "instruction".into(),
                serde_json::json!("compact the sidebar"),
            )]),
            created_at: "2026-01-01T00:00:00Z".into(),
            turn_id: None,
        }];
        let with_stored = handoff_brief(
            &task,
            &events,
            Provider::Claude,
            &HandoffBriefOptions {
                same_account: true,
                fresh_session_cause: Some(FreshSessionCause::SessionNotCaptured),
                ..HandoffBriefOptions::default()
            },
        );
        assert!(with_stored.prompt.contains("compact the sidebar"));

        let suppressed = handoff_brief(
            &task,
            &events,
            Provider::Claude,
            &HandoffBriefOptions {
                same_account: true,
                instruction: Some(String::new()),
                fresh_session_cause: Some(FreshSessionCause::SessionNotCaptured),
            },
        );
        assert!(!suppressed.prompt.contains("compact the sidebar"));
    }
}
