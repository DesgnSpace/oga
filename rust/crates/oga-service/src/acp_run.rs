//! One run over ACP: open or restore the agent's session, send the prompt
//! once, and settle the task from the agent's answer.
//!
//! The prompt turn decides the task, not the process: an agent is a server and
//! may stay up after it answers, so the run ends it once the turn is over and
//! only then settles. A failure after the prompt was written is reported, never
//! run again, because the turn may already have happened.

use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use oga_acp::{
    AcpConfig, AcpError, AcpPolicy, AcpSession, AgentRelease, Decision, Launch, PolicyFuture,
    Refusal, SentPrompt, SessionSetting, SessionStart, Stage, Steered,
    schema::{
        AgentCapabilities, ContentBlock, HttpHeader, McpServer, McpServerHttp, PermissionOption,
        PermissionOptionKind, RequestPermissionRequest, SessionId, SessionNotification,
        SessionUpdate, StopReason, ToolKind,
    },
};
use oga_domain::{
    AcpAgentIdentity, AcpRestore, AcpSteering, CompletionCode, Profile, Task, TaskScope, TaskState,
    TaskTransport, TaskWorker, Transport, TransportReason,
};
use oga_events::ModelRecovery;
use oga_providers::{
    AcpAdapter, AcpLaunch, AcpVersions, NO_FINAL_MESSAGE, OPENCODE_ADAPTER, Usage,
    skill_directories,
};
use oga_runner::{ProviderRunner, RunRequest, Termination};
use oga_store::{Store, StoreError};
use rusqlite::params;
use serde_json::{Value, json};
use tokio::{sync::mpsc, time::Instant};

use crate::{
    lifecycle::{
        ActiveRun, ActiveRuns, LifecycleError, MAX_ABORT_RETRIES, RunOutcome, Settlement,
        append_event_tx, broker_base_url, completion, encode, load_task, now_iso,
        record_profile_outcome, settle_task, worker_env,
    },
    prompt::{WorkerOutcome, interpret_worker_outcome, rate_limit_reset_at},
    transport::{self, AcpStart},
};

/// The longest task timeout bounds a turn with no explicit timeout.
const LONGEST_TURN: Duration = Duration::from_secs(24 * 60 * 60);

pub(crate) enum AcpEnd {
    Settled(Box<RunOutcome>),
    /// ACP never became usable, no prompt was written, and this run may use
    /// the command line instead. Carries the decision to record first.
    FallBack(TaskTransport),
}

pub(crate) struct AcpTurn<'a> {
    pub(crate) store: &'a Arc<Store>,
    pub(crate) runner: &'a ProviderRunner,
    /// The claimed, running task.
    pub(crate) task: &'a Task,
    pub(crate) profile: &'a Profile,
    pub(crate) adapter: &'a AcpAdapter,
    pub(crate) start: AcpStart,
    /// Whether this run is one that may hand the work to the command line at
    /// all, before how far ACP got is weighed.
    pub(crate) may_fall_back: bool,
    pub(crate) prompt: &'a str,
    pub(crate) turn_id: i64,
    pub(crate) active: &'a ActiveRuns,
}

pub(crate) enum Delivered {
    /// The worker took it into the turn it is running.
    InTurn,
    /// It never reached the turn, in the agent's words or the client's.
    Missed(String),
}

pub(crate) struct AcpRun {
    session: AcpSession,
    /// How this agent takes an instruction into a turn it is already running.
    steering: Option<AcpSteering>,
    /// Instructions handed over as their own prompt, still to be answered.
    joined: Mutex<Vec<SentPrompt>>,
    cancelled: AtomicBool,
}

impl AcpRun {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.session.cancel();
        self.session.process().terminate(Termination::Cancelled);
    }

    pub(crate) async fn steer(&self, instruction: &str) -> Delivered {
        let blocks = vec![ContentBlock::from(instruction.to_owned())];
        match self.steering {
            Some(AcpSteering::Extension) => match self.session.steer(blocks).await {
                Ok(Steered::Injected) => Delivered::InTurn,
                Ok(Steered::Elsewhere(outcome)) => Delivered::Missed(missed(&outcome)),
                Err(error) => Delivered::Missed(error.to_string()),
            },
            Some(AcpSteering::Prompt) => match self.session.send_prompt(blocks) {
                Ok(sent) => {
                    self.joined
                        .lock()
                        .expect("joined prompt lock is not poisoned")
                        .push(sent);
                    Delivered::InTurn
                }
                Err(error) => Delivered::Missed(error.to_string()),
            },
            None => Delivered::Missed("this worker only takes an instruction between runs".into()),
        }
    }

    async fn drain_joined(&self) {
        let sent = std::mem::take(
            &mut *self
                .joined
                .lock()
                .expect("joined prompt lock is not poisoned"),
        );
        for prompt in sent {
            prompt.settled().await;
        }
    }

    fn was_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

fn steering_for(adapter: &AcpAdapter, session: &AcpSession) -> Option<AcpSteering> {
    session
        .steering()
        .or_else(|| (adapter.id == OPENCODE_ADAPTER).then_some(AcpSteering::Prompt))
}

