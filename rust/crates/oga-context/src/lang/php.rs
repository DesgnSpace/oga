use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};

pub struct Php;

impl LanguageAdapter for Php {
    fn name(&self) -> &'static str {
        "php"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["php"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_php::LANGUAGE_PHP.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/php.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Colons
    }

    /// A member with no visibility keyword is public, which is why the absence
    /// of the modifier counts as exported. Everything outside a class body is
    /// reachable from anywhere that imports it.
    fn exported(&self, node: Node<'_>, source: &str) -> bool {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|child| child.kind() == "visibility_modifier")
            .is_none_or(|modifier| &source[modifier.byte_range()] == "public")
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path)
    }
}
