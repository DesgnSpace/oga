//! Changing work over the wire: what a waiting task waits on, its brief,
//! putting a cancelled one back in line, and what each run was sent.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use axum::body::Body;
use http_body_util::BodyExt;
use oga_http::HttpState;
use oga_mcp::{McpServer, router};
use oga_store::Store;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

/// A broker whose `quick` worker finishes at once, and whose `slow` and
/// `later` workers each finish only once their gate file exists.
struct Broker {
    directory: TempDir,
    server: McpServer,
}

impl Broker {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        let gate = |name: &str| {
            format!(
                "while [ ! -e '{}' ]; do sleep 0.02; done; printf 'finished\\nOGA_RESULT: completed\\n'",
                directory.path().join(name).display()
            )
        };
        for (id, script) in [
            (
                "quick",
                "printf 'finished\\nOGA_RESULT: completed\\n'".to_owned(),
            ),
            ("slow", gate("slow-gate")),
            ("later", gate("later-gate")),
        ] {
            store
                .repositories()
                .profiles()
                .insert(
                    &oga_domain::Profile {
                        id: id.into(),
                        label: id.into(),
                        provider: oga_domain::Provider::Claude,
                        default_model: "model".into(),
                        enabled: true,
                        env: BTreeMap::new(),
                        capabilities: Vec::new(),
                        command: Some(vec!["sh".into(), "-c".into(), script]),
                    },
                    "2026-09-05T00:00:00.000Z",
                )
                .expect("profile");
        }
        store
            .repositories()
            .settings()
            .put(
                &oga_config::canonical_cwd(oga_config::global_cwd())
                    .display()
                    .to_string(),
                oga_config::MODEL_SETTINGS_KEY,
                &json!({
                    "profiles": {
                        "quick": { "modelEnabled": { "model": true } },
                        "slow": { "modelEnabled": { "model": true } },
                        "later": { "modelEnabled": { "model": true } },
                    }
                })
                .to_string(),
                "2026-09-05T00:00:00.000Z",
            )
            .expect("models on");
        let server = McpServer::new(HttpState::new(store));
        Self { directory, server }
    }

    fn cwd(&self) -> PathBuf {
        self.directory.path().to_path_buf()
    }

    fn open(&self, gate: &str) {
        std::fs::write(self.directory.path().join(gate), "").expect("open gate");
    }

    async fn call(&self, name: &str, arguments: Value) -> Value {
        self.call_from(None, name, arguments).await
    }

    /// A tool call made by a running task, as its worker's MCP client sends it.
    async fn call_from(&self, caller: Option<&str>, name: &str, arguments: Value) -> Value {
        let mut request = axum::http::Request::post("/mcp")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(caller) = caller {
            request = request.header("x-oga-task-id", caller);
        }
        let response = router(self.server.state().clone())
            .oneshot(
                request
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
        let response: Value = serde_json::from_str(payload).expect("JSON-RPC response");
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("tool text in {response}"));
        serde_json::from_str(text).unwrap_or_else(|_| json!({ "error": text }))
    }

    async fn delegate(&self, profile: &str, title: &str, extra: Value) -> String {
        let mut arguments = json!({
            "prompt": format!("{title} work"),
            "cwd": self.cwd(),
            "tldr": title,
            "title": title,
            "profile": profile,
            "model": "model",
        });
        arguments
            .as_object_mut()
            .expect("arguments")
            .extend(extra.as_object().cloned().unwrap_or_default());
        let body = self.call("delegate", arguments).await;
        body["id"]
            .as_str()
            .unwrap_or_else(|| panic!("delegate answered {body}"))
            .to_owned()
    }

    async fn state(&self, id: &str) -> String {
        self.call("inspect", json!({ "taskId": id })).await["state"]
            .as_str()
            .expect("state")
            .to_owned()
    }

    async fn record(&self, id: &str) -> Value {
        self.call("inspect", json!({ "taskId": id, "fields": ["all"] }))
            .await
    }

    async fn wait_for(&self, id: &str, wanted: &str) {
        let mut state = String::new();
        for _ in 0..1_000 {
            state = self.state(id).await;
            if state == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{id} never reached {wanted}; it is {state}");
    }

    /// Asserts `id` holds `state` for long enough that a release would have
    /// shown.
    async fn stays(&self, id: &str, state: &str) {
        for _ in 0..20 {
            assert_eq!(self.state(id).await, state, "{id}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

fn tools(body: &Value) -> Vec<&str> {
    body["next"]
        .as_array()
        .map(|next| {
            next.iter()
                .filter_map(|hint| hint["tool"].as_str())
                .collect()
        })
        .unwrap_or_default()
}

fn error(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn a_waiting_task_moved_onto_new_work_waits_for_that_work_instead() {
    let broker = Broker::new();
    let first = broker.delegate("slow", "First", json!({})).await;
    let urgent = broker.delegate("later", "Urgent", json!({})).await;
    let waiting = broker
        .delegate("quick", "Waiting", json!({ "dependsOn": [first] }))
        .await;
    assert_eq!(broker.state(&waiting).await, "pending");

    let body = broker
        .call("edit", json!({ "taskId": waiting, "dependsOn": [urgent] }))
        .await;
    assert_eq!(body["id"], json!(waiting), "{body}");
    assert_eq!(body["state"], "pending");
    assert!(tools(&body).contains(&"edit"), "{body}");

    broker.open("slow-gate");
    broker.wait_for(&first, "completed").await;
    broker.stays(&waiting, "pending").await;

    broker.open("later-gate");
    broker.wait_for(&urgent, "completed").await;
    broker.wait_for(&waiting, "completed").await;
}

/// Bug caught: the task record never says what a task waits on, so a caller
/// following a chain cannot tell what holds it or what it ran after.
#[tokio::test]
async fn the_record_names_what_a_task_waits_on_while_waiting_and_after_it_runs() {
    let broker = Broker::new();
    let first = broker.delegate("slow", "First", json!({})).await;
    let urgent = broker.delegate("later", "Urgent", json!({})).await;
    let waiting = broker
        .delegate(
            "quick",
            "Waiting",
            json!({ "dependsOn": [first], "onBlockerFailure": "run" }),
        )
        .await;
    let before = broker.record(&waiting).await;
    assert_eq!(before["dependsOn"], json!([first]), "{before}");
    assert_eq!(before["onBlockerFailure"], "run", "{before}");

    broker
        .call("edit", json!({ "taskId": waiting, "dependsOn": [urgent] }))
        .await;
    let edited = broker.record(&waiting).await;
    assert_eq!(edited["dependsOn"], json!([urgent]), "{edited}");

    broker.open("slow-gate");
    broker.open("later-gate");
    broker.wait_for(&waiting, "completed").await;
    let ran = broker.record(&waiting).await;
    assert_eq!(ran["dependsOn"], json!([urgent]), "{ran}");
    assert!(ran.get("onBlockerFailure").is_none(), "{ran}");
}

/// Bug caught: work a running task hands onward never names that task, so the
/// record of a lead's children shows no link back to it.
#[tokio::test]
async fn work_a_task_hands_onward_names_it_as_parent() {
    let broker = Broker::new();
    let lead = broker
        .delegate("slow", "Lead", json!({ "canDelegate": true }))
        .await;
    let other = broker.delegate("later", "Other", json!({})).await;
    let delegate_from_lead = |title: &str, extra: Value| {
        let mut arguments = json!({
            "prompt": format!("{title} work"),
            "cwd": broker.cwd(),
            "tldr": title,
            "title": title,
            "profile": "quick",
            "model": "model",
        });
        arguments
            .as_object_mut()
            .expect("arguments")
            .extend(extra.as_object().cloned().unwrap_or_default());
        broker.call_from(Some(&lead), "delegate", arguments)
    };

    let child = delegate_from_lead("Child", json!({})).await;
    let grouped = delegate_from_lead("Grouped", json!({ "parent": other })).await;
    let by_owner = broker.delegate("quick", "Owner's", json!({})).await;

    let child = broker.record(child["id"].as_str().expect("child id")).await;
    let grouped = broker
        .record(grouped["id"].as_str().expect("grouped id"))
        .await;
    assert_eq!(child["parentTaskId"], json!(lead), "{child}");
    assert_eq!(grouped["parentTaskId"], json!(other), "{grouped}");
    assert!(broker.record(&by_owner).await.get("parentTaskId").is_none());
    broker.open("slow-gate");
    broker.open("later-gate");
}

/// Bug caught: a resume replaces the shipped prompt with the continuation, and
/// the first run's brief, with its memories and house sections, is lost.
#[tokio::test]
async fn each_attempt_keeps_the_prompt_its_run_was_sent() {
    let broker = Broker::new();
    let task = broker.delegate("quick", "Brief", json!({})).await;
    broker.wait_for(&task, "completed").await;
    let first = broker.record(&task).await["shippedPrompt"].clone();
    assert!(
        first
            .as_str()
            .is_some_and(|prompt| prompt.contains("Brief work"))
    );

    let resumed = broker
        .call(
            "resume",
            json!({ "taskId": task, "instruction": "check the edge case" }),
        )
        .await;
    assert!(resumed["error"].is_null(), "{resumed}");
    assert_ne!(resumed["state"], "completed", "{resumed}");
    broker.wait_for(&task, "completed").await;

    let record = broker.record(&task).await;
    assert_eq!(record["attempts"][0]["shippedPrompt"], first, "{record}");
    assert!(
        record["shippedPrompt"]
            .as_str()
            .is_some_and(|prompt| prompt.contains("check the edge case")),
        "{record}"
    );
}

#[tokio::test]
async fn an_edit_that_closes_a_loop_is_refused_and_changes_nothing() {
    let broker = Broker::new();
    let blocker = broker.delegate("slow", "Blocker", json!({})).await;
    let first = broker
        .delegate("quick", "First", json!({ "dependsOn": [blocker] }))
        .await;
    let second = broker
        .delegate("quick", "Second", json!({ "dependsOn": [first] }))
        .await;

    let cycle = broker
        .call("edit", json!({ "taskId": first, "addDependsOn": [second] }))
        .await;
    assert!(error(&cycle).contains("dependency cycle"), "{cycle}");
    let itself = broker
        .call("edit", json!({ "taskId": first, "addDependsOn": [first] }))
        .await;
    assert!(
        error(&itself).contains("cannot depend on itself"),
        "{itself}"
    );
    let unknown = broker
        .call(
            "edit",
            json!({ "taskId": first, "addDependsOn": ["no-such-task"] }),
        )
        .await;
    assert!(
        error(&unknown).contains("unknown prerequisite"),
        "{unknown}"
    );

    broker.open("slow-gate");
    broker.wait_for(&second, "completed").await;
    assert_eq!(broker.state(&first).await, "completed");
}

#[tokio::test]
async fn a_requeued_task_waits_for_its_prerequisite_instead_of_starting() {
    let broker = Broker::new();
    let blocker = broker.delegate("slow", "Blocker", json!({})).await;
    let waiting = broker
        .delegate("quick", "Waiting", json!({ "dependsOn": [blocker] }))
        .await;
    let cancelled = broker.call("cancel", json!({ "taskId": waiting })).await;
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");
    assert_eq!(tools(&cancelled).first(), Some(&"edit"), "{cancelled}");

    let body = broker
        .call("edit", json!({ "taskId": waiting, "requeue": true }))
        .await;
    assert_eq!(body["state"], "pending", "{body}");
    broker.stays(&waiting, "pending").await;

    broker.open("slow-gate");
    broker.wait_for(&blocker, "completed").await;
    broker.wait_for(&waiting, "completed").await;
}

#[tokio::test]
async fn an_instruction_added_while_waiting_reaches_the_worker() {
    let broker = Broker::new();
    let blocker = broker.delegate("slow", "Blocker", json!({})).await;
    let waiting = broker
        .delegate("quick", "Waiting", json!({ "dependsOn": [blocker] }))
        .await;
    let steered = broker
        .call(
            "steer",
            json!({ "taskId": waiting, "instruction": "push when done" }),
        )
        .await;
    assert_eq!(tools(&steered), ["edit"], "{steered}");

    let body = broker
        .call(
            "edit",
            json!({ "taskId": waiting, "instruction": "Push the branch when done." }),
        )
        .await;
    assert_eq!(body["state"], "pending", "{body}");

    broker.open("slow-gate");
    broker.wait_for(&waiting, "completed").await;
    let record = broker
        .call(
            "inspect",
            json!({ "taskId": waiting, "fields": ["shippedPrompt"] }),
        )
        .await;
    let shipped = record["shippedPrompt"].as_str().expect("shipped prompt");
    assert!(shipped.contains("Waiting work"), "{shipped}");
    assert!(shipped.contains("Push the branch when done."), "{shipped}");
}

/// Moving a prerequisite behind new work, or cancelling it before it started,
/// is not a failure, so a `run` dependent must not start early.
#[tokio::test]
async fn reordering_never_starts_a_task_that_runs_after_failures() {
    let broker = Broker::new();
    let first = broker.delegate("slow", "First", json!({})).await;
    let middle = broker
        .delegate("quick", "Middle", json!({ "dependsOn": [first] }))
        .await;
    let last = broker
        .delegate(
            "quick",
            "Last",
            json!({ "dependsOn": [middle], "onBlockerFailure": "run" }),
        )
        .await;
    let urgent = broker.delegate("later", "Urgent", json!({})).await;

    let moved = broker
        .call(
            "edit",
            json!({ "taskId": middle, "addDependsOn": [urgent] }),
        )
        .await;
    assert_eq!(moved["state"], "pending", "{moved}");
    broker.stays(&last, "pending").await;

    let cancelled = broker.call("cancel", json!({ "taskId": middle })).await;
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");
    broker.wait_for(&last, "blocked").await;

    let requeued = broker
        .call("edit", json!({ "taskId": middle, "requeue": true }))
        .await;
    assert_eq!(requeued["state"], "pending", "{requeued}");
    broker.wait_for(&last, "pending").await;

    broker.open("slow-gate");
    broker.wait_for(&first, "completed").await;
    broker.stays(&middle, "pending").await;
    broker.open("later-gate");
    broker.wait_for(&middle, "completed").await;
    broker.wait_for(&last, "completed").await;
}

#[tokio::test]
async fn a_task_that_already_ran_cannot_be_edited() {
    let broker = Broker::new();
    let done = broker.delegate("quick", "Done", json!({})).await;
    broker.wait_for(&done, "completed").await;

    let body = broker
        .call("edit", json!({ "taskId": done, "instruction": "more" }))
        .await;
    assert!(error(&body).contains("already run"), "{body}");
}
