//! Profile, config, grant, and memory types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::task::{Provider, TaskScope};

/// A worker profile: one provider account and how to launch it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub label: String,
    pub provider: Provider,
    pub default_model: String,
    pub enabled: bool,
    pub env: BTreeMap<String, String>,
    pub capabilities: Vec<String>,
    /// Command-line override replacing the provider's own argv.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

/// What HTTP profile routes serve: the profile with its default model under
/// the wire name `model`, and secrets masked. Also the app's on-disk shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    pub id: String,
    pub label: String,
    pub provider: Provider,
    pub model: String,
    pub enabled: bool,
    pub env: BTreeMap<String, String>,
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

impl From<&Profile> for ProfileView {
    fn from(profile: &Profile) -> Self {
        ProfileView {
            id: profile.id.clone(),
            label: profile.label.clone(),
            provider: profile.provider,
            model: profile.default_model.clone(),
            enabled: profile.enabled,
            env: profile.env.clone(),
            capabilities: profile.capabilities.clone(),
            command: profile.command.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

/// A scope stated for a cwd, kept so later delegations into the same project
/// can reuse it without asking again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeGrant {
    pub id: String,
    pub cwd: String,
    /// Approval is per destination, not just per folder.
    pub profile_id: String,
    pub scope: TaskScope,
    pub created_at: String,
    pub last_used_at: String,
    pub use_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub cwd: String,
    pub key: String,
    pub value: String,
    pub version: u64,
    pub created_at: String,
    pub updated_at: String,
}

/// One cwd's memory footprint, sized without loading the values themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryProject {
    pub cwd: String,
    pub count: u64,
    pub chars: u64,
    pub updated_at: String,
}
