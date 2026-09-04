//! Provider process lifecycle: detached groups, bounded pipes, and cancellation.

pub mod confinement;
pub use confinement::{
    Capability, ConfinementBackend, ConfinementError, ConfinementMode, ConfinementRequest,
    LinuxBubblewrap, MacSeatbelt, NoConfinement, PreparedCommand,
};

use std::{
    collections::BTreeMap,
    fmt, io,
    path::PathBuf,
    process::ExitStatus,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub mod worker_path;

use oga_domain::Provider;
use oga_providers::{ParsedEvent, ProviderCommand, Usage, parse_stream};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    sync::{Notify, broadcast, mpsc, oneshot},
    time::sleep,
};

pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;
pub const DEFAULT_MAX_LINE_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_EVENTS: usize = 5_000;
pub const DEFAULT_EVENT_BUFFER: usize = 256;
pub const DEFAULT_INTERRUPT_GRACE: Duration = Duration::from_millis(1_500);
pub const DEFAULT_TERMINATE_GRACE: Duration = Duration::from_millis(1_500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerConfig {
    pub max_output_bytes: usize,
    pub max_line_bytes: usize,
    pub max_events: usize,
    pub event_buffer: usize,
    pub interrupt_grace: Duration,
    pub terminate_grace: Duration,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            max_events: DEFAULT_MAX_EVENTS,
            event_buffer: DEFAULT_EVENT_BUFFER,
            interrupt_grace: DEFAULT_INTERRUPT_GRACE,
            terminate_grace: DEFAULT_TERMINATE_GRACE,
        }
    }
}

