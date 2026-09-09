//! Recovery for runs the broker was driving when it stopped driving them.
//!
//! Two events leave the same wreckage: the broker process ends (a restart, the
//! desktop app quitting, `make install`) or the machine suspends long enough
//! for a worker to die under it. Both leave rows reading `running` with
//! nothing behind them, and the row is the only record anyone reads.
//!
//! Recovery decides per task against what is actually true of its worker
//! process, then parks the ones worth continuing on a resume hold. The hold is
//! the existing park-and-restart primitive — it survives another crash,
//! carries the attempt count, and releases through the same sweep as a rate
//! limit — so nothing here schedules its own retries.

use std::sync::Arc;

use oga_domain::{
    CompletionCode, HoldArgs, HoldVerb, RestartHoldArgs, TaskCompletion, TaskHold, TaskState,
    TaskWorker,
};
use oga_runner::{Liveness, ProcessIdentity, Signal};
use oga_store::{Store, StoreError};
use rusqlite::params;
use serde_json::json;

use crate::{holds::arm_hold, lifecycle};

/// What woke recovery up. Only the wording the user reads differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileTrigger {
    BrokerStart,
    Wake,
}

impl ReconcileTrigger {
    fn stopped_reason(self) -> &'static str {
        match self {
            Self::BrokerStart => "Stopped when Oga restarted.",
            Self::Wake => "Stopped while this computer was asleep.",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::BrokerStart => "broker_start",
            Self::Wake => "wake",
        }
    }
}

/// A run recovery gives up on after this many restarts in a row that got no
/// further. Past it the task settles `failed` and waits for a person.
const MAX_RESTART_ATTEMPTS: u32 = 3;

const HOLD_EXPIRY_MS: i64 = 24 * 60 * 60 * 1_000;

/// The `running` or `queued` row recovery has to make a call on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InterruptedRun {
    task_id: String,
    has_session: bool,
    worker: Option<TaskWorker>,
    /// Restarts this run has already been through without getting further.
    attempts: u32,
}

