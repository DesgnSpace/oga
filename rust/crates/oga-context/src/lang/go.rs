use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};
use crate::symbols::ExtractedSymbol;

pub struct Go;

impl LanguageAdapter for Go {
    fn name(&self) -> &'static str {
        "go"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/go.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Dot
    }

    /// Go spells visibility with a capital letter and nothing else.
    fn exported(&self, node: Node<'_>, source: &str) -> bool {
        node.child_by_field_name("name")
            .and_then(|name| source.get(name.byte_range()))
            .and_then(|name| name.chars().next())
            .is_some_and(|first| first.is_uppercase())
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path) || path.ends_with("_test.go")
    }

    /// A method sits beside its type rather than inside it, so the tree never
    /// nests the two. The receiver type is what makes `Heal` findable as
    /// `Route.Heal`.
    fn refine(&self, symbol: &mut ExtractedSymbol, node: Node<'_>, source: &str) {
        let Some(receiver) = receiver_type(node, source) else {
            return;
        };
        symbol.qualified = format!("{receiver}.{}", symbol.name);
        symbol.parent = Some(receiver.to_owned());
    }
}

/// The named type a method hangs off, with the pointer star and the binding
/// name dropped: `(r *Route)` is `Route`.
fn receiver_type<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    let receiver = node.child_by_field_name("receiver")?;
    let declaration = receiver.named_child(0)?;
    let mut kind = declaration.child_by_field_name("type")?;
    while kind.kind() == "pointer_type" || kind.kind() == "generic_type" {
        kind = kind.named_child(0)?;
    }
    source.get(kind.byte_range())
}
