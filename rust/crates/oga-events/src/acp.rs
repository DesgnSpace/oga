//! ACP `session/update` notifications as activity rows.

use std::collections::{BTreeMap, HashMap};

use oga_domain::{
    EventKind, EventPhase, PresentationType, Provider, TaskEvent, TaskEventPresentation,
    TaskEventView,
};
use serde_json::{Map, Value};

use crate::{
    ProviderViewOptions, cap, compact_change, file_presentation, format_cost, message_presentation,
    number_f64, number_u64, presentation_detail, presentation_kind, presentation_target,
    provider_view, reasoning_view, string_value, text_value, usage_presentation,
};

const CONTEXT_TITLE: &str = "Context";

const SUBJECT_LIMIT: usize = 120;

const RECOVERY_ACTION_ID: &str = "model-response-recovery";

/// Vendor-namespaced recovery metadata; unknown blocks are ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRecovery {
    /// The agent has stopped trying and needs something to change.
    pub paused: bool,
    /// Why the provider failed, in the agent's own vocabulary.
    pub cause: Option<String>,
    pub attempt: Option<u64>,
    pub attempt_limit: Option<u64>,
    /// The agent's own line about it, written for a terminal.
    pub message: Option<String>,
}

impl ModelRecovery {
    pub fn from_meta(meta: &Value) -> Option<Self> {
        let recovery = meta.get("fx")?.get("modelResponseRecovery")?;
        Some(Self {
            paused: text_value(recovery.get("state")) == Some("paused"),
            cause: text_value(recovery.get("cause")).map(str::to_owned),
            attempt: number_u64(recovery.get("attempt")),
            attempt_limit: number_u64(recovery.get("attemptLimit")),
            message: text_value(recovery.get("message"))
                .map(|message| message.trim_matches(['⚠', ' ']).to_owned())
                .filter(|message| !message.is_empty()),
        })
    }

    pub fn rate_limited(&self) -> bool {
        self.paused && self.cause.as_deref() == Some("rate_limited")
    }

    fn title(&self) -> &'static str {
        match self.cause.as_deref() {
            Some("rate_limited") => "Rate limited",
            Some("provider_unavailable") => "Provider unavailable",
            _ => "Provider error",
        }
    }
}

pub(crate) fn acp_event_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
) -> Option<TaskEventView> {
    match text_value(payload.get("sessionUpdate"))? {
        "agent_message_chunk" => Some(message_view(event, provider, payload)),
        "agent_thought_chunk" => Some(reasoning_view(event, provider, chunk_text(payload))),
        // The person's own words, which the task already shows in full. On a
        // restored session the agent replays every one of them, so the row
        // stays out of the story and behind the technical toggle.
        "user_message_chunk" => Some(prompt_view(event, provider, payload)),
        "tool_call" | "tool_call_update" => Some(tool_view(event, provider, payload)),
        "plan" => Some(plan_view(event, provider, payload)),
        "usage_update" => Some(context_view(event, provider, payload)),
        // Session bookkeeping: the agent listing what it can run, and restating
        // the session's own title. Neither is work, and both arrive around the
        // agent's closing words, where an ordinary row would compete with them.
        "available_commands_update" => Some(bookkeeping_view(event, provider, "Commands listed")),
        // Session bookkeeping is also where an agent reports its own model
        // provider failing, and that is the one thing here a reader needs.
        "session_info_update" => Some(
            match payload.get("_meta").and_then(ModelRecovery::from_meta) {
                Some(recovery) => recovery_view(event, provider, &recovery),
                None => bookkeeping_view(event, provider, "Session updated"),
            },
        ),
        _ => None,
    }
}

fn bookkeeping_view(event: &TaskEvent, provider: Provider, title: &str) -> TaskEventView {
    provider_view(
        event,
        provider,
        EventKind::Lifecycle,
        EventPhase::Info,
        title,
        ProviderViewOptions {
            detail: None,
            presentation: None,
            minor: Some(true),
        },
    )
}

