//! Clients for the services whose accounts follow a user's tier.

pub mod jellyfin;
pub mod seerr;

use crate::config::AppConfig;

/// A place where a user's account is kept in line with their tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Service {
    /// Tier groups in LLDAP (always on).
    Lldap,
    Jellyfin,
    /// Imported from Jellyfin, so it always runs after it.
    Seerr,
}

impl Service {
    pub const ALL: [Service; 3] = [Service::Lldap, Service::Jellyfin, Service::Seerr];

    pub fn as_str(self) -> &'static str {
        match self {
            Service::Lldap => "lldap",
            Service::Jellyfin => "jellyfin",
            Service::Seerr => "seerr",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|service| service.as_str() == value)
    }

    /// Name shown to users.
    pub fn label(self) -> &'static str {
        match self {
            Service::Lldap => "Groups",
            Service::Jellyfin => "Jellyfin",
            Service::Seerr => "Seerr",
        }
    }
}

/// What a successful sync did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncOutcome {
    /// The account's id in the service, if it has one.
    pub remote_id: Option<String>,
    /// True when the account was deliberately left alone.
    pub skipped: bool,
    pub detail: Option<String>,
}

impl SyncOutcome {
    pub fn done(remote_id: Option<String>) -> Self {
        Self { remote_id, skipped: false, detail: None }
    }

    pub fn skipped(remote_id: Option<String>, reason: &str) -> Self {
        Self { remote_id, skipped: true, detail: Some(reason.to_owned()) }
    }
}

/// Clients for the services that are enabled in the config.
#[derive(Clone, Debug)]
pub struct Services {
    pub jellyfin: Option<jellyfin::JellyfinClient>,
    pub seerr: Option<seerr::SeerrClient>,
}

impl Services {
    pub fn new(config: &AppConfig) -> Self {
        Self {
            jellyfin: config
                .jellyfin
                .enabled
                .then(|| jellyfin::JellyfinClient::new(&config.jellyfin)),
            seerr: config.seerr.enabled.then(|| seerr::SeerrClient::new(&config.seerr)),
        }
    }

    /// Services to keep in sync, in the order they must run.
    pub fn enabled(&self) -> Vec<Service> {
        let mut services = vec![Service::Lldap];
        if self.jellyfin.is_some() {
            services.push(Service::Jellyfin);
        }
        if self.seerr.is_some() {
            services.push(Service::Seerr);
        }
        services
    }
}

/// A reqwest client for service APIs, with a timeout so a hung service
/// can't stall the worker.
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .pool_idle_timeout(std::time::Duration::from_secs(3))
        .build()
        .expect("static reqwest configuration is valid")
}

/// Turns a non-2xx response into an error message with the body attached.
pub(crate) async fn check(
    response: reqwest::Response,
    what: &str,
) -> Result<reqwest::Response, String> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let body: String = body.chars().take(300).collect();
    Err(format!("{what} failed with {status}: {body}"))
}
