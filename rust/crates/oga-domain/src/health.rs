//! Build identity and the health report every surface serves.

use serde::{Deserialize, Serialize};

/// The one build identity: `/health` serves it, `oga version` prints it,
/// the event-socket hello carries it, and install verification compares them.
pub const VERSION: &str = "0.6.0";
pub const MCP_CONTRACT_VERSION: u32 = 32;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Staleness {
    pub stale: bool,
    /// The source tree's current sha, shown only when a newer build exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// The `/health` body. `stale` is computed per call so a source tree that
/// moved past this build while it runs shows up instead of staying silent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub status: String,
    pub version: String,
    pub mcp_contract_version: u32,
    /// Baked build stamp; `"dev"` when running from source.
    pub build: String,
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl HealthReport {
    pub fn ok(build: impl Into<String>, staleness: &Staleness) -> Self {
        HealthReport {
            status: "ok".to_string(),
            version: VERSION.to_string(),
            mcp_contract_version: MCP_CONTRACT_VERSION,
            build: build.into(),
            stale: staleness.stale,
            current_sha: staleness.current_sha.clone(),
            hint: staleness.hint.clone(),
        }
    }
}
