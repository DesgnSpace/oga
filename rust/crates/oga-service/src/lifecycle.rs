//! Queued-to-running transitions, provider execution, settlement, and release.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::Path,
    sync::Arc,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::authorization;
use crate::dependencies;
use crate::prompt::{
    WorkerOutcome, WorkerPromptInput, assemble_worker_prompt, interpret_worker_outcome,
};
use oga_domain::{
    CompletionCode, FailureCode, HoldArgs, HoldViewKind, Profile, ProfileFailure, ProfileSuccess,
    Provider, Task, TaskAttempt, TaskCompletion, TaskHoldView, TaskKind, TaskScope, TaskState,
    TaskWorker, TaskWorktree,
};
use oga_providers::{
    CommandOptions, ParsedEvent, Usage, final_text, resume_command_for_with_options,
};
use oga_runner::{ProviderRunner, RunRequest, RunResult, RunnerError, RunningProcess, Termination};
use oga_store::{Store, StoreError};
use rusqlite::{OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::mpsc;

const MAX_ABORT_RETRIES: usize = 2;

/// Errors returned by the lifecycle runner before a terminal row is written.
#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error("task cannot start: {0}")]
    Refusal(String),
    #[error("store error: {0}")]
    Store(String),
    #[error("runner error: {0}")]
    Runner(String),
}

impl From<StoreError> for LifecycleError {
    fn from(error: StoreError) -> Self {
        Self::Store(error.to_string())
    }
}

impl From<RunnerError> for LifecycleError {
    fn from(error: RunnerError) -> Self {
        Self::Runner(error.to_string())
    }
}

/// The settled task and the interpretation that produced it.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub task: Task,
    pub worker: WorkerOutcome,
}

/// Live provider processes owned by this broker instance.
#[derive(Clone, Default)]
pub struct ActiveRuns {
    processes: Arc<Mutex<HashMap<String, Arc<RunningProcess>>>>,
}

#[derive(Default)]
pub(crate) struct RunOptions {
    pub(crate) session_id: Option<String>,
    pub(crate) active: ActiveRuns,
}

impl ActiveRuns {
    pub(crate) fn insert(&self, task_id: &str, process: Arc<RunningProcess>) {
        self.processes
            .lock()
            .expect("active run map is not poisoned")
            .insert(task_id.to_owned(), process);
    }

    pub(crate) fn get(&self, task_id: &str) -> Option<Arc<RunningProcess>> {
        self.processes
            .lock()
            .expect("active run map is not poisoned")
            .get(task_id)
            .cloned()
    }

    pub(crate) fn remove(&self, task_id: &str, process: &Arc<RunningProcess>) {
        let mut processes = self
            .processes
            .lock()
            .expect("active run map is not poisoned");
        if processes
            .get(task_id)
            .is_some_and(|current| Arc::ptr_eq(current, process))
        {
            processes.remove(task_id);
        }
    }
}

impl crate::reconcile::WorkerRegistry for ActiveRuns {
    fn supervises(&self, task_id: &str) -> bool {
        self.get(task_id).is_some()
    }
}

pub(crate) fn load_task(store: &Store, task_id: &str) -> Result<Option<Task>, StoreError> {
    store.with_connection(|connection| {
        let mut task = connection
            .query_row(
                "SELECT id,kind,profile_id,model,prompt,shipped_prompt,cwd,branch,origin_cwd,worktree_path,worktree_branch,worktree_links_json,state,output,error,question,parent_task_id,orchestrator_id,caller_id,scope_json,grant_id,allow_questions,timeout_ms,effort,effort_actual,tldr,title,session_id,completion_json,attempts_json,cost_usd,cost_usd_estimated,turns,archived_at,created_at,updated_at,attachments_json FROM tasks WHERE id=?",
                [task_id],
                task_from_row,
            )
            .optional()
            .map_err(StoreError::from)?;
        if let Some(task) = &mut task {
            task.hold = connection
                .query_row(
                    "SELECT verb,start_at,note,expires_at,args_json FROM task_holds WHERE task_id=?",
                    [task_id],
                    |row| {
                        let args: HoldArgs =
                            serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default();
                        let kind = match row.get::<_, String>(0)?.as_str() {
                            _ if args.scheduled == Some(true) => HoldViewKind::Time,
                            "delegate" => HoldViewKind::Dependency,
                            _ if args.restart.is_some() => HoldViewKind::Restart,
                            _ => HoldViewKind::Time,
                        };
                        Ok(TaskHoldView {
                            kind,
                            until: row.get(1)?,
                            waiting_on: None,
                            note: row.get(2)?,
                            expires_at: row.get(3)?,
                        })
                    },
                )
                .optional()?;
        }
        Ok(task)
    })
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let kind = decode_json::<TaskKind>(&format!("\"{}\"", row.get::<_, String>(1)?), 1)?;
    let state = decode_json::<TaskState>(&format!("\"{}\"", row.get::<_, String>(12)?), 12)?;
    let scope = decode_json::<TaskScope>(&row.get::<_, String>(19)?, 19)?;
    let completion = row
        .get::<_, Option<String>>(28)?
        .map(|value| decode_json(&value, 28))
        .transpose()?;
    let attempts = row
        .get::<_, Option<String>>(29)?
        .map(|value| decode_json::<Vec<TaskAttempt>>(&value, 29))
        .transpose()?
        .unwrap_or_default();
    let worktree = match (
        row.get::<_, Option<String>>(8)?,
        row.get::<_, Option<String>>(9)?,
        row.get::<_, Option<String>>(10)?,
    ) {
        (None, _, _) => None,
        (Some(origin_cwd), Some(path), Some(branch)) => Some(TaskWorktree {
            origin_cwd,
            path,
            branch,
            links: row
                .get::<_, Option<String>>(11)?
                .map(|value| decode_json(&value, 11))
                .transpose()?,
        }),
        _ => {
            return Err(rusqlite::Error::InvalidParameterName(
                "incomplete task worktree".into(),
            ));
        }
    };
    let attachments = row
        .get::<_, Option<String>>(36)?
        .map(|value| decode_json::<Vec<String>>(&value, 36))
        .transpose()?
        .unwrap_or_default();
    Ok(Task {
        id: row.get(0)?,
        kind: Some(kind),
        profile_id: row.get(2)?,
        model: row.get(3)?,
        prompt: row.get(4)?,
        shipped_prompt: row.get(5)?,
        cwd: row.get(6)?,
        branch: row.get(7)?,
        worktree,
        worktree_label: None,
        state,
        created_at: row.get(34)?,
        updated_at: row.get(35)?,
        output: row.get(13)?,
        error: row.get(14)?,
        question: row.get(15)?,
        parent_task_id: row.get(16)?,
        orchestrator_id: row.get(17)?,
        scope,
        grant_id: row.get(20)?,
        allow_questions: row.get::<_, i64>(21)? != 0,
        timeout_ms: row.get(22)?,
        effort: row.get(23)?,
        effort_actual: row.get(24)?,
        tldr: row.get(25)?,
        title: row.get(26)?,
        session_id: row.get(27)?,
        completion,
        attempts,
        cost_usd: row.get(30)?,
        cost_usd_estimated: row.get::<_, Option<i64>>(31)?.unwrap_or(0) != 0,
        turns: row.get(32)?,
        archived_at: row.get(33)?,
        queued_follow_ups: None,
        queued_follow_up_items: None,
        hold: None,
        attachments,
    })
}

