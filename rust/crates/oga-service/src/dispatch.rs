//! Delegate planning, task creation, dependency holds, and worker launch.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use oga_config::DEFAULT_WORKER_PROMPT;
use oga_domain::{
    CompletionCode, HoldArgs, HoldVerb, MemoryEntry, OnBlockerFailure, ScopeGrant,
    SelectionDecision, Task, TaskCompletion, TaskKind, TaskScope, TaskState, TaskWorktree,
    WorktreeOption,
};
use oga_runner::ProviderRunner;
use oga_store::{Store, StoreError};
use oga_worktree::{
    create_task_worktree, current_branch, joined_worktree_of, project_cwd, worktree_request,
};
use rusqlite::params;
use serde::Serialize;
use serde_json::json;
use thiserror::Error;
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    ContinuationError,
    archive::{self, ArchiveRequest, ArchiveResult, WorktreeRemoveRequest},
    authorization,
    cancel::{self, CancelRequest},
    complete::{self, CompletionAssertion},
    handoff::{self, HandoffRequest},
    holds::{HoldSweep, HoldSweepReport},
    lifecycle::{self, ActiveRuns, MapFoldHook, NoopMapFoldHook, RunOptions},
    prompt::{self, WorkerPromptInput},
    reconcile,
    reply::{self, ReplyRequest},
    resume::{self, ResumeRequest},
    schedule::{StartAt, parse_start_at},
    steer::{self, SteerRequest},
    waiting,
};

const MAX_PREREQUISITES: usize = 16;
const DEPENDENCY_EXPIRY_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
/// Default wait before retrying a rate-limited task whose provider message
/// carried no parseable reset time.
const RATE_LIMIT_DEFAULT_DELAY_MS: i64 = 15 * 60 * 1_000;
/// How soon a parked run first tries the connection again. Later tries back off
/// from here inside the sweep.
const NETWORK_FIRST_CHECK_MS: i64 = 30 * 1_000;
const RATE_LIMIT_HOLD_EXPIRY_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
/// What a released resume hold tells the worker when the hold itself carried
/// no instruction of its own.
pub(crate) const CONTINUE_INSTRUCTION: &str =
    "Continue the original task from where the previous run stopped.";
/// Slack over the wake-watch interval before a tick counts as a suspend rather
/// than a slow scheduler.
const WAKE_GAP_TOLERANCE: Duration = Duration::from_secs(60);

