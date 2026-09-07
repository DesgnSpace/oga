use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};

pub struct Java;

impl LanguageAdapter for Java {
    fn name(&self) -> &'static str {
        "java"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_java::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/java.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Dot
    }

    fn exported(&self, node: Node<'_>, source: &str) -> bool {
        modifier(node, source)
            .is_none_or(|modifier| !modifier.contains("private") && !modifier.contains("protected"))
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path)
    }
}

fn modifier<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    node.child_by_field_name("modifiers")
        .or_else(|| {
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .find(|child| child.kind() == "modifiers")
        })
        .and_then(|modifiers| source.get(modifiers.byte_range()))
}
