//! Dependency edges and the graph queries used by dependency holds.

use std::{
    collections::{HashSet, VecDeque},
    time::Duration,
};

use oga_domain::{HoldArgs, HoldVerb, OnBlockerFailure, Task, TaskHold, TaskState};
use oga_store::{Store, StoreError};
use rusqlite::{OptionalExtension, params};
use serde_json::json;

use crate::{holds, lifecycle};

pub const MAX_PREREQUISITES: usize = 16;
pub const MAX_DEPENDENCY_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyBlocker {
    pub id: String,
    pub state: TaskState,
}

pub fn add_dependencies(
    store: &Store,
    task_id: &str,
    blocker_ids: &[String],
    created_at: &str,
) -> Result<Vec<String>, StoreError> {
    let mut blockers = Vec::with_capacity(blocker_ids.len());
    for blocker in blocker_ids {
        if !blockers.contains(blocker) {
            blockers.push(blocker.clone());
        }
    }
    if blockers.iter().any(|blocker| blocker == task_id) {
        return Err(StoreError::Refusal(format!(
            "task cannot depend on itself: {task_id}"
        )));
    }
    store.transaction(|tx| {
        for blocker in &blockers {
            let exists: Option<i64> = tx
                .query_row("SELECT 1 FROM tasks WHERE id=?", [blocker], |row| row.get(0))
                .optional()?;
            if exists.is_none() {
                return Err(StoreError::Refusal(format!(
                    "unknown prerequisite task: {blocker}"
                )));
            }
        }
        if let Some(cycle) = dependency_cycle(tx, task_id, &blockers)? {
            return Err(StoreError::Refusal(format!(
                "dependency cycle: {}",
                cycle.join(" -> ")
            )));
        }
        if blockers.len() > MAX_PREREQUISITES {
            return Err(StoreError::Refusal(format!(
                "too many prerequisites: {} > {MAX_PREREQUISITES}",
                blockers.len()
            )));
        }
        for blocker in &blockers {
            tx.execute(
                "INSERT OR IGNORE INTO task_dependencies(task_id,blocker_id,created_at) VALUES(?,?,?)",
                params![task_id, blocker, created_at],
            )?;
        }
        Ok(blockers)
    })
}

pub fn dependency_closure(
    store: &Store,
    seed_ids: &[String],
) -> Result<HashSet<String>, StoreError> {
    let mut closure = seed_ids.iter().cloned().collect::<HashSet<_>>();
    let mut queue = VecDeque::from(seed_ids.to_vec());
    while let Some(task_id) = queue.pop_front() {
        let blockers = store.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT blocker_id FROM task_dependencies WHERE task_id=?")?;
            Ok(statement
                .query_map([task_id.as_str()], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })?;
        for blocker in blockers {
            if closure.insert(blocker.clone()) {
                queue.push_back(blocker);
            }
        }
    }
    Ok(closure)
}

pub fn dependencies_of(store: &Store, task_id: &str) -> Result<Vec<Task>, StoreError> {
    let ids = edge_ids(store, task_id, false)?;
    ids.into_iter()
        .map(|id| {
            lifecycle::load_task(store, &id)?
                .ok_or_else(|| StoreError::Refusal(format!("dependency task disappeared: {id}")))
        })
        .collect()
}

pub fn dependents_of(store: &Store, blocker_id: &str) -> Result<Vec<Task>, StoreError> {
    let ids = edge_ids(store, blocker_id, true)?;
    ids.into_iter()
        .map(|id| {
            lifecycle::load_task(store, &id)?
                .ok_or_else(|| StoreError::Refusal(format!("dependent task disappeared: {id}")))
        })
        .collect()
}

pub fn unsettled_blockers(
    store: &Store,
    task_id: &str,
) -> Result<Vec<DependencyBlocker>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT tasks.id,tasks.state FROM task_dependencies JOIN tasks ON tasks.id=task_dependencies.blocker_id WHERE task_dependencies.task_id=? AND tasks.state != 'completed' ORDER BY task_dependencies.created_at,task_dependencies.rowid",
        )?;
        Ok(statement
            .query_map([task_id], |row| {
                let state: String = row.get(1)?;
                let state = serde_json::from_str(&format!("\"{state}\""))
                    .map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))?;
                Ok(DependencyBlocker {
                    id: row.get(0)?,
                    state,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    })
}

pub fn dependency_hold(
    task_id: &str,
    waiting_on: &[Task],
    on_blocker_failure: OnBlockerFailure,
    instruction: Option<String>,
    now: &str,
) -> TaskHold {
    let names = waiting_on
        .iter()
        .map(|task| task.title.as_deref().unwrap_or(&task.id))
        .collect::<Vec<_>>();
    TaskHold {
        task_id: task_id.into(),
        verb: HoldVerb::Delegate,
        args: HoldArgs {
            instruction,
            on_blocker_failure: (on_blocker_failure == OnBlockerFailure::Run)
                .then_some(OnBlockerFailure::Run),
            network: None,
            restart: None,
        },
        start_at: None,
        await_profile: None,
        await_model: None,
        next_check_at: now.into(),
        expires_at: parse_iso(now)
            .and_then(|time| time.checked_add(holds::HOLD_EXPIRY))
            .map(lifecycle::iso_from_system_time)
            .unwrap_or_else(|| now.to_owned()),
        probe_count: 0,
        note: if on_blocker_failure == OnBlockerFailure::Run {
            format!(
                "waiting for {} to finish; will start even if one fails",
                names.join(", ")
            )
        } else {
            format!("waiting for {} to finish", names.join(", "))
        },
        created_at: now.into(),
        updated_at: now.into(),
    }
}