impl RunnerConfig {
    fn validate(&self) -> Result<(), RunnerError> {
        if self.max_output_bytes == 0 {
            return Err(RunnerError::InvalidConfig(
                "max_output_bytes must be greater than zero".into(),
            ));
        }
        if self.max_line_bytes == 0 {
            return Err(RunnerError::InvalidConfig(
                "max_line_bytes must be greater than zero".into(),
            ));
        }
        if self.max_events == 0 {
            return Err(RunnerError::InvalidConfig(
                "max_events must be greater than zero".into(),
            ));
        }
        if self.event_buffer == 0 {
            return Err(RunnerError::InvalidConfig(
                "event_buffer must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

/// A provider invocation before any task-store state is involved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    pub provider: Provider,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub timeout: Option<Duration>,
}

impl RunRequest {
    pub fn new(provider: Provider, argv: Vec<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            provider,
            argv,
            cwd: cwd.into(),
            env: BTreeMap::new(),
            timeout: None,
        }
    }

    pub fn from_command(
        provider: Provider,
        command: ProviderCommand,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            provider,
            argv: command.argv,
            cwd: cwd.into(),
            env: command.env,
            timeout: None,
        }
    }

    /// Task-scoped values win over the profile's own: a profile can set `PWD`
    /// or an API key, but it cannot tell the worker it is a different task.
    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env.extend(env);
        self
    }

    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }
}

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("provider command is empty")]
    EmptyCommand,
    #[error("confinement failed: {0}")]
    Confinement(#[from] ConfinementError),
    #[error("invalid runner configuration: {0}")]
    InvalidConfig(String),
    #[error("could not spawn provider in {cwd}: {source}")]
    Spawn {
        cwd: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("provider {stream} pipe was not available")]
    MissingPipe { stream: OutputStream },
    #[error("could not read provider {stream}: {source}")]
    Read {
        stream: OutputStream,
        #[source]
        source: io::Error,
    },
    #[error("runner task ended without a result")]
    LostResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl OutputStream {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

impl fmt::Display for OutputStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    Exited,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub pgid: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Gone,
    Unknown(String),
}

impl ProcessIdentity {
    pub fn liveness(&self) -> Liveness {
        process_liveness(self.pid)
    }
}

/// Whether a pid is a running process, for a caller holding a number the
/// database recorded rather than a handle it owns.
pub fn process_liveness(pid: u32) -> Liveness {
    probe_process(pid)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineDropReason {
    MalformedJson,
    TooLong,
}

#[derive(Debug, Clone)]
pub enum RunnerEvent {
    Output {
        sequence: u64,
        stream: OutputStream,
        data: String,
    },
    Provider {
        sequence: u64,
        event: ParsedEvent,
    },
    LineDropped {
        sequence: u64,
        stream: OutputStream,
        bytes: usize,
        reason: LineDropReason,
    },
    OutputTruncated {
        sequence: u64,
        stream: OutputStream,
        dropped_bytes: usize,
    },
    ProcessExited {
        sequence: u64,
        exit_code: Option<i32>,
        signal: Option<i32>,
    },
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub identity: ProcessIdentity,
    pub termination: Termination,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub events: Vec<ParsedEvent>,
    pub events_dropped: usize,
    pub malformed_lines: usize,
    pub oversized_lines: usize,
    pub session_id: Option<String>,
    pub write_targets: Vec<String>,
    pub usage: Usage,
    pub elapsed: Duration,
}

#[derive(Debug, Clone)]
pub struct ProviderRunner {
    config: RunnerConfig,
    confinement: ConfinementMode,
}

pub type Runner = ProviderRunner;
pub type RunHandle = RunningProcess;

impl ProviderRunner {
    pub fn new(config: RunnerConfig) -> Result<Self, RunnerError> {
        config.validate()?;
        Ok(Self {
            config,
            confinement: ConfinementMode::None,
        })
    }

    pub fn new_with_confinement(
        config: RunnerConfig,
        confinement: ConfinementMode,
    ) -> Result<Self, RunnerError> {
        let mut runner = Self::new(config)?;
        runner.confinement = confinement;
        Ok(runner)
    }

    pub fn with_confinement(mut self, confinement: ConfinementMode) -> Self {
        self.confinement = confinement;
        self
    }

    pub fn config(&self) -> &RunnerConfig {
        &self.config
    }

    pub fn confinement(&self) -> ConfinementMode {
        self.confinement
    }

    pub fn confinement_capability(&self) -> confinement::Capability {
        self.confinement.probe()
    }

    pub async fn spawn(&self, request: RunRequest) -> Result<RunningProcess, RunnerError> {
        self.spawn_with_scope(request, Default::default()).await
    }

    pub async fn spawn_with_scope(
        &self,
        request: RunRequest,
        scope: oga_domain::TaskScope,
    ) -> Result<RunningProcess, RunnerError> {
        if request.argv.is_empty() {
            return Err(RunnerError::EmptyCommand);
        }

        // Confinement resolves the executable off this same PATH, so the
        // merge has to happen before prepare rather than at spawn.
        let mut env = BTreeMap::from([("PATH".to_owned(), worker_path::worker_path())]);
        env.extend(request.env.clone());
        let prepared = self.confinement.prepare(
            &ConfinementRequest::new(request.argv.clone(), request.cwd.clone(), scope)
                .with_env(env.clone()),
        )?;
        let mut command = Command::new(&prepared.argv[0]);
        command
            .args(&prepared.argv[1..])
            .current_dir(&request.cwd)
            .envs(&env)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        detach_process_group(&mut command);
        let mut child = command.spawn().map_err(|source| RunnerError::Spawn {
            cwd: request.cwd.clone(),
            source,
        })?;
        let Some(pid) = child.id() else {
            let _ = child.kill().await;
            return Err(RunnerError::Spawn {
                cwd: request.cwd.clone(),
                source: io::Error::other("spawned provider has no pid"),
            });
        };
        let identity = ProcessIdentity {
            pid,
            pgid: pid as i32,
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill().await;
            return Err(RunnerError::MissingPipe {
                stream: OutputStream::Stdout,
            });
        };
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill().await;
            return Err(RunnerError::MissingPipe {
                stream: OutputStream::Stderr,
            });
        };
        let (events, _) = broadcast::channel(self.config.event_buffer);
        let (provider_event_tx, provider_event_rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();
        let control = Arc::new(Control::new(
            identity.clone(),
            self.config.interrupt_grace,
            self.config.terminate_grace,
        ));
        let task_control = Arc::clone(&control);
        let config = self.config.clone();
        let event_tx = events.clone();
        let context = ProcessContext {
            request,
            identity,
            config,
            control: Arc::clone(&control),
            events: event_tx,
            provider_event_tx,
        };
        tokio::spawn(async move {
            let result = run_process(child, stdout, stderr, context).await;
            task_control.mark_finished();
            let _ = result_tx.send(result);
        });

        Ok(RunningProcess {
            identity: control.identity.clone(),
            control,
            events,
            provider_event_rx: Mutex::new(Some(provider_event_rx)),
            result: Mutex::new(Some(result_rx)),
        })
    }

    pub async fn run(&self, request: RunRequest) -> Result<RunResult, RunnerError> {
        self.run_with_scope(request, Default::default()).await
    }

    pub async fn run_with_scope(
        &self,
        request: RunRequest,
        scope: oga_domain::TaskScope,
    ) -> Result<RunResult, RunnerError> {
        let process = self.spawn_with_scope(request, scope).await?;
        process.wait().await
    }
}

impl Default for ProviderRunner {
    fn default() -> Self {
        Self::new(RunnerConfig::default()).expect("default runner configuration is valid")
    }
}

pub struct RunningProcess {
    identity: ProcessIdentity,
    control: Arc<Control>,
    events: broadcast::Sender<RunnerEvent>,
    provider_event_rx: Mutex<Option<mpsc::UnboundedReceiver<ParsedEvent>>>,
    result: Mutex<Option<oneshot::Receiver<Result<RunResult, RunnerError>>>>,
}

impl fmt::Debug for RunningProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunningProcess")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl RunningProcess {
    pub fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }

    pub fn liveness(&self) -> Liveness {
        self.identity.liveness()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RunnerEvent> {
        self.events.subscribe()
    }

    pub fn take_provider_events(&self) -> Option<mpsc::UnboundedReceiver<ParsedEvent>> {
        self.provider_event_rx
            .lock()
            .expect("provider event lock is not poisoned")
            .take()
    }

    pub async fn cancel(&self) {
        self.control.request(Termination::Cancelled);
    }

    pub fn cancel_now(&self) {
        self.control.request(Termination::Cancelled);
    }

    pub async fn wait(&self) -> Result<RunResult, RunnerError> {
        let receiver = self
            .result
            .lock()
            .expect("runner result lock is not poisoned")
            .take()
            .ok_or(RunnerError::LostResult)?;
        receiver.await.map_err(|_| RunnerError::LostResult)?
    }
}

struct Control {
    identity: ProcessIdentity,
    interrupt_grace: Duration,
    terminate_grace: Duration,
    reason: Mutex<Option<Termination>>,
    finished: AtomicBool,
    notify: Notify,
}

struct ProcessContext {
    request: RunRequest,
    identity: ProcessIdentity,
    config: RunnerConfig,
    control: Arc<Control>,
    events: broadcast::Sender<RunnerEvent>,
    provider_event_tx: mpsc::UnboundedSender<ParsedEvent>,
}

impl Control {
    fn new(
        identity: ProcessIdentity,
        interrupt_grace: Duration,
        terminate_grace: Duration,
    ) -> Self {
        Self {
            identity,
            interrupt_grace,
            terminate_grace,
            reason: Mutex::new(None),
            finished: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    fn request(self: &Arc<Self>, reason: Termination) {
        let accepted = {
            let mut current = self
                .reason
                .lock()
                .expect("termination lock is not poisoned");
            if current.is_some() {
                false
            } else {
                *current = Some(reason);
                true
            }
        };
        if !accepted {
            return;
        }
        signal_process_group(&self.identity, Signal::Interrupt);
        self.notify.notify_one();
        let control = Arc::clone(self);
        tokio::spawn(async move {
            sleep(control.interrupt_grace).await;
            if control.finished.load(Ordering::Acquire) {
                return;
            }
            signal_process_group(&control.identity, Signal::Terminate);
            sleep(control.terminate_grace).await;
            if !control.finished.load(Ordering::Acquire) {
                signal_process_group(&control.identity, Signal::Kill);
            }
        });
    }

    fn requested(&self) -> Option<Termination> {
        *self
            .reason
            .lock()
            .expect("termination lock is not poisoned")
    }

    fn mark_finished(&self) {
        self.finished.store(true, Ordering::Release);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Interrupt,
    Terminate,
    Kill,
}

#[derive(Debug)]
enum RawMessage {
    Chunk(OutputStream, Vec<u8>),
    Closed,
    Error(OutputStream, io::Error),
}

async fn read_pipe<R>(mut reader: R, stream: OutputStream, sender: mpsc::Sender<RawMessage>)
where
    R: AsyncRead + Send + Unpin,
{
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                let _ = sender.send(RawMessage::Closed).await;
                return;
            }
            Ok(length) => {
                if sender
                    .send(RawMessage::Chunk(stream, buffer[..length].to_vec()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(RawMessage::Error(stream, error)).await;
                return;
            }
        }
    }
}

async fn run_process(
    mut child: Child,
    stdout: impl AsyncRead + Send + Unpin + 'static,
    stderr: impl AsyncRead + Send + Unpin + 'static,
    context: ProcessContext,
) -> Result<RunResult, RunnerError> {
    let ProcessContext {
        request,
        identity,
        config,
        control,
        events,
        provider_event_tx,
    } = context;
    let started = Instant::now();
    let (raw_tx, mut raw_rx) = mpsc::channel(64);
    let stdout_reader = tokio::spawn(read_pipe(stdout, OutputStream::Stdout, raw_tx.clone()));
    let stderr_reader = tokio::spawn(read_pipe(stderr, OutputStream::Stderr, raw_tx.clone()));
    drop(raw_tx);

    let mut state = CaptureState::new(&config);
    let mut child_status = None;
    let mut closed_streams = 0_u8;
    let mut termination_requested = false;
    let mut timeout_fired = false;
    let mut read_error = None;
    let timeout_future = async {
        match request.timeout {
            Some(duration) => sleep(duration).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(timeout_future);

    while child_status.is_none() || closed_streams < 2 {
        if child_status.is_none() {
            tokio::select! {
                biased;
                status = child.wait() => {
                    child_status = Some(status.map_err(|source| RunnerError::Read {
                        stream: OutputStream::Stdout,
                        source,
                    })?);
                }
                message = raw_rx.recv() => {
                    match message {
                        Some(RawMessage::Closed) => closed_streams += 1,
                        Some(message) => {
                            if let Some(error) = handle_message(
                                message,
                                &mut state,
                                &events,
                                &provider_event_tx,
                                &request.provider,
                                &config,
                            ) && read_error.is_none() {
                                read_error = Some(error);
                                control.request(Termination::Cancelled);
                                termination_requested = true;
                            }
                        }
                        None => closed_streams = 2,
                    }
                }
                _ = &mut timeout_future, if !timeout_fired && request.timeout.is_some() => {
                    timeout_fired = true;
                    if child.try_wait().map_err(|source| RunnerError::Read {
                        stream: OutputStream::Stdout,
                        source,
                    })?.is_none() {
                        control.request(Termination::TimedOut);
                        termination_requested = true;
                    }
                }
                _ = control.notify.notified(), if !termination_requested => {
                    termination_requested = true;
                    #[cfg(not(unix))]
                    child.kill().await.map_err(|source| RunnerError::Read {
                        stream: OutputStream::Stdout,
                        source,
                    })?;
                }
            }
        } else if let Some(message) = raw_rx.recv().await {
            match message {
                RawMessage::Closed => closed_streams += 1,
                message => {
                    if let Some(error) = handle_message(
                        message,
                        &mut state,
                        &events,
                        &provider_event_tx,
                        &request.provider,
                        &config,
                    ) && read_error.is_none()
                    {
                        read_error = Some(error);
                    }
                }
            }
        } else {
            closed_streams = 2;
        }
    }

    let _ = stdout_reader.await;
    let _ = stderr_reader.await;
    state.finish_lines(&events, &provider_event_tx, &request.provider, &config);
    if state.stdout.dropped_bytes > 0 {
        let sequence = state.next_sequence();
        state.emit(
            &events,
            RunnerEvent::OutputTruncated {
                sequence,
                stream: OutputStream::Stdout,
                dropped_bytes: state.stdout.dropped_bytes,
            },
        );
    }
    if state.stderr.dropped_bytes > 0 {
        let sequence = state.next_sequence();
        state.emit(
            &events,
            RunnerEvent::OutputTruncated {
                sequence,
                stream: OutputStream::Stderr,
                dropped_bytes: state.stderr.dropped_bytes,
            },
        );
    }

    let status = child_status.expect("child status is set before capture finishes");
    if let Some(error) = read_error {
        return Err(error);
    }
    let (exit_code, signal) = exit_parts(status);
    let sequence = state.next_sequence();
    state.emit(
        &events,
        RunnerEvent::ProcessExited {
            sequence,
            exit_code,
            signal,
        },
    );
    let termination = control.requested().unwrap_or(Termination::Exited);
    let stdout_truncated = state.stdout.dropped_bytes > 0;
    let stderr_truncated = state.stderr.dropped_bytes > 0;
    let stdout = state.stdout.finish();
    let stderr = state.stderr.finish();
    Ok(RunResult {
        identity,
        termination,
        exit_code,
        signal,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        events: state.provider_events,
        events_dropped: state.events_dropped + state.delivery_dropped,
        malformed_lines: state.malformed_lines,
        oversized_lines: state.oversized_lines,
        session_id: state.session_id,
        write_targets: state.write_targets,
        usage: state.usage,
        elapsed: started.elapsed(),
    })
}

struct CaptureState {
    stdout: BoundedOutput,
    stderr: BoundedOutput,
    stdout_lines: LineFramer,
    stderr_lines: LineFramer,
    provider_events: Vec<ParsedEvent>,
    events_dropped: usize,
    delivery_dropped: usize,
    malformed_lines: usize,
    oversized_lines: usize,
    session_id: Option<String>,
    write_targets: Vec<String>,
    usage: Usage,
    sequence: u64,
}

impl CaptureState {
    fn new(config: &RunnerConfig) -> Self {
        Self {
            stdout: BoundedOutput::new(config.max_output_bytes),
            stderr: BoundedOutput::new(config.max_output_bytes),
            stdout_lines: LineFramer::new(config.max_line_bytes),
            stderr_lines: LineFramer::new(config.max_line_bytes),
            provider_events: Vec::with_capacity(config.max_events.min(128)),
            events_dropped: 0,
            delivery_dropped: 0,
            malformed_lines: 0,
            oversized_lines: 0,
            session_id: None,
            write_targets: Vec::new(),
            usage: Usage::default(),
            sequence: 0,
        }
    }

    fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        self.sequence
    }

    fn emit(&mut self, sender: &broadcast::Sender<RunnerEvent>, event: RunnerEvent) {
        if sender.receiver_count() == 0 {
            return;
        }
        if sender.send(event).is_err() {
            self.delivery_dropped = self.delivery_dropped.saturating_add(1);
        }
    }

    fn finish_lines(
        &mut self,
        sender: &broadcast::Sender<RunnerEvent>,
        provider_event_tx: &mpsc::UnboundedSender<ParsedEvent>,
        provider: &Provider,
        config: &RunnerConfig,
    ) {
        if let Some(action) = self.stdout_lines.finish() {
            self.process_line(
                OutputStream::Stdout,
                action,
                sender,
                provider_event_tx,
                provider,
                config,
            );
        }
        if let Some(action) = self.stderr_lines.finish() {
            self.process_line(
                OutputStream::Stderr,
                action,
                sender,
                provider_event_tx,
                provider,
                config,
            );
        }
    }

    fn process_line(
        &mut self,
        stream: OutputStream,
        action: LineAction,
        sender: &broadcast::Sender<RunnerEvent>,
        provider_event_tx: &mpsc::UnboundedSender<ParsedEvent>,
        provider: &Provider,
        config: &RunnerConfig,
    ) {
        let LineAction {
            bytes,
            oversized,
            data,
        } = action;
        if oversized {
            self.oversized_lines = self.oversized_lines.saturating_add(1);
            let sequence = self.next_sequence();
            self.emit(
                sender,
                RunnerEvent::LineDropped {
                    sequence,
                    stream,
                    bytes,
                    reason: LineDropReason::TooLong,
                },
            );
            return;
        }
        if stream != OutputStream::Stdout {
            return;
        }
        let line = String::from_utf8_lossy(&data);
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            return;
        }
        let parsed = match parse_stream(*provider, line) {
            Ok(events) => events,
            Err(_) => {
                self.malformed_lines = self.malformed_lines.saturating_add(1);
                let sequence = self.next_sequence();
                self.emit(
                    sender,
                    RunnerEvent::LineDropped {
                        sequence,
                        stream,
                        bytes,
                        reason: LineDropReason::MalformedJson,
                    },
                );
                return;
            }
        };
        for event in parsed {
            if self.provider_events.len() >= config.max_events {
                self.events_dropped = self.events_dropped.saturating_add(1);
                continue;
            }
            if self.session_id.is_none() {
                self.session_id = event.session_id.clone();
            }
            for target in &event.write_targets {
                if !self.write_targets.iter().any(|existing| existing == target) {
                    self.write_targets.push(target.clone());
                }
            }
            accumulate_usage(&mut self.usage, &event);
            self.provider_events.push(event.clone());
            if provider_event_tx.send(event.clone()).is_err() {
                self.delivery_dropped = self.delivery_dropped.saturating_add(1);
            }
            let sequence = self.next_sequence();
            self.emit(sender, RunnerEvent::Provider { sequence, event });
        }
    }
}

fn handle_message(
    message: RawMessage,
    state: &mut CaptureState,
    events: &broadcast::Sender<RunnerEvent>,
    provider_event_tx: &mpsc::UnboundedSender<ParsedEvent>,
    provider: &Provider,
    config: &RunnerConfig,
) -> Option<RunnerError> {
    match message {
        RawMessage::Chunk(stream, bytes) => {
            let output = match stream {
                OutputStream::Stdout => &mut state.stdout,
                OutputStream::Stderr => &mut state.stderr,
            };
            output.push(&bytes);
            let sequence = state.next_sequence();
            state.emit(
                events,
                RunnerEvent::Output {
                    sequence,
                    stream,
                    data: String::from_utf8_lossy(&bytes).into_owned(),
                },
            );
            let framer = match stream {
                OutputStream::Stdout => &mut state.stdout_lines,
                OutputStream::Stderr => &mut state.stderr_lines,
            };
            let actions = framer.push(&bytes);
            for action in actions {
                state.process_line(stream, action, events, provider_event_tx, provider, config);
            }
        }
        RawMessage::Closed => {}
        RawMessage::Error(stream, source) => {
            return Some(RunnerError::Read { stream, source });
        }
    }
    None
}

#[derive(Debug)]
struct BoundedOutput {
    limit: usize,
    data: Vec<u8>,
    dropped_bytes: usize,
}

impl BoundedOutput {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            data: Vec::new(),
            dropped_bytes: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
        if self.data.len() > self.limit {
            let excess = self.data.len() - self.limit;
            self.data.drain(..excess);
            self.dropped_bytes = self.dropped_bytes.saturating_add(excess);
        }
    }

    fn finish(self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

#[derive(Debug)]
struct LineFramer {
    limit: usize,
    data: Vec<u8>,
    bytes: usize,
    oversized: bool,
}

impl LineFramer {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            data: Vec::new(),
            bytes: 0,
            oversized: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Vec<LineAction> {
        let mut actions = Vec::new();
        for byte in bytes {
            if *byte == b'\n' {
                actions.push(self.take_line());
            } else {
                self.bytes = self.bytes.saturating_add(1);
                if self.data.len() < self.limit {
                    self.data.push(*byte);
                } else {
                    self.oversized = true;
                }
            }
        }
        actions
    }

    fn finish(&mut self) -> Option<LineAction> {
        (self.bytes > 0).then(|| self.take_line())
    }

    fn take_line(&mut self) -> LineAction {
        let action = LineAction {
            data: std::mem::take(&mut self.data),
            bytes: self.bytes,
            oversized: self.oversized,
        };
        self.bytes = 0;
        self.oversized = false;
        action
    }
}

#[derive(Debug)]
struct LineAction {
    data: Vec<u8>,
    bytes: usize,
    oversized: bool,
}

fn accumulate_usage(usage: &mut Usage, event: &ParsedEvent) {
    let kind = event
        .payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            event
                .payload
                .get("event")
                .and_then(serde_json::Value::as_str)
        });
    if kind == Some("result") {
        if event.usage.tokens_in.is_some() || event.usage.tokens_out.is_some() {
            usage.tokens_in = event.usage.tokens_in;
            usage.tokens_out = event.usage.tokens_out;
            usage.cached_tokens = event.usage.cached_tokens;
        }
        if event.usage.cost_usd.is_some() || event.usage.turns.is_some() {
            usage.cost_usd = event.usage.cost_usd;
            usage.turns = event.usage.turns;
        }
        return;
    }
    add_optional(&mut usage.tokens_in, event.usage.tokens_in);
    add_optional(&mut usage.tokens_out, event.usage.tokens_out);
    add_optional(&mut usage.cached_tokens, event.usage.cached_tokens);
    add_optional(&mut usage.cost_usd, event.usage.cost_usd);
    add_optional(&mut usage.turns, event.usage.turns);
}

fn add_optional(target: &mut Option<f64>, value: Option<f64>) {
    let Some(value) = value else {
        return;
    };
    *target = Some(target.unwrap_or(0.0) + value);
}

fn exit_parts(status: ExitStatus) -> (Option<i32>, Option<i32>) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        (status.code(), status.signal())
    }
    #[cfg(not(unix))]
    {
        (status.code(), None)
    }
}

