//! Axum routes for broker state, settings, and task actions.

pub mod agents;
pub mod consumers;
pub mod context;
pub mod health;
pub mod hooks;
pub mod profiles;
pub mod router;
pub mod routing;
pub mod settings;
pub mod sse;
pub mod state;
pub mod tasks;
pub mod usage;

pub use router::{AppState, HttpError, HttpState, build_router, router};
