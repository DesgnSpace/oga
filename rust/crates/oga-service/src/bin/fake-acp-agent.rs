//! An ACP agent that answers from a script, so the task lifecycle can be run
//! over ACP without an account or a model.
//!
//! `fake-acp-agent <mode> <log>`: every frame it receives is appended to `log`
//! as one JSON line, after a first line recording the environment it started
//! with, so a test can count prompts and check what reached the agent.
//!
//! A mode starting `opencode` answers the way `opencode acp` does: it reports
//! itself as OpenCode, names its session the way OpenCode names sessions, and
//! offers the model and effort as session settings. The rest of the mode says
//! how its turns go, or `no-model` for an agent that does not offer the
//! model a test asks for.

use std::{
    env,
    fs::OpenOptions,
    io::{self, BufRead, Write},
};

use serde_json::{Value, json};

const SESSION: &str = "acp-session-1";

fn emit(value: &Value) {
    println!("{value}");
    io::stdout().flush().expect("stdout is writable");
}

fn log(path: &str, value: &Value) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("log opens");
    writeln!(file, "{value}").expect("log is writable");
}

fn update(session: &str, update: Value) {
    emit(&json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": session, "update": update},
    }));
}

fn chunk(session: &str, text: &str) {
    update(
        session,
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}),
    );
}

/// OpenCode's settings: a model, and an effort only for the model that has
/// variants, so choosing the model changes what the effort offers.
fn opencode_settings(mode: &str, model: &str, effort: &str) -> Value {
    let models = if mode.ends_with("no-model") {
        json!([{"value": "opencode/big-pickle", "name": "Big Pickle"}])
    } else {
        json!([
            {"value": "opencode/big-pickle", "name": "Big Pickle"},
            {"value": "opencode/deep", "name": "Deep"},
        ])
    };
    let mut settings = vec![json!({
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": model,
        "options": models,
    })];
    if model == "opencode/deep" {
        settings.push(json!({
            "id": "effort",
            "name": "Effort",
            "category": "thought_level",
            "type": "select",
            "currentValue": effort,
            "options": [
                {"value": "high", "name": "High"},
                {"value": "max", "name": "Max"},
                {"value": "default", "name": "Default"},
            ],
        }));
    }
    Value::Array(settings)
}

fn capabilities(mode: &str) -> Value {
    match mode {
        "no-http" => json!({"loadSession": true, "sessionCapabilities": {"resume": {}}}),
        "load-only" => json!({"loadSession": true, "mcpCapabilities": {"http": true}}),
        _ => json!({
            "loadSession": true,
            "mcpCapabilities": {"http": true},
            "sessionCapabilities": {"resume": {}},
        }),
    }
}

/// How many prompts this agent's script has already been given, counted from
/// the log every run of it appends to. Lets a mode report session totals that
/// grow across turns the way a real agent's do.
fn prompts_logged(log_path: &str) -> u64 {
    std::fs::read_to_string(log_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["received"]["method"] == "session/prompt")
        .count()
        .max(1) as u64
}

/// Asks the client for permission and waits for its answer.
fn ask_permission(
    session: &str,
    id: u64,
    path: &str,
    lines: &mut impl Iterator<Item = String>,
    log_path: &str,
) -> String {
    emit(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/request_permission",
        "params": {
            "sessionId": session,
            "toolCall": {
                "toolCallId": format!("edit-{id}"),
                "title": format!("Edit {path}"),
                "kind": "edit",
                "locations": [{"path": path}],
            },
            "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
            ],
        },
    }));
    for line in lines.by_ref() {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        log(log_path, &json!({"received": message}));
        if message["id"] != json!(id) {
            continue;
        }
        return message["result"]["outcome"]["optionId"]
            .as_str()
            .unwrap_or("no answer")
            .to_owned();
    }
    "no answer".into()
}

