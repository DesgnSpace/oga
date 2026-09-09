//! Desktop integrations that need filesystem access outside the webview.

use std::{
    env, fs,
    path::{Component, Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use oga_domain::ProfileView;
use serde::{Deserialize, Serialize};
use tauri::Manager;

const DEFAULT_BROKER_URL: &str = "http://127.0.0.1:7331";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpClient {
    Codex,
    Claude,
    OpenCode,
    Antigravity,
}

impl McpClient {
    fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::OpenCode => "OpenCode",
            Self::Antigravity => "Antigravity",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub client: String,
    pub path: String,
    pub success: bool,
    pub message: String,
}

/// Install the Oga MCP endpoint using an explicit home directory.
pub fn install_everywhere_at(
    profiles: &[ProfileView],
    endpoint: &str,
    home: &Path,
) -> Vec<InstallResult> {
    let mut targets = vec![
        (
            McpClient::Codex,
            PathBuf::from("~/.codex/config.toml"),
            None,
        ),
        (McpClient::Claude, PathBuf::from("~/.claude.json"), None),
        (
            McpClient::OpenCode,
            PathBuf::from("~/.config/opencode/opencode.json"),
            None,
        ),
        (
            McpClient::Antigravity,
            PathBuf::from("~/.gemini/config/mcp_config.json"),
            None,
        ),
    ];

    for profile in profiles {
        if profile.provider != oga_domain::Provider::Claude || !profile.enabled {
            continue;
        }
        if let Some(directory) = profile.env.get("CLAUDE_CONFIG_DIR") {
            let path =
                normalize_path(&expand_path(Path::new(directory), home).join(".claude.json"));
            let error = (!path_within_home(&path, home)).then(|| {
                "Set CLAUDE_CONFIG_DIR inside your home directory, then try again".to_owned()
            });
            targets.push((McpClient::Claude, path, error));
        }
    }

    targets
        .into_iter()
        .map(|(client, raw_path, path_error)| {
            let path = normalize_path(&expand_path(&raw_path, home));
            let path_error = path_error.or_else(|| {
                (!path_within_home(&path, home)).then(|| {
                    "Move the MCP config inside your home directory, then try again".to_owned()
                })
            });
            let result = match path_error {
                Some(error) => Err(error),
                None => match client {
                    McpClient::Codex => install_codex(&path, endpoint),
                    _ => install_json(client, &path, endpoint),
                },
            };
            match result {
                Ok(()) => InstallResult {
                    client: client.label().to_owned(),
                    path: pretty_path(&path, home),
                    success: true,
                    message: "Installed".to_owned(),
                },
                Err(error) => InstallResult {
                    client: client.label().to_owned(),
                    path: pretty_path(&path, home),
                    success: false,
                    message: error,
                },
            }
        })
        .collect()
}

/// Tauri command used by the webview settings page.
#[tauri::command]
pub fn install_mcp_configs(
    app: tauri::AppHandle,
    profiles: Vec<ProfileView>,
) -> Result<Vec<InstallResult>, String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|error| format!("couldn't locate the home directory: {error}"))?;
    Ok(install_everywhere_at(&profiles, &broker_mcp_url(), &home))
}

fn broker_mcp_url() -> String {
    let base = env::var("OGA_BROKER_URL").unwrap_or_else(|_| DEFAULT_BROKER_URL.to_owned());
    format!("{}/mcp", base.trim_end_matches('/'))
}

fn install_codex(path: &Path, endpoint: &str) -> Result<(), String> {
    let existing = read_or_empty(path)?;
    let header = "[mcp_servers.oga]";
    let block = format!("{header}\nurl = \"{}\"", toml_escape(endpoint));
    let mut output = Vec::new();
    let mut skipping = false;
    let mut replaced = false;

    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed == header {
            output.extend(block.lines().map(str::to_owned));
            skipping = true;
            replaced = true;
            continue;
        }
        if skipping && is_toml_table_header(trimmed) {
            skipping = false;
        }
        if !skipping {
            output.push(line.to_owned());
        }
    }

    if !replaced {
        if !output.join("\n").trim().is_empty() {
            output.push(String::new());
        }
        output.extend(block.lines().map(str::to_owned));
    }

    backup_and_write(path, format!("{}\n", output.join("\n").trim_matches('\n')))
}

fn install_json(client: McpClient, path: &Path, endpoint: &str) -> Result<(), String> {
    let existing = read_or_default(path, "{}")?;
    let sanitized = sanitize_jsonc(&existing);
    let mut root = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&sanitized)
        .map_err(|error| format!("couldn't parse existing config: {error}"))?;
    let container = if client == McpClient::OpenCode {
        "mcp"
    } else {
        "mcpServers"
    };
    let servers = root
        .entry(container)
        .or_insert_with(|| serde_json::json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| format!("{container} must be an object"))?;
    let value = match client {
        McpClient::Claude => serde_json::json!({ "type": "http", "url": endpoint }),
        McpClient::OpenCode => serde_json::json!({
            "type": "remote",
            "enabled": true,
            "url": endpoint,
            "oauth": false
        }),
        McpClient::Antigravity => serde_json::json!({ "serverUrl": endpoint }),
        McpClient::Codex => unreachable!("Codex uses TOML"),
    };
    servers.insert("oga".to_owned(), value);
    let encoded = serde_json::to_string_pretty(&root)
        .map_err(|error| format!("couldn't encode config: {error}"))?;
    let comments = jsonc_comments(&existing);
    let suffix = comments.map_or_else(String::new, |comments| format!("\n{comments}"));
    backup_and_write(path, format!("{encoded}{suffix}\n"))
}

