//! Plain-language code lookups for the MCP surface, answered from the shared
//! index after it is reconciled against disk.

use oga_context::{ContextIndex, ContextTarget, QuestionOptions};
use oga_domain::TaskScope;
use oga_http::HttpState;
use std::path::Path;

pub fn query(
    state: &HttpState,
    cwd: &str,
    question: &str,
    paths: &[String],
    limit: Option<usize>,
    code: bool,
) -> Result<String, String> {
    let cwd = canonical_directory(cwd)?;
    let index = refreshed(state, &cwd)?;
    let options = QuestionOptions {
        paths: paths.to_vec(),
        limit,
        code,
    };
    index
        .question_with_options(&ContextTarget::new(&cwd, everything()), question, options)
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

/// Reconcile the index for `cwd` with disk, building it on first use, unless
/// it was already walked within the debounce window.
fn refreshed<'a>(state: &'a HttpState, cwd: &str) -> Result<ContextIndex<'a>, String> {
    let index = ContextIndex::new(&state.store);
    let reconciled = oga_http::context::reconcile_if_stale(&state.reconcile_debounce, &index, cwd)
        .map_err(|error| error.to_string())?;
    if reconciled.is_some_and(|result| result.file_count == 0) {
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
