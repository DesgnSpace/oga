//! Task-id commands find any task the broker holds, however old.

use std::{
    fs,
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

use oga_domain::{Profile, Provider, Task, TaskKind, TaskState};
use oga_store::Store;
use serde_json::Value;
use tempfile::TempDir;

const TASK_COUNT: usize = 2_005;

struct Broker(Child);

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("free port")
        .port()
}

fn task_id(index: usize) -> String {
    format!("{index:08x}-0000-4000-8000-000000000000")
}

fn seed(database: &Path) {
    let store = Store::open_writable(database).expect("store");
    let repositories = store.repositories();
    repositories
        .profiles()
        .insert(
            &Profile {
                id: "main".into(),
                label: "Main".into(),
                provider: Provider::Claude,
                default_model: "sonnet".into(),
                enabled: true,
                env: Default::default(),
                capabilities: Vec::new(),
                command: None,
            },
            "2026-01-01T00:00:00.000Z",
        )
        .expect("profile");
    for index in 0..TASK_COUNT {
        let at = format!(
            "2026-01-01T{:02}:{:02}:{:02}.000Z",
            index / 3_600,
            index / 60 % 60,
            index % 60
        );
        repositories
            .tasks()
            .insert(&Task {
                id: task_id(index),
                kind: Some(TaskKind::Delegated),
                profile_id: "main".into(),
                model: "sonnet".into(),
                prompt: format!("task {index}"),
                cwd: "/project".into(),
                state: if index % 2 == 0 {
                    TaskState::Completed
                } else {
                    TaskState::Failed
                },
                created_at: at.clone(),
                updated_at: at,
                ..Task::default()
            })
            .expect("task");
    }
}

fn start_broker(home: &Path) -> (Broker, u16) {
    let shell = home.join("shell");
    fs::write(
        &shell,
        "#!/bin/sh\nprintf '__OGA_PATH__/usr/bin:/bin__OGA_END__'\n",
    )
    .expect("shell script");
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).expect("shell executable");
    let port = free_port();
    let broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_oga-cli"))
            .args(["serve", "--port", &port.to_string()])
            .env("HOME", home)
            .env("OGA_DB", home.join("oga.db"))
            .env("SHELL", &shell)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("broker started"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "the broker never listened");
        std::thread::sleep(Duration::from_millis(20));
    }
    (broker, port)
}

fn oga(home: &Path, port: u16, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oga-cli"))
        .args(args)
        .env("HOME", home)
        .env("OGA_DB", home.join("oga.db"))
        .env("OGA_PORT", port.to_string())
        .env_remove("OGA_TASK_ID")
        .output()
        .expect("oga ran")
}

fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON output")
}

#[test]
fn task_commands_resolve_ids_older_than_the_newest_two_thousand() {
    let home = TempDir::new().expect("temporary home");
    seed(&home.path().join("oga.db"));
    let (_broker, port) = start_broker(home.path());
    let oldest = task_id(0);

    let by_id = json(&oga(home.path(), port, &["inspect", &oldest]));
    let by_prefix = json(&oga(home.path(), port, &["inspect", &oldest[..8]]));
    let ambiguous = oga(home.path(), port, &["inspect", "0000000"]);
    let unknown = oga(home.path(), port, &["inspect", "ffff"]);
    let listed = json(&oga(home.path(), port, &["tasks", "--archived", "--json"]));
    let failed = json(&oga(
        home.path(),
        port,
        &["tasks", "--state", "failed", "--limit", "2", "--json"],
    ));

    assert_eq!(by_id["id"], oldest);
    assert_eq!(by_id["prompt"], "task 0");
    assert_eq!(by_prefix["id"], oldest);
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("ambiguous task id"));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unknown task: ffff"));
    assert_eq!(listed, Value::Array(Vec::new()));
    let failed = failed.as_array().expect("rows");
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0]["id"], task_id(TASK_COUNT - 2)[..8]);
    assert_eq!(failed[1]["id"], task_id(TASK_COUNT - 4)[..8]);
}
