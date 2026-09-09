use std::{
    env, fs,
    path::{Path, PathBuf},
};

use super::{
    Capability, ConfinementBackend, ConfinementError, ConfinementMode, ConfinementRequest,
    PreparedCommand, ScopeRule, ancestors, is_executable, unavailable, validate_request,
};

const BWRAP: &str = "bwrap";

/// Linux Bubblewrap adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxBubblewrap {
    executable: Option<PathBuf>,
}

impl LinuxBubblewrap {
    pub fn with_executable(path: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(path.into()),
        }
    }

    pub fn executable(&self) -> Option<&Path> {
        self.executable.as_deref()
    }

    fn find_executable(&self) -> Option<PathBuf> {
        if let Some(path) = &self.executable {
            return (path.is_file() && is_executable(path))
                .then(|| fs::canonicalize(path).ok())
                .flatten();
        }
        let path = env::var_os("PATH").unwrap_or_default();
        env::split_paths(&path)
            .map(|directory| directory.join(BWRAP))
            .chain([PathBuf::from("/usr/bin/bwrap"), PathBuf::from("/bin/bwrap")])
            .find(|path| path.is_file() && is_executable(path))
            .and_then(|path| fs::canonicalize(path).ok())
    }

    fn command_line(
        &self,
        executable: PathBuf,
        request: &ConfinementRequest,
    ) -> Result<PreparedCommand, ConfinementError> {
        let request = validate_request(request)?;
        for rule in &request.write {
            if !rule.exists {
                return Err(ConfinementError::UnsupportedPath {
                    backend: ConfinementMode::LinuxBubblewrap,
                    path: rule.path.clone(),
                    reason: "Bubblewrap can bind only paths that already exist".into(),
                });
            }
        }
        let mut argv = vec![
            executable.to_string_lossy().into_owned(),
            "--die-with-parent".into(),
            "--new-session".into(),
            "--unshare-user".into(),
            "--unshare-pid".into(),
            "--unshare-uts".into(),
            "--unshare-ipc".into(),
            "--tmpfs".into(),
            "/".into(),
            "--tmpfs".into(),
            "/tmp".into(),
            "--proc".into(),
            "/proc".into(),
            "--dev".into(),
            "/dev".into(),
        ];
        let runtime_roots = runtime_roots();
        let mut directories = Vec::new();
        add_directory_chain(&mut directories, &request.cwd, true);
        for root in &runtime_roots {
            add_directory_chain(&mut directories, root, true);
        }
        if let Some(provider) = &request.executable
            && !runtime_roots.iter().any(|root| provider.starts_with(root))
        {
            add_directory_chain(&mut directories, provider, false);
        }
        for rule in request.read.iter().chain(&request.write) {
            if rule.exists {
                add_directory_chain(&mut directories, &rule.path, rule.recursive);
            }
        }
        directories.sort_by_key(|path| (path.components().count(), path.clone()));
        directories.dedup();
        for directory in directories {
            if directory != Path::new("/tmp")
                && directory != Path::new("/proc")
                && directory != Path::new("/dev")
                && directory != Path::new("/")
            {
                argv.extend(["--dir".into(), directory.to_string_lossy().into_owned()]);
            }
        }
        for root in &runtime_roots {
            argv.extend([
                "--ro-bind".into(),
                root.to_string_lossy().into_owned(),
                root.to_string_lossy().into_owned(),
            ]);
        }
        if let Some(provider) = &request.executable
            && !runtime_roots.iter().any(|root| provider.starts_with(root))
        {
            argv.extend([
                "--ro-bind".into(),
                provider.to_string_lossy().into_owned(),
                provider.to_string_lossy().into_owned(),
            ]);
        }
        for rule in &request.read {
            add_bind(&mut argv, rule, "--ro-bind");
        }
        for rule in &request.write {
            add_bind(&mut argv, rule, "--bind");
        }
        let mut command = request.argv;
        if let Some(provider) = request.executable {
            command[0] = provider.to_string_lossy().into_owned();
        }
        argv.extend(["--chdir".into(), request.cwd.to_string_lossy().into_owned()]);
        argv.push("--".into());
        argv.extend(command);
        Ok(PreparedCommand { argv })
    }
}

