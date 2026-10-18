//! Helpers available to server functions only (compiled with the `ssr` feature).

use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use backend::{
    accounts::AccountError,
    session::{SessionUser, SESSION_USER_KEY},
    AppState,
};
use http::request::Parts;
use leptos::prelude::*;
use tower_sessions::Session;

pub const NOT_LOGGED_IN: &str = "Your session has ended. Please log in again.";

/// Everything a server function needs from the request.
///
/// Call [`ctx`] first, before any `.await`: when a server function runs
/// inside a resource during SSR, the reactive context is not reliably
/// available after it has awaited other work, so `use_context` would fail.
#[derive(Clone)]
pub struct Ctx {
    pub state: AppState,
    pub session: Session,
    /// Client IP for rate limiting, honouring `X-Forwarded-For` only when the
    /// config says we sit behind a proxy that overwrites it (Caddy does for
    /// untrusted clients).
    pub client_ip: Option<String>,
}

/// Reads the request context synchronously. See [`Ctx`].
pub fn ctx() -> Result<Ctx, ServerFnError> {
    let state =
        use_context::<AppState>().ok_or_else(|| internal("AppState missing from context"))?;
    let parts =
        use_context::<Parts>().ok_or_else(|| internal("request parts missing from context"))?;
    let session = parts
        .extensions
        .get::<Session>()
        .cloned()
        .ok_or_else(|| internal("session layer missing"))?;

    let forwarded = state
        .config
        .server
        .trust_proxy_headers
        .then(|| {
            parts
                .headers
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .flatten();
    let client_ip = forwarded.or_else(|| {
        parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip().to_string())
    });

    Ok(Ctx {
        state,
        session,
        client_ip,
    })
}

impl Ctx {
    /// The logged-in user, or `None`. Ends sessions that are past their
    /// absolute lifetime, belong to a disabled account, or predate a
    /// password change.
    pub async fn user(&self) -> Result<Option<SessionUser>, ServerFnError> {
        let Some(user) = self
            .session
            .get::<SessionUser>(SESSION_USER_KEY)
            .await
            .map_err(|error| internal(format!("session read failed: {error}")))?
        else {
            return Ok(None);
        };

        let record = self
            .state
            .users
            .find_by_id(&user.user_id)
            .await
            .map_err(|error| internal(format!("user lookup failed: {error}")))?;
        let lifetime = self.state.config.session.absolute_timeout_hours;
        let still_valid = !user.is_expired(lifetime, now_unix())
            && record.is_some_and(|record| {
                record.disabled_at.is_none()
                    && record
                        .password_changed_at
                        .is_none_or(|changed_at| user.authenticated_at >= changed_at)
            });

        if still_valid {
            Ok(Some(user))
        } else {
            let _ = self.session.flush().await;
            Ok(None)
        }
    }

    pub async fn require_user(&self) -> Result<SessionUser, ServerFnError> {
        self.user()
            .await?
            .ok_or_else(|| ServerFnError::new(NOT_LOGGED_IN))
    }

    /// Like `require_user`, but also re-checks admin group membership in
    /// LLDAP, so removing someone from the group takes effect immediately.
    pub async fn require_admin(&self) -> Result<SessionUser, ServerFnError> {
        let user = self.require_user().await?;
        let is_admin = self
            .state
            .lldap
            .get_user(&user.username)
            .await
            .map_err(|error| {
                internal(format!("LLDAP lookup for {} failed: {error}", user.username))
            })?
            .is_some_and(|profile| profile.is_member_of(&self.state.config.auth.admin_group));
        if is_admin {
            Ok(user)
        } else {
            Err(ServerFnError::new("Only administrators can do that."))
        }
    }

    /// Stores a freshly authenticated user under a new session id (so a
    /// planted id can't be reused).
    pub async fn start_session(&self, user: SessionUser) -> Result<(), ServerFnError> {
        self.session
            .cycle_id()
            .await
            .map_err(|error| internal(format!("session cycle failed: {error}")))?;
        self.session
            .insert(SESSION_USER_KEY, user)
            .await
            .map_err(|error| internal(format!("session write failed: {error}")))
    }

    pub async fn end_session(&self) -> Result<(), ServerFnError> {
        self.session
            .flush()
            .await
            .map_err(|error| internal(format!("session flush failed: {error}")))
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

/// Logs the real cause and returns a generic message to the browser.
pub fn internal(detail: impl std::fmt::Display) -> ServerFnError {
    log::error!("{detail}");
    ServerFnError::new("Something went wrong. Please try again.")
}

/// Shows rejections as-is and hides internal failures behind `internal`.
pub fn account_error(error: AccountError) -> ServerFnError {
    match error {
        AccountError::Rejected(message) => ServerFnError::new(message),
        AccountError::Internal(detail) => internal(detail),
    }
}
