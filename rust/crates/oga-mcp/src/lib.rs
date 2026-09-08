//! Stateless MCP server for the broker's task and discovery surface.

use std::{io, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{Local, SecondsFormat, TimeZone, Utc};
use oga_config::{canonical_cwd, global_cwd};
use oga_domain::{
    ArchivedFilter, ListOrder, ModelInfo, OnBlockerFailure, Provider, StateFilter, Task, TaskKind,
    TaskListQuery, TaskMatch, TaskScope, TaskState, WorkKind,
};
use oga_http::{HttpState, settings::ModelQuery as SettingsModelQuery};
use oga_routing::{ModelNameMatch, ambiguous_message, not_enabled_message, resolve_model_name};
use oga_service::{
    ArchiveRequest, CancelRequest, CompletionAssertion, DispatchRequest, FollowUpQueue,
    HandoffRequest, ReplyRequest, ResumeRequest, SteerRequest, WorktreeRemoveRequest,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

mod hints;
mod protocol;
mod query;
mod shaping;

pub use protocol::{
    EARLIEST_PROTOCOL_VERSION, LEGACY_PROTOCOL_VERSION, MCP_INSTRUCTIONS, MCP_PROTOCOL_VERSION,
};

#[derive(Debug, Error)]
enum McpError {
    #[error("{0}")]
    Message(String),
    /// A refusal that already knows what the caller should do instead. The
    /// hints ride on the error result, where the caller is looking.
    #[error("{message}")]
    Refused { message: String, next: Vec<Value> },
    #[error("method not found: {0}")]
    MethodNotFound(String),
    #[error("invalid params: {0}")]
    InvalidParams(String),
}

impl From<oga_store::StoreError> for McpError {
    fn from(error: oga_store::StoreError) -> Self {
        Self::Message(error.to_string())
    }
}

impl From<oga_service::ContinuationError> for McpError {
    fn from(error: oga_service::ContinuationError) -> Self {
        Self::Message(error.to_string())
    }
}

impl From<oga_service::DispatchError> for McpError {
    fn from(error: oga_service::DispatchError) -> Self {
        Self::Message(error.to_string())
    }
}

#[derive(Clone)]
pub struct McpServer {
    state: HttpState,
    orchestrator_id: Option<String>,
}

impl McpServer {
    pub fn new(state: HttpState) -> Self {
        Self {
            state,
            orchestrator_id: None,
        }
    }

    pub fn with_orchestrator(mut self, orchestrator_id: impl Into<String>) -> Self {
        self.orchestrator_id = Some(orchestrator_id.into());
        self
    }

    pub fn state(&self) -> &HttpState {
        &self.state
    }

    /// Whether the task on the other end may hand work onward. A caller that
    /// names no task is a person's own session and always may.
    fn caller_may_delegate(&self) -> bool {
        let Some(caller_id) = self.orchestrator_id.as_deref() else {
            return true;
        };
        self.state.dispatcher.task(caller_id).is_ok_and(|caller| {
            caller.archived_at.is_none()
                && (caller.kind == Some(TaskKind::Orchestrator) || caller.can_delegate)
        })
    }

    pub async fn handle_value(&self, value: Value) -> Option<Value> {
        let request = match serde_json::from_value::<protocol::JsonRpcRequest>(value) {
            Ok(request) => request,
            Err(error) => {
                return Some(
                    serde_json::to_value(protocol::JsonRpcResponse::error(
                        Value::Null,
                        -32600,
                        format!("invalid request: {error}"),
                    ))
                    .expect("JSON-RPC response is serializable"),
                );
            }
        };
        if request
            .jsonrpc
            .as_deref()
            .is_some_and(|version| version != "2.0")
        {
            return Some(
                serde_json::to_value(protocol::JsonRpcResponse::error(
                    request.id.unwrap_or(Value::Null),
                    -32600,
                    "jsonrpc must be 2.0",
                ))
                .expect("JSON-RPC response is serializable"),
            );
        }
        let Some(id) = request.id.clone() else {
            self.execute_notification(&request).await;
            return None;
        };
        let response = match self.execute(&request).await {
            Ok(result) => protocol::JsonRpcResponse::result(id, result),
            Err(McpError::Refused { message, next }) if request.method == "tools/call" => {
                let body = json!({ "error": message, "next": next });
                protocol::JsonRpcResponse::result(
                    id,
                    json!({
                        "content": [protocol::text_content(
                            serde_json::to_string_pretty(&body).expect("refusal is serializable"),
                        )],
                        "isError": true,
                    }),
                )
            }
            Err(error) if request.method == "tools/call" => protocol::JsonRpcResponse::result(
                id,
                json!({
                    "content": [protocol::text_content(error.to_string())],
                    "isError": true,
                }),
            ),
            Err(error) => {
                protocol::JsonRpcResponse::error(id, error_code(&error), error.to_string())
            }
        };
        Some(serde_json::to_value(response).expect("JSON-RPC response is serializable"))
    }

    async fn execute_notification(&self, request: &protocol::JsonRpcRequest) {
        if request.method == "tools/call" {
            let _ = self.execute(request).await;
        }
    }

    async fn execute(&self, request: &protocol::JsonRpcRequest) -> Result<Value, McpError> {
        match request.method.as_str() {
            "initialize" => {
                let requested = request
                    .params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(MCP_PROTOCOL_VERSION);
                let version = match requested {
                    MCP_PROTOCOL_VERSION | LEGACY_PROTOCOL_VERSION | EARLIEST_PROTOCOL_VERSION => {
                        requested
                    }
                    _ => MCP_PROTOCOL_VERSION,
                };
                Ok(protocol::initialize_result(version, oga_domain::VERSION))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(protocol::tool_list(self.caller_may_delegate())),
            "tools/call" => self.tool_call(&request.params).await,
            "notifications/initialized" | "notifications/cancelled" => Ok(json!({})),
            method => Err(McpError::MethodNotFound(method.into())),
        }
    }

    async fn tool_call(&self, params: &Value) -> Result<Value, McpError> {
        let name = required_string(params, "name")?;
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !args.is_object() {
            return Err(McpError::InvalidParams(
                "arguments must be an object".into(),
            ));
        }
        let (value, cwd) = match name.as_str() {
            "delegate" => self.delegate(&args).await?,
            "models" => self.models(&args).await?,
            "inspect" => self.inspect(&args).await?,
            "health" => (self.health(), None),
            "tasks" => (self.tasks(&args)?, None),
            "memory" => self.memory(&args)?,
            "query" => self.query(&args)?,
            "reply" => self.reply(&args).await?,
            "resume" => self.resume(&args).await?,
            "steer" => self.steer(&args).await?,
            "handoff" => self.handoff(&args).await?,
            "cancel" => self.cancel(&args).await?,
            "complete" => self.complete(&args).await?,
            "archive" => self.archive(&args).await?,
            "worktree-remove" => self.worktree_remove(&args).await?,
            _ => return Err(McpError::Message(format!("unknown tool: {name}"))),
        };
        self.mcp_response(value, cwd.as_deref())
    }

    fn mcp_response(&self, value: Value, cwd: Option<&str>) -> Result<Value, McpError> {
        let content = vec![protocol::text_content(
            serde_json::to_string_pretty(&value).expect("tool value is serializable"),
        )];
        let _ = cwd;
        Ok(json!({ "content": content }))
    }

    async fn delegate(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        if !self.caller_may_delegate() {
            return Err(McpError::Message(
                "this task cannot hand work onward; finish the brief here".into(),
            ));
        }
        let prompt = required_string(args, "prompt")?;
        let cwd = required_string(args, "cwd")?;
        let tldr = required_string(args, "tldr")?;
        let title = required_string(args, "title")?;
        validate_length(&prompt, 64_000, "prompt")?;
        validate_length(&tldr, 200, "tldr")?;
        validate_length(&title, 60, "title")?;
        validate_delegate_options(args)?;
        // Validated above, so parsing cannot fail here.
        let kind = optional_string(args, "kind")
            .map(|value| WorkKind::parse(&value).expect("kind validated above"));
        let profile_arg = optional_string(args, "profile");
        let model_arg = optional_string(args, "model");
        let effort_arg = optional_string(args, "effort");
        // Automatic routing — the caller named neither account — is the one
        // path a stated kind can actually reach: it is what reads the prompt,
        // applies a loved rule, and resolves an effort. Naming a profile or a
        // model is the caller's own call, same as ever.
        let (profile_id, model, route_effort) = if profile_arg.is_none() && model_arg.is_none() {
            let route = oga_http::routing::plan(
                &self.state,
                oga_http::routing::RouteInput {
                    prompt: prompt.clone(),
                    cwd: cwd.clone(),
                    profile: None,
                    model: None,
                    kind,
                    effort: effort_arg.clone(),
                    default_profile_shortcut: false,
                },
            )
            .map_err(|error| McpError::Message(error.message))?;
            (route.profile_id, route.model, route.effort)
        } else {
            let (profile_id, model) = self.route(&cwd, profile_arg, model_arg)?;
            (profile_id, model, None)
        };
        let mut request = DispatchRequest::new(profile_id, prompt, PathBuf::from(&cwd));
        request.model = Some(model);
        request.scope = scope(args.get("scope"))?;
        request.allow_questions = optional_bool(args, "allowQuestions").unwrap_or(true);
        request.can_delegate = optional_bool(args, "canDelegate").unwrap_or(false);
        request.timeout = optional_u64(args, "timeoutMs")?.map(Duration::from_millis);
        request.parent_task_id = optional_string(args, "parent");
        request.effort = effort_arg.or(route_effort);
        request.tldr = Some(tldr);
        request.title = Some(title);
        request.worktree = args
            .get("worktree")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| McpError::InvalidParams(format!("worktree: {error}")))?;
        request.depends_on = string_array(args.get("dependsOn"), "dependsOn")?;
        request.start_at = optional_string(args, "startAt");
        request.attachments = string_array(args.get("attachments"), "attachments")?;
        request.on_blocker_failure = match optional_string(args, "onBlockerFailure").as_deref() {
            Some("run") => OnBlockerFailure::Run,
            Some("hold") | None => OnBlockerFailure::Hold,
            Some(value) => {
                return Err(McpError::InvalidParams(format!(
                    "onBlockerFailure must be hold or run, got {value}"
                )));
            }
        };
        if let Some(caller_id) = &self.orchestrator_id
            && self
                .state
                .dispatcher
                .task(caller_id)
                .is_ok_and(|caller| caller.kind == Some(TaskKind::Orchestrator))
        {
            request.orchestrator_id = Some(caller_id.clone());
        }
        let task = self.state.dispatcher.dispatch(request).await?.task;
        let task = self.enrich_task(task)?;
        let fields = fields(args.get("fields"))?.unwrap_or_else(|| vec!["routing".into()]);
        let mut view = shaping::task_view(&task, &fields);
        view.as_object_mut()
            .expect("task view is an object")
            .insert("cursor".into(), json!(self.task_cursor(&task.id)?));
        Ok((
            shaping::with_next(view, &task, hints::Move::Started),
            Some(cwd),
        ))
    }

    async fn inspect(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let task_id = required_string(args, "taskId")?;
        let task = self.enrich_task(
            self.state
                .dispatcher
                .task(&task_id)
                .map_err(McpError::from)?,
        )?;
        let fields = fields(args.get("fields"))?.unwrap_or_else(shaping::default_inspect_fields);
        let cwd = project_cwd(&task);
        let branch_gone = task_branch_unavailable(&task).await;
        let action = if task.archived_at.is_some() {
            hints::Move::Archived { branch_gone }
        } else {
            hints::Move::Settled { branch_gone }
        };
        Ok((
            shaping::with_next(shaping::task_view(&task, &fields), &task, action),
            Some(cwd),
        ))
    }

    async fn models(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let only_preferred = args
            .get("onlyPreferred")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let only_enabled = args
            .get("onlyEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let profile_filter = optional_string(args, "profile");
        let provider_filter = optional_string(args, "provider")
            .map(|provider| parse_provider(&provider))
            .transpose()?;
        let query = optional_string(args, "query").map(|value| value.to_lowercase());
        let requested_cwd = optional_string(args, "cwd");
        let cwd = requested_cwd
            .as_deref()
            .map(|cwd| canonical_cwd(cwd).display().to_string())
            .unwrap_or_else(|| global_cwd().display().to_string());
        let include_usage = optional_bool(args, "usage").unwrap_or(true);
        let rows = oga_http::settings::model_rows(
            &self.state.store,
            &SettingsModelQuery {
                profile: profile_filter,
                provider: provider_filter.map(|provider| provider.as_str().to_owned()),
                refresh: optional_bool(args, "refresh"),
                cwd: Some(cwd.clone()),
                include_disabled: Some(!only_enabled),
                only_preferred: Some(only_preferred),
                only_enabled: Some(only_enabled),
                query,
            },
            include_usage,
        )
        .await
        .map_err(|error| McpError::Message(error.message))?;
        let love = oga_http::settings::love_rules(&cwd)
            .map_err(|error| McpError::Message(error.message))?;
        Ok((
            json!({
                "love": love,
                "models": rows,
            }),
            requested_cwd,
        ))
    }

    fn health(&self) -> Value {
        json!(oga_domain::HealthReport::ok(
            self.state.build.clone(),
            &self.state.staleness
        ))
    }

    fn tasks(&self, args: &Value) -> Result<Value, McpError> {
        let query = task_query(args)?;
        let rows = self.list_tasks(&query).map_err(McpError::Message)?;
        let fields = fields(args.get("fields"))?;
        Ok(Value::Array(
            rows.into_iter()
                .take(query.limit.unwrap_or(20).clamp(1, 100) as usize)
                .map(|row| {
                    let summary = shaping::task_summary(&row.task);
                    shaping::summary_view(&summary, fields.as_deref(), row.matched)
                })
                .collect(),
        ))
    }

    fn memory(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let action = required_string(args, "action")?;
        let cwd = required_string(args, "cwd")?;
        let key = optional_string(args, "key");
        let service = self.state.store.repositories().memories();
        match action.as_str() {
            "list" => Ok((json!(service.list(&cwd)?), Some(cwd))),
            "get" => {
                let key = key.ok_or_else(|| McpError::Message("memory get requires key".into()))?;
                Ok((
                    json!(
                        service
                            .list(&cwd)?
                            .into_iter()
                            .find(|entry| entry.key == key)
                    ),
                    Some(cwd),
                ))
            }
            "set" => {
                let key = key.ok_or_else(|| McpError::Message("memory set requires key".into()))?;
                let value = args
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| McpError::Message("memory set requires value".into()))?;
                if value.trim().is_empty() {
                    return Err(McpError::Message("memory value must not be empty".into()));
                }
                let current = service
                    .list(&cwd)?
                    .into_iter()
                    .find(|entry| entry.key == key);
                let entry = oga_domain::MemoryEntry {
                    cwd: cwd.clone(),
                    key,
                    value: value.trim().into(),
                    version: current.as_ref().map_or(1, |entry| entry.version),
                    created_at: current.as_ref().map_or_else(
                        || Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                        |entry| entry.created_at.clone(),
                    ),
                    updated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                };
                let expected = optional_u64(args, "expectedVersion")?;
                Ok((json!(service.upsert(&entry, expected)?), Some(cwd)))
            }
            "remove" => {
                let key =
                    key.ok_or_else(|| McpError::Message("memory remove requires key".into()))?;
                let expected = optional_u64(args, "expectedVersion")?;
                let removed = self.state.store.transaction(|tx| {
                    if let Some(expected) = expected {
                        let current: Option<u64> = tx
                            .query_row(
                                "SELECT version FROM memories WHERE cwd=? AND key=?",
                                params![cwd, key],
                                |row| row.get(0),
                            )
                            .optional()?;
                        if current != Some(expected) {
                            return Err(oga_store::StoreError::Refusal(
                                "memory revision conflict".into(),
                            ));
                        }
                    }
                    Ok(tx.execute(
                        "DELETE FROM memories WHERE cwd=? AND key=?",
                        params![cwd, key],
                    )? != 0)
                })?;
                Ok((json!({ "removed": removed }), Some(cwd)))
            }
            _ => Err(McpError::InvalidParams(format!(
                "unknown memory action: {action}"
            ))),
        }
    }

    fn query(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let requested_cwd = required_string(args, "cwd")?;
        let question = required_string(args, "q")?;
        let cwd = if let Some(orchestrator_id) = &self.orchestrator_id {
            let task = self
                .state
                .dispatcher
                .task(orchestrator_id)
                .map_err(McpError::from)?;
            if task.archived_at.is_some() {
                return Err(McpError::Message(format!(
                    "unknown task: {orchestrator_id}"
                )));
            }
            task.worktree
                .as_ref()
                .map_or(task.cwd.clone(), |worktree| worktree.origin_cwd.clone())
        } else {
            requested_cwd
        };
        Ok((
            json!(query::query(&self.state, &cwd, &question).map_err(McpError::Message)?),
            Some(cwd),
        ))
    }

    async fn reply(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let task_id = required_string(args, "taskId")?;
        let answer = required_string(args, "answer")?;
        let mut request = ReplyRequest::new(task_id, answer);
        if let Some(scope) = args.get("scope") {
            request.scope = Some(scope_value(scope)?);
        }
        let task = self.state.dispatcher.reply(request).await?;
        self.started_response(
            task,
            fields(args.get("fields"))?.unwrap_or_default(),
            hints::Move::Started,
        )
    }

    async fn resume(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let task_id = required_string(args, "taskId")?;
        let current = self
            .state
            .dispatcher
            .task(&task_id)
            .map_err(McpError::from)?;
        if let Some(queue) = optional_string(args, "queue") {
            let follow_ups = FollowUpQueue::new(self.state.store.clone());
            let cleared = match queue.as_str() {
                "add" => {
                    let instruction = required_string(args, "instruction")?;
                    if args.get("timeoutMs").is_some()
                        || args.get("scope").is_some()
                        || args.get("allowQuestions").is_some()
                        || args.get("model").is_some()
                        || args.get("effort").is_some()
                        || args.get("startAt").is_some()
                    {
                        return Err(McpError::Message(
                            "a queued follow-up takes only an instruction".into(),
                        ));
                    }
                    follow_ups.queue(&task_id, current.state, &instruction)?;
                    false
                }
                "clear" => {
                    follow_ups.clear(&task_id, current.state, "removed on request")?;
                    true
                }
                _ => return Err(McpError::InvalidParams("queue must be add or clear".into())),
            };
            let task = self.enrich_task(
                self.state
                    .dispatcher
                    .task(&task_id)
                    .map_err(McpError::from)?,
            )?;
            let action = if cleared {
                hints::Move::Settled {
                    branch_gone: task_branch_unavailable(&task).await,
                }
            } else {
                hints::Move::Queued
            };
            return self.started_response(
                task,
                fields(args.get("fields"))?.unwrap_or_default(),
                action,
            );
        }
        if let Some((message, next)) = hints::resume_refusal(&current) {
            return Err(McpError::Refused { message, next });
        }
        let mut request = ResumeRequest::new(task_id);
        if let Some(value) = optional_string(args, "startAt") {
            request = request.start_at(value);
        }
        if let Some(value) = optional_string(args, "instruction") {
            request = request.instruction(value);
        }
        if let Some(value) = optional_u64(args, "timeoutMs")? {
            request = request.timeout_ms(value);
        }
        if let Some(value) = args.get("scope") {
            request = request.scope(scope_value(value)?);
        }
        if let Some(value) = optional_bool(args, "allowQuestions") {
            request = request.allow_questions(value);
        }
        if let Some(value) = optional_string(args, "model") {
            request = request.model(value);
        }
        if let Some(value) = optional_string(args, "effort") {
            request = request.effort(value);
        }
        let task = self.state.dispatcher.resume(request).await?;
        self.started_response(
            task,
            fields(args.get("fields"))?.unwrap_or_default(),
            hints::Move::Started,
        )
    }

    async fn steer(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let task_id = required_string(args, "taskId")?;
        let mut request = SteerRequest::new(task_id);
        if let Some(value) = optional_string(args, "instruction") {
            request = request.instruction(value);
        }
        if let Some(value) = optional_string(args, "model") {
            request = request.model(value);
        }
        let outcome = self.state.dispatcher.steer(request).await?;
        let task = self.enrich_task(outcome.task)?;
        let cwd = project_cwd(&task);
        let fields = fields(args.get("fields"))?.unwrap_or_default();
        let mut value = shaping::task_view(&task, &fields);
        if outcome.queued {
            value
                .as_object_mut()
                .expect("task view is an object")
                .insert(
                    "note".into(),
                    json!("Queued as a follow-up. It will be applied after the current run."),
                );
        }
        let action = if outcome.queued {
            hints::Move::Queued
        } else {
            hints::Move::Started
        };
        Ok((shaping::with_next(value, &task, action), Some(cwd)))
    }

    async fn handoff(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let task_id = required_string(args, "taskId")?;
        let mut request = HandoffRequest::new(task_id);
        if let Some(value) = optional_string(args, "profile") {
            request = request.profile(value);
        }
        if let Some(value) = optional_string(args, "model") {
            request = request.model(value);
        }
        if let Some(value) = optional_string(args, "effort") {
            request = request.effort(value);
        }
        if let Some(value) = args.get("scope") {
            request = request.scope(scope_value(value)?);
        }
        let task = self.state.dispatcher.handoff(request).await?;
        self.started_response(
            task,
            fields(args.get("fields"))?.unwrap_or_else(|| vec!["routing".into()]),
            hints::Move::Started,
        )
    }

    async fn cancel(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let ids = task_ids(args.get("taskId"))?;
        let reason = optional_string(args, "reason");
        let fields = fields(args.get("fields"))?.unwrap_or_default();
        let outcomes = ids
            .iter()
            .map(|id| {
                let result = self.state.dispatcher.cancel(if let Some(reason) = &reason {
                    CancelRequest::new(id).reason(reason.clone())
                } else {
                    CancelRequest::new(id)
                });
                async move { (id, result.await) }
            })
            .collect::<Vec<_>>();
        let mut resolved = Vec::with_capacity(outcomes.len());
        for future in outcomes {
            let (id, result) = future.await;
            resolved.push(match result {
                Ok(task) => {
                    let task = self.enrich_task(task)?;
                    let branch_gone = task_branch_unavailable(&task).await;
                    Ok(shaping::TaskActionSuccess {
                        task,
                        stopped: None,
                        checkout: None,
                        branch_outcome: None,
                        branch_reason: None,
                        branch_gone,
                    })
                }
                Err(error) => Err((id.clone(), error.to_string())),
            });
        }
        let cwd = if ids.len() == 1 {
            self.state
                .dispatcher
                .task(&ids[0])
                .ok()
                .map(|task| project_cwd(&task))
        } else {
            None
        };
        Ok((
            shaping::task_action_response(
                &ids,
                resolved,
                &fields,
                hints::Move::Settled { branch_gone: false },
            ),
            cwd,
        ))
    }

    async fn complete(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let task_id = required_string(args, "taskId")?;
        let asserted_by = required_string(args, "assertedBy")?;
        let reason = required_string(args, "reason")?;
        let task = self
            .state
            .dispatcher
            .force_complete(CompletionAssertion::new(task_id, asserted_by, reason))?;
        let task = self.enrich_task(task)?;
        let cwd = project_cwd(&task);
        let branch_gone = task_branch_unavailable(&task).await;
        Ok((
            shaping::with_next(
                shaping::task_view(&task, &fields(args.get("fields"))?.unwrap_or_default()),
                &task,
                hints::Move::Settled { branch_gone },
            ),
            Some(cwd),
        ))
    }

    async fn archive(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let ids = task_ids(args.get("taskId"))?;
        let archived = optional_bool(args, "archived").unwrap_or(true);
        let delete_branch = optional_bool(args, "deleteBranch").unwrap_or(false);
        if delete_branch && !archived {
            return Err(McpError::InvalidParams(
                "deleteBranch only applies when archiving".into(),
            ));
        }
        let fields = fields(args.get("fields"))?.unwrap_or_default();
        let mut outcomes = Vec::with_capacity(ids.len());
        for id in &ids {
            let mut request = ArchiveRequest::new(id, archived);
            if delete_branch {
                request = request.delete_branch();
            }
            outcomes.push(match self.state.dispatcher.archive(request).await {
                Ok(result) => {
                    let task = self.enrich_task(result.task)?;
                    let branch_gone = task_branch_unavailable(&task).await;
                    Ok(shaping::TaskActionSuccess {
                        task,
                        stopped: result.stopped.then_some("stopped".into()),
                        checkout: result.checkout,
                        branch_outcome: result.branch,
                        branch_reason: result.branch_reason,
                        branch_gone,
                    })
                }
                Err(error) => Err((id.clone(), error.to_string())),
            });
        }
        let cwd = if ids.len() == 1 {
            self.state
                .dispatcher
                .task(&ids[0])
                .ok()
                .map(|task| project_cwd(&task))
        } else {
            None
        };
        let action = if archived {
            hints::Move::Archived { branch_gone: false }
        } else {
            hints::Move::Settled { branch_gone: false }
        };
        Ok((
            shaping::task_action_response(&ids, outcomes, &fields, action),
            cwd,
        ))
    }

    async fn worktree_remove(&self, args: &Value) -> Result<(Value, Option<String>), McpError> {
        let task_id = optional_string(args, "taskId");
        let project = optional_string(args, "project");
        if task_id.is_some() == project.is_some() {
            return Err(McpError::Message(
                "provide exactly one of taskId or project".into(),
            ));
        }
        let delete_branch = optional_bool(args, "deleteBranch").unwrap_or(false);
        let cwd = task_id
            .as_deref()
            .and_then(|id| {
                self.state
                    .dispatcher
                    .task(id)
                    .ok()
                    .map(|task| project_cwd(&task))
            })
            .or_else(|| project.clone());
        let value = if let Some(task_id) = task_id {
            let removed = serde_json::to_value(
                self.state
                    .dispatcher
                    .remove_worktree(WorktreeRemoveRequest {
                        task_id: task_id.clone(),
                        delete_branch,
                    })
                    .await?,
            )
            .expect("worktree result is serializable");
            // A skipped removal left the checkout where it was, so the moves
            // that follow one being gone do not apply to it.
            let skipped = removed.get("skipped").is_some();
            match self.state.dispatcher.task(&task_id) {
                Ok(task) if !skipped => {
                    let branch_gone = task_branch_unavailable(&task).await;
                    let branch_kept = removed.get("branch").and_then(Value::as_str) == Some("kept")
                        && !branch_gone;
                    shaping::with_next(removed, &task, hints::Move::CheckoutRemoved { branch_kept })
                }
                _ => removed,
            }
        } else {
            serde_json::to_value(
                self.state
                    .dispatcher
                    .remove_project_worktrees(
                        project.clone().expect("project exists"),
                        delete_branch,
                    )
                    .await?,
            )
            .expect("worktree result is serializable")
        };
        Ok((value, cwd))
    }

    fn route(
        &self,
        cwd: &str,
        requested_profile: Option<String>,
        requested_model: Option<String>,
    ) -> Result<(String, String), McpError> {
        let profiles = self.state.store.repositories().profiles().list()?;
        let settings = oga_service::authorization::resolved_model_settings(&self.state.store, cwd)
            .map_err(|error| McpError::Message(error.to_string()))?;
        // The same catalog Settings itself reads from, so a name resolves the
        // same way here as it shows there. A profile the settings screen has
        // never loaded falls back to a short alias list, same as before.
        let catalog_for = |profile: &oga_domain::Profile| {
            oga_http::settings::cached_catalog(std::slice::from_ref(profile))
        };
        // One resolution rule everywhere: the exact id Settings shows, or its
        // short trailing name when that names exactly one model this account
        // offers. Nothing here trusts the caller's string past this point —
        // whatever it resolves to is what gets checked and dispatched.
        let resolve = |profile: &oga_domain::Profile, hint: &str| -> Result<String, McpError> {
            let catalog = catalog_for(profile);
            let refs: Vec<&ModelInfo> = catalog.iter().collect();
            match resolve_model_name(&refs, hint) {
                ModelNameMatch::Ambiguous(candidates) => Err(McpError::Message(ambiguous_message(
                    &profile.id,
                    hint,
                    &candidates
                        .iter()
                        .map(|candidate| candidate.id.clone())
                        .collect::<Vec<_>>(),
                ))),
                ModelNameMatch::Resolved(resolved) => {
                    if oga_config::model_enabled(&settings, &profile.id, &resolved.id) {
                        Ok(resolved.id.clone())
                    } else {
                        let on: Vec<String> = catalog
                            .iter()
                            .filter(|candidate| {
                                oga_config::model_enabled(&settings, &profile.id, &candidate.id)
                            })
                            .map(|candidate| candidate.id.clone())
                            .collect();
                        Err(McpError::Message(not_enabled_message(
                            &profile.id,
                            hint,
                            &resolved.id,
                            &on,
                        )))
                    }
                }
                // Discovery can fall back to a short list that says nothing
                // about what else the account offers, so an unknown name is
                // not evidence it does not exist. Pass it through unresolved;
                // dispatch still refuses it if it is genuinely not turned on.
                ModelNameMatch::Unknown => Ok(hint.to_owned()),
            }
        };
        let profile = if let Some(profile_id) = requested_profile {
            profiles
                .into_iter()
                .find(|profile| profile.id == profile_id)
                .ok_or_else(|| McpError::Message(format!("unknown profile: {profile_id}")))?
        } else if let Some(model) = requested_model.as_deref() {
            let offering: Vec<oga_domain::Profile> = profiles
                .into_iter()
                .filter(|profile| {
                    profile.enabled
                        && matches!(
                            resolve_model_name(
                                &catalog_for(profile).iter().collect::<Vec<_>>(),
                                model
                            ),
                            ModelNameMatch::Resolved(_)
                        )
                })
                .collect();
            // Among the accounts offering it, the one where it is switched on:
            // otherwise a model the user did enable is refused because a second
            // account happened to list it first.
            offering
                .iter()
                .find(|profile| resolve(profile, model).is_ok())
                .or_else(|| offering.first())
                .cloned()
                .ok_or_else(|| {
                    McpError::Message(format!("no enabled profile provides model: {model}"))
                })?
        } else {
            profiles
                .into_iter()
                .find(|profile| profile.id == "default" && profile.enabled)
                .or_else(|| {
                    self.state
                        .store
                        .repositories()
                        .profiles()
                        .list()
                        .ok()?
                        .into_iter()
                        .find(|profile| profile.enabled)
                })
                .ok_or_else(|| McpError::Message("no enabled profile is configured".into()))?
        };
        if !profile.enabled {
            return Err(McpError::Message(format!(
                "profile disabled: {}",
                profile.id
            )));
        }
        let model = match requested_model {
            Some(hint) => resolve(&profile, &hint)?,
            None => profile.default_model.clone(),
        };
        if model.trim().is_empty() || model.chars().count() > 200 {
            return Err(McpError::InvalidParams(
                "model must be between 1 and 200 characters".into(),
            ));
        }
        Ok((profile.id, model))
    }

    fn enrich_task(&self, mut task: Task) -> Result<Task, McpError> {
        let count = FollowUpQueue::new(self.state.store.clone()).count(&task.id)?;
        task.queued_follow_ups = (count > 0).then_some(count as u64);
        Ok(task)
    }

    fn task_cursor(&self, task_id: &str) -> Result<i64, McpError> {
        self.state
            .store
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT COALESCE(MAX(id),0) FROM task_events WHERE task_id=?",
                    [task_id],
                    |row| row.get(0),
                )?)
            })
            .map_err(McpError::from)
    }

    fn started_response(
        &self,
        task: Task,
        fields: Vec<String>,
        action: hints::Move,
    ) -> Result<(Value, Option<String>), McpError> {
        let task = self.enrich_task(task)?;
        let cwd = project_cwd(&task);
        let mut value = shaping::task_view(&task, &fields);
        value
            .as_object_mut()
            .expect("task view is an object")
            .insert("cursor".into(), json!(self.task_cursor(&task.id)?));
        Ok((shaping::with_next(value, &task, action), Some(cwd)))
    }

    fn list_tasks(&self, query: &TaskListQuery) -> Result<Vec<ListedTask>, String> {
        if query.query.is_some() {
            let matches = self
                .state
                .store
                .repositories()
                .tasks()
                .search(query)
                .map_err(|error| error.to_string())?;
            return matches
                .into_iter()
                .map(|matched| {
                    let task = self
                        .state
                        .dispatcher
                        .task(&matched.id)
                        .map_err(|error| error.to_string())?;
                    Ok(ListedTask {
                        task: self.enrich_task(task).map_err(|error| error.to_string())?,
                        matched: Some(matched.matched),
                    })
                })
                .collect();
        }
        let ids = self
            .state
            .store
            .with_connection(|connection| {
                let order = if query.order == Some(ListOrder::Oldest) {
                    "ASC"
                } else {
                    "DESC"
                };
                let sql = format!("SELECT id FROM tasks ORDER BY updated_at {order},id {order}");
                let mut statement = connection.prepare(&sql)?;
                Ok(statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .map_err(|error| error.to_string())?;
        let mut tasks = Vec::new();
        for id in ids {
            let task = self
                .state
                .dispatcher
                .task(&id)
                .map_err(|error| error.to_string())?;
            if task.kind == Some(TaskKind::Orchestrator) {
                continue;
            }
            let task = self.enrich_task(task).map_err(|error| error.to_string())?;
            if !archive_matches(task.archived_at.is_some(), query.archived)
                || !state_matches(task.state, query.state.as_ref())
                || query
                    .profile
                    .as_deref()
                    .is_some_and(|profile| profile != task.profile_id)
                || query.parent.as_deref().is_some_and(|parent| {
                    task.id != parent && task.parent_task_id.as_deref() != Some(parent)
                })
                || query
                    .since
                    .as_deref()
                    .is_some_and(|since| task.updated_at.as_str() < since)
                || query
                    .until
                    .as_deref()
                    .is_some_and(|until| task.updated_at.as_str() >= until)
            {
                continue;
            }
            tasks.push(ListedTask {
                task,
                matched: None,
            });
        }
        Ok(tasks)
    }
}

struct ListedTask {
    task: Task,
    matched: Option<TaskMatch>,
}

#[derive(Clone)]
struct McpRuntime {
    state: HttpState,
}

pub fn router(state: HttpState) -> Router {
    let runtime = Arc::new(McpRuntime { state });
    Router::new()
        .route("/mcp", post(post_mcp).get(get_mcp))
        .with_state(runtime)
}

pub fn build_router(state: HttpState) -> Router {
    router(state)
}

pub fn mcp_router(state: HttpState) -> Router {
    router(state)
}

async fn get_mcp() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        "MCP GET requires a subscription request",
    )
        .into_response()
}

