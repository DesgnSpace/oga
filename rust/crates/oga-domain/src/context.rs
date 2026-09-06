//! Symbol kinds shared by the code index and its callers.

use serde::{Deserialize, Serialize};

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