fn missed(outcome: &str) -> String {
    match outcome {
        "promptRequired" => "the run had already finished",
        "startedNewTurn" => "the worker put it in a run of its own",
        _ => "the worker did not take it",
    }
    .to_owned()
}

enum TurnEnd {
    Answered(Result<oga_acp::schema::PromptResponse, AcpError>),
    TimedOut,
}

pub(crate) async fn run(turn: AcpTurn<'_>) -> Result<AcpEnd, LifecycleError> {
    let AcpTurn {
        store,
        runner,
        task,
        profile,
        adapter,
        ..
    } = &turn;
    let acp_launch = AcpLaunch {
        profile,
        model: &task.model,
        effort: task.effort.as_deref(),
        cwd: &task.cwd,
    };
    if let Some(reason) = adapter.incompatibility_for(&acp_launch) {
        return open_failed(
            &turn,
            AcpError::Unavailable {
                stage: Stage::Spawn,
                reason,
            },
        );
    }
    let request =
        RunRequest::from_command(profile.provider, adapter.command(&acp_launch), &task.cwd)
            .with_env(worker_env(&task.id, &task.cwd));
    let start = match &turn.start {
        AcpStart::New => SessionStart::New,
        AcpStart::Restore {
            session_id,
            how: AcpRestore::Resume,
        } => SessionStart::Resume(SessionId::new(session_id.as_str())),
        AcpStart::Restore {
            session_id,
            how: AcpRestore::Load,
        } => SessionStart::Load(SessionId::new(session_id.as_str())),
    };
    let settings = adapter
        .settings_for(&acp_launch)
        .into_iter()
        .map(|setting| SessionSetting {
            id: setting.id,
            value: setting.value,
            required: setting.required,
        })
        .collect();
    let mut launch = Launch::new(request, task.scope.clone(), start)
        .settings(settings)
        .additional_directories(adapter.directories_for(&acp_launch));
    if let Some(release) = &adapter.release {
        launch = launch.release(match &release.versions {
            AcpVersions::Line(line) => AgentRelease::line(&release.agent, line),
            AcpVersions::Build(build) => AgentRelease::build(&release.agent, build),
        });
    }
    if let Some(method_id) = &adapter.authentication {
        launch = launch.authentication(method_id);
    }
    if adapter.oga_tools {
        launch = launch.mcp_servers(vec![oga_mcp_server(&task.id)]);
    }
    let policy = Arc::new(TaskPolicy {
        store: Arc::clone(store),
        task_id: task.id.clone(),
        turn_id: turn.turn_id,
        cwd: PathBuf::from(&task.cwd),
        scope: task.scope.clone(),
        declines_questions: adapter.declines_questions,
        skills: skill_directories(turn.profile),
        refused: Refused::default(),
    });
    let refused = Arc::clone(&policy.refused);
    let turn_bound = task.timeout_ms.map_or(LONGEST_TURN, Duration::from_millis);
    let config = AcpConfig {
        // The lifecycle enforces the task's own bound; the transport's is only
        // a backstop behind it.
        prompt_timeout: turn_bound + Duration::from_secs(60),
        ..AcpConfig::default()
    };
    let opening_bound = config.handshake_timeout;
    let session = match AcpSession::open(runner, launch, policy, config).await {
        Ok(session) => session,
        Err(error) => return open_failed(&turn, error),
    };
    if adapter.oga_tools && !session.agent_capabilities().mcp_capabilities.http {
        session.shutdown().await;
        return open_failed(
            &turn,
            AcpError::Unavailable {
                stage: Stage::Initialize,
                reason: "the agent cannot connect to Oga's tools over HTTP".into(),
            },
        );
    }

    let mut updates = session
        .take_updates()
        .expect("a freshly opened session still has its update stream");
    // `session/load` replays the whole conversation before it answers, and an
    // agent that finishes opening a session later keeps writing to it until it
    // says so. None of that is this turn, so it is counted and dropped rather
    // than recorded.
    let mut replayed = 0usize;
    if let Some(opened) = adapter.opened_with.as_deref() {
        match wait_for_update(&mut updates, opened, opening_bound).await {
            Some(dropped) => replayed += dropped,
            None => {
                session.shutdown().await;
                return open_failed(
                    &turn,
                    AcpError::Unavailable {
                        stage: Stage::Session,
                        reason: "the agent never finished opening the session".into(),
                    },
                );
            }
        }
    }
    while updates.try_recv().is_ok() {
        replayed += 1;
    }
    let steering = steering_for(turn.adapter, &session);
    if let Err(error) = record_session(&turn, &acp_launch, &session, replayed, steering) {
        session.shutdown().await;
        return Err(error);
    }

    let run = Arc::new(AcpRun {
        session,
        steering,
        joined: Mutex::new(Vec::new()),
        cancelled: AtomicBool::new(false),
    });
    let active = ActiveRun::Acp(Arc::clone(&run));
    turn.active.insert(&task.id, active.clone());

    let mut transcript = Transcript {
        refused,
        ..Transcript::default()
    };
    let deadline = Instant::now() + turn_bound;
    let mut ended = match load_task(store, &task.id) {
        Ok(current) => {
            if current.is_none_or(|current| current.state != TaskState::Running) {
                run.cancel();
            }
            converse(
                &turn,
                &run,
                &mut updates,
                &mut transcript,
                turn.prompt,
                deadline,
            )
            .await
        }
        Err(error) => Err(error.into()),
    };
    // An agent that ends a turn on an error of its own is still running with
    // the session open, so it is asked to carry on rather than the task failed.
    let mut retries = 0;
    while retries < MAX_ABORT_RETRIES && !run.was_cancelled() {
        let Ok(TurnEnd::Answered(Err(AcpError::TurnFailed { reason }))) = &ended else {
            break;
        };
        retries += 1;
        let recorded = record_turn_retry(&turn, &mut transcript, reason, retries).await;
        ended = match recorded {
            Ok(()) => {
                converse(
                    &turn,
                    &run,
                    &mut updates,
                    &mut transcript,
                    CONTINUE_AFTER_TURN_ERROR,
                    deadline,
                )
                .await
            }
            Err(error) => Err(error),
        };
    }
    let ended = match ended {
        Ok(ended) => ended,
        Err(error) => {
            run.cancel();
            run.session.shutdown().await;
            turn.active.remove(&task.id, &active);
            return Err(error);
        }
    };
    if matches!(ended, TurnEnd::TimedOut) {
        run.session.cancel();
    }
    run.drain_joined().await;
    run.session.shutdown().await;
    turn.active.remove(&task.id, &active);
    // Updates the agent sent before it answered are still queued once it is
    // gone, so they are recorded after the process is ended rather than lost.
    while let Ok(notification) = updates.try_recv() {
        transcript
            .push(store, &task.id, turn.turn_id, notification)
            .await?;
    }
    transcript.flush(store, &task.id, turn.turn_id).await?;

    let diagnostics = run.session.diagnostics();
    if !diagnostics.stderr.trim().is_empty() {
        append_turn_event(
            store,
            &task.id,
            turn.turn_id,
            "worker_stderr".into(),
            json!({"text": diagnostics.stderr, "truncated": diagnostics.stderr_truncated}),
        )
        .await?;
    }
    let worker = outcome(ended, run.was_cancelled(), &transcript, &diagnostics.stderr);
    let usage = transcript.usage(task.cost_usd);
    let settled = settle_task(
        store,
        task,
        turn.turn_id,
        Settlement::from_usage(&usage),
        &worker,
    )?;
    record_profile_outcome(store, task, &worker)?;
    Ok(AcpEnd::Settled(Box::new(RunOutcome {
        task: settled,
        worker,
    })))
}

