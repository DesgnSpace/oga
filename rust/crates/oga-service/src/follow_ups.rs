//! FIFO follow-up instructions that survive a broker restart.

use std::sync::Arc;

use oga_domain::TaskState;
use oga_store::{Store, StoreError};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::json;

use crate::{
    holds::{Clock, SystemClock},
    lifecycle,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedFollowUp {
    pub id: i64,
    pub instruction: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FollowUpFeed {
    pub instruction: Option<String>,
    pub waiting: usize,
    pub paused: bool,
    pub dropped: usize,
}

pub struct FollowUpQueue<C = SystemClock> {
    store: Arc<Store>,
    clock: C,
}

pub type FollowUpService<C = SystemClock> = FollowUpQueue<C>;

impl FollowUpQueue<SystemClock> {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            clock: SystemClock,
        }
    }
}

impl<C: Clock> FollowUpQueue<C> {
    pub fn with_clock(store: Arc<Store>, clock: C) -> Self {
        Self { store, clock }
    }

    pub fn queue(
        &self,
        task_id: &str,
        state: TaskState,
        instruction: &str,
    ) -> Result<usize, StoreError> {
        let now = lifecycle::iso_from_system_time(self.clock.now());
        queue_follow_up(&self.store, task_id, state, instruction, &now)
    }

    pub fn count(&self, task_id: &str) -> Result<usize, StoreError> {
        count_follow_ups(&self.store, task_id)
    }

    pub fn list(&self, task_id: &str) -> Result<Vec<QueuedFollowUp>, StoreError> {
        list_follow_ups(&self.store, task_id)
    }

    pub fn take_next(&self, task_id: &str) -> Result<Option<QueuedFollowUp>, StoreError> {
        take_next_follow_up(&self.store, task_id)
    }

    pub fn remove_at(
        &self,
        task_id: &str,
        state: TaskState,
        index: usize,
    ) -> Result<bool, StoreError> {
        let now = lifecycle::iso_from_system_time(self.clock.now());
        remove_follow_up_at(&self.store, task_id, state, index, &now)
    }

    pub fn clear(
        &self,
        task_id: &str,
        state: TaskState,
        reason: &str,
    ) -> Result<usize, StoreError> {
        let now = lifecycle::iso_from_system_time(self.clock.now());
        clear_follow_ups(&self.store, task_id, state, reason, &now)
    }

    pub fn feed(&self, task_id: &str, state: TaskState) -> Result<FollowUpFeed, StoreError> {
        let now = lifecycle::iso_from_system_time(self.clock.now());
        feed_follow_up(&self.store, task_id, state, &now)
    }
}

pub fn queue_follow_up(
    store: &Store,
    task_id: &str,
    state: TaskState,
    instruction: &str,
    now: &str,
) -> Result<usize, StoreError> {
    let instruction = instruction.trim();
    if instruction.is_empty() {
        return Err(StoreError::Refusal(
            "a queued follow-up needs an instruction".into(),
        ));
    }
    if !matches!(
        state,
        TaskState::Queued | TaskState::Pending | TaskState::Running | TaskState::Answered
    ) {
        return Err(StoreError::Refusal(format!(
            "follow-up cannot be queued from state {}",
            state.as_str()
        )));
    }
    store.transaction(|tx| {
        tx.execute(
            "INSERT INTO task_follow_ups(task_id,instruction,created_at) VALUES(?,?,?)",
            params![task_id, instruction, now],
        )?;
        tx.execute(
            "UPDATE tasks SET updated_at=? WHERE id=?",
            params![now, task_id],
        )?;
        let waiting: i64 = tx.query_row(
            "SELECT COUNT(*) FROM task_follow_ups WHERE task_id=?",
            [task_id],
            |row| row.get(0),
        )?;
        append_event(
            tx,
            task_id,
            "follow_up_queued",
            state,
            json!({"instruction": instruction, "waiting": waiting}),
            now,
        )?;
        Ok(waiting as usize)
    })
}

