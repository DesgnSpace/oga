//! Connection ownership for writable and read-only stores.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use rusqlite::{Connection, OpenFlags};

use crate::schema::{LATEST_SCHEMA_VERSION, create_fresh_schema, migration_after};

pub const BUSY_TIMEOUT_MS: u64 = 5000;

#[derive(Debug)]
pub enum StoreError {
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

pub struct Store {
    path: PathBuf,
    observe: bool,
    connection: Mutex<Connection>,
    readers: ReaderPool,
    maintenance: RwLock<()>,
    viewed_task: Mutex<Option<String>>,
}

impl Store {
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
        while let Some(migration) = migration_after(version) {
            (migration.run)(connection)?;
            version = migration.version;
        }
        Self::require_current_schema(connection, path)
    }

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

    pub fn is_observe(&self) -> bool {
        self.observe
    }

    pub fn with_connection<T>(
        &self,
        work: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let connection = self.readers.acquire()?;
        work(&connection)
    }

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

    pub async fn write<T, F>(self: &Arc<Self>, work: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError> + Send + 'static,
    {
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || store.transaction(work))
            .await
            .map_err(|error| StoreError::Refusal(format!("write stopped: {error}")))?
    }

    pub fn checkpoint(&self) -> Result<(), StoreError> {
        if self.observe {
            return Ok(());
        }
        let connection = self.lock()?;
        connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE)")?;
        Ok(())
    }

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

pub fn configure_writable(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(concat!(
        "PRAGMA busy_timeout = 5000;\n",
        "PRAGMA synchronous = NORMAL;\n",
        "PRAGMA journal_size_limit = 67108864;\n",
        "PRAGMA foreign_keys = ON;",
    ))?;
    let _mode: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    Ok(())
}

pub fn configure_read_only(connection: &Connection) -> Result<(), StoreError> {
    Ok(connection.execute_batch(concat!(
        "PRAGMA busy_timeout = 5000;\n",
        "PRAGMA foreign_keys = ON;",
    ))?)
}

fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.execute_batch("PRAGMA query_only = ON")?;
    configure_read_only(&connection)?;
    Ok(connection)
}

// Idle reader cap; excess connections close after use.
const MAX_IDLE_READERS: usize = 8;

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
