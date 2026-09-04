use std::{
    env,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

use oga_client::{ClientError, LoopbackClient};
use oga_domain::HealthReport;
use serde::Serialize;
use thiserror::Error;
use tokio::time::{sleep, timeout};

const HEALTH_ATTEMPTS: usize = 20;
const HEALTH_TIMEOUT: Duration = Duration::from_secs(1);
const HEALTH_INTERVAL: Duration = Duration::from_millis(350);

#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("invalid broker URL: {0}")]
    Client(Box<ClientError>),
    #[error("OGA_SERVER_PATH does not point to an executable: {0}")]
    MissingExecutable(PathBuf),
    #[error("could not start the Rust broker at {path}: {source}")]
    Spawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not stop the Rust broker: {0}")]
    Stop(#[source] std::io::Error),
    #[error("Rust broker executable is unavailable: {0}")]
    Unavailable(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BrokerStatus {
    Connecting,
    Starting,
    Running,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerSnapshot {
    pub status: BrokerStatus,
    pub url: String,
    pub managed: bool,
    pub health: Option<HealthReport>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerCommand {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: Option<PathBuf>,
}

impl BrokerCommand {
    pub fn resolve(resource_directory: Option<&Path>) -> Result<Self, BrokerError> {
        if let Some(path) = env::var_os("OGA_SERVER_PATH") {
            return Self::from_path(PathBuf::from(path), None);
        }

        if let Some(resource_directory) = resource_directory {
            let bundled = resource_directory.join("oga-server");
            if bundled.is_file() {
                return Self::from_path(bundled, None);
            }
        }

        // Tauri places the sidecar next to the app executable.
        if let Some(sidecar) = env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("oga-server")))
            .filter(|path| path.is_file())
        {
            return Self::from_path(sidecar, None);
        }

        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for name in ["oga-cli", "oga-server", "oga"] {
            let development_binary = workspace.join("target/debug").join(name);
            if development_binary.is_file() {
                return Self::from_path(development_binary, Some(workspace));
            }
        }

        Err(BrokerError::MissingExecutable(
            workspace.join("target/debug/oga-cli"),
        ))
    }

    fn from_path(
        executable: PathBuf,
        working_directory: Option<PathBuf>,
    ) -> Result<Self, BrokerError> {
        if !executable.is_file() {
            return Err(BrokerError::MissingExecutable(executable));
        }
        Ok(Self {
            executable,
            arguments: vec!["serve".to_owned()],
            working_directory,
        })
    }
}

pub struct BrokerSupervisor {
    client: LoopbackClient,
    command: Option<BrokerCommand>,
    command_error: Option<String>,
    child: Option<Child>,
    snapshot: BrokerSnapshot,
}

impl BrokerSupervisor {
    pub fn new(resource_directory: Option<&Path>) -> Result<Self, BrokerError> {
        let client =
            LoopbackClient::from_env().map_err(|error| BrokerError::Client(Box::new(error)))?;
        let (command, command_error) = match BrokerCommand::resolve(resource_directory) {
            Ok(command) => (Some(command), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let snapshot = BrokerSnapshot {
            status: BrokerStatus::Connecting,
            url: client.base_url().to_string(),
            managed: false,
            health: None,
            error: None,
        };
        Ok(Self {
            client,
            command,
            command_error,
            child: None,
            snapshot,
        })
    }

    pub fn snapshot(&self) -> BrokerSnapshot {
        self.snapshot.clone()
    }

    pub async fn connect_or_start(&mut self) -> Result<BrokerSnapshot, BrokerError> {
        if let Some(health) = self.health().await {
            self.set_running(health, false);
            return Ok(self.snapshot());
        }

        if let Some(child) = self.child.as_mut()
            && child.try_wait().map_err(BrokerError::Stop)?.is_some()
        {
            self.child = None;
        }

        if self.child.is_none() {
            let Some(command) = self.command.clone() else {
                let error = BrokerError::Unavailable(
                    self.command_error
                        .clone()
                        .unwrap_or_else(|| "no broker executable was configured".to_owned()),
                );
                self.set_offline(error.to_string());
                return Err(error);
            };
            self.snapshot.status = BrokerStatus::Starting;
            self.snapshot.error = None;
            if let Err(error) = self.spawn(&command) {
                self.set_offline(error.to_string());
                return Err(error);
            }
        } else {
            self.snapshot.status = BrokerStatus::Starting;
            self.snapshot.error = None;
        }

        for _ in 0..HEALTH_ATTEMPTS {
            if let Some(health) = self.health().await {
                self.set_running(health, true);
                return Ok(self.snapshot());
            }
            if let Some(child) = self.child.as_mut()
                && child.try_wait().map_err(BrokerError::Stop)?.is_some()
            {
                self.child = None;
                break;
            }
            sleep(HEALTH_INTERVAL).await;
        }

        self.snapshot.status = BrokerStatus::Offline;
        self.snapshot.managed = self.child.is_some();
        self.snapshot.error = Some("Oga did not become ready".to_owned());
        Ok(self.snapshot())
    }

    pub fn shutdown(&mut self) -> Result<(), BrokerError> {
        let Some(mut child) = self.child.take() else {
            self.snapshot.status = BrokerStatus::Offline;
            self.snapshot.managed = false;
            self.snapshot.health = None;
            return Ok(());
        };

        if child.try_wait().map_err(BrokerError::Stop)?.is_none() {
            child.kill().map_err(BrokerError::Stop)?;
            child.wait().map_err(BrokerError::Stop)?;
        }
        self.snapshot.status = BrokerStatus::Offline;
        self.snapshot.managed = false;
        self.snapshot.health = None;
        Ok(())
    }

    async fn health(&self) -> Option<HealthReport> {
        timeout(HEALTH_TIMEOUT, self.client.health())
            .await
            .ok()
            .and_then(Result::ok)
    }

    fn set_running(&mut self, health: HealthReport, managed: bool) {
        self.snapshot.status = BrokerStatus::Running;
        self.snapshot.managed = managed;
        self.snapshot.health = Some(health);
        self.snapshot.error = None;
    }

    fn set_offline(&mut self, error: String) {
        self.snapshot.status = BrokerStatus::Offline;
        self.snapshot.managed = self.child.is_some();
        self.snapshot.health = None;
        self.snapshot.error = Some(error);
    }

    fn spawn(&mut self, command: &BrokerCommand) -> Result<(), BrokerError> {
        let mut child_command = Command::new(&command.executable);
        child_command
            .args(&command.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(directory) = &command.working_directory {
            child_command.current_dir(directory);
        }
        if let Some(port) = self.client.base_url().port() {
            child_command.env("OGA_PORT", port.to_string());
        }
        self.child = Some(child_command.spawn().map_err(|source| BrokerError::Spawn {
            path: command.executable.clone(),
            source,
        })?);
        Ok(())
    }
}

impl Drop for BrokerSupervisor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn command_uses_the_bundled_server_when_present() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let server = directory.path().join("oga-server");
        fs::write(&server, "server").expect("server fixture");

        let command = BrokerCommand::from_path(server.clone(), None).expect("command");

        assert_eq!(command.executable, server);
        assert_eq!(command.arguments, vec!["serve".to_owned()]);
    }

    #[test]
    fn missing_server_is_reported_without_a_fallback() {
        let missing = PathBuf::from("/tmp/oga-server-missing");

        let result = BrokerCommand::from_path(missing.clone(), None);

        assert!(matches!(result, Err(BrokerError::MissingExecutable(path)) if path == missing));
    }
}
