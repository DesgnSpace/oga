use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};

pub struct Cpp;

impl LanguageAdapter for Cpp {
    fn name(&self) -> &'static str {
        "cpp"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["cc", "cp", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_cpp::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/cpp.scm")
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
}

fn has_storage_class(node: Node<'_>, source: &str, expected: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        child.kind() == "storage_class_specifier"
            && source.get(child.byte_range()) == Some(expected)
    })
}
