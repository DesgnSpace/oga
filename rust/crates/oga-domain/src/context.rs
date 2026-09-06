//! Context-map view types served by the map and query routes.

use serde::{Deserialize, Serialize};

/// One declaration the index knows about, at any nesting depth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSymbol {
    pub kind: SymbolKind,
    pub name: String,
    /// The name with its enclosing symbols, joined the way the language reads:
    /// `Store::open`, `TaskList.render`, `Install > Homebrew`.
    pub qualified: String,
    pub line: u64,
    pub end_line: u64,
    /// The declaration text up to its body, on one line.
    pub signature: String,
    /// The doc comment attached to the declaration, markers stripped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Whether the language treats this as visible outside its file.
    pub exported: bool,
    /// Qualified name of the enclosing symbol, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Fn,
    Method,
    Struct,
    Class,
    Enum,
    Variant,
    Trait,
    Impl,
    Type,
    Const,
    Static,
    Module,
    Field,
    Macro,
    Heading,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Fn => "fn",
            SymbolKind::Method => "method",
            SymbolKind::Struct => "struct",
            SymbolKind::Class => "class",
            SymbolKind::Enum => "enum",
            SymbolKind::Variant => "variant",
            SymbolKind::Trait => "trait",
            SymbolKind::Impl => "impl",
            SymbolKind::Type => "type",
            SymbolKind::Const => "const",
            SymbolKind::Static => "static",
            SymbolKind::Module => "module",
            SymbolKind::Field => "field",
            SymbolKind::Macro => "macro",
            SymbolKind::Heading => "heading",
        }
    }

    pub fn parse(value: &str) -> Option<SymbolKind> {
        let kind = match value {
            "fn" => SymbolKind::Fn,
            "method" => SymbolKind::Method,
            "struct" => SymbolKind::Struct,
            "class" => SymbolKind::Class,
            "enum" => SymbolKind::Enum,
            "variant" => SymbolKind::Variant,
            "trait" => SymbolKind::Trait,
            "impl" => SymbolKind::Impl,
            "type" => SymbolKind::Type,
            "const" => SymbolKind::Const,
            "static" => SymbolKind::Static,
            "module" => SymbolKind::Module,
            "field" => SymbolKind::Field,
            "macro" => SymbolKind::Macro,
            "heading" => SymbolKind::Heading,
            _ => return None,
        };
        Some(kind)
    }
}

/// One indexed file, assembled from its file row and its symbol rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextFile {
    pub cwd: String,
    pub path: String,
    /// The adapter that parsed the file: `rust`, `typescript`, `swift`, `markdown`.
    pub lang: String,
    pub lines: u64,
    pub size: u64,
    pub mtime_ms: f64,
    pub digest: String,
    pub symbols: Vec<ContextSymbol>,
    pub updated_at: String,
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
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMapState {
    Building,
    Ready,
    Partial,
}
