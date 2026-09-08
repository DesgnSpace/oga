//! SQLite persistence for the Rust broker: one writable handle, strict
//! observe mode and the current database schema.

pub mod connection;
pub mod maintenance;
pub mod repo;
pub mod schema;
pub mod writer;

pub use connection::{BUSY_TIMEOUT_MS, Store, StoreError, configure_read_only, configure_writable};
pub use repo::{TaskTiming, attach_task_timing, task_timing};
pub use schema::{LATEST_SCHEMA_VERSION, create_fresh_schema};
pub use writer::TransactionError;
