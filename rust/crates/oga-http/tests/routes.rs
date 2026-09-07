use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use oga_domain::{Profile, Provider, Task, TaskEvent, TaskKind, TaskScope, TaskState};
use oga_http::{HttpState, router};
use oga_runner::ProviderRunner;
use oga_service::{DispatchRequest, Dispatcher};
use oga_store::Store;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

/// A model is unavailable until someone turns it on. These routes are about
/// what happens once one is, so the fixtures switch theirs on everywhere.
fn switch_on(store: &Store, profile_id: &str, model: &str) {
    store
        .repositories()
        .settings()
        .put(
            &oga_config::canonical_cwd(oga_config::global_cwd())
                .display()
                .to_string(),
            oga_config::MODEL_SETTINGS_KEY,
            &json!({ "profiles": { profile_id: { "modelEnabled": { model: true } } } }).to_string(),
            "2026-01-01T00:00:00.000Z",
        )
        .expect("model settings");
}

struct Fixture {
    _directory: TempDir,
    cwd: String,
    store: Arc<Store>,
    router: Router,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("oga.db");
        let store = Arc::new(Store::open_writable(database).expect("store"));
        let cwd = directory.path().display().to_string();
        let profile = Profile {
            id: "profile".into(),
            label: "Test Profile".into(),
            provider: Provider::Antigravity,
            default_model: "fake".into(),
            enabled: true,
            env: BTreeMap::from([
                (String::from("SECRET_TOKEN"), String::from("secret")),
                (String::from("PATH"), String::from("/missing")),
            ]),
            capabilities: vec!["review".into()],
            command: None,
        };
        store
            .repositories()
            .profiles()
            .insert(&profile, "2026-01-01T00:00:00.000Z")
            .expect("profile insert");
        store
            .repositories()
            .tasks()
            .insert(&Task {
                id: "task".into(),
                kind: Some(TaskKind::Delegated),
                profile_id: profile.id,
                model: "fake".into(),
                prompt: "inspect the fixture".into(),
                cwd: cwd.clone(),
                state: TaskState::Completed,
                created_at: "2026-01-01T00:00:00.000Z".into(),
                updated_at: "2026-01-01T00:01:00.000Z".into(),
                output: "finished".into(),
                scope: TaskScope {
                    read: vec!["**".into()],
                    write: vec!["**".into()],
                },
                allow_questions: true,
                tldr: Some("fixture task".into()),
                title: Some("Fixture task".into()),
                session_id: Some("session".into()),
                ..Task::default()
            })
            .expect("task insert");
        store
            .repositories()
            .events()
            .append(&TaskEvent {
                id: 0,
                task_id: "task".into(),
                kind: "created".into(),
                state: TaskState::Completed,
                payload: BTreeMap::from([(String::from("title"), json!("Task queued"))]),
                created_at: "2026-01-01T00:00:30.000Z".into(),
                turn_id: None,
            })
            .expect("event insert");
        store
            .repositories()
            .turns()
            .start("task", 1, "2026-01-01T00:00:00.000Z")
            .expect("turn insert");
        switch_on(&store, "profile", "fake");
        Self {
            _directory: directory,
            cwd,
            store: store.clone(),
            router: router(HttpState::new(store)),
        }
    }

    fn insert_task(&self, task: &Task) {
        self.store
            .repositories()
            .tasks()
            .insert(task)
            .expect("task insert");
    }

    fn canonical_cwd(&self) -> String {
        std::fs::canonicalize(&self.cwd)
            .expect("canonicalize fixture cwd")
            .display()
            .to_string()
    }

    fn write_source(&self, path: &str, body: &str) {
        let absolute = std::path::Path::new(&self.cwd).join(path);
        std::fs::create_dir_all(absolute.parent().expect("source parent"))
            .expect("source directory is creatable");
        std::fs::write(absolute, body).expect("source fixture writes");
    }
}

async fn request(
    router: &Router,
    method: Method,
    uri: &str,
    body: Body,
) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(body)
                .expect("request"),
        )
        .await
        .expect("response")
}

async fn json_response(response: axum::response::Response) -> (StatusCode, Value) {
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json;charset=utf-8")
    );
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&body).expect("JSON body"))
}

