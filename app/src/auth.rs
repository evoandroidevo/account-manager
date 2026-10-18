use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use crate::server::{ctx, internal, now_unix};

/// Who is logged in, for the page chrome.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CurrentUser {
    pub username: String,
    pub is_admin: bool,
}

/// `Ok(None)` when nobody is logged in.
#[server]
pub async fn whoami() -> Result<Option<CurrentUser>, ServerFnError> {
    let ctx = ctx()?;
    Ok(ctx.user().await?.map(|user| CurrentUser {
        username: user.username,
        is_admin: user.is_admin,
    }))
}

/// The profile shown on the account page, read live from LLDAP.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AccountProfile {
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub member_since: String,
    pub is_admin: bool,
}

#[server]
pub async fn login(username: String, password: String) -> Result<(), ServerFnError> {
    use backend::session::SessionUser;

    let ctx = ctx()?;
    let state = &ctx.state;
    // LLDAP user ids are case-insensitive and stored lowercase.
    let username = username.trim().to_ascii_lowercase();
    let ip = ctx.client_ip.clone();

    let mut keys = vec![format!("user:{username}")];
    if let Some(ip) = &ip {
        keys.push(format!("ip:{ip}"));
    }
    let audit_meta = serde_json::json!({ "ip": ip });

    if state.login_throttle.is_blocked(&keys) {
        state
            .audit
            .record_best_effort(None, "login_throttled", Some(&username), Some(audit_meta))
            .await;
        return Err(ServerFnError::new(
            "Too many failed attempts. Please wait a few minutes and try again.",
        ));
    }

    let valid = state
        .lldap
        .verify_credentials(&username, &password)
        .await
        .map_err(|error| internal(format!("LDAP bind for {username} failed: {error}")))?;
    if !valid {
        state.login_throttle.record_failure(&keys);
        state
            .audit
            .record_best_effort(None, "login_failed", Some(&username), Some(audit_meta))
            .await;
        return Err(ServerFnError::new("Invalid username or password."));
    }

    let profile = state
        .lldap
        .get_user(&username)
        .await
        .map_err(|error| internal(format!("LLDAP lookup for {username} failed: {error}")))?
        .ok_or_else(|| internal(format!("{username} bound successfully but has no LLDAP record")))?;

    let user = state
        .users
        .ensure(&profile.id)
        .await
        .map_err(|error| internal(format!("user mirror for {username} failed: {error}")))?;
    if user.disabled_at.is_some() {
        return Err(ServerFnError::new("This account has been disabled."));
    }

    state.login_throttle.clear(&keys);
    if let Err(error) = state.users.touch_login(&user.id).await {
        log::warn!("failed to record login time for {username}: {error}");
    }
    // Accounts that predate this app get set up in each service on first login.
    if let Err(error) = backend::worker::reconcile_user(state, &user.id).await {
        log::error!("queueing provisioning for {username} failed: {error}");
    }

    ctx.start_session(SessionUser {
        user_id: user.id.clone(),
        username: profile.id.clone(),
        is_admin: profile.is_member_of(&state.config.auth.admin_group),
        authenticated_at: now_unix(),
    })
    .await?;

    state
        .audit
        .record_best_effort(Some(&profile.id), "login", None, Some(audit_meta))
        .await;

    leptos_axum::redirect("/");
    Ok(())
}

/// Ends the session and goes to `next` (a local path) or the login page.
#[server]
pub async fn logout(next: Option<String>) -> Result<(), ServerFnError> {
    ctx()?.end_session().await?;
    // Only same-site paths, so this can't be used as an open redirect.
    let target = next
        .filter(|path| path.starts_with('/') && !path.starts_with("//") && !path.contains('\\'))
        .unwrap_or_else(|| "/login".to_owned());
    leptos_axum::redirect(&target);
    Ok(())
}

/// `Ok(None)` means "not logged in"; errors mean the directory couldn't be read.
#[server]
pub async fn account_overview() -> Result<Option<AccountProfile>, ServerFnError> {
    let ctx = ctx()?;
    let Some(user) = ctx.user().await? else {
        return Ok(None);
    };

    let Some(profile) = ctx
        .state
        .lldap
        .get_user(&user.username)
        .await
        .map_err(|error| internal(format!("LLDAP lookup for {} failed: {error}", user.username)))?
    else {
        // Deleted from LLDAP while logged in: end the session.
        ctx.end_session().await?;
        return Ok(None);
    };

    Ok(Some(AccountProfile {
        is_admin: profile.is_member_of(&ctx.state.config.auth.admin_group),
        username: profile.id,
        display_name: profile.display_name,
        email: profile.email,
        member_since: profile
            .creation_date
            .split('T')
            .next()
            .unwrap_or_default()
            .to_owned(),
    }))
}

/// Strips Leptos' "error running server function" wrapper for display.
pub fn error_text(error: &ServerFnError) -> String {
    match error {
        ServerFnError::ServerError(message) => message.clone(),
        other => other.to_string(),
    }
}
