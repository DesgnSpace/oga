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
//!
//! A mode starting `opencode2` answers the way `opencode2 acp` build
//! `0.0.0-beta-18999` does: as OpenCode does, under that version. `next` is a
//! build Oga was not verified against.
//!
//! A mode starting `claude` answers the way `claude-agent-acp` does: it reports
//! itself as that adapter, names its session the way Claude Code names one, and
//! offers the effort as a session setting. `no-effort` is the adapter on a model
//! with no effort levels to offer.
//!
//! A mode starting `codex` answers the way `codex-acp` 1.12.0 does: it reports
//! itself as that adapter, names its session the way Codex names a thread,
//! starts on the model and effort `CODEX_CONFIG` names, and offers its approval
//! and sandbox preset as a session setting. `next` is a release Oga was not
//! verified against, and `no-full-access` an adapter without that preset.

use std::{
    env,
    fs::OpenOptions,
    io::{self, BufRead, Write},
};

use serde_json::{Value, json};

const SESSION: &str = "acp-session-1";
/// Claude Code names a session with a UUID, and `claude-agent-acp` opens the
/// session under that same id.
const CLAUDE_SESSION: &str = "9f3c0c10-0e2a-4b47-8f1f-3f0c9a2b7c51";
/// Codex names a thread with a UUID, and `codex-acp` opens the session under
/// that same id.
const CODEX_THREAD: &str = "019a4c1e-7b2d-7c30-9e41-5d6f7a8b9c0d";

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

/// `claude-agent-acp`'s settings: an effort ladder for a model that has one,
/// and the CLI's short model aliases, which Oga leaves alone.
fn claude_settings(mode: &str, effort: &str) -> Value {
    let mut settings = vec![json!({
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": "opus",
        "options": [
            {"value": "default", "name": "Default"},
            {"value": "opus", "name": "Opus"},
            {"value": "sonnet", "name": "Sonnet"},
        ],
    })];
    if !mode.ends_with("no-effort") {
        settings.push(json!({
            "id": "effort",
            "name": "Effort",
            "category": "thought_level",
            "type": "select",
            "currentValue": effort,
            "options": [
                {"value": "default", "name": "Default"},
                {"value": "medium", "name": "Medium"},
                {"value": "high", "name": "High"},
                {"value": "max", "name": "Max"},
            ],
        }));
    }
    Value::Array(settings)
}

/// `codex-acp`'s settings: its approval and sandbox preset, Codex's model
/// catalogue with the session's own model among it, and that model's efforts.
fn codex_settings(mode: &str, model: &str, effort: &str, access: &str) -> Value {
    let mut presets = vec![
        json!({"value": "read-only", "name": "Ask for approval"}),
        json!({"value": "agent", "name": "Approve for me"}),
    ];
    if !mode.ends_with("no-full-access") {
        presets.push(json!({"value": "agent-full-access", "name": "Full access"}));
    }
    let mut models = vec![
        json!({"value": "gpt-5.5", "name": "5.5"}),
        json!({"value": "gpt-5.3-codex", "name": "5.3 Codex"}),
    ];
    if !["gpt-5.5", "gpt-5.3-codex"].contains(&model) {
        models.insert(0, json!({"value": model, "name": model}));
    }
    json!([
        {
            "id": "mode",
            "name": "Mode",
            "category": "mode",
            "type": "select",
            "currentValue": access,
            "options": presets,
        },
        {
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": model,
            "options": models,
        },
        {
            "id": "reasoning_effort",
            "name": "Reasoning effort",
            "category": "thought_level",
            "type": "select",
            "currentValue": effort,
            "options": [
                {"value": "low", "name": "Low"},
                {"value": "medium", "name": "Medium"},
                {"value": "high", "name": "High"},
                {"value": "xhigh", "name": "Xhigh"},
            ],
        },
    ])
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
                "ANTHROPIC_MODEL": env::var("ANTHROPIC_MODEL").ok(),
                "CLAUDE_CONFIG_DIR": env::var("CLAUDE_CONFIG_DIR").ok(),
                "XDG_DATA_HOME": env::var("XDG_DATA_HOME").ok(),
                "CODEX_HOME": env::var("CODEX_HOME").ok(),
                "CODEX_CONFIG": env::var("CODEX_CONFIG").ok(),
                "DISABLE_MCP_CONFIG_FILTERING": env::var("DISABLE_MCP_CONFIG_FILTERING").ok(),
            },
        }),
    );
    let opencode = mode.starts_with("opencode");
    let claude = mode.starts_with("claude");
    let codex = mode.starts_with("codex");
    let session_id = match (opencode, claude, codex) {
        (true, _, _) => "ses_acp1",
        (_, true, _) => CLAUDE_SESSION,
        (_, _, true) => CODEX_THREAD,
        _ => SESSION,
    };
    let renamed = mode.ends_with("renamed");
    let agent_name = match (opencode, claude, codex) {
        (true, _, _) if !renamed => "OpenCode",
        (_, true, _) if !renamed => "@agentclientprotocol/claude-agent-acp",
        (_, _, true) if !renamed => "@agentclientprotocol/codex-acp",
        _ => "fake-acp-agent",
    };
    let opencode2 = mode.starts_with("opencode2");
    let version = match (codex, opencode2, mode.ends_with("next")) {
        (true, _, true) => "1.13.0",
        (true, _, false) => "1.12.0",
        (_, true, true) => "0.0.0-beta-19000",
        (_, true, false) => "0.0.0-beta-18999",
        _ => "2.1.0",
    };
    let turns = mode
        .strip_prefix("opencode2-")
        .or_else(|| mode.strip_prefix("opencode-"))
        .or_else(|| mode.strip_prefix("claude-"))
        .or_else(|| mode.strip_prefix("codex-"))
        .unwrap_or(&mode)
        .to_owned();
    let (mut model, mut effort) = ("opencode/big-pickle".to_owned(), "default".to_owned());
    let mut access = "agent".to_owned();
    if codex {
        let config: Value = env::var("CODEX_CONFIG")
            .ok()
            .and_then(|config| serde_json::from_str(&config).ok())
            .unwrap_or_default();
        model = config["model"].as_str().unwrap_or("gpt-5.5").to_owned();
        effort = config["model_reasoning_effort"]
            .as_str()
            .unwrap_or("medium")
            .to_owned();
    }

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
                    "agentInfo": {"name": agent_name, "version": version},
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
            "session/new" if claude => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": session_id,
                    "configOptions": claude_settings(&mode, &effort),
                },
            })),
            "session/new" if codex => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": session_id,
                    "configOptions": codex_settings(&mode, &model, &effort, &access),
                },
            })),
            "session/new" => {
                emit(&json!({"jsonrpc": "2.0", "id": id, "result": {"sessionId": session_id}}))
            }
            "session/set_config_option" => {
                let value = params["value"].as_str().unwrap_or_default().to_owned();
                match params["configId"].as_str() {
                    Some("model") => model = value,
                    Some("mode") => access = value,
                    _ => effort = value,
                }
                let offered = if claude {
                    claude_settings(&mode, &effort)
                } else if codex {
                    codex_settings(&mode, &model, &effort, &access)
                } else {
                    opencode_settings(&mode, &model, &effort)
                };
                emit(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"configOptions": offered},
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
            "session/resume" if claude => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"configOptions": claude_settings(&mode, &effort)},
            })),
            "session/resume" if codex => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"configOptions": codex_settings(&mode, &model, &effort, &access)},
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
