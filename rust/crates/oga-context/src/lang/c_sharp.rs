use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};

pub struct CSharp;

impl LanguageAdapter for CSharp {
    fn name(&self) -> &'static str {
        "csharp"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["cs"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_c_sharp::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/c_sharp.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Dot
    }

    fn exported(&self, node: Node<'_>, source: &str) -> bool {
        let mut cursor = node.walk();
        !node.children(&mut cursor).any(|child| {
            child.kind() == "modifier"
                && matches!(
                    source.get(child.byte_range()),
                    Some("private" | "protected")
                )
        })
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path)
    }
}
