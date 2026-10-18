//! Server-only building blocks for the account manager: configuration,
//! persistence, the LLDAP client, service provisioning and the sync worker.
//! Never compiled into the wasm bundle.

pub mod accounts;
pub mod config;
pub mod lldap;
pub mod mailer;
pub mod repo;
pub mod services;
pub mod session;
pub mod state;
pub mod throttle;
pub mod tiers;
pub mod validation;
pub mod worker;

pub use state::AppState;
