//! Durable consumer cursors and live event-feed deliveries.

use std::{
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use oga_domain::{ConsumerCursor, ConsumerDelivery, DeliveryStatus, EventKind, EventPointer};
use oga_store::{Store, StoreError};
use rusqlite::{OptionalExtension, ToSql, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    holds::{Clock, SystemClock},
    lifecycle,
};

pub const MAX_UNSEEN_DELIVERIES: i64 = 10_000;
pub const CONSUMER_REBASELINE_LAG: i64 = MAX_UNSEEN_DELIVERIES * 2;
pub const CONSUMER_BACKLOG_REBASELINE: i64 = 300;
pub const SEEN_DELIVERY_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const REDELIVER_WINDOW: Duration = Duration::from_secs(60);
pub const MAX_RENDERS: i64 = 3;
pub const MAX_INBOX_LIMIT: i64 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rebaselined {
    pub dropped: i64,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryInbox {
    pub cursor: ConsumerCursor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebaselined: Option<Rebaselined>,
    pub deliveries: Vec<ConsumerDelivery>,
}

pub struct DeliveryService<C = SystemClock> {
    store: Arc<Store>,
    clock: C,
}

impl DeliveryService<SystemClock> {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            clock: SystemClock,
        }
    }
}

impl<C: Clock> DeliveryService<C> {
    pub fn with_clock(store: Arc<Store>, clock: C) -> Self {
        Self { store, clock }
    }

    pub fn register(&self, consumer_id: &str) -> Result<ConsumerCursor, StoreError> {
        register_consumer(&self.store, consumer_id, &self.now())
    }

    pub fn read(
        &self,
        consumer_id: &str,
        channel: &str,
        limit: i64,
        scope_cwd: Option<&str>,
    ) -> Result<DeliveryInbox, StoreError> {
        read_inbox(
            &self.store,
            consumer_id,
            channel,
            limit,
            scope_cwd,
            &self.clock,
        )
    }

    pub fn advance(&self, consumer_id: &str, cursor: i64) -> Result<ConsumerCursor, StoreError> {
        advance_cursor(&self.store, consumer_id, cursor, &self.now())
    }

    pub fn remember_scope(&self, consumer_id: &str, cwd: &str) -> Result<(), StoreError> {
        remember_consumer_scope(&self.store, consumer_id, cwd, &self.now())
    }

    pub fn retire_sent(
        &self,
        consumer_id: &str,
        channel: &str,
        max_renders: i64,
    ) -> Result<usize, StoreError> {
        let now = self.clock.now();
        let sent_before = lifecycle::iso_from_system_time(
            now.checked_sub(REDELIVER_WINDOW).unwrap_or(UNIX_EPOCH),
        );
        retire_sent_deliveries(
            &self.store,
            consumer_id,
            channel,
            &sent_before,
            max_renders,
            &lifecycle::iso_from_system_time(now),
        )
    }

    fn now(&self) -> String {
        lifecycle::iso_from_system_time(self.clock.now())
    }
}

pub fn register_consumer(
    store: &Store,
    consumer_id: &str,
    now: &str,
) -> Result<ConsumerCursor, StoreError> {
    store.transaction(|tx| {
        tx.execute(
            "INSERT OR IGNORE INTO consumer_cursors(consumer_id,cursor,updated_at) VALUES(?,0,?)",
            params![consumer_id, now],
        )?;
        Ok(())
    })?;
    get_cursor(store, consumer_id)?.ok_or_else(|| {
        StoreError::Refusal(format!(
            "consumer disappeared after registration: {consumer_id}"
        ))
    })
}

