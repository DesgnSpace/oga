//! Plain-language code lookups for the MCP surface, answered from the shared
//! index after it is reconciled against disk.

use oga_context::{BuildOptions, ContextIndex, ContextTarget};
use oga_domain::TaskScope;
use oga_http::HttpState;
use std::path::Path;

pub fn query(state: &HttpState, cwd: &str, question: &str) -> Result<String, String> {
    let cwd = canonical_directory(cwd)?;
    let index = refreshed(state, &cwd)?;
    index
        .question(&ContextTarget::new(&cwd, everything()), question)
        .map(|result| result.markdown)
        .map_err(|error| error.to_string())
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

/// Reconcile the index for `cwd` with disk, building it on first use.
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
