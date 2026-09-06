use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};

pub struct Rust;

impl LanguageAdapter for Rust {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/rust.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Colons
    }

    /// `pub`, in any of its restricted forms. An `impl` block carries no
    /// visibility of its own and is reachable wherever its type is.
    fn exported(&self, node: Node<'_>, source: &str) -> bool {
        if node.kind() == "impl_item" {
            return true;
        }
        let mut cursor = node.walk();
        node.children(&mut cursor).any(|child| {
            child.kind() == "visibility_modifier" && source[child.byte_range()].starts_with("pub")
        })
    }

    /// `//!` and `/*!` document the module they sit in, never the item that
    /// happens to follow them.
    fn is_doc_comment(&self, node: Node<'_>) -> bool {
        if !matches!(node.kind(), "line_comment" | "block_comment") {
            return false;
        }
        let mut cursor = node.walk();
        !node
            .children(&mut cursor)
            .any(|child| child.kind() == "inner_doc_comment_marker")
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path)
    }

    /// Rust keeps its tests beside the code they cover. They are the biggest
    /// single source of names that answer a question wrongly, so the whole
    /// `#[cfg(test)]` module goes.
    fn is_noise_node(&self, node: Node<'_>, source: &str) -> bool {
        if node.kind() == "mod_item"
            && node
                .child_by_field_name("name")
                .and_then(|name| source.get(name.byte_range()))
                .is_some_and(|name| name == "tests" || name == "test")
        {
            return true;
        }
        let mut previous = node.prev_named_sibling();
        while let Some(candidate) = previous {
            if candidate.kind() != "attribute_item" {
                return false;
            }
            if source
                .get(candidate.byte_range())
                .is_some_and(|text| text.contains("test"))
            {
                return true;
            }
            previous = candidate.prev_named_sibling();
        }
        false
    }
}
