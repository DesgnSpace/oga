//! Which provider recorded each event of a task.
//!
//! A handoff moves a task to another profile, so its transcript can hold
//! events from several providers. Each event is read with the provider that
//! recorded it, which the task's handoffs settle: events before a handoff come
//! from the profile it left, events after the last one from the task's
//! current profile.

use std::collections::HashMap;

use oga_domain::{Provider, TaskEvent};
use rusqlite::{Connection, params};
use serde_json::{Map, Value};

/// A point where the task changed profile. The row that recorded it is the
/// first one read as the new profile's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderBoundary {
    pub event_id: i64,
    /// The provider of the profile the task left, when that profile still exists.
    pub from: Option<Provider>,
}

#[derive(Debug, Clone)]
pub struct ProviderTimeline {
    current: Provider,
    /// Ascending by event id. Boundaries before the first event read are not
    /// needed, so a reader may hold only the later ones.
    boundaries: Vec<ProviderBoundary>,
}

impl ProviderTimeline {
    /// A task that never changed provider.
    pub fn single(provider: Provider) -> Self {
        Self {
            current: provider,
            boundaries: Vec::new(),
        }
    }

    pub fn new(current: Provider, mut boundaries: Vec<ProviderBoundary>) -> Self {
        boundaries.sort_by_key(|boundary| boundary.event_id);
        Self {
            current,
            boundaries,
        }
    }

    /// The stretch of the transcript an event sits in. It changes at each
    /// handoff, which is where per-provider reading state starts over.
    pub fn segment(&self, event_id: i64) -> usize {
        self.boundaries
            .partition_point(|boundary| boundary.event_id <= event_id)
    }

    pub fn provider_for(&self, event: &TaskEvent) -> Provider {
        match self.boundaries.get(self.segment(event.id)) {
            Some(next) => next
                .from
                .or_else(|| provider_from_shape(event))
                .unwrap_or(self.current),
            None => self.current,
        }
    }
}

/// The provider an event's payload can only have come from, for events whose
/// profile is gone. An ACP update reads the same under every ACP agent, so it
/// names none.
fn provider_from_shape(event: &TaskEvent) -> Option<Provider> {
    let payload = &event.payload;
    if payload.contains_key("sessionUpdate") {
        let meta = payload.get("_meta")?;
        return if meta.get("claudeCode").is_some() {
            Some(Provider::Claude)
        } else {
            meta.get("opencode/child-session")
                .map(|_| Provider::OpenCode2)
        };
    }
    let event_type = payload.get("type").and_then(Value::as_str)?;
    match event_type {
        "thread.started" | "turn.completed" | "turn.failed" => Some(Provider::Codex),
        kind if kind.starts_with("item.") => Some(Provider::Codex),
        "rate_limit_event" => Some(Provider::Claude),
        "system" if payload.contains_key("subtype") => Some(Provider::Claude),
        "assistant" if payload.get("message").is_some_and(Value::is_object) => {
            Some(Provider::Claude)
        }
        "session"
        | "message_start"
        | "message_update"
        | "message_end"
        | "turn_start"
        | "turn_end"
        | "agent_start"
        | "agent_end"
        | "agent_settled"
        | "auto_retry_start"
        | "auto_retry_end"
        | "tool_execution_start"
        | "tool_execution_update"
        | "tool_execution_end" => Some(Provider::Pi),
        "step_start" | "step_finish" | "text" | "reasoning" | "tool_use" => {
            Some(Provider::OpenCode)
        }
        _ => None,
    }
}

/// The task's handoffs at or after `since_event_id`, with `current` as the
/// provider of the profile it runs on now. A reader of events from that id on
/// needs no earlier boundary, so a page of recent events reads only the
/// handoffs after it.
pub fn load_provider_timeline(
    connection: &Connection,
    task_id: &str,
    current: Provider,
    since_event_id: i64,
) -> rusqlite::Result<ProviderTimeline> {
    // A profile that was edited in place records the move the same way a
    // handoff does, so both count.
    let mut statement = connection.prepare(
        "SELECT e.id,e.payload,p.provider FROM task_events e \
         LEFT JOIN profiles p ON p.id=json_extract(e.payload,'$.fromProfile') \
         WHERE e.task_id=? AND e.id>=? AND e.event_type IN ('handed_off','edited') \
         ORDER BY e.id",
    )?;
    let boundaries = statement
        .query_map(params![task_id, since_event_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .filter_map(|row| match row {
            Ok((event_id, payload, provider)) => {
                let payload: Map<String, Value> = serde_json::from_str(&payload).ok()?;
                // An edit names the profiles only when it changed them.
                payload.get("fromProfile").and_then(Value::as_str)?;
                payload.get("toProfile").and_then(Value::as_str)?;
                Some(Ok(ProviderBoundary {
                    event_id,
                    from: provider
                        .and_then(|value| serde_json::from_value(Value::String(value)).ok()),
                }))
            }
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ProviderTimeline::new(current, boundaries))
}

/// The lowest event id each task has among `events`, which is how far back
/// that task's timeline has to reach.
pub fn first_event_ids(events: &[TaskEvent]) -> HashMap<&str, i64> {
    let mut first = HashMap::new();
    for event in events {
        let lowest = first.entry(event.task_id.as_str()).or_insert(event.id);
        *lowest = (*lowest).min(event.id);
    }
    first
}
