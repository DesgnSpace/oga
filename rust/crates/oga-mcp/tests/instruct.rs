//! One instruction for tasks in every state, over the wire: where it lands for
//! each, what the next run carries, and the call a refusal points at.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use axum::body::Body;
use http_body_util::BodyExt;
use oga_domain::{Task, TaskKind, TaskState};
use oga_http::HttpState;
use oga_mcp::{McpServer, router};
use oga_store::Store;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

const RULE: &str = "Merge origin/main before each push.";

/// A broker whose `quick` worker finishes at once, whose `slow` worker finishes
/// once its gate file exists, and whose `failing` worker always fails.
struct Broker {
    directory: TempDir,
    server: McpServer,
}

impl Broker {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        let gate = format!(
            "while [ ! -e '{}' ]; do sleep 0.02; done; printf 'finished\\nOGA_RESULT: completed\\n'",
            directory.path().join("slow-gate").display()
        );
        for (id, script) in [
            (
                "quick",
                "printf 'finished\\nOGA_RESULT: completed\\n'".to_owned(),
            ),
            ("slow", gate),
            (
                "failing",
                "printf 'oops\\nOGA_RESULT: failed\\n'; exit 1".to_owned(),
            ),
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
                        "failing": { "modelEnabled": { "model": true } },
                    }
                })
                .to_string(),
                "2026-09-05T00:00:00.000Z",
            )
            .expect("models on");
        let server = McpServer::new(HttpState::new(store));
        Self { directory, server }
    }

    fn open_gate(&self) {
        std::fs::write(self.directory.path().join("slow-gate"), "").expect("open gate");
    }

    async fn call(&self, name: &str, arguments: Value) -> Value {
        let response = router(self.server.state().clone())
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
        let response: Value = serde_json::from_str(payload).expect("JSON-RPC response");
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("tool text in {response}"));
        serde_json::from_str(text).unwrap_or_else(|_| json!({ "error": text }))
    }

    async fn delegate(&self, profile: &str, title: &str, extra: Value) -> String {
        let mut arguments = json!({
            "prompt": format!("{title} work"),
            "cwd": self.directory.path(),
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

    async fn settled(&self, profile: &str, title: &str, wanted: &str) -> String {
        let id = self.delegate(profile, title, json!({})).await;
        self.wait_for(&id, wanted).await;
        id
    }

    /// A task parked on a question it asked its caller.
    fn asking(&self) -> String {
        let task = Task {
            id: "task-asking".into(),
            kind: Some(TaskKind::Delegated),
            profile_id: "quick".into(),
            model: "model".into(),
            prompt: "Asking work".into(),
            shipped_prompt: Some("Asking work".into()),
            cwd: self.directory.path().display().to_string(),
            state: TaskState::NeedsInput,
            question: Some("Which branch?".into()),
            created_at: "2026-09-05T00:00:00.000Z".into(),
            updated_at: "2026-09-05T00:00:00.000Z".into(),
            ..Task::default()
        };
        self.server
            .state()
            .store
            .repositories()
            .tasks()
            .insert(&task)
            .expect("task");
        task.id
    }

    async fn inspect(&self, id: &str) -> Value {
        self.call(
            "inspect",
            json!({ "taskId": id, "fields": ["shippedPrompt"] }),
        )
        .await
    }

    async fn wait_for(&self, id: &str, wanted: &str) {
        let mut state = Value::Null;
        for _ in 0..1_000 {
            state = self.inspect(id).await["state"].clone();
            if state == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{id} never reached {wanted}; it is {state}");
    }

    /// Waits until the brief `id` last shipped carries `text`.
    async fn wait_for_shipped(&self, id: &str, text: &str) {
        let mut record = Value::Null;
        for _ in 0..1_000 {
            record = self.inspect(id).await;
            if record["state"] == "completed"
                && record["shippedPrompt"]
                    .as_str()
                    .is_some_and(|shipped| shipped.contains(text))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{id} never ran with {text:?}: {record}");
    }
}

fn outcomes(body: &Value) -> Vec<&str> {
    body["results"]
        .as_array()
        .unwrap_or_else(|| panic!("results in {body}"))
        .iter()
        .map(|result| result["outcome"].as_str().expect("outcome"))
        .collect()
}

#[tokio::test]
async fn one_call_reaches_a_chain_in_every_state_and_answers_per_task() {
    let broker = Broker::new();
    let running = broker.delegate("slow", "Running", json!({})).await;
    broker.wait_for(&running, "running").await;
    let waiting = broker
        .delegate("quick", "Waiting", json!({ "dependsOn": [running] }))
        .await;
    let completed = broker.settled("quick", "Completed", "completed").await;
    let failed = broker.settled("failing", "Failed", "failed").await;
    let asking = broker.asking();

    let body = broker
        .call(
            "instruct",
            json!({
                "taskId": [waiting, running, completed, failed, asking, "task-missing"],
                "instruction": RULE,
            }),
        )
        .await;

    assert_eq!(
        outcomes(&body),
        ["appended", "queued", "saved", "saved", "refused", "refused"],
        "{body}"
    );
    let results = body["results"].as_array().expect("results");
    let ids = results
        .iter()
        .map(|result| result["id"].as_str().expect("id"))
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            waiting.as_str(),
            &running,
            &completed,
            &failed,
            &asking,
            "task-missing"
        ]
    );
    assert_eq!(results[2]["state"], "completed", "saving starts nothing");
    assert_eq!(results[3]["state"], "failed", "saving starts nothing");
    assert_eq!(results[4]["next"][0]["tool"], "reply", "{body}");
    assert_eq!(
        results[4]["next"][0]["arguments"],
        json!({ "taskId": asking, "answer": "<answer>" })
    );
    assert!(
        results[5]["error"]
            .as_str()
            .is_some_and(|error| error.contains("unknown task")),
        "{body}"
    );

    broker.open_gate();
    broker.wait_for_shipped(&waiting, RULE).await;
    broker.wait_for_shipped(&running, RULE).await;
}

#[tokio::test]
async fn a_saved_instruction_rides_the_next_resume_and_only_that_one() {
    let broker = Broker::new();
    let done = broker.settled("quick", "Done", "completed").await;

    let body = broker
        .call("instruct", json!({ "taskId": done, "instruction": RULE }))
        .await;
    assert_eq!(outcomes(&body), ["saved"], "{body}");
    assert_eq!(broker.inspect(&done).await["state"], "completed");

    let resumed = broker.call("resume", json!({ "taskId": done })).await;
    assert!(resumed.get("error").is_none(), "{resumed}");
    broker.wait_for_shipped(&done, RULE).await;

    let again = broker.call("resume", json!({ "taskId": done })).await;
    let error = again["error"].as_str().unwrap_or_default();
    assert!(error.contains("only with an instruction"), "{again}");
    assert!(error.contains(&format!("\"taskId\":\"{done}\"")), "{again}");
}

#[tokio::test]
async fn clearing_the_queue_drops_what_was_saved() {
    let broker = Broker::new();
    let done = broker.settled("quick", "Done", "completed").await;
    broker
        .call("instruct", json!({ "taskId": done, "instruction": RULE }))
        .await;

    broker
        .call("resume", json!({ "taskId": done, "queue": "clear" }))
        .await;

    let resumed = broker.call("resume", json!({ "taskId": done })).await;
    assert!(
        resumed["error"]
            .as_str()
            .is_some_and(|error| error.contains("only with an instruction")),
        "{resumed}"
    );
}

#[tokio::test]
async fn now_starts_a_stopped_task_with_the_instruction() {
    let broker = Broker::new();
    let failed = broker.settled("failing", "Failed", "failed").await;
    let done = broker.settled("quick", "Done", "completed").await;

    let body = broker
        .call(
            "instruct",
            json!({ "taskId": [done, failed], "instruction": RULE, "now": true }),
        )
        .await;

    assert_eq!(outcomes(&body), ["resumed", "resumed"], "{body}");
    broker.wait_for_shipped(&done, RULE).await;
    broker.wait_for(&failed, "failed").await;
    let shipped = broker.inspect(&failed).await["shippedPrompt"].clone();
    assert!(
        shipped
            .as_str()
            .is_some_and(|shipped| shipped.contains(RULE)),
        "{shipped}"
    );
}

#[tokio::test]
async fn steering_a_stopped_task_names_the_instruct_call_that_works() {
    let broker = Broker::new();
    let done = broker.settled("quick", "Done", "completed").await;

    let body = broker
        .call("steer", json!({ "taskId": done, "instruction": RULE }))
        .await;

    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("only a running task can be steered"),
        "{body}"
    );
    assert_eq!(body["next"][0]["tool"], "instruct", "{body}");
    assert_eq!(
        body["next"][0]["arguments"],
        json!({ "taskId": [done], "instruction": RULE })
    );
}
