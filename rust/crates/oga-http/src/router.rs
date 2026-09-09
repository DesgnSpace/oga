//! The broker's HTTP state and route assembly.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use oga_domain::Staleness;
use oga_runner::ProviderRunner;
use oga_service::{ContinuationError, DispatchError, Dispatcher};
use oga_store::Store;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::{
    agents, consumers, context, health, hooks, profiles, routing, settings, sse, state, tasks,
    usage,
};

/// Shared state for every HTTP handler.
#[derive(Clone)]
pub struct HttpState {
    pub store: Arc<Store>,
    pub events: Arc<sse::EventFanout>,
    pub dispatcher: Arc<Dispatcher>,
    pub build: String,
    pub staleness: Staleness,
}

/// Name used by applications that construct the broker router.
pub type AppState = HttpState;

impl HttpState {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            events: Arc::new(sse::EventFanout::new(store.clone())),
            dispatcher: Arc::new(Dispatcher::new(store.clone(), ProviderRunner::default())),
            store,
            build: "dev".into(),
            staleness: Staleness::default(),
        }
    }

    pub fn with_dispatcher(mut self, dispatcher: Arc<Dispatcher>) -> Self {
        self.dispatcher = dispatcher;
        self
    }

    pub fn with_build(mut self, build: impl Into<String>) -> Self {
        self.build = build.into();
        self
    }

    pub fn with_staleness(mut self, staleness: Staleness) -> Self {
        self.staleness = staleness;
        self
    }
}

/// A route error that preserves the broker's JSON `{error}` response shape.
#[derive(Debug, Clone)]
pub struct HttpError {
    pub status: StatusCode,
    pub message: String,
}

impl HttpError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: message.into(),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> axum::response::Response {
        (self.status, axum::Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<oga_store::StoreError> for HttpError {
    fn from(error: oga_store::StoreError) -> Self {
        Self::bad_request(error.to_string())
    }
}

/// Build the broker's HTTP surface.
pub fn router(state: HttpState) -> Router {
    Router::new()
        .route("/health", get(health::get))
        .route("/api/state", get(state::get_state))
        .route("/api/activity", get(state::get_activity))
        .route("/api/events", get(sse::events))
        .route("/api/events/head", get(sse::head))
        .route(
            "/api/tasks/{id}",
            get(state::get_task)
                .patch(tasks::archive)
                .delete(tasks::cancel),
        )
        .route(
            "/api/tasks/{id}/view",
            post(state::mark_task_viewed).delete(state::unmark_task_viewed),
        )
        .route("/api/tasks/{id}/turns", get(state::get_task_turns))
        .route("/api/tasks/{id}/events", get(state::get_task_events))
        .route("/api/tasks/{id}/diff", get(state::get_task_diff))
        .route("/api/tasks", post(tasks::dispatch))
        .route("/api/tasks/{id}/resume", post(tasks::resume))
        .route("/api/tasks/{id}/reply", post(tasks::reply))
        .route(
            "/api/tasks/{id}/follow_ups/{index}",
            delete(tasks::remove_follow_up),
        )
        .route("/api/tasks/{id}/steer", post(tasks::steer))
        .route("/api/tasks/{id}/handoff", post(tasks::handoff))
        .route("/api/tasks/{id}/worktree", delete(tasks::remove_worktree))
        .route("/api/tasks/{id}/complete", post(tasks::complete))
        .route("/api/hooks/{id}", post(hooks::append))
        .route("/api/consumers/{id}/inbox", get(consumers::inbox))
        .route(
            "/api/consumers/{id}/cursor",
            post(consumers::advance_cursor),
        )
        .route("/api/routing/preview", post(routing::preview))
        .route("/api/agents", post(agents::dispatch))
        .route("/api/agents/{id}", get(agents::get).delete(agents::remove))
        .route("/api/agents/{id}/tasks", post(agents::dispatch_task))
        .route("/api/agents/{id}/stop", post(agents::stop))
        .route("/api/agents/{id}/steer", post(agents::steer))
        .route("/api/agents/{id}/turns", get(state::get_agent_turns))
        .route("/api/agents/{id}/events", get(state::get_agent_events))
        .route(
            "/api/profiles",
            get(profiles::list_not_found).post(profiles::create),
        )
        .route(
            "/api/profiles/{id}",
            put(profiles::update).delete(profiles::remove),
        )
        .route(
            "/api/memories",
            get(settings::get_memories).put(settings::put_memory),
        )
        .route(
            "/api/prompts",
            get(settings::get_prompt)
                .put(settings::put_prompt)
                .delete(settings::delete_prompt),
        )
        .route(
            "/api/caller-prompts",
            get(settings::get_caller_prompt)
                .put(settings::put_caller_prompt)
                .delete(settings::delete_caller_prompt),
        )
        .route("/api/projects", get(settings::get_projects))
        .route(
            "/api/cleanup",
            get(settings::get_cleanup).put(settings::put_cleanup),
        )
        .route(
            "/api/waiting",
            get(settings::get_waiting).put(settings::put_waiting),
        )
        .route("/api/cleanup/preview", get(settings::preview_cleanup))
        .route("/api/cleanup/run", post(settings::run_cleanup))
        .route(
            "/api/model-settings",
            get(settings::get_model_settings)
                .put(settings::put_model_settings)
                .delete(settings::delete_model_settings),
        )
        .route(
            "/api/grants/{id}",
            axum::routing::delete(settings::delete_grant),
        )
        .route("/api/models", get(settings::get_models))
        .route("/api/usage", get(usage::get_usage))
        .route("/api/provider-usage", get(settings::get_usage))
        .route("/api/query", get(context::get_query))
        .route("/api/query/init", axum::routing::post(context::init_index))
        .layer(middleware::from_fn(json_content_type))
        .with_state(state)
}

/// Run a handler's blocking work on the blocking pool instead of the async
/// executor. The runtime has one worker thread per core, so a handful of
/// handlers walking the filesystem or driving SQLite inline is enough to pin
/// every one of them and stall every other request, streams included.
pub(crate) async fn run_blocking<T, F>(work: F) -> Result<T, HttpError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, HttpError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| {
            Err(HttpError::internal(format!(
                "the broker could not finish this request: {error}"
            )))
        })
}

pub(crate) fn parse_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, HttpError> {
    serde_json::from_slice(body).map_err(|_| HttpError::bad_request("invalid JSON body"))
}

pub(crate) fn parse_optional_json<T: DeserializeOwned + Default>(
    body: &[u8],
) -> Result<T, HttpError> {
    if body.is_empty() {
        Ok(T::default())
    } else {
        parse_json(body)
    }
}

pub(crate) fn service_error(error: impl std::fmt::Display) -> HttpError {
    HttpError::bad_request(error.to_string())
}

pub(crate) fn latest_event_id(store: &Store, task_id: &str) -> Result<i64, HttpError> {
    store
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT COALESCE(MAX(id),0) FROM task_events WHERE task_id=?",
                [task_id],
                |row| row.get(0),
            )?)
        })
        .map_err(HttpError::from)
}

impl From<ContinuationError> for HttpError {
    fn from(error: ContinuationError) -> Self {
        HttpError::bad_request(error.message())
    }
}

impl From<DispatchError> for HttpError {
    fn from(error: DispatchError) -> Self {
        service_error(error)
    }
}

/// Explicit alias for callers that prefer the constructor-style name.
pub fn build_router(state: HttpState) -> Router {
    router(state)
}

async fn json_content_type(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value == "application/json")
    {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            "application/json;charset=utf-8"
                .parse()
                .expect("valid header"),
        );
    }
    response
}
