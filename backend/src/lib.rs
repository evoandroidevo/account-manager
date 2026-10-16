//! Server-only building blocks for the account manager: configuration,
//! persistence, and the LLDAP client. Never compiled into the wasm bundle.

pub mod config;
pub mod lldap;
pub mod repo;
pub mod session;
pub mod state;
pub mod throttle;
pub mod validation;

pub use state::AppState;