fn decode_json<T: DeserializeOwned>(value: &str, column: usize) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

/// Run a queued task with the default prompt context.
pub async fn run_task(
    store: Arc<Store>,
    runner: ProviderRunner,
    task: Task,
    profile: Profile,
) -> Result<RunOutcome, LifecycleError> {
    let prompt = WorkerPromptInput {
        task: task.prompt.clone(),
        allow_questions: task.allow_questions,
        scope: Some(task.scope.clone()),
        worker_prompt: oga_config::DEFAULT_WORKER_PROMPT.to_owned(),
        attribution: crate::prompt::attribution_for_task(&task, profile.provider),
        identity: crate::prompt::PromptIdentity::new(
            &task.id,
            profile.provider,
            &task.model,
            task.effort.as_deref(),
        ),
        ..WorkerPromptInput::default()
    };
    run_task_and_release(store, runner, task, profile, prompt).await
}

/// Run one task and then drain any dependency tasks it releases.
pub async fn run_task_and_release(
    store: Arc<Store>,
    runner: ProviderRunner,
    task: Task,
    profile: Profile,
    prompt: WorkerPromptInput,
) -> Result<RunOutcome, LifecycleError> {
    run_task_and_release_with_active(store, runner, task, profile, prompt, RunOptions::default())
        .await
}

/// Run one task, then drain the dependents its ending releases. The session id
/// belongs to the first run only: a continuation resumes its own session, and
/// every dependent that follows starts a fresh one.
pub(crate) async fn run_task_and_release_with_active(
    store: Arc<Store>,
    runner: ProviderRunner,
    task: Task,
    profile: Profile,
    prompt: WorkerPromptInput,
    options: RunOptions,
) -> Result<RunOutcome, LifecycleError> {
    let RunOptions {
        mut session_id,
        active,
    } = options;
    let mut pending = vec![(task, profile, prompt)];
    let mut last = None;
    while let Some((task, profile, prompt)) = pending.pop() {
        let outcome = run_task_with_session_and_active(
            store.clone(),
            runner.clone(),
            task,
            profile,
            prompt,
            RunOptions {
                session_id: session_id.take(),
                active: active.clone(),
            },
        )
        .await?;
        let released = release_dependents(&store, &outcome.task)?;
        for queued in released {
            let Some(profile) = store.repositories().profiles().get(&queued.profile_id)? else {
                block_queued_task(&store, &queued.id, "unknown profile for dependent task")?;
                continue;
            };
            let attribution = crate::prompt::attribution_for_task(&queued, profile.provider);
            let identity = crate::prompt::PromptIdentity::new(
                &queued.id,
                profile.provider,
                &queued.model,
                queued.effort.as_deref(),
            );
            pending.push((
                queued.clone(),
                profile,
                WorkerPromptInput {
                    task: queued.prompt.clone(),
                    allow_questions: queued.allow_questions,
                    scope: Some(queued.scope.clone()),
                    worker_prompt: oga_config::DEFAULT_WORKER_PROMPT.to_owned(),
                    attribution,
                    identity,
                    ..WorkerPromptInput::default()
                },
            ));
        }
        last = Some(outcome);
    }
    last.ok_or_else(|| LifecycleError::Refusal("no task was queued".into()))
}

/// Claim, execute, persist, and release a task's provider run.
pub async fn run_task_with_prompt(
    store: Arc<Store>,
    runner: ProviderRunner,
    task: Task,
    profile: Profile,
    prompt: WorkerPromptInput,
) -> Result<RunOutcome, LifecycleError> {
    run_task_with_session_and_active(store, runner, task, profile, prompt, RunOptions::default())
        .await
}