#[cfg(unix)]
fn detach_process_group(command: &mut Command) {
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(not(unix))]
fn detach_process_group(_command: &mut Command) {}

/// Signal a worker's whole process group by identity alone, for a caller that
/// holds the recorded pid rather than the live handle — recovery after a
/// broker restart, where the pipes are gone but the process is not.
pub fn signal_group(identity: &ProcessIdentity, signal: Signal) {
    signal_process_group(identity, signal);
}

fn signal_process_group(identity: &ProcessIdentity, signal: Signal) {
    #[cfg(unix)]
    {
        let signal = match signal {
            Signal::Interrupt => libc::SIGINT,
            Signal::Terminate => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        unsafe {
            let _ = libc::kill(-(identity.pgid as libc::pid_t), signal);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (identity, signal);
    }
}

fn probe_process(pid: u32) -> Liveness {
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if result == 0 {
            Liveness::Alive
        } else {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                Liveness::Gone
            } else {
                Liveness::Unknown(error.to_string())
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Liveness::Unknown("process liveness is unavailable on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_framer_bounds_unterminated_lines() {
        let mut framer = LineFramer::new(4);
        assert!(framer.push(b"12345").is_empty());
        let action = framer.push(b"6\n").pop().unwrap();
        assert_eq!(action.bytes, 6);
        assert!(action.oversized);
        assert_eq!(action.data, b"1234");
    }

    #[test]
    fn usage_receipts_replace_incremental_totals() {
        let mut usage = Usage::default();
        let incremental = ParsedEvent {
            provider: Provider::Codex,
            payload: serde_json::json!({"type":"turn.completed"}),
            session_id: None,
            write_targets: vec![],
            usage: Usage {
                tokens_in: Some(2.0),
                ..Usage::default()
            },
        };
        accumulate_usage(&mut usage, &incremental);
        let receipt = ParsedEvent {
            provider: Provider::Claude,
            payload: serde_json::json!({"type":"result"}),
            session_id: None,
            write_targets: vec![],
            usage: Usage {
                tokens_in: Some(9.0),
                ..Usage::default()
            },
        };
        accumulate_usage(&mut usage, &receipt);
        assert_eq!(usage.tokens_in, Some(9.0));
    }
}