async fn sse_frames(response: axum::response::Response, count: usize) -> Vec<(String, Value)> {
    let mut body = response.into_body();
    let mut buffer = String::new();
    let mut frames = Vec::new();
    while frames.len() < count {
        let Some(frame) = body.frame().await else {
            break;
        };
        let frame = frame.expect("SSE frame");
        let Ok(data) = frame.into_data() else {
            continue;
        };
        buffer.push_str(&String::from_utf8_lossy(&data));
        while let Some(end) = buffer.find("\n\n") {
            let block = buffer[..end].to_owned();
            buffer.drain(..end + 2);
            let event = block
                .lines()
                .find_map(|line| line.strip_prefix("event: "))
                .expect("SSE event")
                .to_owned();
            let data = block
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .map(|value| serde_json::from_str(value).expect("SSE JSON"))
                .expect("SSE data");
            frames.push((event, data));
            if frames.len() == count {
                break;
            }
        }
    }
    frames
}

#[tokio::test]
async fn read_routes() {
    let fixture = Fixture::new();

    let (status, health) =
        json_response(request(&fixture.router, Method::GET, "/health", Body::empty()).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        health,
        json!({
            "status": "ok",
            "version": "0.6.0",
            "mcpContractVersion": 32,
            "build": "dev",
            "stale": false
        })
    );

    let (status, task) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/tasks/task",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(task["id"], "task");
    assert_eq!(task["kind"], "delegated");
    assert_eq!(task["state"], "completed");
    assert_eq!(task["output"], "finished");
    assert!(task.get("queuedFollowUps").is_none());

    let (status, turns) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/tasks/task/turns",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(turns["turns"].as_array().expect("turns").len(), 1);

    let (status, events) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/tasks/task/events",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events.as_array().expect("event list").len(), 1);
    assert_eq!(events[0]["type"], "created");
    assert_eq!(events[0]["title"], "Task queued");

    let (status, state) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/state?view=summary&compact=1",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["profiles"][0]["env"]["SECRET_TOKEN"], "••••••••");
    assert_eq!(state["tasks"][0]["promptPreview"], "inspect the fixture");
    assert_eq!(state["tasksHasMore"], false);
    assert!(state.get("profileFailures").is_none());

    let (status, missing) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/tasks/does-not-exist",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing, json!({ "error": "unknown task" }));
}

