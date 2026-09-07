//! Provider argv, transcripts, sessions, and usage/event parsing.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use oga_domain::{Profile, Provider};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const NO_FINAL_MESSAGE: &str =
    "(no final message: the provider stream carried no assistant text)";
const WORKER_TOOLS: &str = "Bash,mcp__oga__query";
const WRITE_TOOLS: &[&str] = &[
    "write",
    "write_file",
    "edit",
    "multiedit",
    "create_file",
    "notebookedit",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCommand {
    pub argv: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Variables the child has to run without, even when the broker exports
    /// them.
    #[serde(default)]
    pub env_remove: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CommandOptions<'a> {
    pub hook_url: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub allowed_tools: Option<&'a str>,
    pub mcp_config: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub tokens_in: Option<f64>,
    pub tokens_out: Option<f64>,
    pub cached_tokens: Option<f64>,
    pub cost_usd: Option<f64>,
    pub turns: Option<f64>,
    /// `cost_usd` was priced from public model rates because the provider
    /// reported tokens but no amount, not read off the provider itself.
    #[serde(default)]
    pub cost_usd_estimated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedEvent {
    pub provider: Provider,
    pub payload: Value,
    pub session_id: Option<String>,
    pub write_targets: Vec<String>,
    pub usage: Usage,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("invalid provider event: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0} cannot continue an earlier session")]
    CannotResume(String),
}

pub fn command_for(
    profile: &Profile,
    prompt: &str,
    cwd: &str,
    model: Option<&str>,
    effort: Option<&str>,
    mcp_config: Option<&str>,
) -> ProviderCommand {
    command_for_with_options(
        profile,
        prompt,
        cwd,
        model,
        CommandOptions {
            effort,
            mcp_config,
            ..CommandOptions::default()
        },
    )
}

pub fn command_for_with_options(
    profile: &Profile,
    prompt: &str,
    cwd: &str,
    model: Option<&str>,
    options: CommandOptions<'_>,
) -> ProviderCommand {
    let CommandOptions {
        hook_url,
        effort,
        allowed_tools,
        mcp_config,
    } = options;
    let model = model.unwrap_or(&profile.default_model);
    let argv = if let Some(command) = &profile.command {
        command
            .iter()
            .map(|part| {
                part.replace("{prompt}", prompt)
                    .replace("{model}", model)
                    .replace("{cwd}", cwd)
                    .replace("{effort}", effort.unwrap_or(""))
            })
            .collect::<Vec<_>>()
    } else {
        match profile.provider {
            Provider::Claude => {
                let skills = skills_dir(profile);
                let hook_settings = hook_url.map(claude_hook_settings);
                let mut a = vec![
                    "claude",
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--model",
                    model,
                ];
                if let Some(e) = effort {
                    a.extend(["--effort", e]);
                }
                a.extend([
                    "--permission-mode",
                    "acceptEdits",
                    "--allowedTools",
                    allowed_tools.unwrap_or(WORKER_TOOLS),
                    "--add-dir",
                    skills.as_str(),
                ]);
                if let Some(settings) = hook_settings.as_deref() {
                    a.extend(["--settings", settings]);
                }
                if let Some(c) = mcp_config {
                    a.extend(["--mcp-config", c, "--strict-mcp-config"]);
                }
                a.extend(["--", prompt]);
                a.into_iter().map(String::from).collect()
            }
            Provider::Codex => {
                let mut a = vec!["codex", "exec", "--json", "--model", model];
                let effort_config =
                    effort.map(|value| format!("model_reasoning_effort=\"{value}\""));
                if let Some(e) = effort {
                    a.extend(["-c", effort_config.as_deref().unwrap_or(e)]);
                }
                a.extend([
                    "--dangerously-bypass-approvals-and-sandbox",
                    "--cd",
                    cwd,
                    "--skip-git-repo-check",
                    prompt,
                ]);
                a.into_iter().map(String::from).collect()
            }
            Provider::OpenCode => {
                let mut a = vec!["opencode", "run", "--format", "json", "--model", model];
                if let Some(e) = effort {
                    a.extend(["--variant", e]);
                }
                a.extend(["--dir", cwd, "--auto", prompt]);
                a.into_iter().map(String::from).collect()
            }
            Provider::OpenCode2 => {
                let selected = effort.map_or_else(|| model.to_owned(), |e| format!("{model}#{e}"));
                vec![
                    "opencode2".into(),
                    "run".into(),
                    "--format".into(),
                    "json".into(),
                    "--model".into(),
                    selected,
                    "--auto".into(),
                    prompt.into(),
                ]
            }
            Provider::Antigravity => vec![
                "agy",
                "--print",
                prompt,
                "--output-format",
                "stream-json",
                "--model",
                model,
                "--new-project",
                "--add-dir",
                cwd,
                "--mode",
                "accept-edits",
                "--dangerously-skip-permissions",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            Provider::Pi => {
                let mut a = vec!["pi", "--mode", "json", "--model", model];
                if let Some(e) = effort {
                    a.extend(["--thinking", e]);
                }
                a.extend(["--no-approve", prompt]);
                a.into_iter().map(String::from).collect()
            }
        }
    };
    ProviderCommand {
        argv,
        env: environment_for(profile),
        env_remove: unset_environment_for(profile),
    }
}

pub fn resume_command_for(
    profile: &Profile,
    prompt: &str,
    cwd: &str,
    session: &str,
    model: Option<&str>,
    effort: Option<&str>,
    mcp_config: Option<&str>,
) -> Result<ProviderCommand, ProviderError> {
    resume_command_for_with_options(
        profile,
        prompt,
        cwd,
        session,
        model,
        CommandOptions {
            effort,
            mcp_config,
            ..CommandOptions::default()
        },
    )
}

pub fn resume_command_for_with_options(
    profile: &Profile,
    prompt: &str,
    cwd: &str,
    session: &str,
    model: Option<&str>,
    options: CommandOptions<'_>,
) -> Result<ProviderCommand, ProviderError> {
    let CommandOptions {
        hook_url,
        effort,
        allowed_tools,
        mcp_config,
    } = options;
    if profile.command.is_some() {
        return Err(ProviderError::CannotResume(profile.id.clone()));
    }
    let model = model.unwrap_or(&profile.default_model);
    let argv = match profile.provider {
        Provider::Claude => {
            let skills = skills_dir(profile);
            let hook_settings = hook_url.map(claude_hook_settings);
            let mut a = vec![
                "claude",
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--model",
                model,
            ];
            if let Some(e) = effort {
                a.extend(["--effort", e]);
            }
            a.extend([
                "--permission-mode",
                "acceptEdits",
                "--allowedTools",
                allowed_tools.unwrap_or(WORKER_TOOLS),
                "--resume",
                session,
                "--add-dir",
                skills.as_str(),
            ]);
            if let Some(settings) = hook_settings.as_deref() {
                a.extend(["--settings", settings]);
            }
            if let Some(c) = mcp_config {
                a.extend(["--mcp-config", c, "--strict-mcp-config"]);
            }
            a.extend(["--", prompt]);
            a.into_iter().map(String::from).collect()
        }
        Provider::Codex => {
            let mut a = vec![
                "codex", "exec", "resume", session, "--json", "--model", model,
            ];
            let effort_config = effort.map(|value| format!("model_reasoning_effort=\"{value}\""));
            if let Some(e) = effort {
                a.extend(["-c", effort_config.as_deref().unwrap_or(e)]);
            }
            a.extend([
                "--dangerously-bypass-approvals-and-sandbox",
                "--skip-git-repo-check",
                prompt,
            ]);
            a.into_iter().map(String::from).collect()
        }
        Provider::OpenCode => {
            let mut a = vec!["opencode", "run", "--format", "json", "--model", model];
            if let Some(e) = effort {
                a.extend(["--variant", e]);
            }
            a.extend(["--dir", cwd, "--auto", "--session", session, prompt]);
            a.into_iter().map(String::from).collect()
        }
        Provider::OpenCode2 => {
            let selected = effort.map_or_else(|| model.to_owned(), |e| format!("{model}#{e}"));
            vec![
                "opencode2".into(),
                "run".into(),
                "--format".into(),
                "json".into(),
                "--model".into(),
                selected,
                "--auto".into(),
                "--session".into(),
                session.into(),
                prompt.into(),
            ]
        }
        Provider::Antigravity => vec![
            "agy",
            "--print",
            prompt,
            "--output-format",
            "stream-json",
            "--model",
            model,
            "--conversation",
            session,
            "--add-dir",
            cwd,
            "--mode",
            "accept-edits",
            "--dangerously-skip-permissions",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        Provider::Pi => {
            let mut a = vec!["pi", "--mode", "json", "--model", model];
            if let Some(e) = effort {
                a.extend(["--thinking", e]);
            }
            a.extend(["--no-approve", "--session-id", session, prompt]);
            a.into_iter().map(String::from).collect()
        }
    };
    Ok(ProviderCommand {
        argv,
        env: environment_for(profile),
        env_remove: unset_environment_for(profile),
    })
}

/// The oga MCP server a worker reaches its own broker through. The task id
/// rides on a header so the broker answers as that task rather than trusting
/// whatever id the worker names.
pub fn worker_mcp_config(base_url: &str, task_id: &str) -> String {
    serde_json::json!({
        "mcpServers": {
            "oga": {
                "type": "http",
                "url": format!("{base_url}/mcp"),
                "headers": { "x-oga-task-id": task_id },
            }
        }
    })
    .to_string()
}

fn claude_hook_settings(url: &str) -> String {
    let hook =
        format!(r#"[{{"matcher":"","hooks":[{{"type":"http","url":"{url}","timeout":10}}]}}]"#);
    format!(
        r#"{{"hooks":{{"PreToolUse":{hook},"PostToolUse":{hook},"PostToolUseFailure":{hook},"SubagentStart":{hook},"SubagentStop":{hook},"Notification":{hook},"StopFailure":{hook}}}}}"#
    )
}

pub fn session_id_from(provider: Provider, event: &Value) -> Option<String> {
    let value = match provider {
        Provider::Claude
            if matches!(
                event.get("type").and_then(Value::as_str),
                Some("system" | "result")
            ) =>
        {
            event.get("session_id")
        }
        Provider::Codex if event.get("type").and_then(Value::as_str) == Some("thread.started") => {
            event.get("thread_id")
        }
        Provider::OpenCode | Provider::OpenCode2
            if event.get("type").and_then(Value::as_str) == Some("step_start") =>
        {
            event.get("sessionID")
        }
        Provider::Antigravity
            if matches!(
                event.get("event").and_then(Value::as_str),
                Some("init" | "result")
            ) =>
        {
            event
                .get("conversation_id")
                .or_else(|| event.get("result").and_then(|v| v.get("conversation_id")))
        }
        Provider::Pi if event.get("type").and_then(Value::as_str) == Some("session") => {
            event.get("id")
        }
        _ => None,
    }?;
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from)
}

pub fn write_targets_from(payload: &Value) -> Vec<String> {
    if payload.get("type").and_then(Value::as_str) == Some("message_update") {
        return Vec::new();
    }
    fn visit(value: &Value, output: &mut Vec<String>) {
        let Some(object) = value.as_object() else {
            return;
        };
        let tool = ["tool_name", "toolName", "tool", "name"]
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str));
        if tool.is_some_and(|tool| {
            WRITE_TOOLS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(tool))
        }) {
            for key in ["input", "tool_input", "toolInput", "args", "arguments"] {
                if let Some(input) = object.get(key).and_then(Value::as_object) {
                    add_path(input, output);
                }
            }
            if let Some(input) = object
                .get("state")
                .and_then(|v| v.get("input"))
                .and_then(Value::as_object)
            {
                add_path(input, output);
            }
        }
        for key in ["item", "part"] {
            if let Some(child) = object.get(key) {
                visit(child, output);
            }
        }
        if let Some(content) = object
            .get("message")
            .and_then(|v| v.get("content"))
            .and_then(Value::as_array)
        {
            for child in content {
                visit(child, output);
            }
        }
    }
    fn add_path(input: &serde_json::Map<String, Value>, output: &mut Vec<String>) {
        if let Some(path) = ["file_path", "filePath", "path"]
            .iter()
            .find_map(|key| input.get(*key).and_then(Value::as_str))
            .filter(|path| !path.is_empty())
        {
            output.push(path.to_owned());
        }
    }
    let mut output = Vec::new();
    visit(payload, &mut output);
    output
}

