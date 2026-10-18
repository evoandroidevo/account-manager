//! Server-only building blocks for the account manager: configuration,
//! persistence, the LLDAP client and service clients. Never compiled into the
//! wasm bundle.

pub mod accounts;
pub mod config;
pub mod lldap;
pub mod mailer;
pub mod repo;
pub mod services;
pub mod session;
pub mod state;
pub mod throttle;
pub mod validation;

pub use state::AppState;
