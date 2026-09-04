//! Atomic write helpers shared by store repositories.

use crate::connection::{Store, StoreError};

pub type TransactionError = StoreError;

impl Store {
    /// Execute repository writes as one SQLite transaction.
    pub fn transaction<T>(
        &self,
        work: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with_transaction(work)
    }
}
