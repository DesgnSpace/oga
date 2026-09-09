//! Optional operating-system confinement for provider processes.

mod bubblewrap;
mod seatbelt;

pub use bubblewrap::LinuxBubblewrap;
pub use seatbelt::MacSeatbelt;

use std::{
    collections::BTreeMap,
    env, fmt, fs, io,
    path::{Component, Path, PathBuf},
};

use oga_domain::TaskScope;
use thiserror::Error;

/// The confinement implementation requested for a provider run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConfinementMode {
    /// Preserve migration parity and run the provider without OS confinement.
    #[default]
    None,
    /// Use the macOS Seatbelt profile interpreter.
    MacSeatbelt,
    /// Use Bubblewrap's Linux mount and process namespaces.
    LinuxBubblewrap,
}

impl fmt::Display for ConfinementMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::None => "none",
            Self::MacSeatbelt => "macOS Seatbelt",
            Self::LinuxBubblewrap => "Linux Bubblewrap",
        })
    }
}

/// The result of checking whether a backend can be used on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    Available { executable: Option<PathBuf> },
    Unavailable { reason: String },
}

impl Capability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub fn executable(&self) -> Option<&Path> {
        match self {
            Self::Available { executable } => executable.as_deref(),
            Self::Unavailable { .. } => None,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Available { .. } => None,
            Self::Unavailable { reason } => Some(reason),
        }
    }
}

/// A provider command before a backend wraps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinementRequest {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub scope: TaskScope,
    pub env: BTreeMap<String, String>,
}

impl ConfinementRequest {
    pub fn new(argv: Vec<String>, cwd: impl Into<PathBuf>, scope: TaskScope) -> Self {
        Self {
            argv,
            cwd: cwd.into(),
            scope,
            env: BTreeMap::new(),
        }
    }

    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }
}

/// The argv that the runner should execute after confinement is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCommand {
    pub argv: Vec<String>,
}

/// An OS-specific adapter that either wraps a command or refuses it.
pub trait ConfinementBackend: fmt::Debug + Send + Sync {
    fn kind(&self) -> ConfinementMode;
    fn probe(&self) -> Capability;
    fn prepare(&self, request: &ConfinementRequest) -> Result<PreparedCommand, ConfinementError>;
}

/// The explicit no-op backend used by the default runner mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoConfinement;

impl ConfinementBackend for NoConfinement {
    fn kind(&self) -> ConfinementMode {
        ConfinementMode::None
    }

    fn probe(&self) -> Capability {
        Capability::Available { executable: None }
    }

    fn prepare(&self, request: &ConfinementRequest) -> Result<PreparedCommand, ConfinementError> {
        validate_command(&request.argv)?;
        Ok(PreparedCommand {
            argv: request.argv.clone(),
        })
    }
}

impl ConfinementBackend for ConfinementMode {
    fn kind(&self) -> ConfinementMode {
        *self
    }

    fn probe(&self) -> Capability {
        match self {
            Self::None => NoConfinement.probe(),
            Self::MacSeatbelt => MacSeatbelt::default().probe(),
            Self::LinuxBubblewrap => LinuxBubblewrap::default().probe(),
        }
    }

