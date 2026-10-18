//! Server functions for the logged-in user's own account.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use crate::server::{account_error, ctx};

/// The password rules in words, for hints under password fields.
#[server]
pub async fn password_policy_hint() -> Result<String, ServerFnError> {
    let ctx = ctx()?;
    Ok(backend::validation::describe_password_policy(
        &ctx.state.config.password_policy,
    ))
}

/// Changes the password, signs out every other session, and keeps this one.
#[server]
pub async fn change_password(
    current_password: String,
    new_password: String,
    new_password_confirm: String,
) -> Result<(), ServerFnError> {
    use backend::session::SessionUser;

    let ctx = ctx()?;
    let user = ctx.require_user().await?;
    let changed_at = backend::accounts::change_password(
        &ctx.state,
        &user.user_id,
        &user.username,
        &current_password,
        &new_password,
        &new_password_confirm,
    )
    .await
    .map_err(account_error)?;

    // Re-stamp this session so it survives the cut-off that ends the others.
    ctx.start_session(SessionUser {
        authenticated_at: changed_at,
        ..user
    })
    .await
}

/// Emails a confirmation link to the new address. Returns the message to show.
#[server]
pub async fn request_email_change(
    new_email: String,
    current_password: String,
) -> Result<String, ServerFnError> {
    let ctx = ctx()?;
    let user = ctx.require_user().await?;
    backend::accounts::request_email_change(
        &ctx.state,
        &user.user_id,
        &user.username,
        &new_email,
        &current_password,
    )
    .await
    .map_err(account_error)?;
    Ok(format!(
        "We sent a confirmation link to {}. It expires in {} hours.",
        new_email.trim(),
        ctx.state.config.mail.email_change_link_hours
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EmailChangeInfo {
    pub username: String,
    pub new_email: String,
}

/// What a confirmation link would do, or `None` if it's invalid or expired.
/// Public: the token itself is the credential.
#[server]
pub async fn get_email_change(token: String) -> Result<Option<EmailChangeInfo>, ServerFnError> {
    let ctx = ctx()?;
    Ok(backend::accounts::pending_email_change(&ctx.state, &token)
        .await
        .map_err(account_error)?
        .map(|change| EmailChangeInfo {
            username: change.username,
            new_email: change.new_email,
        }))
}

/// Applies a confirmed email change. Done by POST from the confirmation page,
/// not on GET, so mail scanners that prefetch links can't trigger it.
#[server]
pub async fn confirm_email_change(token: String) -> Result<EmailChangeInfo, ServerFnError> {
    let ctx = ctx()?;
    let change = backend::accounts::confirm_email_change(&ctx.state, &token)
        .await
        .map_err(account_error)?;
    Ok(EmailChangeInfo {
        username: change.username,
        new_email: change.new_email,
    })
}
