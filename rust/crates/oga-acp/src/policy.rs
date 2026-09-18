//! What the client will do on the agent's behalf, and nothing more.

use std::{future::Future, pin::Pin};

use agent_client_protocol_schema::v1::{
    CreateTerminalRequest, Error, KillTerminalRequest, PermissionOptionId, ReadTextFileRequest,
    ReleaseTerminalRequest, RequestPermissionRequest, TerminalOutputRequest,
    WaitForTerminalExitRequest, WriteTextFileRequest,
};
use serde_json::Value;

pub type PolicyFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The callbacks the client offers the agent. Every field starts `false`, and
/// the handshake advertises exactly what is set here, so an agent never learns
/// about a door the caller did not open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Grants {
    pub read_text_file: bool,
    pub write_text_file: bool,
    pub terminal: bool,
}

impl Grants {
    pub fn read_text_file(mut self, granted: bool) -> Self {
        self.read_text_file = granted;
        self
    }

    pub fn write_text_file(mut self, granted: bool) -> Self {
        self.write_text_file = granted;
        self
    }

    pub fn terminal(mut self, granted: bool) -> Self {
        self.terminal = granted;
        self
    }
}

/// The client's answer to a permission request. The agent offers the options;
/// the policy picks one, treats the turn as cancelled, or refuses to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Select(PermissionOptionId),
    Cancel,
    Refuse { message: String },
}

/// A terminal call, handed over whole so a policy that grants terminals owns
/// their semantics rather than inheriting any from the transport.
#[derive(Debug, Clone)]
pub enum TerminalCall {
    Create(Box<CreateTerminalRequest>),
    Output(TerminalOutputRequest),
    WaitForExit(WaitForTerminalExitRequest),
    Kill(KillTerminalRequest),
    Release(ReleaseTerminalRequest),
}

/// The decisions a client makes while a turn is running.
///
/// Every method denies by default. Granting access takes two deliberate steps:
/// setting the matching flag in [`Grants`], which is what the handshake
/// advertises, and overriding the matching method, which is what actually
/// answers. A policy that does one without the other still denies.
pub trait AcpPolicy: Send + Sync + 'static {
    fn grants(&self) -> Grants {
        Grants::default()
    }

    fn permission(&self, request: RequestPermissionRequest) -> PolicyFuture<'_, Decision> {
        let _ = request;
        Box::pin(async {
            Decision::Refuse {
                message: "this client answers no permission requests".into(),
            }
        })
    }

    fn read_text_file(
        &self,
        request: ReadTextFileRequest,
    ) -> PolicyFuture<'_, Result<String, Error>> {
        let _ = request;
        Box::pin(async { Err(denied("fs/read_text_file")) })
    }

    fn write_text_file(
        &self,
        request: WriteTextFileRequest,
    ) -> PolicyFuture<'_, Result<(), Error>> {
        let _ = request;
        Box::pin(async { Err(denied("fs/write_text_file")) })
    }

    /// Answers a `terminal/*` call with that method's ACP response body.
    fn terminal(&self, call: TerminalCall) -> PolicyFuture<'_, Result<Value, Error>> {
        let _ = call;
        Box::pin(async { Err(denied("terminal")) })
    }
}

pub(crate) fn denied(capability: &str) -> Error {
    Error::method_not_found().data(Value::String(format!(
        "this client does not grant {capability}"
    )))
}

/// The policy a session gets when the caller names none: it advertises nothing
/// and answers nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAll;

impl AcpPolicy for DenyAll {}
