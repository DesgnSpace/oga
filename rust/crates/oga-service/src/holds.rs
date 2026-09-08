//! Durable task holds and the clock-driven release sweep.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use oga_domain::{CompletionCode, HoldVerb, Task, TaskCompletion, TaskHold, TaskState};
use oga_store::{Store, StoreError};
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use thiserror::Error;

use crate::{dependencies, lifecycle};

pub const HOLD_EXPIRY: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const MIN_RECHECK: Duration = Duration::from_secs(30);
pub const MAX_RECHECK: Duration = Duration::from_secs(60 * 60);

/// Time is a dependency because persisted holds must be testable without sleep.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[derive(Debug, Clone)]
pub struct FixedClock {
    now: Arc<Mutex<SystemTime>>,
}

impl FixedClock {
    pub fn new(now: SystemTime) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    pub fn at_unix_millis(millis: u64) -> Self {
        Self::new(UNIX_EPOCH + Duration::from_millis(millis))
    }

    pub fn set(&self, now: SystemTime) {
        *self.now.lock().expect("fixed clock is not poisoned") = now;
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.now.lock().expect("fixed clock is not poisoned");
        *now = now.checked_add(by).expect("fixed clock overflowed");
    }
}

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        *self.now.lock().expect("fixed clock is not poisoned")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    pub available: bool,
    pub retry_at: Option<String>,
}

impl Availability {
    pub fn available() -> Self {
        Self {
            available: true,
            retry_at: None,
        }
    }

    pub fn unavailable(retry_at: Option<String>) -> Self {
        Self {
            available: false,
            retry_at,
        }
    }
}

/// External status and connectivity checks used by rate-limit and network holds.
pub trait HoldProbe {
    fn profile_available(
        &self,
        profile: &str,
        model: Option<&str>,
        cwd: &str,
    ) -> Result<Availability, String>;

    fn network_available(&self, cwd: &str) -> Result<bool, String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AlwaysAvailable;

impl HoldProbe for AlwaysAvailable {
    fn profile_available(
        &self,
        _profile: &str,
        _model: Option<&str>,
        _cwd: &str,
    ) -> Result<Availability, String> {
        Ok(Availability::available())
    }

    fn network_available(&self, _cwd: &str) -> Result<bool, String> {
        Ok(true)
    }
}

#[derive(Debug, Error)]
pub enum HoldError {
    #[error(transparent)]
    Store(#[from] StoreError),
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct HoldSweepReport {
    /// The whole hold, not just its task: what release replays — an
    /// instruction, a restart's attempt count — lives in its args, and the row
    /// is already deleted by the time the caller launches.
    pub released: Vec<TaskHold>,
    pub rechecked: Vec<String>,
    pub expired: Vec<String>,
    pub dropped: Vec<String>,
    pub blocked: Vec<String>,
    pub probe_errors: Vec<String>,
}

pub struct HoldSweep<C = SystemClock> {
    store: Arc<Store>,
    clock: C,
}

pub type HoldService<C = SystemClock> = HoldSweep<C>;

impl HoldSweep<SystemClock> {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            clock: SystemClock,
        }
    }
}

impl<C: Clock> HoldSweep<C> {
    pub fn with_clock(store: Arc<Store>, clock: C) -> Self {
        Self { store, clock }
    }

    pub fn arm(&self, hold: &TaskHold) -> Result<Task, HoldError> {
        arm_hold(&self.store, hold)?;
        lifecycle::load_task(&self.store, &hold.task_id)?.ok_or_else(|| {
            HoldError::Store(StoreError::Refusal(format!(
                "task disappeared after hold arm: {}",
                hold.task_id
            )))
        })
    }

    pub fn sweep(&self) -> Result<HoldSweepReport, HoldError> {
        self.sweep_with(&AlwaysAvailable)
    }

