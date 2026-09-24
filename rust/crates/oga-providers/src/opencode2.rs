//! Which executable an `opencode-2` profile runs. OpenCode 1 and 2 both install
//! as `opencode`, so that name is trusted only once it reports version 2.

use oga_domain::Profile;

use crate::{
    environment_for,
    opencode::{explicit, find_on_path, major, reported_version},
    worker_path::worker_path,
};

/// The profile environment key naming the OpenCode 2 executable outright.
pub const OPENCODE2_BIN: &str = "OPENCODE2_BIN";

/// The OpenCode 2 executable for a profile: the path its `OPENCODE2_BIN`
/// names, else `opencode2` on the path the worker is started with, else an
/// `opencode` there that reports version 2. With none of them the bare name
/// `opencode2` is returned, so a failed start names what is missing.
pub fn opencode2_executable(profile: &Profile) -> String {
    let env = environment_for(profile);
    if let Some(explicit) = explicit(&env, OPENCODE2_BIN) {
        return explicit;
    }
    let path = env.get("PATH").cloned().unwrap_or_else(worker_path);
    if let Some(found) = find_on_path("opencode2", &path) {
        return found.display().to_string();
    }
    find_on_path("opencode", &path)
        .filter(|candidate| reported_version(candidate).as_deref().and_then(major) == Some(2))
        .map_or_else(
            || "opencode2".to_owned(),
            |found| found.display().to_string(),
        )
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::Path};

    use oga_domain::Provider;

    use super::*;
    use crate::opencode::tests::script;

    fn profile(env: BTreeMap<String, String>) -> Profile {
        Profile {
            id: "oc2".into(),
            label: "OpenCode 2".into(),
            provider: Provider::OpenCode2,
            default_model: "opencode/space-bunny-free".into(),
            enabled: true,
            env,
            capabilities: vec![],
            command: None,
        }
    }

    fn on_path(directory: &Path) -> BTreeMap<String, String> {
        BTreeMap::from([("PATH".into(), directory.display().to_string())])
    }

    #[test]
    fn a_configured_path_wins_over_anything_on_the_path() {
        let bin = tempfile::tempdir().expect("bin");
        script(bin.path(), "opencode2", "opencode v2.0.1");
        let mut env = on_path(bin.path());
        env.insert(OPENCODE2_BIN.into(), "~/tools/opencode-next".into());

        assert_eq!(
            opencode2_executable(&profile(env)),
            format!("{}/tools/opencode-next", crate::home())
        );
    }

    #[test]
    fn opencode2_is_preferred_over_an_opencode_that_reports_v2() {
        let bin = tempfile::tempdir().expect("bin");
        let named = script(bin.path(), "opencode2", "opencode v2.0.1");
        script(bin.path(), "opencode", "opencode v2.0.1");

        assert_eq!(
            opencode2_executable(&profile(on_path(bin.path()))),
            named.display().to_string()
        );
    }

    #[test]
    fn opencode_is_used_only_when_it_reports_v2() {
        let v2 = tempfile::tempdir().expect("bin");
        let opencode = script(v2.path(), "opencode", "opencode v2.0.1");
        assert_eq!(
            opencode2_executable(&profile(on_path(v2.path()))),
            opencode.display().to_string()
        );

        let v1 = tempfile::tempdir().expect("bin");
        script(v1.path(), "opencode", "1.18.32");
        assert_eq!(
            opencode2_executable(&profile(on_path(v1.path()))),
            "opencode2",
            "an OpenCode 1 under the shared name is never run as OpenCode 2"
        );
    }
}