const CONTINUE_AFTER_TURN_ERROR: &str = "Your last step stopped on an error inside the worker, not in your work. Continue from where you stopped, without redoing finished steps.";

async fn record_turn_retry(
    turn: &AcpTurn<'_>,
    transcript: &mut Transcript,
    reason: &str,
    attempt: usize,
) -> Result<(), LifecycleError> {
    transcript
        .flush(turn.store, &turn.task.id, turn.turn_id)
        .await?;
    append_turn_event(
        turn.store,
        &turn.task.id,
        turn.turn_id,
        "provider_retry".into(),
        json!({
            "provider": turn.profile.provider.as_str(),
            "kind": "turn_error",
            "attempt": attempt,
            "maxAttempts": MAX_ABORT_RETRIES,
            "error": reason,
        }),
    )
    .await?;
    Ok(())
}

async fn converse(
    turn: &AcpTurn<'_>,
    run: &AcpRun,
    updates: &mut mpsc::UnboundedReceiver<SessionNotification>,
    transcript: &mut Transcript,
    prompt: &str,
    deadline: Instant,
) -> Result<TurnEnd, LifecycleError> {
    let prompt = run
        .session
        .prompt(vec![ContentBlock::from(prompt.to_owned())]);
    tokio::pin!(prompt);
    let deadline = tokio::time::sleep_until(deadline);
    tokio::pin!(deadline);
    let mut receiving = true;
    loop {
        tokio::select! {
            answer = &mut prompt => return Ok(TurnEnd::Answered(answer)),
            () = &mut deadline => return Ok(TurnEnd::TimedOut),
            notification = updates.recv(), if receiving => match notification {
                Some(notification) => {
                    transcript
                        .push(turn.store, &turn.task.id, turn.turn_id, notification)
                        .await?;
                }
                None => receiving = false,
            },
        }
    }
}

async fn wait_for_update(
    updates: &mut mpsc::UnboundedReceiver<SessionNotification>,
    kind: &str,
    bound: Duration,
) -> Option<usize> {
    let waiting = async {
        let mut taken = 0;
        while let Some(notification) = updates.recv().await {
            taken += 1;
            let update = serde_json::to_value(&notification.update).ok();
            if update.is_some_and(|update| update["sessionUpdate"] == kind) {
                return Some(taken);
            }
        }
        None
    };
    tokio::time::timeout(bound, waiting).await.ok().flatten()
}

