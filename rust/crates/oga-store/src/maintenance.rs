use std::fs;

use oga_domain::{CleanupPlan, CleanupRecord, CleanupResult, CleanupStateCount, TaskState};
use rusqlite::types::Value;
use rusqlite::{Connection, params, params_from_iter};
use serde_json::json;

use crate::{Store, StoreError};

const SETTLED_STATES: &str = "'completed','failed','cancelled'";

impl Store {
    pub fn cleanup_plan(
        &self,
        cutoff: &str,
        archived_only: bool,
    ) -> Result<CleanupPlan, StoreError> {
        self.with_connection(|connection| {
            cleanup_plan_connection(
                connection,
                cutoff,
                archived_only,
                self.viewed_task().as_deref(),
            )
        })
    }

    /// Delete old activity and rewrite the database while broker writes wait.
    pub fn cleanup(
        &self,
        cutoff: &str,
        archived_only: bool,
        finished_at: &str,
    ) -> Result<CleanupResult, StoreError> {
        let before = file_bytes(self.path());
        let result = self.with_maintenance(|connection| {
            let viewed = self.viewed_task();
            let transaction = connection.transaction()?;
            let plan = cleanup_plan_connection(
                &transaction,
                cutoff,
                archived_only,
                viewed.as_deref(),
            )?;
            let mut statement = transaction.prepare(&format!(
                "SELECT task_id,state,COUNT(*) FROM task_events WHERE task_id IN ({}) AND event_type != 'history_dropped' GROUP BY task_id,state",
                eligible_tasks_sql(archived_only),
            ))?;
            let rows = statement
                .query_map(cleanup_params(cutoff, viewed.as_deref()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        parse_state(&row.get::<_, String>(1)?)?,
                        row.get::<_, u64>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            let removed = transaction.execute(
                &format!(
                    "DELETE FROM task_events WHERE task_id IN ({}) AND event_type != 'history_dropped'",
                    eligible_tasks_sql(archived_only),
                ),
                cleanup_params(cutoff, viewed.as_deref()),
            )? as u64;
            if removed != plan.events {
                return Err(StoreError::Refusal(format!(
                    "cleanup aborted: expected to remove {} records, the delete touched {removed}",
                    plan.events
                )));
            }
            for (task_id, state, count) in rows {
                transaction.execute(
                    "INSERT INTO task_events(task_id,event_type,state,payload,created_at,turn_id) VALUES(?,?,?,?,?,NULL)",
                    params![
                        task_id,
                        "history_dropped",
                        state.as_str(),
                        json!({ "dropped": count, "before": cutoff }).to_string(),
                        finished_at,
                    ],
                )?;
            }
            transaction.commit()?;
            if plan.events > 0 {
                connection.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
            }
            Ok((plan,))
        })?;
        let plan = result.0;
        Ok(CleanupResult {
            record: CleanupRecord {
                plan,
                finished_at: finished_at.to_owned(),
            },
            file_bytes_before: before,
            file_bytes_after: file_bytes(self.path()),
        })
    }
}

fn cleanup_plan_connection(
    connection: &Connection,
    cutoff: &str,
    archived_only: bool,
    viewed_task: Option<&str>,
) -> Result<CleanupPlan, StoreError> {
    let sql = eligible_tasks_sql(archived_only);
    let mut states = connection.prepare(&format!(
        "SELECT state,COUNT(*) FROM tasks WHERE id IN ({sql}) GROUP BY state ORDER BY COUNT(*) DESC,state"
    ))?;
    let by_state = states
        .query_map(cleanup_params(cutoff, viewed_task), |row| {
            Ok(CleanupStateCount {
                state: parse_state(&row.get::<_, String>(0)?)?,
                tasks: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let totals = connection.query_row(
        &format!(
            "SELECT COUNT(*),COALESCE(SUM(LENGTH(CAST(payload AS BLOB))),0) FROM task_events WHERE task_id IN ({sql}) AND event_type != 'history_dropped'"
        ),
        cleanup_params(cutoff, viewed_task),
        |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
    )?;
    let archive_clause = archive_clause(archived_only, "");
    let candidates: u64 = connection.query_row(
        &format!(
            "SELECT COUNT(*) FROM tasks WHERE state IN ({SETTLED_STATES}){archive_clause} AND updated_at < ? AND (? IS NULL OR id != ?)"
        ),
        params![cutoff, viewed_task, viewed_task],
        |row| row.get(0),
    )?;
    let tasks = by_state.iter().map(|row| row.tasks).sum();
    Ok(CleanupPlan {
        cutoff: cutoff.to_owned(),
        tasks,
        by_state,
        events: totals.0,
        bytes: totals.1,
        held_back: candidates.saturating_sub(tasks),
    })
}

fn eligible_tasks_sql(archived_only: bool) -> String {
    let archive = archive_clause(archived_only, "");
    let child_archive = archive_clause(archived_only, "child.");
    format!(
        "SELECT parent.id FROM tasks parent WHERE parent.state IN ({SETTLED_STATES}){archive} AND parent.updated_at < ? AND (? IS NULL OR parent.id != ?) AND NOT EXISTS (SELECT 1 FROM tasks child WHERE child.parent_task_id = parent.id AND NOT (child.state IN ({SETTLED_STATES}){child_archive} AND child.updated_at < ? AND (? IS NULL OR child.id != ?)))"
    )
}

fn archive_clause(archived_only: bool, prefix: &str) -> String {
    if archived_only {
        format!(" AND {prefix}archived_at IS NOT NULL")
    } else {
        String::new()
    }
}

fn cleanup_params(cutoff: &str, viewed_task: Option<&str>) -> impl rusqlite::Params {
    let viewed = viewed_task.map_or(Value::Null, |value| Value::Text(value.to_owned()));
    params_from_iter(vec![
        Value::Text(cutoff.to_owned()),
        viewed.clone(),
        viewed.clone(),
        Value::Text(cutoff.to_owned()),
        viewed.clone(),
        viewed,
    ])
}

fn parse_state(value: &str) -> rusqlite::Result<TaskState> {
    serde_json::from_str(&format!("\"{value}\"")).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn file_bytes(path: &std::path::Path) -> u64 {
    [
        path.to_path_buf(),
        std::path::PathBuf::from(format!("{}-wal", path.display())),
    ]
    .into_iter()
    .filter_map(|path| fs::metadata(path).ok().map(|metadata| metadata.len()))
    .sum()
}
