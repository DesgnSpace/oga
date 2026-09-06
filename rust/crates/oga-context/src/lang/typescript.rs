use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};

pub struct TypeScript;

const TSX: &[&str] = &["tsx", "jsx"];
const PLAIN_JS: &[&str] = &["js", "mjs", "cjs"];

impl LanguageAdapter for TypeScript {
    fn name(&self) -> &'static str {
        "typescript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"]
    }

    /// JSX changes how `<` parses, so TSX gets its own grammar. Plain
    /// JavaScript gets its own too, because the TypeScript grammar rejects
    /// syntax that is legal in a `.js` file.
    fn language(&self, extension: &str) -> Language {
        if TSX.contains(&extension) {
            tree_sitter_typescript::LANGUAGE_TSX.into()
        } else if PLAIN_JS.contains(&extension) {
            tree_sitter_javascript::LANGUAGE.into()
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
        }
    }

    fn query(&self, extension: &str) -> &'static str {
        if PLAIN_JS.contains(&extension) {
            include_str!("queries/javascript.scm")
        } else {
            include_str!("queries/typescript.scm")
        }
    }

    fn separator(&self) -> Separator {
        Separator::Dot
    }

    /// A top-level declaration is exported when `export` precedes it. A class
    /// or interface member is exported unless it is marked private or named
    /// with the private-by-convention prefixes.
    fn exported(&self, node: Node<'_>, source: &str) -> bool {
        if is_class_member(node) {
            let mut cursor = node.walk();
            let restricted = node.children(&mut cursor).any(|child| {
                child.kind() == "accessibility_modifier" && &source[child.byte_range()] != "public"
            });
            let name = node
                .child_by_field_name("name")
                .map(|name| &source[name.byte_range()])
                .unwrap_or_default();
            return !restricted && !name.starts_with('#') && !name.starts_with('_');
        }
        let mut current = Some(node);
        while let Some(here) = current {
            if here.kind() == "export_statement" {
                return true;
            }
            current = here.parent();
        }
        false
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path)
    }
}

fn is_class_member(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "method_definition"
            | "method_signature"
            | "abstract_method_signature"
            | "public_field_definition"
            | "field_definition"
            | "property_signature"
    )
}