/// What is true of the worker process the row points at.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkerVerdict {
    /// This broker is supervising it right now.
    Supervised,
    /// Another broker that is still up spawned it.
    OwnedElsewhere,
    /// Running, but with nobody left holding its pipes.
    Orphaned,
    /// Provably not running, or never spawned at all.
    Gone,
    /// The pid exists and could not be identified.
    Unconfirmed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RunAction {
    LeaveAlone,
    /// Park it on a resume hold and continue the captured session.
    Resume {
        reason: String,
        attempt: u32,
    },
    /// Settle it with a reason a person can act on; `resume` still works.
    Stop {
        reason: String,
    },
    /// Say what was found and touch nothing else.
    Block {
        reason: String,
    },
    /// Restarted too many times without progress.
    GiveUp {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunPlan {
    /// The worker outlived its broker and has to be stopped before the row can
    /// be rewritten; a signal cannot be rolled back, so it goes first.
    reap: bool,
    action: RunAction,
}

/// Live runs this broker is supervising, keyed by task.
pub trait WorkerRegistry {
    fn supervises(&self, task_id: &str) -> bool;
}

/// Whether a recorded pid is still a running process.
pub trait ProcessProbe {
    fn liveness(&self, pid: u32) -> Liveness;
}

pub struct SystemProbe;

impl ProcessProbe for SystemProbe {
    fn liveness(&self, pid: u32) -> Liveness {
        oga_runner::process_liveness(pid)
    }
}

fn verdict(
    run: &InterruptedRun,
    broker_pid: u32,
    registry: &dyn WorkerRegistry,
    probe: &dyn ProcessProbe,
) -> WorkerVerdict {
    if registry.supervises(&run.task_id) {
        return WorkerVerdict::Supervised;
    }
    let Some(worker) = &run.worker else {
        return WorkerVerdict::Gone;
    };
    // Opening this database is not the same as owning the work in it: a second
    // broker holding this worker's pipes is running it healthily, and reaping
    // it here would kill a live run.
    if worker.broker_pid != broker_pid && probe.liveness(worker.broker_pid) == Liveness::Alive {
        return WorkerVerdict::OwnedElsewhere;
    }
    match probe.liveness(worker.pid) {
        Liveness::Alive => WorkerVerdict::Orphaned,
        Liveness::Gone => WorkerVerdict::Gone,
        Liveness::Unknown(reason) => WorkerVerdict::Unconfirmed(reason),
    }
}

fn plan_run(run: &InterruptedRun, verdict: &WorkerVerdict, trigger: ReconcileTrigger) -> RunPlan {
    match verdict {
        WorkerVerdict::Supervised | WorkerVerdict::OwnedElsewhere => RunPlan {
            reap: false,
            action: RunAction::LeaveAlone,
        },
        WorkerVerdict::Unconfirmed(detail) => RunPlan {
            reap: false,
            action: RunAction::Block {
                reason: format!(
                    "Oga could not identify a leftover process, so it left it running \
                     ({detail}). Check it before you resume this task."
                ),
            },
        },
        // Orphaned and gone settle the same way. An orphan's output can never
        // be captured again — it is an unsupervised writer in the user's
        // repository — so it is stopped and the run continues from its
        // captured session instead.
        WorkerVerdict::Orphaned | WorkerVerdict::Gone => {
            let reap = *verdict == WorkerVerdict::Orphaned;
            let reason = trigger.stopped_reason().to_owned();
            if !run.has_session {
                return RunPlan {
                    reap,
                    action: RunAction::Stop { reason },
                };
            }
            let attempt = run.attempts + 1;
            if attempt > MAX_RESTART_ATTEMPTS {
                return RunPlan {
                    reap,
                    action: RunAction::GiveUp {
                        reason: format!(
                            "{reason} Picked up {} times without getting any further, so Oga stopped trying.",
                            run.attempts
                        ),
                    },
                };
            }
            RunPlan {
                reap,
                action: RunAction::Resume { reason, attempt },
            }
        }
    }
}

/// How long a restart hold waits before its release. The first pick-up is
/// immediate; a run that keeps dying gets more room each time.
fn restart_backoff_ms(attempt: u32) -> i64 {
    match attempt {
        0 | 1 => 0,
        2 => 60_000,
        _ => 300_000,
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    pub resumed: Vec<String>,
    pub stopped: Vec<String>,
    pub blocked: Vec<String>,
    pub given_up: Vec<String>,
}

impl ReconcileReport {
    pub fn touched(&self) -> usize {
        self.resumed.len() + self.stopped.len() + self.blocked.len() + self.given_up.len()
    }
}

/// Every task the store still believes is in flight.
fn interrupted_runs(store: &Store) -> Result<Vec<InterruptedRun>, StoreError> {
    let rows: Vec<(String, Option<String>, bool)> = store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT id,worker_json,session_id FROM tasks WHERE state IN ('queued','running')",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?.is_some(),
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?)
    })?;
    rows.into_iter()
        .map(|(task_id, worker_json, has_session)| {
            let attempts = restart_attempts(store, &task_id)?;
            Ok(InterruptedRun {
                worker: worker_json
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok()),
                task_id,
                has_session,
                attempts,
            })
        })
        .collect()
}

/// Restarts this run has been through without getting anywhere. A pick-up that
/// produced any provider activity made progress, so the count starts over —
/// the ceiling is for a run that dies on every attempt, not for a long task
/// that has been interrupted many times over its life.
fn restart_attempts(store: &Store, task_id: &str) -> Result<u32, StoreError> {
    store.with_connection(|connection| {
        let latest: Option<(i64, Option<String>)> = connection
            .query_row(
                "SELECT id,payload FROM task_events WHERE task_id=? AND event_type='run_interrupted' ORDER BY id DESC LIMIT 1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        let Some((event_id, payload)) = latest else {
            return Ok(0);
        };
        let progressed: i64 = connection.query_row(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND id>? AND event_type LIKE 'agent.%'",
            params![task_id, event_id],
            |row| row.get(0),
        )?;
        if progressed > 0 {
            return Ok(0);
        }
        Ok(payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .and_then(|payload| payload.get("attempt").and_then(serde_json::Value::as_u64))
            .unwrap_or(1) as u32)
    })
}

