use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};

pub struct Swift;

impl LanguageAdapter for Swift {
    fn name(&self) -> &'static str {
        "swift"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["swift"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_swift::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/swift.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Colons
    }

    /// Swift declares access on a `modifiers` child, and defaults to
    /// `internal`, which the rest of the module can see. Only `private` and
    /// `fileprivate` keep a declaration inside its file.
    fn exported(&self, node: Node<'_>, source: &str) -> bool {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|child| child.kind() == "modifiers")
            .is_none_or(|modifiers| {
                let text = &source[modifiers.byte_range()];
                !text.contains("private") && !text.contains("fileprivate")
            })
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path)
    }
}
