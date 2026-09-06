use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};
use crate::symbols::ExtractedSymbol;
use crate::text::clean_comment;

pub struct Python;

impl LanguageAdapter for Python {
    fn name(&self) -> &'static str {
        "python"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["py", "pyi"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/python.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Dot
    }

    /// Python has no access keywords. A leading underscore is the whole
    /// convention, and it applies to a module-level name as much as to a
    /// method.
    fn exported(&self, node: Node<'_>, source: &str) -> bool {
        declared_name(node, source).is_some_and(|name| !name.starts_with('_'))
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path)
    }

    /// A module-level assignment is only worth a symbol when it names a
    /// constant. Everything else at that level is module state that no
    /// question asks for by name.
    fn is_noise_node(&self, node: Node<'_>, source: &str) -> bool {
        node.kind() == "assignment"
            && node
                .parent()
                .and_then(|parent| parent.parent())
                .map(|grandparent| grandparent.kind())
                == Some("module")
            && !declared_name(node, source).is_some_and(is_screaming_case)
    }

    /// Python documents a declaration from inside it: the docstring is the
    /// first statement of the body.
    fn refine(&self, symbol: &mut ExtractedSymbol, node: Node<'_>, source: &str) {
        if symbol.doc.is_none() {
            symbol.doc = docstring(node, source);
        }
    }
}

fn declared_name<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    let name = node
        .child_by_field_name("name")
        .or_else(|| node.child_by_field_name("left"))?;
    source.get(name.byte_range())
}

fn is_screaming_case(name: &str) -> bool {
    name.chars().any(|char| char.is_ascii_uppercase())
        && !name.chars().any(|char| char.is_ascii_lowercase())
}

fn docstring(node: Node<'_>, source: &str) -> Option<String> {
    let first = node.child_by_field_name("body")?.named_child(0)?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let literal = first.named_child(0)?;
    if literal.kind() != "string" {
        return None;
    }
    let content = literal
        .named_children(&mut literal.walk())
        .find(|child| child.kind() == "string_content")?;
    clean_comment(source.get(content.byte_range())?)
}