    fn prepare(&self, request: &ConfinementRequest) -> Result<PreparedCommand, ConfinementError> {
        match self {
            Self::None => NoConfinement.prepare(request),
            Self::MacSeatbelt => MacSeatbelt::default().prepare(request),
            Self::LinuxBubblewrap => LinuxBubblewrap::default().prepare(request),
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfinementError {
    #[error("provider command is empty")]
    EmptyCommand,
    #[error("provider command argument {index} contains a NUL byte")]
    NulArgument { index: usize },
    #[error("{backend} confinement is unavailable: {reason}")]
    Unavailable {
        backend: ConfinementMode,
        reason: String,
    },
    #[error("confinement cwd is not an existing absolute directory: {path}")]
    InvalidCwd { path: PathBuf },
    #[error("invalid {field} scope rule {rule:?}: {reason}")]
    InvalidScope {
        field: &'static str,
        rule: String,
        reason: String,
    },
    #[error("{backend} cannot expose {path}: {reason}")]
    UnsupportedPath {
        backend: ConfinementMode,
        path: PathBuf,
        reason: String,
    },
    #[error("could not inspect {path}: {source}")]
    InspectPath { path: PathBuf, source: io::Error },
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedRequest {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub read: Vec<ScopeRule>,
    pub write: Vec<ScopeRule>,
    pub executable: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeRule {
    pub path: PathBuf,
    pub recursive: bool,
    pub exists: bool,
}

pub(crate) fn validate_request(
    request: &ConfinementRequest,
) -> Result<ValidatedRequest, ConfinementError> {
    validate_command(&request.argv)?;
    if !request.cwd.is_absolute()
        || !request.cwd.is_dir()
        || fs::symlink_metadata(&request.cwd).is_err()
    {
        return Err(ConfinementError::InvalidCwd {
            path: request.cwd.clone(),
        });
    }
    let cwd = fs::canonicalize(&request.cwd).map_err(|source| ConfinementError::InspectPath {
        path: request.cwd.clone(),
        source,
    })?;
    let read = validate_scope_rules(&request.scope.read, &cwd, "scope.read")?;
    let write = validate_scope_rules(&request.scope.write, &cwd, "scope.write")?;
    Ok(ValidatedRequest {
        argv: request.argv.clone(),
        cwd: cwd.clone(),
        read,
        write,
        executable: resolve_executable(request, &cwd),
    })
}

pub(crate) fn validate_command(argv: &[String]) -> Result<(), ConfinementError> {
    if argv.is_empty() || argv[0].is_empty() {
        return Err(ConfinementError::EmptyCommand);
    }
    for (index, argument) in argv.iter().enumerate() {
        if argument.contains('\0') {
            return Err(ConfinementError::NulArgument { index });
        }
    }
    Ok(())
}

fn validate_scope_rules(
    rules: &[String],
    cwd: &Path,
    field: &'static str,
) -> Result<Vec<ScopeRule>, ConfinementError> {
    rules
        .iter()
        .map(|rule| validate_scope_rule(rule, cwd, field))
        .collect()
}

fn validate_scope_rule(
    raw: &str,
    cwd: &Path,
    field: &'static str,
) -> Result<ScopeRule, ConfinementError> {
    let mut rule = raw.trim().replace('\\', "/");
    while let Some(stripped) = rule.strip_prefix("./") {
        rule = stripped.to_owned();
    }
    while rule.ends_with('/') {
        rule.pop();
    }
    let invalid = |reason: &str| ConfinementError::InvalidScope {
        field,
        rule: raw.to_owned(),
        reason: reason.to_owned(),
    };
    if rule.is_empty() {
        return Err(invalid("the path is empty"));
    }
    let path = Path::new(&rule);
    if path.is_absolute() {
        return Err(invalid("the path must stay inside cwd"));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(invalid("the path must stay inside cwd"));
    }
    let recursive = rule == "**" || rule.ends_with("/**");
    if rule.contains('*') && (!recursive || rule[..rule.len() - 3].contains('*')) {
        return Err(invalid("only a /** suffix is supported"));
    }
    let base = if rule == "**" {
        ""
    } else if recursive {
        &rule[..rule.len() - 3]
    } else {
        rule.as_str()
    };
    let lexical = if base.is_empty() {
        cwd.to_path_buf()
    } else {
        cwd.join(base)
    };
    let existing = existing_path(&lexical);
    let actual = fs::canonicalize(&existing).map_err(|source| ConfinementError::InspectPath {
        path: existing.clone(),
        source,
    })?;
    if !actual.starts_with(cwd) {
        return Err(invalid("the path resolves outside cwd"));
    }
    let exists = fs::symlink_metadata(&lexical).is_ok();
    let path = if exists {
        fs::canonicalize(&lexical).map_err(|source| ConfinementError::InspectPath {
            path: lexical.clone(),
            source,
        })?
    } else {
        lexical
    };
    let recursive = recursive || (exists && path.is_dir());
    Ok(ScopeRule {
        path,
        recursive,
        exists,
    })
}

fn existing_path(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    while fs::symlink_metadata(&current).is_err() {
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    current
}

fn resolve_executable(request: &ConfinementRequest, cwd: &Path) -> Option<PathBuf> {
    let program = Path::new(request.argv.first()?);
    if program.is_absolute() || program.components().count() > 1 {
        let candidate = if program.is_absolute() {
            program.to_path_buf()
        } else {
            cwd.join(program)
        };
        return executable_path(&candidate);
    }
    let path = request
        .env
        .get("PATH")
        .cloned()
        .or_else(|| env::var("PATH").ok())
        .unwrap_or_default();
    for directory in env::split_paths(std::ffi::OsStr::new(&path)) {
        if let Some(candidate) = executable_path(&directory.join(program)) {
            return Some(candidate);
        }
    }
    None
}

fn executable_path(path: &Path) -> Option<PathBuf> {
    (path.is_file() && is_executable(path))
        .then(|| fs::canonicalize(path).ok())
        .flatten()
}

pub(crate) fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

pub(crate) fn quote_path(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy().into_owned())
        .expect("serializing a filesystem path cannot fail")
}

pub(crate) fn ancestors(path: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut current = Some(path);
    while let Some(path) = current {
        paths.push(path.to_path_buf());
        current = path.parent();
    }
    paths.reverse();
    paths
}

pub(crate) fn unavailable(backend: ConfinementMode, capability: Capability) -> ConfinementError {
    ConfinementError::Unavailable {
        backend,
        reason: capability
            .reason()
            .unwrap_or("the capability probe returned no executable")
            .to_owned(),
    }
}
