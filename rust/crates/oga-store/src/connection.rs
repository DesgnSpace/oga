//! Connection ownership: how the broker opens its one writable handle and
//! how everyone else opens a read-only observe handle.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, RwLock};

use rusqlite::{Connection, OpenFlags};

use crate::schema::{
    LATEST_SCHEMA_VERSION, create_fresh_schema, migrate_v37_to_v38, migrate_v38_to_v39,
    migrate_v39_to_v40, migrate_v40_to_v41, migrate_v41_to_v42,
};

pub const BUSY_TIMEOUT_MS: u64 = 5000;

#[derive(Debug)]
pub enum StoreError {
    /// A refusal raised before touching the database.
    Refusal(String),
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Refusal(message) => write!(formatter, "{message}"),
            StoreError::Sqlite(error) => write!(formatter, "sqlite error: {error}"),
            StoreError::Io(error) => write!(formatter, "io error: {error}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Refusal(_) => None,
            StoreError::Sqlite(error) => Some(error),
            StoreError::Io(error) => Some(error),
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        StoreError::Sqlite(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        StoreError::Io(error)
    }
}

/// The store handle. The writable variant owns the database the way the
/// broker does: created if missing, configured for writing, migrated to the
/// current schema. The observe variant is a pure reader that refuses to
/// touch the file.
pub struct Store {
    path: PathBuf,
    observe: bool,
    connection: Mutex<Connection>,
    readers: ReaderPool,
    maintenance: RwLock<()>,
    viewed_task: Mutex<Option<String>>,
}

impl Store {
    /// Open (creating if needed) the broker's database. Creates parent
    /// directories with owner-only permissions, configures WAL, brings a new
    /// database to the current schema version.
    pub fn open_writable(path: impl Into<PathBuf>) -> Result<Store, StoreError> {
        let path = path.into();
        if let Some(directory) = path.parent() {
            fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(directory)?;
        }
        let fresh = !path.exists() || fs::metadata(&path)?.len() == 0;
        let connection = Connection::open(&path)?;
        // Best effort on a file this process just created; an odd umask must
        // not fail the whole open over permissions tightening.
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        configure_writable(&connection)?;
        if fresh {
            create_fresh_schema(&connection)?;
        } else {
            Self::migrate_schema(&connection, &path)?;
        }
        Ok(Store {
            readers: ReaderPool::new(path.clone()),
            path,
            observe: false,
            connection: Mutex::new(connection),
            maintenance: RwLock::new(()),
            viewed_task: Mutex::new(None),
        })
    }

    /// Open an existing database for maintenance writes without seeding
    /// profiles, recovering interrupted tasks, or recording a broker session.
    pub fn open_maintenance(path: impl Into<PathBuf>) -> Result<Store, StoreError> {
        let path = path.into();
        if !path.exists() {
            return Err(StoreError::Refusal(format!(
                "no database at {}; run the broker once to create it, or fix OGA_DB",
                path.display()
            )));
        }
        let connection = Connection::open(&path)?;
        configure_writable(&connection)?;
        Self::migrate_schema(&connection, &path)?;
        Ok(Store {
            readers: ReaderPool::new(path.clone()),
            path,
            observe: false,
            connection: Mutex::new(connection),
            maintenance: RwLock::new(()),
            viewed_task: Mutex::new(None),
        })
    }

    fn require_current_schema(connection: &Connection, path: &Path) -> Result<(), StoreError> {
        let version: Option<i64> = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .map_err(|_| {
                StoreError::Refusal(format!(
                    "cannot use {}: not an oga database",
                    path.display()
                ))
            })?;
        match version {
            Some(version) if version == LATEST_SCHEMA_VERSION => Ok(()),
            Some(version) => Err(StoreError::Refusal(format!(
                "cannot use {}: database schema v{version} is not supported (v{LATEST_SCHEMA_VERSION} required)",
                path.display()
            ))),
            None => Err(StoreError::Refusal(format!(
                "cannot use {}: database schema is empty",
                path.display()
            ))),
        }
    }