/// Errors raised before or while a task is dispatched.
#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("dispatch refused: {0}")]
    Refusal(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Worktree(#[from] oga_worktree::WorktreeError),
    #[error(transparent)]
    Lifecycle(#[from] lifecycle::LifecycleError),
}

/// Input accepted by the Rust equivalent of `delegate`.
#[derive(Debug, Clone)]
pub struct DispatchRequest {
    pub profile_id: String,
    pub model: Option<String>,
    pub prompt: String,
    pub cwd: PathBuf,
    pub scope: TaskScope,
    pub grant_id: Option<String>,
    pub remember_scope: bool,
    pub allow_questions: bool,
    pub timeout: Option<Duration>,
    pub parent_task_id: Option<String>,
    pub orchestrator_id: Option<String>,
    pub kind: TaskKind,
    pub effort: Option<String>,
    pub tldr: Option<String>,
    pub title: Option<String>,
    pub worktree: Option<WorktreeOption>,
    pub depends_on: Vec<String>,
    pub on_blocker_failure: OnBlockerFailure,
    pub selection: Option<SelectionDecision>,
    pub worker_prompt: Option<String>,
    pub context_map: Option<String>,
    pub memories: Vec<MemoryEntry>,
    pub caller_id: Option<String>,
    /// The caller's own start time, unparsed. Absent means start now.
    pub start_at: Option<String>,
}

impl DispatchRequest {
    pub fn new(
        profile_id: impl Into<String>,
        prompt: impl Into<String>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            model: None,
            prompt: prompt.into(),
            cwd: cwd.into(),
            scope: TaskScope {
                read: vec!["**".into()],
                write: vec!["**".into()],
            },
            grant_id: None,
            remember_scope: false,
            allow_questions: true,
            timeout: None,
            parent_task_id: None,
            orchestrator_id: None,
            kind: TaskKind::Delegated,
            effort: None,
            tldr: None,
            title: None,
            worktree: None,
            depends_on: Vec::new(),
            on_blocker_failure: OnBlockerFailure::Hold,
            selection: None,
            worker_prompt: None,
            context_map: None,
            memories: Vec::new(),
            caller_id: None,
            start_at: None,
        }
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn scope(mut self, scope: TaskScope) -> Self {
        self.scope = scope;
        self.remember_scope = true;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn depends_on(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.depends_on = ids.into_iter().map(Into::into).collect();
        self
    }

    pub fn worktree(mut self, worktree: WorktreeOption) -> Self {
        self.worktree = Some(worktree);
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn tldr(mut self, tldr: impl Into<String>) -> Self {
        self.tldr = Some(tldr.into());
        self
    }

    pub fn start_at(mut self, start_at: impl Into<String>) -> Self {
        self.start_at = Some(start_at.into());
        self
    }
}

pub type DelegateRequest = DispatchRequest;

/// The complete row and launch context produced by planning.
#[derive(Debug, Clone)]
pub struct DispatchPlan {
    pub task: Task,
    pub profile: oga_domain::Profile,
    pub prompt: WorkerPromptInput,
    pub task_selection: Option<oga_domain::TaskSelection>,
    pub caller_id: Option<String>,
    pub dependencies: Vec<String>,
    pub hold: Option<oga_domain::TaskHold>,
    pub launch: bool,
    pub worktree_created: bool,
}

/// Result returned after the row is persisted. The worker itself is detached.
#[derive(Debug, Clone)]
pub struct DispatchResult {
    pub task: Task,
    pub launched: bool,
}

/// Dispatch coordinator. It owns the store, runner, and live run handles.
#[derive(Clone)]
pub struct Dispatcher {
    store: Arc<Store>,
    runner: ProviderRunner,
    map_fold: Arc<dyn MapFoldHook>,
    active: ActiveRuns,
}

pub type TaskService = Dispatcher;

impl Dispatcher {
    pub fn new(store: Arc<Store>, runner: ProviderRunner) -> Self {
        Self {
            store,
            runner,
            map_fold: Arc::new(NoopMapFoldHook),
            active: ActiveRuns::default(),
        }
    }

    pub fn with_map_fold_hook(mut self, hook: Arc<dyn MapFoldHook>) -> Self {
        self.map_fold = hook;
        self
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub(crate) fn active_runs(&self) -> ActiveRuns {
        self.active.clone()
    }

    pub(crate) fn launch_continuation(
        &self,
        task: Task,
        profile: oga_domain::Profile,
        prompt: WorkerPromptInput,
        session_id: Option<String>,
    ) {
        self.launch_task(task, profile, prompt, session_id);
    }

    pub async fn reply(&self, request: ReplyRequest) -> Result<Task, ContinuationError> {
        reply::reply(self, request).await
    }

    pub async fn resume(&self, request: ResumeRequest) -> Result<Task, ContinuationError> {
        resume::resume(self, request).await
    }

    pub async fn steer(&self, request: SteerRequest) -> Result<Task, ContinuationError> {
        steer::steer(self, request).await
    }

    pub async fn handoff(&self, request: HandoffRequest) -> Result<Task, ContinuationError> {
        handoff::handoff(self, request).await
    }

    pub async fn cancel(&self, request: CancelRequest) -> Result<Task, ContinuationError> {
        cancel::cancel(self, request).await
    }

    pub async fn archive(
        &self,
        request: ArchiveRequest,
    ) -> Result<ArchiveResult, ContinuationError> {
        archive::archive(self, request).await
    }

    pub async fn remove_worktree(
        &self,
        request: WorktreeRemoveRequest,
    ) -> Result<oga_domain::WorktreeDeleteEntry, ContinuationError> {
        archive::remove_worktree(self, request).await
    }

    pub async fn remove_project_worktrees(
        &self,
        project: impl Into<String>,
        delete_branch: bool,
    ) -> Result<oga_domain::WorktreeDeleteBatchResult, ContinuationError> {
        archive::remove_project_worktrees(self, project, delete_branch).await
    }

    pub fn assert_completion(
        &self,
        assertion: CompletionAssertion,
    ) -> Result<Task, ContinuationError> {
        complete::assert_completion(self, assertion)
    }

    pub fn force_complete(
        &self,
        assertion: CompletionAssertion,
    ) -> Result<Task, ContinuationError> {
        complete::force_complete(self, assertion)
    }

    pub async fn plan(&self, request: DispatchRequest) -> Result<DispatchPlan, DispatchError> {
        validate_request(&request)?;
        let workspace = fs::canonicalize(&request.cwd).map_err(|error| {
            DispatchError::Refusal(format!(
                "cannot resolve cwd {}: {error}",
                request.cwd.display()
            ))
        })?;
        let profile = self
            .store
            .repositories()
            .profiles()
            .get(&request.profile_id)?
            .ok_or_else(|| {
                DispatchError::Refusal(format!("unknown profile: {}", request.profile_id))
            })?;
        if !profile.enabled {
            return Err(DispatchError::Refusal(format!(
                "profile disabled: {}",
                request.profile_id
            )));
        }
        let model = request
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .unwrap_or(profile.default_model.as_str())
            .to_owned();
        if model.len() > 200 {
            return Err(DispatchError::Refusal(
                "model exceeds 200 characters".into(),
            ));
        }
        authorization::check_model_enabled(
            &self.store,
            &workspace.to_string_lossy(),
            &profile.id,
            &model,
        )
        .map_err(DispatchError::Refusal)?;
        let parent = if let Some(parent_id) = request.parent_task_id.as_deref() {
            let parent = lifecycle::load_task(&self.store, parent_id)?.ok_or_else(|| {
                DispatchError::Refusal(format!("unknown parent task: {parent_id}"))
            })?;
            if parent.kind == Some(TaskKind::Orchestrator) {
                return Err(DispatchError::Refusal(format!(
                    "parent must name a delegated task: {parent_id}"
                )));
            }
            Some(parent)
        } else {
            None
        };

        let task_id = Uuid::new_v4().to_string();
        let grant_id = if request.remember_scope && request.grant_id.is_none() {
            let id = Uuid::new_v4().to_string();
            let now = lifecycle::now_iso();
            self.store.repositories().grants().upsert(&ScopeGrant {
                id: id.clone(),
                cwd: workspace.to_string_lossy().into_owned(),
                profile_id: profile.id.clone(),
                scope: request.scope.clone(),
                created_at: now.clone(),
                last_used_at: now,
                use_count: 1,
            })?;
            Some(id)
        } else {
            request.grant_id.clone()
        };
        let mut dependency_plan = self.plan_dependencies(&task_id, &request)?;
        if let Some(start_at) = request.start_at.as_deref() {
            dependency_plan = schedule_plan(dependency_plan, &task_id, start_at)?;
        }
        let (task_cwd, worktree, worktree_created, branch) = self
            .resolve_worktree(
                &workspace,
                &task_id,
                request.worktree.as_ref(),
                request.title.as_deref(),
            )
            .await?;
        let now = lifecycle::now_iso();
        let DependencyPlan {
            state,
            completion,
            hold,
        } = dependency_plan;
        let task_selection = request
            .selection
            .clone()
            .map(|selection| oga_domain::TaskSelection {
                record: selection.record,
                chosen: oga_domain::ChosenRoute {
                    profile_id: profile.id.clone(),
                    model: model.clone(),
                    effort: request.effort.clone(),
                },
            });
        let task = Task {
            id: task_id,
            kind: Some(request.kind),
            profile_id: profile.id.clone(),
            model,
            prompt: request.prompt.clone(),
            shipped_prompt: None,
            cwd: task_cwd.to_string_lossy().into_owned(),
            branch,
            worktree,
            worktree_label: None,
            state,
            created_at: now.clone(),
            updated_at: now,
            output: String::new(),
            error: completion.as_ref().and_then(|value| value.reason.clone()),
            question: None,
            parent_task_id: parent.map(|value| value.id),
            orchestrator_id: request.orchestrator_id.clone(),
            scope: request.scope.clone(),
            grant_id,
            allow_questions: request.allow_questions,
            timeout_ms: request.timeout.map(|value| value.as_millis() as u64),
            effort: request.effort.clone(),
            effort_actual: None,
            tldr: request.tldr.clone(),
            title: request.title.clone(),
            session_id: None,
            completion,
            attempts: Vec::new(),
            cost_usd: None,
            cost_usd_estimated: false,
            turns: None,
            archived_at: None,
            queued_follow_ups: None,
            queued_follow_up_items: None,
            hold: None,
        };
        let prompt = WorkerPromptInput {
            task: request.prompt,
            allow_questions: request.allow_questions,
            scope: Some(request.scope),
            worker_prompt: request
                .worker_prompt
                .unwrap_or_else(|| DEFAULT_WORKER_PROMPT.to_owned()),
            context_map: request.context_map,
            memories: if request.memories.is_empty() {
                self.store
                    .repositories()
                    .memories()
                    .list(&workspace.to_string_lossy())?
            } else {
                request.memories
            },
        };
        let hold = hold.map(|mut hold| {
            hold.task_id = task.id.clone();
            hold
        });
        Ok(DispatchPlan {
            task,
            profile,
            prompt,
            task_selection,
            caller_id: request.caller_id,
            dependencies: request.depends_on,
            hold,
            launch: state == TaskState::Queued,
            worktree_created,
        })
    }

    pub async fn dispatch(
        &self,
        request: DispatchRequest,
    ) -> Result<DispatchResult, DispatchError> {
        let plan = self.plan(request).await?;
        let task_id = plan.task.id.clone();
        let launched = plan.launch;
        if let Err(error) = persist_plan(&self.store, &plan) {
            if plan.worktree_created
                && let Some(worktree) = &plan.task.worktree
            {
                let _ = oga_worktree::remove_task_worktree(worktree).await;
                let _ = oga_worktree::remove_task_branch(worktree).await;
            }
            return Err(error);
        }
        if launched {
            self.launch(plan);
        }
        let task = lifecycle::load_task(&self.store, &task_id)?.ok_or_else(|| {
            DispatchError::Refusal(format!("task disappeared after dispatch: {task_id}"))
        })?;
        Ok(DispatchResult { task, launched })
    }

    pub async fn dispatch_and_wait(&self, request: DispatchRequest) -> Result<Task, DispatchError> {
        let result = self.dispatch(request).await?;
        if !result.launched {
            return self.task(&result.task.id);
        }
        loop {
            let task = self.task(&result.task.id)?;
            if task.state.settled() {
                return Ok(task);
            }
            sleep(Duration::from_millis(5)).await;
        }
    }

    pub fn task(&self, id: &str) -> Result<Task, DispatchError> {
        lifecycle::load_task(&self.store, id)?
            .ok_or_else(|| DispatchError::Refusal(format!("unknown task: {id}")))
    }

    fn launch(&self, plan: DispatchPlan) {
        self.launch_task(plan.task, plan.profile, plan.prompt, None);
    }

    /// Detach one run and the chain of dependents its ending releases.
    fn launch_task(
        &self,
        task: Task,
        profile: oga_domain::Profile,
        prompt: WorkerPromptInput,
        session_id: Option<String>,
    ) {
        let dispatcher = self.clone();
        let task_id = task.id.clone();
        let task_updated_at = task.updated_at.clone();
        tokio::spawn(async move {
            match lifecycle::run_task_and_release_with_active(
                dispatcher.store.clone(),
                dispatcher.runner.clone(),
                task,
                profile,
                prompt,
                dispatcher.map_fold.clone(),
                RunOptions {
                    session_id,
                    active: dispatcher.active.clone(),
                },
            )
            .await
            {
                Ok(outcome) => {
                    if !dispatcher.park_unattended_failure(&outcome.task).await {
                        dispatcher.drain_follow_ups(&outcome.task);
                    }
                }
                Err(error) => {
                    let _ = lifecycle::fail_unstarted_task(
                        &dispatcher.store,
                        &task_id,
                        &task_updated_at,
                        &error.to_string(),
                    );
                    if let Ok(Some(task)) = lifecycle::load_task(&dispatcher.store, &task_id) {
                        dispatcher.drain_follow_ups(&task);
                        dispatcher.settle_dependents(&task);
                    }
                }
            }
        });
    }

    /// The two endings nobody should have to watch for: the account ran out of
    /// usage, and the connection went away. Both park the task on a hold the
    /// sweep picks back up, so the run continues without anyone noticing it
    /// stopped. Every other ending stands as it is.
    async fn park_unattended_failure(&self, task: &Task) -> bool {
        if task.state != TaskState::Failed {
            return false;
        }
        match task.completion.as_ref().map(|completion| completion.code) {
            Some(CompletionCode::RateLimit) => self.park_rate_limited(task).await,
            Some(CompletionCode::Network) => self.park_disconnected(task),
            _ => false,
        }
    }

    /// A rate-limited task waits for the reset the provider named. When the
    /// project asked for it and another worker of the same calibre is free, it
    /// moves there instead of waiting at all.
    async fn park_rate_limited(&self, task: &Task) -> bool {
        let settings = waiting::wait_settings(&self.store);
        if settings.move_on_rate_limit && self.move_to_free_worker(task).await {
            return true;
        }
        let start_at = task
            .completion
            .as_ref()
            .and_then(|completion| completion.resets_at.clone())
            .unwrap_or_else(|| {
                oga_routing::format_rfc3339_ms(
                    oga_routing::now_ms().saturating_add(RATE_LIMIT_DEFAULT_DELAY_MS),
                )
            });
        let now = lifecycle::now_iso();
        let hold = oga_domain::TaskHold {
            task_id: task.id.clone(),
            verb: HoldVerb::Resume,
            args: HoldArgs::default(),
            start_at: Some(start_at.clone()),
            await_profile: Some(task.profile_id.clone()),
            await_model: Some(task.model.clone()),
            next_check_at: start_at.clone(),
            expires_at: oga_routing::format_rfc3339_ms(
                oga_routing::now_ms().saturating_add(RATE_LIMIT_HOLD_EXPIRY_MS),
            ),
            probe_count: 0,
            note: waiting::rate_limit_wait_note(&start_at),
            created_at: now.clone(),
            updated_at: now,
        };
        HoldSweep::new(self.store.clone()).arm(&hold).is_ok()
    }

    /// A run the network killed waits for the connection instead of dying with
    /// it. The give-up budget is cumulative across parks, so a connection that
    /// keeps flapping cannot hold one task forever.
    fn park_disconnected(&self, task: &Task) -> bool {
        let settings = waiting::wait_settings(&self.store);
        let spent = self.network_parks(&task.id);
        if spent >= settings.network_max_attempts {
            let _ = self.append_event(
                &task.id,
                "network_retry_exhausted",
                task.state,
                json!({"attempts": spent}),
            );
            return false;
        }
        let now = lifecycle::now_iso();
        let original_error = task
            .error
            .clone()
            .or_else(|| {
                task.completion
                    .as_ref()
                    .and_then(|completion| completion.reason.clone())
            })
            .unwrap_or_else(|| "network error".into());
        let next_check_at = oga_routing::format_rfc3339_ms(
            oga_routing::now_ms().saturating_add(NETWORK_FIRST_CHECK_MS),
        );
        let expires_at = oga_routing::format_rfc3339_ms(
            oga_routing::now_ms()
                .saturating_add((settings.network_max_wait_minutes as i64).saturating_mul(60_000)),
        );
        let hold = oga_domain::TaskHold {
            task_id: task.id.clone(),
            verb: HoldVerb::Resume,
            args: HoldArgs {
                network: Some(oga_domain::NetworkHoldArgs {
                    attempt: spent,
                    max_attempts: settings.network_max_attempts,
                    original_error: Some(original_error.clone()),
                }),
                ..HoldArgs::default()
            },
            start_at: None,
            await_profile: None,
            await_model: None,
            next_check_at: next_check_at.clone(),
            expires_at: expires_at.clone(),
            probe_count: 0,
            note: waiting::NETWORK_WAIT_NOTE.into(),
            created_at: now.clone(),
            updated_at: now,
        };
        if HoldSweep::new(self.store.clone()).arm(&hold).is_err() {
            return false;
        }
        let _ = self.append_event(
            &task.id,
            "network_retry_scheduled",
            TaskState::Pending,
            json!({
                "originalError": original_error,
                "nextCheckAt": next_check_at,
                "expiresAt": expires_at,
                "maxAttempts": settings.network_max_attempts,
            }),
        );
        true
    }

    /// How much of the give-up budget this task has already spent. The hold row
    /// is gone once it releases, so the count lives in the trace.
    fn network_parks(&self, task_id: &str) -> u32 {
        self.store
            .repositories()
            .events()
            .list(task_id)
            .map(|events| {
                events
                    .iter()
                    .filter(|event| event.kind == "network_retry_scheduled")
                    .count() as u32
            })
            .unwrap_or(0)
    }

    async fn move_to_free_worker(&self, task: &Task) -> bool {
        let probe = waiting::StoreProbe::new(self.store.clone());
        let Some((profile_id, model)) = waiting::alternative_worker(&self.store, task, &probe)
        else {
            return false;
        };
        self.handoff(
            HandoffRequest::new(task.id.clone())
                .profile(profile_id)
                .model(model),
        )
        .await
        .is_ok()
    }

    fn append_event(
        &self,
        task_id: &str,
        kind: &str,
        state: TaskState,
        payload: serde_json::Value,
    ) -> Result<(), StoreError> {
        let now = lifecycle::now_iso();
        self.store.transaction(|tx| {
            append_event_tx(tx, task_id, kind, state, payload, &now, None)?;
            Ok(())
        })
    }

    pub(crate) fn drain_follow_ups(&self, task: &Task) {
        let store = self.store.clone();
        let task_id = task.id.clone();
        let state = task.state;
        let now = lifecycle::now_iso();
        let feed = match crate::follow_ups::feed_follow_up(&store, &task_id, state, &now) {
            Ok(feed) => feed,
            Err(error) => {
                eprintln!("follow-up drain failed for {}: {error}", task.id);
                return;
            }
        };
        if let Some(instruction) = feed.instruction {
            let dispatcher = self.clone();
            let store_clone = store.clone();
            let task_id_clone = task_id.clone();
            let instruction_for_error = instruction.clone();
            tokio::spawn(async move {
                let request = crate::resume::ResumeRequest::new(task_id_clone.clone())
                    .instruction(instruction.clone());
                if let Err(error) = dispatcher.resume(request).await {
                    let now = lifecycle::now_iso();
                    let _ = store_clone.transaction(|tx| {
                        let waiting: i64 = tx.query_row(
                            "SELECT COUNT(*) FROM task_follow_ups WHERE task_id=?",
                            [&task_id_clone],
                            |row| row.get(0),
                        )?;
                        crate::append_event_tx(
                            tx,
                            &task_id_clone,
                            "follow_ups_paused",
                            oga_domain::TaskState::Completed,
                            serde_json::json!({
                                "instruction": instruction_for_error,
                                "waiting": waiting,
                                "error": error.to_string()
                            }),
                            &now,
                        )?;
                        Ok(())
                    });
                    eprintln!("follow-up resume failed for {}: {error}", task_id_clone);
                }
            });
        }
    }

    /// Settle every run the store still believes is in flight against what is
    /// actually true of its worker, and park the ones worth continuing. Safe to
    /// call at any time: a run this broker is supervising is never touched.
    pub fn reconcile(
        &self,
        trigger: reconcile::ReconcileTrigger,
    ) -> Result<reconcile::ReconcileReport, DispatchError> {
        Ok(reconcile::reconcile(
            &self.store,
            trigger,
            &self.active,
            &reconcile::SystemProbe,
        )?)
    }

    /// Notice the machine having been suspended and check every run against it.
    /// Nothing else fires while the host is asleep, so the first tick after
    /// waking is where a wall clock far ahead of the tick interval shows up.
    pub fn start_wake_watch(&self, interval: Duration) -> tokio::task::JoinHandle<()> {
        let dispatcher = self.clone();
        tokio::spawn(async move {
            loop {
                let before = SystemTime::now();
                sleep(interval).await;
                let slept = SystemTime::now()
                    .duration_since(before)
                    .unwrap_or(Duration::ZERO);
                if slept < interval + WAKE_GAP_TOLERANCE {
                    continue;
                }
                match dispatcher.reconcile(reconcile::ReconcileTrigger::Wake) {
                    Ok(report) if report.touched() > 0 => eprintln!(
                        "recovered {} interrupted runs after a {}s gap",
                        report.touched(),
                        slept.as_secs()
                    ),
                    Ok(_) => {}
                    Err(error) => eprintln!("wake recovery failed: {error}"),
                }
            }
        })
    }

    /// One pass of the hold sweep, launching every task it releases. A hold is
    /// the broker's only timer — a scheduled start, a network park, a
    /// prerequisite still to re-check — so without a pass on a schedule none of
    /// them ever come due.
    pub fn sweep_holds(&self) -> Result<HoldSweepReport, DispatchError> {
        let report = HoldSweep::new(self.store.clone())
            .sweep_with(&waiting::StoreProbe::new(self.store.clone()))
            .map_err(|error| DispatchError::Refusal(error.to_string()))?;
        for hold in &report.released {
            let Some(task) = lifecycle::load_task(&self.store, &hold.task_id)? else {
                continue;
            };
            match self.store.repositories().profiles().get(&task.profile_id) {
                Ok(Some(profile)) => {
                    // A task that captured a session is continued in it. The
                    // rate-limit hold, the restart hold and every other resume
                    // park exist to pick a run back up, and re-shipping the
                    // original prompt would start it over instead.
                    let session_id = task
                        .session_id
                        .clone()
                        .filter(|_| hold.verb == HoldVerb::Resume && profile.command.is_none());
                    let prompt = match &session_id {
                        Some(_) => WorkerPromptInput {
                            task: crate::resume_prompt(
                                TaskState::Cancelled,
                                hold.args
                                    .instruction
                                    .as_deref()
                                    .unwrap_or(CONTINUE_INSTRUCTION),
                                true,
                            ),
                            allow_questions: task.allow_questions,
                            scope: Some(task.scope.clone()),
                            ..WorkerPromptInput::default()
                        },
                        None => WorkerPromptInput {
                            task: task.prompt.clone(),
                            allow_questions: task.allow_questions,
                            scope: Some(task.scope.clone()),
                            worker_prompt: DEFAULT_WORKER_PROMPT.to_owned(),
                            ..WorkerPromptInput::default()
                        },
                    };
                    self.launch_task(task, profile, prompt, session_id);
                }
                _ => {
                    let _ = lifecycle::block_queued_task(
                        &self.store,
                        &hold.task_id,
                        "unknown profile for held task",
                    );
                }
            }
        }
        Ok(report)
    }

    /// Run the hold sweep for as long as the broker is up. The first pass runs
    /// straight away: a hold is persisted state and a timer is not, so a hold
    /// that came due while the broker was down has nothing else to catch it.
    pub fn start_hold_sweep(&self, interval: Duration) -> tokio::task::JoinHandle<()> {
        let dispatcher = self.clone();
        tokio::spawn(async move {
            loop {
                // A pass probes accounts and the network, so it runs off the
                // async runtime rather than stalling it for its timeouts.
                let pass = dispatcher.clone();
                match tokio::task::spawn_blocking(move || pass.sweep_holds()).await {
                    Ok(Err(error)) => eprintln!("hold sweep failed: {error}"),
                    Err(error) => eprintln!("hold sweep failed: {error}"),
                    Ok(Ok(_)) => {}
                }
                sleep(interval).await;
            }
        })
    }

    /// A task that leaves a blocking end — resumed, handed off — was the reason
    /// its dependents were dropped, so they go back to waiting.
    pub(crate) fn restore_dropped_dependents(&self, task: &Task) {
        if let Err(error) = lifecycle::restore_dependency_blocked_dependents(&self.store, task) {
            eprintln!("restoring dependents of {} failed: {error}", task.id);
        }
    }

    /// The dependency trigger for a settle written straight to the store rather
    /// than reached through a run: an asserted completion, a cancel. Dependents
    /// an earlier ending dropped go back to waiting, and the ones now clear to
    /// start are launched.
    pub(crate) fn settle_dependents(&self, blocker: &Task) {
        if let Err(error) = lifecycle::restore_dependency_blocked_dependents(&self.store, blocker) {
            eprintln!("restoring dependents of {} failed: {error}", blocker.id);
        }
        let released = match lifecycle::release_dependents(&self.store, blocker) {
            Ok(released) => released,
            Err(error) => {
                eprintln!("releasing dependents of {} failed: {error}", blocker.id);
                return;
            }
        };
        for queued in released {
            let profile = self.store.repositories().profiles().get(&queued.profile_id);
            match profile {
                Ok(Some(profile)) => {
                    let prompt = WorkerPromptInput {
                        task: queued.prompt.clone(),
                        allow_questions: queued.allow_questions,
                        scope: Some(queued.scope.clone()),
                        worker_prompt: oga_config::DEFAULT_WORKER_PROMPT.to_owned(),
                        ..WorkerPromptInput::default()
                    };
                    self.launch_task(queued, profile, prompt, None);
                }
                _ => {
                    let _ = lifecycle::block_queued_task(
                        &self.store,
                        &queued.id,
                        "unknown profile for dependent task",
                    );
                }
            }
        }
    }

    async fn resolve_worktree(
        &self,
        workspace: &Path,
        task_id: &str,
        option: Option<&WorktreeOption>,
        title: Option<&str>,
    ) -> Result<(PathBuf, Option<TaskWorktree>, bool, Option<String>), DispatchError> {
        let Some(request) = worktree_request(option) else {
            return Ok((
                workspace.to_path_buf(),
                None,
                false,
                current_branch(workspace).await?,
            ));
        };
        if let Some(join_id) = request.join.as_deref() {
            if request.branch.is_some() || request.from.is_some() || request.link.is_some() {
                return Err(DispatchError::Refusal(
                    "worktree.join cannot be combined with worktree.branch, worktree.from or worktree.link".into(),
                ));
            }
            let joined = lifecycle::load_task(&self.store, join_id)?
                .ok_or_else(|| DispatchError::Refusal(format!("unknown task: {join_id}")))?;
            let worktree = joined_worktree_of(&joined)?;
            if project_cwd(&joined) != workspace {
                return Err(DispatchError::Refusal(format!(
                    "worktree.join runs in the checkout's own project: {}",
                    worktree.origin_cwd
                )));
            }
            return Ok((
                PathBuf::from(&joined.cwd),
                Some(worktree.clone()),
                false,
                Some(worktree.branch),
            ));
        }
        let created = create_task_worktree(workspace, task_id, &request, title).await?;
        let branch = created.worktree.branch.clone();
        Ok((created.cwd, Some(created.worktree), true, Some(branch)))
    }

    fn plan_dependencies(
        &self,
        task_id: &str,
        request: &DispatchRequest,
    ) -> Result<DependencyPlan, DispatchError> {
        let mut dependencies = request.depends_on.clone();
        dependencies.sort();
        dependencies.dedup();
        if dependencies.len() > MAX_PREREQUISITES {
            return Err(DispatchError::Refusal(format!(
                "too many prerequisites: {} > {MAX_PREREQUISITES}",
                dependencies.len()
            )));
        }
        if dependencies.iter().any(|id| id == task_id) {
            return Err(DispatchError::Refusal(format!(
                "task cannot depend on itself: {task_id}"
            )));
        }
        let blockers = dependencies
            .iter()
            .map(|id| {
                lifecycle::load_task(&self.store, id)
                    .map_err(DispatchError::from)
                    .and_then(|task| {
                        task.ok_or_else(|| {
                            DispatchError::Refusal(format!("unknown prerequisite task: {id}"))
                        })
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if self.dependency_would_cycle(task_id, &dependencies)? {
            return Err(DispatchError::Refusal(format!(
                "dependency cycle involving {task_id}"
            )));
        }
        if dependencies.is_empty() {
            return Ok(DependencyPlan::ready());
        }
        let all_settled = blockers.iter().all(|task| {
            task.state == TaskState::Completed
                || matches!(
                    task.state,
                    TaskState::Failed | TaskState::Cancelled | TaskState::Blocked
                )
        });
        let all_completed = blockers
            .iter()
            .all(|task| task.state == TaskState::Completed);
        if all_completed || (request.on_blocker_failure == OnBlockerFailure::Run && all_settled) {
            return Ok(DependencyPlan::ready());
        }
        if blockers.iter().any(|task| {
            matches!(
                task.state,
                TaskState::Failed | TaskState::Cancelled | TaskState::Blocked
            )
        }) {
            let blocker = blockers
                .iter()
                .find(|task| {
                    matches!(
                        task.state,
                        TaskState::Failed | TaskState::Cancelled | TaskState::Blocked
                    )
                })
                .expect("a blocker was found");
            let reason = format!(
                "did not start: {} ended {}",
                blocker.title.as_deref().unwrap_or(&blocker.id),
                blocker.state.as_str()
            );
            return Ok(DependencyPlan {
                state: TaskState::Blocked,
                completion: Some(TaskCompletion {
                    blocked: true,
                    code: CompletionCode::Cancelled,
                    reason: Some(reason),
                    dependency_blocked: Some(true),
                    ..prompt::empty_completion()
                }),
                hold: None,
            });
        }
        let now = lifecycle::now_iso();
        Ok(DependencyPlan {
            state: TaskState::Pending,
            completion: None,
            hold: Some(oga_domain::TaskHold {
                task_id: task_id.into(),
                verb: HoldVerb::Delegate,
                args: HoldArgs {
                    on_blocker_failure: Some(request.on_blocker_failure),
                    ..HoldArgs::default()
                },
                start_at: None,
                await_profile: None,
                await_model: None,
                next_check_at: now.clone(),
                expires_at: oga_routing::format_rfc3339_ms(
                    oga_routing::now_ms().saturating_add(DEPENDENCY_EXPIRY_MS),
                ),
                probe_count: 0,
                note: dependency_note(&blockers, request.on_blocker_failure),
                created_at: now.clone(),
                updated_at: now,
            }),
        })
    }

    fn dependency_would_cycle(
        &self,
        task_id: &str,
        dependencies: &[String],
    ) -> Result<bool, DispatchError> {
        self.store
            .with_connection(|connection| {
                let mut queue = dependencies.to_vec();
                let mut visited = std::collections::HashSet::new();
                while let Some(node) = queue.pop() {
                    if node == task_id {
                        return Ok(true);
                    }
                    if !visited.insert(node.clone()) {
                        continue;
                    }
                    let mut statement = connection
                        .prepare("SELECT blocker_id FROM task_dependencies WHERE task_id = ?")?;
                    let next = statement
                        .query_map([node], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    queue.extend(next);
                }
                Ok(false)
            })
            .map_err(DispatchError::from)
    }
}

fn dependency_note(blockers: &[Task], on_blocker_failure: OnBlockerFailure) -> String {
    let waiting_on = blockers
        .iter()
        .find(|task| task.state != TaskState::Completed)
        .map(|task| task.title.as_deref().unwrap_or(&task.id))
        .unwrap_or("prerequisite tasks");
    let suffix = match on_blocker_failure {
        OnBlockerFailure::Run => "; will start even if one fails",
        OnBlockerFailure::Hold => "",
    };
    format!("waiting for {waiting_on} to finish{suffix}")
}

#[derive(Debug)]
struct DependencyPlan {
    state: TaskState,
    completion: Option<TaskCompletion>,
    hold: Option<oga_domain::TaskHold>,
}

impl DependencyPlan {
    fn ready() -> Self {
        Self {
            state: TaskState::Queued,
            completion: None,
            hold: None,
        }
    }
}

/// Folds a caller's start time into the plan. A task already waiting on a
/// prerequisite keeps that wait and gains a floor under it; one that would
/// otherwise start now waits on the clock alone.
fn schedule_plan(
    plan: DependencyPlan,
    task_id: &str,
    start_at: &str,
) -> Result<DependencyPlan, DispatchError> {
    let now_ms = oga_routing::now_ms();
    let until = match parse_start_at(start_at, now_ms).map_err(DispatchError::Refusal)? {
        StartAt::At(instant) => instant,
        StartAt::WhenUsageResets => {
            return Err(DispatchError::Refusal(
                "startAt \"rate_limit\" continues a run that already hit one; a new task takes an ISO timestamp or a duration like \"30m\"".into(),
            ));
        }
    };
    if plan.state == TaskState::Blocked {
        return Ok(plan);
    }
    let note = waiting::scheduled_start_note(&until);
    let now = lifecycle::now_iso();
    let hold = match plan.hold {
        Some(hold) => oga_domain::TaskHold {
            note: format!("{}; {note}", hold.note),
            start_at: Some(until.clone()),
            next_check_at: until,
            ..hold
        },
        None => oga_domain::TaskHold {
            task_id: task_id.into(),
            verb: HoldVerb::Delegate,
            args: HoldArgs {
                scheduled: Some(true),
                ..HoldArgs::default()
            },
            start_at: Some(until.clone()),
            await_profile: None,
            await_model: None,
            next_check_at: until,
            expires_at: oga_routing::format_rfc3339_ms(now_ms.saturating_add(DEPENDENCY_EXPIRY_MS)),
            probe_count: 0,
            note,
            created_at: now.clone(),
            updated_at: now,
        },
    };
    Ok(DependencyPlan {
        state: TaskState::Pending,
        completion: plan.completion,
        hold: Some(hold),
    })
}

fn validate_request(request: &DispatchRequest) -> Result<(), DispatchError> {
    if request.profile_id.trim().is_empty() {
        return Err(DispatchError::Refusal("profile is required".into()));
    }
    if request.prompt.trim().is_empty() {
        return Err(DispatchError::Refusal("prompt must not be empty".into()));
    }
    if !request.cwd.is_absolute() {
        return Err(DispatchError::Refusal(
            "cwd must be an absolute path".into(),
        ));
    }
    if !request.cwd.is_dir() {
        return Err(DispatchError::Refusal("cwd does not exist".into()));
    }
    if request.timeout.is_some_and(|timeout| timeout.is_zero()) {
        return Err(DispatchError::Refusal(
            "timeout must be greater than zero".into(),
        ));
    }
    Ok(())
}

fn persist_plan(store: &Store, plan: &DispatchPlan) -> Result<(), DispatchError> {
    let task = &plan.task;
    let scope = encode(&task.scope)?;
    let completion = task.completion.as_ref().map(encode).transpose()?;
    let attempts = encode(&task.attempts)?;
    let selection = plan.task_selection.as_ref().map(encode).transpose()?;
    let links = task
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.links.as_ref())
        .filter(|links| !links.is_empty())
        .map(encode)
        .transpose()?;
    store.transaction(|tx| {
        tx.execute(
            "INSERT INTO tasks(id,kind,profile_id,model,prompt,cwd,branch,origin_cwd,worktree_path,worktree_branch,worktree_links_json,state,output,error,question,parent_task_id,orchestrator_id,caller_id,scope_json,grant_id,allow_questions,timeout_ms,effort,tldr,title,session_id,shipped_prompt,completion_json,attempts_json,cost_usd,turns,archived_at,created_at,updated_at,selection_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                task.id,
                kind_string(task.kind.unwrap_or(TaskKind::Delegated)),
                task.profile_id,
                task.model,
                task.prompt,
                task.cwd,
                task.branch,
                task.worktree.as_ref().map(|worktree| worktree.origin_cwd.as_str()),
                task.worktree.as_ref().map(|worktree| worktree.path.as_str()),
                task.worktree.as_ref().map(|worktree| worktree.branch.as_str()),
                links,
                task.state.as_str(),
                task.output,
                task.error,
                task.question,
                task.parent_task_id,
                task.orchestrator_id,
                plan.caller_id.as_deref(),
                scope,
                task.grant_id,
                i64::from(task.allow_questions),
                task.timeout_ms,
                task.effort,
                task.tldr,
                task.title,
                task.session_id,
                task.shipped_prompt,
                completion,
                attempts,
                task.cost_usd,
                task.turns,
                task.archived_at,
                task.created_at,
                task.updated_at,
                selection,
            ],
        )?;
        for blocker in &plan.dependencies {
            tx.execute(
                "INSERT INTO task_dependencies(task_id,blocker_id,created_at) VALUES(?,?,?)",
                params![task.id, blocker, task.created_at],
            )?;
        }
        append_event_tx(tx, &task.id, "created", task.state, json!({}), &task.created_at, None)?;
        if let Some(hold) = &plan.hold {
            let args = serde_json::to_string(&hold.args)
                .map_err(|error| StoreError::Refusal(format!("invalid hold JSON: {error}")))?;
            tx.execute(
                "INSERT INTO task_holds(task_id,verb,args_json,start_at,await_profile,await_model,next_check_at,expires_at,probe_count,note,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    hold.task_id,
                    hold_verb_string(hold.verb),
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
            append_event_tx(
                tx,
                &task.id,
                "hold_armed",
                TaskState::Pending,
                json!({"note": hold.note}),
                &hold.created_at,
                None,
            )?;
        }
        if let Some(completion) = &task.completion {
            append_event_tx(
                tx,
                &task.id,
                "blocked",
                TaskState::Blocked,
                json!({"error": task.error, "completion": completion}),
                &task.updated_at,
                None,
            )?;
        }
        Ok(())
    })?;
    Ok(())
}

fn kind_string(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Delegated => "delegated",
        TaskKind::Orchestrator => "orchestrator",
    }
}

fn hold_verb_string(verb: HoldVerb) -> &'static str {
    match verb {
        HoldVerb::Resume => "resume",
        HoldVerb::Delegate => "delegate",
    }
}

fn encode<T: Serialize>(value: &T) -> Result<String, DispatchError> {
    serde_json::to_string(value)
        .map_err(|error| DispatchError::Refusal(format!("invalid task JSON: {error}")))
}

fn append_event_tx(
    tx: &rusqlite::Transaction<'_>,
    task_id: &str,
    kind: &str,
    state: TaskState,
    payload: serde_json::Value,
    at: &str,
    turn_id: Option<i64>,
) -> Result<i64, rusqlite::Error> {
    tx.execute(
        "INSERT INTO task_events(task_id,event_type,state,payload,created_at,turn_id) VALUES(?,?,?,?,?,?)",
        params![task_id, kind, state.as_str(), payload.to_string(), at, turn_id],
    )?;
    Ok(tx.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use oga_domain::{Profile, Provider};
    use oga_store::Store;
    use tempfile::tempdir;

    use super::*;

    fn profile(command: Vec<String>) -> Profile {
        Profile {
            id: "fake".into(),
            label: "Fake".into(),
            provider: Provider::Claude,
            default_model: "fake-model".into(),
            enabled: true,
            env: BTreeMap::new(),
            capabilities: vec![],
            command: Some(command),
        }
    }

    /// Write the switch the user would flip in Settings.
    fn switch(store: &Store, cwd: &std::path::Path, model: &str, on: bool) {
        store
            .repositories()
            .settings()
            .put(
                &oga_config::canonical_cwd(cwd).display().to_string(),
                oga_config::MODEL_SETTINGS_KEY,
                &serde_json::json!({ "profiles": { "fake": { "modelEnabled": { model: on } } } })
                    .to_string(),
                "2026-01-01T00:00:00Z",
            )
            .expect("model setting");
    }

    fn service() -> (tempfile::TempDir, Dispatcher) {
        let (directory, dispatcher) = service_with_nothing_on();
        switch(&dispatcher.store, directory.path(), "fake-model", true);
        (directory, dispatcher)
    }

    fn service_with_nothing_on() -> (tempfile::TempDir, Dispatcher) {
        let directory = tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        store
            .repositories()
            .profiles()
            .insert(
                &profile(vec![
                    "sh".into(),
                    "-c".into(),
                    "sleep 0.05; printf 'finished\\nOGA_RESULT: completed\\n'".into(),
                ]),
                "2026-01-01T00:00:00Z",
            )
            .expect("profile");
        (directory, Dispatcher::new(store, ProviderRunner::default()))
    }

    #[tokio::test]
    async fn a_model_nobody_turned_on_never_runs() {
        let (directory, dispatcher) = service_with_nothing_on();
        let refusal = dispatcher
            .dispatch(DispatchRequest::new(
                "fake",
                "do the thing",
                directory.path(),
            ))
            .await
            .expect_err("dispatch refused");

        assert!(
            refusal
                .to_string()
                .contains("fake-model is not turned on for fake"),
            "{refusal}"
        );
        assert!(refusal.to_string().contains("Open Settings"), "{refusal}");
        // A refusal, not a task that quietly went somewhere else.
        let tasks: i64 = dispatcher
            .store
            .with_connection(|connection| {
                Ok(connection.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?)
            })
            .expect("task count");
        assert_eq!(tasks, 0);
    }

    #[tokio::test]
    async fn switching_a_model_off_again_stops_the_next_dispatch() {
        let (directory, dispatcher) = service();
        dispatcher
            .dispatch(DispatchRequest::new("fake", "first", directory.path()))
            .await
            .expect("dispatched while on");

        switch(&dispatcher.store, directory.path(), "fake-model", false);
        let refusal = dispatcher
            .dispatch(DispatchRequest::new("fake", "second", directory.path()))
            .await
            .expect_err("dispatch refused");

        assert!(
            refusal
                .to_string()
                .contains("fake-model is not turned on for fake"),
            "{refusal}"
        );
    }

    #[tokio::test]
    async fn dispatch_creates_and_runs_a_task() {
        let (directory, dispatcher) = service();
        let task = dispatcher
            .dispatch_and_wait(
                DispatchRequest::new("fake", "do the thing", directory.path())
                    .title("A task")
                    .tldr("Do the thing"),
            )
            .await
            .expect("task completed");
        assert_eq!(task.state, TaskState::Completed);
        assert_eq!(task.output, "finished");
        assert_eq!(task.title.as_deref(), Some("A task"));
        assert_eq!(task.tldr.as_deref(), Some("Do the thing"));
        assert!(task.shipped_prompt.as_deref().is_some_and(|prompt| {
            prompt.contains("do the thing")
                && !prompt.contains("If the requested work is fully done")
        }));
        assert_eq!(
            task.completion.expect("completion").code,
            CompletionCode::Completed
        );
    }

    #[tokio::test]
    async fn dispatch_releases_a_dependent_after_completion() {
        let (directory, dispatcher) = service();
        let blocker = dispatcher
            .dispatch(DispatchRequest::new(
                "fake",
                "finish first",
                directory.path(),
            ))
            .await
            .expect("blocker dispatched");
        let dependent = dispatcher
            .dispatch(
                DispatchRequest::new("fake", "finish second", directory.path())
                    .depends_on([blocker.task.id.clone()]),
            )
            .await
            .expect("dependent dispatched");
        assert!(!dependent.launched);

        for _ in 0..100 {
            let task = dispatcher.task(&dependent.task.id).expect("dependent task");
            if task.state.settled() {
                assert_eq!(task.state, TaskState::Completed);
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("dependent task did not settle");
    }

    /// Dispatches one task against a worker whose only behaviour is to die
    /// with `message`, and waits for the broker to park it.
    async fn park_a_failing_run(
        message: &str,
    ) -> (tempfile::TempDir, Dispatcher, oga_domain::TaskHold, Task) {
        let (directory, dispatcher) = service_with_nothing_on();
        dispatcher
            .store
            .repositories()
            .profiles()
            .insert(
                &Profile {
                    id: "limited".into(),
                    label: "Limited".into(),
                    provider: Provider::Claude,
                    default_model: "limited-model".into(),
                    enabled: true,
                    env: BTreeMap::new(),
                    capabilities: vec![],
                    command: Some(vec![
                        "sh".into(),
                        "-c".into(),
                        format!("printf '{message}\\n' 1>&2; exit 1"),
                    ]),
                },
                "2026-01-01T00:00:00Z",
            )
            .expect("profile");
        dispatcher
            .store
            .repositories()
            .settings()
            .put(
                &oga_config::canonical_cwd(directory.path())
                    .display()
                    .to_string(),
                oga_config::MODEL_SETTINGS_KEY,
                &serde_json::json!({
                    "profiles": { "limited": { "modelEnabled": { "limited-model": true } } }
                })
                .to_string(),
                "2026-01-01T00:00:00Z",
            )
            .expect("model setting");

        let dispatched = dispatcher
            .dispatch(DispatchRequest::new(
                "limited",
                "do the thing",
                directory.path(),
            ))
            .await
            .expect("dispatched");
        assert!(dispatched.launched);

        for _ in 0..400 {
            let task = dispatcher.task(&dispatched.task.id).expect("task");
            if task.state == TaskState::Pending {
                let hold = crate::holds::get_hold(&dispatcher.store, &task.id)
                    .expect("hold lookup")
                    .expect("hold armed");
                return (directory, dispatcher, hold, task);
            }
            assert_ne!(
                task.state,
                TaskState::Failed,
                "task died instead of waiting"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("task never parked on a hold");
    }

    #[tokio::test]
    async fn a_rate_limited_run_gets_a_hold_instead_of_dying() {
        let (_directory, _dispatcher, hold, task) =
            park_a_failing_run("rate limit reached \u{b7} resets in 30 minutes").await;
        assert_eq!(hold.verb, HoldVerb::Resume);
        assert_eq!(hold.await_profile.as_deref(), Some("limited"));
        assert_eq!(hold.await_model.as_deref(), Some("limited-model"));
        assert!(
            hold.note.starts_with("Waiting for usage to reset"),
            "unexpected wait reason: {}",
            hold.note
        );
        assert_eq!(
            task.completion.expect("completion").code,
            CompletionCode::RateLimit
        );
    }

    #[tokio::test]
    async fn a_scheduled_task_waits_for_its_start_time_and_then_runs() {
        let (directory, dispatcher) = service();
        let dispatched = dispatcher
            .dispatch(
                DispatchRequest::new("fake", "do the thing later", directory.path()).start_at("4h"),
            )
            .await
            .expect("dispatched");
        assert!(!dispatched.launched);
        assert_eq!(dispatched.task.state, TaskState::Pending);
        let hold = crate::holds::get_hold(&dispatcher.store, &dispatched.task.id)
            .expect("hold lookup")
            .expect("hold armed");
        assert_eq!(hold.verb, HoldVerb::Delegate);
        assert_eq!(hold.args.scheduled, Some(true));
        assert!(hold.start_at.is_some());
        assert!(
            hold.note.starts_with("Starts "),
            "unexpected wait reason: {}",
            hold.note
        );
        assert_eq!(
            dispatcher
                .task(&dispatched.task.id)
                .expect("task")
                .hold
                .expect("hold view")
                .kind,
            oga_domain::HoldViewKind::Time
        );

        let clock = crate::holds::FixedClock::new(SystemTime::now());
        let sweep = HoldSweep::with_clock(dispatcher.store.clone(), clock.clone());
        assert!(
            sweep.sweep().expect("early sweep").released.is_empty(),
            "the start time has not come yet"
        );

        clock.advance(Duration::from_secs(4 * 60 * 60 + 1));
        let report = sweep.sweep().expect("release sweep");
        assert_eq!(
            report
                .released
                .iter()
                .map(|hold| hold.task_id.as_str())
                .collect::<Vec<_>>(),
            vec![dispatched.task.id.as_str()]
        );
        assert_eq!(
            dispatcher.task(&dispatched.task.id).expect("task").state,
            TaskState::Queued
        );
    }

    #[tokio::test]
    async fn resuming_a_scheduled_task_starts_it_without_waiting() {
        let (directory, dispatcher) = service();
        let dispatched = dispatcher
            .dispatch(
                DispatchRequest::new("fake", "do the thing later", directory.path()).start_at("2d"),
            )
            .await
            .expect("dispatched");
        assert_eq!(dispatched.task.state, TaskState::Pending);

        let task = dispatcher
            .resume(ResumeRequest::new(dispatched.task.id.clone()))
            .await
            .expect("resumed");
        assert_ne!(task.state, TaskState::Pending);
        assert!(
            crate::holds::get_hold(&dispatcher.store, &dispatched.task.id)
                .expect("hold lookup")
                .is_none(),
            "starting it now drops the wait"
        );
    }

    #[tokio::test]
    async fn a_start_time_that_has_passed_is_refused() {
        let (directory, dispatcher) = service();
        let refusal = dispatcher
            .dispatch(
                DispatchRequest::new("fake", "do the thing later", directory.path())
                    .start_at("2020-01-01T00:00:00.000Z"),
            )
            .await
            .expect_err("dispatch refused");
        assert!(refusal.to_string().contains("already past"), "{refusal}");
    }

    #[tokio::test]
    async fn a_run_the_network_killed_waits_for_the_connection() {
        let (_directory, dispatcher, hold, task) = park_a_failing_run("connection refused").await;
        assert_eq!(hold.verb, HoldVerb::Resume);
        assert_eq!(hold.note, "Waiting for network");
        let network = hold.args.network.expect("network bookkeeping");
        assert_eq!(network.attempt, 0);
        assert_eq!(
            network.original_error.as_deref(),
            Some("connection refused")
        );
        assert_eq!(
            task.completion.expect("completion").code,
            CompletionCode::Network
        );
        let events = dispatcher
            .store
            .repositories()
            .events()
            .list(&task.id)
            .expect("events");
        assert!(
            events
                .iter()
                .any(|event| event.kind == "network_retry_scheduled"),
            "the give-up budget is counted in the trace"
        );
    }
}