pub(crate) async fn run_task_with_session_and_active(
    store: Arc<Store>,
    runner: ProviderRunner,
    mut task: Task,
    profile: Profile,
    prompt: WorkerPromptInput,
    options: RunOptions,
) -> Result<RunOutcome, LifecycleError> {
    authorization::check_model_enabled(
        &store,
        &authorization::settings_cwd(&task),
        &task.profile_id,
        &task.model,
    )
    .map_err(LifecycleError::Refusal)?;
    let shipped_prompt = if options.session_id.is_some() {
        prompt.task.clone()
    } else {
        assemble_worker_prompt(&prompt)
    };
    let hook_url = hook_url_for(&task.id);
    let mcp_config = worker_mcp_config(&profile, &task.id);
    let command_options = CommandOptions {
        hook_url: Some(hook_url.as_str()),
        effort: task.effort.as_deref(),
        allowed_tools: None,
        mcp_config: mcp_config.as_deref(),
    };
    let initial_command = if let Some(session_id) = options.session_id.as_deref() {
        resume_command_for_with_options(
            &profile,
            &shipped_prompt,
            &task.cwd,
            session_id,
            Some(&task.model),
            command_options,
        )
        .map_err(|error| LifecycleError::Refusal(error.to_string()))?
    } else {
        oga_providers::command_for_with_options(
            &profile,
            &shipped_prompt,
            &task.cwd,
            Some(&task.model),
            command_options,
        )
    };
    let turn_id = claim_task(&store, &task.id, &shipped_prompt)?;
    task.state = TaskState::Running;
    task.shipped_prompt = Some(shipped_prompt.clone());
    let mut session_id = options.session_id.clone();
    let mut retries = 0;
    let mut accumulated_usage = Usage::default();
    loop {
        let command = if retries == 0 {
            initial_command.clone()
        } else {
            resume_command_for_with_options(
                &profile,
                &shipped_prompt,
                &task.cwd,
                session_id
                    .as_deref()
                    .expect("abort retry has a captured session"),
                Some(&task.model),
                command_options,
            )
            .map_err(|error| LifecycleError::Refusal(error.to_string()))?
        };
        let request = RunRequest::from_command(profile.provider, command, &task.cwd)
            .with_env(worker_env(&task.id, &task.cwd))
            .with_timeout(task.timeout_ms.map(Duration::from_millis));
        let process = match runner.spawn(request).await {
            Ok(run) => run,
            Err(error) => {
                let worker = failed_worker(&error);
                let settled = settle_task(&store, &task, turn_id, None, None, &worker, None)?;
                record_profile_outcome(&store, &task, &worker)?;
                return Ok(RunOutcome {
                    task: settled,
                    worker,
                });
            }
        };
        let process = Arc::new(process);
        let process_events = matches!(
            profile.provider,
            Provider::Codex | Provider::OpenCode | Provider::OpenCode2 | Provider::Antigravity
        )
        .then(|| {
            process
                .take_provider_events()
                .expect("provider events are available")
        });
        let resumed_session = session_id.as_deref();
        let now = now_iso();
        let mut spawn_payload = json!({
            "provider": profile.provider,
            "model": &task.model,
            "pid": process.identity().pid,
        });
        if let Some(session_id) = session_id.as_deref() {
            spawn_payload["resumedSession"] = json!(session_id);
        }
        let worker_identity = TaskWorker {
            pid: process.identity().pid,
            pgid: process.identity().pgid,
            broker_pid: std::process::id(),
            started_at: now.clone(),
        };
        let worker_json = encode(&worker_identity)?;
        if let Err(error) = store.transaction(|tx| {
            tx.execute(
                "UPDATE tasks SET worker_json=? WHERE id=?",
                params![worker_json, task.id],
            )?;
            append_event_tx(
                tx,
                &task.id,
                "worker_spawned",
                TaskState::Running,
                spawn_payload,
                &now,
                Some(turn_id),
            )?;
            Ok(())
        }) {
            process.cancel().await;
            return Err(error.into());
        }
        options.active.insert(&task.id, process.clone());
        if load_task(&store, &task.id)?.is_none_or(|current| current.state != TaskState::Running) {
            process.cancel().await;
        }
        let process_result = if let Some(process_events) = process_events {
            wait_for_process_events(
                &store,
                &task,
                turn_id,
                resumed_session,
                &process,
                process_events,
            )
            .await
        } else {
            Ok((process.wait().await, LiveEventCapture::default()))
        };
        options.active.remove(&task.id, &process);
        let (run, live_events) = process_result?;
        let mut run = match run {
            Ok(run) => run,
            Err(error) => {
                let worker = failed_worker(&error);
                let settled = settle_task(&store, &task, turn_id, None, None, &worker, None)?;
                record_profile_outcome(&store, &task, &worker)?;
                return Ok(RunOutcome {
                    task: settled,
                    worker,
                });
            }
        };
        apply_estimated_cost(&mut run.usage, profile.provider, &task.model).await;
        let retry_session = session_id.clone().or_else(|| run.session_id.clone());
        if aborted_turn(&run.stdout).is_some()
            && profile.command.is_none()
            && retry_session.is_some()
            && retries < MAX_ABORT_RETRIES
        {
            append_retry_events(
                &store,
                &task,
                turn_id,
                &run,
                resumed_session,
                profile.provider,
                retries + 1,
                &live_events,
            )?;
            add_usage(&mut accumulated_usage, &run.usage);
            session_id = retry_session;
            retries += 1;
            continue;
        }
        if retries > 0 {
            add_usage(&mut accumulated_usage, &run.usage);
            run.usage = accumulated_usage;
        }
        let worker = interpret_run(&profile, &run, retries);
        let settled = settle_task(
            &store,
            &task,
            turn_id,
            Some(&run),
            resumed_session,
            &worker,
            Some(&live_events),
        )?;
        record_profile_outcome(&store, &task, &worker)?;
        return Ok(RunOutcome {
            task: settled,
            worker,
        });
    }
}

fn interpret_run(profile: &Profile, run: &RunResult, retries: usize) -> WorkerOutcome {
    let aborted = aborted_turn(&run.stdout);
    let mut outcome = match run.termination {
        Termination::TimedOut => {
            let reason = "provider run timed out".to_owned();
            WorkerOutcome {
                state: TaskState::Failed,
                output: final_text(profile.provider, &run.stdout),
                question: None,
                error: Some(reason.clone()),
                completion: completion(run.exit_code, true, CompletionCode::Timeout, Some(reason)),
            }
        }
        Termination::Cancelled => {
            let reason = "provider run was cancelled".to_owned();
            WorkerOutcome {
                state: TaskState::Cancelled,
                output: final_text(profile.provider, &run.stdout),
                question: None,
                error: Some(reason.clone()),
                completion: completion(
                    run.exit_code,
                    true,
                    CompletionCode::Cancelled,
                    Some(reason),
                ),
            }
        }
        Termination::Exited => interpret_worker_outcome(
            run.exit_code,
            final_text(profile.provider, &run.stdout),
            run.stderr.clone(),
            aborted,
        ),
    };
    if outcome.completion.code == CompletionCode::Aborted {
        let guidance = if retries == 0 {
            "with no session to pick back up".to_owned()
        } else {
            format!(
                "picked back up {} {} before the provider kept cutting out",
                retries,
                if retries == 1 { "time" } else { "times" }
            )
        };
        let reason = format!("The turn was cut off mid-response and {guidance}.");
        outcome.error = Some(reason.clone());
        outcome.completion.reason = Some(reason);
    }
    outcome
}

fn failed_worker(error: &RunnerError) -> WorkerOutcome {
    let reason = error.to_string();
    WorkerOutcome {
        state: TaskState::Failed,
        output: String::new(),
        question: None,
        error: Some(reason.clone()),
        completion: completion(None, true, CompletionCode::WorkerError, Some(reason)),
    }
}

#[derive(Default)]
struct LiveEventCapture {
    persisted_provider_events: usize,
    session_event_written: bool,
}

async fn wait_for_process_events(
    store: &Store,
    task: &Task,
    turn_id: i64,
    resumed_session: Option<&str>,
    process: &RunningProcess,
    mut events: mpsc::UnboundedReceiver<ParsedEvent>,
) -> Result<(Result<RunResult, RunnerError>, LiveEventCapture), LifecycleError> {
    let mut capture = LiveEventCapture::default();
    let mut receiving = true;
    let wait = process.wait();
    tokio::pin!(wait);
    loop {
        if !receiving {
            return Ok((wait.await, capture));
        }
        tokio::select! {
            result = &mut wait => {
                while let Ok(event) = events.try_recv() {
                    persist_live_provider_event(
                        store,
                        task,
                        turn_id,
                        resumed_session,
                        event,
                        &mut capture,
                    ).await?;
                }
                return Ok((result, capture));
            }
            event = events.recv() => match event {
                Some(event) => persist_live_provider_event(
                    store,
                    task,
                    turn_id,
                    resumed_session,
                    event,
                    &mut capture,
                ).await?,
                None => receiving = false,
            },
        }
    }
}