    fn migrate_schema(connection: &Connection, path: &Path) -> Result<(), StoreError> {
        let mut version: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .map_err(|_| {
                StoreError::Refusal(format!(
                    "cannot use {}: not an oga database",
                    path.display()
                ))
            })?;
        while version < LATEST_SCHEMA_VERSION {
            match version {
                37 => migrate_v37_to_v38(connection)?,
                38 => migrate_v38_to_v39(connection)?,
                39 => migrate_v39_to_v40(connection)?,
                40 => migrate_v40_to_v41(connection)?,
                41 => migrate_v41_to_v42(connection)?,
                _ => break,
            }
            version += 1;
        }
        Self::require_current_schema(connection, path)
    }

    /// Open the database as a pure reader: no create, no migration, no write
    /// of any kind. A watcher must be unable to change the file it is
    /// watching, even by accident.
    ///
    /// `SQLITE_OPEN_READ_ONLY` cannot create or open-for-writing the file at
    /// all, and it reads WAL databases natively; `query_only` is defense in
    /// depth on top, so any write attempt fails instead of mutating. The
    /// ledger check keeps an old binary honest in both directions: a schema
    /// ahead of [`LATEST_SCHEMA_VERSION`] was migrated by a newer broker,
    /// and one behind it needs a migration only a writable broker run can
    /// apply, so exactly the built-for version is observable.
    pub fn open_observe(path: impl Into<PathBuf>) -> Result<Store, StoreError> {
        let path = path.into();
        if !path.exists() {
            return Err(StoreError::Refusal(format!(
                "cannot observe {}: no database at this path; run the broker once to create it, or fix OGA_DB",
                path.display()
            )));
        }
        let connection = open_reader(&path)?;

        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
        )?;
        let ledger_exists = statement.query([])?.next()?.is_some();
        drop(statement);
        if !ledger_exists {
            return Err(StoreError::Refusal(format!(
                "cannot observe {}: not an oga database (no schema_migrations table)",
                path.display()
            )));
        }
        let applied: i64 = connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        if applied > LATEST_SCHEMA_VERSION {
            return Err(StoreError::Refusal(format!(
                "cannot observe {}: database schema v{applied} is newer than this binary knows \
                 (v{LATEST_SCHEMA_VERSION}); run `make install` to rebuild it",
                path.display()
            )));
        }
        if applied < LATEST_SCHEMA_VERSION {
            return Err(StoreError::Refusal(format!(
                "cannot observe {}: database schema v{applied} predates this binary \
                  (v{LATEST_SCHEMA_VERSION}); run a compatible broker",
                path.display()
            )));
        }
        Ok(Store {
            readers: ReaderPool::new(path.clone()),
            path,
            observe: true,
            connection: Mutex::new(connection),
            maintenance: RwLock::new(()),
            viewed_task: Mutex::new(None),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True when this handle must never write, schema work included.
    pub fn is_observe(&self) -> bool {
        self.observe
    }

    /// Run a read against its own connection from the reader pool, never the
    /// single writer lock. WAL lets any number of readers proceed alongside
    /// the one writer and each other, so a slow read no longer holds up
    /// every other caller sharing this `Store`.
    pub fn with_connection<T>(
        &self,
        work: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let connection = self.readers.acquire()?;
        work(&connection)
    }

    /// Run a group of writes atomically while retaining the store's single
    /// writer lock for the whole transaction.
    pub fn with_transaction<T>(
        &self,
        work: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        if self.observe {
            return Err(StoreError::Refusal(
                "observe store cannot start a write transaction".to_string(),
            ));
        }
        let _maintenance = self
            .maintenance
            .read()
            .map_err(|_| StoreError::Refusal("maintenance lock poisoned".to_string()))?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        match work(&transaction) {
            Ok(value) => {
                transaction.commit()?;
                Ok(value)
            }
            Err(error) => {
                let _ = transaction.rollback();
                Err(error)
            }
        }
    }

    /// Close the handle. Only a writable handle pays for `PRAGMA optimize`:
    /// it can run ANALYZE, which writes.
    pub fn close(self) -> Result<(), StoreError> {
        let connection = self
            .connection
            .into_inner()
            .map_err(|_| StoreError::Refusal("store connection lock poisoned".to_string()))?;
        if !self.observe {
            connection.execute_batch("PRAGMA optimize")?;
        }
        Ok(connection.close().map_err(|(_, error)| error)?)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.connection
            .lock()
            .map_err(|_| StoreError::Refusal("store connection lock poisoned".to_string()))
    }

    /// Run an exclusive maintenance operation while all broker writes wait.
    /// The same connection is kept open for the rewrite and checkpoint.
    pub fn with_maintenance<T>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        if self.observe {
            return Err(StoreError::Refusal(
                "observe store cannot run maintenance".to_string(),
            ));
        }
        let _maintenance = self
            .maintenance
            .write()
            .map_err(|_| StoreError::Refusal("maintenance lock poisoned".to_string()))?;
        let mut connection = self.lock()?;
        work(&mut connection)
    }

    pub fn set_viewed_task(&self, task_id: impl Into<String>) {
        if let Ok(mut viewed) = self.viewed_task.lock() {
            *viewed = Some(task_id.into());
        }
    }

    pub fn clear_viewed_task(&self, task_id: &str) {
        if let Ok(mut viewed) = self.viewed_task.lock()
            && viewed.as_deref() == Some(task_id)
        {
            *viewed = None;
        }
    }

    pub(crate) fn viewed_task(&self) -> Option<String> {
        self.viewed_task
            .lock()
            .ok()
            .and_then(|viewed| viewed.clone())
    }
}

/// The writable profile: busy timeout, WAL journaling, normal synchronous
/// mode, foreign keys enforced. Order matters only for readability; WAL is
/// set through its own query because the pragma reports the applied mode.
pub fn configure_writable(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(concat!(
        "PRAGMA busy_timeout = 5000;\n",
        "PRAGMA synchronous = NORMAL;\n",
        "PRAGMA foreign_keys = ON;",
    ))?;
    let _mode: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    Ok(())
}

/// The subset of [`configure_writable`] safe on a read-only handle. WAL is
/// deliberately absent: switching journal modes needs a writable connection,
/// and the broker that owns the file has already set it. Busy timeout and
/// foreign keys are per-connection settings and never write to the file.
pub fn configure_read_only(connection: &Connection) -> Result<(), StoreError> {
    Ok(connection.execute_batch(concat!(
        "PRAGMA busy_timeout = 5000;\n",
        "PRAGMA foreign_keys = ON;",
    ))?)
}

/// Open one reader: read-only at the OS level, `query_only` on top as
/// defense in depth, native to a WAL file so it never contends with the
/// writer or with another reader.
fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.execute_batch("PRAGMA query_only = ON")?;
    configure_read_only(&connection)?;
    Ok(connection)
}

/// A cap on idle connections, not on concurrency: a burst past this many
/// simultaneous reads just opens extra handles and lets them go afterward,
/// so no reader ever queues behind another.
const MAX_IDLE_READERS: usize = 8;

/// Every read pulls its own connection from here instead of sharing the
/// writer's lock. WAL mode is built for exactly this: many readers and one
/// writer, none of them blocking each other.
struct ReaderPool {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
}

impl ReaderPool {
    fn new(path: PathBuf) -> Self {
        ReaderPool {
            path,
            idle: Mutex::new(Vec::new()),
        }
    }

    fn acquire(&self) -> Result<PooledReader<'_>, StoreError> {
        let cached = self
            .idle
            .lock()
            .map_err(|_| StoreError::Refusal("reader pool lock poisoned".to_string()))?
            .pop();
        let connection = match cached {
            Some(connection) => connection,
            None => open_reader(&self.path)?,
        };
        Ok(PooledReader {
            pool: self,
            connection: Some(connection),
        })
    }
}

struct PooledReader<'a> {
    pool: &'a ReaderPool,
    connection: Option<Connection>,
}

impl std::ops::Deref for PooledReader<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.connection
            .as_ref()
            .expect("connection present until returned to the pool")
    }
}

impl Drop for PooledReader<'_> {
    fn drop(&mut self) {
        let Some(connection) = self.connection.take() else {
            return;
        };
        if let Ok(mut idle) = self.pool.idle.lock()
            && idle.len() < MAX_IDLE_READERS
        {
            idle.push(connection);
        }
    }
}