    pub fn sweep_with<P: HoldProbe>(&self, probe: &P) -> Result<HoldSweepReport, HoldError> {
        let now = self.clock.now();
        let now_iso = lifecycle::iso_from_system_time(now);
        let holds = due_holds(&self.store, &now_iso)?;
        let mut report = HoldSweepReport::default();

        for hold in holds {
            match self.evaluate(&hold, now, &now_iso, probe)? {
                HoldAction::Released => report.released.push(hold),
                HoldAction::Rechecked => report.rechecked.push(hold.task_id),
                HoldAction::Expired => report.expired.push(hold.task_id),
                HoldAction::Dropped => report.dropped.push(hold.task_id),
                HoldAction::Blocked => report.blocked.push(hold.task_id),
                HoldAction::ProbeError => {
                    report.probe_errors.push(hold.task_id.clone());
                    report.rechecked.push(hold.task_id);
                }
            }
        }
        Ok(report)
    }

    fn evaluate<P: HoldProbe>(
        &self,
        hold: &TaskHold,
        now: SystemTime,
        now_iso: &str,
        probe: &P,
    ) -> Result<HoldAction, HoldError> {
        let Some(task) = lifecycle::load_task(&self.store, &hold.task_id)? else {
            drop_hold(
                &self.store,
                hold,
                "hold_dropped",
                json!({"reason": "task no longer exists"}),
                now_iso,
            )?;
            return Ok(HoldAction::Dropped);
        };
        if task.state != TaskState::Pending {
            drop_hold(
                &self.store,
                hold,
                "hold_dropped",
                json!({"reason": format!("task is {}", task.state.as_str())}),
                now_iso,
            )?;
            return Ok(HoldAction::Dropped);
        }
        if now_iso >= hold.expires_at.as_str() {
            if hold.args.network.is_some() {
                fail_held_task(
                    &self.store,
                    &task,
                    &network_gave_up(hold),
                    CompletionCode::Network,
                    now_iso,
                )?;
            } else {
                block_held_task(
                    &self.store,
                    &task,
                    &format!(
                        "held until {} without its start condition coming true; {}",
                        hold.expires_at, hold.note
                    ),
                    false,
                    now_iso,
                )?;
            }
            drop_hold(
                &self.store,
                hold,
                "hold_expired",
                json!({"dueAt": hold.start_at_or_next_check()}),
                now_iso,
            )?;
            return Ok(HoldAction::Expired);
        }
        if hold
            .start_at
            .as_deref()
            .is_some_and(|start| now_iso < start)
        {
            touch_hold(
                &self.store,
                hold,
                hold.start_at.as_deref().unwrap(),
                hold.probe_count,
                now_iso,
            )?;
            return Ok(HoldAction::Rechecked);
        }
        if let Some(await_profile) = hold.await_profile.as_deref() {
            let availability = match probe.profile_available(
                await_profile,
                hold.await_model.as_deref(),
                &task.cwd,
            ) {
                Ok(value) => value,
                Err(_) => {
                    touch_hold(
                        &self.store,
                        hold,
                        &next_check(now, None),
                        hold.probe_count,
                        now_iso,
                    )?;
                    return Ok(HoldAction::ProbeError);
                }
            };
            if !availability.available {
                touch_hold(
                    &self.store,
                    hold,
                    &next_check(now, availability.retry_at.as_deref()),
                    hold.probe_count + 1,
                    now_iso,
                )?;
                return Ok(HoldAction::Rechecked);
            }
        }
        if hold.args.network.is_some() {
            let max_attempts = hold
                .args
                .network
                .as_ref()
                .map(|network| network.max_attempts)
                .unwrap_or(0);
            if hold.probe_count >= max_attempts {
                fail_held_task(
                    &self.store,
                    &task,
                    &network_gave_up(hold),
                    CompletionCode::Network,
                    now_iso,
                )?;
                drop_hold(
                    &self.store,
                    hold,
                    "network_retry_exhausted",
                    json!({"attempts": hold.probe_count}),
                    now_iso,
                )?;
                return Ok(HoldAction::Expired);
            }
            let online = match probe.network_available(&task.cwd) {
                Ok(value) => value,
                Err(_) => {
                    touch_hold(
                        &self.store,
                        hold,
                        &next_check(now, None),
                        hold.probe_count,
                        now_iso,
                    )?;
                    return Ok(HoldAction::ProbeError);
                }
            };
            if !online {
                let delay = network_backoff(hold.probe_count);
                let next = lifecycle::iso_from_system_time(now.checked_add(delay).unwrap_or(now));
                touch_hold(&self.store, hold, &next, hold.probe_count + 1, now_iso)?;
                return Ok(HoldAction::Rechecked);
            }
        }
        if hold.verb == HoldVerb::Delegate {
            let blockers = dependencies::unsettled_blockers(&self.store, &task.id)?;
            if let Some(blocker) = blockers
                .iter()
                .find(|blocker| is_blocking_end(blocker.state))
                && hold.args.on_blocker_failure != Some(oga_domain::OnBlockerFailure::Run)
            {
                let reason = format!(
                    "prerequisite {} ended {}; {}",
                    blocker.id,
                    blocker.state.as_str(),
                    hold.note
                );
                block_held_task(&self.store, &task, &reason, true, now_iso)?;
                drop_hold(
                    &self.store,
                    hold,
                    "hold_dropped",
                    json!({"reason": format!("prerequisite {} ended {}", blocker.id, blocker.state.as_str())}),
                    now_iso,
                )?;
                return Ok(HoldAction::Blocked);
            }
            if blockers
                .iter()
                .any(|blocker| !is_blocking_end(blocker.state))
            {
                touch_hold(
                    &self.store,
                    hold,
                    &next_check(now, None),
                    hold.probe_count,
                    now_iso,
                )?;
                return Ok(HoldAction::Rechecked);
            }
        }
        if release_hold(&self.store, hold, now_iso)? {
            Ok(HoldAction::Released)
        } else {
            Ok(HoldAction::Dropped)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoldAction {
    Released,
    Rechecked,
    Expired,
    Dropped,
    Blocked,
    ProbeError,
}

pub fn arm_hold(store: &Store, hold: &TaskHold) -> Result<(), StoreError> {
    let args = serde_json::to_string(&hold.args)
        .map_err(|error| StoreError::Refusal(format!("invalid hold JSON: {error}")))?;
    store.transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='pending',updated_at=? WHERE id=? AND state IN ('failed','cancelled','blocked','queued','pending','completed')",
            params![hold.updated_at, hold.task_id],
        )?;
        if changed != 1 {
            return Err(StoreError::Refusal(format!(
                "task cannot be held: {}",
                hold.task_id
            )));
        }
        tx.execute(
            "INSERT INTO task_holds(task_id,verb,args_json,start_at,await_profile,await_model,next_check_at,expires_at,probe_count,note,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET verb=excluded.verb,args_json=excluded.args_json,start_at=excluded.start_at,await_profile=excluded.await_profile,await_model=excluded.await_model,next_check_at=excluded.next_check_at,expires_at=excluded.expires_at,probe_count=excluded.probe_count,note=excluded.note,updated_at=excluded.updated_at",
            params![
                hold.task_id,
                hold_verb(hold.verb),
                args,
                hold.start_at,
                hold.await_profile,
                hold.await_model,
                hold.next_check_at,
                hold.expires_at,
                hold.probe_count,
                hold.note,
                hold.created_at,
                hold.updated_at,
            ],
        )?;
        append_event(
            tx,
            &hold.task_id,
            "hold_armed",
            TaskState::Pending,
            json!({
                "note": hold.note,
                "wait": wait_kind(hold),
                "resumesAt": hold.start_at.as_deref().unwrap_or(&hold.next_check_at),
            }),
            &hold.updated_at,
        )?;
        Ok(())
    })
}

pub fn get_hold(store: &Store, task_id: &str) -> Result<Option<TaskHold>, StoreError> {
    store.with_connection(|connection| {
        connection
            .query_row(
                "SELECT task_id,verb,args_json,start_at,await_profile,await_model,next_check_at,expires_at,probe_count,note,created_at,updated_at FROM task_holds WHERE task_id=?",
                [task_id],
                hold_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    })
}

pub fn due_holds(store: &Store, now: &str) -> Result<Vec<TaskHold>, StoreError> {
    store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT task_id,verb,args_json,start_at,await_profile,await_model,next_check_at,expires_at,probe_count,note,created_at,updated_at FROM task_holds WHERE next_check_at <= ? ORDER BY created_at",
        )?;
        Ok(statement
            .query_map([now], hold_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    })
}

pub fn touch_hold(
    store: &Store,
    hold: &TaskHold,
    next_check_at: &str,
    probe_count: u32,
    now: &str,
) -> Result<bool, StoreError> {
    store.transaction(|tx| {
        Ok(tx.execute(
            "UPDATE task_holds SET next_check_at=?,probe_count=?,updated_at=? WHERE task_id=?",
            params![next_check_at, probe_count, now, hold.task_id],
        )? == 1)
    })
}

pub fn drop_hold(
    store: &Store,
    hold: &TaskHold,
    event: &str,
    payload: serde_json::Value,
    now: &str,
) -> Result<bool, StoreError> {
    store.transaction(|tx| {
        let changed = tx.execute("DELETE FROM task_holds WHERE task_id=?", [&hold.task_id])?;
        if changed != 1 {
            return Ok(false);
        }
        let state = task_state(tx, &hold.task_id)?.unwrap_or(TaskState::Pending);
        append_event(tx, &hold.task_id, event, state, payload, now)?;
        Ok(true)
    })
}

fn release_hold(store: &Store, hold: &TaskHold, now: &str) -> Result<bool, StoreError> {
    store.transaction(|tx| {
        let changed = tx.execute(
            "DELETE FROM task_holds WHERE task_id=? AND EXISTS (SELECT 1 FROM tasks WHERE id=? AND state='pending')",
            params![hold.task_id, hold.task_id],
        )?;
        if changed != 1 {
            return Ok(false);
        }
        // The park is over, so the ending that caused it goes with it: a run
        // that starts again must not carry the last one's error into the UI.
        tx.execute(
            "UPDATE tasks SET state='queued',error=NULL,completion_json=NULL,updated_at=? WHERE id=? AND state='pending'",
            params![now, hold.task_id],
        )?;
        append_event(
            tx,
            &hold.task_id,
            "hold_released",
            TaskState::Pending,
            json!({"note": hold.note, "wait": wait_kind(hold)}),
            now,
        )?;
        append_event(
            tx,
            &hold.task_id,
            "queued",
            TaskState::Queued,
            json!({"note": hold.note, "verb": hold_verb(hold.verb)}),
            now,
        )?;
        Ok(true)
    })
}

/// Which unattended wait a hold is, for the surfaces that show one differently
/// from a scheduled start or a prerequisite. `null` for those two.
fn wait_kind(hold: &TaskHold) -> Option<&'static str> {
    if hold.args.network.is_some() {
        return Some("network");
    }
    hold.await_profile.is_some().then_some("rate_limit")
}