pub fn parse_stream(provider: Provider, raw: &str) -> Result<Vec<ParsedEvent>, ProviderError> {
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let payload = serde_json::from_str(line)?;
            Ok(ParsedEvent {
                provider,
                session_id: session_id_from(provider, &payload),
                write_targets: write_targets_from(&payload),
                usage: usage_from_event(&payload),
                payload,
            })
        })
        .collect()
}

pub fn final_text(provider: Provider, raw: &str) -> String {
    for event in raw
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        let text = if provider == Provider::Claude {
            event
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else if provider == Provider::Pi
            && event.get("type").and_then(Value::as_str) == Some("message_end")
        {
            pi_message_text(&event)
        } else {
            generic_text(&event)
        };
        if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
            return text;
        }
    }
    let trimmed = raw.trim();
    if trimmed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .all(|line| line.trim_start().starts_with('{'))
    {
        return raw
            .lines()
            .find_map(marker_line)
            .unwrap_or_else(|| NO_FINAL_MESSAGE.to_owned());
    }
    trimmed.to_owned()
}

pub fn usage_from_event(event: &Value) -> Usage {
    let nested = event.get("result").and_then(Value::as_object);
    let usage = nested
        .and_then(|v| v.get("usage"))
        .or_else(|| event.get("usage"))
        .or_else(|| event.get("message").and_then(|v| v.get("usage")))
        .and_then(Value::as_object);
    let kind = event
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| event.get("event").and_then(Value::as_str));
    if kind == Some("result") {
        let cache_read = number(usage.and_then(|v| v.get("cache_read_tokens")));
        let thinking = number(usage.and_then(|v| v.get("thinking_tokens"))).unwrap_or_default();
        return Usage {
            tokens_in: number(usage.and_then(|v| v.get("input_tokens"))).map(|value| {
                value
                    + number(usage.and_then(|u| u.get("cache_creation_input_tokens")))
                        .unwrap_or(0.0)
            }),
            tokens_out: number(usage.and_then(|v| v.get("output_tokens")))
                .map(|value| value + thinking),
            cached_tokens: cache_read,
            cost_usd: number(
                nested
                    .and_then(|v| v.get("total_cost_usd"))
                    .or_else(|| event.get("total_cost_usd")),
            ),
            turns: number(
                nested
                    .and_then(|v| v.get("num_turns"))
                    .or_else(|| event.get("num_turns")),
            ),
            cost_usd_estimated: false,
        };
    }
    if kind == Some("turn.completed") {
        return Usage {
            tokens_in: number(usage.and_then(|v| v.get("input_tokens"))).map(|v| {
                (v - number(usage.and_then(|u| u.get("cached_input_tokens"))).unwrap_or(0.0))
                    .max(0.0)
            }),
            tokens_out: number(usage.and_then(|v| v.get("output_tokens"))),
            cached_tokens: number(usage.and_then(|v| v.get("cached_input_tokens"))),
            ..Usage::default()
        };
    }
    if kind == Some("step_finish") {
        let tokens = event
            .get("part")
            .and_then(|v| v.get("tokens"))
            .and_then(Value::as_object);
        return Usage {
            tokens_in: number(tokens.and_then(|v| v.get("input"))),
            tokens_out: number(tokens.and_then(|v| v.get("output"))),
            ..Usage::default()
        };
    }
    if kind == Some("message_end") {
        let pi_usage = event
            .get("message")
            .and_then(|value| value.get("usage"))
            .is_some();
        return Usage {
            tokens_in: number(usage.and_then(|v| v.get("input"))),
            tokens_out: number(usage.and_then(|v| v.get("output"))),
            cached_tokens: number(usage.and_then(|v| v.get("cacheRead"))),
            // Let lifecycle pricing use the shared catalogue for Pi usage.
            cost_usd: if pi_usage {
                None
            } else {
                number(
                    usage
                        .and_then(|v| v.get("cost"))
                        .and_then(|v| v.get("total")),
                )
            },
            ..Usage::default()
        };
    }
    if kind == Some("turn_end") {
        let has_turn_usage = usage
            .and_then(|value| value.get("cost"))
            .is_some_and(|value| value.is_object())
            || usage.is_some_and(|value| value.contains_key("totalTokens"));
        return Usage {
            turns: has_turn_usage.then_some(1.0),
            ..Usage::default()
        };
    }
    Usage::default()
}

