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
        KillTerminalRequest, LoadSessionRequest, LoadSessionResponse, McpServer, NewSessionRequest,
        NewSessionResponse, PromptRequest, PromptResponse, ReadTextFileRequest,
        ReadTextFileResponse, ReleaseTerminalRequest, RequestPermissionOutcome,
        RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest,
        ResumeSessionResponse, SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption,
        SessionConfigSelectOptions, SessionId, SessionNotification, SetSessionConfigOptionRequest,
        SetSessionConfigOptionResponse, TerminalOutputRequest, WaitForTerminalExitRequest,
        WriteTextFileRequest, WriteTextFileResponse,
    },
};
use oga_domain::{AcpSteering, TaskScope};
use oga_runner::{ProcessControl, ProviderRunner, RunRequest, Termination};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use crate::{
    outcome::{AcpError, Refusal, Stage},
    policy::{AcpPolicy, Decision, Grants, PolicyFuture, TerminalCall, denied},
    transport::{
        Connection, DEFAULT_MAX_FRAME_BYTES, DEFAULT_MAX_STDERR_BYTES, Diagnostics, Handler,
        RpcError, Sent,
    },
};

pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// `initialize` only exchanges capabilities, so an agent that has not answered
/// long after the slowest healthy start is stuck rather than busy. It is held
/// well short of the rest of the handshake, which may replay a conversation
/// before it answers.
pub const DEFAULT_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(15);
pub const DEFAULT_PROMPT_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// How long an agent has to say what it did with an instruction. The turn it
/// is running is not waited on, only the answer about the instruction.
const STEER_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a prompt that joined a turn is given to answer once that turn has
/// ended. It answers off the same idle event as the turn's own prompt, so an
/// answer that is coming is already in flight and a longer wait only holds the
/// run open behind an agent that will never send one.
const JOINED_TIMEOUT: Duration = Duration::from_secs(5);
/// The extension request that carries an instruction into a running turn,
/// advertised at `_meta.steering.supported` on the initialize response.
const STEER_METHOD: &str = "_session/steering";

/// What an agent did with an instruction sent into the turn it is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Steered {
    /// It went into the turn that is running.
    Injected,
    /// It did not, in the agent's own word for what happened instead.
    Elsewhere(String),
}

/// A prompt the agent has been given and not yet answered, sent to carry an
/// instruction into a turn already under way.
pub struct SentPrompt(Sent<PromptResponse>);

impl SentPrompt {
    /// Reads the answer and lets it go. It reports the turn the instruction
    /// joined, which that turn's own prompt has already settled, so reading it
    /// clears the frame rather than producing an outcome.
    pub async fn settled(self) {
        let _: Result<PromptResponse, RpcError> = self.0.answer(JOINED_TIMEOUT).await;
    }
}

/// How the transport behaves, independent of which agent it is talking to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpConfig {
    pub protocol_version: ProtocolVersion,
    pub client_info: Implementation,
    /// The bound on creating or restoring a session.
    pub handshake_timeout: Duration,
    /// The bound on `initialize`, ahead of any session.
    pub initialize_timeout: Duration,
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
            initialize_timeout: DEFAULT_INITIALIZE_TIMEOUT,
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
    /// Session settings to select, in order, before the prompt.
    pub settings: Vec<SessionSetting>,
    /// The released agent this launch was verified against. Any other agent
    /// answering the command is turned away before a session opens.
    pub release: Option<AgentRelease>,
}

/// A released agent, by the name it reports and the versions of it that share
/// the behaviour a caller relies on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRelease {
    pub name: String,
    pub versions: AgentVersions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentVersions {
    /// The patch releases of one `major.minor` line.
    Line(String),
    /// One build and no other, for an agent published only as numbered
    /// builds that promise nothing between them.
    Build(String),
}

impl std::fmt::Display for AgentVersions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Line(line) => write!(formatter, "{line}.x"),
            Self::Build(build) => formatter.write_str(build),
        }
    }
}