fn open_failed(turn: &AcpTurn<'_>, error: AcpError) -> Result<AcpEnd, LifecycleError> {
    let now = now_iso();
    let (worker, forget_session) = match error {
        AcpError::Unavailable { stage, reason } => {
            if turn.may_fall_back && transport::failed_before_a_verified_agent(stage) {
                return Ok(AcpEnd::FallBack(TaskTransport::cli(
                    TransportReason::Unavailable,
                    Some(format!("{stage}: {reason}")),
                    &now,
                )));
            }
            let restoring = matches!(turn.start, AcpStart::Restore { .. });
            let message = if restoring && stage == Stage::Session {
                format!(
                    "{} couldn't reopen this task's conversation ({reason}). Resume the task to start a new one from a summary of the work so far.",
                    turn.profile.id
                )
            } else if stage == Stage::Configure && restoring {
                format!(
                    "{} can't use this task's model or effort over ACP ({reason}). Move the task to another model or worker to continue.",
                    turn.profile.id
                )
            } else if stage == Stage::Configure {
                format!(
                    "{} can't use this task's model or effort over ACP ({reason}). Set the worker to use its command line, or move the task to another model or worker.",
                    turn.profile.id
                )
            } else {
                format!(
                    "{} couldn't connect over ACP ({reason}). Resume the task once it can, or move it to another worker.",
                    turn.profile.id
                )
            };
            (
                failed(CompletionCode::WorkerError, message),
                restoring && stage == Stage::Session,
            )
        }
        AcpError::Refused {
            kind: Refusal::Authentication,
            reason,
        } => (
            failed(
                CompletionCode::Auth,
                format!(
                    "{} needs you to sign in again ({reason}). Sign in to that account, then resume the task.",
                    turn.profile.id
                ),
            ),
            false,
        ),
        AcpError::Refused {
            kind: Refusal::Permission,
            reason,
        } => (
            failed(
                CompletionCode::PermissionDenied,
                format!(
                    "{} declined to start this task ({reason}). Check what that worker is allowed to do, then resume the task.",
                    turn.profile.id
                ),
            ),
            false,
        ),
        AcpError::PromptInFlight { reason } | AcpError::TurnFailed { reason } => (
            failed(CompletionCode::WorkerError, stopped_mid_turn(&reason)),
            false,
        ),
    };
    if forget_session {
        turn.store.transaction(|tx| {
            tx.execute(
                "UPDATE tasks SET transport_json=json_remove(transport_json,'$.acpSessionId') WHERE id=?",
                [&turn.task.id],
            )?;
            append_event_tx(
                tx,
                &turn.task.id,
                "session_rejected",
                TaskState::Running,
                json!({"transport": Transport::Acp, "profile": turn.task.profile_id}),
                &now,
                Some(turn.turn_id),
            )?;
            Ok(())
        })?;
    }
    let settled = settle_task(
        turn.store,
        turn.task,
        turn.turn_id,
        Settlement::default(),
        &worker,
    )?;
    record_profile_outcome(turn.store, turn.task, &worker)?;
    Ok(AcpEnd::Settled(Box::new(RunOutcome {
        task: settled,
        worker,
    })))
}

fn record_session(
    turn: &AcpTurn<'_>,
    launch: &AcpLaunch<'_>,
    session: &AcpSession,
    replayed: usize,
    steering: Option<AcpSteering>,
) -> Result<(), LifecycleError> {
    let now = now_iso();
    let task = turn.task;
    let acp_session_id = session.session_id().0.to_string();
    let transport = TaskTransport {
        kind: Transport::Acp,
        reason: None,
        detail: None,
        acp_session_id: Some(acp_session_id.clone()),
        restore: restore_for(session.agent_capabilities()),
        steering,
        agent: Some(AcpAgentIdentity {
            adapter: turn.adapter.id.clone(),
            name: session.agent_info().map(|info| info.name.clone()),
            version: session.agent_info().map(|info| info.version.clone()),
            protocol_version: session.protocol_version().as_u16(),
        }),
        decided_at: if matches!(turn.start, AcpStart::Restore { .. }) {
            task.transport
                .as_ref()
                .map_or_else(|| now.clone(), |recorded| recorded.decided_at.clone())
        } else {
            now.clone()
        },
    };
    let identity = session.process().identity();
    let worker = TaskWorker {
        pid: identity.pid,
        pgid: identity.pgid,
        broker_pid: std::process::id(),
        started_at: now.clone(),
    };
    let native_session = turn
        .adapter
        .native_sessions_from
        .as_deref()
        .filter(|agent| session.agent_info().is_some_and(|info| info.name == *agent))
        .filter(|_| turn.adapter.is_native_session(launch, &acp_session_id))
        .map(|_| acp_session_id.clone());
    let restored = matches!(turn.start, AcpStart::Restore { .. });
    let (transport_json, worker_json) = (encode(&transport)?, encode(&worker)?);
    turn.store.transaction(|tx| {
        tx.execute(
            "UPDATE tasks SET transport_json=?,worker_json=?,session_id=COALESCE(?,session_id) WHERE id=?",
            params![transport_json, worker_json, native_session, task.id],
        )?;
        append_event_tx(
            tx,
            &task.id,
            "worker_spawned",
            TaskState::Running,
            json!({
                "provider": turn.profile.provider,
                "model": &task.model,
                "pid": identity.pid,
                "transport": Transport::Acp,
                "agent": transport.agent,
            }),
            &now,
            Some(turn.turn_id),
        )?;
        let mut payload = json!({
            "provider": turn.profile.provider.as_str(),
            "transport": Transport::Acp,
            "acpSessionId": acp_session_id,
        });
        if let Some(native) = &native_session {
            payload["sessionId"] = json!(native);
        }
        if restored {
            payload["replayedUpdates"] = json!(replayed);
        }
        append_event_tx(
            tx,
            &task.id,
            if restored {
                "session_reused"
            } else {
                "session_captured"
            },
            TaskState::Running,
            payload,
            &now,
            Some(turn.turn_id),
        )?;
        Ok(())
    })?;
    Ok(())
}

