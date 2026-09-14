//! Plain-language code lookups for the MCP surface, answered from the shared
//! index after it is reconciled against disk.

use oga_context::QuestionOptions;
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
    let target =
        oga_http::context::target_for(&state.store, &cwd).map_err(|error| error.to_string())?;
    let options = QuestionOptions {
        paths: paths.to_vec(),
        limit,
        code,
    };
    oga_http::context::answer(
        &state.reconcile_debounce,
        &state.store,
        &target,
        question,
        options,
    )
    .map_err(|error| error.to_string())?
    .map(|result| result.markdown)
    .ok_or_else(|| {
        format!("no indexable files found in {cwd}; add source files, then run 'oga query --init'")
    })
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