impl AgentRelease {
    pub fn line(name: impl Into<String>, line: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            versions: AgentVersions::Line(line.into()),
        }
    }

    pub fn build(name: impl Into<String>, build: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            versions: AgentVersions::Build(build.into()),
        }
    }

    /// Whether the agent is one of these versions. A pre-release carries a
    /// suffix after its patch number, so it is never part of a line.
    fn admits(&self, agent: &Implementation) -> bool {
        agent.name == self.name
            && match &self.versions {
                AgentVersions::Line(line) => agent
                    .version
                    .strip_prefix(line.as_str())
                    .and_then(|rest| rest.strip_prefix('.'))
                    .is_some_and(|patch| {
                        !patch.is_empty() && patch.bytes().all(|b| b.is_ascii_digit())
                    }),
                AgentVersions::Build(build) => agent.version == *build,
            }
    }
}

/// A value the session has to hold before the prompt, picked from the choices
/// the agent advertises for one of its session settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSetting {
    pub id: String,
    pub value: String,
    /// Whether an agent that offers no such choice leaves the session
    /// unusable. A setting that is not required is left to the agent instead.
    pub required: bool,
}

impl SessionSetting {
    pub fn required(id: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            required: true,
        }
    }

    pub fn if_offered(id: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            required: false,
            ..Self::required(id, value)
        }
    }
}

impl Launch {
    pub fn new(request: RunRequest, scope: TaskScope, start: SessionStart) -> Self {
        Self {
            request,
            scope,
            start,
            additional_directories: Vec::new(),
            mcp_servers: Vec::new(),
            settings: Vec::new(),
            release: None,
        }
    }

    pub fn release(mut self, release: AgentRelease) -> Self {
        self.release = Some(release);
        self
    }

    pub fn settings(mut self, settings: Vec<SessionSetting>) -> Self {
        self.settings = settings;
        self
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
                let error = with_last_words(error, &connection.diagnostics());
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
        let answer: Result<Value, RpcError> = self
            .connection
            .request(
                AGENT_METHOD_NAMES.session_prompt,
                &request,
                self.prompt_timeout,
            )
            .await;
        let answer = answer.map_err(|error| match error {
            RpcError::NotSent { .. } => AcpError::unavailable(
                Stage::Session,
                "the agent closed before the prompt was written",
            ),
            RpcError::Agent { code, message, .. } if code == i32::from(ErrorCode::AuthRequired) => {
                AcpError::refused(Refusal::Authentication, message)
            }
            RpcError::Closed => AcpError::in_flight(self.closing_reason()),
            error @ RpcError::Agent { .. } => AcpError::turn_failed(error.to_string()),
            other => AcpError::in_flight(other.to_string()),
        })?;
        serde_json::from_value(spec_stop_reason(answer)).map_err(|error| {
            AcpError::in_flight(format!(
                "could not read the agent's answer to session/prompt: {error}"
            ))
        })
    }

    /// How this agent takes an instruction while a turn is running, from what
    /// it advertised. `None` from one that never said it can.
    pub fn steering(&self) -> Option<AcpSteering> {
        self.agent
            .meta
            .as_ref()
            .and_then(|meta| meta.get("steering"))
            .and_then(|steering| steering.get("supported"))
            .and_then(Value::as_bool)
            .unwrap_or_default()
            .then_some(AcpSteering::Extension)
    }

    /// Hands the agent an instruction for the turn it is already running.
    ///
    /// The turn is left to answer its own prompt: this call only carries the
    /// instruction and reads back what became of it. Asking the agent to
    /// require a prompt when it is idle keeps it from starting a turn nobody
    /// is waiting on, so an instruction that arrives a moment too late is
    /// still this client's to place.
    pub async fn steer(&self, blocks: Vec<ContentBlock>) -> Result<Steered, AcpError> {
        let request = json!({
            "sessionId": self.session_id,
            "prompt": blocks,
            "_meta": {"steering": {"idleBehavior": "promptRequired"}},
        });
        let answer: Value = self
            .connection
            .request(STEER_METHOD, &request, STEER_TIMEOUT)
            .await
            .map_err(|error| AcpError::unavailable(Stage::Session, error.to_string()))?;
        Ok(match answer["outcome"].as_str() {
            Some("injected") => Steered::Injected,
            Some(other) => Steered::Elsewhere(other.to_owned()),
            None => Steered::Elsewhere("no outcome".into()),
        })
    }

