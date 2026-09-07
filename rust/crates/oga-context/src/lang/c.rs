use tree_sitter::{Language, Node};

use oga_domain::SymbolKind;

use super::{LanguageAdapter, Separator};
use crate::symbols::ExtractedSymbol;

pub struct C;

impl LanguageAdapter for C {
    fn name(&self) -> &'static str {
        "c"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["c", "h"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_c::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/c.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Colons
    }

    fn exported(&self, node: Node<'_>, source: &str) -> bool {
        !has_storage_class(node, source, "static")
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path)
    }

    fn refine(&self, symbol: &mut ExtractedSymbol, _node: Node<'_>, _source: &str) {
        if symbol.kind == SymbolKind::Struct
            && symbol.parent.as_deref() == Some(symbol.name.as_str())
        {
            symbol.qualified = symbol.name.clone();
            symbol.parent = None;
        }
    }
}

fn has_storage_class(node: Node<'_>, source: &str, expected: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        child.kind() == "storage_class_specifier"
            && source.get(child.byte_range()) == Some(expected)
    })
}