fn restore_for(capabilities: &AgentCapabilities) -> Option<AcpRestore> {
    if capabilities.session_capabilities.resume.is_some() {
        Some(AcpRestore::Resume)
    } else if capabilities.load_session {
        Some(AcpRestore::Load)
    } else {
        None
    }
}

fn oga_mcp_server(task_id: &str) -> McpServer {
    McpServer::Http(
        McpServerHttp::new("oga", format!("{}/mcp", broker_base_url()))
            .headers(vec![HttpHeader::new("x-oga-task-id", task_id)]),
    )
}

fn outcome(
    ended: TurnEnd,
    was_cancelled: bool,
    transcript: &Transcript,
    stderr: &str,
) -> WorkerOutcome {
    let (answer, stop_reason) = match ended {
        TurnEnd::TimedOut => (None, None),
        TurnEnd::Answered(answer) => {
            let stop_reason = answer
                .as_ref()
                .ok()
                .map(|response| stop_reason_name(&response.stop_reason));
            (Some(answer), stop_reason)
        }
    };
    // An agent that gave up on a rate-limited provider is what stopped the run,
    // whatever the turn went on to call it: the work stands and only the limit
    // has to clear, so that reading wins over the stop reason.
    let mut worker = if was_cancelled {
        cancelled()
    } else if let Some(waiting) = transcript.rate_limit_wait() {
        waiting
    } else {
        match answer {
            None => failed(CompletionCode::Timeout, "provider run timed out".into()),
            Some(Ok(response)) => match response.stop_reason {
            StopReason::EndTurn => match transcript.stopped_on_refusal() {
                Some(paths) => asks_after_refusal(&paths),
                None => interpret_worker_outcome(Some(0), transcript.final_text(), stderr, None),
            },
            StopReason::Cancelled => cancelled(),
            StopReason::MaxTokens => failed(
                CompletionCode::WorkerError,
                "The worker stopped at its output limit. Resume the task to continue.".into(),
            ),
            StopReason::MaxTurnRequests => failed(
                CompletionCode::WorkerError,
                "The worker stopped at its limit on steps for one run. Resume the task to continue."
                    .into(),
            ),
            StopReason::Refusal => failed(
                CompletionCode::WorkerError,
                "The worker declined to continue this task. Resume it with more context, or move it to another worker."
                    .into(),
            ),
            _ => failed(
                CompletionCode::WorkerError,
                "The worker stopped without saying why. Resume the task to continue.".into(),
            ),
            },
            Some(Err(AcpError::Refused {
                kind: Refusal::Authentication,
                reason,
            })) => failed(
                CompletionCode::Auth,
                format!(
                    "This worker needs you to sign in again ({reason}). Sign in to that account, then resume the task."
                ),
            ),
            Some(Err(AcpError::Refused {
                kind: Refusal::Permission,
                reason,
            })) => failed(
                CompletionCode::PermissionDenied,
                format!(
                    "The worker declined part of this task ({reason}). Check what that worker is allowed to do, then resume the task."
                ),
            ),
            Some(Err(
                AcpError::Unavailable { reason, .. }
                | AcpError::PromptInFlight { reason }
                | AcpError::TurnFailed { reason },
            )) => failed(CompletionCode::WorkerError, stopped_mid_turn(&reason)),
        }
    };
    worker.completion.stop_reason = stop_reason;
    worker
}

fn stop_reason_name(reason: &StopReason) -> String {
    match reason {
        StopReason::EndTurn => "end_turn".into(),
        StopReason::Cancelled => "cancelled".into(),
        StopReason::MaxTokens => "max_tokens".into(),
        StopReason::MaxTurnRequests => "max_turn_requests".into(),
        StopReason::Refusal => "refusal".into(),
        _ => serde_json::to_value(reason)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".into()),
    }
}

fn stopped_mid_turn(reason: &str) -> String {
    format!(
        "The worker stopped partway through ({reason}). Some of the work may already be done, so Oga didn't start it over; resume the task to continue."
    )
}