async fn persist_live_provider_event(
    store: &Store,
    task: &Task,
    turn_id: i64,
    resumed_session: Option<&str>,
    event: ParsedEvent,
    capture: &mut LiveEventCapture,
) -> Result<(), LifecycleError> {
    let event = enrich_codex_file_change(task, event).await;
    let now = now_iso();
    let mut session_event_written = capture.session_event_written;
    store.transaction(|tx| {
        append_provider_event_tx(
            tx,
            task,
            turn_id,
            &event,
            resumed_session,
            &mut session_event_written,
            &now,
        )?;
        Ok(())
    })?;
    capture.persisted_provider_events += 1;
    capture.session_event_written = session_event_written;
    Ok(())
}

async fn enrich_codex_file_change(task: &Task, mut event: ParsedEvent) -> ParsedEvent {
    if event.provider != Provider::Codex
        || event.payload.get("type").and_then(Value::as_str) != Some("item.completed")
        || event
            .payload
            .get("item")
            .and_then(Value::as_object)
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            != Some("file_change")
    {
        return event;
    }

    let Ok(diff) = oga_worktree::task_diff(task).await else {
        return event;
    };
    let cwd = Path::new(&task.cwd);
    let Some(changes) = event
        .payload
        .get_mut("item")
        .and_then(Value::as_object_mut)
        .and_then(|item| item.get_mut("changes"))
        .and_then(Value::as_array_mut)
    else {
        return event;
    };
    for change in changes {
        let Some(change) = change.as_object_mut() else {
            continue;
        };
        let Some(path) = change.get("path").and_then(Value::as_str) else {
            continue;
        };
        let Some(path) = relative_codex_path(cwd, path) else {
            continue;
        };
        let Some(file) = diff.files.iter().find(|file| file.path == path) else {
            continue;
        };
        let Some(patch) = file.patch.as_deref() else {
            continue;
        };
        change.insert("patch".into(), Value::String(patch.to_owned()));
    }
    event
}