    /// Hands the agent an instruction as a prompt of its own, without waiting
    /// for it to be answered.
    ///
    /// An agent that folds a second prompt into the run it is already on takes
    /// the instruction into that turn. Both prompts then answer for the same
    /// turn, so this one's answer is left unread for the caller to drain once
    /// the turn it joined has ended.
    pub fn send_prompt(&self, blocks: Vec<ContentBlock>) -> Result<SentPrompt, AcpError> {
        let request = PromptRequest::new(self.session_id.clone(), blocks);
        self.connection
            .start_request(AGENT_METHOD_NAMES.session_prompt, &request)
            .map(SentPrompt)
            .map_err(|error| AcpError::unavailable(Stage::Session, error.to_string()))
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
            config.initialize_timeout,
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
    if let Some(release) = &launch.release
        && !agent
            .agent_info
            .as_ref()
            .is_some_and(|info| release.admits(info))
    {
        let answered = agent.agent_info.as_ref().map_or_else(
            || "an agent that doesn't say which it is".to_owned(),
            |info| format!("{} {}", info.name, info.version),
        );
        return Err(AcpError::unavailable(
            Stage::Initialize,
            format!(
                "Oga works with {} {}, not {answered}",
                release.name, release.versions
            ),
        ));
    }

    let (session_id, offered) = open_session(connection, launch, cwd, &agent, config).await?;
    configure(connection, &session_id, offered, &launch.settings, config).await?;
    Ok((agent, session_id))
}

/// Opens the session and returns the settings the agent advertises for it.
async fn open_session(
    connection: &Connection,
    launch: &Launch,
    cwd: &Path,
    agent: &InitializeResponse,
    config: &AcpConfig,
) -> Result<(SessionId, Vec<SessionConfigOption>), AcpError> {
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
            Ok((answer.session_id, answer.config_options.unwrap_or_default()))
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
            let answer = connection
                .request::<_, Value>(
                    AGENT_METHOD_NAMES.session_load,
                    &request,
                    config.handshake_timeout,
                )
                .await
                .map_err(|error| classify_handshake(Stage::Session, error))?;
            let offered = serde_json::from_value::<LoadSessionResponse>(answer)
                .ok()
                .and_then(|answer| answer.config_options);
            Ok((session_id.clone(), offered.unwrap_or_default()))
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
            let answer = connection
                .request::<_, Value>(
                    AGENT_METHOD_NAMES.session_resume,
                    &request,
                    config.handshake_timeout,
                )
                .await
                .map_err(|error| classify_handshake(Stage::Session, error))?;
            let offered = serde_json::from_value::<ResumeSessionResponse>(answer)
                .ok()
                .and_then(|answer| answer.config_options);
            Ok((session_id.clone(), offered.unwrap_or_default()))
        }
    }
}

/// Selects each setting in order, answering from the choices the agent last
/// advertised, since one choice can change what the next one offers. A choice
/// the agent does not advertise is never guessed at: a required one leaves the
/// session unusable, and any other is left to the agent.
async fn configure(
    connection: &Connection,
    session_id: &SessionId,
    mut offered: Vec<SessionConfigOption>,
    settings: &[SessionSetting],
    config: &AcpConfig,
) -> Result<(), AcpError> {
    for setting in settings {
        match choice(&offered, setting) {
            Choice::Held => continue,
            Choice::Offered => {}
            Choice::Missing(_) if !setting.required => continue,
            Choice::Missing(reason) => return Err(AcpError::unavailable(Stage::Configure, reason)),
        }
        let answer = connection
            .request::<_, Value>(
                AGENT_METHOD_NAMES.session_set_config_option,
                &SetSessionConfigOptionRequest::new(
                    session_id.clone(),
                    setting.id.clone(),
                    setting.value.as_str(),
                ),
                config.handshake_timeout,
            )
            .await
            .map_err(|error| classify_handshake(Stage::Configure, error))?;
        if let Ok(answer) = serde_json::from_value::<SetSessionConfigOptionResponse>(answer) {
            offered = answer.config_options;
        }
    }
    Ok(())
}