/// A worker that gave up on a refused step waits for a person to say how it
/// goes on, and the reply resumes it.
fn asks_after_refusal(paths: &[String]) -> WorkerOutcome {
    let question = if paths.is_empty() {
        "The worker stopped after Oga refused a step it asked to take. How should it go on without that step?".to_owned()
    } else {
        format!(
            "The worker stopped after Oga refused it access to {}, which this task can't reach. How should it go on without it?",
            paths.join(", ")
        )
    };
    WorkerOutcome {
        state: TaskState::NeedsInput,
        output: String::new(),
        question: Some(question.clone()),
        error: None,
        completion: completion(None, true, CompletionCode::PermissionDenied, Some(question)),
    }
}

fn failed(code: CompletionCode, reason: String) -> WorkerOutcome {
    WorkerOutcome {
        state: TaskState::Failed,
        output: String::new(),
        question: None,
        error: Some(reason.clone()),
        completion: completion(None, true, code, Some(reason)),
    }
}

fn cancelled() -> WorkerOutcome {
    let reason = "provider run was cancelled".to_owned();
    WorkerOutcome {
        state: TaskState::Cancelled,
        output: String::new(),
        question: None,
        error: Some(reason.clone()),
        completion: completion(None, true, CompletionCode::Cancelled, Some(reason)),
    }
}

async fn append_turn_event(
    store: &Arc<Store>,
    task_id: &str,
    turn_id: i64,
    kind: String,
    payload: Value,
) -> Result<(), StoreError> {
    let task_id = task_id.to_owned();
    let now = now_iso();
    store
        .write(move |tx| {
            append_event_tx(
                tx,
                &task_id,
                &kind,
                TaskState::Running,
                payload,
                &now,
                Some(turn_id),
            )?;
            Ok(())
        })
        .await
}

// Streamed message and thought chunks become one event per sentence.
#[derive(Default)]
struct Transcript {
    pending: Option<(&'static str, String)>,
    /// The agent's last message: its text since the last tool call.
    final_message: String,
    /// The freshest total the agent reported for the whole session, in USD.
    session_cost: Option<f64>,
    /// The last thing the agent said about recovering from its model provider.
    /// Only the last one matters: an agent that paused and then got through
    /// reports that too.
    recovery: Option<ModelRecovery>,
    refused: Refused,
}

/// Tool calls the task's policy refused, by id, with the paths outside its scope.
type Refused = Arc<Mutex<HashMap<String, Vec<String>>>>;

impl Transcript {
    async fn push(
        &mut self,
        store: &Arc<Store>,
        task_id: &str,
        turn_id: i64,
        notification: SessionNotification,
    ) -> Result<(), LifecycleError> {
        let chunk = match &notification.update {
            SessionUpdate::AgentMessageChunk(chunk) => Some(("agent_message_chunk", chunk)),
            SessionUpdate::AgentThoughtChunk(chunk) => Some(("agent_thought_chunk", chunk)),
            _ => None,
        };
        if let Some((kind, chunk)) = chunk {
            let ContentBlock::Text(text) = &chunk.content else {
                return Ok(());
            };
            if kind == "agent_message_chunk" {
                self.final_message.push_str(&text.text);
            }
            match &mut self.pending {
                Some((pending, joined)) if *pending == kind => joined.push_str(&text.text),
                _ => {
                    self.flush(store, task_id, turn_id).await?;
                    self.pending = Some((kind, text.text.clone()));
                }
            }
            return Ok(());
        }
        if matches!(notification.update, SessionUpdate::ToolCall(_)) {
            self.final_message.clear();
        }
        if let SessionUpdate::UsageUpdate(usage) = &notification.update {
            self.session_cost = usage
                .cost
                .as_ref()
                .filter(|cost| cost.currency.eq_ignore_ascii_case("usd"))
                .map(|cost| cost.amount)
                .filter(|amount| amount.is_finite() && *amount >= 0.0)
                .or(self.session_cost);
        }
        self.flush(store, task_id, turn_id).await?;
        let payload = serde_json::to_value(&notification.update)
            .map_err(|error| LifecycleError::Store(format!("invalid ACP update: {error}")))?;
        if let Some(recovery) = payload.get("_meta").and_then(ModelRecovery::from_meta) {
            self.recovery = Some(recovery);
        }
        let kind = payload
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or("session_update")
            .to_owned();
        append_turn_event(store, task_id, turn_id, format!("agent.{kind}"), payload).await?;
        Ok(())
    }

    async fn flush(
        &mut self,
        store: &Arc<Store>,
        task_id: &str,
        turn_id: i64,
    ) -> Result<(), LifecycleError> {
        let Some((kind, text)) = self.pending.take() else {
            return Ok(());
        };
        let payload = json!({
            "sessionUpdate": kind,
            "content": {"type": "text", "text": text},
        });
        append_turn_event(store, task_id, turn_id, format!("agent.{kind}"), payload).await?;
        Ok(())
    }

