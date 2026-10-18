//! A task handed to another provider keeps the earlier provider's events in
//! its transcript. Each row reads as the provider that recorded it.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use oga_domain::{Profile, Provider, Task, TaskEvent, TaskKind, TaskScope, TaskState};
use oga_http::{HttpState, router};
use oga_store::Store;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

/// Trimmed from a real task: a Claude subagent launch, the handoff to
/// OpenCode 2, and OpenCode 2's own subagent launch.
const RUNS: &str = include_str!("fixtures/handoff_claude_to_opencode2.json");

struct Handed {
    _directory: TempDir,
    store: Arc<Store>,
    router: Router,
    /// Event ids of the Claude run, the handoff row and the OpenCode 2 run.
    claude: Vec<i64>,
    handoff: i64,
    opencode2: Vec<i64>,
}

fn profile(id: &str, provider: Provider) -> Profile {
    Profile {
        id: id.into(),
        label: id.into(),
        provider,
        default_model: "m".into(),
        enabled: true,
        env: BTreeMap::new(),
        capabilities: Vec::new(),
        command: None,
    }
}

fn append(store: &Store, kind: &str, state: TaskState, payload: Value, turn: Option<i64>) -> i64 {
    store
        .repositories()
        .events()
        .append(&TaskEvent {
            id: 0,
            task_id: "task".into(),
            kind: kind.into(),
            state,
            payload: serde_json::from_value(payload).expect("payload is an object"),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            turn_id: turn,
        })
        .expect("event insert")
}

fn append_run(store: &Store, runs: &Value, key: &str) -> Vec<i64> {
    runs[key]
        .as_array()
        .expect("run")
        .iter()
        .map(|row| {
            append(
                store,
                row["type"].as_str().expect("type"),
                serde_json::from_value(row["state"].clone()).expect("state"),
                row["payload"].clone(),
                None,
            )
        })
        .collect()
}

impl Handed {
    /// The task now runs on `current`. `claude_profile` is false when the
    /// profile the task left has since been removed.
    fn new(current: &str, claude_profile: bool) -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        let profiles = store.repositories().profiles();
        if claude_profile {
            profiles
                .insert(
                    &profile("claude", Provider::Claude),
                    "2026-01-01T00:00:00.000Z",
                )
                .expect("profile");
        }
        profiles
            .insert(
                &profile("opencode2", Provider::OpenCode2),
                "2026-01-01T00:00:00.000Z",
            )
            .expect("profile");
        store
            .repositories()
            .tasks()
            .insert(&Task {
                id: "task".into(),
                kind: Some(TaskKind::Delegated),
                profile_id: current.into(),
                model: "m".into(),
                prompt: "audit".into(),
                cwd: directory.path().display().to_string(),
                state: TaskState::Completed,
                created_at: "2026-01-01T00:00:00.000Z".into(),
                updated_at: "2026-01-01T00:01:00.000Z".into(),
                scope: TaskScope::default(),
                ..Task::default()
            })
            .expect("task insert");
        let runs: Value = serde_json::from_str(RUNS).expect("fixture");
        let claude = append_run(&store, &runs, "claude");
        let handoff = append_run(&store, &runs, "handoff")[0];
        let opencode2 = append_run(&store, &runs, "opencode2");
        Self {
            _directory: directory,
            router: router(HttpState::new(store.clone())),
            store,
            claude,
            handoff,
            opencode2,
        }
    }

    async fn rows(&self, query: &str) -> Vec<Value> {
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/tasks/task/events?{query}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let value: Value = serde_json::from_slice(&body).expect("JSON");
        match value {
            Value::Array(rows) => rows,
            other => other["events"].as_array().expect("event page").clone(),
        }
    }
}

fn row(rows: &[Value], id: i64) -> &Value {
    rows.iter()
        .find(|row| row["id"] == id)
        .expect("row present")
}

/// Claude's `Task` call is a subagent. Read as OpenCode 2 it shows as plain
/// thinking, with no subagent behind it.
fn assert_claude_subagent(row: &Value) {
    assert_eq!(row["source"], "claude");
    assert_eq!(row["title"], "Subagent");
    assert_eq!(row["kind"], "tool");
    assert_eq!(row["subagents"][0]["role"], "launch");
}

