//! Talking ACP to a provider process.
//!
//! The CLI runner reads a provider to exit; this one holds a JSON-RPC
//! conversation with it instead. Both share [`oga_runner`]'s spawning, so
//! confinement, environment filtering, and the detached process group are the
//! same either way.
//!
//! Three things shape the surface:
//!
//! - The wire types are [`agent_client_protocol_schema`], pinned to the
//!   released stable protocol. No unstable feature is required, so an agent's
//!   token usage stays unknown rather than guessed at.
//! - A caller supplies an [`AcpPolicy`]. Filesystem and terminal callbacks are
//!   denied and unadvertised until that policy grants them.
//! - A failure says whether the prompt was ever written. Only
//!   [`AcpError::Unavailable`] happens strictly before that, so it is the only
//!   one another transport may pick up.

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