    // A rate-limited provider leaves the work untouched; wait for its reset.
    fn rate_limit_wait(&self) -> Option<WorkerOutcome> {
        let reason = self
            .recovery
            .as_ref()
            .filter(|recovery| recovery.rate_limited())?
            .message
            .clone()
            .unwrap_or_else(|| "the provider stopped answering".to_owned());
        let mut worker = failed(
            CompletionCode::RateLimit,
            format!(
                "This worker's model is rate limited ({reason}). Oga waits for the usage to reset, then runs the task again."
            ),
        );
        worker.completion.resets_at = rate_limit_reset_at(&reason);
        Some(worker)
    }

    /// An agent that had a step refused and ended without a word after its
    /// last tool call gave up on the task rather than finished it. The paths
    /// are every one it was refused this turn.
    fn stopped_on_refusal(&self) -> Option<Vec<String>> {
        if !self.final_message.trim().is_empty() {
            return None;
        }
        let refused = self
            .refused
            .lock()
            .expect("refused tool call lock is not poisoned");
        if refused.is_empty() {
            return None;
        }
        let mut paths: Vec<String> = refused.values().flatten().cloned().collect();
        paths.sort();
        paths.dedup();
        Some(paths)
    }

    fn final_text(&self) -> String {
        if self.final_message.trim().is_empty() {
            NO_FINAL_MESSAGE.to_owned()
        } else {
            self.final_message.clone()
        }
    }

    // ACP reports session totals, so charge only the growth since the last charge.
    fn usage(&self, charged: Option<f64>) -> Usage {
        Usage {
            cost_usd: self
                .session_cost
                .map(|total| (total - charged.unwrap_or_default()).max(0.0)),
            ..Usage::default()
        }
    }
}

struct TaskPolicy {
    store: Arc<Store>,
    task_id: String,
    turn_id: i64,
    cwd: PathBuf,
    scope: TaskScope,
    /// Every request is a question for a person, and nobody is there to answer.
    declines_questions: bool,
    /// Skill folders, which the agent reads whatever the scope.
    skills: Vec<PathBuf>,
    refused: Refused,
}

impl AcpPolicy for TaskPolicy {
    fn permission(&self, request: RequestPermissionRequest) -> PolicyFuture<'_, Decision> {
        Box::pin(async move {
            let fields = &request.tool_call.fields;
            let writes = matches!(
                fields.kind,
                Some(ToolKind::Edit | ToolKind::Delete | ToolKind::Move)
            );
            let rules = if writes {
                &self.scope.write
            } else {
                &self.scope.read
            };
            let paths: Vec<&Path> = fields
                .locations
                .iter()
                .flatten()
                .map(|location| location.path.as_path())
                .collect();
            let outside: Vec<String> = paths
                .iter()
                .filter(|path| !scope_covers(&self.cwd, rules, path))
                .filter(|path| {
                    writes
                        || !self
                            .skills
                            .iter()
                            .any(|skills| relative_inside(skills, path).is_some())
                })
                .map(|path| path.display().to_string())
                .collect();
            let allowed = outside.is_empty() && !self.declines_questions;
            let decision = choose(&request.options, allowed);
            if !allowed {
                self.refused
                    .lock()
                    .expect("refused tool call lock is not poisoned")
                    .insert(
                        request.tool_call.tool_call_id.0.to_string(),
                        outside.clone(),
                    );
            }
            let mut payload = json!({
                "toolCallId": request.tool_call.tool_call_id.0.as_ref(),
                "title": fields.title,
                "kind": fields.kind,
                "access": if writes { "write" } else { "read" },
                "allowed": allowed,
                "outsideScope": outside,
            });
            if self.declines_questions {
                payload["unattended"] = json!(true);
            }
            if let Err(error) = append_turn_event(
                &self.store,
                &self.task_id,
                self.turn_id,
                "permission_answered".into(),
                payload,
            )
            .await
            {
                eprintln!(
                    "recording a permission answer for {} failed: {error}",
                    self.task_id
                );
            }
            decision
        })
    }
}

fn choose(options: &[PermissionOption], allowed: bool) -> Decision {
    let preferred = if allowed {
        [
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ]
    } else {
        [
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ]
    };
    preferred
        .iter()
        .find_map(|kind| options.iter().find(|option| option.kind == *kind))
        .map_or_else(
            || Decision::Refuse {
                message: if allowed {
                    "the agent offered no way to allow this".into()
                } else {
                    "this reaches outside the task's scope".into()
                },
            },
            |option| Decision::Select(option.option_id.clone()),
        )
}

fn scope_covers(cwd: &Path, rules: &[String], path: &Path) -> bool {
    let Some(relative) = relative_inside(cwd, path) else {
        return false;
    };
    rules.iter().any(|rule| {
        let mut rule = rule.trim().replace('\\', "/");
        while let Some(stripped) = rule.strip_prefix("./") {
            rule = stripped.to_owned();
        }
        let rule = rule.trim_end_matches('/');
        if rule == "**" {
            return true;
        }
        let base = rule.strip_suffix("/**").unwrap_or(rule);
        relative == base || relative.starts_with(&format!("{base}/"))
    })
}

fn relative_inside(cwd: &Path, path: &Path) -> Option<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let normal = normalize(&absolute)?;
    let bases = [
        normalize(cwd),
        std::fs::canonicalize(cwd)
            .ok()
            .and_then(|real| normalize(&real)),
    ];
    bases.into_iter().flatten().find_map(|base| {
        normal
            .strip_prefix(&base)
            .ok()
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
    })
}