pub fn restore_dependency_hold(
    store: &Store,
    hold: &TaskHold,
    now: &str,
) -> Result<bool, StoreError> {
    let args = serde_json::to_string(&hold.args)
        .map_err(|error| StoreError::Refusal(format!("invalid hold JSON: {error}")))?;
    store.transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='pending',error=NULL,completion_json=NULL,updated_at=? WHERE id=? AND state IN ('blocked','cancelled') AND json_extract(completion_json,'$.dependencyBlocked')=1",
            params![now, hold.task_id],
        )?;
        if changed != 1 {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO task_holds(task_id,verb,args_json,start_at,await_profile,await_model,next_check_at,expires_at,probe_count,note,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET verb=excluded.verb,args_json=excluded.args_json,start_at=excluded.start_at,await_profile=excluded.await_profile,await_model=excluded.await_model,next_check_at=excluded.next_check_at,expires_at=excluded.expires_at,probe_count=excluded.probe_count,note=excluded.note,updated_at=excluded.updated_at",
            params![
                hold.task_id,
                "delegate",
                args,
                hold.start_at,
                hold.await_profile,
                hold.await_model,
                hold.next_check_at,
                hold.expires_at,
                hold.probe_count,
                hold.note,
                hold.created_at,
                now,
            ],
        )?;
        append_event(
            tx,
            &hold.task_id,
            "hold_armed",
            TaskState::Pending,
            json!({"note": hold.note}),
            now,
        )?;
        Ok(true)
    })
}

fn edge_ids(store: &Store, task_id: &str, reverse: bool) -> Result<Vec<String>, StoreError> {
    let sql = if reverse {
        "SELECT task_id FROM task_dependencies WHERE blocker_id=? ORDER BY created_at,rowid"
    } else {
        "SELECT blocker_id FROM task_dependencies WHERE task_id=? ORDER BY created_at,rowid"
    };
    store.with_connection(|connection| {
        let mut statement = connection.prepare(sql)?;
        Ok(statement
            .query_map([task_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?)
    })
}

fn dependency_cycle(
    tx: &rusqlite::Transaction<'_>,
    task_id: &str,
    blockers: &[String],
) -> Result<Option<Vec<String>>, StoreError> {
    let mut queue = blockers
        .iter()
        .map(|blocker| (blocker.clone(), vec![task_id.to_owned(), blocker.clone()]))
        .collect::<VecDeque<_>>();
    let mut visited = blockers.iter().cloned().collect::<HashSet<_>>();
    while let Some((node, chain)) = queue.pop_front() {
        let next_ids = {
            let mut statement = tx.prepare(
                "SELECT blocker_id FROM task_dependencies WHERE task_id=? ORDER BY created_at,rowid",
            )?;
            statement
                .query_map([node.as_str()], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for next in next_ids {
            let mut next_chain = chain.clone();
            next_chain.push(next.clone());
            if next == task_id || next_chain.len() > MAX_DEPENDENCY_DEPTH {
                return Ok(Some(next_chain));
            }
            if !visited.insert(next.clone()) {
                continue;
            }
            queue.push_back((next, next_chain));
        }
    }
    Ok(None)
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

fn parse_iso(value: &str) -> Option<std::time::SystemTime> {
    let parts = value.get(0..23).and_then(|value| {
        let year = value.get(0..4)?.parse::<i32>().ok()?;
        let month = value.get(5..7)?.parse::<u32>().ok()?;
        let day = value.get(8..10)?.parse::<u32>().ok()?;
        let hour = value.get(11..13)?.parse::<u64>().ok()?;
        let minute = value.get(14..16)?.parse::<u64>().ok()?;
        let second = value.get(17..19)?.parse::<u64>().ok()?;
        let millis = value.get(20..23)?.parse::<u64>().ok()?;
        Some((year, month, day, hour, minute, second, millis))
    });
    let (year, month, day, hour, minute, second, millis) = parts?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)?
        .checked_add((hour * 3_600 + minute * 60 + second) as i64)?;
    let millis = seconds.checked_mul(1_000)?.checked_add(millis as i64)?;
    if millis < 0 {
        return None;
    }
    Some(std::time::UNIX_EPOCH + Duration::from_millis(millis as u64))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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
            state: TaskState::Queued,
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
        }
    }

    #[test]
    fn dependency_edges_reject_cycles_and_keep_fifo_queries() {
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
        for id in ["a", "b", "c"] {
            store
                .repositories()
                .tasks()
                .insert(&task(id, directory.path().to_str().unwrap()))
                .expect("task");
        }
        let now = "2026-01-01T00:00:00.000Z";
        add_dependencies(&store, "a", &["b".into(), "c".into()], now).expect("a waits on b and c");
        add_dependencies(&store, "b", &["c".into()], now).expect("b waits on c");
        let cycle = add_dependencies(&store, "c", &["a".into()], now);
        assert!(cycle.unwrap_err().to_string().contains("dependency cycle"));
        assert_eq!(dependency_closure(&store, &["a".into()]).unwrap().len(), 3);
        let dependencies = dependencies_of(&store, "a").unwrap();
        assert_eq!(dependencies[0].id, "b");
        assert_eq!(dependencies[1].id, "c");
    }
}
