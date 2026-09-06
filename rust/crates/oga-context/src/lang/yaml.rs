use tree_sitter::{Language, Node};

use super::{LanguageAdapter, Separator, config};
use crate::symbols::ExtractedSymbol;

pub struct Yaml;

impl LanguageAdapter for Yaml {
    fn name(&self) -> &'static str {
        "yaml"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["yaml", "yml"]
    }

    fn language(&self, _extension: &str) -> Language {
        tree_sitter_yaml::LANGUAGE.into()
    }

    fn query(&self, _extension: &str) -> &'static str {
        include_str!("queries/yaml.scm")
    }

    fn separator(&self) -> Separator {
        Separator::Dot
    }

    fn exported(&self, _node: Node<'_>, _source: &str) -> bool {
        true
    }

    fn is_noise(&self, path: &str) -> bool {
        super::is_support_path(path) || config::is_lockfile(path)
    }

    fn refine(&self, symbol: &mut ExtractedSymbol, node: Node<'_>, source: &str) {
        config::refine(symbol, node, source);
    }
}