#[tokio::test]
async fn profile_routes_mutate_store_profiles() {
    let fixture = Fixture::new();
    let (status, created) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/profiles",
            Body::from(
                json!({
                    "label": "New worker",
                    "provider": "codex",
                    "model": "",
                    "env": {
                        "API_TOKEN": "created",
                        "PATH": "$HOME/bin"
                    },
                    "capabilities": ["build"]
                })
                .to_string(),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["id"], "new-worker");
    assert_eq!(created["model"], "gpt-5");
    assert_eq!(created["env"]["API_TOKEN"], "••••••••");
    assert_eq!(created["env"]["PATH"], "$HOME/bin");

    let (status, updated) = json_response(
        request(
            &fixture.router,
            Method::PUT,
            "/api/profiles/new-worker",
            Body::from(
                json!({
                    "enabled": false,
                    "label": "Updated worker",
                    "model": "",
                    "env": {
                        "API_TOKEN": "••••••••",
                        "PATH": "/tmp/bin"
                    }
                })
                .to_string(),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["label"], "Updated worker");
    assert_eq!(updated["model"], "gpt-5");
    assert_eq!(updated["env"]["API_TOKEN"], "••••••••");
    assert_eq!(updated["env"]["PATH"], "/tmp/bin");

    let response = request(
        &fixture.router,
        Method::DELETE,
        "/api/profiles/new-worker",
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        fixture
            .store
            .repositories()
            .profiles()
            .get("new-worker")
            .unwrap()
            .is_none()
    );

    let (status, recreated) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/profiles",
            Body::from(
                json!({
                    "id": "new-worker",
                    "label": "New worker again",
                    "provider": "codex"
                })
                .to_string(),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_ne!(recreated["id"], "new-worker");
    assert!(recreated["id"].as_str().unwrap().starts_with("new-worker-"));
}

#[tokio::test]
async fn profile_provider_change_resets_an_omitted_model() {
    let fixture = Fixture::new();
    let (status, updated) = json_response(
        request(
            &fixture.router,
            Method::PUT,
            "/api/profiles/profile",
            Body::from(json!({ "provider": "claude" }).to_string()),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["model"], "sonnet");
}

#[tokio::test]
async fn sse_routes_replay_and_filter_events() {
    let fixture = Fixture::new();
    let first = fixture
        .store
        .repositories()
        .events()
        .append(&TaskEvent {
            id: 0,
            task_id: "task".into(),
            kind: "agent.message".into(),
            state: TaskState::Completed,
            payload: BTreeMap::from([(String::from("kind"), json!("message"))]),
            created_at: "2026-01-01T00:01:00.000Z".into(),
            turn_id: None,
        })
        .expect("message event");
    let second = fixture
        .store
        .repositories()
        .events()
        .append(&TaskEvent {
            id: 0,
            task_id: "task".into(),
            kind: "failed".into(),
            state: TaskState::Failed,
            payload: BTreeMap::from([
                (String::from("kind"), json!("error")),
                (String::from("detail"), json!("private failure")),
            ]),
            created_at: "2026-01-01T00:02:00.000Z".into(),
            turn_id: None,
        })
        .expect("error event");

    let response = request(
        &fixture.router,
        Method::GET,
        &format!("/api/events?after={first}&task=task&kinds=error"),
        Body::empty(),
    )
    .await;
    if response.status() != StatusCode::OK {
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("error body")
            .to_bytes();
        panic!(
            "SSE request failed with {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    assert!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"))
    );
    let frames = sse_frames(response, 3).await;
    assert_eq!(frames[0].0, "ready");
    assert_eq!(frames[0].1["cursor"], first);
    assert_eq!(frames[1].0, "task");
    assert_eq!(frames[1].1["id"], second);
    assert_eq!(frames[1].1["taskId"], "task");
    assert_eq!(frames[1].1["kind"], "error");
    assert!(frames[1].1.get("payload").is_none());
    assert_eq!(
        frames[2],
        (String::from("cursor"), json!({ "cursor": second }))
    );
}

#[tokio::test]
async fn sse_route_reports_stale_cursors_and_invalid_queries() {
    let fixture = Fixture::new();
    let saved_cursor = fixture
        .store
        .repositories()
        .events()
        .append(&TaskEvent {
            id: 0,
            task_id: "task".into(),
            kind: "created".into(),
            state: TaskState::Completed,
            payload: BTreeMap::new(),
            created_at: "2026-01-01T00:01:00.000Z".into(),
            turn_id: None,
        })
        .expect("saved event");
    let survivor = Task {
        id: "survivor".into(),
        kind: Some(TaskKind::Delegated),
        profile_id: "profile".into(),
        model: "fake".into(),
        prompt: "survive".into(),
        cwd: fixture.cwd.clone(),
        state: TaskState::Running,
        created_at: "2026-01-01T00:00:00.000Z".into(),
        updated_at: "2026-01-01T00:00:00.000Z".into(),
        ..Task::default()
    };
    fixture.insert_task(&survivor);
    let floor = fixture
        .store
        .repositories()
        .events()
        .append(&TaskEvent {
            id: 0,
            task_id: survivor.id.clone(),
            kind: "started".into(),
            state: TaskState::Running,
            payload: BTreeMap::new(),
            created_at: "2026-01-01T00:02:00.000Z".into(),
            turn_id: None,
        })
        .expect("survivor event");

    let response = request(
        &fixture.router,
        Method::GET,
        &format!("/api/events?after={saved_cursor}&task=survivor"),
        Body::empty(),
    )
    .await;
    let frames = sse_frames(response, 1).await;
    assert_eq!(frames[0].1["stale"], true);
    assert!(frames[0].1["cursor"].as_i64().expect("head cursor") >= floor);
    assert_eq!(frames[0].1["streamFloor"], floor);

    let (status, error) =
        json_response(request(&fixture.router, Method::GET, "/api/events", Body::empty()).await)
            .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"], "after must be a non-negative integer");
}

#[tokio::test]
async fn sse_task_event_long_poll_wakes_on_a_new_event() {
    let fixture = Fixture::new();
    let task = Task {
        id: "running".into(),
        kind: Some(TaskKind::Delegated),
        profile_id: "profile".into(),
        model: "fake".into(),
        prompt: "wait for progress".into(),
        cwd: fixture.cwd.clone(),
        state: TaskState::Running,
        created_at: "2026-01-01T00:00:00.000Z".into(),
        updated_at: "2026-01-01T00:00:00.000Z".into(),
        ..Task::default()
    };
    fixture.insert_task(&task);
    let initial = fixture
        .store
        .repositories()
        .events()
        .append(&TaskEvent {
            id: 0,
            task_id: task.id.clone(),
            kind: "started".into(),
            state: TaskState::Running,
            payload: BTreeMap::new(),
            created_at: "2026-01-01T00:00:01.000Z".into(),
            turn_id: None,
        })
        .expect("initial event");
    let store = fixture.store.clone();
    let append = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        store
            .repositories()
            .events()
            .append(&TaskEvent {
                id: 0,
                task_id: "running".into(),
                kind: "progress".into(),
                state: TaskState::Running,
                payload: BTreeMap::new(),
                created_at: "2026-01-01T00:00:02.000Z".into(),
                turn_id: None,
            })
            .expect("progress event")
    });

    let (status, body) = json_response(
        request(
            &fixture.router,
            Method::GET,
            &format!("/api/tasks/running/events?after={initial}&waitMs=500"),
            Body::empty(),
        )
        .await,
    )
    .await;
    append.await.expect("append task");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["events"].as_array().expect("events").len(), 1);
    assert_eq!(body["cursor"], initial + 1);
}

