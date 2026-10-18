use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};

pub struct Css;

impl LanguageAdapter for Css {
    fn name(&self) -> &'static str {
        "css"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["css"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_css::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/css.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Dot
    }

    fn exported(&self, _node: Node<'_>, _source: &str) -> bool {
        true
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path) || path.ends_with(".min.css")
    }
}
