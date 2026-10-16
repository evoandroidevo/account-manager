use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use crate::server::{app_state, client_ip, internal, now_unix, session, session_user};

/// The profile shown on the account page, read live from LLDAP.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AccountProfile {
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub member_since: String,
    pub groups: Vec<String>,
    pub is_admin: bool,
}

#[server]
pub async fn login(username: String, password: String) -> Result<(), ServerFnError> {
    use backend::session::{SessionUser, SESSION_USER_KEY};

    let state = app_state()?;
    // LLDAP user ids are case-insensitive and stored lowercase.
    let username = username.trim().to_ascii_lowercase();
    let ip = client_ip().await;

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

    let session = session().await?;
    // New session id on privilege change, so a planted id can't be reused.
    session
        .cycle_id()
        .await
        .map_err(|error| internal(format!("session cycle failed: {error}")))?;
    session
        .insert(
            SESSION_USER_KEY,
            SessionUser {
                user_id: user.id.clone(),
                username: profile.id.clone(),
                is_admin: profile.is_member_of(&state.config.auth.admin_group),
                authenticated_at: now_unix(),
            },
        )
        .await
        .map_err(|error| internal(format!("session write failed: {error}")))?;

    state
        .audit
        .record_best_effort(Some(&profile.id), "login", None, Some(audit_meta))
        .await;

    leptos_axum::redirect("/");
    Ok(())
}

#[server]
pub async fn logout() -> Result<(), ServerFnError> {
    let session = session().await?;
    session
        .flush()
        .await
        .map_err(|error| internal(format!("session flush failed: {error}")))?;
    leptos_axum::redirect("/login");
    Ok(())
}

/// `Ok(None)` means "not logged in"; errors mean the directory couldn't be read.
#[server]
pub async fn account_overview() -> Result<Option<AccountProfile>, ServerFnError> {
    let Some(user) = session_user().await? else {
        return Ok(None);
    };
    let state = app_state()?;

    let Some(profile) = state
        .lldap
        .get_user(&user.username)
        .await
        .map_err(|error| internal(format!("LLDAP lookup for {} failed: {error}", user.username)))?
    else {
        // Deleted from LLDAP while logged in: end the session.
        let _ = session().await?.flush().await;
        return Ok(None);
    };

    Ok(Some(AccountProfile {
        is_admin: profile.is_member_of(&state.config.auth.admin_group),
        username: profile.id,
        display_name: profile.display_name,
        email: profile.email,
        member_since: profile
            .creation_date
            .split('T')
            .next()
            .unwrap_or_default()
            .to_owned(),
        groups: profile
            .groups
            .into_iter()
            .map(|group| group.display_name)
            .collect(),
    }))
}

/// Strips Leptos' "error running server function" wrapper for display.
pub fn error_text(error: &ServerFnError) -> String {
    match error {
        ServerFnError::ServerError(message) => message.clone(),
        other => other.to_string(),
    }
}
