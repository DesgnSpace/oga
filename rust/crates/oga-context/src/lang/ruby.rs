use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator};

pub struct Ruby;

impl LanguageAdapter for Ruby {
    fn name(&self) -> &'static str {
        "ruby"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rb", "rake", "gemspec"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_ruby::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/ruby.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Colons
    }

    fn exported(&self, _node: Node<'_>, _source: &str) -> bool {
        true
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path)
    }
}