async fn post_mcp(
    State(runtime): State<Arc<McpRuntime>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !accepts_mcp_stream(&headers) {
        return not_acceptable_response();
    }
    let value = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => value,
        Err(error) => {
            return rpc_response(protocol::JsonRpcResponse::error(
                Value::Null,
                -32700,
                format!("parse error: {error}"),
            ));
        }
    };
    let mut server = McpServer::new(runtime.state.clone());
    if let Some(orchestrator) = headers
        .get("x-oga-task-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    {
        server = server.with_orchestrator(orchestrator);
    }
    let response = if let Some(batch) = value.as_array() {
        let mut responses = Vec::new();
        for request in batch {
            if let Some(response) = server.handle_value(request.clone()).await {
                responses.push(response);
            }
        }
        if responses.is_empty() {
            return StatusCode::ACCEPTED.into_response();
        }
        Value::Array(responses)
    } else {
        let Some(response) = server.handle_value(value).await else {
            return StatusCode::ACCEPTED.into_response();
        };
        response
    };
    event_stream_response(response)
}

pub async fn serve_stdio(server: McpServer) -> io::Result<()> {
    let server = server;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                let response = serde_json::to_string(&protocol::JsonRpcResponse::error(
                    Value::Null,
                    -32700,
                    format!("parse error: {error}"),
                ))
                .expect("JSON-RPC response is serializable");
                stdout.write_all(response.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
                continue;
            }
        };
        if let Some(response) = server.handle_value(value).await {
            stdout
                .write_all(
                    serde_json::to_string(&response)
                        .expect("response is serializable")
                        .as_bytes(),
                )
                .await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

fn rpc_response(response: protocol::JsonRpcResponse) -> Response {
    json_value_response(serde_json::to_value(response).expect("response is serializable"))
}

fn json_value_response(value: Value) -> Response {
    let body = serde_json::to_vec(&value).expect("JSON value is serializable");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json;charset=utf-8")
        .body(axum::body::Body::from(body))
        .expect("valid JSON response")
}

fn not_acceptable_response() -> Response {
    let body = json!({
        "jsonrpc": "2.0",
        "error": {
            "code": -32000,
            "message": "Not Acceptable: Client must accept both application/json and text/event-stream",
        },
        "id": Value::Null,
    });
    Response::builder()
        .status(StatusCode::NOT_ACCEPTABLE)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("valid JSON response")
}

fn accepts_mcp_stream(headers: &HeaderMap) -> bool {
    let Ok(accept) = headers
        .get(header::ACCEPT)
        .map(|value| value.as_bytes())
        .ok_or(())
    else {
        return false;
    };
    let accept = String::from_utf8_lossy(accept);
    let accepts_json = accept
        .split(',')
        .any(|value| value.trim().split(';').next() == Some("application/json"));
    let accepts_sse = accept
        .split(',')
        .any(|value| value.trim().split(';').next() == Some("text/event-stream"));
    accepts_json && accepts_sse
}

fn event_stream_response(value: Value) -> Response {
    let body = format!(
        "event: message\ndata: {}\n\n",
        serde_json::to_string(&value).unwrap()
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(axum::body::Body::from(body))
        .expect("valid event stream response")
}

fn error_code(error: &McpError) -> i64 {
    match error {
        McpError::MethodNotFound(_) => -32601,
        McpError::InvalidParams(_) => -32602,
        McpError::Message(_) | McpError::Refused { .. } => -32000,
    }
}

fn required_string(value: &Value, key: &str) -> Result<String, McpError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpError::InvalidParams(format!("{key} is required")))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn optional_bool(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

fn optional_u64(value: &Value, key: &str) -> Result<Option<u64>, McpError> {
    let Some(value) = value.get(key) else {
        return Ok(None);
    };
    value
        .as_u64()
        .map(Some)
        .ok_or_else(|| McpError::InvalidParams(format!("{key} must be an integer")))
}

fn validate_length(value: &str, max: usize, name: &str) -> Result<(), McpError> {
    if value.chars().count() > max {
        return Err(McpError::InvalidParams(format!(
            "{name} exceeds {max} characters"
        )));
    }
    Ok(())
}

fn validate_delegate_options(args: &Value) -> Result<(), McpError> {
    if let Some(preference) = optional_string(args, "preference")
        && !["balanced", "quality", "cost", "speed"].contains(&preference.as_str())
    {
        return Err(McpError::InvalidParams(
            "preference must be balanced, quality, cost, or speed".into(),
        ));
    }
    if args.get("difficulty").is_some() {
        return Err(McpError::InvalidParams(
            "difficulty is no longer an option; name the kind of work with kind, and set how \
             hard the model thinks with effort"
                .into(),
        ));
    }
    if let Some(kind) = optional_string(args, "kind")
        && WorkKind::parse(&kind).is_none()
    {
        return Err(McpError::InvalidParams(format!(
            "there is no kind of work called '{kind}'; name one of {}",
            WorkKind::ALL
                .iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if let Some(effort) = optional_string(args, "effort")
        && !["minimal", "low", "medium", "high", "xhigh", "max"].contains(&effort.as_str())
    {
        return Err(McpError::InvalidParams(
            "effort must be minimal, low, medium, high, xhigh, or max".into(),
        ));
    }
    if optional_u64(args, "timeoutMs")?.is_some_and(|timeout| timeout > 86_400_000) {
        return Err(McpError::InvalidParams(
            "timeoutMs must be between 1 and 86400000".into(),
        ));
    }
    if let Some(depends_on) = args.get("dependsOn")
        && depends_on
            .as_array()
            .is_some_and(|values| values.len() > 16)
    {
        return Err(McpError::InvalidParams(
            "dependsOn must contain at most 16 task ids".into(),
        ));
    }
    Ok(())
}

fn scope(value: Option<&Value>) -> Result<TaskScope, McpError> {
    value.map(scope_value).transpose().map(|value| {
        value.unwrap_or_else(|| TaskScope {
            read: vec!["**".into()],
            write: vec!["**".into()],
        })
    })
}

fn scope_value(value: &Value) -> Result<TaskScope, McpError> {
    let scope: TaskScope = serde_json::from_value(value.clone())
        .map_err(|error| McpError::InvalidParams(format!("scope: {error}")))?;
    if scope.read.len() > 200 || scope.write.len() > 200 {
        return Err(McpError::InvalidParams(
            "scope read and write arrays must contain at most 200 paths".into(),
        ));
    }
    Ok(scope)
}

fn string_array(value: Option<&Value>, key: &str) -> Result<Vec<String>, McpError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| McpError::InvalidParams(format!("{key} must be an array")))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    McpError::InvalidParams(format!("{key} must contain non-empty strings"))
                })
        })
        .collect()
}

