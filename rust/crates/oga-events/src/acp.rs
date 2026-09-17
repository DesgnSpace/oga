//! ACP `session/update` notifications as ordinary activity rows.
//!
//! An agent that speaks ACP already describes its work the way a reader wants
//! it: a call has a kind, a status, a human title, the files it touched, and
//! the content it produced. So the mapping is a translation, not an
//! interpretation — the agent's own words become the row's subject, and its
//! tool-call id becomes the row identity that later updates patch in place.
//!
//! Everything a row cannot learn from the protocol stays empty. ACP's stable
//! surface reports the context window and a session total, never per-call
//! token counts, so a usage row says how full the window is and nothing more.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
};

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

/// The reader-facing title of an ACP context-window row.
const CONTEXT_TITLE: &str = "Context";

/// The longest subject a row shows before it is clipped.
const SUBJECT_LIMIT: usize = 120;

/// One ACP update as a row, or `None` when the update is not one this presents
/// and the generic view should name it.
pub(crate) fn acp_event_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> Option<TaskEventView> {
    match text_value(payload.get("sessionUpdate"))? {
        "agent_message_chunk" => Some(message_view(event, provider, payload, raw_text)),
        "agent_thought_chunk" => Some(reasoning_view(
            event,
            provider,
            chunk_text(payload),
            raw_text,
        )),
        // The person's own words, which the task already shows in full. On a
        // restored session the agent replays every one of them, so the row
        // stays out of the story and behind the technical toggle.
        "user_message_chunk" => Some(prompt_view(event, provider, payload, raw_text)),
        "tool_call" | "tool_call_update" => Some(tool_view(event, provider, payload, raw_text)),
        "plan" => Some(plan_view(event, provider, payload, raw_text)),
        "usage_update" => Some(context_view(event, provider, payload, raw_text)),
        // Session bookkeeping: the agent listing what it can run, and restating
        // the session's own title. Neither is work, and both arrive around the
        // agent's closing words, where an ordinary row would compete with them.
        "available_commands_update" => Some(bookkeeping_view(
            event,
            provider,
            "Commands listed",
            raw_text,
        )),
        "session_info_update" => Some(bookkeeping_view(
            event,
            provider,
            "Session updated",
            raw_text,
        )),
        _ => None,
    }
}

/// A row the record keeps and the story leaves out.
fn bookkeeping_view(
    event: &TaskEvent,
    provider: Provider,
    title: &str,
    raw_text: Option<String>,
) -> TaskEventView {
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
            raw_text,
        },
    )
}

/// The text of a content chunk, when it is text at all. An image or an
/// embedded resource has nothing a row can read out.
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
    raw_text: Option<String>,
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
            raw_text,
        },
    );
    view.verb = Some("Said".to_owned());
    view.complete = Some(true);
    view
}

fn prompt_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
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
            raw_text,
        },
    )
}

/// What kind of subject a call's row is about.
#[derive(PartialEq)]
enum Subject {
    File,
    Command,
    Other,
}

/// What a tool call is called and how it reads, chosen from the only
/// classification ACP guarantees: the call's kind.
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

/// One tool call, whether this row opened it or updated it.
///
/// A call keeps its id from the first row to the last, so an update patches
/// the row the reader is already looking at instead of appending a near
/// duplicate — and a session replay of a finished call folds back into it.
fn tool_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
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
        let mut view = reasoning_view(event, provider, subject, raw_text);
        view.action_id = tool_call_id(payload);
        view.source_id = Some(tool_call_id(payload));
        view.complete = Some(complete);
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
            raw_text,
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

/// The fields that say what a tool call is and where it stands, as opposed to
/// what it produced.
const CALL_FIELDS: [&str; 5] = ["kind", "status", "title", "locations", "rawInput"];

/// The tool calls of one task, as far as its recorded updates have described
/// them.
///
/// ACP sends only what changed about a call, so an update that leaves out the
/// call's kind or files means they stand as they were. Reading the updates in
/// order lets each one be presented as the call it patched, while its output
/// stays its own and never repeats on a later row.
#[derive(Default)]
pub(crate) struct AcpCalls {
    known: HashMap<(Option<i64>, String), Map<String, Value>>,
}

impl AcpCalls {
    /// The event with the call's earlier fields filled in where it left them
    /// out, or the event itself when it needs nothing. A call is known only
    /// within its own turn, and a fresh `tool_call` describes it anew.
    pub(crate) fn patch<'a>(&mut self, event: &'a TaskEvent) -> Cow<'a, TaskEvent> {
        let update = text_value(event.payload.get("sessionUpdate"));
        if !matches!(update, Some("tool_call" | "tool_call_update")) {
            return Cow::Borrowed(event);
        }
        let Some(id) = tool_call_id(&event.payload) else {
            return Cow::Borrowed(event);
        };
        let known = self.known.entry((event.turn_id, id)).or_default();
        if update == Some("tool_call") {
            known.clear();
        }
        let carried: Vec<(String, Value)> = CALL_FIELDS
            .into_iter()
            .filter(|field| !event.payload.contains_key(*field))
            .filter_map(|field| Some((field.to_owned(), known.get(field)?.clone())))
            .collect();
        for field in CALL_FIELDS {
            if let Some(value) = event.payload.get(field) {
                known.insert(field.to_owned(), value.clone());
            }
        }
        if carried.is_empty() {
            return Cow::Borrowed(event);
        }
        let mut patched = event.clone();
        patched.payload.extend(carried);
        Cow::Owned(patched)
    }
}

/// What the call was about, read from the facts ACP publishes in its own
/// fields first and the provider's raw input only where those run out.
///
/// A category with no subject of its own falls back to a plain tool row
/// carrying the agent's title: a row that names nothing is worse than a row
/// that names the call the way the agent did.
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

/// The first file the call named, which is where a reader following along
/// wants to be.
fn location_path(payload: &BTreeMap<String, Value>) -> Option<String> {
    let locations = payload.get("locations")?.as_array()?;
    let first = locations.first()?;
    let path = text_value(first.get("path"))?;
    match number_u64(first.get("line")) {
        Some(line) => Some(format!("{path}:{line}")),
        None => Some(path.to_owned()),
    }
}

/// The diff a call produced, when it produced one. A diff is the strongest
/// thing a row can say about an edit, so it wins over every other content.
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

/// What the call produced, from the first text content it reported.
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

/// The agent's plan for the turn, as the same checklist every provider's todo
/// list renders as. A plan replaces the one before it, and the agent gives it
/// no id, so the row matches the newest open plan by title.
fn plan_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
) -> TaskEventView {
    let entries = payload
        .get("entries")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
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
    let done = total > 0 && completed == total;
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
            raw_text,
        },
    );
    view.verb = Some(if done { "Updated" } else { "Updating" }.to_owned());
    view.target = presentation.text.clone();
    view.result = presentation.outcome.clone();
    view.complete = Some(done);
    view.source_id = Some(None);
    view
}

/// How full the agent says its context window is, plus what the session has
/// cost so far.
///
/// Both numbers are running totals for the whole session, not this call's
/// share, so the row carries them as the freshest reading and nothing is
/// added up across rows. The window fill is what the footer reads.
fn context_view(
    event: &TaskEvent,
    provider: Provider,
    payload: &BTreeMap<String, Value>,
    raw_text: Option<String>,
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
            raw_text,
        },
    )
}

/// The session total in dollars. An agent billing in another currency is left
/// unknown rather than converted at a rate Oga would have to invent.
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
            &[
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