fn recovery_view(event: &TaskEvent, provider: Provider, recovery: &ModelRecovery) -> TaskEventView {
    let title = recovery.title();
    let stage = match (recovery.paused, recovery.attempt, recovery.attempt_limit) {
        (true, _, Some(limit)) => format!("stopped after {limit} attempts"),
        (true, _, None) => "stopped".to_owned(),
        (false, Some(attempt), Some(limit)) => format!("retrying, attempt {attempt} of {limit}"),
        (false, _, _) => "retrying".to_owned(),
    };
    let mut view = provider_view(
        event,
        provider,
        EventKind::Lifecycle,
        if recovery.paused {
            EventPhase::Failed
        } else {
            EventPhase::Info
        },
        title,
        ProviderViewOptions {
            detail: Some(format!("{title} · {stage}")),
            presentation: None,
            minor: None,
        },
    );
    view.result = recovery
        .message
        .as_deref()
        .filter(|_| recovery.paused)
        .map(|message| cap(message, SUBJECT_LIMIT).0);
    view.action_id = Some(RECOVERY_ACTION_ID.to_owned());
    view.source_id = Some(Some(RECOVERY_ACTION_ID.to_owned()));
    view
}

fn chunk_text(payload: &BTreeMap<String, Value>) -> Option<String> {
    let content = payload.get("content")?;
    if text_value(content.get("type")) != Some("text") {
        return None;
    }
    text_value(content.get("text"))
        .map(str::to_owned)
        .filter(|text| !text.trim().is_empty())
}

fn message_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
) -> TaskEventView {
    let text = chunk_text(payload);
    let mut view = provider_view(
        event,
        provider,
        EventKind::Message,
        EventPhase::Info,
        "Agent message",
        ProviderViewOptions {
            detail: text.clone(),
            presentation: text.map(message_presentation),
            minor: None,
        },
    );
    view.complete = Some(true);
    view
}

fn prompt_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
) -> TaskEventView {
    let text = chunk_text(payload);
    provider_view(
        event,
        provider,
        EventKind::Message,
        EventPhase::Info,
        "Prompt",
        ProviderViewOptions {
            detail: text.clone(),
            presentation: text.map(message_presentation),
            minor: Some(true),
        },
    )
}

#[derive(PartialEq)]
enum Subject {
    File,
    Command,
    Other,
}

struct Category {
    title: &'static str,
    done: &'static str,
    doing: &'static str,
    subject: Subject,
}

fn category(kind: Option<&str>) -> Category {
    match kind {
        Some("read") => Category {
            title: "Read file",
            done: "Read",
            doing: "Read",
            subject: Subject::File,
        },
        Some("edit") => Category {
            title: "Edit file",
            done: "Edited",
            doing: "Edited",
            subject: Subject::File,
        },
        Some("delete") => Category {
            title: "Delete file",
            done: "Deleted",
            doing: "Deleting",
            subject: Subject::File,
        },
        Some("move") => Category {
            title: "Move file",
            done: "Moved",
            doing: "Moving",
            subject: Subject::File,
        },
        Some("search") => Category {
            title: "Search",
            done: "Searched",
            doing: "Searching",
            subject: Subject::Other,
        },
        Some("execute") => Category {
            title: "Run command",
            done: "Ran",
            doing: "Running",
            subject: Subject::Command,
        },
        Some("fetch") => Category {
            title: "Fetch page",
            done: "Fetched",
            doing: "Fetching",
            subject: Subject::Other,
        },
        Some("switch_mode") => Category {
            title: "Change mode",
            done: "Changed",
            doing: "Changing",
            subject: Subject::Other,
        },
        // A call that names neither a kind nor a title says only that the
        // worker did something.
        _ => Category {
            title: "Activity",
            done: "Used",
            doing: "Using",
            subject: Subject::Other,
        },
    }
}

