//! What a caller is allowed to do after an ACP turn goes wrong.

use std::fmt;

use thiserror::Error;

/// How far the handshake got before the transport gave up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Spawn,
    Initialize,
    Session,
    /// Choosing the session's settings, after it opened and before any prompt.
    Configure,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Initialize => "initialize",
            Self::Session => "session",
            Self::Configure => "configure",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why the agent turned the client away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The agent wants an authentication method the client has not completed.
    Authentication,
    /// The agent declined an operation the client asked for.
    Permission,
}

impl Refusal {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Permission => "permission",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// An ACP turn that did not finish, sorted by what the caller may safely do
/// next. Only [`AcpError::Unavailable`] happens strictly before the prompt
/// leaves the client, so it is the only variant another transport may retry.
#[derive(Debug, Error)]
pub enum AcpError {
    /// The agent never became usable and no prompt was written to it.
    #[error("ACP is unavailable at {stage}: {reason}")]
    Unavailable { stage: Stage, reason: String },
    /// The agent turned the client away. No turn ran, but running the same
    /// work another way would step around the refusal rather than answer it.
    #[error("agent refused the client on {kind}: {reason}")]
    Refused { kind: Refusal, reason: String },
    /// The prompt reached the agent's stdin. Whatever happened afterwards, the
    /// turn may have run, so it must be reported rather than run again.
    #[error("the prompt was sent and its outcome is unknown: {reason}")]
    PromptInFlight { reason: String },
    /// The agent answered the prompt with an error of its own. The turn ended
    /// partway, but the agent is still running and its session can take
    /// another prompt.
    #[error("the agent ended the turn with an error: {reason}")]
    TurnFailed { reason: String },
}

impl AcpError {
    pub(crate) fn unavailable(stage: Stage, reason: impl Into<String>) -> Self {
        Self::Unavailable {
            stage,
            reason: reason.into(),
        }
    }

    pub(crate) fn refused(kind: Refusal, reason: impl Into<String>) -> Self {
        Self::Refused {
            kind,
            reason: reason.into(),
        }
    }

    pub(crate) fn in_flight(reason: impl Into<String>) -> Self {
        Self::PromptInFlight {
            reason: reason.into(),
        }
    }

    pub(crate) fn turn_failed(reason: impl Into<String>) -> Self {
        Self::TurnFailed {
            reason: reason.into(),
        }
    }

    /// Whether the same work can run through another transport without any
    /// chance of the agent having already started it.
    pub fn allows_retry_elsewhere(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }
}