fn normalize(path: &Path) -> Option<PathBuf> {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normal.pop() {
                    return None;
                }
            }
            other => normal.push(other),
        }
    }
    Some(normal)
}

#[cfg(test)]
mod tests {
    use oga_acp::schema::{
        PermissionOptionId, ToolCallLocation, ToolCallUpdate, ToolCallUpdateFields,
    };

    use super::*;

    fn rules(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn scope_rules_read_like_the_sandbox() {
        let cwd = Path::new("/repo");
        let write = rules(&["src/**", "README.md", "docs"]);
        assert!(scope_covers(cwd, &write, Path::new("/repo/src/lib.rs")));
        assert!(scope_covers(cwd, &write, Path::new("/repo/README.md")));
        assert!(scope_covers(cwd, &write, Path::new("/repo/docs/guide.md")));
        assert!(!scope_covers(cwd, &write, Path::new("/repo/srcs/lib.rs")));
        assert!(!scope_covers(
            cwd,
            &write,
            Path::new("/repo/src/../Cargo.toml")
        ));
        assert!(!scope_covers(cwd, &write, Path::new("/etc/passwd")));
        assert!(scope_covers(
            cwd,
            &rules(&["**"]),
            Path::new("/repo/any/where")
        ));
        assert!(!scope_covers(cwd, &rules(&["**"]), Path::new("/elsewhere")));
    }

    fn option(id: &str, kind: PermissionOptionKind) -> PermissionOption {
        PermissionOption::new(PermissionOptionId::new(id), id, kind)
    }

    #[test]
    fn an_answer_is_a_one_time_choice_matching_the_verdict() {
        let options = [
            option("always", PermissionOptionKind::AllowAlways),
            option("once", PermissionOptionKind::AllowOnce),
            option("no", PermissionOptionKind::RejectOnce),
        ];
        assert_eq!(
            choose(&options, true),
            Decision::Select(PermissionOptionId::new("once"))
        );
        assert_eq!(
            choose(&options, false),
            Decision::Select(PermissionOptionId::new("no"))
        );
        assert!(matches!(
            choose(&options[..2], false),
            Decision::Refuse { .. }
        ));
    }

    #[tokio::test]
    async fn a_write_outside_the_scope_is_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        let policy = TaskPolicy {
            store: Arc::clone(&store),
            task_id: "task".into(),
            turn_id: 1,
            cwd: PathBuf::from("/repo"),
            scope: TaskScope {
                read: rules(&["**"]),
                write: rules(&["src/**"]),
            },
            declines_questions: false,
            skills: vec![PathBuf::from("/home/.agents/skills")],
            refused: Refused::default(),
        };
        let request = |path: &str| {
            RequestPermissionRequest::new(
                "session",
                ToolCallUpdate::new(
                    "tool",
                    ToolCallUpdateFields::new()
                        .kind(ToolKind::Edit)
                        .locations(vec![ToolCallLocation::new(path)]),
                ),
                vec![
                    option("yes", PermissionOptionKind::AllowOnce),
                    option("no", PermissionOptionKind::RejectOnce),
                ],
            )
        };

        let inside = policy.permission(request("/repo/src/main.rs")).await;
        let outside = policy.permission(request("/repo/Cargo.toml")).await;
        let skill = policy
            .permission(request("/home/.agents/skills/ux/SKILL.md"))
            .await;

        assert_eq!(inside, Decision::Select(PermissionOptionId::new("yes")));
        assert_eq!(outside, Decision::Select(PermissionOptionId::new("no")));
        assert_eq!(skill, Decision::Select(PermissionOptionId::new("no")));
    }

    #[tokio::test]
    async fn a_skill_folder_reads_whatever_the_scope() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        let policy = TaskPolicy {
            store: Arc::clone(&store),
            task_id: "task".into(),
            turn_id: 1,
            cwd: PathBuf::from("/repo"),
            scope: TaskScope {
                read: rules(&["**"]),
                write: rules(&["src/**"]),
            },
            declines_questions: false,
            skills: vec![PathBuf::from("/home/.agents/skills")],
            refused: Refused::default(),
        };
        let read = |path: &str| {
            RequestPermissionRequest::new(
                "session",
                ToolCallUpdate::new(
                    "tool",
                    ToolCallUpdateFields::new()
                        .kind(ToolKind::Read)
                        .locations(vec![ToolCallLocation::new(path)]),
                ),
                vec![
                    option("yes", PermissionOptionKind::AllowOnce),
                    option("no", PermissionOptionKind::RejectOnce),
                ],
            )
        };

        let skill = policy
            .permission(read("/home/.agents/skills/ux/references/copy.md"))
            .await;
        let elsewhere = policy.permission(read("/home/.cargo/registry/src")).await;

        assert_eq!(skill, Decision::Select(PermissionOptionId::new("yes")));
        assert_eq!(elsewhere, Decision::Select(PermissionOptionId::new("no")));
    }
}