#[tokio::test]
async fn sse_route_receives_antigravity_events_before_worker_settles() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    store
        .repositories()
        .profiles()
        .insert(
            &Profile {
                id: "streaming".into(),
                label: "Streaming".into(),
                 provider: Provider::Antigravity,
                default_model: "fake".into(),
                enabled: true,
                env: BTreeMap::new(),
                capabilities: vec![],
                command: Some(vec![
                    "sh".into(),
                    "-c".into(),
                    r#"printf '%s\n' '{"type":"message","part":{"text":"live"}}'; sleep 0.25; printf '%s\n' '{"type":"event","text":"OGA_RESULT: completed"}'"#.into(),
                ]),
            },
            "2026-01-01T00:00:00.000Z",
        )
        .expect("profile");
    switch_on(&store, "streaming", "fake");
    let dispatcher = Arc::new(Dispatcher::new(store.clone(), ProviderRunner::default()));
    let app = router(HttpState::new(store.clone()).with_dispatcher(dispatcher.clone()));
    let task = dispatcher
        .dispatch(DispatchRequest::new(
            "streaming",
            "stream one event",
            directory.path(),
        ))
        .await
        .expect("dispatch")
        .task;
    let cursor = 'cursor: {
        for _ in 0..100 {
            if let Some(event) = store
                .repositories()
                .events()
                .list(&task.id)
                .expect("initial events")
                .into_iter()
                .find(|event| event.kind == "worker_spawned")
            {
                break 'cursor event.id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("worker did not spawn");
    };

    let response = request(
        &app,
        Method::GET,
        &format!("/api/events?after={cursor}&task={}", task.id),
        Body::empty(),
    )
    .await;
    let frames = sse_frames(response, 2).await;

    assert_eq!(frames[0].0, "ready");
    assert_eq!(frames[1].0, "task");
    assert_eq!(frames[1].1["type"], "agent.message");
    assert_eq!(
        dispatcher.task(&task.id).expect("live task").state,
        TaskState::Running
    );

    for _ in 0..100 {
        if dispatcher.task(&task.id).expect("task").state.settled() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        dispatcher.task(&task.id).expect("settled task").state,
        TaskState::Completed
    );
    let events = store
        .repositories()
        .events()
        .list(&task.id)
        .expect("events");
    let live_event = events
        .iter()
        .find(|event| event.kind == "agent.message")
        .expect("live event");
    let completed = events
        .iter()
        .find(|event| event.kind == "completed")
        .expect("completed event");
    assert!(live_event.created_at < completed.created_at);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "agent.message")
            .count(),
        1
    );
}

