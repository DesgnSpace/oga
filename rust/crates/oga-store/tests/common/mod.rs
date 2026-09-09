//! Shared store-test fixtures: throwaway databases in their own directories.

use std::path::PathBuf;

use oga_store::Store;

/// A unique directory per call, removed when the guard drops. The database
/// file lives at `dir.join("oga.db")`, matching the real layout.
pub struct TestDatabase {
    pub dir: tempfile::TempDir,
}

#[allow(dead_code)]
impl TestDatabase {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temporary directory is creatable"),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.path().join("oga.db")
    }

    /// A writable store over this fixture's database.
    pub fn open_writable(&self) -> Store {
        Store::open_writable(self.path()).expect("writable store opens")
    }

    /// An observe store over a database a previous step created.
    pub fn open_observe(&self) -> Store {
        Store::open_observe(self.path()).expect("observe store opens")
    }
}

impl Default for TestDatabase {
    fn default() -> Self {
        Self::new()
    }
}
