//! Which executable a `cursor` profile runs. Cursor's CLI installs as `agent`,
//! a name other tools install too, so it is trusted only once it prints one of
//! Cursor's own builds, such as `2026.05.20-2b5dd59`.

use std::{fmt, path::PathBuf};

use oga_domain::Profile;

use crate::{
    environment_for,
    opencode::{all_named, printed_version, search_directories},
};

/// The command Cursor's CLI installs as.
pub const CURSOR_BIN: &str = "agent";

/// How a user puts Cursor's CLI back, or on for the first time.
const INSTALL_COMMAND: &str = "curl https://cursor.com/install -fsS | bash";

/// No `agent` this profile can run is Cursor's CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoCursorCli {
    /// There is no `agent` to run.
    Missing,
    /// Each `agent` found, with what it printed for `--version`.
    Other(Vec<(PathBuf, Option<String>)>),
}

impl fmt::Display for NoCursorCli {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "Cursor's CLI isn't installed. Install it with `{INSTALL_COMMAND}`, then sign in with `agent login`."
            ),
            Self::Other(found) => {
                let found = found
                    .iter()
                    .map(|(path, printed)| match printed {
                        Some(printed) if printed.starts_with("grok") => {
                            format!("{} is Grok ({printed})", path.display())
                        }
                        Some(printed) => format!("{} prints \"{printed}\"", path.display()),
                        None => format!("{} did not report a version", path.display()),
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                write!(
                    f,
                    "The `agent` command on this machine isn't Cursor's CLI: {found}. \
                     Reinstall Cursor's CLI with `{INSTALL_COMMAND}` so `agent` is Cursor's again."
                )
            }
        }
    }
}

impl std::error::Error for NoCursorCli {}

/// The first `agent` on the path the worker is started with, and then in the
/// usual install directories, that prints one of Cursor's builds.
pub fn cursor_executable(profile: &Profile) -> Result<String, NoCursorCli> {
    let directories = search_directories(&environment_for(profile));
    let found = all_named(CURSOR_BIN, &directories);
    if found.is_empty() {
        return Err(NoCursorCli::Missing);
    }
    let mut reported = Vec::with_capacity(found.len());
    for candidate in found {
        let printed = printed_version(&candidate);
        if printed.as_deref().is_some_and(is_cursor_build) {
            return Ok(candidate.display().to_string());
        }
        reported.push((candidate, printed));
    }
    Err(NoCursorCli::Other(reported))
}

/// Cursor prints a dated build, `2026.05.20-2b5dd59`, alone on its line.
fn is_cursor_build(printed: &str) -> bool {
    let mut parts = printed.trim().splitn(2, '-');
    let (Some(date), Some(hash)) = (parts.next(), parts.next()) else {
        return false;
    };
    let mut fields = date.split('.');
    let numbers = [4, 2, 2].map(|width| {
        fields
            .next()
            .is_some_and(|field| field.len() == width && field.bytes().all(|b| b.is_ascii_digit()))
    });
    numbers.iter().all(|ok| *ok)
        && fields.next().is_none()
        && !hash.is_empty()
        && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, env, path::Path};

    use oga_domain::Provider;

    use super::*;
    use crate::opencode::tests::script;

    fn profile(directories: &[&Path]) -> Profile {
        let path = env::join_paths(directories).expect("path");
        Profile {
            id: "cursor".into(),
            label: "Cursor".into(),
            provider: Provider::Cursor,
            default_model: "auto".into(),
            enabled: true,
            env: BTreeMap::from([("PATH".into(), path.to_string_lossy().into_owned())]),
            capabilities: vec![],
            command: None,
        }
    }

    #[test]
    fn grok_installed_as_agent_is_never_run_as_cursor() {
        let bin = tempfile::tempdir().expect("bin");
        let grok = script(bin.path(), "agent", "grok 1.0.62 (1474f9cbdd91) [stable]");

        let refused = cursor_executable(&profile(&[bin.path()])).expect_err("Grok is not Cursor");

        assert_eq!(
            refused,
            NoCursorCli::Other(vec![(
                grok.clone(),
                Some("grok 1.0.62 (1474f9cbdd91) [stable]".to_owned())
            )])
        );
        let message = refused.to_string();
        assert!(message.contains("is Grok"), "{message}");
        assert!(
            message.contains("curl https://cursor.com/install"),
            "{message}"
        );
        assert_eq!(
            crate::cannot_start(&profile(&[bin.path()])).as_deref(),
            Some(message.as_str()),
            "setup and health say the same thing the run would"
        );
    }

    #[test]
    fn a_later_cursor_agent_is_run_when_the_first_agent_is_grok() {
        let first = tempfile::tempdir().expect("bin");
        let later = tempfile::tempdir().expect("bin");
        script(first.path(), "agent", "grok 1.0.62 (1474f9cbdd91) [stable]");
        let cursor = script(later.path(), "agent", "2026.05.20-2b5dd59");

        assert_eq!(
            cursor_executable(&profile(&[first.path(), later.path()])),
            Ok(cursor.display().to_string())
        );
    }

    #[test]
    fn no_agent_at_all_says_cursor_is_not_installed() {
        let empty = tempfile::tempdir().expect("bin");

        assert_eq!(
            cursor_executable(&profile(&[empty.path()])),
            Err(NoCursorCli::Missing)
        );
    }
}
