//! `oga watch` follows a task to its ending even when the event socket drops.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::Path,
    process::{Command, Output},
    thread,
    time::Duration,
};

use oga_domain::{Profile, Provider, Task, TaskKind, TaskState};
use oga_store::Store;
use tempfile::TempDir;

const TASK_ID: &str = "0b8263f2-0000-4000-8000-000000000000";
const HELLO: &str =
    r#"{"hello":{"version":"test","mcpContractVersion":1,"initialCursor":0,"streamFloor":0}}"#;

fn seed_running_task(database: &Path) -> Store {
    let store = Store::open_writable(database).expect("store");
    store
        .repositories()
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
    store
        .repositories()
        .tasks()
        .insert(&Task {
            id: TASK_ID.into(),
            kind: Some(TaskKind::Delegated),
            profile_id: "main".into(),
            model: "sonnet".into(),
            prompt: "work".into(),
            cwd: "/project".into(),
            state: TaskState::Running,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            ..Task::default()
        })
        .expect("task");
    store
}

/// An event socket that greets the watcher and then hangs up, as a broker
/// does when it restarts.
fn socket_that_hangs_up(path: &Path) -> thread::JoinHandle<()> {
    let listener = UnixListener::bind(path).expect("socket");
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("watcher connected");
        let mut subscribe = String::new();
        BufReader::new(stream.try_clone().expect("clone"))
            .read_line(&mut subscribe)
            .expect("subscribe frame");
        let mut stream = stream;
        writeln!(stream, "{HELLO}").expect("hello");
    })
}

fn watch(home: &Path, socket: &Path, timeout: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oga-cli"))
        .args(["watch", TASK_ID, "--timeout", timeout])
        .env("HOME", home)
        .env("OGA_DB", home.join("oga.db"))
        .env("OGA_SOCK", socket)
        .env_remove("OGA_TASK_ID")
        .output()
        .expect("oga ran")
}

#[test]
fn watch_keeps_following_a_task_after_the_event_socket_drops() {
    let home = TempDir::new().expect("temporary home");
    let store = seed_running_task(&home.path().join("oga.db"));
    let socket = home.path().join("e.sock");
    let hung_up = socket_that_hangs_up(&socket);

    let finisher = thread::spawn(move || {
        thread::sleep(Duration::from_millis(1_500));
        store
            .transaction(|tx| {
                tx.execute(
                    "UPDATE tasks SET state='completed',updated_at='2026-01-01T00:01:00.000Z' WHERE id=?",
                    [TASK_ID],
                )?;
                Ok(())
            })
            .expect("task completes");
        store
    });

    let output = watch(home.path(), &socket, "30s");
    hung_up.join().expect("socket thread");
    let _store = finisher.join().expect("finisher thread");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(r#""type":"settled""#) && stdout.contains(r#""state":"completed""#),
        "watch should report the ending, got stdout {stdout:?}, stderr {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn watch_that_runs_out_of_time_says_so_instead_of_blaming_the_socket() {
    let home = TempDir::new().expect("temporary home");
    let _store = seed_running_task(&home.path().join("oga.db"));
    let socket = home.path().join("e.sock");
    // The server greets the watcher and then says nothing, so only the
    // deadline can end the watch.
    let listener = UnixListener::bind(&socket).expect("socket");
    let silent = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("watcher connected");
        let mut subscribe = String::new();
        BufReader::new(stream.try_clone().expect("clone"))
            .read_line(&mut subscribe)
            .expect("subscribe frame");
        let mut stream = stream;
        writeln!(stream, "{HELLO}").expect("hello");
        thread::sleep(Duration::from_secs(4));
    });

    let output = watch(home.path(), &socket, "1s");
    silent.join().expect("socket thread");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("event socket lost"),
        "a timeout is not a lost socket: {stderr:?}"
    );
}

#[test]
fn watch_reports_a_task_that_ended_while_the_event_socket_said_nothing() {
    let home = TempDir::new().expect("temporary home");
    let store = seed_running_task(&home.path().join("oga.db"));
    let socket = home.path().join("e.sock");
    let listener = UnixListener::bind(&socket).expect("socket");
    let silent = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("watcher connected");
        let mut subscribe = String::new();
        BufReader::new(stream.try_clone().expect("clone"))
            .read_line(&mut subscribe)
            .expect("subscribe frame");
        let mut stream = stream;
        writeln!(stream, "{HELLO}").expect("hello");
        thread::sleep(Duration::from_secs(4));
    });
    thread::sleep(Duration::from_millis(300));
    store
        .transaction(|tx| {
            tx.execute(
                "UPDATE tasks SET state='completed',updated_at='2026-01-01T00:01:00.000Z' WHERE id=?",
                [TASK_ID],
            )?;
            Ok(())
        })
        .expect("task completes");

    let output = watch(home.path(), &socket, "1s");
    silent.join().expect("socket thread");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(r#""state":"completed""#),
        "a finished task is reported even when no event said so, got {stdout:?}"
    );
    assert_eq!(output.status.code(), Some(0));
}