impl ConfinementBackend for LinuxBubblewrap {
    fn kind(&self) -> ConfinementMode {
        ConfinementMode::LinuxBubblewrap
    }

    fn probe(&self) -> Capability {
        if !cfg!(target_os = "linux") {
            return Capability::Unavailable {
                reason: "Bubblewrap namespaces are only available on Linux".into(),
            };
        }
        match self.find_executable() {
            Some(executable) => Capability::Available {
                executable: Some(executable),
            },
            None => Capability::Unavailable {
                reason: "bwrap was not found on PATH or in /usr/bin or /bin".into(),
            },
        }
    }

    fn prepare(&self, request: &ConfinementRequest) -> Result<PreparedCommand, ConfinementError> {
        let capability = self.probe();
        let Some(executable) = capability.executable().map(Path::to_path_buf) else {
            return Err(unavailable(self.kind(), capability));
        };
        self.command_line(executable, request)
    }
}

fn add_bind(argv: &mut Vec<String>, rule: &ScopeRule, operation: &str) {
    if !rule.exists {
        return;
    }
    let path = rule.path.to_string_lossy().into_owned();
    argv.extend([operation.into(), path.clone(), path]);
}

fn add_directory_chain(directories: &mut Vec<PathBuf>, path: &Path, include_path: bool) {
    let target = if include_path {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    directories.extend(ancestors(target));
}

fn runtime_roots() -> Vec<PathBuf> {
    ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc"]
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| fs::metadata(path).is_ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use oga_domain::TaskScope;
    use tempfile::TempDir;

    use super::*;

    fn request(temp: &TempDir) -> ConfinementRequest {
        ConfinementRequest::new(
            vec!["/bin/true".into(), "literal;argument".into()],
            temp.path(),
            TaskScope {
                read: vec!["input".into()],
                write: vec!["output/**".into()],
            },
        )
    }

    #[test]
    fn bubblewrap_confinement_keeps_scope_bindings_and_arguments_separate() {
        let temp = TempDir::new().expect("temporary directory");
        fs::create_dir(temp.path().join("input")).expect("input directory");
        fs::create_dir(temp.path().join("output")).expect("output directory");
        let prepared = LinuxBubblewrap::default()
            .command_line(PathBuf::from("/bin/bwrap"), &request(&temp))
            .expect("bubblewrap command");

        assert!(
            prepared
                .argv
                .windows(2)
                .any(|pair| pair[0] == "--unshare-pid" && pair[1] == "--unshare-uts")
        );
        let input = fs::canonicalize(temp.path().join("input"))
            .expect("canonical input directory")
            .to_string_lossy()
            .into_owned();
        assert!(prepared.argv.windows(3).any(|triple| {
            triple[0] == "--ro-bind" && triple[1] == input && triple[2] == input
        }));
        let output = fs::canonicalize(temp.path().join("output"))
            .expect("canonical output directory")
            .to_string_lossy()
            .into_owned();
        assert!(
            prepared.argv.windows(3).any(|triple| {
                triple[0] == "--bind" && triple[1] == output && triple[2] == output
            })
        );
        let separator = prepared
            .argv
            .iter()
            .position(|argument| argument == "--")
            .expect("provider separator");
        assert_eq!(
            &prepared.argv[separator + 1..],
            ["/bin/true", "literal;argument"]
        );
    }

    #[test]
    fn bubblewrap_confinement_rejects_missing_write_bindings() {
        let temp = TempDir::new().expect("temporary directory");
        let result = LinuxBubblewrap::default().command_line(
            PathBuf::from("/bin/bwrap"),
            &ConfinementRequest::new(
                vec!["/bin/true".into()],
                temp.path(),
                TaskScope {
                    read: vec![],
                    write: vec!["new-output/**".into()],
                },
            ),
        );

        assert!(matches!(
            result,
            Err(ConfinementError::UnsupportedPath { .. })
        ));
    }
}