fn block_held_task(
    store: &Store,
    task: &Task,
    reason: &str,
    dependency_blocked: bool,
    now: &str,
) -> Result<bool, StoreError> {
    let completion = TaskCompletion {
        blocked: true,
        code: CompletionCode::Cancelled,
        reason: Some(reason.to_owned()),
        dependency_blocked: dependency_blocked.then_some(true),
        exit_code: None,
        suggested_scope: None,
        resets_at: None,
        asserted_completion: None,
    };
    store.transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='blocked',error=?,completion_json=?,updated_at=? WHERE id=? AND state='pending'",
            params![reason, serde_json::to_string(&completion).map_err(|error| StoreError::Refusal(error.to_string()))?, now, task.id],
        )?;
        if changed == 1 {
            append_event(
                tx,
                &task.id,
                "blocked",
                TaskState::Blocked,
                json!({"error": reason, "completion": completion}),
                now,
            )?;
        }
        Ok(changed == 1)
    })
}

fn fail_held_task(
    store: &Store,
    task: &Task,
    reason: &str,
    code: CompletionCode,
    now: &str,
) -> Result<bool, StoreError> {
    let completion = TaskCompletion {
        blocked: true,
        code,
        reason: Some(reason.to_owned()),
        dependency_blocked: None,
        exit_code: None,
        suggested_scope: None,
        resets_at: None,
        asserted_completion: None,
    };
    store.transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='failed',error=?,completion_json=?,updated_at=? WHERE id=? AND state='pending'",
            params![reason, serde_json::to_string(&completion).map_err(|error| StoreError::Refusal(error.to_string()))?, now, task.id],
        )?;
        if changed == 1 {
            append_event(
                tx,
                &task.id,
                "failed",
                TaskState::Failed,
                json!({"error": reason, "completion": completion}),
                now,
            )?;
        }
        Ok(changed == 1)
    })
}