/// Applies one recovery pass. Idempotent by construction: a task this broker
/// is supervising is never touched, the settle only fires on a row still
/// reading `queued` or `running`, and arming the hold is what makes the resume
/// happen — so a second pass finds a `pending` row and nothing left to do.
pub fn reconcile(
    store: &Arc<Store>,
    trigger: ReconcileTrigger,
    registry: &dyn WorkerRegistry,
    probe: &dyn ProcessProbe,
) -> Result<ReconcileReport, StoreError> {
    let broker_pid = std::process::id();
    let mut report = ReconcileReport::default();
    for run in interrupted_runs(store)? {
        let verdict = verdict(&run, broker_pid, registry, probe);
        let plan = plan_run(&run, &verdict, trigger);
        if plan.reap
            && let Some(worker) = &run.worker
        {
            reap(worker);
        }
        match plan.action {
            RunAction::LeaveAlone => {}
            RunAction::Block { reason } => {
                settle(
                    store,
                    &run.task_id,
                    TaskState::Blocked,
                    &reason,
                    trigger,
                    None,
                )?;
                report.blocked.push(run.task_id);
            }
            RunAction::Stop { reason } => {
                settle(
                    store,
                    &run.task_id,
                    TaskState::Cancelled,
                    &reason,
                    trigger,
                    None,
                )?;
                report.stopped.push(run.task_id);
            }
            RunAction::GiveUp { reason } => {
                settle(
                    store,
                    &run.task_id,
                    TaskState::Failed,
                    &reason,
                    trigger,
                    None,
                )?;
                report.given_up.push(run.task_id);
            }
            RunAction::Resume { reason, attempt } => {
                settle(
                    store,
                    &run.task_id,
                    TaskState::Cancelled,
                    &reason,
                    trigger,
                    Some(attempt),
                )?;
                arm_hold(store, &restart_hold(&run.task_id, &reason, attempt))?;
                report.resumed.push(run.task_id);
            }
        }
    }
    Ok(report)
}

fn reap(worker: &TaskWorker) {
    let identity = ProcessIdentity {
        pid: worker.pid,
        pgid: worker.pgid,
    };
    oga_runner::signal_group(&identity, Signal::Terminate);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        if identity.liveness() == Liveness::Alive {
            oga_runner::signal_group(&identity, Signal::Kill);
        }
    });
}

fn settle(
    store: &Store,
    task_id: &str,
    state: TaskState,
    reason: &str,
    trigger: ReconcileTrigger,
    attempt: Option<u32>,
) -> Result<(), StoreError> {
    let now = lifecycle::now_iso();
    let completion = TaskCompletion {
        exit_code: None,
        blocked: true,
        code: match state {
            TaskState::Cancelled => CompletionCode::Cancelled,
            _ => CompletionCode::WorkerError,
        },
        reason: Some(reason.to_owned()),
        suggested_scope: None,
        resets_at: None,
        asserted_completion: None,
        dependency_blocked: None,
    };
    let completion_json = serde_json::to_string(&completion)
        .map_err(|error| StoreError::Refusal(error.to_string()))?;
    store.transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state=?,error=?,completion_json=?,worker_json=NULL,updated_at=? WHERE id=? AND state IN ('queued','running')",
            params![state.as_str(), reason, completion_json, now, task_id],
        )?;
        if changed != 1 {
            return Ok(());
        }
        tx.execute(
            "UPDATE task_turns SET status='interrupted',ended_at=? WHERE task_id=? AND status='running'",
            params![now, task_id],
        )?;
        let mut payload = json!({"reason": reason, "trigger": trigger.label()});
        if let Some(attempt) = attempt {
            payload["attempt"] = json!(attempt);
        }
        crate::append_event_tx(tx, task_id, "run_interrupted", state, payload, &now)?;
        Ok(())
    })
}

