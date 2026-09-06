use std::{
    env, fs,
    io::{self, Write},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

fn emit(value: Value) {
    println!("{value}");
    io::stdout().flush().expect("stdout is writable");
}

fn delay() {
    let millis = env::var("FAKE_PROVIDER_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    if millis > 0 {
        thread::sleep(Duration::from_millis(millis));
    }
}

fn basic() {
    emit(json!({"type":"system","session_id":"fake-session"}));
    emit(json!({
        "type":"result",
        "session_id":"fake-session",
        "result":"fake provider completed",
        "usage":{"input_tokens":11,"output_tokens":7},
        "total_cost_usd":0.25,
        "num_turns":1
    }));
}

fn pi() {
    emit(json!({"type":"session","id":"fake-pi-session"}));
    emit(json!({
        "type":"message_end",
        "message":{"role":"assistant","content":[{"type":"text","text":"done"}]},
        "usage":{"input":3,"output":2,"cost":{"total":0.05}}
    }));
}

fn burst() {
    let count = env::var("FAKE_PROVIDER_LINES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100);
    let text = env::var("FAKE_PROVIDER_TEXT").unwrap_or_else(|_| "x".repeat(128));
    for _ in 0..count {
        emit(json!({"type":"event","text":text}));
    }
    basic();
}

fn codex_stream() {
    emit(json!({"type":"thread.started","thread_id":"fake-codex-session"}));
    delay();
    emit(json!({
        "type":"item.started",
        "item": {
            "id":"item-file-change",
            "type":"file_change",
            "changes":[{"path":"src/main.rs","kind":"update"}],
            "status":"in_progress"
        }
    }));
    delay();
    emit(json!({
        "type":"item.completed",
        "item": {
            "id":"item-file-change",
            "type":"file_change",
            "changes":[{"path":"src/main.rs","kind":"update"}],
            "status":"completed"
        }
    }));
    emit(json!({
        "type":"turn.completed",
        "usage":{"input_tokens":2,"cached_input_tokens":1,"output_tokens":3}
    }));
}

fn child() -> io::Result<()> {
    let mut child = Command::new("sh")
        .args(["-c", "trap '' INT TERM; while true; do sleep 1; done"])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    if let Some(path) = env::var_os("FAKE_PROVIDER_CHILD_PID_FILE") {
        fs::write(path, child.id().to_string())?;
    }
    emit(json!({"type":"system","session_id":"fake-child-session"}));
    while child.try_wait()?.is_none() {
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let mode = env::args().nth(1).unwrap_or_else(|| "basic".into());
    delay();
    match mode.as_str() {
        "basic" => basic(),
        "pi" => pi(),
        "burst" => burst(),
        "codex-stream" => codex_stream(),
        "malformed" => {
            println!("not-json");
            io::stdout().flush()?;
            basic();
        }
        "oversized" => {
            let size = env::var("FAKE_PROVIDER_SIZE")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(128 * 1024);
            emit(json!({"type":"event","text":"x".repeat(size)}));
            basic();
        }
        "stderr" => {
            eprintln!("fake provider warning");
            basic();
        }
        "sleep" => {
            let millis = env::var("FAKE_PROVIDER_SLEEP_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(10_000);
            thread::sleep(Duration::from_millis(millis));
            basic();
        }
        "probe-env" => {
            eprintln!(
                "FAKE_PROVIDER_PROBE={}",
                env::var("FAKE_PROVIDER_PROBE").unwrap_or_else(|_| "<absent>".into())
            );
            basic();
        }
        "child" => child()?,
        other => {
            eprintln!("unknown fake provider mode: {other}");
            std::process::exit(2);
        }
    }
    Ok(())
}
