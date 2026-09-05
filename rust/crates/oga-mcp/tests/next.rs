//! What a task response tells the caller to do next, over the wire.

use std::sync::Arc;

use axum::body::Body;
use http_body_util::BodyExt;
use oga_domain::{Task, TaskState, TaskWorktree};
use oga_http::HttpState;
use oga_mcp::{McpServer, router};
use oga_store::Store;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

fn test_server() -> (TempDir, McpServer) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    store
        .repositories()
        .profiles()
        .insert(
            &oga_domain::Profile {
                id: "main".into(),
                label: "Main".into(),
                provider: oga_domain::Provider::Claude,
                default_model: "model".into(),
                enabled: true,
                env: std::collections::BTreeMap::new(),
                capabilities: Vec::new(),
                command: None,
            },
            "2026-09-05T00:00:00.000Z",
        )
        .expect("profile");
    (directory, McpServer::new(HttpState::new(store)))
}

async fn call(server: &McpServer, name: &str, arguments: Value) -> Value {
    let response = router(server.state().clone())
        .oneshot(
            axum::http::Request::post("/mcp")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "tools/call",
                        "params": { "name": name, "arguments": arguments }
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = String::from_utf8(body.to_vec()).expect("UTF-8 body");
    let payload = body
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap_or(&body);
    serde_json::from_str(payload).expect("JSON-RPC response")
}

fn tool_body(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("tool text");
    serde_json::from_str(text).expect("tool JSON")
}

fn insert(server: &McpServer, task: &Task) {
    server
        .state()
        .store
        .repositories()
        .tasks()
        .insert(task)
        .expect("task");
    // The checkout columns belong to dispatch, not the task repository.
    if let Some(worktree) = &task.worktree {
        server
            .state()
            .store
            .transaction(|tx| {
                tx.execute(
                    "UPDATE tasks SET origin_cwd=?,worktree_path=?,worktree_branch=? WHERE id=?",
                    rusqlite::params![worktree.origin_cwd, worktree.path, worktree.branch, task.id],
                )?;
                Ok(())
            })
            .expect("checkout columns");
    }
}

fn task(id: &str, state: TaskState) -> Task {
    Task {
        id: id.into(),
        profile_id: "main".into(),
        model: "model".into(),
        prompt: "work".into(),
        cwd: "/project".into(),
        state,
        created_at: "2026-09-05T00:00:00.000Z".into(),
        updated_at: "2026-09-05T00:00:00.000Z".into(),
        ..Task::default()
    }
}

#[tokio::test]
async fn task_search_returns_the_matching_field() {
    let (_directory, server) = test_server();
    let task = task("search-task", TaskState::Completed);
    insert(&server, &task);
    server
        .state()
        .store
        .transaction(|connection| {
            connection.execute(
                "UPDATE tasks SET title='Search task history' WHERE id=?",
                [&task.id],
            )?;
            Ok(())
        })
        .expect("title updates");

    let body = tool_body(&call(&server, "tasks", json!({ "query": "SEARCH" })).await);

    assert_eq!(body[0]["id"], task.id);
    assert_eq!(body[0]["match"], "title");
}

#[tokio::test]
async fn cancelling_a_worktree_task_offers_the_checkout_it_left_behind() {
    let (directory, server) = test_server();
    let checkout = directory.path().join("checkout");
    std::fs::create_dir(&checkout).expect("checkout directory");
    let path = checkout.display().to_string();
    let mut running = task("task-worktree", TaskState::Running);
    running.worktree = Some(TaskWorktree {
        origin_cwd: "/project".into(),
        path: path.clone(),
        branch: "oga/thing".into(),
        links: None,
    });
    insert(&server, &running);

    let body = tool_body(&call(&server, "cancel", json!({ "taskId": running.id })).await);

    assert_eq!(body["state"], json!("cancelled"));
    let next = body["next"].as_array().expect("next");
    let tools = next
        .iter()
        .map(|hint| hint["tool"].as_str().expect("tool"))
        .collect::<Vec<_>>();
    assert_eq!(tools, ["resume", "handoff", "archive"]);
    let archive = next[2]["when"].as_str().expect("when");
    assert!(archive.contains(&path), "{archive}");
    assert!(archive.contains("the branch stays"), "{archive}");
}

#[tokio::test]
async fn cancelling_a_task_without_a_checkout_never_mentions_one() {
    let (_directory, server) = test_server();
    let running = task("task-plain", TaskState::Running);
    insert(&server, &running);

    let body = tool_body(&call(&server, "cancel", json!({ "taskId": running.id })).await);

    let tools = body["next"]
        .as_array()
        .expect("next")
        .iter()
        .map(|hint| hint["tool"].as_str().expect("tool"))
        .collect::<Vec<_>>();
    assert_eq!(tools, ["resume", "handoff"]);
}

#[tokio::test]
async fn resuming_a_running_task_is_refused_and_points_at_steer() {
    let (_directory, server) = test_server();
    let running = task("task-running", TaskState::Running);
    insert(&server, &running);

    let response = call(
        &server,
        "resume",
        json!({ "taskId": running.id, "instruction": "look at the other file too" }),
    )
    .await;

    assert_eq!(response["result"]["isError"], json!(true));
    let body = tool_body(&response);
    assert!(
        body["error"]
            .as_str()
            .expect("error")
            .contains("cannot be resumed from state running")
    );
    let next = body["next"].as_array().expect("next");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0]["tool"], json!("steer"));
}

#[tokio::test]
async fn a_completed_worktree_task_reads_as_a_branch_to_ship() {
    let (directory, server) = test_server();
    let checkout = directory.path().join("done");
    std::fs::create_dir(&checkout).expect("checkout directory");
    let path = checkout.display().to_string();
    let mut completed = task("task-done", TaskState::Completed);
    completed.worktree = Some(TaskWorktree {
        origin_cwd: "/project".into(),
        path: path.clone(),
        branch: "oga/ship-it".into(),
        links: None,
    });
    insert(&server, &completed);

    let body = tool_body(&call(&server, "inspect", json!({ "taskId": completed.id })).await);

    let next = body["next"].as_array().expect("next");
    let tools = next
        .iter()
        .map(|hint| hint["tool"].as_str().expect("tool"))
        .collect::<Vec<_>>();
    assert_eq!(tools, ["resume", "shell", "archive"]);
    let ship = next[1]["when"].as_str().expect("when");
    assert!(ship.contains("oga/ship-it"), "{ship}");
    assert!(ship.contains("pull request"), "{ship}");
}

#[tokio::test]
async fn every_tool_description_stays_short() {
    let (_directory, server) = test_server();
    let response = router(server.state().clone())
        .oneshot(
            axum::http::Request::post("/mcp")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(Body::from(
                    json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = String::from_utf8(body.to_vec()).expect("UTF-8 body");
    let payload = body
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap_or(&body);
    let value: Value = serde_json::from_str(payload).expect("JSON-RPC response");

    // A description says what the tool does and when to pick it over its
    // neighbours. Anything state-specific belongs in a response's `next`.
    for tool in value["result"]["tools"].as_array().expect("tools") {
        let name = tool["name"].as_str().expect("name");
        let words = tool["description"]
            .as_str()
            .expect("description")
            .split_whitespace()
            .count();
        assert!(words <= 90, "{name} description runs {words} words");
    }
}
