//! An ACP agent that answers from a script, so protocol behaviour can be
//! tested without an account or a model.
//!
//! The mode comes from argv. Frames are hand-written JSON rather than the
//! schema types, so the tests exercise the client against the wire format.

use std::{
    env,
    io::{self, BufRead, Write},
};

use serde_json::{Value, json};

const SESSION: &str = "fake-acp-session";

fn emit(value: &Value) {
    println!("{value}");
    io::stdout().flush().expect("stdout is writable");
}

fn result(id: &Value, result: Value) {
    emit(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

fn failure(id: &Value, code: i32, message: &str) {
    emit(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message},
    }));
}

fn chunk(text: &str) {
    emit(&json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": SESSION,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": text},
            },
        },
    }));
}

fn initialize(mode: &str, id: &Value) {
    match mode {
        "auth" => failure(id, -32000, "sign in to this agent first"),
        "old-protocol" => result(id, json!({"protocolVersion": 0})),
        _ => result(
            id,
            json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": true,
                    "sessionCapabilities": {"resume": {}},
                },
                "agentInfo": {"name": "fake-agent", "version": "1.0.0"},
            }),
        ),
    }
}

/// Answers one prompt, in whichever way the mode asks for.
fn prompt(mode: &str, id: &Value, lines: &mut impl Iterator<Item = String>) {
    match mode {
        "exit-after-prompt" => std::process::exit(0),
        "malformed" => {
            println!("not json at all");
            let size = env::var("FAKE_AGENT_OVERSIZE_BYTES")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            if size > 0 {
                emit(
                    &json!({"jsonrpc": "2.0", "method": "session/update", "params": {
                        "sessionId": SESSION,
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": {"type": "text", "text": "x".repeat(size)},
                        },
                    }}),
                );
            }
            chunk("survived");
            result(id, json!({"stopReason": "end_turn"}));
        }
        "cancel" => {
            chunk("working");
            // The turn only ends once the client says so, which is what makes
            // cancellation observable rather than raced.
            for line in lines.by_ref() {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if message["method"] == "session/cancel" {
                    break;
                }
            }
            result(id, json!({"stopReason": "cancelled"}));
        }
        "permission" => {
            ask(
                "session/request_permission",
                json!({
                    "sessionId": SESSION,
                    "toolCall": {"toolCallId": "tool-1", "title": "Run a command"},
                    "options": [
                        {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                        {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
                    ],
                }),
                lines,
                |answer| {
                    chunk(&format!("permission: {answer}"));
                },
            );
            result(id, json!({"stopReason": "end_turn"}));
        }
        "read-file" => {
            ask(
                "fs/read_text_file",
                json!({"sessionId": SESSION, "path": "/tmp/fake.txt"}),
                lines,
                |answer| {
                    chunk(&format!("read: {answer}"));
                },
            );
            result(id, json!({"stopReason": "end_turn"}));
        }
        "terminal" => {
            ask(
                "terminal/create",
                json!({"sessionId": SESSION, "command": "echo hi"}),
                lines,
                |answer| {
                    chunk(&format!("terminal: {answer}"));
                },
            );
            result(id, json!({"stopReason": "end_turn"}));
        }
        _ => {
            chunk("hello");
            chunk(" world");
            result(id, json!({"stopReason": "end_turn"}));
        }
    }
}

/// Calls back into the client and reports whichever answer came back.
fn ask(
    method: &str,
    params: Value,
    lines: &mut impl Iterator<Item = String>,
    report: impl FnOnce(String),
) {
    emit(&json!({"jsonrpc": "2.0", "id": 9000, "method": method, "params": params}));
    for line in lines.by_ref() {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message["id"] != json!(9000) {
            continue;
        }
        let answer = match message.get("error") {
            Some(error) => format!("error {}", error["code"]),
            None => message["result"].to_string(),
        };
        report(answer);
        return;
    }
    report("no answer".into());
}

fn main() {
    let mode = env::args().nth(1).unwrap_or_else(|| "turn".into());
    if mode == "silent" {
        // Started, but never speaks: the handshake has to time out on its own.
        std::thread::sleep(std::time::Duration::from_secs(600));
        return;
    }
    if mode == "exit-immediately" {
        eprintln!("fake agent could not start");
        std::process::exit(3);
    }
    if mode == "noisy" {
        eprintln!("fake agent warning: deprecated flag");
    }

    let stdin = io::stdin();
    let mut lines = stdin.lock().lines().map_while(Result::ok);
    while let Some(line) = lines.next() {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(method) = message["method"].as_str() else {
            continue;
        };
        let id = message["id"].clone();
        match method {
            "initialize" => initialize(&mode, &id),
            "session/new" => result(&id, json!({"sessionId": SESSION})),
            "session/load" | "session/resume" => result(&id, json!({})),
            "session/prompt" => prompt(&mode, &id, &mut lines),
            "session/cancel" => {}
            other => failure(&id, -32601, &format!("unknown method: {other}")),
        }
    }
}
