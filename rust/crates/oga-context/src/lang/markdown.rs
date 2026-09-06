use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};

pub struct Markdown;

impl LanguageAdapter for Markdown {
    fn name(&self) -> &'static str {
        "markdown"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["md", "markdown", "mdx"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_md::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/markdown.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Arrow
    }

    /// Prose has no private half.
    fn exported(&self, _node: Node<'_>, _source: &str) -> bool {
        true
    }

    /// Markdown comments are HTML comments, and they never document a
    /// heading.
    fn is_doc_comment(&self, _node: Node<'_>) -> bool {
        false
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path) || path.to_ascii_lowercase().ends_with("changelog.md")
    }
}
