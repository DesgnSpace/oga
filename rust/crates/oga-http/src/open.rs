//! Handing a file to the app the system opens that kind of file with.

use std::{path::Path, process::Command, sync::Arc};

pub type Opener = Arc<dyn Fn(&Path) -> std::io::Result<()> + Send + Sync>;

pub fn system_opener() -> Opener {
    Arc::new(open_in_default_app)
}

/// The app registered for this kind of file, which on a Mac is the editor the
/// reader set by default.
#[cfg(target_os = "macos")]
fn open_in_default_app(path: &Path) -> std::io::Result<()> {
    Command::new("open").arg(path).spawn().map(|_| ())
}

#[cfg(target_os = "linux")]
fn open_in_default_app(path: &Path) -> std::io::Result<()> {
    Command::new("xdg-open").arg(path).spawn().map(|_| ())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn open_in_default_app(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this system has no opener Oga knows how to ask",
    ))
}
