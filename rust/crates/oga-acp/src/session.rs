//! An ACP turn: handshake, session, prompt, and how each of those can fail.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use agent_client_protocol_schema::{
    ProtocolVersion,
    v1::{
        AGENT_METHOD_NAMES, AgentCapabilities, CLIENT_METHOD_NAMES, CancelNotification,
        ClientCapabilities, ContentBlock, CreateTerminalRequest, Error, ErrorCode,
        FileSystemCapabilities, Implementation, InitializeRequest, InitializeResponse,
        KillTerminalRequest, LoadSessionRequest, McpServer, NewSessionRequest, NewSessionResponse,
        PromptRequest, PromptResponse, ReadTextFileRequest, ReadTextFileResponse,
        ReleaseTerminalRequest, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, ResumeSessionRequest, SelectedPermissionOutcome, SessionId,
        SessionNotification, TerminalOutputRequest, WaitForTerminalExitRequest,
        WriteTextFileRequest, WriteTextFileResponse,
    },
};
use oga_domain::TaskScope;
use oga_runner::{ProcessControl, ProviderRunner, RunRequest, Termination};
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use crate::{
    outcome::{AcpError, Refusal, Stage},
    policy::{AcpPolicy, Decision, Grants, PolicyFuture, TerminalCall, denied},
    transport::{
        Connection, DEFAULT_MAX_FRAME_BYTES, DEFAULT_MAX_STDERR_BYTES, Diagnostics, Handler,
        RpcError,
    },
};

pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_PROMPT_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// How the transport behaves, independent of which agent it is talking to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpConfig {
    pub protocol_version: ProtocolVersion,
    pub client_info: Implementation,
    /// The bound on `initialize` and on creating or restoring a session.
    pub handshake_timeout: Duration,
    /// The bound on one prompt turn.
    pub prompt_timeout: Duration,
    pub max_frame_bytes: usize,
    pub max_stderr_bytes: usize,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            protocol_version: ProtocolVersion::V1,
            client_info: Implementation::new("oga", env!("CARGO_PKG_VERSION")),
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            prompt_timeout: DEFAULT_PROMPT_TIMEOUT,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            max_stderr_bytes: DEFAULT_MAX_STDERR_BYTES,
        }
    }
}

/// Which conversation the turn belongs to. An ACP session id is the agent's
/// own and is not the provider session id Oga already records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStart {
    New,
    /// Restores a conversation and replays its history as notifications.
    Load(SessionId),
    /// Restores a conversation without replaying it.
    Resume(SessionId),
}

/// Everything about the session to open, apart from who answers for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The agent process to start.
    pub request: RunRequest,
    /// What the task may read and write, for OS confinement.
    pub scope: TaskScope,
    /// Which conversation this turn belongs to.
    pub start: SessionStart,
    /// Workspace roots beyond the process working directory.
    pub additional_directories: Vec<PathBuf>,
    /// MCP servers the agent should connect to.
    pub mcp_servers: Vec<McpServer>,
}

impl Launch {
    pub fn new(request: RunRequest, scope: TaskScope, start: SessionStart) -> Self {
        Self {
            request,
            scope,
            start,
            additional_directories: Vec::new(),
            mcp_servers: Vec::new(),
        }
    }

    pub fn additional_directories(mut self, directories: Vec<PathBuf>) -> Self {
        self.additional_directories = directories;
        self
    }

    pub fn mcp_servers(mut self, servers: Vec<McpServer>) -> Self {
        self.mcp_servers = servers;
        self
    }
}

/// How a child ended, for a caller that has already read its prompt outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// A live ACP conversation with one agent process.
///
/// The prompt turn and the child's lifetime are separate: [`Self::prompt`]
/// returns when the agent answers, whether or not the process is still
/// running, and [`Self::shutdown`] is what ends the process.
pub struct AcpSession {
    connection: Connection,
    process: ProcessControl,
    exit: watch::Receiver<Option<Exit>>,
    reaper: Mutex<Option<tokio::task::JoinHandle<()>>>,
    session_id: SessionId,
    agent: InitializeResponse,
    updates: Mutex<Option<mpsc::UnboundedReceiver<SessionNotification>>>,
    prompt_timeout: Duration,
}