fn relative_codex_path(cwd: &Path, raw: &str) -> Option<String> {
    let path = Path::new(raw);
    let relative = if path.is_absolute() {
        path.strip_prefix(cwd).ok()?
    } else {
        path
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    let relative = relative.strip_prefix("./").unwrap_or(&relative);
    (!relative.is_empty() && relative != ".." && !relative.starts_with("../"))
        .then(|| relative.to_owned())
}

fn settle_task(
    store: &Store,
    task: &Task,
    turn_id: i64,
    run: Option<&RunResult>,
    resumed_session: Option<&str>,
    worker: &WorkerOutcome,
    live_events: Option<&LiveEventCapture>,
) -> Result<Task, LifecycleError> {
    let now = now_iso();
    let mut worker = worker.clone();
    if worker.state == TaskState::Blocked
        && worker.output.contains("https://github.com/")
        && worker.output.contains("/pull/")
        && has_invalid_routes(store, &task.id)?
    {
        worker.state = TaskState::Completed;
        worker.error = None;
        worker.completion = completion(
            None,
            false,
            CompletionCode::Completed,
            Some("Pull request opened, but one or more source routes were rejected.".into()),
        );
        worker
            .output
            .push_str("\n\nPull request opened, but one or more source routes were rejected.");
    }
    let completion = encode(&worker.completion)?;
    let session_id = run.and_then(|run| run.session_id.as_deref());
    let usage = run.map(|run| &run.usage);
    store.transaction(|tx| {
        append_provider_events(
            tx,
            task,
            turn_id,
            run,
            resumed_session,
            &now,
            live_events.map_or(0, |events| events.persisted_provider_events),
            live_events.is_some_and(|events| events.session_event_written),
        )?;
        let updated = tx.execute(
            "UPDATE tasks SET state=?,output=?,error=?,question=?,completion_json=?,worker_json=NULL,session_id=COALESCE(?,session_id),cost_usd=CASE WHEN ? IS NULL THEN cost_usd ELSE COALESCE(cost_usd,0)+? END,cost_usd_estimated=CASE WHEN ? IS NULL THEN cost_usd_estimated ELSE (COALESCE(cost_usd_estimated,0) OR ?) END,turns=CASE WHEN ? IS NULL THEN turns ELSE COALESCE(turns,0)+? END,tokens_in=CASE WHEN ? IS NULL THEN tokens_in ELSE COALESCE(tokens_in,0)+? END,tokens_out=CASE WHEN ? IS NULL THEN tokens_out ELSE COALESCE(tokens_out,0)+? END,spend_at=?,updated_at=? WHERE id=? AND state='running' AND EXISTS (SELECT 1 FROM task_turns WHERE task_turns.id=? AND task_turns.task_id=tasks.id AND task_turns.status='running')",
            params![
                worker.state.as_str(),
                worker.output,
                worker.error,
                worker.question,
                completion,
                session_id,
                usage.and_then(|usage| usage.cost_usd),
                usage.and_then(|usage| usage.cost_usd),
                usage.and_then(|usage| usage.cost_usd),
                usage.is_some_and(|usage| usage.cost_usd_estimated),
                usage.and_then(|usage| usage.turns.map(|value| value.round() as i64)),
                usage.and_then(|usage| usage.turns.map(|value| value.round() as i64)),
                usage.and_then(|usage| usage.tokens_in.map(|value| value.round() as i64)),
                usage.and_then(|usage| usage.tokens_in.map(|value| value.round() as i64)),
                usage.and_then(|usage| usage.tokens_out.map(|value| value.round() as i64)),
                usage.and_then(|usage| usage.tokens_out.map(|value| value.round() as i64)),
                now,
                now,
                task.id,
                turn_id,
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::Refusal(format!(
                "task is no longer running: {}",
                task.id
            )));
        }
        let turn_status = turn_status(worker.state);
        tx.execute(
            "UPDATE task_turns SET status=?,ended_at=? WHERE id=? AND status='running'",
            params![turn_status, now, turn_id],
        )?;
        let mut completion_payload = json!({"completion": worker.completion});
        if worker.state == TaskState::Completed {
            let attempt = task.attempts.len() + 1;
            let recorded = has_learned_routes(tx, &task.id, attempt)?;
            completion_payload["learnRoutes"] = json!({
                "status": if recorded { "recorded" } else { "missing" },
                "attempt": attempt,
            });
        }
        if let Some(error) = &worker.error {
            completion_payload["error"] = json!(error);
        }
        if let Some(question) = &worker.question {
            completion_payload["question"] = json!(question);
        }
        append_event_tx(
            tx,
            &task.id,
            event_type(worker.state),
            worker.state,
            completion_payload,
            &now,
            Some(turn_id),
        )?;
        Ok(())
    })?;
    let settled = load_task(store, &task.id)?.ok_or_else(|| {
        LifecycleError::Refusal(format!("task disappeared after settlement: {}", task.id))
    })?;
    Ok(settled)
}

fn has_invalid_routes(store: &Store, task_id: &str) -> Result<bool, LifecycleError> {
    store.with_connection(|connection| {
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='learn_routes_invalid'",
            [task_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }).map_err(LifecycleError::from)
}

fn append_provider_events(
    tx: &rusqlite::Transaction<'_>,
    task: &Task,
    turn_id: i64,
    run: Option<&RunResult>,
    resumed_session: Option<&str>,
    now: &str,
    already_persisted: usize,
    session_event_written: bool,
) -> Result<(), StoreError> {
    let Some(run) = run else {
        return Ok(());
    };
    let mut session_event_written = session_event_written;
    for event in run.events.iter().skip(already_persisted) {
        append_provider_event_tx(
            tx,
            task,
            turn_id,
            event,
            resumed_session,
            &mut session_event_written,
            now,
        )?;
    }
    if run.events_dropped > 0 {
        append_event_tx(
            tx,
            &task.id,
            "events_truncated",
            TaskState::Running,
            json!({"dropped": run.events_dropped}),
            now,
            Some(turn_id),
        )?;
    }
    if run.malformed_lines > 0 || run.oversized_lines > 0 {
        append_event_tx(
            tx,
            &task.id,
            "line_dropped",
            TaskState::Running,
            json!({
                "malformed": run.malformed_lines,
                "oversized": run.oversized_lines,
            }),
            now,
            Some(turn_id),
        )?;
    }
    if !run.stderr.trim().is_empty() {
        append_event_tx(
            tx,
            &task.id,
            "worker_stderr",
            TaskState::Running,
            json!({"text": run.stderr}),
            now,
            Some(turn_id),
        )?;
    }
    Ok(())
}

fn append_provider_event_tx(
    tx: &rusqlite::Transaction<'_>,
    task: &Task,
    turn_id: i64,
    event: &ParsedEvent,
    resumed_session: Option<&str>,
    session_event_written: &mut bool,
    now: &str,
) -> Result<(), rusqlite::Error> {
    append_event_tx(
        tx,
        &task.id,
        &format!("agent.{}", event_kind(event)),
        TaskState::Running,
        event.payload.clone(),
        now,
        Some(turn_id),
    )?;
    if let Some(session_id) = event.session_id.as_deref()
        && !*session_event_written
    {
        let (kind, payload) = if let Some(expected) = resumed_session {
            if expected != session_id {
                return Ok(());
            }
            (
                "session_reused",
                json!({
                    "provider": event.provider.as_str(),
                    "sessionId": session_id,
                }),
            )
        } else {
            (
                "session_captured",
                json!({
                    "provider": event.provider.as_str(),
                    "sessionId": session_id,
                }),
            )
        };
        append_event_tx(
            tx,
            &task.id,
            kind,
            TaskState::Running,
            payload,
            now,
            Some(turn_id),
        )?;
        *session_event_written = true;
    }
    Ok(())
}

fn has_learned_routes(
    tx: &rusqlite::Transaction<'_>,
    task_id: &str,
    attempt: usize,
) -> Result<bool, rusqlite::Error> {
    let mut statement = tx
        .prepare("SELECT payload FROM task_events WHERE task_id=? AND event_type='learn_routes'")?;
    let payloads = statement.query_map([task_id], |row| row.get::<_, String>(0))?;
    for payload in payloads {
        let payload = payload?;
        if serde_json::from_str::<Value>(&payload)
            .ok()
            .and_then(|value| value.get("attempt").and_then(Value::as_u64))
            == Some(attempt as u64)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn append_retry_events(
    store: &Store,
    task: &Task,
    turn_id: i64,
    run: &RunResult,
    resumed_session: Option<&str>,
    provider: Provider,
    attempt: usize,
    live_events: &LiveEventCapture,
) -> Result<(), LifecycleError> {
    let now = now_iso();
    store.transaction(|tx| {
        append_provider_events(
            tx,
            task,
            turn_id,
            Some(run),
            resumed_session,
            &now,
            live_events.persisted_provider_events,
            live_events.session_event_written,
        )?;
        append_event_tx(
            tx,
            &task.id,
            "provider_retry",
            TaskState::Running,
            json!({
                "provider": provider.as_str(),
                "kind": "abort",
                "attempt": attempt,
                "maxAttempts": MAX_ABORT_RETRIES,
                "error": "The turn was cut off mid-response; the session was picked back up.",
            }),
            &now,
            Some(turn_id),
        )?;
        Ok(())
    })?;
    Ok(())
}

fn aborted_turn(raw: &str) -> Option<String> {
    const ABORTED_REASONS: [&str; 5] = ["unknown", "aborted", "error", "cancelled", "canceled"];
    for line in raw.lines().rev() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("step_finish") {
            continue;
        }
        let part = event.get("part").and_then(Value::as_object);
        let reason = part
            .and_then(|part| part.get("reason"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let output = part
            .and_then(|part| part.get("tokens"))
            .and_then(Value::as_object)
            .and_then(|tokens| tokens.get("output"))
            .and_then(Value::as_f64)
            .unwrap_or_default();
        if reason.is_empty()
            || output > 0.0
            || !ABORTED_REASONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(reason))
        {
            return None;
        }
        return Some(format!(
            "the provider ended the turn mid-generation: step_finish reason \"{reason}\", no output tokens"
        ));
    }
    None
}

fn add_usage(target: &mut Usage, source: &Usage) {
    add_optional(&mut target.tokens_in, source.tokens_in);
    add_optional(&mut target.tokens_out, source.tokens_out);
    add_optional(&mut target.cached_tokens, source.cached_tokens);
    add_optional(&mut target.cost_usd, source.cost_usd);
    add_optional(&mut target.turns, source.turns);
    target.cost_usd_estimated |= source.cost_usd_estimated;
}

/// Fills `usage.cost_usd` from public model pricing when the provider
/// reported tokens but no amount of its own, so runs from providers that
/// never surface a dollar figure still show a spend. Never touches a
/// provider-reported amount, and never blocks settlement: a catalogue miss
/// or unknown model just leaves the amount empty.
async fn apply_estimated_cost(usage: &mut Usage, provider: Provider, model: &str) {
    if usage.cost_usd.is_some() || (usage.tokens_in.is_none() && usage.tokens_out.is_none()) {
        return;
    }
    let Some(estimated) = oga_pricing::estimate(
        provider.as_str(),
        model,
        usage.tokens_in.unwrap_or_default(),
        usage.tokens_out.unwrap_or_default(),
        usage.cached_tokens.unwrap_or_default(),
    )
    .await
    else {
        return;
    };
    usage.cost_usd = Some(estimated);
    usage.cost_usd_estimated = true;
}

fn add_optional(target: &mut Option<f64>, value: Option<f64>) {
    if let Some(value) = value {
        *target = Some(target.unwrap_or_default() + value);
    }
}

fn event_kind(event: &ParsedEvent) -> &str {
    event
        .payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("event")
}

fn record_profile_outcome(
    store: &Store,
    task: &Task,
    worker: &WorkerOutcome,
) -> Result<(), LifecycleError> {
    let code = match worker.completion.code {
        CompletionCode::Auth => Some(FailureCode::Auth),
        CompletionCode::Billing => Some(FailureCode::Billing),
        CompletionCode::RateLimit => Some(FailureCode::RateLimit),
        CompletionCode::Network => Some(FailureCode::Network),
        _ => None,
    };
    if let Some(code) = code {
        store.repositories().failures().record(&ProfileFailure {
            profile_id: task.profile_id.clone(),
            code,
            message: worker
                .completion
                .reason
                .clone()
                .or_else(|| worker.error.clone())
                .unwrap_or_else(|| "provider failure".into()),
            failed_at: now_iso(),
            consecutive_failures: 1,
            retry_at: worker.completion.resets_at.clone(),
            model: Some(task.model.clone()),
        })?;
    } else if worker.state == TaskState::Completed {
        store.repositories().failures().clear(&ProfileSuccess {
            profile_id: task.profile_id.clone(),
            succeeded_at: now_iso(),
        })?;
    }
    Ok(())
}

/// Release dependency holds after one blocker settles. The operation is safe to
/// call more than once because deleting the hold is the atomic release claim.
pub fn release_dependents(store: &Store, blocker: &Task) -> Result<Vec<Task>, LifecycleError> {
    let mut released = Vec::new();
    let dependents = store.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT tasks.id FROM task_dependencies JOIN tasks ON tasks.id=task_dependencies.task_id WHERE task_dependencies.blocker_id=? ORDER BY task_dependencies.created_at, task_dependencies.rowid",
        )?;
        statement
            .query_map([blocker.id.as_str()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    })?;
    for dependent_id in dependents {
        let Some(dependent) = load_task(store, &dependent_id)? else {
            continue;
        };
        if dependent.state != TaskState::Pending {
            continue;
        }
        let Some((hold_policy, hold_note)) = dependency_hold(store, &dependent.id)? else {
            continue;
        };
        let states = dependency_states(store, &dependent.id)?;
        let ready = if hold_policy == oga_domain::OnBlockerFailure::Run {
            states.iter().all(|state| {
                *state == TaskState::Completed
                    || matches!(
                        state,
                        TaskState::Failed | TaskState::Cancelled | TaskState::Blocked
                    )
            })
        } else {
            states.iter().all(|state| *state == TaskState::Completed)
        };
        if !ready {
            if is_blocking_end(blocker.state) && hold_policy == oga_domain::OnBlockerFailure::Hold {
                block_dependent(store, &dependent, blocker, &hold_note)?;
            }
            continue;
        }
        if !claim_dependency_hold(store, &dependent.id, &hold_note)? {
            continue;
        }
        let Some(queued) = queue_held_task(store, &dependent.id, &hold_note)? else {
            continue;
        };
        released.push(queued);
    }
    Ok(released)
}

/// Revive the dependents an earlier ending dropped. A task that leaves a
/// blocking end — asserted complete, resumed, handed off — was the reason its
/// dependents landed `blocked`, so they go back to waiting, transitively.
pub fn restore_dependency_blocked_dependents(
    store: &Store,
    blocker: &Task,
) -> Result<(), LifecycleError> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::from([blocker.id.clone()]);
    while let Some(current) = queue.pop_front() {
        for dependent in dependencies::dependents_of(store, &current)? {
            if !visited.insert(dependent.id.clone()) {
                continue;
            }
            queue.push_back(dependent.id.clone());
            if !is_blocking_end(dependent.state) {
                continue;
            }
            let dependency_blocked = dependent
                .completion
                .as_ref()
                .and_then(|completion| completion.dependency_blocked)
                .unwrap_or(false);
            if !dependency_blocked {
                continue;
            }
            let waiting_on = dependencies::dependencies_of(store, &dependent.id)?
                .into_iter()
                .filter(|task| task.state != TaskState::Completed)
                .collect::<Vec<_>>();
            if waiting_on.iter().any(|task| is_blocking_end(task.state)) {
                continue;
            }
            let now = now_iso();
            let hold = dependencies::dependency_hold(
                &dependent.id,
                &waiting_on,
                oga_domain::OnBlockerFailure::Hold,
                None,
                &now,
            );
            dependencies::restore_dependency_hold(store, &hold, &now)?;
        }
    }
    Ok(())
}

fn dependency_hold(
    store: &Store,
    task_id: &str,
) -> Result<Option<(oga_domain::OnBlockerFailure, String)>, LifecycleError> {
    store
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT args_json,note FROM task_holds WHERE task_id=?",
                    [task_id],
                    |row| {
                        let args: HoldArgs = serde_json::from_str(&row.get::<_, String>(0)?)
                            .map_err(|error| {
                                rusqlite::Error::InvalidParameterName(error.to_string())
                            })?;
                        Ok((
                            args.on_blocker_failure
                                .unwrap_or(oga_domain::OnBlockerFailure::Hold),
                            row.get(1)?,
                        ))
                    },
                )
                .optional()
                .map_err(StoreError::from)
        })
        .map_err(LifecycleError::from)
}