fn fields(value: Option<&Value>) -> Result<Option<Vec<String>>, McpError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let values = value
        .as_array()
        .ok_or_else(|| McpError::InvalidParams("fields must be an array".into()))?;
    let allowed = [
        "routing",
        "context",
        "label",
        "location",
        "scope",
        "prompt",
        "shippedPrompt",
        "output",
        "attempts",
        "completion",
        "spend",
        "all",
    ];
    values
        .iter()
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| McpError::InvalidParams("fields must contain strings".into()))?;
            if !allowed.contains(&value) {
                return Err(McpError::InvalidParams(format!(
                    "unknown task field: {value}"
                )));
            }
            Ok(value.to_owned())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn task_ids(value: Option<&Value>) -> Result<Vec<String>, McpError> {
    let Some(value) = value else {
        return Err(McpError::InvalidParams("taskId is required".into()));
    };
    if let Some(id) = value.as_str() {
        if id.is_empty() {
            return Err(McpError::InvalidParams("taskId is required".into()));
        }
        return Ok(vec![id.into()]);
    }
    let values = value
        .as_array()
        .filter(|values| !values.is_empty())
        .ok_or_else(|| {
            McpError::InvalidParams("taskId must be a string or non-empty array".into())
        })?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| McpError::InvalidParams("taskId array must contain strings".into()))
        })
        .collect()
}

