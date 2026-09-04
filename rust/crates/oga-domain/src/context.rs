//! Context-map view types served by the map and query routes.

use serde::{Deserialize, Serialize};

/// One top-level declaration the context map knows about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSymbol {
    pub line: u64,
    pub end_line: u64,
    pub kind: SymbolKind,
    pub name: String,
    /// Parameter names only, `?` kept; absent when there are none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<String>,
    /// Present only when the source declares a return type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    pub exported: bool,
    /// Null until a describe pass or a worker correction wrote one.
    pub purpose: Option<String>,
    /// Cleaned rationale comment blocks attributed to this declaration.
    pub comments: Vec<String>,
    /// False once the purpose came from a worker correction or a changed
    /// signature.
    pub confirmed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Fn,
    Class,
    Type,
    Const,
    Struct,
    Enum,
    Ext,
    View,
}

/// A file's reference clues: what it imports and what it calls, names only,
/// never bodies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRefs {
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub calls: Vec<String>,
}

/// One mapped file, assembled from its entity row and its symbols' rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextFile {
    pub cwd: String,
    pub path: String,
    pub lang: SourceLang,
    pub purpose: Option<String>,
    /// Cleaned file-header and unattached top-level rationale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_comment: Option<String>,
    pub lines: u64,
    pub size: u64,
    pub mtime_ms: f64,
    pub digest: String,
    pub symbols: Vec<ContextSymbol>,
    pub status: MapFileStatus,
    pub touch_count: u64,
    pub touched_at: Option<String>,
    pub mapped_at: String,
    pub updated_at: String,
    pub refs: ContextRefs,
    /// PageRank over the project's import graph, computed at build/fold time.
    pub importance: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLang {
    Ts,
    Swift,
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapFileStatus {
    Mapped,
    Unparsed,
}

/// One project's row in context_maps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextMapRow {
    pub cwd: String,
    pub scheme: u32,
    pub state: ContextMapState,
    pub built_at: Option<String>,
    pub file_count: u64,
    pub symbol_count: u64,
    pub pending_prose: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMapState {
    Building,
    Ready,
    Partial,
}

/// One cwd's map footprint, sized without loading the rows themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextFileProject {
    pub cwd: String,
    pub file_count: u64,
    pub symbol_count: u64,
    pub pending_prose: u64,
    pub updated_at: String,
}
