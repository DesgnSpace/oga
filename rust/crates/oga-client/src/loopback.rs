//! The loopback HTTP and SSE transport.
//!
//! Native only. The desktop web view runs in WebAssembly and reaches the
//! broker through the shell's bridge instead, so nothing here compiles there.

use std::{env, time::Duration};

use bytes::Bytes;
use futures_util::{StreamExt, stream::BoxStream};
use oga_domain::{
    ActivityCounts, ArchivedFilter, CleanupSettings, CleanupSnapshot, ConsumerCursor, HealthReport,
    MemoryEntry, ModelInfo, ModelQuery, ProfileUsage, ProfileView, Task, TaskDiff, TaskTurn,
    WorktreeDeleteEntry,
};
use reqwest::{Method, StatusCode, header};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;
use url::Url;

use crate::{
    AgentRemoved, AgentStopped, BrokerState, BrokerSummaryState, CompletionRequest, ConsumerInbox,
    DispatchRequest, EventFrame, EventHead, EventStreamOptions, EventStreamQuery, HandoffRequest,
    MapInitRequest, MapInitResponse, MapQuery, MapResponse, MemoryList, MemoryWrite,
    ModelSettingsSnapshot, ModelSettingsUpdate, ProfileCreate, ProfilePatch, ProjectList,
    PromptConfig, PromptWrite, QueryRequest, ReplyRequest, ResumeRequest, RoutingPreview,
    RoutingPreviewRequest, StateQuery, SteerRequest, TaskActionResponse, TaskEventPage,
    TaskEventsQuery, TurnsResponse, UsageResponse,
};

/// Errors returned by the loopback transport or by a broker response.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("invalid broker URL: {0}")]
    InvalidBaseUrl(String),
    #[error("request {method} {url} failed: {source}")]
    Transport {
        method: String,
        url: Url,
        #[source]
        source: reqwest::Error,
    },
    #[error("{method} {url} -> {status}: {message}")]
    Http {
        method: String,
        url: Box<Url>,
        status: StatusCode,
        message: String,
    },
    #[error("could not encode request body: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("invalid JSON from {method} {url}: {source}")]
    Decode {
        method: String,
        url: Url,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid SSE frame: {0}")]
    Sse(String),
}

impl ClientError {
    /// Returns the HTTP status when the broker answered with an error.
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Self::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

type EventBody = BoxStream<'static, Result<Bytes, reqwest::Error>>;

/// A cursor-aware global event stream that reconnects after EOF or a read error.
pub struct EventStream {
    client: LoopbackClient,
    query: EventStreamQuery,
    options: EventStreamOptions,
    cursor: i64,
    backoff: Duration,
    body: Option<EventBody>,
    buffer: Vec<u8>,
}

impl EventStream {
    pub fn cursor(&self) -> i64 {
        self.cursor
    }

    pub async fn next(&mut self) -> Result<EventFrame, ClientError> {
        self.next_with_reconnect(true)
            .await?
            .ok_or_else(|| ClientError::Sse("event stream closed".into()))
    }

    pub async fn next_once(&mut self) -> Result<Option<EventFrame>, ClientError> {
        self.next_with_reconnect(false).await
    }

    async fn next_with_reconnect(
        &mut self,
        reconnect: bool,
    ) -> Result<Option<EventFrame>, ClientError> {
        loop {
            while let Some(block) = take_sse_block(&mut self.buffer) {
                let Some(frame) = parse_sse_block(&block)? else {
                    continue;
                };
                self.advance(&frame);
                return Ok(Some(frame));
            }
            if self.body.is_none() {
                if !reconnect {
                    return Ok(None);
                }
                self.reconnect().await?;
            }

            let chunk = match self.body.as_mut() {
                Some(body) => body.next().await,
                None => continue,
            };
            match chunk {
                Some(Ok(bytes)) => {
                    self.buffer.extend_from_slice(&bytes);
                }
                Some(Err(_)) | None => {
                    self.body = None;
                    self.buffer.clear();
                    if !reconnect {
                        return Ok(None);
                    }
                    self.reconnect().await?;
                }
            }
        }
    }

    fn advance(&mut self, frame: &EventFrame) {
        if matches!(frame, EventFrame::Ready(_)) {
            self.backoff = self.options.initial_backoff.min(self.options.max_backoff);
        }
        if let Some(cursor) = frame.cursor() {
            self.cursor = self.cursor.max(cursor);
        }
    }

    async fn reconnect(&mut self) -> Result<(), ClientError> {
        if self.body.is_some() {
            return Ok(());
        }
        tokio::time::sleep(self.backoff).await;
        let url = self.client.event_stream_url(&self.query, self.cursor);
        let response = self.client.send_stream_request(url.clone()).await?;
        let next_backoff = self.next_backoff();
        self.body = Some(response.bytes_stream().boxed());
        self.backoff = next_backoff;
        Ok(())
    }

    fn next_backoff(&self) -> Duration {
        self.backoff
            .checked_mul(2)
            .unwrap_or(self.options.max_backoff)
            .min(self.options.max_backoff)
    }
}

/// HTTP/SSE access to a running loopback broker.
#[derive(Clone)]
pub struct LoopbackClient {
    http: reqwest::Client,
    base_url: Url,
}

impl Default for LoopbackClient {
    fn default() -> Self {
        Self::localhost(7331)
    }
}

impl LoopbackClient {
    pub fn new(base_url: impl AsRef<str>) -> Result<Self, ClientError> {
        Self::with_http_client(base_url, reqwest::Client::new())
    }

    pub fn from_env() -> Result<Self, ClientError> {
        let base_url = env::var("OGA_BROKER_URL").unwrap_or_else(|_| {
            let port = env::var("OGA_PORT")
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(7331);
            format!("http://127.0.0.1:{port}")
        });
        Self::new(base_url)
    }

    pub fn localhost(port: u16) -> Self {
        Self::new(format!("http://127.0.0.1:{port}")).expect("the localhost broker URL is valid")
    }