pub fn get_cursor(store: &Store, consumer_id: &str) -> Result<Option<ConsumerCursor>, StoreError> {
    store.with_connection(|connection| {
        connection
            .query_row(
                "SELECT consumer_id,cursor,updated_at FROM consumer_cursors WHERE consumer_id=?",
                [consumer_id],
                |row| {
                    Ok(ConsumerCursor {
                        consumer_id: row.get(0)?,
                        cursor: row.get(1)?,
                        updated_at: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    })
}

pub fn remember_consumer_scope(
    store: &Store,
    consumer_id: &str,
    cwd: &str,
    now: &str,
) -> Result<(), StoreError> {
    register_consumer(store, consumer_id, now)?;
    store.transaction(|tx| {
        tx.execute(
            "UPDATE consumer_cursors SET scope_cwd=? WHERE consumer_id=?",
            params![cwd, consumer_id],
        )?;
        Ok(())
    })
}

pub fn consumer_scope(store: &Store, consumer_id: &str) -> Result<Option<String>, StoreError> {
    store.with_connection(|connection| {
        let scope = connection
            .query_row(
                "SELECT scope_cwd FROM consumer_cursors WHERE consumer_id=?",
                [consumer_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(scope.flatten())
    })
}

pub fn advance_cursor(
    store: &Store,
    consumer_id: &str,
    cursor: i64,
    now: &str,
) -> Result<ConsumerCursor, StoreError> {
    if cursor < 0 {
        return Err(StoreError::Refusal("cursor must not be negative".into()));
    }
    register_consumer(store, consumer_id, now)?;
    store.transaction(|tx| {
        tx.execute(
            "UPDATE consumer_cursors SET cursor=MAX(cursor,?),updated_at=CASE WHEN ? > cursor THEN ? ELSE updated_at END WHERE consumer_id=?",
            params![cursor, cursor, now, consumer_id],
        )?;
        Ok(())
    })?;
    get_cursor(store, consumer_id)?.ok_or_else(|| {
        StoreError::Refusal(format!(
            "consumer disappeared after cursor advance: {consumer_id}"
        ))
    })
}

pub fn read_inbox<C: Clock>(
    store: &Store,
    consumer_id: &str,
    channel: &str,
    limit: i64,
    scope_cwd: Option<&str>,
    clock: &C,
) -> Result<DeliveryInbox, StoreError> {
    let limit = limit.clamp(1, MAX_INBOX_LIMIT);
    let now = clock.now();
    let now_iso = lifecycle::iso_from_system_time(now);
    let scope_cwd = match scope_cwd {
        Some(cwd) => Some(cwd.to_owned()),
        None => consumer_scope(store, consumer_id)?,
    };
    let registered = get_cursor(store, consumer_id)?;
    let head = latest_event_id(store)?;
    let mut rebaselined = None;
    if registered.is_none()
        || registered
            .as_ref()
            .is_some_and(|cursor| head - cursor.cursor > CONSUMER_REBASELINE_LAG)
        || consumer_backlog_depth(store, consumer_id, channel)? > CONSUMER_BACKLOG_REBASELINE
    {
        let dropped = baseline_consumer(
            store,
            consumer_id,
            channel,
            scope_cwd.as_deref(),
            &now_iso,
            head,
        )?;
        if registered.is_some() {
            rebaselined = Some(Rebaselined {
                dropped,
                at: now_iso.clone(),
            });
        }
    }
    let cursor = get_cursor(store, consumer_id)?.ok_or_else(|| {
        StoreError::Refusal(format!(
            "consumer disappeared before inbox read: {consumer_id}"
        ))
    })?;
    let events = attention_event_ids(
        store,
        cursor.cursor,
        consumer_id,
        channel,
        scope_cwd.as_deref(),
        MAX_UNSEEN_DELIVERIES + 1,
    )?;
    store.transaction(|tx| {
        for event_id in events {
            tx.execute(
                "INSERT OR IGNORE INTO deliveries(consumer_id,event_id,channel,status,attempts) VALUES(?,?,?,'pending',0)",
                params![consumer_id, event_id, channel],
            )?;
        }
        let seen_before = lifecycle::iso_from_system_time(
            now.checked_sub(SEEN_DELIVERY_RETENTION)
                .unwrap_or(UNIX_EPOCH),
        );
        tx.execute(
            "DELETE FROM deliveries WHERE consumer_id=? AND channel=? AND status='seen' AND seen_at < ?",
            params![consumer_id, channel, seen_before],
        )?;
        tx.execute(
            "UPDATE deliveries SET status='expired',last_error='delivery retention limit' WHERE consumer_id=? AND channel=? AND status IN ('pending','sent') AND event_id IN (SELECT event_id FROM deliveries WHERE consumer_id=? AND channel=? AND status IN ('pending','sent') ORDER BY event_id DESC LIMIT -1 OFFSET ?)",
            params![consumer_id, channel, consumer_id, channel, MAX_UNSEEN_DELIVERIES],
        )?;
        let pending_ids = {
            let mut statement = tx.prepare(
                "SELECT event_id FROM deliveries WHERE consumer_id=? AND channel=? AND status IN ('pending','sent') ORDER BY event_id DESC LIMIT ?",
            )?;
            statement
                .query_map(params![consumer_id, channel, limit], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for event_id in pending_ids {
            tx.execute(
                "UPDATE deliveries SET status='sent',attempts=attempts+1,sent_at=COALESCE(sent_at,?) WHERE consumer_id=? AND event_id=? AND channel=? AND status IN ('pending','sent')",
                params![now_iso, consumer_id, event_id, channel],
            )?;
        }
        Ok(())
    })?;
    Ok(DeliveryInbox {
        cursor: get_cursor(store, consumer_id)?.expect("registered cursor exists"),
        rebaselined,
        deliveries: list_deliveries(store, consumer_id, channel, limit, None)?,
    })
}

pub fn baseline_consumer(
    store: &Store,
    consumer_id: &str,
    channel: &str,
    scope_cwd: Option<&str>,
    now: &str,
    head: i64,
) -> Result<i64, StoreError> {
    register_consumer(store, consumer_id, now)?;
    let waiting = open_question_event_ids(store, consumer_id, scope_cwd)?;
    store.transaction(|tx| {
        tx.execute(
            "UPDATE consumer_cursors SET cursor=?,updated_at=? WHERE consumer_id=?",
            params![head, now, consumer_id],
        )?;
        let dropped = tx.execute(
            "UPDATE deliveries SET status='expired',last_error='consumer baseline reset' WHERE consumer_id=? AND channel=? AND status IN ('pending','sent')",
            params![consumer_id, channel],
        )?;
        for event_id in waiting {
            tx.execute(
                "INSERT INTO deliveries(consumer_id,event_id,channel,status,attempts) VALUES(?,?,?,'pending',0) ON CONFLICT(consumer_id,event_id,channel) DO UPDATE SET status='pending',attempts=0,last_error=NULL,sent_at=NULL,seen_at=NULL",
                params![consumer_id, event_id, channel],
            )?;
        }
        Ok(dropped as i64)
    })
}

pub fn retire_sent_deliveries(
    store: &Store,
    consumer_id: &str,
    channel: &str,
    sent_before: &str,
    max_renders: i64,
    now: &str,
) -> Result<usize, StoreError> {
    let changed = store.transaction(|tx| {
        Ok(tx.execute(
            "UPDATE deliveries SET status='seen',seen_at=COALESCE(seen_at,?) WHERE consumer_id=? AND channel=? AND status='sent' AND (sent_at IS NULL OR sent_at < ? OR attempts >= ?)",
            params![now, consumer_id, channel, sent_before, max_renders],
        )?)
    })?;
    Ok(changed)
}

pub fn mark_deliveries_pending(
    store: &Store,
    consumer_id: &str,
    channel: &str,
    event_ids: &[i64],
) -> Result<usize, StoreError> {
    if event_ids.is_empty() {
        return Ok(0);
    }
    let placeholders = std::iter::repeat_n("?", event_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "UPDATE deliveries SET status='pending',sent_at=NULL,attempts=MAX(attempts-1,0) WHERE consumer_id=? AND channel=? AND event_id IN ({placeholders}) AND status='sent'"
    );
    let mut values: Vec<&dyn ToSql> = Vec::with_capacity(event_ids.len() + 2);
    values.push(&consumer_id);
    values.push(&channel);
    for event_id in event_ids {
        values.push(event_id);
    }
    store.transaction(|tx| Ok(tx.execute(&sql, params_from_iter(values))?))
}

fn latest_event_id(store: &Store) -> Result<i64, StoreError> {
    store.with_connection(|connection| {
        Ok(
            connection.query_row("SELECT COALESCE(MAX(id),0) FROM task_events", [], |row| {
                row.get(0)
            })?,
        )
    })
}

fn consumer_backlog_depth(
    store: &Store,
    consumer_id: &str,
    channel: &str,
) -> Result<i64, StoreError> {
    store.with_connection(|connection| {
        Ok(connection.query_row(
            "SELECT COUNT(*) FROM deliveries WHERE consumer_id=? AND channel=? AND status IN ('pending','sent')",
            params![consumer_id, channel],
            |row| row.get(0),
        )?)
    })
}

fn attention_event_ids(
    store: &Store,
    after_id: i64,
    consumer_id: &str,
    channel: &str,
    scope_cwd: Option<&str>,
    limit: i64,
) -> Result<Vec<i64>, StoreError> {
    store.with_connection(|connection| {
        let sql = if scope_cwd.is_some() {
            "SELECT te.id FROM task_events te JOIN tasks t ON t.id=te.task_id WHERE te.id>? AND (te.event_type IN ('needs_input','blocked','failed','completed','cancelled') OR te.state IN ('needs_input','blocked','failed','completed','cancelled')) AND NOT EXISTS (SELECT 1 FROM deliveries d WHERE d.consumer_id=? AND d.event_id=te.id AND d.channel=?) AND (t.caller_id=? OR (t.caller_id IS NULL AND t.cwd=?)) ORDER BY te.id LIMIT ?"
        } else {
            "SELECT te.id FROM task_events te JOIN tasks t ON t.id=te.task_id WHERE te.id>? AND (te.event_type IN ('needs_input','blocked','failed','completed','cancelled') OR te.state IN ('needs_input','blocked','failed','completed','cancelled')) AND NOT EXISTS (SELECT 1 FROM deliveries d WHERE d.consumer_id=? AND d.event_id=te.id AND d.channel=?) AND (t.caller_id=? OR t.caller_id IS NULL) ORDER BY te.id LIMIT ?"
        };
        let mut statement = connection.prepare(sql)?;
        if let Some(cwd) = scope_cwd {
            let values: Vec<&dyn ToSql> =
                vec![&after_id, &consumer_id, &channel, &consumer_id, &cwd, &limit];
            Ok(statement
                .query_map(params_from_iter(values), |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        } else {
            let values: Vec<&dyn ToSql> =
                vec![&after_id, &consumer_id, &channel, &consumer_id, &limit];
            Ok(statement
                .query_map(params_from_iter(values), |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        }
    })
}

fn open_question_event_ids(
    store: &Store,
    consumer_id: &str,
    scope_cwd: Option<&str>,
) -> Result<Vec<i64>, StoreError> {
    store.with_connection(|connection| {
        let sql = if scope_cwd.is_some() {
            "SELECT MAX(e.id) FROM task_events e JOIN tasks t ON t.id=e.task_id WHERE t.state='needs_input' AND t.archived_at IS NULL AND (e.event_type='needs_input' OR e.state='needs_input') AND (t.caller_id=? OR (t.caller_id IS NULL AND t.cwd=?)) GROUP BY e.task_id"
        } else {
            "SELECT MAX(e.id) FROM task_events e JOIN tasks t ON t.id=e.task_id WHERE t.state='needs_input' AND t.archived_at IS NULL AND (e.event_type='needs_input' OR e.state='needs_input') AND (t.caller_id=? OR t.caller_id IS NULL) GROUP BY e.task_id"
        };
        let mut statement = connection.prepare(sql)?;
        if let Some(cwd) = scope_cwd {
            let values: Vec<&dyn ToSql> = vec![&consumer_id, &cwd];
            Ok(statement
                .query_map(params_from_iter(values), |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        } else {
            let values: Vec<&dyn ToSql> = vec![&consumer_id];
            Ok(statement
                .query_map(params_from_iter(values), |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        }
    })
}

fn list_deliveries(
    store: &Store,
    consumer_id: &str,
    channel: &str,
    limit: i64,
    event_id: Option<i64>,
) -> Result<Vec<ConsumerDelivery>, StoreError> {
    store.with_connection(|connection| {
        let (event_filter, status_filter) = if event_id.is_some() {
            (" AND d.event_id=?", "")
        } else {
            ("", " AND d.status IN ('pending','sent')")
        };
        let sql = format!(
            "SELECT d.consumer_id,d.event_id,d.channel,d.status,d.attempts,d.last_error,d.sent_at,d.seen_at,e.event_type,e.state,e.payload,e.created_at,e.turn_id,t.title,t.id FROM deliveries d JOIN task_events e ON e.id=d.event_id JOIN tasks t ON t.id=e.task_id WHERE d.consumer_id=? AND d.channel=?{status_filter}{event_filter} ORDER BY d.event_id DESC LIMIT ?"
        );
        let mut statement = connection.prepare(&sql)?;
        let mut values: Vec<&dyn ToSql> = vec![&consumer_id, &channel];
        if let Some(event_id) = event_id.as_ref() {
            values.push(event_id);
        }
        values.push(&limit);
        let mut deliveries = statement
            .query_map(params_from_iter(values), delivery_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        deliveries.reverse();
        Ok(deliveries)
    })
}

fn delivery_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConsumerDelivery> {
    let event_type: String = row.get(8)?;
    let state_literal: String = row.get(9)?;
    let state = serde_json::from_str(&format!("\"{state_literal}\""))
        .map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))?;
    let payload: Value = serde_json::from_str(&row.get::<_, String>(10)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let task_id: String = row.get(14)?;
    let title = row
        .get::<_, Option<String>>(13)?
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| task_id.clone());
    let summary = payload
        .get("summary")
        .and_then(Value::as_str)
        .or_else(|| payload.get("text").and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if event_type == "agent.result" {
                "Run summary".into()
            } else {
                event_type.clone()
            }
        });
    let kind = event_kind(&event_type, &payload);
    let event = EventPointer {
        id: row.get(1)?,
        cursor: row.get(1)?,
        task_id,
        event_type,
        kind,
        state,
        at: row.get(11)?,
        title,
        summary,
        turn_id: row.get(12)?,
    };
    Ok(ConsumerDelivery {
        consumer_id: row.get(0)?,
        event_id: row.get(1)?,
        channel: row.get(2)?,
        status: delivery_status(&row.get::<_, String>(3)?),
        attempts: row.get(4)?,
        last_error: row.get(5)?,
        sent_at: row.get(6)?,
        seen_at: row.get(7)?,
        event,
    })
}

fn delivery_status(value: &str) -> DeliveryStatus {
    match value {
        "sent" => DeliveryStatus::Sent,
        "seen" => DeliveryStatus::Seen,
        "expired" => DeliveryStatus::Expired,
        _ => DeliveryStatus::Pending,
    }
}

fn event_kind(event_type: &str, payload: &Value) -> EventKind {
    let value = payload
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or(event_type);
    match value {
        "message" | "agent.message" => EventKind::Message,
        "reasoning" => EventKind::Reasoning,
        "tool" => EventKind::Tool,
        "command" => EventKind::Command,
        "file" => EventKind::File,
        "error" => EventKind::Error,
        "usage" | "agent.result" => EventKind::Usage,
        "retry" => EventKind::Retry,
        "raw" => EventKind::Raw,
        _ => EventKind::Lifecycle,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use oga_domain::{Profile, Provider, Task, TaskKind, TaskScope, TaskState};
    use tempfile::tempdir;

    use super::*;

    fn task(id: &str, cwd: &str) -> Task {
        Task {
            id: id.into(),
            kind: Some(TaskKind::Delegated),
            profile_id: "profile".into(),
            model: "model".into(),
            prompt: "work".into(),
            shipped_prompt: None,
            cwd: cwd.into(),
            branch: None,
            worktree: None,
            worktree_label: None,
            state: TaskState::Completed,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            output: String::new(),
            error: None,
            question: None,
            parent_task_id: None,
            orchestrator_id: None,
            scope: TaskScope::default(),
            grant_id: None,
            allow_questions: true,
            can_delegate: false,
            timeout_ms: None,
            effort: None,
            effort_actual: None,
            tldr: None,
            title: None,
            session_id: None,
            completion: None,
            attempts: vec![],
            cost_usd: None,
            cost_usd_estimated: false,
            turns: None,
            archived_at: None,
            queued_follow_ups: None,
            queued_follow_up_items: None,
            hold: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn delivery_cursor_reads_newest_first_but_returns_log_order_and_acks() {
        let directory = tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        store
            .repositories()
            .profiles()
            .insert(
                &Profile {
                    id: "profile".into(),
                    label: "profile".into(),
                    provider: Provider::Claude,
                    default_model: "model".into(),
                    enabled: true,
                    env: BTreeMap::new(),
                    capabilities: vec![],
                    command: None,
                },
                "2026-01-01T00:00:00.000Z",
            )
            .expect("profile");
        let cwd = directory.path().to_string_lossy().into_owned();
        store
            .repositories()
            .tasks()
            .insert(&task("task", &cwd))
            .expect("task");
        let now = "2026-01-01T00:00:00.000Z";
        store
            .repositories()
            .events()
            .append(&oga_domain::TaskEvent {
                id: 0,
                task_id: "task".into(),
                kind: "completed".into(),
                state: TaskState::Completed,
                payload: BTreeMap::new(),
                created_at: now.into(),
                turn_id: None,
            })
            .expect("event");
        let clock = crate::holds::FixedClock::at_unix_millis(1_735_689_600_000);
        let service = DeliveryService::with_clock(store.clone(), clock);
        assert!(
            service
                .read("client", "app", 10, Some(&cwd))
                .expect("baseline")
                .deliveries
                .is_empty()
        );
        store
            .repositories()
            .events()
            .append(&oga_domain::TaskEvent {
                id: 0,
                task_id: "task".into(),
                kind: "completed".into(),
                state: TaskState::Completed,
                payload: BTreeMap::new(),
                created_at: now.into(),
                turn_id: None,
            })
            .expect("new event");
        let inbox = service
            .read("client", "app", 10, Some(&cwd))
            .expect("inbox");
        assert_eq!(inbox.deliveries.len(), 1);
        assert_eq!(inbox.deliveries[0].status, DeliveryStatus::Sent);
    }
}