fn tool_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
) -> TaskEventView {
    let kind = text_value(payload.get("kind"));
    let status = text_value(payload.get("status"));
    let complete = matches!(status, Some("completed" | "failed"));
    let phase = match status {
        Some("failed") => EventPhase::Failed,
        Some("completed") => EventPhase::Completed,
        _ => EventPhase::Started,
    };
    let subject = text_value(payload.get("title"))
        .map(|title| cap(title.trim(), SUBJECT_LIMIT).0)
        .filter(|title| !title.is_empty());

    // A thinking call is the model reasoning out loud through a tool. It reads
    // as thinking, so it folds in with the thought chunks around it.
    if kind == Some("think") {
        let mut view = reasoning_view(event, provider, subject);
        view.action_id = tool_call_id(payload);
        view.source_id = Some(tool_call_id(payload));
        view.complete = Some(complete);
        return view;
    }

    if let Some(items) = todo_items(payload) {
        let mut view = todo_view(event, provider, items, Some(complete));
        view.action_id = tool_call_id(payload);
        view.source_id = Some(tool_call_id(payload));
        return view;
    }

    let category = category(kind);
    let presentation = tool_presentation(&category, payload, subject.clone());
    // Nothing but the agent's own title says what an uncategorised call did,
    // so that title names the row.
    let uncategorised =
        category.subject == Subject::Other && kind.is_none_or(|kind| kind == "other");
    let title = match subject.clone().filter(|_| uncategorised) {
        Some(agent_title) => agent_title,
        None => category.title.to_owned(),
    };
    let mut view = provider_view(
        event,
        provider,
        presentation_kind(&presentation),
        phase,
        &title,
        ProviderViewOptions {
            detail: presentation_detail(Some(&presentation)),
            presentation: Some(presentation.clone()),
            minor: None,
        },
    );
    view.verb = Some(
        if complete {
            category.done
        } else {
            category.doing
        }
        .to_owned(),
    );
    view.target = presentation_target(Some(&presentation));
    view.result = presentation.outcome.clone();
    view.complete = Some(complete);
    view.action_id = tool_call_id(payload);
    view.source_id = Some(tool_call_id(payload));
    view
}

fn tool_call_id(payload: &BTreeMap<String, Value>) -> Option<String> {
    text_value(payload.get("toolCallId"))
        .map(str::to_owned)
        .filter(|id| !id.is_empty())
}

const CALL_FIELDS: [&str; 6] = ["kind", "status", "title", "name", "locations", "rawInput"];

#[derive(Default)]
pub(crate) struct AcpCalls {
    known: HashMap<(Option<i64>, String), Map<String, Value>>,
}

impl AcpCalls {
    pub(crate) fn patch(&mut self, event: &mut TaskEvent) {
        let update = text_value(event.payload.get("sessionUpdate"));
        if !matches!(update, Some("tool_call" | "tool_call_update")) {
            return;
        }
        let Some(id) = tool_call_id(&event.payload) else {
            return;
        };
        let known = self.known.entry((event.turn_id, id)).or_default();
        if update == Some("tool_call") {
            known.clear();
        }
        for field in CALL_FIELDS {
            match event.payload.get(field) {
                Some(value) => {
                    known.insert(field.to_owned(), value.clone());
                }
                None => {
                    if let Some(value) = known.get(field) {
                        event.payload.insert(field.to_owned(), value.clone());
                    }
                }
            }
        }
    }
}

fn tool_presentation(
    category: &Category,
    payload: &BTreeMap<String, Value>,
    subject: Option<String>,
) -> TaskEventPresentation {
    let raw_input = payload.get("rawInput").and_then(Value::as_object);
    let diff = diff_content(payload);
    let mut presentation = usage_presentation();
    presentation.outcome = outcome(payload);

    if category.subject == Subject::File {
        let path = diff
            .and_then(|diff| text_value(diff.get("path")))
            .map(str::to_owned)
            .or_else(|| location_path(payload))
            .or_else(|| {
                raw_input.and_then(|input| string_value(input, &["filePath", "file_path", "path"]))
            });
        if let Some(path) = path {
            let mut file = file_presentation(path);
            file.outcome = presentation.outcome;
            file.change = diff.and_then(change_summary);
            return file;
        }
    }
    if category.subject == Subject::Command
        && let Some(command) = raw_input
            .and_then(|input| string_value(input, &["command", "cmd"]))
            .or_else(|| subject.clone())
    {
        presentation.kind = PresentationType::Command;
        presentation.text = Some(cap(&command, SUBJECT_LIMIT).0);
        presentation.command = Some(command);
        return presentation;
    }
    presentation.kind = PresentationType::Tool;
    presentation.text = subject.or_else(|| location_path(payload));
    presentation
}

fn location_path(payload: &BTreeMap<String, Value>) -> Option<String> {
    let locations = payload.get("locations")?.as_array()?;
    let first = locations.first()?;
    let path = text_value(first.get("path"))?;
    match number_u64(first.get("line")) {
        Some(line) => Some(format!("{path}:{line}")),
        None => Some(path.to_owned()),
    }
}

fn diff_content(payload: &BTreeMap<String, Value>) -> Option<&Map<String, Value>> {
    payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .find(|entry| text_value(entry.get("type")) == Some("diff"))
}