    pub fn with_http_client(
        base_url: impl AsRef<str>,
        http: reqwest::Client,
    ) -> Result<Self, ClientError> {
        let raw = base_url.as_ref();
        let mut base_url =
            Url::parse(raw).map_err(|error| ClientError::InvalidBaseUrl(error.to_string()))?;
        if base_url.host_str().is_none() || base_url.cannot_be_a_base() {
            return Err(ClientError::InvalidBaseUrl(raw.to_owned()));
        }
        base_url.set_query(None);
        base_url.set_fragment(None);
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        Ok(Self { http, base_url })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub async fn health(&self) -> Result<HealthReport, ClientError> {
        self.get_json(self.endpoint(&["health"])).await
    }

    pub async fn get_state(&self, query: &StateQuery) -> Result<BrokerState, ClientError> {
        let mut url = self.endpoint(&["api", "state"]);
        append_state_query(&mut url, query, false);
        self.get_json(url).await
    }

    pub async fn get_summary(&self, query: &StateQuery) -> Result<BrokerSummaryState, ClientError> {
        let mut url = self.endpoint(&["api", "state"]);
        append_state_query(&mut url, query, true);
        self.get_json(url).await
    }

    pub async fn get_activity(&self) -> Result<ActivityCounts, ClientError> {
        self.get_json(self.endpoint(&["api", "activity"])).await
    }

    pub async fn get_task(&self, task_id: &str) -> Result<Task, ClientError> {
        self.get_json(self.endpoint(&["api", "tasks", task_id]))
            .await
    }

    pub async fn get_task_turns(&self, task_id: &str) -> Result<Vec<TaskTurn>, ClientError> {
        let response: TurnsResponse = self
            .get_json(self.endpoint(&["api", "tasks", task_id, "turns"]))
            .await?;
        Ok(response.turns)
    }

    pub async fn get_task_events(
        &self,
        task_id: &str,
        query: &TaskEventsQuery,
    ) -> Result<TaskEventPage, ClientError> {
        self.get_event_page(&["api", "tasks", task_id, "events"], query)
            .await
    }

    pub async fn get_task_diff(&self, task_id: &str) -> Result<TaskDiff, ClientError> {
        self.get_json(self.endpoint(&["api", "tasks", task_id, "diff"]))
            .await
    }

    pub async fn mark_task_viewed(&self, task_id: &str) -> Result<(), ClientError> {
        self.send_empty(
            Method::POST,
            self.endpoint(&["api", "tasks", task_id, "view"]),
        )
        .await
    }

    pub async fn unmark_task_viewed(&self, task_id: &str) -> Result<(), ClientError> {
        self.send_empty(
            Method::DELETE,
            self.endpoint(&["api", "tasks", task_id, "view"]),
        )
        .await
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<Task, ClientError> {
        self.get_json(self.endpoint(&["api", "agents", agent_id]))
            .await
    }

    pub async fn get_agent_turns(&self, agent_id: &str) -> Result<Vec<TaskTurn>, ClientError> {
        let response: TurnsResponse = self
            .get_json(self.endpoint(&["api", "agents", agent_id, "turns"]))
            .await?;
        Ok(response.turns)
    }

    pub async fn get_agent_events(
        &self,
        agent_id: &str,
        query: &TaskEventsQuery,
    ) -> Result<TaskEventPage, ClientError> {
        self.get_event_page(&["api", "agents", agent_id, "events"], query)
            .await
    }

    pub async fn event_stream(&self, query: EventStreamQuery) -> Result<EventStream, ClientError> {
        self.event_stream_with_options(query, EventStreamOptions::default())
            .await
    }

    pub async fn event_head(&self) -> Result<i64, ClientError> {
        Ok(self
            .get_json::<EventHead>(self.endpoint(&["api", "events", "head"]))
            .await?
            .cursor)
    }

    pub async fn event_stream_with_options(
        &self,
        query: EventStreamQuery,
        options: EventStreamOptions,
    ) -> Result<EventStream, ClientError> {
        let mut stream = EventStream {
            cursor: query.after,
            query,
            backoff: options.initial_backoff.min(options.max_backoff),
            options,
            client: self.clone(),
            body: None,
            buffer: Vec::new(),
        };
        stream.reconnect().await?;
        Ok(stream)
    }

    pub async fn dispatch(
        &self,
        request: &DispatchRequest,
    ) -> Result<TaskActionResponse, ClientError> {
        self.post_json(
            self.endpoint(&["api", "tasks"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn start_agent(&self, request: &DispatchRequest) -> Result<Task, ClientError> {
        self.post_json(
            self.endpoint(&["api", "agents"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn dispatch_agent_task(
        &self,
        agent_id: &str,
        request: &DispatchRequest,
    ) -> Result<TaskActionResponse, ClientError> {
        self.post_json(
            self.endpoint(&["api", "agents", agent_id, "tasks"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn archive_task(
        &self,
        task_id: &str,
        archived: bool,
    ) -> Result<TaskActionResponse, ClientError> {
        self.archive_task_with_branch_deletion(task_id, archived, false)
            .await
    }

    pub async fn archive_task_with_branch_deletion(
        &self,
        task_id: &str,
        archived: bool,
        delete_branch: bool,
    ) -> Result<TaskActionResponse, ClientError> {
        self.send_json(
            Method::PATCH,
            self.endpoint(&["api", "tasks", task_id]),
            Some(serde_json::json!({
                "archived": archived,
                "deleteBranch": delete_branch,
            })),
            None,
        )
        .await
    }

    pub async fn cancel_task(
        &self,
        task_id: &str,
        reason: Option<&str>,
    ) -> Result<TaskActionResponse, ClientError> {
        let mut url = self.endpoint(&["api", "tasks", task_id]);
        if let Some(reason) = reason {
            url.query_pairs_mut().append_pair("reason", reason);
        }
        self.send_json(Method::DELETE, url, None, None).await
    }

    pub async fn resume_task(
        &self,
        task_id: &str,
        request: &ResumeRequest,
    ) -> Result<TaskActionResponse, ClientError> {
        self.post_json(
            self.endpoint(&["api", "tasks", task_id, "resume"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn reply_task(
        &self,
        task_id: &str,
        request: &ReplyRequest,
    ) -> Result<TaskActionResponse, ClientError> {
        self.post_json(
            self.endpoint(&["api", "tasks", task_id, "reply"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn remove_follow_up(
        &self,
        task_id: &str,
        index: usize,
    ) -> Result<Value, ClientError> {
        self.send_json(
            Method::DELETE,
            self.endpoint(&["api", "tasks", task_id, "follow_ups", &index.to_string()]),
            None,
            None,
        )
        .await
    }

    pub async fn steer_task(
        &self,
        task_id: &str,
        request: &SteerRequest,
    ) -> Result<TaskActionResponse, ClientError> {
        self.post_json(
            self.endpoint(&["api", "tasks", task_id, "steer"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn steer_agent(
        &self,
        agent_id: &str,
        request: &SteerRequest,
    ) -> Result<TaskActionResponse, ClientError> {
        self.post_json(
            self.endpoint(&["api", "agents", agent_id, "steer"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn handoff_task(
        &self,
        task_id: &str,
        request: &HandoffRequest,
    ) -> Result<TaskActionResponse, ClientError> {
        self.post_json(
            self.endpoint(&["api", "tasks", task_id, "handoff"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn remove_worktree(
        &self,
        task_id: &str,
        delete_branch: bool,
    ) -> Result<WorktreeDeleteEntry, ClientError> {
        let mut url = self.endpoint(&["api", "tasks", task_id, "worktree"]);
        url.query_pairs_mut()
            .append_pair("deleteBranch", if delete_branch { "true" } else { "false" });
        self.send_json(Method::DELETE, url, None, None).await
    }

    pub async fn complete_task(
        &self,
        task_id: &str,
        request: &CompletionRequest,
    ) -> Result<TaskActionResponse, ClientError> {
        self.post_json(
            self.endpoint(&["api", "tasks", task_id, "complete"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn append_hook(&self, task_id: &str, payload: &Value) -> Result<Value, ClientError> {
        self.post_json(self.endpoint(&["api", "hooks", task_id]), payload.clone())
            .await
    }

    pub async fn stop_agent(&self, agent_id: &str) -> Result<AgentStopped, ClientError> {
        self.post_json(
            self.endpoint(&["api", "agents", agent_id, "stop"]),
            Value::Null,
        )
        .await
    }

    pub async fn remove_agent(&self, agent_id: &str) -> Result<AgentRemoved, ClientError> {
        self.send_json(
            Method::DELETE,
            self.endpoint(&["api", "agents", agent_id]),
            None,
            None,
        )
        .await
    }

    pub async fn consumer_inbox(
        &self,
        consumer_id: &str,
        channel: Option<&str>,
        limit: Option<u64>,
    ) -> Result<ConsumerInbox, ClientError> {
        let mut url = self.endpoint(&["api", "consumers", consumer_id, "inbox"]);
        if let Some(channel) = channel {
            url.query_pairs_mut().append_pair("channel", channel);
        }
        if let Some(limit) = limit {
            url.query_pairs_mut()
                .append_pair("limit", &limit.to_string());
        }
        self.get_json(url).await
    }

    pub async fn advance_cursor(
        &self,
        consumer_id: &str,
        cursor: i64,
    ) -> Result<ConsumerCursor, ClientError> {
        self.post_json(
            self.endpoint(&["api", "consumers", consumer_id, "cursor"]),
            serde_json::json!({ "cursor": cursor }),
        )
        .await
    }

    pub async fn routing_preview(
        &self,
        request: &RoutingPreviewRequest,
    ) -> Result<RoutingPreview, ClientError> {
        self.post_json(
            self.endpoint(&["api", "routing", "preview"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn memories(&self, cwd: &str) -> Result<Vec<MemoryEntry>, ClientError> {
        let mut url = self.endpoint(&["api", "memories"]);
        url.query_pairs_mut().append_pair("cwd", cwd);
        let response: MemoryList = self.get_json(url).await?;
        Ok(response.memories)
    }

    pub async fn put_memory(&self, request: &MemoryWrite) -> Result<MemoryEntry, ClientError> {
        self.put_json(
            self.endpoint(&["api", "memories"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn prompt(&self, cwd: Option<&str>) -> Result<PromptConfig, ClientError> {
        let mut url = self.endpoint(&["api", "prompts"]);
        if let Some(cwd) = cwd {
            url.query_pairs_mut().append_pair("cwd", cwd);
        }
        self.get_json(url).await
    }

    pub async fn put_prompt(&self, request: &PromptWrite) -> Result<PromptConfig, ClientError> {
        self.put_json(
            self.endpoint(&["api", "prompts"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn delete_prompt(&self, cwd: Option<&str>) -> Result<PromptConfig, ClientError> {
        let mut url = self.endpoint(&["api", "prompts"]);
        if let Some(cwd) = cwd {
            url.query_pairs_mut().append_pair("cwd", cwd);
        }
        self.send_json(Method::DELETE, url, None, None).await
    }

    pub async fn projects(&self) -> Result<ProjectList, ClientError> {
        self.get_json(self.endpoint(&["api", "projects"])).await
    }

    pub async fn cleanup(&self) -> Result<CleanupSnapshot, ClientError> {
        self.get_json(self.endpoint(&["api", "cleanup"])).await
    }

    pub async fn put_cleanup(
        &self,
        settings: &CleanupSettings,
    ) -> Result<CleanupSnapshot, ClientError> {
        self.put_json(
            self.endpoint(&["api", "cleanup"]),
            serde_json::to_value(settings).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn waiting(&self) -> Result<oga_domain::WaitSettings, ClientError> {
        self.get_json(self.endpoint(&["api", "waiting"])).await
    }

    pub async fn put_waiting(
        &self,
        settings: &oga_domain::WaitSettings,
    ) -> Result<oga_domain::WaitSettings, ClientError> {
        self.put_json(
            self.endpoint(&["api", "waiting"]),
            serde_json::to_value(settings).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn run_cleanup(&self) -> Result<oga_domain::CleanupResult, ClientError> {
        self.post_json(self.endpoint(&["api", "cleanup", "run"]), Value::Null)
            .await
    }

    pub async fn models(&self, query: &ModelQuery) -> Result<Vec<ModelInfo>, ClientError> {
        let mut url = self.endpoint(&["api", "models"]);
        append_model_query(&mut url, query);
        self.get_json(url).await
    }

    pub async fn usage(&self, query: &ModelQuery) -> Result<Vec<ProfileUsage>, ClientError> {
        let mut url = self.endpoint(&["api", "provider-usage"]);
        append_model_query(&mut url, query);
        self.get_json(url).await
    }

    pub async fn spend_usage(&self, tz_offset: Option<i32>) -> Result<UsageResponse, ClientError> {
        let mut url = self.endpoint(&["api", "usage"]);
        if let Some(offset) = tz_offset {
            url.query_pairs_mut()
                .append_pair("tzOffset", &offset.to_string());
        }
        self.get_json(url).await
    }

    pub async fn delete_grant(&self, grant_id: &str) -> Result<(), ClientError> {
        self.send_empty(Method::DELETE, self.endpoint(&["api", "grants", grant_id]))
            .await
    }

    pub async fn model_settings(
        &self,
        cwd: Option<&str>,
        refresh: bool,
    ) -> Result<ModelSettingsSnapshot, ClientError> {
        let mut url = self.endpoint(&["api", "model-settings"]);
        if let Some(cwd) = cwd {
            url.query_pairs_mut().append_pair("cwd", cwd);
        }
        url.query_pairs_mut()
            .append_pair("refresh", if refresh { "true" } else { "false" });
        self.get_json(url).await
    }

    pub async fn put_model_settings(
        &self,
        request: &ModelSettingsUpdate,
    ) -> Result<ModelSettingsSnapshot, ClientError> {
        self.put_json(
            self.endpoint(&["api", "model-settings"]),
            serde_json::to_value(request).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn delete_model_settings(
        &self,
        cwd: Option<&str>,
        revision: Option<&str>,
    ) -> Result<ModelSettingsSnapshot, ClientError> {
        let mut url = self.endpoint(&["api", "model-settings"]);
        if let Some(cwd) = cwd {
            url.query_pairs_mut().append_pair("cwd", cwd);
        }
        if let Some(revision) = revision {
            url.query_pairs_mut().append_pair("revision", revision);
        }
        self.send_json(Method::DELETE, url, None, None).await
    }

    pub async fn query(&self, request: &QueryRequest) -> Result<String, ClientError> {
        let mut url = self.endpoint(&["api", "query"]);
        url.query_pairs_mut().append_pair("cwd", &request.cwd);
        url.query_pairs_mut().append_pair("q", &request.question);
        if let Some(limit) = request.limit {
            url.query_pairs_mut()
                .append_pair("limit", &limit.to_string());
        }
        if request.code {
            url.query_pairs_mut().append_pair("code", "true");
        }
        let response: MarkdownResponse = self.get_json(url).await?;
        Ok(response.markdown)
    }

    pub async fn map(&self, request: &MapQuery) -> Result<MapResponse, ClientError> {
        let mut url = self.endpoint(&["api", "map"]);
        url.query_pairs_mut().append_pair("task", &request.task);
        for path in &request.paths {
            url.query_pairs_mut().append_pair("path", path);
        }
        for symbol in &request.symbols {
            url.query_pairs_mut().append_pair("symbol", symbol);
        }
        if let Some(depth) = request.depth {
            url.query_pairs_mut()
                .append_pair("depth", &depth.to_string());
        }
        if let Some(question) = &request.question {
            url.query_pairs_mut().append_pair("q", question);
        }
        if let Some(tier) = request.tier {
            url.query_pairs_mut().append_pair("tier", tier.as_str());
        }
        if let Some(limit) = request.limit {
            url.query_pairs_mut()
                .append_pair("limit", &limit.to_string());
        }
        if request.code {
            url.query_pairs_mut().append_pair("code", "true");
        }
        self.send_json(Method::GET, url, None, Some("application/json"))
            .await
    }

    pub async fn init_map(&self, request: &MapInitRequest) -> Result<MapInitResponse, ClientError> {
        let mut url = self.endpoint(&["api", "map", "init"]);
        url.query_pairs_mut().append_pair("cwd", &request.cwd);
        if request.force {
            url.query_pairs_mut().append_pair("force", "true");
        }
        self.send_json(Method::POST, url, None, None).await
    }

    pub async fn create_profile(
        &self,
        profile: &ProfileCreate,
    ) -> Result<ProfileView, ClientError> {
        self.post_json(
            self.endpoint(&["api", "profiles"]),
            serde_json::to_value(profile).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn update_profile(
        &self,
        profile_id: &str,
        patch: &ProfilePatch,
    ) -> Result<ProfileView, ClientError> {
        self.put_json(
            self.endpoint(&["api", "profiles", profile_id]),
            serde_json::to_value(patch).map_err(ClientError::Encode)?,
        )
        .await
    }

    pub async fn delete_profile(&self, profile_id: &str) -> Result<(), ClientError> {
        self.send_empty(
            Method::DELETE,
            self.endpoint(&["api", "profiles", profile_id]),
        )
        .await
    }

    async fn get_event_page(
        &self,
        segments: &[&str],
        query: &TaskEventsQuery,
    ) -> Result<TaskEventPage, ClientError> {
        let mut url = self.endpoint(segments);
        append_event_query(&mut url, query);
        let value: Value = self.send_json(Method::GET, url.clone(), None, None).await?;
        if value.is_array() {
            let events = serde_json::from_value(value).map_err(|source| ClientError::Decode {
                method: Method::GET.as_str().to_owned(),
                url,
                source,
            })?;
            return Ok(TaskEventPage {
                events,
                cursor: None,
                has_more: None,
                oldest_id: None,
                has_earlier: None,
            });
        }
        serde_json::from_value(value).map_err(|source| ClientError::Decode {
            method: Method::GET.as_str().to_owned(),
            url,
            source,
        })
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> Result<T, ClientError> {
        self.send_json(Method::GET, url, None, None).await
    }

    async fn post_json<T: DeserializeOwned>(
        &self,
        url: Url,
        body: Value,
    ) -> Result<T, ClientError> {
        self.send_json(Method::POST, url, Some(body), None).await
    }

    async fn put_json<T: DeserializeOwned>(&self, url: Url, body: Value) -> Result<T, ClientError> {
        self.send_json(Method::PUT, url, Some(body), None).await
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<Value>,
        accept: Option<&str>,
    ) -> Result<T, ClientError> {
        let bytes = self.send(method.clone(), url.clone(), body, accept).await?;
        serde_json::from_slice(&bytes).map_err(|source| ClientError::Decode {
            method: method.as_str().to_owned(),
            url,
            source,
        })
    }

    async fn send_empty(&self, method: Method, url: Url) -> Result<(), ClientError> {
        self.send(method, url, None, None).await.map(|_| ())
    }

    async fn send(
        &self,
        method: Method,
        url: Url,
        body: Option<Value>,
        accept: Option<&str>,
    ) -> Result<Bytes, ClientError> {
        let method_name = method.as_str().to_owned();
        let mut request = self
            .http
            .request(method, url.clone())
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(accept) = accept {
            request = request.header(header::ACCEPT, accept);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|source| ClientError::Transport {
                method: method_name.clone(),
                url: url.clone(),
                source,
            })?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|source| ClientError::Transport {
                method: method_name.clone(),
                url: url.clone(),
                source,
            })?;
        if !status.is_success() {
            return Err(ClientError::Http {
                method: method_name,
                url: Box::new(url),
                status,
                message: error_message(&bytes, status),
            });
        }
        Ok(bytes)
    }

    async fn send_stream_request(&self, url: Url) -> Result<reqwest::Response, ClientError> {
        let method = Method::GET;
        let response = self
            .http
            .get(url.clone())
            .header(header::ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(|source| ClientError::Transport {
                method: method.as_str().to_owned(),
                url: url.clone(),
                source,
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|source| ClientError::Transport {
                method: method.as_str().to_owned(),
                url: url.clone(),
                source,
            })?;
        Err(ClientError::Http {
            method: method.as_str().to_owned(),
            url: Box::new(url),
            status,
            message: error_message(&bytes, status),
        })
    }

    fn event_stream_url(&self, query: &EventStreamQuery, cursor: i64) -> Url {
        let mut url = self.endpoint(&["api", "events"]);
        url.query_pairs_mut()
            .append_pair("after", &cursor.to_string());
        for task in &query.tasks {
            url.query_pairs_mut().append_pair("task", task);
        }
        if !query.kinds.is_empty() {
            let kinds = query
                .kinds
                .iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<_>>()
                .join(",");
            url.query_pairs_mut().append_pair("kinds", &kinds);
        }
        if query.agents {
            url.query_pairs_mut().append_pair("agents", "1");
        }
        url
    }

    fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.base_url.clone();
        let mut path = url
            .path_segments_mut()
            .expect("broker base URL can be used as a path base");
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
        drop(path);
        url
    }
}

fn append_state_query(url: &mut Url, query: &StateQuery, summary: bool) {
    if summary {
        url.query_pairs_mut().append_pair("view", "summary");
    }
    if query.compact {
        url.query_pairs_mut().append_pair("compact", "1");
    }
    if let Some(archived) = query.archived {
        url.query_pairs_mut()
            .append_pair("archived", archived_name(archived));
    }
    if let Some(limit) = query.limit {
        url.query_pairs_mut()
            .append_pair("limit", &limit.to_string());
    }
}

fn append_event_query(url: &mut Url, query: &TaskEventsQuery) {
    if let Some(after) = query.after {
        url.query_pairs_mut()
            .append_pair("after", &after.to_string());
    }
    if let Some(wait_ms) = query.wait_ms {
        url.query_pairs_mut()
            .append_pair("waitMs", &wait_ms.to_string());
    }
    if let Some(last) = query.last {
        url.query_pairs_mut().append_pair("last", &last.to_string());
    }
    if let Some(before) = query.before {
        url.query_pairs_mut()
            .append_pair("before", &before.to_string());
    }
    if let Some(limit) = query.limit {
        url.query_pairs_mut()
            .append_pair("limit", &limit.to_string());
    }
}

fn append_model_query(url: &mut Url, query: &ModelQuery) {
    if let Some(profile) = &query.profile {
        url.query_pairs_mut().append_pair("profile", profile);
    }
    if let Some(provider) = query.provider {
        url.query_pairs_mut()
            .append_pair("provider", provider.as_str());
    }
    if let Some(refresh) = query.refresh {
        url.query_pairs_mut()
            .append_pair("refresh", if refresh { "true" } else { "false" });
    }
    if let Some(cwd) = &query.cwd {
        url.query_pairs_mut().append_pair("cwd", cwd);
    }
    if let Some(include_disabled) = query.include_disabled {
        url.query_pairs_mut().append_pair(
            "includeDisabled",
            if include_disabled { "true" } else { "false" },
        );
    }
    if let Some(only_preferred) = query.only_preferred {
        url.query_pairs_mut().append_pair(
            "onlyPreferred",
            if only_preferred { "true" } else { "false" },
        );
    }
    if let Some(only_enabled) = query.only_enabled {
        url.query_pairs_mut()
            .append_pair("onlyEnabled", if only_enabled { "true" } else { "false" });
    }
    if let Some(query_text) = &query.query {
        url.query_pairs_mut().append_pair("query", query_text);
    }
}

fn archived_name(filter: ArchivedFilter) -> &'static str {
    match filter {
        ArchivedFilter::Active => "active",
        ArchivedFilter::Only => "only",
        ArchivedFilter::Include => "include",
    }
}

fn error_message(bytes: &[u8], status: StatusCode) -> String {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: Option<String>,
    }

    serde_json::from_slice::<ErrorBody>(bytes)
        .ok()
        .and_then(|body| body.error)
        .filter(|message| !message.is_empty())
        .or_else(|| {
            let text = String::from_utf8_lossy(bytes).trim().to_owned();
            (!text.is_empty()).then_some(text)
        })
        .unwrap_or_else(|| {
            status
                .canonical_reason()
                .unwrap_or("request failed")
                .to_owned()
        })
}

fn take_sse_block(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let line_feed_end = buffer.windows(2).position(|window| window == b"\n\n");
    let carriage_return_end = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    let (end, delimiter_len) = match (line_feed_end, carriage_return_end) {
        (None, None) => return None,
        (Some(end), None) => (end, 2),
        (None, Some(end)) => (end, 4),
        (Some(line_feed_end), Some(carriage_return_end)) => {
            if line_feed_end < carriage_return_end {
                (line_feed_end, 2)
            } else {
                (carriage_return_end, 4)
            }
        }
    };
    let block = buffer[..end].to_vec();
    buffer.drain(..end + delimiter_len);
    Some(block)
}

fn parse_sse_block(block: &[u8]) -> Result<Option<EventFrame>, ClientError> {
    let text = std::str::from_utf8(block)
        .map_err(|error| ClientError::Sse(format!("event data is not UTF-8: {error}")))?;
    let mut event = None;
    let mut data = String::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        }
    }
    let Some(event) = event.filter(|event| !event.is_empty()) else {
        return Ok(None);
    };
    if data.is_empty() {
        return Ok(None);
    }
    let value = serde_json::from_str::<Value>(&data)
        .map_err(|error| ClientError::Sse(format!("{event} data is not JSON: {error}")))?;
    let frame = EventFrame::from_parts(&event, value)
        .map_err(|error| ClientError::Sse(error.to_string()))?;
    Ok(Some(frame))
}

#[derive(Debug, Deserialize)]
struct MarkdownResponse {
    markdown: String,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{Request, StatusCode},
        response::Response,
        routing::any,
    };
    use oga_domain::Provider;
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::*;
    use crate::{CursorFrame, MapTier};

    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<String>>>,
        sse_connections: Arc<AtomicUsize>,
        error: bool,
    }

    async fn start_mock(error: bool) -> (String, MockState, tokio::task::JoinHandle<()>) {
        let state = MockState {
            requests: Arc::new(Mutex::new(Vec::new())),
            sse_connections: Arc::new(AtomicUsize::new(0)),
            error,
        };
        let app = Router::new()
            .fallback(any(mock_request))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener");
        let address = listener.local_addr().expect("mock address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });
        (format!("http://{address}"), state, handle)
    }

    async fn mock_request(
        State(state): State<MockState>,
        request: Request<Body>,
    ) -> Response<Body> {
        let path = request.uri().to_string();
        state
            .requests
            .lock()
            .expect("request lock")
            .push(path.clone());
        if state.error {
            return json_response(StatusCode::CONFLICT, json!({ "error": "stale revision" }));
        }
        if path.starts_with("/api/events") {
            return sse_response(&state);
        }
        let method = request.method().as_str();
        let path_only = request.uri().path();
        let is_summary = request
            .uri()
            .query()
            .is_some_and(|query| query.contains("view=summary"));
        match (method, path_only) {
            ("GET", "/health") => json_response(
                StatusCode::OK,
                json!({
                    "status": "ok", "version": "0.6.0", "mcpContractVersion": 32,
                    "build": "dev", "stale": false
                }),
            ),
            ("GET", "/api/state") if is_summary => json_response(
                StatusCode::OK,
                json!({
                    "profiles": [profile_json()],
                    "tasks": [task_summary_json()],
                    "tasksHasMore": false,
                    "memoryProjects": [],
                    "spend": null
                }),
            ),
            ("GET", "/api/state") => json_response(
                StatusCode::OK,
                json!({
                    "profiles": [profile_json()],
                    "tasks": [task_json()],
                    "tasksHasMore": false,
                    "memoryProjects": [],
                    "spend": null
                }),
            ),
            ("GET", path) if path.ends_with("/turns") => {
                json_response(StatusCode::OK, json!({ "turns": [] }))
            }
            ("GET", path) if path.ends_with("/events") => {
                json_response(StatusCode::OK, json!([event_json()]))
            }
            ("GET", path)
                if path.starts_with("/api/tasks/") || path.starts_with("/api/agents/") =>
            {
                json_response(StatusCode::OK, task_json())
            }
            ("POST", "/api/tasks") => json_response(StatusCode::ACCEPTED, action_json()),
            ("POST", "/api/agents") => json_response(StatusCode::ACCEPTED, task_json()),
            ("POST", path) if path.ends_with("/tasks") => {
                json_response(StatusCode::ACCEPTED, action_json())
            }
            ("PATCH", path) if path.starts_with("/api/tasks/") => {
                json_response(StatusCode::OK, action_json())
            }
            ("DELETE", path) if path.starts_with("/api/tasks/") && path.ends_with("/worktree") => {
                json_response(
                    StatusCode::OK,
                    json!({ "taskId": "task", "checkout": "removed", "branch": "kept" }),
                )
            }
            ("DELETE", path)
                if path.starts_with("/api/tasks/") && path.contains("/follow_ups/") =>
            {
                json_response(StatusCode::OK, json!({ "ok": true }))
            }
            ("DELETE", path) if path.starts_with("/api/tasks/") => {
                json_response(StatusCode::OK, action_json())
            }
            ("POST", path) if path.starts_with("/api/tasks/") => {
                json_response(StatusCode::ACCEPTED, action_json())
            }
            ("POST", path) if path.ends_with("/stop") => {
                json_response(StatusCode::OK, json!({ "stopped": true }))
            }
            ("POST", path) if path.starts_with("/api/agents/") => {
                json_response(StatusCode::ACCEPTED, action_json())
            }
            ("DELETE", path) if path.starts_with("/api/agents/") => {
                json_response(StatusCode::OK, json!({ "removed": true }))
            }
            ("POST", path) if path.ends_with("/cursor") => json_response(
                StatusCode::OK,
                json!({ "consumerId": "consumer", "cursor": 1, "updatedAt": "2026-01-01T00:00:00.000Z" }),
            ),
            ("GET", path) if path.ends_with("/inbox") => json_response(
                StatusCode::OK,
                json!({ "consumerId": "consumer", "channel": "app", "cursor": 0,
                    "updatedAt": "2026-01-01T00:00:00.000Z", "deliveries": [] }),
            ),
            ("POST", path) if path.ends_with("/seen") => json_response(
                StatusCode::OK,
                json!({ "consumerId": "consumer", "eventId": 1, "channel": "app",
                    "status": "seen", "attempts": 1 }),
            ),
            ("POST", "/api/routing/preview") => json_response(
                StatusCode::OK,
                json!({ "profileId": "profile", "model": "model", "difficulty": "standard",
                    "taskClass": "general", "reason": "test", "warnings": [] }),
            ),
            ("GET", "/api/memories") => json_response(StatusCode::OK, json!({ "memories": [] })),
            ("PUT", "/api/memories") => json_response(StatusCode::OK, memory_json()),
            ("GET", "/api/prompts") | ("PUT", "/api/prompts") | ("DELETE", "/api/prompts") => {
                json_response(StatusCode::OK, prompt_json())
            }
            ("GET", "/api/projects") => json_response(
                StatusCode::OK,
                json!({ "global": "/home/test", "projects": [] }),
            ),
            ("GET", "/api/models") => json_response(StatusCode::OK, json!([model_json()])),
            ("GET", "/api/provider-usage") => json_response(StatusCode::OK, json!([])),
            ("DELETE", path) if path.starts_with("/api/grants/") => Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .expect("response"),
            ("GET", "/api/model-settings")
            | ("PUT", "/api/model-settings")
            | ("DELETE", "/api/model-settings") => {
                json_response(StatusCode::OK, model_settings_json())
            }
            ("GET", "/api/query") => {
                json_response(StatusCode::OK, json!({ "markdown": "# Context" }))
            }
            ("GET", "/api/map") => json_response(
                StatusCode::OK,
                json!({ "markdown": "# Map", "files": [], "omitted": { "outsideScope": 2, "gone": 1 } }),
            ),
            ("POST", "/api/map/init") => json_response(
                StatusCode::OK,
                json!({ "fileCount": 1, "symbolCount": 1, "partial": false, "changed": true, "elapsedMs": 1 }),
            ),
            ("POST", "/api/hooks/task") => json_response(StatusCode::OK, json!({})),
            ("POST", "/api/profiles") => json_response(StatusCode::OK, profile_json()),
            ("PUT", path) if path.starts_with("/api/profiles/") => {
                json_response(StatusCode::OK, profile_json())
            }
            ("DELETE", path) if path.starts_with("/api/profiles/") => Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .expect("response"),
            _ => json_response(
                StatusCode::NOT_FOUND,
                json!({ "error": "unhandled mock route" }),
            ),
        }
    }

    fn json_response(status: StatusCode, value: Value) -> Response<Body> {
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .expect("response")
    }

    fn sse_response(state: &MockState) -> Response<Body> {
        let connection = state.sse_connections.fetch_add(1, Ordering::SeqCst);
        let body = if connection == 0 {
            format!(
                "event: ready\ndata: {}\n\nevent: task\ndata: {}\n\n",
                json!({ "version": 1, "cursor": 0, "streamFloor": 0 }),
                json!({
                    "id": 1, "cursor": 1, "taskId": "task", "type": "started",
                    "kind": "lifecycle", "state": "running", "at": "2026-01-01T00:00:00.000Z",
                    "title": "Task", "summary": "started"
                }),
            )
        } else {
            format!(
                "event: ready\ndata: {}\n\nevent: cursor\ndata: {}\n\n",
                json!({ "version": 1, "cursor": 1, "streamFloor": 0 }),
                json!({ "cursor": 2 }),
            )
        };
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(Body::from(body))
            .expect("response")
    }

    fn profile_json() -> Value {
        json!({ "id": "profile", "label": "Profile", "provider": "claude", "model": "model",
            "enabled": true, "env": {}, "capabilities": [] })
    }

    fn task_json() -> Value {
        json!({ "id": "task", "kind": "delegated", "profileId": "profile", "model": "model",
            "prompt": "inspect", "cwd": "/home/test", "state": "completed",
            "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z",
            "output": "done", "scope": { "read": [], "write": [] }, "allowQuestions": true })
    }

    fn task_summary_json() -> Value {
        json!({ "id": "task", "profileId": "profile", "model": "model", "cwd": "/home/test",
            "state": "completed", "promptPreview": "inspect", "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z" })
    }

    fn action_json() -> Value {
        json!({ "id": "task", "state": "queued", "cursor": 1 })
    }

    fn event_json() -> Value {
        json!({ "id": 1, "taskId": "task", "source": "broker", "type": "started",
            "kind": "lifecycle", "phase": "info", "title": "Started",
            "createdAt": "2026-01-01T00:00:00.000Z" })
    }

    fn memory_json() -> Value {
        json!({ "cwd": "/home/test", "key": "decision", "value": "thin", "version": 1,
            "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z" })
    }

    fn prompt_json() -> Value {
        json!({ "cwd": "/home/test", "scope": "project", "written": false,
            "value": "default", "inherited": "default" })
    }

    fn model_json() -> Value {
        json!({ "id": "model", "label": "Model", "provider": "claude", "profileId": "profile",
            "source": "configured" })
    }

    fn model_settings_json() -> Value {
        json!({ "cwd": "/home/test", "scope": "project", "revision": "revision", "workers": [] })
    }

    #[test]
    fn parses_crlf_sse_frames() {
        let mut buffer = b"event: cursor\r\ndata: {\"cursor\": 2}\r\n\r\n".to_vec();
        let block = take_sse_block(&mut buffer).expect("SSE block");
        assert!(matches!(
            parse_sse_block(&block).expect("cursor frame"),
            Some(EventFrame::Cursor(CursorFrame { cursor: 2 }))
        ));
        assert!(buffer.is_empty());
    }

    #[tokio::test]
    async fn reconnects_from_the_last_cursor() {
        let (base_url, state, server) = start_mock(false).await;
        let client = LoopbackClient::new(base_url).expect("client");
        let mut stream = client
            .event_stream_with_options(
                EventStreamQuery::new(0).agents(true),
                EventStreamOptions {
                    initial_backoff: Duration::from_millis(1),
                    max_backoff: Duration::from_millis(1),
                },
            )
            .await
            .expect("stream");

        assert!(
            matches!(stream.next().await.expect("ready"), EventFrame::Ready(frame) if frame.cursor == 0)
        );
        assert!(
            matches!(stream.next().await.expect("task"), EventFrame::Task(pointer) if pointer.cursor == 1)
        );
        assert!(
            matches!(stream.next().await.expect("reconnected ready"), EventFrame::Ready(frame) if frame.cursor == 1)
        );
        assert!(
            matches!(stream.next().await.expect("reconnected cursor"), EventFrame::Cursor(frame) if frame.cursor == 2)
        );
        assert_eq!(stream.cursor(), 2);

        let requests = state.requests.lock().expect("request lock");
        assert!(
            requests
                .iter()
                .any(|request| request.contains("/api/events?after=1"))
        );
        server.abort();
    }

    #[tokio::test]
    async fn next_once_stops_when_the_stream_closes() {
        let (base_url, state, server) = start_mock(false).await;
        let client = LoopbackClient::new(base_url).expect("client");
        let mut stream = client
            .event_stream_with_options(
                EventStreamQuery::new(0).agents(true),
                EventStreamOptions {
                    initial_backoff: Duration::from_millis(1),
                    max_backoff: Duration::from_millis(1),
                },
            )
            .await
            .expect("stream");

        assert!(matches!(
            stream.next_once().await.expect("ready"),
            Some(EventFrame::Ready(_))
        ));
        assert!(matches!(
            stream.next_once().await.expect("task"),
            Some(EventFrame::Task(_))
        ));
        assert!(stream.next_once().await.expect("closed").is_none());
        assert_eq!(state.requests.lock().expect("request lock").len(), 1);
        server.abort();
    }

    #[tokio::test]
    async fn returns_server_errors_with_status_and_message() {
        let (base_url, _, server) = start_mock(true).await;
        let client = LoopbackClient::new(base_url).expect("client");
        let error = client.health().await.expect_err("health error");
        assert_eq!(error.status(), Some(StatusCode::CONFLICT));
        assert!(error.to_string().contains("stale revision"));
        server.abort();
    }

    #[tokio::test]
    async fn covers_all_loopback_endpoints() {
        let (base_url, _, server) = start_mock(false).await;
        let client = LoopbackClient::new(base_url).expect("client");
        let state_query = StateQuery::default().compact(true).limit(20);
        client.health().await.expect("health");
        client.get_state(&state_query).await.expect("state");
        client.get_summary(&state_query).await.expect("summary");
        client.get_task("task").await.expect("task");
        client.get_task_turns("task").await.expect("task turns");
        client
            .get_task_events("task", &TaskEventsQuery::default())
            .await
            .expect("task events");
        client.get_agent("agent").await.expect("agent");
        client.get_agent_turns("agent").await.expect("agent turns");
        client
            .get_agent_events("agent", &TaskEventsQuery::default())
            .await
            .expect("agent events");

        let dispatch = DispatchRequest::new("inspect", "/home/test", "Inspect the fixture");
        client.dispatch(&dispatch).await.expect("dispatch");
        client.start_agent(&dispatch).await.expect("start agent");
        client
            .dispatch_agent_task("agent", &dispatch)
            .await
            .expect("agent task");
        client.archive_task("task", true).await.expect("archive");
        client
            .cancel_task("task", Some("stop now"))
            .await
            .expect("cancel");
        client
            .resume_task("task", &ResumeRequest::default())
            .await
            .expect("resume");
        client
            .reply_task("task", &ReplyRequest::new("answer"))
            .await
            .expect("reply");
        client.remove_follow_up("task", 0).await.expect("follow-up");
        client
            .steer_task("task", &SteerRequest::default())
            .await
            .expect("steer");
        client
            .steer_agent("agent", &SteerRequest::default())
            .await
            .expect("agent steer");
        client
            .handoff_task("task", &HandoffRequest::default())
            .await
            .expect("handoff");
        client
            .remove_worktree("task", true)
            .await
            .expect("worktree");
        client
            .complete_task("task", &CompletionRequest::default())
            .await
            .expect("complete");
        client
            .append_hook("task", &json!({ "source": "test" }))
            .await
            .expect("hook");
        client.stop_agent("agent").await.expect("stop agent");
        client.remove_agent("agent").await.expect("remove agent");

        client
            .consumer_inbox("consumer", Some("app"), Some(10))
            .await
            .expect("inbox");
        client.advance_cursor("consumer", 1).await.expect("cursor");
        client
            .routing_preview(&RoutingPreviewRequest {
                cwd: "/home/test".into(),
                prompt: "inspect".into(),
                difficulty: None,
                kind: None,
            })
            .await
            .expect("routing");
        client.memories("/home/test").await.expect("memories");
        client
            .put_memory(&MemoryWrite {
                cwd: "/home/test".into(),
                key: "decision".into(),
                value: "thin".into(),
                expected_version: None,
            })
            .await
            .expect("put memory");
        client.prompt(Some("/home/test")).await.expect("prompt");
        client
            .put_prompt(&PromptWrite {
                cwd: "/home/test".into(),
                written: true,
                value: "prompt".into(),
            })
            .await
            .expect("put prompt");
        client
            .delete_prompt(Some("/home/test"))
            .await
            .expect("delete prompt");
        client.projects().await.expect("projects");
        client.models(&ModelQuery::default()).await.expect("models");
        client.usage(&ModelQuery::default()).await.expect("usage");
        client.delete_grant("grant").await.expect("grant");
        client
            .model_settings(Some("/home/test"), false)
            .await
            .expect("model settings");
        client
            .put_model_settings(&ModelSettingsUpdate {
                cwd: "/home/test".into(),
                profile_id: "profile".into(),
                model_id: Some("model".into()),
                ..ModelSettingsUpdate::default()
            })
            .await
            .expect("put model settings");
        client
            .delete_model_settings(Some("/home/test"), Some("revision"))
            .await
            .expect("delete model settings");
        client
            .query(&QueryRequest::new("/home/test", "where is the task"))
            .await
            .expect("query");
        let map = client
            .map(&MapQuery::new("task").path("src").tier(MapTier::Full))
            .await
            .expect("map");
        assert_eq!(map.omitted.outside_scope, 2);
        assert_eq!(map.omitted.gone, 1);
        client
            .init_map(&MapInitRequest::new("/home/test"))
            .await
            .expect("map init");
        client
            .create_profile(&ProfileCreate {
                id: None,
                label: "Profile".into(),
                provider: Provider::Claude,
                model: Some("model".into()),
                enabled: Some(true),
                env: Some(BTreeMap::new()),
                capabilities: Some(Vec::new()),
                command: None,
            })
            .await
            .expect("create profile");
        client
            .update_profile("profile", &ProfilePatch::default())
            .await
            .expect("update profile");
        client
            .delete_profile("profile")
            .await
            .expect("delete profile");
        server.abort();
    }
}