fn hold_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskHold> {
    let verb = match row.get::<_, String>(1)?.as_str() {
        "delegate" => HoldVerb::Delegate,
        _ => HoldVerb::Resume,
    };
    let args = serde_json::from_str(&row.get::<_, String>(2)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(TaskHold {
        task_id: row.get(0)?,
        verb,
        args,
        start_at: row.get(3)?,
        await_profile: row.get(4)?,
        await_model: row.get(5)?,
        next_check_at: row.get(6)?,
        expires_at: row.get(7)?,
        probe_count: row.get(8)?,
        note: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn task_state(
    tx: &rusqlite::Transaction<'_>,
    task_id: &str,
) -> Result<Option<TaskState>, StoreError> {
    tx.query_row("SELECT state FROM tasks WHERE id=?", [task_id], |row| {
        let state: String = row.get(0)?;
        serde_json::from_str(&format!("\"{state}\""))
            .map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))
    })
    .optional()
    .map_err(StoreError::from)
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

fn hold_verb(verb: HoldVerb) -> &'static str {
    match verb {
        HoldVerb::Resume => "resume",
        HoldVerb::Delegate => "delegate",
    }
}

fn is_blocking_end(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Failed | TaskState::Cancelled | TaskState::Blocked
    )
}

fn next_check(now: SystemTime, retry_at: Option<&str>) -> String {
    let minimum = lifecycle::iso_from_system_time(now + MIN_RECHECK);
    let maximum = lifecycle::iso_from_system_time(now + MAX_RECHECK);
    let hinted = retry_at.unwrap_or(&minimum);
    if hinted < minimum.as_str() {
        minimum
    } else if hinted > maximum.as_str() {
        maximum
    } else {
        hinted.to_owned()
    }
}

/// What a task that waited out its budget ends with. It keeps the provider's
/// own words after the plain sentence, because that is all anyone debugging a
/// dropped connection has to go on.
fn network_gave_up(hold: &TaskHold) -> String {
    let error = hold
        .args
        .network
        .as_ref()
        .and_then(|network| network.original_error.as_deref())
        .unwrap_or(&hold.note);
    format!(
        "The connection didn't come back, so this task stopped waiting. \
         Check your connection and start it again. {error}"
    )
}

fn network_backoff(attempt: u32) -> Duration {
    let multiplier = 1_u32.checked_shl(attempt.min(6)).unwrap_or(64);
    MIN_RECHECK.saturating_mul(multiplier).min(MAX_RECHECK)
}

trait HoldFields {
    fn start_at_or_next_check(&self) -> &str;
}

impl HoldFields for TaskHold {
    fn start_at_or_next_check(&self) -> &str {
        self.start_at.as_deref().unwrap_or(&self.next_check_at)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{collections::BTreeMap, sync::Arc, time::Duration};

    use oga_domain::{HoldArgs, Profile, Provider, Task, TaskKind, TaskScope};
    use tempfile::tempdir;

    use super::*;

    pub(crate) fn task(id: &str, cwd: &str, state: TaskState) -> Task {
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
            state,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            duration_ms: 0,
            running_since: None,
            output: String::new(),
            error: None,
            question: None,
            parent_task_id: None,
            orchestrator_id: None,
            scope: TaskScope {
                read: vec!["**".into()],
                write: vec!["**".into()],
            },
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

    fn service() -> (tempfile::TempDir, Arc<Store>, FixedClock, String) {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("oga.db");
        let store = Arc::new(Store::open_writable(path).expect("store"));
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
            .insert(&task("held", &cwd, TaskState::Queued))
            .expect("task");
        let clock = FixedClock::at_unix_millis(1_735_689_600_000);
        (directory, store, clock, cwd)
    }

    /// A hold built the way one of the unattended waits builds it, with the
    /// clock fields the test wants to drive.
    struct Wait {
        args: HoldArgs,
        start_at: Option<String>,
        await_profile: Option<String>,
        next_check_at: String,
    }

    fn arm(sweep: &HoldSweep<FixedClock>, clock: &FixedClock, wait: Wait) {
        let now = lifecycle::iso_from_system_time(clock.now());
        sweep
            .arm(&TaskHold {
                task_id: "held".into(),
                verb: HoldVerb::Resume,
                args: wait.args,
                start_at: wait.start_at,
                await_profile: wait.await_profile.clone(),
                await_model: wait.await_profile.map(|_| "model".into()),
                next_check_at: wait.next_check_at,
                expires_at: lifecycle::iso_from_system_time(clock.now() + HOLD_EXPIRY),
                probe_count: 0,
                note: "waiting".into(),
                created_at: now.clone(),
                updated_at: now,
            })
            .expect("arm");
    }

    fn in_seconds(clock: &FixedClock, seconds: u64) -> String {
        lifecycle::iso_from_system_time(clock.now() + Duration::from_secs(seconds))
    }

    fn released_ids(report: &HoldSweepReport) -> Vec<&str> {
        report
            .released
            .iter()
            .map(|hold| hold.task_id.as_str())
            .collect()
    }

    /// A probe with one answer, for the sweep decisions that turn on it.
    struct Answer {
        account: Availability,
        online: bool,
    }

    impl HoldProbe for Answer {
        fn profile_available(
            &self,
            _profile: &str,
            _model: Option<&str>,
            _cwd: &str,
        ) -> Result<Availability, String> {
            Ok(self.account.clone())
        }

        fn network_available(&self, _cwd: &str) -> Result<bool, String> {
            Ok(self.online)
        }
    }

    fn answer(account: Availability, online: bool) -> Answer {
        Answer { account, online }
    }

    fn network_wait(max_attempts: u32) -> HoldArgs {
        HoldArgs {
            network: Some(oga_domain::NetworkHoldArgs {
                attempt: 0,
                max_attempts,
                original_error: Some("fetch failed".into()),
            }),
            ..HoldArgs::default()
        }
    }

    /// The wait is over when the reset time has passed *and* the account reads
    /// usable. A provider that is still refusing pushes the next look out to
    /// its own retry time instead of starting a run that would only fail.
    #[test]
    fn a_rate_limit_wait_re_arms_while_the_account_is_still_out() {
        let (_directory, store, clock, _cwd) = service();
        let sweep = HoldSweep::with_clock(store.clone(), clock.clone());
        arm(
            &sweep,
            &clock,
            Wait {
                args: HoldArgs::default(),
                start_at: Some(in_seconds(&clock, 60)),
                await_profile: Some("profile".into()),
                next_check_at: in_seconds(&clock, 60),
            },
        );
        clock.advance(Duration::from_secs(60));

        let still_limited = answer(
            Availability::unavailable(Some(in_seconds(&clock, 600))),
            true,
        );
        let report = sweep.sweep_with(&still_limited).expect("first sweep");
        assert_eq!(report.rechecked, vec!["held"]);
        let hold = get_hold(&store, "held").expect("lookup").expect("hold");
        assert_eq!(hold.probe_count, 1);
        assert_eq!(hold.next_check_at, in_seconds(&clock, 600));

        clock.advance(Duration::from_secs(600));
        let report = sweep
            .sweep_with(&answer(Availability::available(), true))
            .expect("second sweep");
        assert_eq!(released_ids(&report), vec!["held"]);
        assert!(get_hold(&store, "held").expect("lookup").is_none());
    }

    #[test]
    fn a_network_wait_backs_off_while_the_connection_is_down() {
        let (_directory, store, clock, _cwd) = service();
        let sweep = HoldSweep::with_clock(store.clone(), clock.clone());
        arm(
            &sweep,
            &clock,
            Wait {
                args: network_wait(2),
                start_at: None,
                await_profile: None,
                next_check_at: lifecycle::iso_from_system_time(clock.now()),
            },
        );
        let offline = answer(Availability::available(), false);

        assert_eq!(
            sweep.sweep_with(&offline).expect("first").rechecked,
            vec!["held"]
        );
        let hold = get_hold(&store, "held").expect("lookup").expect("hold");
        assert_eq!(hold.probe_count, 1);
        assert_eq!(hold.next_check_at, in_seconds(&clock, 30));

        clock.advance(Duration::from_secs(30));
        assert_eq!(
            sweep.sweep_with(&offline).expect("second").rechecked,
            vec!["held"]
        );
        assert_eq!(
            get_hold(&store, "held")
                .expect("lookup")
                .expect("hold")
                .next_check_at,
            in_seconds(&clock, 60)
        );

        clock.advance(Duration::from_secs(60));
        let report = sweep.sweep_with(&offline).expect("third");
        assert_eq!(report.expired, vec!["held"]);
        let task = lifecycle::load_task(&store, "held")
            .expect("load")
            .expect("task");
        assert_eq!(task.state, TaskState::Failed);
        assert_eq!(
            task.completion.expect("completion").code,
            CompletionCode::Network
        );
        assert!(
            task.error.expect("error").contains("fetch failed"),
            "the give-up message keeps the provider's own words"
        );
    }

    #[test]
    fn a_network_wait_releases_as_soon_as_the_connection_answers() {
        let (_directory, store, clock, _cwd) = service();
        let sweep = HoldSweep::with_clock(store.clone(), clock.clone());
        arm(
            &sweep,
            &clock,
            Wait {
                args: network_wait(12),
                start_at: None,
                await_profile: None,
                next_check_at: lifecycle::iso_from_system_time(clock.now()),
            },
        );
        let report = sweep
            .sweep_with(&answer(Availability::available(), true))
            .expect("sweep");
        assert_eq!(released_ids(&report), vec!["held"]);
        assert_eq!(
            lifecycle::load_task(&store, "held")
                .expect("load")
                .expect("task")
                .state,
            TaskState::Queued
        );
    }

    #[test]
    fn timed_hold_waits_then_releases_on_the_fixed_clock() {
        let (_directory, store, clock, _cwd) = service();
        let now = lifecycle::iso_from_system_time(clock.now());
        let start = lifecycle::iso_from_system_time(clock.now() + Duration::from_secs(60));
        let expiry = lifecycle::iso_from_system_time(clock.now() + HOLD_EXPIRY);
        let sweep = HoldSweep::with_clock(store.clone(), clock.clone());
        sweep
            .arm(&TaskHold {
                task_id: "held".into(),
                verb: HoldVerb::Resume,
                args: HoldArgs::default(),
                start_at: Some(start.clone()),
                await_profile: None,
                await_model: None,
                next_check_at: start.clone(),
                expires_at: expiry,
                probe_count: 0,
                note: "scheduled".into(),
                created_at: now.clone(),
                updated_at: now,
            })
            .expect("arm");
        assert_eq!(
            sweep.sweep().expect("early sweep").rechecked,
            Vec::<String>::new()
        );
        clock.advance(Duration::from_secs(60));
        let report = sweep.sweep().expect("release sweep");
        assert_eq!(
            report
                .released
                .iter()
                .map(|hold| hold.task_id.as_str())
                .collect::<Vec<_>>(),
            vec!["held"]
        );
        assert_eq!(
            lifecycle::load_task(&store, "held").unwrap().unwrap().state,
            TaskState::Queued
        );
    }
}