pub fn count_follow_ups(store: &Store, task_id: &str) -> Result<usize, StoreError> {
    store.with_connection(|connection| {
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM task_follow_ups WHERE task_id=?",
            [task_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    })
}

pub fn list_follow_ups(store: &Store, task_id: &str) -> Result<Vec<QueuedFollowUp>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT id,instruction,created_at FROM task_follow_ups WHERE task_id=? ORDER BY id",
        )?;
        Ok(statement
            .query_map([task_id], |row| {
                Ok(QueuedFollowUp {
                    id: row.get(0)?,
                    instruction: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    })
}

pub fn take_next_follow_up(
    store: &Store,
    task_id: &str,
) -> Result<Option<QueuedFollowUp>, StoreError> {
    store.transaction(|tx| {
        let row = tx
            .query_row(
                "SELECT id,instruction,created_at FROM task_follow_ups WHERE task_id=? ORDER BY id LIMIT 1",
                [task_id],
                |row| {
                    Ok(QueuedFollowUp {
                        id: row.get(0)?,
                        instruction: row.get(1)?,
                        created_at: row.get(2)?,
                    })
                },
            )
            .optional()?;
        let Some(row) = row else {
            return Ok(None);
        };
        let changed = tx.execute("DELETE FROM task_follow_ups WHERE id=?", [row.id])?;
        Ok((changed == 1).then_some(row))
    })
}

pub fn take_next_instruction(store: &Store, task_id: &str) -> Result<Option<String>, StoreError> {
    Ok(take_next_follow_up(store, task_id)?.map(|follow_up| follow_up.instruction))
}

pub fn remove_follow_up_at(
    store: &Store,
    task_id: &str,
    state: TaskState,
    index: usize,
    now: &str,
) -> Result<bool, StoreError> {
    store.transaction(|tx| {
        let ids = {
            let mut statement =
                tx.prepare("SELECT id FROM task_follow_ups WHERE task_id=? ORDER BY id")?;
            statement
                .query_map([task_id], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let Some(id) = ids.get(index) else {
            return Ok(false);
        };
        if tx.execute("DELETE FROM task_follow_ups WHERE id=?", [id])? != 1 {
            return Ok(false);
        }
        tx.execute(
            "UPDATE tasks SET updated_at=? WHERE id=?",
            params![now, task_id],
        )?;
        append_event(
            tx,
            task_id,
            "follow_ups_dropped",
            state,
            json!({"dropped": 1, "reason": "removed by user"}),
            now,
        )?;
        Ok(true)
    })
}

pub fn clear_follow_ups(
    store: &Store,
    task_id: &str,
    state: TaskState,
    reason: &str,
    now: &str,
) -> Result<usize, StoreError> {
    store.transaction(|tx| {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM task_follow_ups WHERE task_id=?",
            [task_id],
            |row| row.get(0),
        )?;
        if count == 0 {
            return Ok(0);
        }
        tx.execute("DELETE FROM task_follow_ups WHERE task_id=?", [task_id])?;
        tx.execute(
            "UPDATE tasks SET updated_at=? WHERE id=?",
            params![now, task_id],
        )?;
        append_event(
            tx,
            task_id,
            "follow_ups_dropped",
            state,
            json!({"dropped": count, "reason": reason}),
            now,
        )?;
        Ok(count as usize)
    })
}

/// Cancelling a task abandons the instructions queued behind it. Every other
/// ending keeps them, so a resume can still send what was waiting.
pub fn settling_clears_follow_ups(state: TaskState) -> bool {
    state == TaskState::Cancelled
}

pub fn feed_follow_up(
    store: &Store,
    task_id: &str,
    state: TaskState,
    now: &str,
) -> Result<FollowUpFeed, StoreError> {
    if settling_clears_follow_ups(state) {
        let dropped = clear_follow_ups(store, task_id, state, "the task was cancelled", now)?;
        return Ok(FollowUpFeed {
            dropped,
            ..FollowUpFeed::default()
        });
    }
    let waiting = count_follow_ups(store, task_id)?;
    if state != TaskState::Completed {
        if waiting > 0 {
            store.transaction(|tx| {
                append_event(
                    tx,
                    task_id,
                    "follow_ups_paused",
                    state,
                    json!({"waiting": waiting}),
                    now,
                )?;
                Ok(())
            })?;
        }
        return Ok(FollowUpFeed {
            waiting,
            paused: waiting > 0,
            ..FollowUpFeed::default()
        });
    }
    let Some(follow_up) = take_next_follow_up(store, task_id)? else {
        return Ok(FollowUpFeed::default());
    };
    let waiting = count_follow_ups(store, task_id)?;
    store.transaction(|tx| {
        append_event(
            tx,
            task_id,
            "follow_up_started",
            state,
            json!({"instruction": follow_up.instruction, "waiting": waiting}),
            now,
        )?;
        Ok(())
    })?;
    Ok(FollowUpFeed {
        instruction: Some(follow_up.instruction),
        waiting,
        ..FollowUpFeed::default()
    })
}

fn append_event(
    tx: &rusqlite::Transaction<'_>,
    task_id: &str,
    kind: &str,
    state: TaskState,
    payload: serde_json::Value,
    now: &str,
) -> Result<i64, StoreError> {
    tx.execute(
        "INSERT INTO task_events(task_id,event_type,state,payload,created_at) VALUES(?,?,?,?,?)",
        params![task_id, kind, state.as_str(), payload.to_string(), now],
    )?;
    Ok(tx.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use oga_domain::{Profile, Provider, Task, TaskKind, TaskScope};
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
            duration_ms: 0,
            running_since: None,
            output: String::new(),
            error: None,
            question: None,
            parent_task_id: None,
            orchestrator_id: None,
            scope: TaskScope::default(),
            grant_id: None,
            allow_questions: true,
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

    fn service() -> (
        tempfile::TempDir,
        Arc<Store>,
        FollowUpQueue<crate::holds::FixedClock>,
        String,
    ) {
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
        let clock = crate::holds::FixedClock::at_unix_millis(1_735_689_600_000);
        let queue = FollowUpQueue::with_clock(store.clone(), clock);
        (directory, store, queue, cwd)
    }

    #[test]
    fn follow_ups_are_fifo_and_only_clean_completion_starts_one() {
        let (_directory, store, queue, _cwd) = service();
        assert_eq!(
            queue.queue("task", TaskState::Running, " first ").unwrap(),
            1
        );
        assert_eq!(
            queue.queue("task", TaskState::Running, "second").unwrap(),
            2
        );
        let paused = queue.feed("task", TaskState::Failed).unwrap();
        assert!(paused.paused);
        assert_eq!(queue.list("task").unwrap().len(), 2);
        let started = queue.feed("task", TaskState::Completed).unwrap();
        assert_eq!(started.instruction.as_deref(), Some("first"));
        assert_eq!(
            queue.take_next("task").unwrap().unwrap().instruction,
            "second"
        );
        assert_eq!(count_follow_ups(&store, "task").unwrap(), 0);
    }
    #[test]
    fn only_cancelling_abandons_what_is_queued() {
        assert!(settling_clears_follow_ups(TaskState::Cancelled));
        for state in [
            TaskState::Failed,
            TaskState::Blocked,
            TaskState::NeedsInput,
            TaskState::Completed,
        ] {
            assert!(!settling_clears_follow_ups(state), "{state:?}");
        }
    }
}
