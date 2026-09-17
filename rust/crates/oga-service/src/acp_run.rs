//! One run over ACP: open or restore the agent's session, send the prompt
//! once, and settle the task from the agent's answer.
//!
//! The prompt turn decides the task, not the process: an agent is a server and
//! may stay up after it answers, so the run ends it once the turn is over and
//! only then settles. A failure after the prompt was written is reported, never
//! run again, because the turn may already have happened.

use std::{
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use oga_acp::{
    AcpConfig, AcpError, AcpPolicy, AcpSession, Decision, Launch, PolicyFuture, Refusal,
    SessionStart, Stage,
    schema::{
        AgentCapabilities, ContentBlock, HttpHeader, McpServer, McpServerHttp, PermissionOption,
        PermissionOptionKind, RequestPermissionRequest, SessionId, SessionNotification,
        SessionUpdate, StopReason, ToolKind,
    },
};
use oga_domain::{
    AcpAgentIdentity, AcpRestore, CompletionCode, Profile, Task, TaskScope, TaskState,
    TaskTransport, TaskWorker, Transport, TransportReason,
};
use oga_providers::{AcpAdapter, AcpLaunch, NO_FINAL_MESSAGE, Usage};
use oga_runner::{ProviderRunner, RunRequest, Termination};
use oga_store::{Store, StoreError};
use rusqlite::params;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::{
    lifecycle::{
        ActiveRun, ActiveRuns, LifecycleError, RunOutcome, Settlement, append_event_tx,
        broker_base_url, completion, encode, load_task, now_iso, record_profile_outcome,
        settle_task, worker_env,
    },
    prompt::{WorkerOutcome, interpret_worker_outcome},
    transport::AcpStart,
};

/// How long a turn may run when the task names no timeout of its own. The
/// longest timeout a task can ask for, so no run outlives what a caller could
/// have chosen.
const LONGEST_TURN: Duration = Duration::from_secs(24 * 60 * 60);

/// How a run over ACP ended, from the lifecycle's point of view.
pub(crate) enum AcpEnd {
    Settled(Box<RunOutcome>),
    /// ACP never became usable, no prompt was written, and this run may use
    /// the command line instead. Carries the decision to record first.
    FallBack(TaskTransport),
}

/// Everything one ACP run needs from the lifecycle that claimed it.
pub(crate) struct AcpTurn<'a> {
    pub(crate) store: &'a Arc<Store>,
    pub(crate) runner: &'a ProviderRunner,
    /// The claimed, running task.
    pub(crate) task: &'a Task,
    pub(crate) profile: &'a Profile,
    pub(crate) adapter: &'a AcpAdapter,
    pub(crate) start: AcpStart,
    pub(crate) may_fall_back: bool,
    pub(crate) prompt: &'a str,
    pub(crate) turn_id: i64,
    pub(crate) active: &'a ActiveRuns,
}

/// A live ACP run, as cancel and handoff reach it.
pub(crate) struct AcpRun {
    session: AcpSession,
    cancelled: AtomicBool,
}