fn dependency_states(store: &Store, task_id: &str) -> Result<Vec<TaskState>, LifecycleError> {
    store
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT tasks.state FROM task_dependencies JOIN tasks ON tasks.id=task_dependencies.blocker_id WHERE task_dependencies.task_id=? ORDER BY task_dependencies.created_at, task_dependencies.rowid",
            )?;
            statement
                .query_map([task_id], |row| {
                    let state: String = row.get(0)?;
                    serde_json::from_str(&format!("\"{state}\""))
                        .map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from)
        })
        .map_err(LifecycleError::from)
}

fn claim_dependency_hold(store: &Store, task_id: &str, note: &str) -> Result<bool, LifecycleError> {
    let now = now_iso();
    store
        .transaction(|tx| {
            let removed = tx.execute("DELETE FROM task_holds WHERE task_id=?", [task_id])?;
            if removed != 1 {
                return Ok(false);
            }
            append_event_tx(
                tx,
                task_id,
                "hold_released",
                TaskState::Pending,
                json!({"note": note}),
                &now,
                None,
            )?;
            Ok(true)
        })
        .map_err(LifecycleError::from)
}

fn queue_held_task(
    store: &Store,
    task_id: &str,
    note: &str,
) -> Result<Option<Task>, LifecycleError> {
    let now = now_iso();
    let changed = store.transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='queued',updated_at=? WHERE id=? AND state='pending'",
            params![now, task_id],
        )?;
        if changed == 1 {
            append_event_tx(
                tx,
                task_id,
                "queued",
                TaskState::Queued,
                json!({"note": note}),
                &now,
                None,
            )?;
        }
        Ok(changed == 1)
    })?;
    if changed {
        load_task(store, task_id).map_err(LifecycleError::from)
    } else {
        Ok(None)
    }
}

