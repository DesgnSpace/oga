use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    Capability, ConfinementBackend, ConfinementError, ConfinementMode, ConfinementRequest,
    PreparedCommand, ScopeRule, ValidatedRequest, ancestors, quote_path, unavailable,
    validate_request,
};

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// macOS Seatbelt policy adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacSeatbelt {
    executable: PathBuf,
}

impl Default for MacSeatbelt {
    fn default() -> Self {
        Self {
            executable: PathBuf::from(SANDBOX_EXEC),
        }
    }
}

impl MacSeatbelt {
    pub fn with_executable(path: impl Into<PathBuf>) -> Self {
        Self {
            executable: path.into(),
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Generate a deny-by-default Seatbelt profile without running it.
    pub fn profile(&self, request: &ConfinementRequest) -> Result<String, ConfinementError> {
        let request = validate_request(request)?;
        Ok(self.profile_for(&request))
    }

    fn profile_for(&self, request: &ValidatedRequest) -> String {
        let mut lines = vec![
            "(version 1)".to_owned(),
            "(deny default)".to_owned(),
            "(import \"system.sb\")".to_owned(),
            "(allow process*)".to_owned(),
            "(allow signal)".to_owned(),
            "(allow network*)".to_owned(),
            "(allow sysctl-read)".to_owned(),
            "(allow mach-lookup)".to_owned(),
            "(allow ipc-posix-shm)".to_owned(),
        ];
        for path in ancestors(&request.cwd) {
            lines.push(format!(
                "(allow file-read-metadata (literal {}))",
                quote_path(&path)
            ));
        }
        for path in runtime_read_roots() {
            lines.push(format!(
                "(allow file-read* (subpath {}))",
                quote_path(&path)
            ));
        }
        if let Some(executable) = &request.executable {
            lines.push(format!(
                "(allow file-read* (literal {}))",
                quote_path(executable)
            ));
            for path in ancestors(executable) {
                lines.push(format!(
                    "(allow file-read-metadata (literal {}))",
                    quote_path(&path)
                ));
            }
        }
        for rule in request.read.iter().chain(&request.write) {
            for path in ancestors(&rule.path) {
                lines.push(format!(
                    "(allow file-read-metadata (literal {}))",
                    quote_path(&path)
                ));
            }
            lines.push(file_rule("file-read*", rule));
        }
        for rule in &request.write {
            lines.push(file_rule("file-write*", rule));
        }
        lines.join("\n")
    }
}

impl ConfinementBackend for MacSeatbelt {
    fn kind(&self) -> ConfinementMode {
        ConfinementMode::MacSeatbelt
    }

    fn probe(&self) -> Capability {
        if !cfg!(target_os = "macos") {
            return Capability::Unavailable {
                reason: "the Seatbelt interpreter is only available on macOS".into(),
            };
        }
        if !self.executable.is_file() || !super::is_executable(&self.executable) {
            return Capability::Unavailable {
                reason: format!("{} is not an executable file", self.executable.display()),
            };
        }
        Capability::Available {
            executable: Some(self.executable.clone()),
        }
    }

    fn prepare(&self, request: &ConfinementRequest) -> Result<PreparedCommand, ConfinementError> {
        let capability = self.probe();
        if !capability.is_available() {
            return Err(unavailable(self.kind(), capability));
        }
        let validated = validate_request(request)?;
        let profile = self.profile_for(&validated);
        let mut argv = vec![
            self.executable.to_string_lossy().into_owned(),
            "-p".into(),
            profile,
        ];
        let mut command = validated.argv;
        if let Some(executable) = validated.executable {
            command[0] = executable.to_string_lossy().into_owned();
        }
        argv.extend(command);
        Ok(PreparedCommand { argv })
    }
}

fn file_rule(operation: &str, rule: &ScopeRule) -> String {
    let filter = if rule.recursive { "subpath" } else { "literal" };
    format!("(allow {operation} ({filter} {}))", quote_path(&rule.path))
}

fn runtime_read_roots() -> Vec<PathBuf> {
    [
        "/System",
        "/usr",
        "/bin",
        "/sbin",
        "/Library",
        "/dev",
        "/private/etc",
        "/private/var/db",
        "/private/var/select",
        "/private/var/run",
    ]
    .into_iter()
    .map(PathBuf::from)
    .filter(|path| fs::metadata(path).is_ok())
    .collect()
}
