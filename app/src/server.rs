//! Helpers available to server functions only (compiled with the `ssr` feature).

use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use backend::{
    session::{SessionUser, SESSION_USER_KEY},
    AppState,
};
use leptos::prelude::*;
use tower_sessions::Session;

pub fn app_state() -> Result<AppState, ServerFnError> {
    use_context::<AppState>().ok_or_else(|| internal("AppState missing from context"))
}

pub async fn session() -> Result<Session, ServerFnError> {
    leptos_axum::extract::<Session>()
        .await
        .map_err(|error| internal(format!("session layer missing: {error}")))
}

/// The logged-in user, or `None`. Sessions past their absolute lifetime are
/// destroyed here.
pub async fn session_user() -> Result<Option<SessionUser>, ServerFnError> {
    let state = app_state()?;
    let session = session().await?;
    let user = session
        .get::<SessionUser>(SESSION_USER_KEY)
        .await
        .map_err(|error| internal(format!("session read failed: {error}")))?;

    match user {
        Some(user) if user.is_expired(state.config.session.absolute_timeout_hours, now_unix()) => {
            let _ = session.flush().await;
            Ok(None)
        }
        other => Ok(other),
    }
}

/// Client IP for rate limiting. `X-Forwarded-For` is only honoured when the
/// config says we sit behind a proxy that overwrites it (Caddy does for
/// untrusted clients).
pub async fn client_ip() -> Option<String> {
    let state = app_state().ok()?;
    if state.config.server.trust_proxy_headers {
        let headers = leptos_axum::extract::<http::HeaderMap>().await.ok()?;
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(ip.to_owned());
        }
    }
    leptos_axum::extract::<ConnectInfo<SocketAddr>>()
        .await
        .ok()
        .map(|ConnectInfo(addr)| addr.ip().to_string())
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