fn task_query(args: &Value) -> Result<TaskListQuery, McpError> {
    let explicit = [
        "state", "since", "until", "profile", "parent", "archived", "query",
    ]
    .iter()
    .any(|key| args.get(*key).is_some());
    let since = optional_string(args, "since").or_else(|| (!explicit).then(local_midnight));
    let state = args.get("state").map(parse_state_filter).transpose()?;
    let archived = optional_string(args, "archived")
        .map(|value| match value.as_str() {
            "only" => Ok(ArchivedFilter::Only),
            "include" => Ok(ArchivedFilter::Include),
            "active" => Ok(ArchivedFilter::Active),
            _ => Err(McpError::InvalidParams(
                "archived must be active, only, or include".into(),
            )),
        })
        .transpose()?;
    let order = match optional_string(args, "order").as_deref() {
        Some("oldest") => Some(ListOrder::Oldest),
        Some("newest") | None => Some(ListOrder::Newest),
        Some(_) => {
            return Err(McpError::InvalidParams(
                "order must be newest or oldest".into(),
            ));
        }
    };
    Ok(TaskListQuery {
        limit: optional_u64(args, "limit")?.or(Some(20)),
        state,
        since,
        until: optional_string(args, "until"),
        order,
        profile: optional_string(args, "profile"),
        parent: optional_string(args, "parent"),
        archived: archived.or(Some(ArchivedFilter::Active)),
        query: optional_string(args, "query"),
    })
}