fn restart_hold(task_id: &str, reason: &str, attempt: u32) -> TaskHold {
    let now_ms = oga_routing::now_ms();
    let now = oga_routing::format_rfc3339_ms(now_ms);
    let due = oga_routing::format_rfc3339_ms(now_ms.saturating_add(restart_backoff_ms(attempt)));
    TaskHold {
        task_id: task_id.to_owned(),
        verb: HoldVerb::Resume,
        args: HoldArgs {
            restart: Some(RestartHoldArgs { attempt }),
            ..HoldArgs::default()
        },
        start_at: Some(due.clone()),
        await_profile: None,
        await_model: None,
        next_check_at: due,
        expires_at: oga_routing::format_rfc3339_ms(now_ms.saturating_add(HOLD_EXPIRY_MS)),
        probe_count: 0,
        note: reason.to_owned(),
        created_at: now.clone(),
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[derive(Default)]
    struct FakeRegistry {
        supervised: HashSet<String>,
    }

    impl WorkerRegistry for FakeRegistry {
        fn supervises(&self, task_id: &str) -> bool {
            self.supervised.contains(task_id)
        }
    }

    #[derive(Default)]
    struct FakeProbe {
        liveness: HashMap<u32, Liveness>,
    }

    impl ProcessProbe for FakeProbe {
        fn liveness(&self, pid: u32) -> Liveness {
            self.liveness.get(&pid).cloned().unwrap_or(Liveness::Gone)
        }
    }

    fn worker(pid: u32, broker_pid: u32) -> TaskWorker {
        TaskWorker {
            pid,
            pgid: pid as i32,
            broker_pid,
            started_at: "2026-01-01T00:00:00.000Z".into(),
        }
    }

    fn run(worker: Option<TaskWorker>, has_session: bool, attempts: u32) -> InterruptedRun {
        InterruptedRun {
            task_id: "task".into(),
            has_session,
            worker,
            attempts,
        }
    }

    #[test]
    fn a_run_this_broker_supervises_is_not_recovered() {
        let registry = FakeRegistry {
            supervised: HashSet::from(["task".to_owned()]),
        };
        let run = run(Some(worker(10, 1)), true, 0);

        let verdict = verdict(&run, 1, &registry, &FakeProbe::default());

        assert_eq!(verdict, WorkerVerdict::Supervised);
        assert_eq!(
            plan_run(&run, &verdict, ReconcileTrigger::BrokerStart).action,
            RunAction::LeaveAlone
        );
    }

    #[test]
    fn a_worker_another_live_broker_owns_is_left_alone() {
        let probe = FakeProbe {
            liveness: HashMap::from([(10, Liveness::Alive), (99, Liveness::Alive)]),
        };
        let run = run(Some(worker(10, 99)), true, 0);

        let verdict = verdict(&run, 1, &FakeRegistry::default(), &probe);

        assert_eq!(verdict, WorkerVerdict::OwnedElsewhere);
        assert!(!plan_run(&run, &verdict, ReconcileTrigger::BrokerStart).reap);
    }

    #[test]
    fn a_worker_whose_broker_is_gone_is_reaped_and_resumed() {
        let probe = FakeProbe {
            liveness: HashMap::from([(10, Liveness::Alive)]),
        };
        let run = run(Some(worker(10, 99)), true, 0);

        let verdict = verdict(&run, 1, &FakeRegistry::default(), &probe);
        let plan = plan_run(&run, &verdict, ReconcileTrigger::BrokerStart);

        assert_eq!(verdict, WorkerVerdict::Orphaned);
        assert!(plan.reap);
        assert_eq!(
            plan.action,
            RunAction::Resume {
                reason: "Stopped when Oga restarted.".into(),
                attempt: 1,
            }
        );
    }

    #[test]
    fn a_dead_worker_without_a_session_stops_with_a_reason() {
        let run = run(Some(worker(10, 1)), false, 0);

        let plan = plan_run(
            &run,
            &verdict(&run, 1, &FakeRegistry::default(), &FakeProbe::default()),
            ReconcileTrigger::BrokerStart,
        );

        assert!(!plan.reap);
        assert_eq!(
            plan.action,
            RunAction::Stop {
                reason: "Stopped when Oga restarted.".into()
            }
        );
    }

    #[test]
    fn a_row_that_never_spawned_a_worker_is_treated_as_gone() {
        let run = run(None, true, 0);

        let verdict = verdict(&run, 1, &FakeRegistry::default(), &FakeProbe::default());

        assert_eq!(verdict, WorkerVerdict::Gone);
    }

    #[test]
    fn an_unidentifiable_process_is_reported_rather_than_signalled() {
        let probe = FakeProbe {
            liveness: HashMap::from([(10, Liveness::Unknown("permission denied".into()))]),
        };
        let run = run(Some(worker(10, 1)), true, 0);

        let plan = plan_run(
            &run,
            &verdict(&run, 1, &FakeRegistry::default(), &probe),
            ReconcileTrigger::BrokerStart,
        );

        assert!(!plan.reap);
        assert!(matches!(plan.action, RunAction::Block { .. }));
    }

    #[test]
    fn waking_up_names_sleep_rather_than_a_restart() {
        let run = run(Some(worker(10, 1)), true, 0);

        let plan = plan_run(
            &run,
            &verdict(&run, 1, &FakeRegistry::default(), &FakeProbe::default()),
            ReconcileTrigger::Wake,
        );

        assert_eq!(
            plan.action,
            RunAction::Resume {
                reason: "Stopped while this computer was asleep.".into(),
                attempt: 1,
            }
        );
    }

    #[test]
    fn a_run_that_keeps_dying_settles_instead_of_restarting_forever() {
        let run = run(Some(worker(10, 1)), true, MAX_RESTART_ATTEMPTS);

        let plan = plan_run(
            &run,
            &verdict(&run, 1, &FakeRegistry::default(), &FakeProbe::default()),
            ReconcileTrigger::BrokerStart,
        );

        assert!(matches!(plan.action, RunAction::GiveUp { .. }));
    }

    #[test]
    fn the_first_pick_up_is_immediate_and_later_ones_wait() {
        assert_eq!(restart_backoff_ms(1), 0);
        assert!(restart_backoff_ms(2) > 0);
        assert!(restart_backoff_ms(3) > restart_backoff_ms(2));
    }

    fn store_with_running_task(session_id: Option<&str>) -> (tempfile::TempDir, Arc<Store>) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        store
            .repositories()
            .profiles()
            .insert(
                &oga_domain::Profile {
                    id: "profile".into(),
                    label: "profile".into(),
                    provider: oga_domain::Provider::Claude,
                    default_model: "model".into(),
                    enabled: true,
                    env: Default::default(),
                    capabilities: vec![],
                    command: None,
                },
                "2026-01-01T00:00:00.000Z",
            )
            .expect("profile");
        let mut task = crate::holds::tests::task(
            "run",
            &directory.path().to_string_lossy(),
            TaskState::Running,
        );
        task.session_id = session_id.map(str::to_owned);
        store.repositories().tasks().insert(&task).expect("task");
        (directory, store)
    }

    #[test]
    fn a_dead_run_with_a_session_is_parked_ready_to_pick_up() {
        let (_directory, store) = store_with_running_task(Some("session"));

        let report = reconcile(
            &store,
            ReconcileTrigger::BrokerStart,
            &FakeRegistry::default(),
            &FakeProbe::default(),
        )
        .expect("reconcile");

        assert_eq!(report.resumed, vec!["run".to_owned()]);
        let task = lifecycle::load_task(&store, "run").unwrap().unwrap();
        assert_eq!(task.state, TaskState::Pending);
        assert_eq!(
            task.hold.expect("hold").kind,
            oga_domain::HoldViewKind::Restart
        );
    }

    #[test]
    fn a_dead_run_without_a_session_stops_where_a_person_can_resume_it() {
        let (_directory, store) = store_with_running_task(None);

        let report = reconcile(
            &store,
            ReconcileTrigger::BrokerStart,
            &FakeRegistry::default(),
            &FakeProbe::default(),
        )
        .expect("reconcile");

        assert_eq!(report.stopped, vec!["run".to_owned()]);
        let task = lifecycle::load_task(&store, "run").unwrap().unwrap();
        assert_eq!(task.state, TaskState::Cancelled);
        assert_eq!(task.error.as_deref(), Some("Stopped when Oga restarted."));
    }

    #[test]
    fn a_second_pass_finds_nothing_left_to_pick_up() {
        let (_directory, store) = store_with_running_task(Some("session"));
        let pass = || {
            reconcile(
                &store,
                ReconcileTrigger::BrokerStart,
                &FakeRegistry::default(),
                &FakeProbe::default(),
            )
            .expect("reconcile")
        };

        pass();

        assert_eq!(pass(), ReconcileReport::default());
    }
}