fn generic_text(event: &Value) -> Option<String> {
    [
        event.get("text"),
        event.get("message"),
        event.get("content"),
        event.get("result").and_then(|v| v.get("response")),
        event.get("item").and_then(|v| v.get("text")),
        event.get("part").and_then(|v| v.get("text")),
    ]
    .into_iter()
    .flatten()
    .find_map(Value::as_str)
    .map(String::from)
}
fn pi_message_text(event: &Value) -> Option<String> {
    let message = event.get("message")?.as_object()?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    Some(
        message
            .get("content")?
            .as_array()?
            .iter()
            .filter_map(|block| {
                (block.get("type")?.as_str() == Some("text"))
                    .then(|| block.get("text")?.as_str())
                    .flatten()
            })
            .collect(),
    )
}
fn marker_line(line: &str) -> Option<String> {
    let line = line.trim();
    ["OGA_RESULT:", "OGA_BLOCKED:", "OGA_NEEDS_INPUT:"]
        .iter()
        .any(|prefix| line.starts_with(prefix))
        .then(|| line.to_owned())
}
fn number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}
fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "~".into())
}
fn expand_home(value: &str, home: &str) -> String {
    for prefix in ["$HOME", "~"] {
        let Some(rest) = value.strip_prefix(prefix) else {
            continue;
        };
        if rest.is_empty() {
            return home.to_owned();
        }
        if let Some(rest) = rest.strip_prefix('/') {
            return format!("{home}/{rest}");
        }
    }
    value.to_owned()
}
/// Where a provider keeps the account it is signed in as: the profile's own
/// override when it sets one, otherwise the provider's default directory
/// under the home folder. Read from the profile alone, never from this
/// process, so a broker launched with the variable exported cannot lend its
/// account to a profile that never asked for it.
fn account_dir(profile: &Profile, key: &str, default: &str) -> String {
    let home = home();
    profile.env.get(key).map_or_else(
        || format!("{home}{default}"),
        |value| expand_home(value, &home),
    )
}
/// Claude's account directory for this profile. Credentials and limits live
/// below it, so a profile that names no directory gets one of its own rather
/// than sharing the default with every other Claude profile.
pub fn claude_config_dir(profile: &Profile) -> String {
    let default = if profile.id == "claude" {
        "/.claude".to_owned()
    } else {
        format!("/.{}", profile.id)
    };
    account_dir(profile, "CLAUDE_CONFIG_DIR", &default)
}
/// The value `CLAUDE_CONFIG_DIR` has to carry for this profile, or `None` when
/// the profile wants the directory Claude already reads by default. Claude
/// keys the keychain entry holding the account off whether the variable is set
/// at all, not off where it points: set to any path, that default one
/// included, it looks under an entry named for the path and finds nothing. So
/// the default profile has to reach Claude with the variable absent.
fn claude_config_override(profile: &Profile) -> Option<String> {
    let dir = claude_config_dir(profile);
    (dir != format!("{}/.claude", home())).then_some(dir)
}
/// Codex's account directory for this profile.
pub fn codex_home(profile: &Profile) -> String {
    account_dir(profile, "CODEX_HOME", "/.codex")
}
/// Variables a provider process must not merely be given a value for, but must
/// not see at all. A worker inherits the broker's own environment, so absent
/// means removed rather than left unset.
pub fn unset_environment_for(profile: &Profile) -> BTreeSet<String> {
    match profile.provider {
        Provider::Claude if claude_config_override(profile).is_none() => {
            BTreeSet::from(["CLAUDE_CONFIG_DIR".to_owned()])
        }
        _ => BTreeSet::new(),
    }
}
/// The environment a provider process receives: the profile's own env with
/// `$HOME` and `~` expanded before they reach `execve`, plus the provider's
/// account directory set explicitly, so an inherited broker value cannot join
/// otherwise separate profiles. Pair it with `unset_environment_for`, which
/// covers the directories this map deliberately names nothing for.
pub fn environment_for(profile: &Profile) -> BTreeMap<String, String> {
    let home = home();
    let mut env = profile
        .env
        .iter()
        .map(|(key, value)| (key.clone(), expand_home(value, &home)))
        .collect::<BTreeMap<_, _>>();
    match profile.provider {
        Provider::Claude => match claude_config_override(profile) {
            Some(dir) => {
                env.insert("CLAUDE_CONFIG_DIR".into(), dir);
            }
            None => {
                env.remove("CLAUDE_CONFIG_DIR");
            }
        },
        Provider::Codex => {
            env.insert("CODEX_HOME".into(), codex_home(profile));
        }
        Provider::Pi => {
            let agent = account_dir(profile, "PI_CODING_AGENT_DIR", "/.pi/agent");
            let sessions = profile.env.get("PI_CODING_AGENT_SESSION_DIR").map_or_else(
                || format!("{agent}/sessions"),
                |value| expand_home(value, &home),
            );
            env.insert("PI_CODING_AGENT_DIR".into(), agent);
            env.insert("PI_CODING_AGENT_SESSION_DIR".into(), sessions);
        }
        Provider::OpenCode | Provider::OpenCode2 | Provider::Antigravity => {}
    }
    env
}
fn skills_dir(profile: &Profile) -> String {
    PathBuf::from(claude_config_dir(profile))
        .join("skills")
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(provider: Provider) -> Profile {
        Profile {
            id: "p".into(),
            label: "P".into(),
            provider,
            default_model: "model".into(),
            enabled: true,
            env: BTreeMap::new(),
            capabilities: vec![],
            command: None,
        }
    }
    #[test]
    fn sessions_cover_all_providers() {
        for (provider, line) in [
            (Provider::Claude, r#"{"type":"system","session_id":"c"}"#),
            (
                Provider::Codex,
                r#"{"type":"thread.started","thread_id":"x"}"#,
            ),
            (
                Provider::OpenCode,
                r#"{"type":"step_start","sessionID":"o"}"#,
            ),
            (
                Provider::OpenCode2,
                r#"{"type":"step_start","sessionID":"v"}"#,
            ),
            (
                Provider::Antigravity,
                r#"{"event":"init","conversation_id":"a"}"#,
            ),
            (Provider::Pi, r#"{"type":"session","id":"p"}"#),
        ] {
            assert!(session_id_from(provider, &serde_json::from_str(line).unwrap()).is_some());
        }
    }
    #[test]
    fn text_and_write_targets_are_extracted() {
        let raw = "{\"type\":\"tool_use\",\"part\":{\"tool\":\"write\",\"state\":{\"input\":{\"filePath\":\"a.md\"}}}}\n{\"type\":\"message\",\"part\":{\"text\":\"answer\"}}";
        assert_eq!(final_text(Provider::OpenCode, raw), "answer");
        assert_eq!(
            write_targets_from(
                &serde_json::from_str::<Value>(raw.lines().next().unwrap()).unwrap()
            ),
            vec!["a.md"]
        );
    }
    #[test]
    fn antigravity_result_bills_thinking_and_cache_reads() {
        let event = serde_json::json!({
            "event": "result",
            "result": {"usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "thinking_tokens": 5,
                "cache_read_tokens": 30
            }}
        });
        let usage = usage_from_event(&event);
        assert_eq!(usage.tokens_in, Some(100.0));
        assert_eq!(usage.tokens_out, Some(25.0));
        assert_eq!(usage.cached_tokens, Some(30.0));
    }
    #[test]
    fn pi_events_report_tokens_for_models_dev_pricing() {
        let usage = serde_json::json!({
            "input": 1520,
            "output": 177,
            "cacheRead": 13312,
            "cacheWrite": 0,
            "reasoning": 11,
            "totalTokens": 15009,
            "cost": {
                "input": 0.0002128,
                "output": 0.00004956,
                "cacheRead": 0.0000372736,
                "cacheWrite": 0,
                "total": 0.0002996336
            }
        });
        let message_end = serde_json::json!({
            "type": "message_end",
            "message": {"usage": usage.clone()}
        });
        let turn_end = serde_json::json!({
            "type": "turn_end",
            "message": {"usage": usage}
        });
        let message_usage = usage_from_event(&message_end);
        assert_eq!(message_usage.tokens_in, Some(1520.0));
        assert_eq!(message_usage.tokens_out, Some(177.0));
        assert_eq!(message_usage.cached_tokens, Some(13312.0));
        assert_eq!(message_usage.cost_usd, None);
        let turn_usage = usage_from_event(&turn_end);
        assert_eq!(turn_usage.tokens_in, None);
        assert_eq!(turn_usage.turns, Some(1.0));
    }
    #[test]
    fn claude_prompt_is_separated_from_variadic_options() {
        let argv = command_for(&profile(Provider::Claude), "go", "/repo", None, None, None).argv;
        assert_eq!(argv[argv.len() - 2..], ["--".to_owned(), "go".to_owned()]);
        let resumed = resume_command_for(
            &profile(Provider::Claude),
            "go",
            "/repo",
            "session",
            None,
            None,
            None,
        )
        .expect("claude supports resume")
        .argv;
        assert_eq!(
            resumed[resumed.len() - 2..],
            ["--".to_owned(), "go".to_owned()]
        );
    }
    #[test]
    fn command_uses_provider_program() {
        assert_eq!(
            command_for(&profile(Provider::Pi), "go", "/repo", None, None, None).argv[0],
            "pi"
        );
    }
    #[test]
    fn home_expands_only_on_a_path_boundary() {
        for (value, expected) in [
            ("$HOME/.claude-me", "/h/.claude-me"),
            ("~/.claude-me", "/h/.claude-me"),
            ("$HOME", "/h"),
            ("~", "/h"),
            ("$HOMEBREW/bin", "$HOMEBREW/bin"),
            ("~other/bin", "~other/bin"),
            ("/etc/claude", "/etc/claude"),
        ] {
            assert_eq!(expand_home(value, "/h"), expected, "{value}");
        }
    }
    #[test]
    fn spawn_environment_carries_expanded_paths() {
        let home = home();
        let mut profile = profile(Provider::Claude);
        profile
            .env
            .insert("CLAUDE_CONFIG_DIR".into(), "$HOME/.claude-me".into());
        let command = command_for(&profile, "go", "/repo", None, None, None);
        assert_eq!(
            command.env["CLAUDE_CONFIG_DIR"],
            format!("{home}/.claude-me")
        );
        let resumed = resume_command_for(&profile, "go", "/repo", "session", None, None, None)
            .expect("claude supports resume");
        assert_eq!(
            resumed.env["CLAUDE_CONFIG_DIR"],
            format!("{home}/.claude-me")
        );
        assert!(
            command.argv.contains(&format!("{home}/.claude-me/skills")),
            "skills directory is expanded too: {:?}",
            command.argv
        );
    }

    #[test]
    fn claude_profiles_without_overrides_use_separate_config_directories() {
        let home = home();
        let mut primary = profile(Provider::Claude);
        primary.id = "claude".into();
        let mut personal = profile(Provider::Claude);
        personal.id = "claude-me".into();

        assert_eq!(claude_config_dir(&primary), format!("{home}/.claude"));
        assert_eq!(
            environment_for(&personal)["CLAUDE_CONFIG_DIR"],
            format!("{home}/.claude-me")
        );
    }

    /// Claude names the keychain entry holding the account after
    /// `CLAUDE_CONFIG_DIR` whenever the variable is set, so a profile pointed
    /// at the directory Claude already defaults to has to see no variable at
    /// all — otherwise it hunts for an entry no login ever wrote and reports
    /// itself signed out.
    #[test]
    fn the_default_claude_directory_reaches_the_worker_as_no_variable() {
        let home = home();
        let mut primary = profile(Provider::Claude);
        primary.id = "claude".into();
        let mut spelled_out = profile(Provider::Claude);
        spelled_out.id = "claude".into();
        spelled_out
            .env
            .insert("CLAUDE_CONFIG_DIR".into(), "$HOME/.claude".into());

        for candidate in [&primary, &spelled_out] {
            let command = command_for(candidate, "go", "/repo", None, None, None);
            assert!(!command.env.contains_key("CLAUDE_CONFIG_DIR"));
            assert!(command.env_remove.contains("CLAUDE_CONFIG_DIR"));
            assert!(
                command.argv.contains(&format!("{home}/.claude/skills")),
                "skills still come from the default directory: {:?}",
                command.argv
            );
        }

        let mut personal = profile(Provider::Claude);
        personal.id = "claude-me".into();
        let command = command_for(&personal, "go", "/repo", None, None, None);
        assert_eq!(
            command.env["CLAUDE_CONFIG_DIR"],
            format!("{home}/.claude-me")
        );
        assert!(command.env_remove.is_empty());
    }

    #[test]
    fn claude_profiles_with_config_directories_keep_them_separate() {
        let mut work = profile(Provider::Claude);
        work.env
            .insert("CLAUDE_CONFIG_DIR".into(), "$HOME/.claude-work".into());
        let mut personal = profile(Provider::Claude);
        personal
            .env
            .insert("CLAUDE_CONFIG_DIR".into(), "$HOME/.claude-me".into());

        assert_ne!(
            environment_for(&work)["CLAUDE_CONFIG_DIR"],
            environment_for(&personal)["CLAUDE_CONFIG_DIR"]
        );
    }

    #[test]
    fn account_directories_come_from_the_profile_not_this_process() {
        // SAFETY: test-only env mutation; nothing else in this crate reads these.
        unsafe {
            std::env::set_var("CLAUDE_CONFIG_DIR", "/broker/.claude-me");
            std::env::set_var("CODEX_HOME", "/broker/.codex-me");
            std::env::set_var("PI_CODING_AGENT_DIR", "/broker/.pi-me");
        }
        let mut claude = profile(Provider::Claude);
        claude.id = "claude".into();
        let command = command_for(&claude, "go", "/repo", None, None, None);
        assert!(!command.env.contains_key("CLAUDE_CONFIG_DIR"));
        assert!(command.env_remove.contains("CLAUDE_CONFIG_DIR"));
        assert_eq!(claude_config_dir(&claude), format!("{}/.claude", home()));

        let codex = profile(Provider::Codex);
        assert_eq!(
            environment_for(&codex)["CODEX_HOME"],
            format!("{}/.codex", home())
        );
        assert_eq!(environment_for(&codex)["CODEX_HOME"], codex_home(&codex));

        let pi_env = environment_for(&profile(Provider::Pi));
        assert_eq!(
            pi_env["PI_CODING_AGENT_DIR"],
            format!("{}/.pi/agent", home())
        );
        assert_eq!(
            pi_env["PI_CODING_AGENT_SESSION_DIR"],
            format!("{}/.pi/agent/sessions", home())
        );
    }

    #[test]
    fn claude_skills_follow_the_profile_account_directory() {
        let mut me = profile(Provider::Claude);
        me.env
            .insert("CLAUDE_CONFIG_DIR".into(), "~/.claude-work".into());
        assert_eq!(skills_dir(&me), format!("{}/.claude-work/skills", home()));
    }
}