impl std::fmt::Debug for AcpSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AcpSession")
            .field("session_id", &self.session_id)
            .field("pid", &self.process.identity().pid)
            .finish_non_exhaustive()
    }
}

impl AcpSession {
    /// Starts an agent, shakes hands, and opens a session. Everything here
    /// happens before any prompt exists, so every failure short of a refusal
    /// leaves the work safe to run another way.
    pub async fn open<P: AcpPolicy>(
        runner: &ProviderRunner,
        launch: Launch,
        policy: Arc<P>,
        config: AcpConfig,
    ) -> Result<Self, AcpError> {
        let cwd = launch.request.cwd.clone();
        let mut process = runner
            .spawn_duplex(launch.request.clone(), launch.scope.clone())
            .await
            .map_err(|error| AcpError::unavailable(Stage::Spawn, error.to_string()))?;
        let control = process.control();
        let Some((stdin, stdout, stderr)) = process.take_pipes() else {
            control.terminate(Termination::Cancelled);
            return Err(AcpError::unavailable(
                Stage::Spawn,
                "the agent was started without usable pipes",
            ));
        };

        let (updates_tx, updates_rx) = mpsc::unbounded_channel();
        let grants = policy.grants();
        let handler = Arc::new(ClientHandler {
            policy,
            grants,
            updates: updates_tx,
        });
        let connection = Connection::start(
            stdin,
            stdout,
            stderr,
            handler,
            config.max_frame_bytes,
            config.max_stderr_bytes,
        );

        let (exit_tx, exit) = watch::channel(None);
        let reaper = tokio::spawn(async move {
            let status = process.wait().await.ok();
            let _ = exit_tx.send(status.map(|status| Exit {
                code: status.code(),
                signal: exit_signal(&status),
            }));
        });

        let opening = handshake(&connection, grants, &launch, &cwd, &config).await;
        let (agent, session_id) = match opening {
            Ok(opened) => opened,
            Err(error) => {
                control.terminate(Termination::Cancelled);
                reaper.abort();
                return Err(error);
            }
        };

        Ok(Self {
            connection,
            process: control,
            exit,
            reaper: Mutex::new(Some(reaper)),
            session_id,
            agent,
            updates: Mutex::new(Some(updates_rx)),
            prompt_timeout: config.prompt_timeout,
        })
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn agent_capabilities(&self) -> &AgentCapabilities {
        &self.agent.agent_capabilities
    }

    pub fn agent_info(&self) -> Option<&Implementation> {
        self.agent.agent_info.as_ref()
    }

    pub fn protocol_version(&self) -> ProtocolVersion {
        self.agent.protocol_version
    }

    pub fn process(&self) -> &ProcessControl {
        &self.process
    }

    pub fn diagnostics(&self) -> Diagnostics {
        self.connection.diagnostics()
    }

    /// The agent's `session/update` stream, taken once.
    pub fn take_updates(&self) -> Option<mpsc::UnboundedReceiver<SessionNotification>> {
        self.updates
            .lock()
            .expect("update lock is not poisoned")
            .take()
    }

    /// Sends one prompt and waits for the agent to say why the turn ended.
    ///
    /// Everything after the frame is queued counts as sent: the turn may have
    /// run, so a failure from here is reported rather than retried.
    pub async fn prompt(&self, blocks: Vec<ContentBlock>) -> Result<PromptResponse, AcpError> {
        let request = PromptRequest::new(self.session_id.clone(), blocks);
        let answer: Result<PromptResponse, RpcError> = self
            .connection
            .request(
                AGENT_METHOD_NAMES.session_prompt,
                &request,
                self.prompt_timeout,
            )
            .await;
        answer.map_err(|error| match error {
            RpcError::NotSent { .. } => AcpError::unavailable(
                Stage::Session,
                "the agent closed before the prompt was written",
            ),
            RpcError::Agent { code, message, .. } if code == i32::from(ErrorCode::AuthRequired) => {
                AcpError::refused(Refusal::Authentication, message)
            }
            RpcError::Closed => AcpError::in_flight(self.closing_reason()),
            other => AcpError::in_flight(other.to_string()),
        })
    }

    /// Asks the agent to stop the turn. The prompt still answers, with
    /// `StopReason::Cancelled` from an agent that follows the protocol.
    pub fn cancel(&self) {
        let _ = self.connection.notify(
            AGENT_METHOD_NAMES.session_cancel,
            &CancelNotification::new(self.session_id.clone()),
        );
    }

    /// Ends the agent process and waits for it, escalating through the
    /// runner's signal grace periods.
    pub async fn shutdown(&self) -> Option<Exit> {
        self.process.terminate(Termination::Cancelled);
        let reaper = self
            .reaper
            .lock()
            .expect("reaper lock is not poisoned")
            .take();
        if let Some(reaper) = reaper {
            let _ = reaper.await;
        }
        *self.exit.borrow()
    }

    fn closing_reason(&self) -> String {
        match *self.exit.borrow() {
            Some(Exit {
                code: Some(code), ..
            }) => format!("the agent exited with status {code} before answering the prompt"),
            Some(Exit {
                signal: Some(signal),
                ..
            }) => format!("the agent was killed by signal {signal} before answering the prompt"),
            _ => "the agent closed the connection before answering the prompt".into(),
        }
    }
}

async fn handshake(
    connection: &Connection,
    grants: Grants,
    launch: &Launch,
    cwd: &Path,
    config: &AcpConfig,
) -> Result<(InitializeResponse, SessionId), AcpError> {
    let agent: InitializeResponse = connection
        .request(
            AGENT_METHOD_NAMES.initialize,
            &InitializeRequest::new(config.protocol_version)
                .client_capabilities(capabilities_for(grants))
                .client_info(config.client_info.clone()),
            config.handshake_timeout,
        )
        .await
        .map_err(|error| classify_handshake(Stage::Initialize, error))?;

    if agent.protocol_version != config.protocol_version {
        return Err(AcpError::unavailable(
            Stage::Initialize,
            format!(
                "the agent speaks ACP {} and this client speaks {}",
                agent.protocol_version.as_u16(),
                config.protocol_version.as_u16()
            ),
        ));
    }

    let session_id = open_session(connection, launch, cwd, &agent, config).await?;
    Ok((agent, session_id))
}

async fn open_session(
    connection: &Connection,
    launch: &Launch,
    cwd: &Path,
    agent: &InitializeResponse,
    config: &AcpConfig,
) -> Result<SessionId, AcpError> {
    match &launch.start {
        SessionStart::New => {
            let answer: NewSessionResponse = connection
                .request(
                    AGENT_METHOD_NAMES.session_new,
                    &NewSessionRequest::new(cwd)
                        .additional_directories(launch.additional_directories.clone())
                        .mcp_servers(launch.mcp_servers.clone()),
                    config.handshake_timeout,
                )
                .await
                .map_err(|error| classify_handshake(Stage::Session, error))?;
            Ok(answer.session_id)
        }
        SessionStart::Load(session_id) => {
            if !agent.agent_capabilities.load_session {
                return Err(AcpError::unavailable(
                    Stage::Session,
                    "the agent cannot load an existing session",
                ));
            }
            let request = LoadSessionRequest::new(session_id.clone(), cwd)
                .additional_directories(launch.additional_directories.clone())
                .mcp_servers(launch.mcp_servers.clone());
            connection
                .request::<_, Value>(
                    AGENT_METHOD_NAMES.session_load,
                    &request,
                    config.handshake_timeout,
                )
                .await
                .map_err(|error| classify_handshake(Stage::Session, error))?;
            Ok(session_id.clone())
        }
        SessionStart::Resume(session_id) => {
            if agent
                .agent_capabilities
                .session_capabilities
                .resume
                .is_none()
            {
                return Err(AcpError::unavailable(
                    Stage::Session,
                    "the agent cannot resume an existing session",
                ));
            }
            let request = ResumeSessionRequest::new(session_id.clone(), cwd)
                .additional_directories(launch.additional_directories.clone())
                .mcp_servers(launch.mcp_servers.clone());
            connection
                .request::<_, Value>(
                    AGENT_METHOD_NAMES.session_resume,
                    &request,
                    config.handshake_timeout,
                )
                .await
                .map_err(|error| classify_handshake(Stage::Session, error))?;
            Ok(session_id.clone())
        }
    }
}

/// Nothing here has run a turn, so only a refusal is worth keeping apart: an
/// agent that wants credentials must not be worked around.
fn classify_handshake(stage: Stage, error: RpcError) -> AcpError {
    match error {
        RpcError::Agent { code, message, .. } if code == i32::from(ErrorCode::AuthRequired) => {
            AcpError::refused(Refusal::Authentication, message)
        }
        other => AcpError::unavailable(stage, other.to_string()),
    }
}

/// The handshake advertises exactly what the policy grants, so an agent never
/// learns about a callback the caller did not open.
fn capabilities_for(grants: Grants) -> ClientCapabilities {
    ClientCapabilities::new()
        .fs(FileSystemCapabilities::new()
            .read_text_file(grants.read_text_file)
            .write_text_file(grants.write_text_file))
        .terminal(grants.terminal)
}

struct ClientHandler<P: AcpPolicy> {
    policy: Arc<P>,
    grants: Grants,
    updates: mpsc::UnboundedSender<SessionNotification>,
}

impl<P: AcpPolicy> ClientHandler<P> {
    async fn answer(&self, method: &str, params: Value) -> Result<Value, Error> {
        let names = CLIENT_METHOD_NAMES;
        if method == names.session_request_permission {
            return self.permission(decode(params)?).await;
        }
        if method == names.fs_read_text_file {
            let request: ReadTextFileRequest = decode(params)?;
            self.gate(self.grants.read_text_file, "fs/read_text_file")?;
            let content = self.policy.read_text_file(request).await?;
            return encode(&ReadTextFileResponse::new(content));
        }
        if method == names.fs_write_text_file {
            let request: WriteTextFileRequest = decode(params)?;
            self.gate(self.grants.write_text_file, "fs/write_text_file")?;
            self.policy.write_text_file(request).await?;
            return encode(&WriteTextFileResponse::new());
        }
        if let Some(call) = terminal_call(method, params)? {
            self.gate(self.grants.terminal, "terminal")?;
            return self.policy.terminal(call).await;
        }
        Err(Error::method_not_found().data(Value::String(method.to_owned())))
    }

