//! Instructions left on a stopped task for whichever run starts it next.
//! They live in the event log, so an older binary reads the same database.

use oga_domain::TaskState;
use oga_store::{Store, StoreError, append_event};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const SAVED: &str = "instruction_saved";
const SENT: &str = "saved_instructions_sent";
const DROPPED: &str = "saved_instructions_dropped";

/// Saves `instruction` for the next run and returns how many now wait.
pub fn save(
    store: &Store,
    task_id: &str,
    state: TaskState,
    instruction: &str,
    now: &str,
) -> Result<usize, StoreError> {
    store.transaction(|tx| {
        append_event(
            tx,
            task_id,
            SAVED,
            state,
            &json!({"instruction": instruction}),
            now,
            None,
        )?;
        tx.execute(
            "UPDATE tasks SET updated_at=? WHERE id=?",
            params![now, task_id],
        )?;
        Ok(waiting_in(tx, task_id)?.len())
    })
}

/// The saved instructions not yet sent, oldest first.
pub fn waiting(store: &Store, task_id: &str) -> Result<Vec<String>, StoreError> {
    store.with_connection(|connection| Ok(waiting_in(connection, task_id)?))
}

/// Drops every saved instruction still waiting and returns how many there were.
pub fn clear(store: &Store, task_id: &str, state: TaskState) -> Result<usize, StoreError> {
    let now = crate::lifecycle::now_iso();
    store.transaction(|tx| {
        let count = waiting_in(tx, task_id)?.len();
        if count > 0 {
            append_event(
                tx,
                task_id,
                DROPPED,
                state,
                &json!({"count": count, "reason": "removed on request"}),
                &now,
                None,
            )?;
        }
        Ok(count)
    })
}

pub(crate) fn mark_sent(
    connection: &Connection,
    task_id: &str,
    state: TaskState,
    count: usize,
    now: &str,
) -> rusqlite::Result<()> {
    if count == 0 {
        return Ok(());
    }
    append_event(
        connection,
        task_id,
        SENT,
        state,
        &json!({"count": count}),
        now,
        None,
    )?;
    Ok(())
}

/// The run's own instruction followed by what was saved for it.
pub(crate) fn with_saved(instruction: Option<String>, saved: &[String]) -> Option<String> {
    let parts = instruction
        .into_iter()
        .chain(saved.iter().cloned())
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn waiting_in(connection: &Connection, task_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT payload FROM task_events WHERE task_id=?1 AND event_type=?2 AND id > COALESCE((SELECT MAX(id) FROM task_events WHERE task_id=?1 AND event_type IN (?3,?4)),0) ORDER BY id",
    )?;
    let payloads = statement
        .query_map(params![task_id, SAVED, SENT, DROPPED], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(payloads
        .iter()
        .filter_map(|payload| serde_json::from_str::<Value>(payload).ok())
        .filter_map(|payload| payload["instruction"].as_str().map(str::to_owned))
        .collect())
}