fn read_or_empty(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.to_string()),
    }
}

fn read_or_default(path: &Path, default: &str) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(default.to_owned()),
        Err(error) => Err(error.to_string()),
    }
}

fn backup_and_write(path: &Path, text: String) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "config path has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let original_mode = existing_mode(path)?;
    if path.is_file() {
        let backup = path.with_extension(format!(
            "{}bak",
            path.extension()
                .and_then(|extension| extension.to_str())
                .map_or_else(String::new, |extension| format!("{extension}."))
        ));
        if backup.exists() {
            if !backup.is_file() {
                return Err(format!("backup path is not a file: {}", backup.display()));
            }
        } else {
            fs::copy(path, &backup).map_err(|error| error.to_string())?;
        }
    }

    let temporary = temporary_path(path);
    if let Err(error) = fs::write(&temporary, text) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    if let Err(error) = apply_mode(&temporary, original_mode) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    path.with_file_name(format!(".{name}.tmp.{}.{}", process::id(), stamp))
}

fn expand_path(path: &Path, home: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    let raw = if raw == "$HOME" {
        home.display().to_string()
    } else if let Some(rest) = raw.strip_prefix("$HOME/") {
        home.join(rest).display().to_string()
    } else {
        raw.into_owned()
    };
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    PathBuf::from(raw)
}

fn path_within_home(path: &Path, home: &Path) -> bool {
    let path = normalize_path(path);
    let home = normalize_path(home);
    if !path.starts_with(&home) {
        return false;
    }

    let Ok(canonical_home) = fs::canonicalize(&home) else {
        return false;
    };
    let mut existing = path;
    while !existing.exists() {
        if existing.file_name().is_none() {
            return false;
        }
        if !existing.pop() {
            return false;
        }
    }
    let Ok(canonical_existing) = fs::canonicalize(existing) else {
        return false;
    };
    canonical_existing.starts_with(canonical_home)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(unix)]
fn existing_mode(path: &Path) -> Result<Option<u32>, String> {
    if path.is_file() {
        Ok(Some(
            fs::metadata(path)
                .map_err(|error| error.to_string())?
                .permissions()
                .mode(),
        ))
    } else {
        Ok(None)
    }
}

#[cfg(not(unix))]
fn existing_mode(path: &Path) -> Result<Option<()>, String> {
    let _ = path;
    Ok(None)
}

#[cfg(unix)]
fn apply_mode(path: &Path, mode: Option<u32>) -> Result<(), String> {
    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_mode(path: &Path, mode: Option<()>) -> Result<(), String> {
    let _ = (path, mode);
    Ok(())
}

fn pretty_path(path: &Path, home: &Path) -> String {
    path.strip_prefix(home).map_or_else(
        |_| path.display().to_string(),
        |relative| format!("~/{}", relative.display()),
    )
}

fn is_toml_table_header(line: &str) -> bool {
    line.starts_with('[') && line.ends_with(']')
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Remove the JSONC syntax used by client config files without touching strings.
pub fn sanitize_jsonc(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];
        if in_string {
            output.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if character == '"' {
            in_string = true;
            output.push(character);
            index += 1;
            continue;
        }
        if character == '/' && chars.get(index + 1) == Some(&'/') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if character == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            let mut closed = false;
            while index < chars.len() {
                if chars[index] == '*' && chars.get(index + 1) == Some(&'/') {
                    index += 2;
                    closed = true;
                    break;
                }
                if chars[index] == '\n' {
                    output.push('\n');
                }
                index += 1;
            }
            if !closed {
                output.push('/');
                output.push('*');
            }
            continue;
        }
        if character == ','
            && let Some(next) = next_jsonc_token(&chars, index + 1)
            && (chars.get(next) == Some(&'}') || chars.get(next) == Some(&']'))
        {
            index += 1;
            continue;
        }
        output.push(character);
        index += 1;
    }

    output
}

fn next_jsonc_token(chars: &[char], mut index: usize) -> Option<usize> {
    loop {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if chars.get(index) == Some(&'/') && chars.get(index + 1) == Some(&'/') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if chars.get(index) == Some(&'/') && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            if index + 1 >= chars.len() {
                return None;
            }
            index += 2;
            continue;
        }
        return Some(index);
    }
}