fn block_dependent(
    store: &Store,
    dependent: &Task,
    blocker: &Task,
    note: &str,
) -> Result<(), LifecycleError> {
    let reason = format!(
        "did not start: {} ended {}",
        blocker.title.as_deref().unwrap_or(&blocker.id),
        blocker.state.as_str()
    );
    let completion = completion(None, true, CompletionCode::Cancelled, Some(reason.clone()));
    let completion = TaskCompletion {
        dependency_blocked: Some(true),
        ..completion
    };
    let now = now_iso();
    store.transaction(|tx| {
        if tx.execute("DELETE FROM task_holds WHERE task_id=?", [dependent.id.as_str()])? != 1 {
            return Ok(());
        }
        tx.execute(
            "UPDATE tasks SET state='blocked',error=?,completion_json=?,updated_at=? WHERE id=? AND state='pending'",
            params![reason, encode(&completion)?, now, dependent.id],
        )?;
        append_event_tx(
            tx,
            &dependent.id,
            "hold_dropped",
            TaskState::Pending,
            json!({"reason": reason, "note": note, "blockerId": blocker.id, "blockerState": blocker.state}),
            &now,
            None,
        )?;
        append_event_tx(
            tx,
            &dependent.id,
            "blocked",
            TaskState::Blocked,
            json!({"error": reason, "completion": completion}),
            &now,
            None,
        )?;
        Ok(())
    })?;
    Ok(())
}

pub(crate) fn block_queued_task(
    store: &Store,
    task_id: &str,
    reason: &str,
) -> Result<(), LifecycleError> {
    let completion = completion(
        None,
        true,
        CompletionCode::WorkerError,
        Some(reason.to_owned()),
    );
    let now = now_iso();
    store.transaction(|tx| {
        tx.execute(
            "UPDATE tasks SET state='blocked',error=?,completion_json=?,updated_at=? WHERE id=? AND state='queued'",
            params![reason, encode(&completion)?, now, task_id],
        )?;
        append_event_tx(
            tx,
            task_id,
            "blocked",
            TaskState::Blocked,
            json!({"error": reason, "completion": completion}),
            &now,
            None,
        )?;
        Ok(())
    })?;
    Ok(())
}

fn is_blocking_end(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Failed | TaskState::Cancelled | TaskState::Blocked
    )
}

/// Best-effort fallback for a detached launch that fails before it can claim a
/// task. A settled task is never overwritten by this path.
pub(crate) fn fail_unstarted_task(
    store: &Store,
    task_id: &str,
    expected_updated_at: &str,
    reason: &str,
) -> Result<(), LifecycleError> {
    let completion = completion(
        None,
        true,
        CompletionCode::WorkerError,
        Some(reason.to_owned()),
    );
    let now = now_iso();
    store.transaction(|tx| {
        let changed = tx.execute(
            "UPDATE tasks SET state='failed',error=?,completion_json=?,worker_json=NULL,updated_at=? WHERE id=? AND updated_at=? AND state IN ('queued','running')",
            params![reason, encode(&completion)?, now, task_id, expected_updated_at],
        )?;
        if changed == 1 {
            append_event_tx(
                tx,
                task_id,
                "failed",
                TaskState::Failed,
                json!({"error": reason, "completion": completion}),
                &now,
                None,
            )?;
        }
        Ok(())
    })?;
    Ok(())
}

fn claim_task(store: &Store, task_id: &str, shipped_prompt: &str) -> Result<i64, LifecycleError> {
    let now = now_iso();
    store
        .transaction(|tx| {
            let changed = tx.execute(
                "UPDATE tasks SET state='running',shipped_prompt=?,worker_json=NULL,updated_at=? WHERE id=? AND state='queued'",
                params![shipped_prompt, now, task_id],
            )?;
            if changed != 1 {
                return Err(StoreError::Refusal(format!(
                    "task is not queued: {task_id}"
                )));
            }
            tx.execute(
                "UPDATE task_turns SET status='interrupted',ended_at=? WHERE task_id=? AND status='running'",
                params![now, task_id],
            )?;
            let ordinal: u64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(ordinal),0)+1 FROM task_turns WHERE task_id=?",
                    [task_id],
                    |row| row.get(0),
                )?;
            tx.execute(
                "INSERT INTO task_turns(task_id,ordinal,status,started_at) VALUES(?,?,?,?)",
                params![task_id, ordinal, "running", now],
            )?;
            let turn_id = tx.last_insert_rowid();
            append_event_tx(
                tx,
                task_id,
                "started",
                TaskState::Running,
                json!({}),
                &now,
                None,
            )?;
            Ok(turn_id)
        })
        .map_err(LifecycleError::from)
}

fn completion(
    exit_code: Option<i32>,
    blocked: bool,
    code: CompletionCode,
    reason: Option<String>,
) -> TaskCompletion {
    TaskCompletion {
        exit_code: exit_code.map(i64::from),
        blocked,
        code,
        reason,
        suggested_scope: None,
        resets_at: None,
        asserted_completion: None,
        dependency_blocked: None,
    }
}

fn turn_status(state: TaskState) -> &'static str {
    match state {
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::NeedsInput => "needs_input",
        TaskState::Blocked => "blocked",
        TaskState::Cancelled => "cancelled",
        _ => "interrupted",
    }
}

fn event_type(state: TaskState) -> &'static str {
    match state {
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::NeedsInput => "needs_input",
        TaskState::Blocked => "blocked",
        TaskState::Cancelled => "cancelled",
        _ => "state_changed",
    }
}

