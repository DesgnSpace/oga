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
//! OpenCode 1 reports version 1.18.31. A mode starting `opencode2` answers
//! the way OpenCode 2.0.1's `acp` does: as OpenCode does, under that version.
//! `next` is a release line Oga was not verified against.
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
//!
//! A mode starting `antigravity` answers the way `agy_acp_server.par` 1.1.1
//! does: it reports itself as `antigravity-acp` under its build label, names its
//! session with a UUID, and offers the model and its permission mode as session
//! settings. `next` is a build Oga was not verified against, `no-model` a server
//! that does not offer the task's model, and `auth` one that refuses the
//! session until someone signs in.
//!
//! A mode starting `cursor` answers the way `cursor-agent acp` does: it names
//! neither itself nor its version, offers one sign-in method and refuses every
//! session until the client claims it, names its session with a UUID of
//! Cursor's own, and offers the model and the execution mode as session
//! settings. `no-model` is an account that does not offer the task's model,
//! and `no-sign-in` a server offering a method this client cannot claim.
//!
//! A mode whose turns are `steer` waits for an instruction to be handed to the
//! turn it is already running, the way `claude-agent-acp` takes one over
//! `_session/steering`, and finishes with what it was told. `steer-refused`
//! answers that request without taking the instruction. Only a mode naming
//! `steer` advertises that it takes one at all.
//!
//! A mode whose turns are `recovery` has a model provider failing under it: it
//! retries by itself, reports every attempt in the vendor block of a session
//! update, gives up on a usage limit, and then calls the turn a refusal.
//!
//! A mode whose turns are `join` waits for a second `session/prompt` and folds
//! it into the turn it is already running, the way `opencode acp` does, then
//! answers both prompts from the one run it shared. It advertises nothing.
//!
//! A mode starting `pi` answers the way `pi-acp` 0.0.33 does: it reports itself
//! as that adapter, names its session with Pi's own UUID, records the session
//! in its session map under the profile's session directory, and offers the
//! model and thinking level as session settings. It writes a startup summary
//! into a new session a moment after answering, then announces its commands,
//! and asks about an extension's question with neither kind nor location.
//! `next` is a release Oga was not verified against, `unmapped` an adapter that
//! records no session file, and `ask` a turn that puts a question to a person.
//!
//! A mode whose turns are `outside` asks to run a command that reaches outside
//! the task's folder, and finishes with the option it was given.
//! `outside-steer` also waits for an instruction once it has its answer.

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
/// Antigravity's ACP server names a session with a UUID of its own.
const ANTIGRAVITY_SESSION: &str = "5b8e2c4a-1f3d-4e6b-9a7c-2d4f6e8a0b1c";
/// Pi names a session with a UUID, and `pi-acp` opens the session under it.
const PI_SESSION: &str = "0b7d4a8e-5c1f-4d2a-9e3b-6f8c1a2d4e5f";
/// Cursor's ACP server names a session with a UUID of its own.
const CURSOR_SESSION: &str = "3c9a1d7e-4b2f-4a8c-91d6-7e5b3f2a8c04";

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

