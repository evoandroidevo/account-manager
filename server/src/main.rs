use std::{net::SocketAddr, sync::Arc, time::Duration as StdDuration};

use app::*;
use axum::{
    body::Bytes,
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use backend::{
    config::AppConfig,
    repo::invites::{InviteRepository, STALE_INVITE_RETENTION_DAYS},
    AppState,
};
use leptos::prelude::*;
use leptos_axum::{generate_route_list, LeptosRoutes};
use tower_sessions::{cookie::SameSite, session_store::ExpiredDeletion, Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::SqliteStore;

#[tokio::main]
async fn main() {
    // `account-manager healthcheck`: the container's HEALTHCHECK (the image
    // has no curl).
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if healthcheck() { 0 } else { 1 });
    }

    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        // tower-sessions emits a tracing span per store call; keep those out.
        .with_module_level("tracing::span", log::LevelFilter::Warn)
        .with_module_level("sqlx", log::LevelFilter::Warn)
        .init()
        .expect("logger must initialize");

    let config = AppConfig::load_default().expect("configuration must be valid");
    let allowed_origin: Arc<str> = origin_of(&config.server.public_url).into();
    let state = AppState::initialize(config)
        .await
        .expect("application state must initialize");

    let session_store = SqliteStore::new(state.pool.clone());
    session_store
        .migrate()
        .await
        .expect("session table must migrate");
    tokio::spawn(
        session_store
            .clone()
            .continuously_delete_expired(StdDuration::from_secs(60 * 60)),
    );
    spawn_invite_cleanup_task(state.invites.clone());
    backend::worker::spawn(state.clone());

    let session_config = &state.config.session;
    let session_layer = SessionManagerLayer::new(session_store)
        .with_name(session_config.cookie_name.clone())
        .with_secure(session_config.secure_cookie)
        .with_http_only(true)
        // Lax (not Strict) so the cookie survives top-level redirects back
        // from Whop checkout; cross-site POSTs are stopped by origin_guard.
        .with_same_site(SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(time::Duration::minutes(
            session_config.idle_timeout_minutes,
        )));

    let conf = get_configuration(None).expect("leptos configuration must load");
    let addr = conf.leptos_options.site_addr;
    let mut leptos_options = conf.leptos_options;
    if std::env::var_os("LEPTOS_SITE_ROOT").is_none() {
        if let Some(site) = bundled_site_root() {
            leptos_options.site_root = site.into();
        }
    }
    let routes = generate_route_list(App);

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        // Server-to-server (no Origin header), so origin_guard lets it through;
        // the Whop signature is the authentication.
        .route("/api/webhooks/whop", post(whop_webhook).with_state(state.clone()))
        .leptos_routes_with_context(
            &leptos_options,
            routes,
            {
                let state = state.clone();
                move || provide_context(state.clone())
            },
            {
                let leptos_options = leptos_options.clone();
                move || shell(leptos_options.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler(shell))
        .layer(middleware::from_fn_with_state(allowed_origin, origin_guard))
        .layer(session_layer)
        .with_state(leptos_options);

    log::info!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("listen address must be available");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server must run");
}

/// POST /api/webhooks/whop: Whop membership events, verified against the raw body.
async fn whop_webhook(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let reply = backend::whop::handle_webhook(
        &state,
        header("webhook-id"),
        header("webhook-timestamp"),
        header("webhook-signature"),
        &body,
    )
    .await;
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, reply.body).into_response()
}

/// CSRF defence for state-changing requests: browsers always send `Origin`
/// on cross-origin POSTs, so anything not from our own public origin is
/// refused. Requests without `Origin` (curl, older browsers on same-origin
/// form posts) fall back to `Sec-Fetch-Site`.
async fn origin_guard(State(allowed): State<Arc<str>>, request: Request, next: Next) -> Response {
    let safe_method = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if !safe_method {
        let headers = request.headers();
        let rejected = match headers.get(header::ORIGIN) {
            Some(origin) => origin.as_bytes() != allowed.as_bytes(),
            None => headers
                .get("sec-fetch-site")
                .is_some_and(|site| site.as_bytes() == b"cross-site"),
        };
        if rejected {
            log::warn!(
                "rejected cross-origin {} {} (origin {:?})",
                request.method(),
                request.uri().path(),
                headers.get(header::ORIGIN)
            );
            return (StatusCode::FORBIDDEN, "cross-origin request refused").into_response();
        }
    }
    next.run(request).await
}

/// `https://account.example.com/some/path` -> `https://account.example.com`
fn origin_of(url: &str) -> String {
    let url = url.trim_end_matches('/');
    match url.find("://") {
        Some(scheme_end) => {
            let rest = &url[scheme_end + 3..];
            let host_end = rest.find('/').map_or(url.len(), |i| scheme_end + 3 + i);
            url[..host_end].to_owned()
        }
        None => url.to_owned(),
    }
}

/// Periodically deletes expired/revoked invites older than the retention window.
fn spawn_invite_cleanup_task(invites: InviteRepository) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(StdDuration::from_secs(24 * 60 * 60));
        loop {
            interval.tick().await;
            let cutoff = chrono::Utc::now() - chrono::Duration::days(STALE_INVITE_RETENTION_DAYS);
            match invites.cleanup_stale(&cutoff.to_rfc3339()).await {
                Ok(removed) if removed > 0 => log::info!("removed {removed} stale invites"),
                Ok(_) => {}
                Err(error) => log::error!("invite cleanup failed: {error}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::origin_of;

    #[test]
    fn origin_strips_path_and_trailing_slash() {
        assert_eq!(origin_of("https://account.example.com/"), "https://account.example.com");
        assert_eq!(origin_of("https://account.example.com/a/b"), "https://account.example.com");
        assert_eq!(origin_of("http://127.0.0.1:3000"), "http://127.0.0.1:3000");
    }
}

/// Standalone release archives ship the site (wasm, JS, CSS) in `site/`
/// next to the executable; Leptos would otherwise look in `target/site`.
fn bundled_site_root() -> Option<String> {
    let site = std::env::current_exe().ok()?.parent()?.join("site");
    site.is_dir().then(|| site.to_string_lossy().into_owned())
}

/// Asks the running server's `/health` on this machine, at the port from
/// `LEPTOS_SITE_ADDR`.
fn healthcheck() -> bool {
    use std::io::{Read, Write};

    let port = std::env::var("LEPTOS_SITE_ADDR")
        .ok()
        .and_then(|addr| addr.parse::<SocketAddr>().ok())
        .map_or(3000, |addr| addr.port());
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, StdDuration::from_secs(3)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(StdDuration::from_secs(3)));
    if stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.starts_with("HTTP/1.1 200")
}
