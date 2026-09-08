use std::sync::Arc;

use axum::body::Body;
use http_body_util::BodyExt;
use oga_domain::{Profile, Provider, Task, TaskKind, TaskState};
use oga_http::HttpState;
use oga_mcp::{MCP_PROTOCOL_VERSION, McpServer, router};
use oga_store::Store;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

const TIMESTAMP: &str = "2026-08-26T00:00:00.000Z";

fn test_server() -> (TempDir, McpServer) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
    (directory, McpServer::new(HttpState::new(store)))
}

async fn post(server: &McpServer, request: Value) -> Value {
    post_as(server, None, request).await
}

async fn post_as(server: &McpServer, task_id: Option<&str>, request: Value) -> Value {
    let mut builder = axum::http::Request::post("/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(task_id) = task_id {
        builder = builder.header("x-oga-task-id", task_id);
    }
    let response = router(server.state().clone())
        .oneshot(
            builder
                .body(Body::from(request.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    // Responses ride the MCP event stream, one `data:` line per frame.
    let body = String::from_utf8(body.to_vec()).expect("UTF-8 body");
    let payload = body
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap_or(&body);
    serde_json::from_str(payload).expect("JSON-RPC response")
}

#[tokio::test]
async fn requests_without_the_mcp_accept_header_are_rejected() {
    let (_directory, server) = test_server();
    let response = router(server.state().clone())
        .oneshot(
            axum::http::Request::post("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), axum::http::StatusCode::NOT_ACCEPTABLE);
    assert_eq!(response.headers()["content-type"], "application/json");
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value: Value = serde_json::from_slice(&body).expect("JSON-RPC response");
    assert_eq!(value["error"]["code"], -32000);
    assert_eq!(
        value["error"]["message"],
        "Not Acceptable: Client must accept both application/json and text/event-stream"
    );
    assert_eq!(value["id"], Value::Null);
}

#[tokio::test]
async fn initialize_advertises_protocol_and_instructions() {
    let (_directory, server) = test_server();
    let response = post(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": MCP_PROTOCOL_VERSION }
        }),
    )
    .await;

    assert_eq!(response["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
    assert!(response["result"]["capabilities"]["resources"].is_null());
    let instructions = response["result"]["instructions"]
        .as_str()
        .expect("instructions");
    assert!(instructions.contains("oga watch <taskId>"));
    assert!(instructions.contains("oga query \"<what you need>\"`"));
    assert!(
        instructions
            .contains("add `--code` when you want the code back instead of just the location")
    );
}

#[tokio::test]
async fn tools_list_exposes_the_complete_mcp_surface() {
    let (_directory, server) = test_server();
    let response = post(
        &server,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    let tools = response["result"]["tools"].as_array().expect("tools");
    let names = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();

    assert_eq!(names.len(), 15);
    for name in [
        "delegate",
        "models",
        "inspect",
        "health",
        "tasks",
        "memory",
        "query",
        "reply",
        "resume",
        "steer",
        "handoff",
        "cancel",
        "complete",
        "archive",
        "worktree-remove",
    ] {
        assert!(names.contains(&name), "missing tool {name}");
    }
}

#[tokio::test]
async fn a_worker_only_sees_delegate_when_its_task_may_hand_work_onward() {
    let (_directory, server) = test_server();
    let profile = insert_profile(&server);
    insert_worker(&server, &profile, "kept", false);
    insert_worker(&server, &profile, "fanning-out", true);

    for (task_id, expected) in [("kept", false), ("fanning-out", true)] {
        let response = post_as(
            &server,
            Some(task_id),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await;
        let served = response["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .any(|tool| tool["name"] == "delegate");
        assert_eq!(served, expected, "{task_id}");
    }

    let refused = post_as(
        &server,
        Some("kept"),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "delegate", "arguments": {
                "prompt": "do more", "cwd": "/repo", "tldr": "more work", "title": "More"
            }}
        }),
    )
    .await;
    assert_eq!(refused["result"]["isError"], true);
}

fn insert_profile(server: &McpServer) -> Profile {
    let profile = Profile {
        id: "main".into(),
        label: "Main".into(),
        provider: Provider::Claude,
        default_model: "sonnet".into(),
        enabled: true,
        env: std::collections::BTreeMap::new(),
        capabilities: Vec::new(),
        command: None,
    };
    server
        .state()
        .store
        .repositories()
        .profiles()
        .insert(&profile, TIMESTAMP)
        .expect("profile insert");
    profile
}

fn insert_worker(server: &McpServer, profile: &Profile, id: &str, can_delegate: bool) {
    server
        .state()
        .store
        .repositories()
        .tasks()
        .insert(&Task {
            id: id.into(),
            kind: Some(TaskKind::Delegated),
            profile_id: profile.id.clone(),
            model: profile.default_model.clone(),
            prompt: "work".into(),
            cwd: "/repo".into(),
            state: TaskState::Running,
            created_at: TIMESTAMP.into(),
            updated_at: TIMESTAMP.into(),
            can_delegate,
            ..Task::default()
        })
        .expect("task insert");
}

#[tokio::test]
async fn tasks_schema_exposes_text_search() {
    let (_directory, server) = test_server();
    let response = post(
        &server,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    let task_tool = response["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == "tasks")
        .expect("tasks tool");

    assert_eq!(
        task_tool["inputSchema"]["properties"]["query"]["type"],
        "string"
    );
    assert!(
        task_tool["description"]
            .as_str()
            .expect("description")
            .contains("ranks title matches first")
    );
}

#[tokio::test]
async fn tool_call_returns_mcp_content() {
    let (_directory, server) = test_server();
    let response = post(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "health", "arguments": {} }
        }),
    )
    .await;

    assert_eq!(response["result"]["content"][0]["type"], "text");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("tool text");
    assert!(text.contains("mcpContractVersion"));
}

#[tokio::test]
async fn models_honor_project_model_enablement() {
    let (directory, server) = test_server();
    let profile = Profile {
        id: "main".into(),
        label: "Main".into(),
        provider: Provider::Claude,
        default_model: "sonnet".into(),
        enabled: true,
        env: std::collections::BTreeMap::new(),
        capabilities: Vec::new(),
        command: None,
    };
    let cwd = oga_config::canonical_cwd(directory.path())
        .display()
        .to_string();
    server
        .state()
        .store
        .repositories()
        .profiles()
        .insert(&profile, "2026-08-26T00:00:00.000Z")
        .expect("profile");
    server
        .state()
        .store
        .repositories()
        .settings()
        .put(
            &cwd,
            "models",
            r#"{"profiles":{"main":{"models":{"sonnet":false}}}}"#,
            "2026-08-26T00:00:00.000Z",
        )
        .expect("model settings");

    let response = post(
        &server,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "models",
                "arguments": { "cwd": cwd, "onlyPreferred": false }
            }
        }),
    )
    .await;
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("model rows");
    let catalog: Value = serde_json::from_str(text).expect("model rows JSON");
    let rows = catalog["models"].as_array().expect("model rows");
    assert!(rows.iter().all(|row| row["model"] != "sonnet"));
    assert_eq!(catalog["love"], json!([]));
}