/// A recovery run as the agent publishes it: a session update carrying nothing
/// of its own, with the story in its vendor block.
fn recovery_update(session: &str, recovery: Value) {
    update(
        session,
        json!({
            "sessionUpdate": "session_info_update",
            "_meta": {"fx": {"modelResponseRecovery": recovery}},
        }),
    );
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

/// Antigravity's settings: the Gemini models the account offers, and the
/// permission modes, starting on the one that asks before every tool call.
fn antigravity_settings(mode: &str, model: &str, permissions: &str) -> Value {
    let mut models =
        vec![json!({"value": "gemini-3.7-flash-high", "name": "Gemini 3.7 Flash (High)"})];
    if !mode.ends_with("no-model") {
        models
            .push(json!({"value": "gemini-3.6-flash-medium", "name": "Gemini 3.6 Flash (Medium)"}));
    }
    json!([
        {
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": model,
            "options": models,
        },
        {
            "id": "mode",
            "name": "Session Mode",
            "category": "mode",
            "type": "select",
            "currentValue": permissions,
            "options": [
                {"value": "default", "name": "Default"},
                {"value": "auto_edit", "name": "Auto Edit"},
                {"value": "yolo", "name": "YOLO"},
            ],
        },
    ])
}

/// `pi-acp`'s settings: the models Pi's account offers, and the thinking
/// levels it knows, which stop short of `max`.
fn pi_settings(mode: &str, model: &str, thinking: &str) -> Value {
    let mut models = vec![
        json!({"value": "anthropic/claude-sonnet-4-5", "name": "anthropic/Claude Sonnet 4.5"}),
    ];
    if !mode.ends_with("no-model") {
        models.push(json!({"value": "openai/gpt-5.5", "name": "openai/GPT-5.5"}));
    }
    let levels: Vec<Value> = ["off", "minimal", "low", "medium", "high", "xhigh"]
        .iter()
        .map(|level| json!({"value": level, "name": format!("Thinking: {level}")}))
        .collect();
    json!([
        {
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": model,
            "options": models,
        },
        {
            "id": "thought_level",
            "name": "Thinking",
            "category": "thought_level",
            "type": "select",
            "currentValue": thinking,
            "options": levels,
        },
    ])
}

/// Cursor's settings: the models its session offers, each id carrying its own
/// thinking and context choices, and the execution modes, starting on the one
/// with full tool access.
fn cursor_settings(mode: &str, model: &str, access: &str) -> Value {
    let mut models =
        vec![json!({"value": "auto-smart[optimize_for=balanced]", "name": "Auto Balance"})];
    if !mode.ends_with("no-model") {
        models.push(json!({"value": "claude-sonnet-5[thinking=true,effort=high]", "name": "claude-sonnet-5"}));
    }
    json!([
        {
            "id": "mode",
            "name": "Mode",
            "category": "mode",
            "type": "select",
            "currentValue": access,
            "options": [
                {"value": "agent", "name": "Agent"},
                {"value": "plan", "name": "Plan"},
                {"value": "ask", "name": "Ask"},
            ],
        },
        {
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": model,
            "options": models,
        },
    ])
}

/// Records the session the way `pi-acp` does, keyed by its id, with the file
/// Pi keeps it in under the session directory it was started with.
fn record_pi_session(session: &str, cwd: &str) {
    let (Ok(home), Ok(sessions)) = (env::var("HOME"), env::var("PI_CODING_AGENT_SESSION_DIR"))
    else {
        return;
    };
    let map = std::path::Path::new(&home).join(".pi/pi-acp");
    std::fs::create_dir_all(&map).expect("session map directory");
    let file = format!("{sessions}/--project--/2026-09-17T10-00-00-000Z_{session}.jsonl");
    std::fs::write(
        map.join("session-map.json"),
        json!({"version": 1, "sessions": {session: {"sessionId": session, "cwd": cwd, "sessionFile": file}}})
            .to_string(),
    )
    .expect("session map");
}

fn commands_announced(session: &str) {
    update(
        session,
        json!({"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "compact", "description": "Compact the session"}]}),
    );
}

/// What the agent attaches to its initialize answer. `claude-agent-acp`
/// advertises that it takes an instruction mid-turn here, beside its
/// capabilities rather than inside them.
fn initialize_meta(mode: &str) -> Option<Value> {
    mode.contains("steer")
        .then(|| json!({"steering": {"supported": true}}))
}

fn capabilities(mode: &str) -> Value {
    if mode.starts_with("cursor") {
        return json!({
            "loadSession": true,
            "mcpCapabilities": {"http": true, "sse": true},
            "sessionCapabilities": {"list": {}},
        });
    }
    if mode.starts_with("pi") {
        return json!({
            "loadSession": true,
            "mcpCapabilities": {"http": false, "sse": false},
            "sessionCapabilities": {"list": {}, "delete": {}},
        });
    }
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

/// Asks to run a command that reaches outside the task's folder and waits for
/// the answer, taking any instruction handed to the turn meanwhile.
fn ask_outside(
    session: &str,
    lines: &mut impl Iterator<Item = String>,
    log_path: &str,
) -> (String, Option<String>) {
    let id = 9004;
    let command = "cp src/lib.rs /tmp/lib.rs.bak";
    update(
        session,
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "run-9004",
            "title": command,
            "kind": "execute",
            "locations": [{"path": "/tmp/lib.rs.bak"}],
        }),
    );
    emit(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/request_permission",
        "params": {
            "sessionId": session,
            "toolCall": {
                "toolCallId": "run-9004",
                "title": command,
                "kind": "execute",
                "locations": [{"path": "/tmp/lib.rs.bak"}],
            },
            "options": [
                {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
            ],
        },
    }));
    let mut told = None;
    for line in lines.by_ref() {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        log(log_path, &json!({"received": message}));
        if let Some(text) = take_steering(&message) {
            told = Some(text);
            continue;
        }
        if message["id"] != json!(id) {
            continue;
        }
        let answer = message["result"]["outcome"]["optionId"]
            .as_str()
            .or_else(|| message["result"]["outcome"]["outcome"].as_str())
            .unwrap_or("no answer")
            .to_owned();
        return (answer, told);
    }
    ("no answer".into(), told)
}

/// Takes an instruction handed to the running turn, answering that it was.
fn take_steering(message: &Value) -> Option<String> {
    if message["method"] != "_session/steering" {
        return None;
    }
    emit(&json!({"jsonrpc": "2.0", "id": message["id"], "result": {"outcome": "injected"}}));
    Some(
        message["params"]["prompt"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

fn wait_for_steering(lines: &mut impl Iterator<Item = String>, log_path: &str) -> Option<String> {
    for line in lines.by_ref() {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        log(log_path, &json!({"received": message}));
        if message["method"] == "session/cancel" {
            return None;
        }
        if let Some(text) = take_steering(&message) {
            return Some(text);
        }
    }
    None
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
                if let Ok(message) = serde_json::from_str::<Value>(&line) {
                    log(log_path, &json!({"received": message}));
                }
            }
            return;
        }
        turns if turns.starts_with("steer") => {
            let taken = turns == "steer";
            for line in lines.by_ref() {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                log(log_path, &json!({"received": message}));
                if message["method"] == "session/cancel" {
                    break;
                }
                if message["method"] != "_session/steering" {
                    continue;
                }
                let answer = if taken {
                    json!({"outcome": "injected"})
                } else {
                    json!({"outcome": "promptRequired", "reason": "noRunningTurn"})
                };
                emit(&json!({"jsonrpc": "2.0", "id": message["id"], "result": answer}));
                if taken {
                    let text = message["params"]["prompt"][0]["text"]
                        .as_str()
                        .unwrap_or_default();
                    chunk(&session, &format!("told mid-turn: {text}\n"));
                }
                break;
            }
            chunk(&session, "OGA_RESULT: completed");
        }
        "join" => {
            let mut joined = None;
            for line in lines.by_ref() {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                log(log_path, &json!({"received": message}));
                if message["method"] == "session/cancel" {
                    break;
                }
                if message["method"] != "session/prompt" {
                    continue;
                }
                let text = message["params"]["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or_default();
                chunk(&session, &format!("told mid-turn: {text}\n"));
                joined = Some(message["id"].clone());
                break;
            }
            update(
                &session,
                json!({
                    "sessionUpdate": "usage_update",
                    "used": 12_000,
                    "size": 200_000,
                    "cost": {"amount": 0.5, "currency": "USD"},
                }),
            );
            chunk(&session, "OGA_RESULT: completed");
            // Both prompts describe the one run they shared, so both answer
            // off the same idle event rather than one turn each.
            if let Some(id) = joined {
                emit(&json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "end_turn"}}));
            }
        }
        "ask" => {
            emit(&json!({
                "jsonrpc": "2.0",
                "id": 9003,
                "method": "session/request_permission",
                "params": {
                    "sessionId": session,
                    "toolCall": {"toolCallId": "pi-ui-1", "title": "Run the deploy script?", "kind": "other", "status": "pending"},
                    "options": [
                        {"optionId": "yes", "name": "Yes", "kind": "allow_once"},
                        {"optionId": "no", "name": "No", "kind": "reject_once"},
                    ],
                },
            }));
            let mut answer = "no answer".to_owned();
            for line in lines.by_ref() {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                log(log_path, &json!({"received": message}));
                if message["id"] == json!(9003) {
                    answer = message["result"]["outcome"]["optionId"]
                        .as_str()
                        .unwrap_or("no answer")
                        .to_owned();
                    break;
                }
            }
            chunk(
                &session,
                &format!("confirmed: {answer}\nOGA_RESULT: completed"),
            );
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
        turns if turns.starts_with("outside") => {
            let (answer, mut told) = ask_outside(&session, lines, log_path);
            if turns == "outside-steer" && told.is_none() {
                told = wait_for_steering(lines, log_path);
            }
            let told = told.map_or_else(String::new, |text| format!(", told: {text}"));
            chunk(
                &session,
                &format!("step: {answer}{told}\nOGA_RESULT: completed"),
            );
        }
        "refused-then-quiet" => {
            update(
                &session,
                json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": "edit-9003",
                    "title": "Edit /tmp/scratch",
                    "kind": "edit",
                    "locations": [{"path": "/tmp/scratch"}],
                }),
            );
            ask_permission(&session, 9003, "/tmp/scratch", lines, log_path);
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
        "recovery" => {
            for attempt in 1..=3 {
                recovery_update(
                    &session,
                    json!({
                        "state": "active",
                        "kind": "auto_retry",
                        "cause": "provider_unavailable",
                        "action": "retrying_request",
                        "attempt": attempt,
                        "attemptLimit": 3,
                        "durable": true,
                        "message": format!("⚠ Provider unavailable · HTTP 503 · service_unavailable_error: Service temporarily unavailable. · retrying request · attempt {attempt}/3"),
                    }),
                );
            }
            recovery_update(
                &session,
                json!({
                    "state": "paused",
                    "kind": "terminal_provider_error",
                    "cause": "rate_limited",
                    "action": "paused",
                    "requiredAction": "continue_later",
                    "attempt": 3,
                    "attemptLimit": 3,
                    "message": "⚠ Rate limited · HTTP 429 · rate_limit_exceeded: Free tier requests on this model are rate-limited. · recovery paused after 3/3 attempts",
                }),
            );
            emit(&json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "refusal"}}));
            return;
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
                "GEMINI_HOME": env::var("GEMINI_HOME").ok(),
                "AGY_ACP_FORCE_FILE_STORAGE": env::var("AGY_ACP_FORCE_FILE_STORAGE").ok(),
                "PI_CODING_AGENT_DIR": env::var("PI_CODING_AGENT_DIR").ok(),
                "PI_CODING_AGENT_SESSION_DIR": env::var("PI_CODING_AGENT_SESSION_DIR").ok(),
            },
        }),
    );
    let opencode = mode.starts_with("opencode");
    let claude = mode.starts_with("claude");
    let codex = mode.starts_with("codex");
    let antigravity = mode.starts_with("antigravity");
    let pi = mode.starts_with("pi");
    let cursor = mode.starts_with("cursor");
    let session_id = match (opencode, claude, codex, antigravity, pi, cursor) {
        (true, ..) => "ses_acp1",
        (_, true, ..) => CLAUDE_SESSION,
        (_, _, true, ..) => CODEX_THREAD,
        (_, _, _, true, ..) => ANTIGRAVITY_SESSION,
        (.., true, _) => PI_SESSION,
        (.., true) => CURSOR_SESSION,
        _ => SESSION,
    };
    let renamed = mode.ends_with("renamed");
    let agent_name = match (opencode, claude, codex, antigravity, pi) {
        (true, ..) if !renamed => "OpenCode",
        (_, true, ..) if !renamed => "@agentclientprotocol/claude-agent-acp",
        (_, _, true, ..) if !renamed => "@agentclientprotocol/codex-acp",
        (.., true, _) if !renamed => "antigravity-acp",
        (.., true) if !renamed => "pi-acp",
        _ => "fake-acp-agent",
    };
    let opencode2 = mode.starts_with("opencode2");
    let next = mode.ends_with("next");
    let version = match (codex, opencode2, antigravity, pi) {
        (true, ..) if next => "1.13.0",
        (true, ..) => "1.12.0",
        (_, true, ..) if next => "2.1.0",
        (_, true, ..) => "2.0.1",
        (.., true, _) if next => "agy_acp_server_1.1.2",
        (.., true, _) => "agy_acp_server_1.1.1",
        (.., true) if next => "0.0.34",
        (.., true) => "0.0.33",
        _ if opencode => "1.18.31",
        _ => "2.1.0",
    };
    let turns = mode
        .strip_prefix("opencode2-")
        .or_else(|| mode.strip_prefix("opencode-"))
        .or_else(|| mode.strip_prefix("claude-"))
        .or_else(|| mode.strip_prefix("codex-"))
        .or_else(|| mode.strip_prefix("antigravity-"))
        .or_else(|| mode.strip_prefix("pi-"))
        .or_else(|| mode.strip_prefix("cursor-"))
        .unwrap_or(&mode)
        .to_owned();
    let (mut model, mut effort) = ("opencode/big-pickle".to_owned(), "default".to_owned());
    let mut access = "agent".to_owned();
    if antigravity {
        model = "gemini-3.7-flash-high".to_owned();
        access = "default".to_owned();
    }
    if pi {
        model = "anthropic/claude-sonnet-4-5".to_owned();
        effort = "medium".to_owned();
    }
    if cursor {
        model = "auto-smart[optimize_for=balanced]".to_owned();
    }
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

    let mut signed_in = !cursor;
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
            "initialize" if cursor => {
                let offered = if mode.ends_with("no-sign-in") {
                    "cursor_api_key"
                } else {
                    "cursor_login"
                };
                emit(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": 1,
                        "agentCapabilities": capabilities(&mode),
                        "authMethods": [{"id": offered, "name": "Cursor Login"}],
                    },
                }));
            }
            "authenticate" => {
                signed_in = true;
                emit(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
            }
            "session/new" | "session/load" if cursor && !signed_in => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32000, "message": "Authentication required"},
            })),
            "session/new" if cursor => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": session_id,
                    "configOptions": cursor_settings(&mode, &model, &access),
                },
            })),
            "initialize" => {
                let mut answer = json!({
                    "protocolVersion": 1,
                    "agentCapabilities": capabilities(&mode),
                    "agentInfo": {"name": agent_name, "version": version},
                });
                if let Some(meta) = initialize_meta(&mode) {
                    answer["_meta"] = meta;
                }
                emit(&json!({"jsonrpc": "2.0", "id": id, "result": answer}));
            }
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
            "session/new" if antigravity && mode.ends_with("auth") => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32000, "message": "Authentication required"},
            })),
            "session/new" if antigravity => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": session_id,
                    "configOptions": antigravity_settings(&mode, &model, &access),
                },
            })),
            "session/new" if pi => {
                if !mode.ends_with("unmapped") {
                    record_pi_session(session_id, params["cwd"].as_str().unwrap_or_default());
                }
                emit(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "sessionId": session_id,
                        "configOptions": pi_settings(&mode, &model, &effort),
                    },
                }));
                std::thread::sleep(std::time::Duration::from_millis(50));
                chunk(session_id, "pi v0.85.1\n\nSkills: none\n");
                commands_announced(session_id);
            }
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
                let offered = if cursor {
                    cursor_settings(&mode, &model, &access)
                } else if pi {
                    pi_settings(&mode, &model, &effort)
                } else if claude {
                    claude_settings(&mode, &effort)
                } else if antigravity {
                    antigravity_settings(&mode, &model, &access)
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
                if cursor {
                    emit(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"configOptions": cursor_settings(&mode, &model, &access)},
                    }));
                } else if pi {
                    emit(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"configOptions": pi_settings(&mode, &model, &effort)},
                    }));
                    commands_announced(session);
                } else {
                    emit(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                }
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
            "session/resume" if antigravity => emit(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"configOptions": antigravity_settings(&mode, &model, &access)},
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
