//! What a task response tells the caller to do next, over the wire.

use std::{path::Path, process::Command, sync::Arc};

use axum::body::Body;
use http_body_util::BodyExt;
use oga_domain::{Task, TaskState, TaskWorktree, WorktreeRequest};
use oga_http::HttpState;
use oga_mcp::{McpServer, router};
use oga_store::Store;
use oga_worktree::{branch_exists, create_task_worktree_at, remove_task_worktree};
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

fn repository(path: &Path, branch: &str) {
    init_repository(path);
    git(path, &["branch", branch]);
}

fn init_repository(path: &Path) {
    std::fs::create_dir(path).expect("repository directory");
    git(path, &["init", "-b", "main"]);
    std::fs::write(path.join("tracked.txt"), "one\n").expect("tracked file");
    git(path, &["add", "tracked.txt"]);
    git(
        path,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ],
    );
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .output()
        .expect("git is installed");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
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
    let project = directory.path().join("project");
    repository(&project, "oga/thing");
    let checkout = directory.path().join("checkout");
    std::fs::create_dir(&checkout).expect("checkout directory");
    let path = checkout.display().to_string();
    let mut running = task("task-worktree", TaskState::Running);
    running.worktree = Some(TaskWorktree {
        origin_cwd: project.display().to_string(),
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
    let project = directory.path().join("project");
    repository(&project, "oga/ship-it");
    let checkout = directory.path().join("done");
    std::fs::create_dir(&checkout).expect("checkout directory");
    let path = checkout.display().to_string();
    let mut completed = task("task-done", TaskState::Completed);
    completed.worktree = Some(TaskWorktree {
        origin_cwd: project.display().to_string(),
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
async fn an_archived_task_with_an_unavailable_repository_never_offers_resume() {
    let (directory, server) = test_server();
    let checkout = directory.path().join("archived");
    std::fs::create_dir(&checkout).expect("checkout directory");
    let mut archived = task("task-archived", TaskState::Completed);
    archived.archived_at = Some("2026-09-05T01:00:00.000Z".into());
    archived.worktree = Some(TaskWorktree {
        origin_cwd: directory
            .path()
            .join("missing-project")
            .display()
            .to_string(),
        path: checkout.display().to_string(),
        branch: "oga/gone".into(),
        links: None,
    });
    insert(&server, &archived);

    let body = tool_body(&call(&server, "inspect", json!({ "taskId": archived.id })).await);

    assert!(
        body["next"]
            .as_array()
            .expect("next")
            .iter()
            .all(|hint| hint["tool"] != "resume")
    );
}

#[tokio::test]
async fn removing_a_busy_branch_never_offers_resume() {
    let (directory, server) = test_server();
    let project = directory.path().join("project");
    init_repository(&project);
    let created = create_task_worktree_at(
        &directory.path().join("worktrees"),
        &project,
        "busy-task",
        &WorktreeRequest {
            branch: Some("oga/busy".into()),
            ..WorktreeRequest::default()
        },
        Some("busy branch"),
    )
    .await
    .expect("worktree created");
    let mut completed = task("task-busy", TaskState::Completed);
    completed.cwd = created.cwd.display().to_string();
    completed.branch = Some(created.worktree.branch.clone());
    completed.worktree = Some(created.worktree.clone());
    insert(&server, &completed);
    remove_task_worktree(&created.worktree)
        .await
        .expect("checkout removed");
    let elsewhere = directory.path().join("elsewhere");
    git(
        &project,
        &[
            "worktree",
            "add",
            elsewhere.to_str().expect("elsewhere path"),
            &created.worktree.branch,
        ],
    );

    let body = tool_body(
        &call(
            &server,
            "worktree-remove",
            json!({ "taskId": completed.id, "deleteBranch": true }),
        )
        .await,
    );

    assert_eq!(body["branch"], "kept");
    assert!(
        body["branchReason"]
            .as_str()
            .is_some_and(|reason| { reason.contains("branch is checked out at") })
    );
    assert!(
        body["next"]
            .as_array()
            .expect("next")
            .iter()
            .all(|hint| hint["tool"] != "resume")
    );

    let body = tool_body(
        &call(
            &server,
            "worktree-remove",
            json!({ "taskId": completed.id, "deleteBranch": false }),
        )
        .await,
    );
    assert_eq!(body["branch"], "kept");
    assert!(
        body["next"]
            .as_array()
            .is_none_or(|next| next.iter().all(|hint| hint["tool"] != "resume"))
    );
}

#[tokio::test]
async fn archiving_a_busy_branch_never_offers_resume() {
    let (directory, server) = test_server();
    let project = directory.path().join("project");
    init_repository(&project);
    let created = create_task_worktree_at(
        &directory.path().join("worktrees"),
        &project,
        "busy-archive-task",
        &WorktreeRequest {
            branch: Some("oga/busy-archive".into()),
            ..WorktreeRequest::default()
        },
        Some("busy archive branch"),
    )
    .await
    .expect("worktree created");
    let mut completed = task("task-busy-archive", TaskState::Completed);
    completed.cwd = created.cwd.display().to_string();
    completed.branch = Some(created.worktree.branch.clone());
    completed.worktree = Some(created.worktree.clone());
    insert(&server, &completed);
    remove_task_worktree(&created.worktree)
        .await
        .expect("checkout removed");
    let elsewhere = directory.path().join("elsewhere");
    git(
        &project,
        &[
            "worktree",
            "add",
            elsewhere.to_str().expect("elsewhere path"),
            &created.worktree.branch,
        ],
    );

    let body = tool_body(
        &call(
            &server,
            "archive",
            json!({ "taskId": completed.id, "deleteBranch": true }),
        )
        .await,
    );

    assert_eq!(body["branchOutcome"], "kept");
    assert!(
        body["branchReason"]
            .as_str()
            .is_some_and(|reason| reason.contains("branch is checked out at"))
    );
    assert!(
        body["next"]
            .as_array()
            .is_none_or(|next| next.iter().all(|hint| hint["tool"] != "resume"))
    );
}

#[tokio::test]
async fn archiving_a_batch_deletes_each_clean_branch() {
    let (directory, server) = test_server();
    let project = directory.path().join("project");
    init_repository(&project);
    let mut tasks = Vec::new();
    let mut branches = Vec::new();
    for id in ["batch-one", "batch-two"] {
        let created = create_task_worktree_at(
            &directory.path().join("worktrees"),
            &project,
            id,
            &WorktreeRequest::default(),
            Some(id),
        )
        .await
        .expect("worktree created");
        let mut completed = task(id, TaskState::Completed);
        completed.cwd = created.cwd.display().to_string();
        completed.branch = Some(created.worktree.branch.clone());
        completed.worktree = Some(created.worktree.clone());
        insert(&server, &completed);
        branches.push(created.worktree.branch);
        tasks.push(id);
    }

    let body = tool_body(
        &call(
            &server,
            "archive",
            json!({ "taskId": tasks, "deleteBranch": true }),
        )
        .await,
    );

    let entries = body.as_array().expect("batch response");
    assert_eq!(entries.len(), 2);
    assert!(
        entries
            .iter()
            .all(|entry| entry["branchOutcome"] == "deleted")
    );
    for branch in branches {
        assert!(!branch_exists(&project, &branch).await.unwrap());
    }
}