    async fn permission(&self, request: RequestPermissionRequest) -> Result<Value, Error> {
        let outcome = match self.policy.permission(request).await {
            Decision::Select(option) => {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option))
            }
            Decision::Cancel => RequestPermissionOutcome::Cancelled,
            Decision::Refuse { message } => {
                return Err(Error::new(i32::from(ErrorCode::InvalidRequest), message));
            }
        };
        encode(&RequestPermissionResponse::new(outcome))
    }

    fn gate(&self, granted: bool, capability: &str) -> Result<(), Error> {
        if granted {
            return Ok(());
        }
        Err(denied(capability))
    }
}

fn terminal_call(method: &str, params: Value) -> Result<Option<TerminalCall>, Error> {
    let names = CLIENT_METHOD_NAMES;
    let call = if method == names.terminal_create {
        TerminalCall::Create(Box::new(decode::<CreateTerminalRequest>(params)?))
    } else if method == names.terminal_output {
        TerminalCall::Output(decode::<TerminalOutputRequest>(params)?)
    } else if method == names.terminal_wait_for_exit {
        TerminalCall::WaitForExit(decode::<WaitForTerminalExitRequest>(params)?)
    } else if method == names.terminal_kill {
        TerminalCall::Kill(decode::<KillTerminalRequest>(params)?)
    } else if method == names.terminal_release {
        TerminalCall::Release(decode::<ReleaseTerminalRequest>(params)?)
    } else {
        return Ok(None);
    };
    Ok(Some(call))
}

impl<P: AcpPolicy> Handler for ClientHandler<P> {
    fn request(
        self: Arc<Self>,
        method: String,
        params: Value,
    ) -> PolicyFuture<'static, Result<Value, Error>> {
        Box::pin(async move { self.answer(&method, params).await })
    }

    fn notification(self: Arc<Self>, method: String, params: Value) {
        if method != CLIENT_METHOD_NAMES.session_update {
            return;
        }
        if let Ok(notification) = serde_json::from_value::<SessionNotification>(params) {
            let _ = self.updates.send(notification);
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, Error> {
    serde_json::from_value(params)
        .map_err(|error| Error::invalid_params().data(Value::String(error.to_string())))
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Value, Error> {
    serde_json::to_value(value)
        .map_err(|error| Error::internal_error().data(Value::String(error.to_string())))
}

#[cfg(unix)]
fn exit_signal(status: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}

#[cfg(not(unix))]
fn exit_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}