fn change_summary(diff: &Map<String, Value>) -> Option<String> {
    let new_text = text_value(diff.get("newText"))?;
    match text_value(diff.get("oldText")) {
        Some(old_text) => Some(compact_change(old_text, new_text)),
        None => {
            let lines = new_text.lines().count().max(1);
            Some(format!("{lines} line{}", if lines == 1 { "" } else { "s" }))
        }
    }
}

fn outcome(payload: &BTreeMap<String, Value>) -> Option<String> {
    let content = payload.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter_map(Value::as_object)
        .filter(|entry| text_value(entry.get("type")) == Some("content"))
        .filter_map(|entry| {
            let block = entry.get("content")?;
            (text_value(block.get("type")) == Some("text"))
                .then(|| text_value(block.get("text")))
                .flatten()
        })
        .find(|text| !text.trim().is_empty())?;
    Some(cap(text.trim(), SUBJECT_LIMIT).0)
}

fn plan_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
) -> TaskEventView {
    let entries = payload
        .get("entries")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut view = todo_view(event, provider, entries, None);
    view.source_id = Some(None);
    view
}

/// The items of an agent's todo tool (opencode's `todowrite` over ACP), from
/// its input or, once the call completes under a renamed title, its output.
fn todo_items(payload: &BTreeMap<String, Value>) -> Option<&[Value]> {
    let from_input = payload.get("rawInput").and_then(|input| input.get("todos"));
    let from_output = payload
        .get("rawOutput")
        .and_then(|output| output.get("metadata"))
        .and_then(|metadata| metadata.get("todos"));
    from_output
        .or(from_input)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

/// A checklist row, shared by ACP plans and todo tool calls. `complete` is the
/// call's own status when there is one; a plan is complete when every step is.
fn todo_view(
    event: &TaskEvent,
    provider: Provider,
    entries: &[Value],
    complete: Option<bool>,
) -> TaskEventView {
    let total = entries.len() as u64;
    let completed = entries
        .iter()
        .filter(|entry| text_value(entry.get("status")) == Some("completed"))
        .count() as u64;
    let mut presentation = usage_presentation();
    presentation.kind = PresentationType::Todo;
    presentation.total = Some(total);
    presentation.completed = Some(completed);
    presentation.text = Some(format!("{total} step{}", if total == 1 { "" } else { "s" }));
    presentation.outcome = entries
        .iter()
        .find(|entry| text_value(entry.get("status")) == Some("in_progress"))
        .or_else(|| {
            entries
                .iter()
                .find(|entry| text_value(entry.get("status")) == Some("pending"))
        })
        .and_then(|entry| text_value(entry.get("content")))
        .map(|content| cap(content.trim(), SUBJECT_LIMIT).0)
        .or_else(|| (total > 0 && completed == total).then(|| "all done".to_owned()));
    let done = complete.unwrap_or(total > 0 && completed == total);
    let mut view = provider_view(
        event,
        provider,
        EventKind::Tool,
        if done {
            EventPhase::Completed
        } else {
            EventPhase::Started
        },
        "Todo list",
        ProviderViewOptions {
            detail: presentation_detail(Some(&presentation)),
            presentation: Some(presentation.clone()),
            minor: None,
        },
    );
    view.verb = Some(if done { "Updated" } else { "Updating" }.to_owned());
    view.target = presentation.text.clone();
    view.result = presentation.outcome.clone();
    view.complete = Some(done);
    view
}

fn context_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
) -> TaskEventView {
    let used = number_u64(payload.get("used"));
    let size = number_u64(payload.get("size"));
    let cost = session_cost(payload);
    let mut presentation = usage_presentation();
    presentation.tokens_in = used;
    presentation.total = size;
    presentation.cost_usd = cost;
    let detail = [
        match (used, size) {
            (Some(used), Some(size)) if size > 0 => Some(format!("{}% used", used * 100 / size)),
            _ => None,
        },
        cost.map(|cost| format!("{} so far", format_cost(cost))),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    provider_view(
        event,
        provider,
        EventKind::Usage,
        EventPhase::Info,
        CONTEXT_TITLE,
        ProviderViewOptions {
            detail: (!detail.is_empty()).then_some(detail),
            presentation: Some(presentation),
            minor: Some(true),
        },
    )
}

fn session_cost(payload: &BTreeMap<String, Value>) -> Option<f64> {
    let cost = payload.get("cost")?;
    if !text_value(cost.get("currency"))
        .is_some_and(|currency| currency.eq_ignore_ascii_case("usd"))
    {
        return None;
    }
    number_f64(cost.get("amount")).filter(|amount| *amount >= 0.0)
}

#[cfg(test)]
mod tests {
    use oga_domain::TaskState;
    use serde_json::json;

    use super::*;
    use crate::REASONING_TITLE;

    fn acp_event(id: i64, update: Value) -> TaskEvent {
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .expect("an update names itself");
        TaskEvent {
            id,
            task_id: "t1".into(),
            kind: format!("agent.{kind}"),
            state: TaskState::Running,
            payload: update
                .as_object()
                .expect("an update is an object")
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            created_at: "2026-09-17T10:00:00.000Z".into(),
            turn_id: Some(1),
        }
    }

    fn view(update: Value) -> TaskEventView {
        crate::event_view(&acp_event(1, update), Provider::Claude)
    }

    #[test]
    fn an_edit_reads_as_a_file_change_with_its_diff() {
        let view = view(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call_1",
            "title": "Edit src/main.rs",
            "kind": "edit",
            "status": "completed",
            "content": [{
                "type": "diff",
                "path": "/repo/src/main.rs",
                "oldText": "fn main() {}",
                "newText": "fn main() { run(); }",
            }],
            "locations": [{"path": "/repo/src/main.rs", "line": 1}],
        }));

        assert_eq!(view.title, "Edit file");
        assert_eq!(view.kind, EventKind::File);
        assert_eq!(view.phase, EventPhase::Completed);
        assert_eq!(view.verb.as_deref(), Some("Edited"));
        assert_eq!(view.target.as_deref(), Some("/repo/src/main.rs"));
        assert_eq!(view.action_id.as_deref(), Some("call_1"));
        assert_eq!(view.source_id, Some(Some("call_1".to_owned())));
        assert_eq!(view.complete, Some(true));
        let presentation = view.presentation.expect("an edit presents a file");
        assert_eq!(presentation.kind, PresentationType::File);
        assert_eq!(
            presentation.change.as_deref(),
            Some("fn main() {} → fn main() { run(); }")
        );
    }

    #[test]
    fn a_call_in_flight_opens_a_row_its_update_settles() {
        let started = view(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call_2",
            "title": "Run the tests",
            "kind": "execute",
            "status": "in_progress",
            "rawInput": {"command": "cargo test -p oga-events"},
        }));
        assert_eq!(started.title, "Run command");
        assert_eq!(started.kind, EventKind::Command);
        assert_eq!(started.phase, EventPhase::Started);
        assert_eq!(started.verb.as_deref(), Some("Running"));
        assert_eq!(started.complete, Some(false));
        assert_eq!(
            started
                .presentation
                .as_ref()
                .and_then(|value| value.command.as_deref()),
            Some("cargo test -p oga-events")
        );

        let settled = view(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call_2",
            "status": "failed",
            "content": [{"type": "content", "content": {"type": "text", "text": "1 test failed"}}],
        }));
        assert_eq!(settled.action_id.as_deref(), Some("call_2"));
        assert_eq!(settled.phase, EventPhase::Failed);
        assert_eq!(settled.complete, Some(true));
        assert_eq!(settled.result.as_deref(), Some("1 test failed"));
    }

    #[test]
    fn an_uncategorised_call_is_named_by_the_agents_own_title() {
        let view = view(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call_3",
            "title": "Ask the design system for a token",
            "status": "pending",
        }));
        assert_eq!(view.title, "Ask the design system for a token");
        assert_eq!(view.kind, EventKind::Tool);
        assert_eq!(view.verb.as_deref(), Some("Using"));
        assert_eq!(
            view.target.as_deref(),
            Some("Ask the design system for a token")
        );
    }

    #[test]
    fn a_read_without_a_path_still_names_what_it_read() {
        let view = view(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call_4",
            "title": "Read the open buffer",
            "kind": "read",
            "status": "completed",
        }));
        assert_eq!(view.title, "Read file");
        assert_eq!(view.target.as_deref(), Some("Read the open buffer"));
    }

    #[test]
    fn a_message_and_a_thought_read_as_themselves() {
        let message = view(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "Checking the runner"},
        }));
        assert_eq!(message.kind, EventKind::Message);
        assert_eq!(message.title, "Agent message");
        assert_eq!(message.detail.as_deref(), Some("Checking the runner"));
        assert_ne!(message.minor, Some(true));

        let thought = view(json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "The timeout is the backstop"},
        }));
        assert_eq!(thought.kind, EventKind::Reasoning);
        assert_eq!(thought.title, REASONING_TITLE);

        let prompt = view(json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "Port the runner"},
        }));
        assert_eq!(prompt.minor, Some(true));
    }

    #[test]
    fn a_plan_reads_as_the_same_checklist_every_provider_shows() {
        let view = view(json!({
            "sessionUpdate": "plan",
            "entries": [
                {"content": "Read the runner", "priority": "high", "status": "completed"},
                {"content": "Port the transport", "priority": "high", "status": "in_progress"},
            ],
        }));
        assert_eq!(view.title, "Todo list");
        assert_eq!(view.complete, Some(false));
        assert_eq!(view.source_id, Some(None));
        let presentation = view.presentation.expect("a plan presents a checklist");
        assert_eq!(presentation.kind, PresentationType::Todo);
        assert_eq!(presentation.total, Some(2));
        assert_eq!(presentation.completed, Some(1));
        assert_eq!(presentation.outcome.as_deref(), Some("Port the transport"));
    }

    #[test]
    fn an_opencode_todo_call_reads_as_the_checklist_after_its_title_changes() {
        let todos = json!([
            {"content": "Audit the tests", "status": "completed", "priority": "high"},
            {"content": "Run the suite", "status": "in_progress", "priority": "high"},
            {"content": "Open the PR", "status": "pending", "priority": "high"},
        ]);
        let view = view(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call_todo",
            "status": "completed",
            "title": "3 todos",
            "rawOutput": {"output": "[]", "metadata": {"todos": todos, "truncated": false}},
        }));
        assert_eq!(view.title, "Todo list");
        assert_eq!(view.complete, Some(true));
        assert_eq!(view.action_id.as_deref(), Some("call_todo"));
        let presentation = view.presentation.expect("a todo call presents a checklist");
        assert_eq!(presentation.kind, PresentationType::Todo);
        assert_eq!(presentation.total, Some(3));
        assert_eq!(presentation.completed, Some(1));
        assert_eq!(presentation.outcome.as_deref(), Some("Run the suite"));
    }

    #[test]
    fn a_context_reading_is_the_window_fill_and_never_a_token_bill() {
        let view = view(json!({
            "sessionUpdate": "usage_update",
            "used": 40_000,
            "size": 200_000,
            "cost": {"amount": 0.42, "currency": "USD"},
        }));
        assert_eq!(view.kind, EventKind::Usage);
        assert_eq!(view.title, CONTEXT_TITLE);
        assert_eq!(view.minor, Some(true));
        assert_eq!(view.detail.as_deref(), Some("20% used · $0.42 so far"));
        let presentation = view.presentation.expect("a reading presents usage");
        assert_eq!(presentation.tokens_in, Some(40_000));
        assert_eq!(presentation.tokens_out, None);
        assert_eq!(presentation.cost_usd, Some(0.42));
    }

    #[test]
    fn a_cost_in_another_currency_stays_unknown() {
        let payload: BTreeMap<String, Value> = json!({"cost": {"amount": 3.0, "currency": "EUR"}})
            .as_object()
            .expect("an object")
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        assert_eq!(session_cost(&payload), None);
    }

    /// Recorded from Antigravity's ACP probe: the call names the file it is
    /// about, and its completion says only that it finished.
    #[test]
    fn a_completion_that_says_only_that_it_finished_keeps_the_call_it_settles() {
        let views = crate::event_views(
            vec![
                acp_event(
                    1,
                    json!({
                        "sessionUpdate": "tool_call",
                        "toolCallId": "session:2",
                        "title": "Running edit_file",
                        "kind": "edit",
                        "status": "in_progress",
                        "locations": [{"path": "/repo/examples/event-sample.txt"}],
                        "rawInput": {"file_path": "/repo/examples/event-sample.txt"},
                    }),
                ),
                acp_event(
                    2,
                    json!({
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": "session:2",
                        "status": "completed",
                        "rawOutput": "Create event sample file",
                    }),
                ),
            ],
            Provider::Antigravity,
        );

        let settled = views.last().expect("the completion");
        assert_eq!(settled.title, "Edit file");
        assert_eq!(settled.kind, EventKind::File);
        assert_eq!(settled.phase, EventPhase::Completed);
        assert_eq!(
            settled
                .presentation
                .as_ref()
                .and_then(|value| value.path.as_deref()),
            Some("/repo/examples/event-sample.txt"),
            "a completion with no fields of its own still names the file"
        );
    }

    #[test]
    fn session_bookkeeping_stays_out_of_the_story() {
        for update in [
            json!({"sessionUpdate": "available_commands_update", "availableCommands": []}),
            json!({"sessionUpdate": "session_info_update", "title": "Port the runner"}),
        ] {
            let view = view(update);
            assert_eq!(view.minor, Some(true));
            assert_eq!(view.kind, EventKind::Lifecycle);
            assert_eq!(
                view.phase,
                EventPhase::Info,
                "{} reads as work in flight",
                view.title
            );
            assert!(
                !view.title.contains(['.', '_']),
                "{} leaked its wire name",
                view.title
            );
        }
    }

    /// Recorded from an fx run whose provider was down: ten identical session
    /// updates, each carrying the attempt it was on.
    #[test]
    fn a_failing_provider_reads_as_one_row_that_counts_its_attempts() {
        let retrying = view(json!({
            "sessionUpdate": "session_info_update",
            "_meta": {"fx": {"modelResponseRecovery": {
                "state": "active",
                "kind": "auto_retry",
                "cause": "provider_unavailable",
                "attempt": 1,
                "attemptLimit": 10,
                "message": "⚠ Provider unavailable · HTTP 503 · service_unavailable_error: Service temporarily unavailable. Please try again shortly. · retrying request · attempt 1/10",
            }}},
        }));

        assert_eq!(retrying.title, "Provider unavailable");
        assert_eq!(retrying.kind, EventKind::Lifecycle);
        assert_eq!(retrying.phase, EventPhase::Info);
        assert_ne!(retrying.minor, Some(true), "a reader has to see this");
        assert_eq!(
            retrying.detail.as_deref(),
            Some("Provider unavailable · retrying, attempt 1 of 10")
        );
        assert_eq!(
            retrying.result, None,
            "an attempt in flight says all it needs in one line"
        );
        assert_eq!(retrying.action_id.as_deref(), Some(RECOVERY_ACTION_ID));
        assert_eq!(
            retrying.source_id,
            Some(Some(RECOVERY_ACTION_ID.to_owned())),
            "every attempt patches the row the reader is already on"
        );
    }

    #[test]
    fn a_provider_that_gave_up_names_the_limit_and_reads_out_its_reason() {
        let paused = view(json!({
            "sessionUpdate": "session_info_update",
            "_meta": {"fx": {"modelResponseRecovery": {
                "state": "paused",
                "kind": "terminal_provider_error",
                "cause": "rate_limited",
                "requiredAction": "continue_later",
                "attempt": 10,
                "attemptLimit": 10,
                "message": "⚠ Rate limited · HTTP 429 · rate_limit_exceeded: Free tier requests on this model are rate-limited. · recovery paused after 10/10 attempts",
            }}},
        }));

        assert_eq!(paused.title, "Rate limited");
        assert_eq!(paused.phase, EventPhase::Failed);
        assert_eq!(
            paused.detail.as_deref(),
            Some("Rate limited · stopped after 10 attempts")
        );
        assert_eq!(
            paused.result.as_deref(),
            Some(
                "Rate limited · HTTP 429 · rate_limit_exceeded: Free tier requests on this model are rate-limited. · recovery paused a"
            ),
            "the agent's own reason stays readable without leading the row"
        );
        assert_eq!(paused.action_id.as_deref(), Some(RECOVERY_ACTION_ID));
    }

    #[test]
    fn a_recovery_shape_this_does_not_know_still_reads_as_a_row() {
        let bare = view(json!({
            "sessionUpdate": "session_info_update",
            "_meta": {"fx": {"modelResponseRecovery": {"state": "active"}}},
        }));
        assert_eq!(bare.title, "Provider error");
        assert_eq!(bare.detail.as_deref(), Some("Provider error · retrying"));

        for meta in [
            json!({"claudeCode": {"toolName": "Bash"}}),
            json!({"fx": {"somethingElse": true}}),
            json!("not an object"),
        ] {
            assert_eq!(
                ModelRecovery::from_meta(&meta),
                None,
                "{meta} is not a recovery run"
            );
        }
    }

    #[test]
    fn an_update_this_does_not_know_still_names_itself() {
        let view = view(json!({"sessionUpdate": "current_mode_update", "currentModeId": "ask"}));
        assert!(
            !view.title.contains(['.', '_']),
            "{} leaked its wire name",
            view.title
        );
    }
}