fn parse_provider(value: &str) -> Result<Provider, McpError> {
    serde_json::from_value(json!(value))
        .map_err(|_| McpError::InvalidParams(format!("unknown provider: {value}")))
}

fn parse_state_filter(value: &Value) -> Result<StateFilter, McpError> {
    if let Some(value) = value.as_str() {
        return parse_state(value).map(StateFilter::One);
    }
    let values = value
        .as_array()
        .filter(|values| !values.is_empty())
        .ok_or_else(|| {
            McpError::InvalidParams("state must be a state or array of states".into())
        })?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| McpError::InvalidParams("state array must contain strings".into()))
                .and_then(parse_state)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(StateFilter::Many)
}

fn parse_state(value: &str) -> Result<TaskState, McpError> {
    serde_json::from_value(json!(value))
        .map_err(|_| McpError::InvalidParams(format!("unknown task state: {value}")))
}

fn state_matches(state: TaskState, filter: Option<&StateFilter>) -> bool {
    match filter {
        None => true,
        Some(StateFilter::One(value)) => state == *value,
        Some(StateFilter::Many(values)) => values.contains(&state),
    }
}

fn archive_matches(archived: bool, filter: Option<ArchivedFilter>) -> bool {
    match filter.unwrap_or(ArchivedFilter::Active) {
        ArchivedFilter::Active => !archived,
        ArchivedFilter::Only => archived,
        ArchivedFilter::Include => true,
    }
}

fn local_midnight() -> String {
    let now = Local::now();
    let date = now.date_naive();
    let local = Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight"))
        .single()
        .unwrap_or(now);
    local
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn project_cwd(task: &Task) -> String {
    task.worktree
        .as_ref()
        .map_or_else(|| task.cwd.clone(), |worktree| worktree.origin_cwd.clone())
}

async fn task_branch_unavailable(task: &Task) -> bool {
    let Some(worktree) = task.worktree.as_ref() else {
        return false;
    };
    !oga_worktree::branch_recreatable(worktree)
        .await
        .unwrap_or(false)
}