impl AcpRun {
    /// Asks the agent to stop, then ends its process the way a command-line
    /// run is ended, so a stop never depends on the agent cooperating.
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.session.cancel();
        self.session.process().terminate(Termination::Cancelled);
    }

    fn was_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
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
    let command = adapter.command(&AcpLaunch {
        profile,
        model: &task.model,
        effort: task.effort.as_deref(),
        cwd: &task.cwd,
    });
    let request = RunRequest::from_command(profile.provider, command, &task.cwd)
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
    let mut launch = Launch::new(request, task.scope.clone(), start);
    if adapter.oga_tools {
        launch = launch.mcp_servers(vec![oga_mcp_server(&task.id)]);
    }
    let policy = Arc::new(TaskPolicy {
        store: Arc::clone(store),
        task_id: task.id.clone(),
        turn_id: turn.turn_id,
        cwd: PathBuf::from(&task.cwd),
        scope: task.scope.clone(),
    });
    let turn_bound = task.timeout_ms.map_or(LONGEST_TURN, Duration::from_millis);
    let config = AcpConfig {
        // The lifecycle enforces the task's own bound; the transport's is only
        // a backstop behind it.
        prompt_timeout: turn_bound + Duration::from_secs(60),
        ..AcpConfig::default()
    };
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
    // `session/load` replays the whole conversation before it answers. None of
    // that is this turn, so it is counted and dropped rather than recorded.
    let mut replayed = 0usize;
    while updates.try_recv().is_ok() {
        replayed += 1;
    }
    if let Err(error) = record_session(&turn, &session, replayed) {
        session.shutdown().await;
        return Err(error);
    }

    let run = Arc::new(AcpRun {
        session,
        cancelled: AtomicBool::new(false),
    });
    let active = ActiveRun::Acp(Arc::clone(&run));
    turn.active.insert(&task.id, active.clone());

    let mut transcript = Transcript::default();
    let ended = match load_task(store, &task.id) {
        Ok(current) => {
            if current.is_none_or(|current| current.state != TaskState::Running) {
                run.cancel();
            }
            converse(&turn, &run, &mut updates, &mut transcript, turn_bound).await
        }
        Err(error) => Err(error.into()),
    };
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

