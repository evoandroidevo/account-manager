//! Server functions for invite signup and invite administration.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use crate::server::{account_error, ctx, internal, now_unix};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InviteInfo {
    pub expires_at: String,
    pub password_hint: String,
    /// Set when someone is already logged in on this browser.
    pub logged_in_as: Option<String>,
}

/// Details for the signup page, or `None` if the code isn't usable.
#[server]
pub async fn get_invite(code: String) -> Result<Option<InviteInfo>, ServerFnError> {
    use backend::accounts::{display_time, find_active_invite};
    use backend::validation::describe_password_policy;

    let ctx = ctx()?;
    let state = &ctx.state;
    let invite = find_active_invite(state, &code)
        .await
        .map_err(account_error)?;
    let Some(invite) = invite else {
        return Ok(None);
    };
    let logged_in_as = ctx.user().await?.map(|user| user.username);
    Ok(Some(InviteInfo {
        expires_at: display_time(&invite.expires_at),
        password_hint: describe_password_policy(&state.config.password_policy),
        logged_in_as,
    }))
}

/// Creates the account from an invite, logs the new user in and redirects
/// to the account page.
#[server]
pub async fn signup(
    code: String,
    username: String,
    email: String,
    first_name: String,
    last_name: String,
    password: String,
    password_confirm: String,
) -> Result<(), ServerFnError> {
    use backend::{
        accounts::{self, SignupForm},
        session::SessionUser,
    };

    let ctx = ctx()?;
    let state = &ctx.state;
    // Signing up would replace the current session with the new account.
    if ctx.user().await?.is_some() {
        return Err(ServerFnError::new(
            "You're logged in to another account. Log out first, then use the invitation again.",
        ));
    }
    let user = accounts::signup(
        state,
        &code,
        SignupForm {
            username,
            email,
            first_name,
            last_name,
            password,
            password_confirm,
        },
    )
    .await
    .map_err(account_error)?;

    let is_admin = match state.lldap.get_user(&user.username).await {
        Ok(Some(profile)) => profile.is_member_of(&state.config.auth.admin_group),
        _ => false,
    };
    ctx.start_session(SessionUser {
        user_id: user.id,
        username: user.username,
        is_admin,
        authenticated_at: now_unix(),
    })
    .await?;

    leptos_axum::redirect("/");
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InviteDefaults {
    /// Every group in LLDAP.
    pub groups: Vec<String>,
    /// Pre-selected groups.
    pub default_groups: Vec<String>,
    pub expiration_hours: i64,
}

#[server]
pub async fn get_invite_defaults() -> Result<InviteDefaults, ServerFnError> {
    let ctx = ctx()?;
    ctx.require_admin().await?;
    let state = &ctx.state;
    // LLDAP's own permission groups are never offered (or accepted).
    let groups: Vec<String> = state
        .lldap
        .group_names()
        .await
        .map_err(|error| internal(format!("listing LLDAP groups failed: {error}")))?
        .into_iter()
        .filter(|group| !backend::accounts::is_lldap_system_group(group))
        .collect();
    let default_groups = state
        .config
        .invites
        .default_groups
        .iter()
        .filter(|group| groups.contains(group))
        .cloned()
        .collect();
    Ok(InviteDefaults {
        groups,
        default_groups,
        expiration_hours: state.config.invites.expiration_hours,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InviteSummary {
    pub id: String,
    pub groups: Vec<String>,
    pub created_at: String,
    pub expires_at: String,
    /// "active", "used", "revoked" or "expired".
    pub status: String,
    pub used_by: Option<String>,
}

#[server]
pub async fn list_invites() -> Result<Vec<InviteSummary>, ServerFnError> {
    use backend::accounts::{
        display_time, invite_groups, invite_status, now_rfc3339, InviteStatus,
    };

    let ctx = ctx()?;
    ctx.require_admin().await?;
    let state = &ctx.state;
    let now = now_rfc3339();
    let invites = state
        .invites
        .list_recent(50)
        .await
        .map_err(|error| internal(format!("listing invites failed: {error}")))?;
    Ok(invites
        .into_iter()
        .map(|invite| InviteSummary {
            status: match invite_status(&invite, &now) {
                InviteStatus::Active => "active",
                InviteStatus::Used => "used",
                InviteStatus::Revoked => "revoked",
                InviteStatus::Expired => "expired",
            }
            .to_owned(),
            groups: invite_groups(&invite),
            created_at: display_time(&invite.created_at),
            expires_at: display_time(&invite.expires_at),
            used_by: invite.consumed_by.clone(),
            id: invite.id,
        })
        .collect())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CreatedInvite {
    pub url: String,
    pub groups: Vec<String>,
    pub expires_at: String,
}

#[server]
pub async fn create_invite(
    groups: Vec<String>,
    expiration_hours: i64,
) -> Result<CreatedInvite, ServerFnError> {
    use backend::accounts::{self, display_time};

    let ctx = ctx()?;
    let admin = ctx.require_admin().await?;
    let state = &ctx.state;
    let invite = accounts::create_invite(state, groups, expiration_hours, &admin.username)
        .await
        .map_err(account_error)?;
    Ok(CreatedInvite {
        url: format!(
            "{}/invite?code={}",
            state.config.server.public_url.trim_end_matches('/'),
            invite.token
        ),
        groups: accounts::invite_groups(&invite.record),
        expires_at: display_time(&invite.record.expires_at),
    })
}

#[server]
pub async fn revoke_invite(id: String) -> Result<(), ServerFnError> {
    let ctx = ctx()?;
    let admin = ctx.require_admin().await?;
    backend::accounts::revoke_invite(&ctx.state, &id, &admin.username)
        .await
        .map_err(account_error)
}