enum Choice {
    /// The session already holds the value.
    Held,
    Offered,
    Missing(String),
}

fn choice(offered: &[SessionConfigOption], setting: &SessionSetting) -> Choice {
    let Some(option) = offered
        .iter()
        .find(|option| option.id.0.as_ref() == setting.id)
    else {
        return Choice::Missing(format!("the agent offers no {} choices", setting.id));
    };
    let SessionConfigKind::Select(select) = &option.kind else {
        return Choice::Missing(format!(
            "the agent's {} setting isn't a list of choices",
            setting.id
        ));
    };
    if select.current_value.0.as_ref() == setting.value {
        return Choice::Held;
    }
    let values: Vec<&str> = match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .map(|option| option.value.0.as_ref())
            .collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| &group.options)
            .map(|option| option.value.0.as_ref())
            .collect(),
        _ => Vec::new(),
    };
    if values.contains(&setting.value.as_str()) {
        Choice::Offered
    } else {
        Choice::Missing(format!(
            "the agent doesn't offer {} as a {} choice",
            setting.value, setting.id
        ))
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

/// How much of the agent's stderr travels with a handshake that failed.
const LAST_WORDS_CHARS: usize = 400;
const LAST_WORDS_LINES: usize = 3;

/// Adds what the agent wrote to stderr to a handshake that failed. An agent
/// that never answered leaves nothing in the protocol to report, so otherwise
/// the only account of the failure is that it did not answer. That it went
/// quiet is itself worth saying: it separates an agent that explained itself
/// from one that simply stopped.
fn with_last_words(error: AcpError, diagnostics: &Diagnostics) -> AcpError {
    let AcpError::Unavailable { stage, reason } = error else {
        return error;
    };
    let lines: Vec<&str> = diagnostics
        .stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let said = if lines.is_empty() {
        "the agent wrote nothing before it stopped".to_owned()
    } else {
        let tail = lines[lines.len().saturating_sub(LAST_WORDS_LINES)..].join(" / ");
        let clipped: String = tail.chars().take(LAST_WORDS_CHARS).collect();
        format!("the agent last wrote: {clipped}")
    };
    AcpError::unavailable(stage, format!("{reason}; {said}"))
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

/// The turn's end as the schema spells it. fx answers `refused` where the
/// schema names that reason `refusal`, and a turn that ended must still be
/// readable when an agent spells one of its words its own way.
fn spec_stop_reason(mut answer: Value) -> Value {
    if answer.get("stopReason").and_then(Value::as_str) == Some("refused") {
        answer["stopReason"] = Value::from("refusal");
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_admits_itself_only() {
        let build = AgentRelease::build("OpenCode", "0.0.0-beta-18999");
        let agent = |name: &str, version: &str| Implementation::new(name, version);

        assert!(build.admits(&agent("OpenCode", "0.0.0-beta-18999")));
        for version in [
            "0.0.0-beta-19000",
            "0.0.0-beta-1899",
            "0.0.0-beta-189990",
            "1.18.31",
        ] {
            assert!(!build.admits(&agent("OpenCode", version)), "{version}");
        }
        assert!(!build.admits(&agent("opencode", "0.0.0-beta-18999")));
    }

    #[test]
    fn a_release_line_admits_its_patch_releases_only() {
        let line = AgentRelease::line("@agentclientprotocol/codex-acp", "1.12");
        let agent = |name: &str, version: &str| Implementation::new(name, version);

        assert!(line.admits(&agent("@agentclientprotocol/codex-acp", "1.12.0")));
        assert!(line.admits(&agent("@agentclientprotocol/codex-acp", "1.12.14")));
        for version in [
            "1.13.0",
            "1.1.20",
            "1.12",
            "1.12.",
            "1.12.1-preview.2",
            "2.12.0",
        ] {
            assert!(
                !line.admits(&agent("@agentclientprotocol/codex-acp", version)),
                "{version}"
            );
        }
        assert!(!line.admits(&agent("codex-acp", "1.12.0")));
    }
}