fn jsonc_comments(text: &str) -> Option<String> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut comments = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if character == '"' {
            in_string = true;
            index += 1;
            continue;
        }
        if character == '/' && chars.get(index + 1) == Some(&'/') {
            let start = index;
            index += 2;
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            comments.push(chars[start..index].iter().collect::<String>());
            continue;
        }
        if character == '/' && chars.get(index + 1) == Some(&'*') {
            let start = index;
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            if index + 1 >= chars.len() {
                return None;
            }
            index += 2;
            comments.push(chars[start..index].iter().collect::<String>());
            continue;
        }
        index += 1;
    }

    (!comments.is_empty()).then(|| comments.join("\n"))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn profile(provider: oga_domain::Provider, env: &[(&str, &str)]) -> ProfileView {
        ProfileView {
            id: "worker".to_owned(),
            label: "Worker".to_owned(),
            provider,
            model: "sonnet".to_owned(),
            enabled: true,
            env: env
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>(),
            capabilities: Vec::new(),
            command: None,
        }
    }

    #[test]
    fn jsonc_sanitizer_preserves_commas_inside_strings() {
        let text = r#"{"value":"one, } two","list":[1,],}"#;

        let parsed: serde_json::Value = serde_json::from_str(&sanitize_jsonc(text)).unwrap();

        assert_eq!(parsed["value"], "one, } two");
        assert_eq!(parsed["list"], serde_json::json!([1]));
    }

    #[test]
    fn jsonc_sanitizer_removes_block_comments() {
        let text = r#"{/* keep */"value":"/* not a comment */",/* remove */}"#;

        let parsed: serde_json::Value = serde_json::from_str(&sanitize_jsonc(text)).unwrap();

        assert_eq!(parsed["value"], "/* not a comment */");
    }

    #[test]
    fn json_install_creates_a_backup_and_updates_the_oga_server() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".claude.json");
        fs::write(&path, r#"{"other":{"url":"keep"}}"#).unwrap();

        let results = install_everywhere_at(
            &[profile(oga_domain::Provider::Claude, &[])],
            "http://127.0.0.1:7331/mcp",
            home.path(),
        );

        assert!(
            results
                .iter()
                .find(|result| result.client == "Claude")
                .unwrap()
                .success
        );
        let current: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let backup = fs::read_to_string(home.path().join(".claude.json.bak")).unwrap();
        assert_eq!(
            current["mcpServers"]["oga"]["url"],
            "http://127.0.0.1:7331/mcp"
        );
        assert!(backup.contains("keep"));
    }

    #[test]
    fn json_install_keeps_the_first_backup_and_existing_mode() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".claude.json");
        fs::write(&path, r#"{"version":1}"#).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        install_json(McpClient::Claude, &path, "http://first/mcp").unwrap();
        install_json(McpClient::Claude, &path, "http://second/mcp").unwrap();

        assert_eq!(
            fs::read_to_string(home.path().join(".claude.json.bak")).unwrap(),
            r#"{"version":1}"#
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn json_install_keeps_jsonc_comments() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".claude.json");
        fs::write(
            &path,
            "{\n  // Keep this note.\n  \"other\": {\"url\": \"keep\"}\n}\n",
        )
        .unwrap();

        install_json(McpClient::Claude, &path, "http://127.0.0.1:7331/mcp").unwrap();

        assert!(
            fs::read_to_string(path)
                .unwrap()
                .contains("// Keep this note.")
        );
    }

    #[test]
    fn codex_install_replaces_only_the_oga_table() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".codex/config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "[mcp_servers.other]\nurl = \"other\"\n\n[mcp_servers.oga]\nurl = \"old\"\n\n[profiles.default]\nmodel = \"gpt\"\n",
        )
        .unwrap();

        let result = install_codex(&path, "http://new/mcp");

        assert!(result.is_ok());
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("url = \"other\""));
        assert!(text.contains("url = \"http://new/mcp\""));
        assert!(text.contains("[profiles.default]"));
        assert!(!text.contains("url = \"old\""));
    }

    #[test]
    fn custom_claude_config_dir_adds_a_second_target() {
        let home = tempfile::tempdir().unwrap();
        let results = install_everywhere_at(
            &[profile(
                oga_domain::Provider::Claude,
                &[("CLAUDE_CONFIG_DIR", "$HOME/.claude-work")],
            )],
            "http://127.0.0.1:7331/mcp",
            home.path(),
        );

        assert!(
            results
                .iter()
                .any(|result| result.path == "~/.claude-work/.claude.json"),
            "custom Claude config target should be installed"
        );
        assert!(home.path().join(".claude-work/.claude.json").is_file());
    }

    #[test]
    fn custom_claude_config_dir_outside_home_is_rejected() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let results = install_everywhere_at(
            &[profile(
                oga_domain::Provider::Claude,
                &[("CLAUDE_CONFIG_DIR", outside.path().to_str().unwrap())],
            )],
            "http://127.0.0.1:7331/mcp",
            home.path(),
        );

        let result = results
            .iter()
            .find(|result| result.path == format!("{}/.claude.json", outside.path().display()))
            .unwrap_or_else(|| panic!("results: {results:?}"));
        assert!(!result.success);
        assert!(!outside.path().join(".claude.json").exists());
    }
}