#[tokio::test]
async fn the_whole_transcript_reads_each_run_as_its_own_provider() {
    let task = Handed::new("opencode2", true);
    let rows = task.rows("").await;
    assert_claude_subagent(row(&rows, task.claude[0]));
    assert_claude_subagent(row(&rows, task.claude[1]));
    let launch = row(&rows, task.opencode2[0]);
    assert_eq!(launch["source"], "opencode-2");
    assert_eq!(launch["title"], "Subagent");
    assert_eq!(launch["subagents"][0]["role"], "launch");
}

#[tokio::test]
async fn an_earlier_page_that_ends_at_the_handoff_still_reads_as_the_earlier_provider() {
    let task = Handed::new("opencode2", true);
    let rows = task.rows(&format!("before={}&last=10", task.handoff)).await;
    assert_eq!(rows.len(), task.claude.len());
    assert_claude_subagent(row(&rows, task.claude[0]));
}

#[tokio::test]
async fn a_page_that_starts_after_the_handoff_reads_as_the_new_provider() {
    let task = Handed::new("opencode2", true);
    let rows = task.rows(&format!("after={}", task.handoff)).await;
    let launch = row(&rows, task.opencode2[0]);
    assert_eq!(launch["source"], "opencode-2");
    assert_eq!(launch["title"], "Subagent");
}

#[tokio::test]
async fn a_page_between_two_handoffs_reads_as_the_provider_in_between() {
    // The task went back to Claude, so the middle run is not the current one.
    let task = Handed::new("claude", true);
    let back = append(
        &task.store,
        "handed_off",
        TaskState::Queued,
        json!({ "fromProfile": "opencode2", "toProfile": "claude" }),
        None,
    );
    let rows = task.rows(&format!("before={back}&last=3")).await;
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| row["source"] == "opencode-2"));
    assert_eq!(row(&rows, task.opencode2[0])["title"], "Subagent");
}

#[tokio::test]
async fn a_removed_profile_still_reads_its_run_by_the_shape_of_what_it_recorded() {
    let task = Handed::new("opencode2", false);
    let rows = task.rows("").await;
    assert_claude_subagent(row(&rows, task.claude[0]));
    assert_eq!(row(&rows, task.opencode2[0])["source"], "opencode-2");
}

#[tokio::test]
async fn a_call_never_pairs_with_one_the_other_provider_started() {
    let task = Handed::new("opencode2", true);
    // An update that names only what changed reads as the call it patched.
    // After the handoff the same id belongs to another provider's call.
    let first = append(
        &task.store,
        "agent.tool_call",
        TaskState::Running,
        json!({ "sessionUpdate": "tool_call", "toolCallId": "reused", "title": "Read src/a.ts", "kind": "read" }),
        None,
    );
    let _ = append(
        &task.store,
        "handed_off",
        TaskState::Queued,
        json!({ "fromProfile": "claude", "toProfile": "opencode2" }),
        None,
    );
    let bare = append(
        &task.store,
        "agent.tool_call_update",
        TaskState::Running,
        json!({ "sessionUpdate": "tool_call_update", "toolCallId": "reused", "status": "completed" }),
        None,
    );
    let rows = task.rows("").await;
    assert_ne!(row(&rows, bare)["title"], row(&rows, first)["title"]);
}

#[tokio::test]
async fn a_client_replaying_from_before_the_handoff_gets_the_earlier_providers_kinds() {
    let task = Handed::new("opencode2", true);
    let response = task
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/api/events?after={}&task=task",
                    task.claude[0] - 1
                ))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let mut body = response.into_body();
    let mut text = String::new();
    while !text.contains(&format!("\"id\":{}", task.opencode2[0])) {
        let frame = body.frame().await.expect("frame").expect("body frame");
        if let Ok(data) = frame.into_data() {
            text.push_str(&String::from_utf8_lossy(&data));
        }
    }
    let pointer = |id: i64| -> Value {
        text.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|data| serde_json::from_str::<Value>(data).ok())
            .find(|value| value["id"] == id)
            .expect("pointer for event")
    };
    assert_eq!(pointer(task.claude[0])["kind"], "tool");
    assert_eq!(pointer(task.opencode2[0])["kind"], "tool");
}
