//! ACP client protocol transport, policy, sessions, and outcomes.

mod outcome;
mod policy;
mod session;
mod transport;

pub use agent_client_protocol_schema::{ProtocolVersion, v1 as schema};
pub use outcome::{AcpError, Refusal, Stage};
pub use policy::{AcpPolicy, Decision, DenyAll, Grants, PolicyFuture, TerminalCall};
pub use session::{
    AcpConfig, AcpSession, AgentRelease, AgentVersions, DEFAULT_HANDSHAKE_TIMEOUT,
    DEFAULT_INITIALIZE_TIMEOUT, DEFAULT_PROMPT_TIMEOUT, Exit, Launch, SentPrompt, SessionSetting,
    SessionStart, Steered,
};
pub use transport::{DEFAULT_MAX_FRAME_BYTES, DEFAULT_MAX_STDERR_BYTES, Diagnostics};