fn prompt(
    mode: &str,
    id: &Value,
    params: &Value,
    lines: &mut impl Iterator<Item = String>,
    log_path: &str,
) {
    let session = params["sessionId"].as_str().unwrap_or(SESSION).to_owned();
    match mode {
        "exit-after-prompt" => std::process::exit(0),
        "hang" => {
            for line in lines.by_ref() {
                log(log_path, &json!({"received": line}));
            }
            return;
        }
        "permission" => {
            let cwd = env::current_dir().expect("cwd").display().to_string();
            let inside = ask_permission(
                &session,
                9001,
                &format!("{cwd}/src/lib.rs"),
                lines,
                log_path,
            );
            let outside = ask_permission(
                &session,
                9002,
                &format!("{cwd}/Cargo.toml"),
                lines,
                log_path,
            );
            chunk(
                &session,
                &format!("inside: {inside}, outside: {outside}\nOGA_RESULT: completed"),
            );
        }
        "usage" => {
            let turn = prompts_logged(log_path);
            update(
                &session,
                json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": format!("edit-{turn}"),
                    "title": "Edit src/lib.rs",
                    "kind": "edit",
                    "status": "completed",
                    "content": [{"type": "diff", "path": "src/lib.rs", "oldText": "old", "newText": "new"}],
                    "locations": [{"path": "src/lib.rs"}],
                }),
            );
            for (used, amount) in [
                (12_000 * turn, 0.5 * turn as f64),
                (24_000 * turn, 1.25 * turn as f64),
            ] {
                update(
                    &session,
                    json!({
                        "sessionUpdate": "usage_update",
                        "used": used,
                        "size": 200_000,
                        "cost": {"amount": amount, "currency": "USD"},
                    }),
                );
            }
            chunk(&session, "OGA_RESULT: completed");
        }
        _ => {
            chunk(&session, "Looking at the task");
            update(
                &session,
                json!({"sessionUpdate": "tool_call", "toolCallId": "read-1", "title": "Read README", "kind": "read"}),
            );
            chunk(&session, "Done.\n");
            chunk(&session, "OGA_RESULT: completed");
        }
    }
    emit(&json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "end_turn"}}));
}

fn main() {
    let mut args = env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "turn".into());
    let log_path = args.next().expect("a log path");
    log(
        &log_path,
        &json!({
            "mode": mode,
            "env": {
                "OGA_TASK_ID": env::var("OGA_TASK_ID").ok(),
                "CLAUDE_CONFIG_DIR": env::var("CLAUDE_CONFIG_DIR").ok(),
                "XDG_DATA_HOME": env::var("XDG_DATA_HOME").ok(),
            },
        }),
    );
    let opencode = mode.starts_with("opencode");
    let session_id = if opencode { "ses_acp1" } else { SESSION };
    let agent_name = if opencode && !mode.ends_with("renamed") {
        "OpenCode"
    } else {
        "fake-acp-agent"
    };
    let turns = mode.strip_prefix("opencode-").unwrap_or(&mode).to_owned();
    let (mut model, mut effort) = ("opencode/big-pickle".to_owned(), "default".to_owned());

    let stdin = io::stdin();
    let mut lines = stdin.lock().lines().map_while(Result::ok);
    while let Some(line) = lines.next() {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        log(&log_path, &json!({"received": message}));
        let Some(method) = message["method"].as_str() else {
            continue;
        };
        let id = message["id"].clone();
        let params = message["params"].clone();
        match method {
            "initialize" if mode == "auth" => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32000, "message": "sign in first"},
            })),
            "initialize" => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": 1,
                    "agentCapabilities": capabilities(&mode),
                    "agentInfo": {"name": agent_name, "version": "2.1.0"},
                },
            })),
            "session/new" if opencode => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": session_id,
                    "configOptions": opencode_settings(&mode, &model, &effort),
                },
            })),
            "session/new" => {
                emit(&json!({"jsonrpc": "2.0", "id": id, "result": {"sessionId": session_id}}))
            }
            "session/set_config_option" => {
                let value = params["value"].as_str().unwrap_or_default().to_owned();
                if params["configId"] == "model" {
                    model = value;
                } else {
                    effort = value;
                }
                emit(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"configOptions": opencode_settings(&mode, &model, &effort)},
                }));
            }
            "session/load" => {
                let session = params["sessionId"].as_str().unwrap_or(SESSION);
                for text in [
                    "an earlier question",
                    "an earlier answer",
                    "OGA_RESULT: completed",
                ] {
                    chunk(session, text);
                }
                emit(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
            }
            "session/resume" if opencode => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"configOptions": opencode_settings(&mode, &model, &effort)},
            })),
            "session/resume" => emit(&json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            "session/prompt" => prompt(&turns, &id, &params, &mut lines, &log_path),
            "session/cancel" => {}
            other => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("unknown method: {other}")},
            })),
        }
    }
}