fn encode<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value)
        .map_err(|error| StoreError::Refusal(format!("invalid lifecycle JSON: {error}")))
}

fn append_event_tx(
    tx: &rusqlite::Transaction<'_>,
    task_id: &str,
    kind: &str,
    state: TaskState,
    payload: Value,
    at: &str,
    turn_id: Option<i64>,
) -> Result<i64, rusqlite::Error> {
    tx.execute(
        "INSERT INTO task_events(task_id,event_type,state,payload,created_at,turn_id) VALUES(?,?,?,?,?,?)",
        params![task_id, kind, state.as_str(), payload.to_string(), at, turn_id],
    )?;
    Ok(tx.last_insert_rowid())
}

pub(crate) fn now_iso() -> String {
    iso_from_system_time(SystemTime::now())
}

pub(crate) fn iso_from_system_time(time: SystemTime) -> String {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch");
    let seconds = duration.as_secs();
    let days = seconds / 86_400;
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        day_seconds / 3_600,
        day_seconds / 60 % 60,
        day_seconds % 60,
        duration.subsec_millis()
    )
}

pub(crate) fn iso_from_unix_millis(millis: u64) -> Option<String> {
    UNIX_EPOCH
        .checked_add(Duration::from_millis(millis))
        .map(iso_from_system_time)
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

fn broker_base_url() -> String {
    let port = std::env::var("OGA_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(7331);
    format!("http://127.0.0.1:{port}")
}

fn hook_url_for(task_id: &str) -> String {
    format!("{}/api/hooks/{task_id}", broker_base_url())
}

/// The oga MCP server rides on the command line for a stock claude worker,
/// which is the only provider that takes one there. A profile with its own
/// command decides its own tools, and every other provider is configured
/// outside the argv.
fn worker_mcp_config(profile: &Profile, task_id: &str) -> Option<String> {
    (profile.provider == Provider::Claude && profile.command.is_none())
        .then(|| oga_providers::worker_mcp_config(&broker_base_url(), task_id))
}

/// What a worker needs to know about itself. `oga query` and `oga
/// relearn` read the task id to scope themselves, and a provider that
/// stats `PWD` rather than calling `getcwd` probes whatever directory the
/// broker was launched from unless it is told otherwise.
fn worker_env(task_id: &str, cwd: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("OGA_TASK_ID".to_owned(), task_id.to_owned()),
        ("OGA_HOOK_URL".to_owned(), hook_url_for(task_id)),
        ("PWD".to_owned(), cwd.to_owned()),
        ("OLDPWD".to_owned(), cwd.to_owned()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_maps_timeout_to_blocked_completion_code() {
        let run = WorkerOutcome {
            state: TaskState::Failed,
            output: String::new(),
            question: None,
            error: Some("provider run timed out".into()),
            completion: completion(
                None,
                true,
                CompletionCode::Timeout,
                Some("provider run timed out".into()),
            ),
        };
        assert_eq!(run.completion.code, CompletionCode::Timeout);
        assert_eq!(turn_status(run.state), "failed");
    }

    #[test]
    fn lifecycle_event_names_match_terminal_states() {
        assert_eq!(event_type(TaskState::Completed), "completed");
        assert_eq!(event_type(TaskState::NeedsInput), "needs_input");
        assert_eq!(event_type(TaskState::Blocked), "blocked");
    }

    // Whoever adds the next way to start a run reaches a provider through here,
    // so the switch is checked here rather than at each caller. A path that
    // skipped every earlier check still cannot spend anything.
    #[tokio::test]
    async fn a_run_started_around_dispatch_still_checks_the_switch() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        let profile = Profile {
            id: "fake".into(),
            label: "Fake".into(),
            provider: Provider::Claude,
            default_model: "fake-model".into(),
            enabled: true,
            env: BTreeMap::new(),
            capabilities: vec![],
            command: Some(vec!["sh".into(), "-c".into(), "printf hello".into()]),
        };
        let now = now_iso();
        store
            .repositories()
            .profiles()
            .insert(&profile, &now)
            .expect("profile");
        let task = Task {
            id: "task-1".into(),
            profile_id: profile.id.clone(),
            model: profile.default_model.clone(),
            prompt: "do the thing".into(),
            cwd: directory.path().display().to_string(),
            state: TaskState::Queued,
            created_at: now.clone(),
            updated_at: now,
            ..Task::default()
        };
        store.repositories().tasks().insert(&task).expect("task");

        let error = run_task_with_prompt(
            store,
            oga_runner::ProviderRunner::default(),
            task,
            profile,
            WorkerPromptInput::default(),
        )
        .await
        .expect_err("run refused");

        assert!(
            error
                .to_string()
                .contains("fake-model is not turned on for fake"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn completed_codex_file_changes_receive_tracked_and_untracked_patches() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let cwd = directory.path();
        run_git(cwd, &["init", "-q"]);
        run_git(cwd, &["config", "user.email", "test@example.com"]);
        run_git(cwd, &["config", "user.name", "Test"]);
        run_git(cwd, &["config", "commit.gpgSign", "false"]);
        std::fs::create_dir(cwd.join("src")).expect("source directory");
        std::fs::write(cwd.join("src/main.rs"), "old\n").expect("tracked file");
        run_git(cwd, &["add", "."]);
        run_git(cwd, &["commit", "-qm", "initial"]);
        std::fs::write(cwd.join("src/main.rs"), "new\n").expect("modified file");
        std::fs::write(cwd.join("src/new.rs"), "hello\n").expect("untracked file");

        let task = Task {
            cwd: cwd.display().to_string(),
            ..Task::default()
        };
        let tracked_path = cwd.join("src/main.rs").display().to_string();
        let event = ParsedEvent {
            provider: Provider::Codex,
            payload: json!({
                "type": "item.completed",
                "item": {
                    "type": "file_change",
                    "changes": [
                        {"path": tracked_path, "kind": "update"},
                        {"path": "src/new.rs", "kind": "add"}
                    ]
                }
            }),
            session_id: None,
            write_targets: vec![],
            usage: Usage::default(),
        };

        let enriched = enrich_codex_file_change(&task, event).await;
        assert!(
            enriched.payload["item"]["changes"][0]["patch"]
                .as_str()
                .expect("tracked patch")
                .contains("+new")
        );
        assert_eq!(
            enriched.payload["item"]["changes"][1]["patch"],
            "@@ -0,0 +1,1 @@\n+hello"
        );
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .status()
                .expect("git")
                .success(),
            "git {:?} failed",
            args
        );
    }
}