/// Sends the prompt once and records the agent's updates until it answers or
/// the task's time runs out.
async fn converse(
    turn: &AcpTurn<'_>,
    run: &AcpRun,
    updates: &mut mpsc::UnboundedReceiver<SessionNotification>,
    transcript: &mut Transcript,
    turn_bound: Duration,
) -> Result<TurnEnd, LifecycleError> {
    let prompt = run
        .session
        .prompt(vec![ContentBlock::from(turn.prompt.to_owned())]);
    tokio::pin!(prompt);
    let deadline = tokio::time::sleep(turn_bound);
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

/// ACP could not be used for this run. Only an agent that never became usable,
/// on a run allowed to, moves to the command line; everything else is settled
/// where a person can see it.
fn open_failed(turn: &AcpTurn<'_>, error: AcpError) -> Result<AcpEnd, LifecycleError> {
    let now = now_iso();
    let (worker, forget_session) = match error {
        AcpError::Unavailable { stage, reason } => {
            if turn.may_fall_back {
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
        AcpError::PromptInFlight { reason } => (
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

/// Writes the transport this run settled on and the process behind it before
/// the prompt goes out, so a broker that stops mid-turn still finds both.
fn record_session(
    turn: &AcpTurn<'_>,
    session: &AcpSession,
    replayed: usize,
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
        agent: Some(AcpAgentIdentity {
            adapter: turn.adapter.id.clone(),
            name: session.agent_info().map(|info| info.name.clone()),
            version: session.agent_info().map(|info| info.version.clone()),
            protocol_version: session.protocol_version().as_u16(),
        }),
        decided_at: task
            .transport
            .as_ref()
            .filter(|recorded| recorded.kind == Transport::Acp)
            .map_or_else(|| now.clone(), |recorded| recorded.decided_at.clone()),
    };
    let identity = session.process().identity();
    let worker = TaskWorker {
        pid: identity.pid,
        pgid: identity.pgid,
        broker_pid: std::process::id(),
        started_at: now.clone(),
    };
    let native_session = turn.adapter.native_session.then(|| acp_session_id.clone());
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

/// Resume replays nothing, so it is preferred wherever the agent offers it.
fn restore_for(capabilities: &AgentCapabilities) -> Option<AcpRestore> {
    if capabilities.session_capabilities.resume.is_some() {
        Some(AcpRestore::Resume)
    } else if capabilities.load_session {
        Some(AcpRestore::Load)
    } else {
        None
    }
}

/// Oga's own tools, bound to this task by header so the broker answers as the
/// task rather than trusting an id the worker names. Added to the servers the
/// agent already loads from its own configuration, never in place of them.
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
    let answer = match ended {
        TurnEnd::TimedOut => {
            return failed(CompletionCode::Timeout, "provider run timed out".into());
        }
        TurnEnd::Answered(_) if was_cancelled => return cancelled(),
        TurnEnd::Answered(answer) => answer,
    };
    match answer {
        Ok(response) => match response.stop_reason {
            StopReason::EndTurn => {
                interpret_worker_outcome(Some(0), transcript.final_text(), stderr, None)
            }
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
        Err(AcpError::Refused {
            kind: Refusal::Authentication,
            reason,
        }) => failed(
            CompletionCode::Auth,
            format!(
                "This worker needs you to sign in again ({reason}). Sign in to that account, then resume the task."
            ),
        ),
        Err(AcpError::Refused {
            kind: Refusal::Permission,
            reason,
        }) => failed(
            CompletionCode::PermissionDenied,
            format!(
                "The worker declined part of this task ({reason}). Check what that worker is allowed to do, then resume the task."
            ),
        ),
        Err(AcpError::Unavailable { reason, .. } | AcpError::PromptInFlight { reason }) => {
            failed(CompletionCode::WorkerError, stopped_mid_turn(&reason))
        }
    }
}

fn stopped_mid_turn(reason: &str) -> String {
    format!(
        "The worker stopped partway through ({reason}). Some of the work may already be done, so Oga didn't start it over; resume the task to continue."
    )
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

/// Records one event on the running turn.
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

/// The agent's updates as trace events. Streamed message and thought chunks
/// are joined while they keep coming, so a sentence is one event rather than
/// one per token.
#[derive(Default)]
struct Transcript {
    pending: Option<(&'static str, String)>,
    /// The agent's last message: its text since the last tool call.
    final_message: String,
    /// The freshest total the agent reported for the whole session, in USD.
    session_cost: Option<f64>,
}

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

    fn final_text(&self) -> String {
        if self.final_message.trim().is_empty() {
            NO_FINAL_MESSAGE.to_owned()
        } else {
            self.final_message.clone()
        }
    }

    /// What this turn spent, from the totals the agent reported.
    ///
    /// ACP's released protocol publishes the session's own running totals, not
    /// one turn's share, so the charge is the growth over what the task has
    /// been charged already and never the whole total a second time. A session
    /// that counts from zero again — a new conversation after one was
    /// rejected — charges nothing until it passes what is already recorded.
    /// Token counts are not part of that surface, so they stay unknown rather
    /// than being guessed from the context window.
    fn usage(&self, charged: Option<f64>) -> Usage {
        Usage {
            cost_usd: self
                .session_cost
                .map(|total| (total - charged.unwrap_or_default()).max(0.0)),
            ..Usage::default()
        }
    }
}

/// Answers the agent's permission requests from the task's own authorization.
///
/// A task is already allowed to act inside its scope, which the runner's
/// sandbox enforces on the process itself; this policy only keeps the answers
/// the agent is given consistent with that scope. It grants no filesystem or
/// terminal callbacks, so the agent works through its own tools, inside the
/// same confinement a command-line run gets.
struct TaskPolicy {
    store: Arc<Store>,
    task_id: String,
    turn_id: i64,
    cwd: PathBuf,
    scope: TaskScope,
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
                .map(|path| path.display().to_string())
                .collect();
            let allowed = outside.is_empty();
            let decision = choose(&request.options, allowed);
            let payload = json!({
                "toolCallId": request.tool_call.tool_call_id.0.as_ref(),
                "title": fields.title,
                "kind": fields.kind,
                "access": if writes { "write" } else { "read" },
                "allowed": allowed,
                "outsideScope": outside,
            });
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

/// Picks the one-time option matching the verdict, so an answer never outlives
/// the request it was given for.
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

/// Whether a scope's rules cover a path, read the way the sandbox reads them:
/// relative to the task's directory, `**` for all of it, `dir/**` or a bare
/// directory for everything below, and a bare file for itself.
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

/// The path relative to `cwd`, or `None` when it lies outside it.
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

        assert_eq!(inside, Decision::Select(PermissionOptionId::new("yes")));
        assert_eq!(outside, Decision::Select(PermissionOptionId::new("no")));
    }
}
