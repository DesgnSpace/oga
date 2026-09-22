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

/// What a configurable agent advertises: a model, and an effort only for the
/// model that has one, so a choice changes what the next one offers.
fn config_options(model: &str, effort: &str) -> Value {
    let mut options = vec![json!({
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": model,
        "options": [{"value": "fast", "name": "Fast"}, {"value": "deep", "name": "Deep"}],
    })];
    if model == "deep" {
        options.push(json!({
            "id": "effort",
            "name": "Effort",
            "category": "thought_level",
            "type": "select",
            "currentValue": effort,
            "options": [{"value": "high", "name": "High"}, {"value": "default", "name": "Default"}],
        }));
    }
    Value::Array(options)
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

    let (mut model, mut effort, mut changes) = ("fast".to_owned(), "default".to_owned(), 0);
    let mut prompts = 0;
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
            "session/new" if mode == "configurable" => result(
                &id,
                json!({"sessionId": SESSION, "configOptions": config_options(&model, &effort)}),
            ),
            "session/new" => result(&id, json!({"sessionId": SESSION})),
            "session/load" | "session/resume" => result(&id, json!({})),
            "session/set_config_option" => {
                let value = message["params"]["value"].as_str().unwrap_or_default();
                match message["params"]["configId"].as_str() {
                    Some("model") => model = value.to_owned(),
                    Some("effort") => effort = value.to_owned(),
                    _ => {}
                }
                changes += 1;
                result(
                    &id,
                    json!({"configOptions": config_options(&model, &effort)}),
                );
            }
            "session/prompt" if mode == "configurable" => {
                chunk(&format!("model={model} effort={effort} changes={changes}"));
                result(&id, json!({"stopReason": "end_turn"}));
            }
            // Fails its first turn the way OpenCode does on a malformed tool
            // call, then answers the next prompt in the same session.
            "session/prompt" if mode == "error-once" => {
                prompts += 1;
                if prompts == 1 {
                    failure(&id, -32603, "Internal error: Expected 'id' to be a string.");
                } else {
                    chunk("carried on");
                    result(&id, json!({"stopReason": "end_turn"}));
                }
            }
            "session/prompt" => prompt(&mode, &id, &mut lines),
            "session/cancel" => {}
            other => failure(&id, -32601, &format!("unknown method: {other}")),
        }
    }
}
