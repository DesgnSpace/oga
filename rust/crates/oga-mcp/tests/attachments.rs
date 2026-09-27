use std::{collections::BTreeMap, sync::Arc};

use axum::{body::Body, http::StatusCode};
use http_body_util::BodyExt;
use oga_domain::{Profile, Provider};
use oga_http::HttpState;
use oga_mcp::McpServer;
use oga_store::Store;
use serde_json::{Value, json};
use tower::ServiceExt;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

struct Broker {
    directory: tempfile::TempDir,
    server: McpServer,
}

impl Broker {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(Store::open_writable(directory.path().join("oga.db")).expect("store"));
        store
            .repositories()
            .profiles()
            .insert(
                &Profile {
                    id: "main".into(),
                    label: "Main".into(),
                    provider: Provider::Antigravity,
                    default_model: "fake".into(),
                    enabled: true,
                    env: BTreeMap::new(),
                    capabilities: Vec::new(),
                    command: None,
                },
                "2026-01-01T00:00:00.000Z",
            )
            .expect("profile insert");
        store
            .repositories()
            .settings()
            .put(
                &oga_config::canonical_cwd(oga_config::global_cwd())
                    .display()
                    .to_string(),
                oga_config::MODEL_SETTINGS_KEY,
                &json!({ "profiles": { "main": { "modelEnabled": { "fake": true } } } })
                    .to_string(),
                "2026-01-01T00:00:00.000Z",
            )
            .expect("model settings");
        Self {
            directory,
            server: McpServer::new(HttpState::new(store)),
        }
    }

    fn project(&self) -> std::path::PathBuf {
        let project = self.directory.path().join("project");
        std::fs::create_dir_all(project.join("refs")).expect("project directory");
        project
    }

    /// Held until a far-off start so no provider runs.
    async fn delegate(&self, cwd: &std::path::Path, attachments: Value) -> String {
        let response = self
            .server
            .handle_value(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": "delegate", "arguments": {
                    "prompt": "restyle the button",
                    "cwd": cwd.display().to_string(),
                    "tldr": "restyle the button",
                    "title": "Button",
                    "profile": "main",
                    "model": "fake",
                    "startAt": "2099-01-01T00:00:00Z",
                    "attachments": attachments,
                }}
            }))
            .await
            .expect("delegate response");
        assert_ne!(response["result"]["isError"], true, "{response}");
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("delegate text");
        let view: Value = serde_json::from_str(text).expect("task view");
        view["id"].as_str().expect("task id").to_owned()
    }

    async fn get(&self, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
        let response = oga_http::router(self.server.state().clone())
            .oneshot(
                axum::http::Request::get(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .map(|value| value.to_str().expect("content type").to_owned());
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .to_vec();
        (status, content_type, body)
    }
}

#[tokio::test]
async fn delegated_attachments_reach_the_task_view_and_only_they_are_served() {
    let broker = Broker::new();
    let project = broker.project();
    let image = project.join("refs/button.png");
    std::fs::write(&image, PNG).expect("image fixture");
    std::fs::write(project.join("refs/notes.md"), "# Notes").expect("file fixture");
    std::fs::write(project.join("refs/secret.png"), PNG).expect("unattached fixture");

    let task_id = broker
        .delegate(
            &project,
            json!([image.display().to_string(), "refs/notes.md"]),
        )
        .await;

    let (status, _, body) = broker.get(&format!("/api/tasks/{task_id}")).await;
    assert_eq!(status, StatusCode::OK);
    let task: Value = serde_json::from_slice(&body).expect("task JSON");
    assert_eq!(
        task["attachments"],
        json!([image.display().to_string(), "refs/notes.md"])
    );

    let (status, content_type, body) = broker
        .get(&format!("/api/tasks/{task_id}/attachments/0"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("image/png"));
    assert_eq!(body, PNG);

    // A relative attachment reads from the delegating directory.
    let (status, content_type, body) = broker
        .get(&format!("/api/tasks/{task_id}/attachments/1"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("application/octet-stream"));
    assert_eq!(body, b"# Notes");

    // Nothing past the attached list, and no path, however it is spelled,
    // names a file.
    for uri in [
        format!("/api/tasks/{task_id}/attachments/2"),
        format!("/api/tasks/{task_id}/attachments/refs%2Fsecret.png"),
        format!("/api/tasks/{task_id}/attachments/..%2F..%2Fetc%2Fpasswd"),
        format!(
            "/api/tasks/{task_id}/attachments/{}",
            project.join("refs/secret.png").display()
        ),
        "/api/tasks/unknown/attachments/0".to_owned(),
    ] {
        let (status, _, body) = broker.get(&uri).await;
        assert!(
            status.is_client_error(),
            "{uri} answered {status}: {}",
            String::from_utf8_lossy(&body)
        );
        assert_ne!(body, PNG, "{uri}");
    }
}

#[tokio::test]
async fn an_attachment_gone_from_disk_is_reported_missing() {
    let broker = Broker::new();
    let project = broker.project();
    let image = project.join("refs/button.png");
    std::fs::write(&image, PNG).expect("image fixture");
    let task_id = broker
        .delegate(&project, json!([image.display().to_string()]))
        .await;
    std::fs::remove_file(&image).expect("remove image");

    let (status, _, _) = broker
        .get(&format!("/api/tasks/{task_id}/attachments/0"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
