//! Context map lookups for the MCP surface, answered from the shared index
//! after it is reconciled against disk.

use oga_context::{BuildOptions, ContextIndex, ContextTarget, QueryOptions, RenderTier};
use oga_domain::TaskScope;
use oga_http::HttpState;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MapOptions {
    #[serde(default)]
    pub path: Vec<String>,
    #[serde(default)]
    pub symbol: Vec<String>,
    pub q: Option<String>,
    pub tier: Option<String>,
    pub depth: Option<u64>,
}

pub fn lookup(state: &HttpState, cwd: &str, options: &MapOptions) -> Result<String, String> {
    let cwd = canonical_directory(cwd)?;
    let index = refreshed(state, &cwd)?;
    let target = ContextTarget::new(&cwd, everything());
    if let Some(question) = options
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        return index
            .question(&target, question)
            .map(|result| result.markdown)
            .map_err(|error| error.to_string());
    }
    let tier = match options.tier.as_deref() {
        None => None,
        Some("full") => Some(RenderTier::Full),
        Some("skeleton") => Some(RenderTier::Skeleton),
        Some("index") => Some(RenderTier::Index),
        Some(other) => {
            return Err(format!(
                "tier must be full, skeleton, or index; got {other}"
            ));
        }
    };
    index
        .list(
            &target,
            &QueryOptions {
                paths: options.path.clone(),
                symbols: options.symbol.clone(),
                tier,
                depth: options.depth.map(|depth| depth as usize),
            },
        )
        .map(|result| result.markdown)
        .map_err(|error| error.to_string())
}

pub fn query(state: &HttpState, cwd: &str, question: &str) -> Result<String, String> {
    let options = MapOptions {
        q: Some(question.to_owned()),
        ..MapOptions::default()
    };
    lookup(state, cwd, &options)
}

fn canonical_directory(cwd: &str) -> Result<String, String> {
    let path = Path::new(cwd);
    if !path.is_absolute() || !path.is_dir() {
        return Err(format!(
            "cannot index {cwd}: choose a directory, then run 'oga query --init'"
        ));
    }
    Ok(oga_config::canonical_cwd(cwd).display().to_string())
}

/// Reconcile the map for `cwd` with disk, building it on first use.
fn refreshed<'a>(state: &'a HttpState, cwd: &str) -> Result<ContextIndex<'a>, String> {
    let index = ContextIndex::new(&state.store);
    let reconciled = index
        .reconcile(cwd, BuildOptions::default())
        .map_err(|error| error.to_string())?;
    if reconciled.file_count == 0 {
        return Err(format!(
            "no indexable files found in {cwd}; add source files, then run 'oga query --init'"
        ));
    }
    Ok(index)
}

fn everything() -> TaskScope {
    TaskScope {
        read: vec!["**".into()],
        write: Vec::new(),
    }
}