#[tokio::test]
async fn task_routes() {
    let fixture = Fixture::new();

    let (status, missing_tldr) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/tasks",
            Body::from(json!({ "prompt": "inspect", "cwd": fixture.cwd.clone() }).to_string()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        missing_tldr["error"],
        "tldr: Invalid input: expected string, received undefined"
    );

    let (status, timeout) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/tasks",
            Body::from(
                json!({
                    "prompt": "inspect",
                    "cwd": fixture.cwd.clone(),
                    "tldr": "inspect the fixture",
                    "timeoutMs": 86_400_001
                })
                .to_string(),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(timeout["error"], "timeoutMs must be between 1 and 86400000");

    let (status, preview) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/routing/preview",
            Body::from(
                json!({ "cwd": fixture.cwd.clone(), "prompt": "inspect the fixture" }).to_string(),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["profileId"], "profile");
    assert_eq!(preview["model"], "fake");

    let (status, invalid_branch_delete) = json_response(
        request(
            &fixture.router,
            Method::PATCH,
            "/api/tasks/task",
            Body::from(json!({ "archived": true, "deleteBranch": true }).to_string()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        invalid_branch_delete["error"],
        "Error: deleteBranch only applies to worktree tasks"
    );

    let (status, archived) = json_response(
        request(
            &fixture.router,
            Method::PATCH,
            "/api/tasks/task",
            Body::from(json!({ "archived": true }).to_string()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived["id"], "task");
    assert_eq!(archived["state"], "completed");
    assert!(archived["archivedAt"].is_string());

    let (status, hook) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/hooks/task",
            Body::from(json!({ "source": "test" }).to_string()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hook, json!({}));

    let (status, invalid_hook) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/hooks/task",
            Body::from("[]"),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid_hook["error"], "hook payload must be an object");
}

#[tokio::test]
async fn agent_routes() {
    let fixture = Fixture::new();
    fixture.insert_task(&Task {
        id: "agent".into(),
        kind: Some(TaskKind::Orchestrator),
        profile_id: "profile".into(),
        model: "fake".into(),
        prompt: "orchestrate the fixture".into(),
        cwd: fixture.cwd.clone(),
        state: TaskState::Completed,
        created_at: "2026-01-01T00:00:00.000Z".into(),
        updated_at: "2026-01-01T00:01:00.000Z".into(),
        ..Task::default()
    });

    let (status, agent) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/agents/agent",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent["kind"], "orchestrator");

    let (status, turns) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/agents/agent/turns",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(turns, json!({ "turns": [] }));

    let (status, events) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/agents/agent/events",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events, json!([]));

    let (status, settled_stop) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/agents/agent/stop",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        settled_stop["error"],
        "Error: task cannot be cancelled from state completed: agent"
    );

    fixture.insert_task(&Task {
        id: "live-agent".into(),
        kind: Some(TaskKind::Orchestrator),
        profile_id: "profile".into(),
        model: "fake".into(),
        prompt: "orchestrate the fixture".into(),
        cwd: fixture.cwd.clone(),
        state: TaskState::Running,
        created_at: "2026-01-01T00:00:00.000Z".into(),
        updated_at: "2026-01-01T00:01:00.000Z".into(),
        ..Task::default()
    });

    let (status, stopped) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/agents/live-agent/stop",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stopped, json!({ "stopped": true }));

    let (status, removed) = json_response(
        request(
            &fixture.router,
            Method::DELETE,
            "/api/agents/agent",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed, json!({ "removed": true }));

    let (status, missing_stop) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/agents/agent/stop",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(missing_stop["error"], "Error: unknown orchestrator: agent");
}

#[tokio::test]
async fn consumer_routes() {
    let fixture = Fixture::new();

    let (status, cursor) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/consumers/client/cursor",
            Body::from(json!({ "cursor": 0 }).to_string()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cursor["consumerId"], "client");
    assert_eq!(cursor["cursor"], 0);

    let (status, _) = json_response(
        request(
            &fixture.router,
            Method::POST,
            "/api/hooks/task",
            Body::from(json!({ "source": "consumer-test" }).to_string()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, inbox) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/consumers/client/inbox?channel=app",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "inbox response: {inbox}");
    let deliveries = inbox["deliveries"].as_array().expect("deliveries");
    assert!(deliveries.len() >= 2);
    let delivery = deliveries.last().expect("latest delivery");
    assert_eq!(delivery["status"], "sent");
    let event_id = delivery["eventId"].as_i64().expect("event id");

    let (status, seen) = json_response(
        request(
            &fixture.router,
            Method::POST,
            &format!("/api/consumers/client/deliveries/{event_id}/seen?channel=app"),
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen["status"], "seen");
    assert!(seen.get("event").is_none());
}

#[tokio::test]
async fn settings_routes() {
    let fixture = Fixture::new();
    let cwd = &fixture.cwd;

    let (status, memory) = json_response(
        request(
            &fixture.router,
            Method::PUT,
            "/api/memories",
            Body::from(
                json!({
                    "cwd": cwd,
                    "key": "decision",
                    "value": "keep the route thin"
                })
                .to_string(),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(memory["key"], "decision");
    assert_eq!(memory["version"], 1);

    let (status, memories) = json_response(
        request(
            &fixture.router,
            Method::GET,
            &format!("/api/memories?cwd={cwd}"),
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(memories["memories"].as_array().expect("memories").len(), 1);

    let (status, prompt) = json_response(
        request(
            &fixture.router,
            Method::GET,
            &format!("/api/prompts?cwd={cwd}"),
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prompt["written"], false);
    assert_eq!(prompt["scope"], "project");

    let (status, prompt) = json_response(
        request(
            &fixture.router,
            Method::PUT,
            "/api/prompts",
            Body::from(
                json!({ "cwd": cwd, "written": true, "value": "Use the fixture." }).to_string(),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prompt["value"], "{{brief}}\n\nUse the fixture.");

    let (status, models) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/models?profile=profile",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(models.as_array().expect("models").len(), 1);
    assert_eq!(models[0]["id"], "fake");

    let (status, usage) = json_response(
        request(
            &fixture.router,
            Method::GET,
            "/api/provider-usage?profile=profile",
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(usage.as_array().expect("usage").len(), 1);
    assert_eq!(usage[0]["profile"], "profile");

    let (status, settings) = json_response(
        request(
            &fixture.router,
            Method::GET,
            &format!("/api/model-settings?cwd={cwd}"),
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["workers"][0]["id"], "profile");
    assert_eq!(settings["workers"][0]["enabled"], true);
    assert_eq!(settings["workers"][0]["models"][0]["enabled"], true);
    assert_eq!(
        settings["workers"][0]["models"][0]["inheritedEnabled"],
        true
    );

    let (status, settings) = json_response(
        request(
            &fixture.router,
            Method::PUT,
            "/api/model-settings",
            Body::from(
                json!({ "cwd": cwd, "profileId": "profile", "modelId": "fake", "enabled": false })
                    .to_string(),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["workers"][0]["enabled"], true);
    assert_eq!(settings["workers"][0]["models"][0]["enabled"], false);
    assert_eq!(
        settings["workers"][0]["models"][0]["inheritedEnabled"],
        true
    );

    let (status, projects) =
        json_response(request(&fixture.router, Method::GET, "/api/projects", Body::empty()).await)
            .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        projects["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|project| project == cwd)
    );
}

#[tokio::test]
async fn model_settings_reflect_yaml_enablement_overrides() {
    let fixture = Fixture::new();
    fixture.write_source(".oga.yaml", "models:\n  fake:\n    enabled: false\n");

    let (status, settings) = json_response(
        request(
            &fixture.router,
            Method::GET,
            &format!("/api/model-settings?cwd={}", fixture.cwd),
            Body::empty(),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["workers"][0]["models"][0]["enabled"], false);
    assert_eq!(
        settings["workers"][0]["models"][0]["inheritedEnabled"],
        true
    );
    assert_eq!(
        settings["workers"][0]["models"][0]["hasEnabledOverride"],
        true
    );
}

#[tokio::test]
async fn a_project_file_owns_its_worker_rules() {
    let fixture = Fixture::new();
    let cwd = fixture.canonical_cwd();
    fixture.write_source(
        ".oga.yaml",
        "version: 1\nworker:\n  prompt: |\n    1. Read first.\n",
    );

    fixture
        .store
        .repositories()
        .settings()
        .put(
            &cwd,
            "prompts",
            &json!({ "written": true, "value": "Saved instructions." }).to_string(),
            "2026-01-01T00:00:00Z",
        )
        .expect("saved instructions");

    let (status, prompt) = json_response(
        request(
            &fixture.router,
            Method::GET,
            &format!("/api/prompts?cwd={cwd}"),
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prompt["value"], "{{brief}}\n\n1. Read first.");
    assert_eq!(prompt["configPath"], format!("{cwd}/.oga.yaml"));

    let (status, refused) = json_response(
        request(
            &fixture.router,
            Method::PUT,
            "/api/prompts",
            Body::from(json!({ "cwd": cwd, "written": true, "value": "Elsewhere." }).to_string()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        refused["error"],
        format!("These instructions come from {cwd}/.oga.yaml. Edit them there.")
    );

    fixture.write_source(".oga.yaml", "version: 1\nworker:\n  tldr: true\n");
    let (status, invalid) = json_response(
        request(
            &fixture.router,
            Method::GET,
            &format!("/api/prompts?cwd={cwd}"),
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        invalid["error"]
            .as_str()
            .expect("error")
            .contains("at worker.tldr:"),
        "{invalid}"
    );
}

#[tokio::test]
async fn plain_language_lookup_indexes_on_first_use_and_follows_edits() {
    let fixture = Fixture::new();
    fixture.write_source(
        "src/auth.ts",
        "export function checkAuth(token: string): boolean { return !!token; }\n",
    );
    let cwd = fixture.canonical_cwd();

    let (status, body) = json_response(
        request(
            &fixture.router,
            Method::GET,
            &format!("/api/query?cwd={cwd}&q=where%20is%20checkAuth%20handled"),
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["candidates"][0]["path"], "src/auth.ts");
    assert_eq!(body["candidates"][0]["line"], 1);

    fixture.write_source(
        "src/auth.ts",
        "import { log } from './log';\n\nlog('auth');\n\nexport function checkAuth(token: string): boolean { return !!token; }\n",
    );
    let (status, body) = json_response(
        request(
            &fixture.router,
            Method::GET,
            &format!("/api/query?cwd={cwd}&q=where%20is%20checkAuth%20handled"),
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["candidates"][0]["line"], 5);
    assert!(
        body["markdown"]
            .as_str()
            .expect("markdown")
            .contains("src/auth.ts:5#checkAuth")
    );
}

#[tokio::test]
async fn plain_language_lookup_inside_a_worktree_answers_from_the_origin_index() {
    let fixture = Fixture::new();
    fixture.write_source(
        "src/auth.ts",
        "export function checkAuth(token: string): boolean { return !!token; }\n",
    );
    let origin = fixture.canonical_cwd();

    // The checkout holds no sources of its own, so an answer can only have come
    // from the origin's index.
    let checkout = tempfile::tempdir().expect("worktree directory");
    let checkout_cwd = std::fs::canonicalize(checkout.path())
        .expect("canonicalize worktree")
        .display()
        .to_string();
    fixture
        .store
        .repositories()
        .tasks()
        .insert(&Task {
            id: "worktree-task".into(),
            kind: Some(TaskKind::Delegated),
            profile_id: "profile".into(),
            model: "fake".into(),
            prompt: "work in a checkout".into(),
            cwd: checkout_cwd.clone(),
            state: TaskState::Running,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            ..Task::default()
        })
        .expect("worktree task insert");
    // The task repository does not carry the checkout columns, so set them here.
    fixture
        .store
        .transaction(|tx| {
            tx.execute(
                "UPDATE tasks SET origin_cwd=?,worktree_path=?,worktree_branch=? WHERE id=?",
                rusqlite::params![&origin, &checkout_cwd, "task/checkout", "worktree-task"],
            )?;
            Ok(())
        })
        .expect("worktree columns");

    for cwd in [checkout_cwd.clone(), format!("{checkout_cwd}/")] {
        let (status, body) = json_response(
            request(
                &fixture.router,
                Method::GET,
                &format!("/api/query?cwd={cwd}&q=where%20is%20checkAuth%20handled"),
                Body::empty(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "cwd {cwd}");
        assert_eq!(body["candidates"][0]["path"], "src/auth.ts");
    }
}

#[tokio::test]
async fn plain_language_lookup_in_an_unindexed_directory_says_so() {
    let fixture = Fixture::new();
    let empty = tempfile::tempdir().expect("empty directory");
    let cwd = std::fs::canonicalize(empty.path())
        .expect("canonicalize empty directory")
        .display()
        .to_string();

    let (status, body) = json_response(
        request(
            &fixture.router,
            Method::GET,
            &format!("/api/query?cwd={cwd}&q=where%20is%20checkAuth%20handled"),
            Body::empty(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .expect("error")
            .contains("is not indexed"),
        "unexpected error: {body}"
    );
}
