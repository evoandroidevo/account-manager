//! Account workflows shared by the server functions: invite signup, invite
//! administration, password changes and confirmed email changes.

use chrono::{Duration, Utc};
use rand::{Rng, distr::Alphanumeric};

use crate::{
    AppState,
    repo::{
        invites::{InviteRecord, hash_token},
        users::UserRecord,
    },
    validation::{validate_email, validate_password, validate_username},
};

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    /// A user-safe explanation, shown as-is.
    #[error("{0}")]
    Rejected(String),
    /// An infrastructure failure. Logged; the user sees a generic message.
    #[error("{0}")]
    Internal(String),
}

fn rejected(message: &str) -> AccountError {
    AccountError::Rejected(message.to_owned())
}

fn internal(context: &str, error: impl std::fmt::Display) -> AccountError {
    AccountError::Internal(format!("{context}: {error}"))
}

const MAX_NAME_LENGTH: usize = 64;
const MAX_INVITE_HOURS: i64 = 24 * 30;
const INVALID_INVITE: &str = "This invitation link is invalid or has expired.";
const INVALID_EMAIL_LINK: &str = "This confirmation link is invalid or has expired.";

/// 32 random alphanumeric characters (~190 bits). Only the hash is stored.
pub fn generate_token() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

/// Current time in the RFC 3339 form all stored timestamps use.
pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// "2026-10-15 19:07 UTC" from a stored RFC 3339 timestamp.
pub fn display_time(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|time| time.with_timezone(&Utc).format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|_| rfc3339.to_owned())
}

// ---------------------------------------------------------------------------
// Invites
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InviteStatus {
    Active,
    Used,
    Revoked,
    Expired,
}

/// Timestamps are RFC 3339 UTC strings, which compare correctly as text.
pub fn invite_status(invite: &InviteRecord, now: &str) -> InviteStatus {
    if invite.revoked_at.is_some() {
        InviteStatus::Revoked
    } else if invite.consumed_at.is_some() {
        InviteStatus::Used
    } else if invite.expires_at.as_str() <= now {
        InviteStatus::Expired
    } else {
        InviteStatus::Active
    }
}

pub fn invite_groups(invite: &InviteRecord) -> Vec<String> {
    serde_json::from_str(&invite.groups_json).unwrap_or_default()
}

pub async fn find_active_invite(
    state: &AppState,
    code: &str,
) -> Result<Option<InviteRecord>, AccountError> {
    let code = code.trim();
    if code.is_empty() {
        return Ok(None);
    }
    let invite = state
        .invites
        .find_by_token_hash(&hash_token(code))
        .await
        .map_err(|error| internal("invite lookup failed", error))?;
    Ok(invite.filter(|invite| invite_status(invite, &now_rfc3339()) == InviteStatus::Active))
}

pub struct NewInvite {
    /// The only time the raw token exists; it goes into the link shown once.
    pub token: String,
    pub record: InviteRecord,
}

pub async fn create_invite(
    state: &AppState,
    groups: Vec<String>,
    expiration_hours: i64,
    created_by: &str,
) -> Result<NewInvite, AccountError> {
    let mut groups: Vec<String> = groups
        .into_iter()
        .map(|group| group.trim().to_owned())
        .filter(|group| !group.is_empty())
        .collect();
    groups.sort();
    groups.dedup();

    if groups.is_empty() {
        return Err(rejected("Choose at least one group."));
    }
    if !(1..=MAX_INVITE_HOURS).contains(&expiration_hours) {
        return Err(AccountError::Rejected(format!(
            "Expiry must be between 1 and {MAX_INVITE_HOURS} hours."
        )));
    }

    let known = state
        .lldap
        .group_names()
        .await
        .map_err(|error| internal("listing LLDAP groups failed", error))?;
    if let Some(unknown) = groups.iter().find(|group| !known.contains(group)) {
        return Err(AccountError::Rejected(format!("Unknown group \"{unknown}\".")));
    }

    let token = generate_token();
    let now = Utc::now();
    let groups_json = serde_json::to_string(&groups).expect("strings always serialize");
    let record = state
        .invites
        .create_invite(
            &uuid::Uuid::new_v4().to_string(),
            &hash_token(&token),
            &groups_json,
            &now.to_rfc3339(),
            &(now + Duration::hours(expiration_hours)).to_rfc3339(),
            Some(created_by),
        )
        .await
        .map_err(|error| internal("storing invite failed", error))?;

    state
        .audit
        .record_best_effort(
            Some(created_by),
            "invite_created",
            Some(&record.id),
            Some(serde_json::json!({ "groups": groups, "expiration_hours": expiration_hours })),
        )
        .await;

    Ok(NewInvite { token, record })
}

pub async fn revoke_invite(state: &AppState, id: &str, actor: &str) -> Result<(), AccountError> {
    let revoked = state
        .invites
        .revoke_invite(id, &now_rfc3339())
        .await
        .map_err(|error| internal("revoking invite failed", error))?;
    if !revoked {
        return Err(rejected("That invite is no longer active."));
    }
    state
        .audit
        .record_best_effort(Some(actor), "invite_revoked", Some(id), None)
        .await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Signup
// ---------------------------------------------------------------------------

pub struct SignupForm {
    pub username: String,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub password: String,
    pub password_confirm: String,
}

pub async fn signup(
    state: &AppState,
    code: &str,
    form: SignupForm,
) -> Result<UserRecord, AccountError> {
    // Check the invite first so people without one can't probe which
    // usernames and emails exist.
    let invite = find_active_invite(state, code)
        .await?
        .ok_or_else(|| rejected(INVALID_INVITE))?;

    let username = form.username.trim().to_ascii_lowercase();
    let email = form.email.trim().to_owned();
    let first_name = form.first_name.trim().to_owned();
    let last_name = form.last_name.trim().to_owned();

    validate_username(&username).map_err(AccountError::Rejected)?;
    validate_email(&email).map_err(AccountError::Rejected)?;
    if first_name.is_empty() {
        return Err(rejected("Enter your first name."));
    }
    if first_name.chars().count() > MAX_NAME_LENGTH || last_name.chars().count() > MAX_NAME_LENGTH
    {
        return Err(AccountError::Rejected(format!(
            "Names can be at most {MAX_NAME_LENGTH} characters."
        )));
    }
    validate_password(&form.password, &state.config.password_policy)
        .map_err(AccountError::Rejected)?;
    if form.password != form.password_confirm {
        return Err(rejected("The passwords don't match."));
    }

    if state
        .lldap
        .username_exists(&username)
        .await
        .map_err(|error| internal("username lookup failed", error))?
    {
        return Err(rejected("That username is already taken."));
    }
    if state
        .lldap
        .email_exists(&email)
        .await
        .map_err(|error| internal("email lookup failed", error))?
    {
        return Err(rejected("That email address is already in use."));
    }

    let groups = invite_groups(&invite);
    state
        .lldap
        .provision_user(&username, &email, &first_name, &last_name, &form.password, &groups)
        .await
        .map_err(|error| internal(&format!("provisioning {username} failed"), error))?;

    // Consumed after provisioning (which rolls itself back on failure). If two
    // people race on one link, the loser's new account is removed again.
    let consumed = state
        .invites
        .consume_invite(&invite.token_hash, &now_rfc3339(), &username)
        .await;
    if !matches!(consumed, Ok(true)) {
        if let Err(error) = state.lldap.delete_user_account(&username).await {
            log::error!("could not roll back {username} after a failed invite consume: {error}");
        }
        return match consumed {
            Ok(_) => Err(rejected(INVALID_INVITE)),
            Err(error) => Err(internal("consuming invite failed", error)),
        };
    }

    let user = state
        .users
        .ensure(&username)
        .await
        .map_err(|error| internal("user mirror failed", error))?;

    state
        .audit
        .record_best_effort(
            Some(&username),
            "signup",
            Some(&invite.id),
            Some(serde_json::json!({ "groups": groups })),
        )
        .await;

    Ok(user)
}

// ---------------------------------------------------------------------------
// Password and email changes
// ---------------------------------------------------------------------------

/// Re-checks the current password before a sensitive change. Failures are
/// throttled per username, separately from login attempts.
async fn verify_current_password(
    state: &AppState,
    username: &str,
    password: &str,
) -> Result<(), AccountError> {
    let keys = [format!("reauth:{username}")];
    if state.login_throttle.is_blocked(&keys) {
        return Err(rejected(
            "Too many incorrect passwords. Please wait a few minutes and try again.",
        ));
    }
    let valid = state
        .lldap
        .verify_credentials(username, password)
        .await
        .map_err(|error| internal("LDAP bind failed", error))?;
    if !valid {
        state.login_throttle.record_failure(&keys);
        state
            .audit
            .record_best_effort(Some(username), "reauth_failed", None, None)
            .await;
        return Err(rejected("Your current password is incorrect."));
    }
    state.login_throttle.clear(&keys);
    Ok(())
}

/// Sends a security notice, logging (not failing) if delivery breaks.
async fn notify(state: &AppState, to: &str, subject: &str, body: String) {
    if let Err(error) = state.mailer.send(to, subject, body).await {
        log::error!("could not send \"{subject}\" to {to}: {error}");
    }
}

/// Changes the password and returns the change time (unix seconds), which
/// invalidates every session authenticated before it.
pub async fn change_password(
    state: &AppState,
    user_id: &str,
    username: &str,
    current_password: &str,
    new_password: &str,
    new_password_confirm: &str,
) -> Result<i64, AccountError> {
    validate_password(new_password, &state.config.password_policy)
        .map_err(AccountError::Rejected)?;
    if new_password != new_password_confirm {
        return Err(rejected("The new passwords don't match."));
    }
    if new_password == current_password {
        return Err(rejected("Choose a password different from your current one."));
    }

    verify_current_password(state, username, current_password).await?;

    state
        .lldap
        .set_password(username, new_password)
        .await
        .map_err(|error| internal("setting password failed", error))?;

    let changed_at = Utc::now().timestamp();
    state
        .users
        .set_password_changed_at(user_id, changed_at)
        .await
        .map_err(|error| internal("recording password change failed", error))?;
    state
        .audit
        .record_best_effort(Some(username), "password_changed", None, None)
        .await;

    match state.lldap.get_user(username).await {
        Ok(Some(profile)) => {
            notify(
                state,
                &profile.email,
                "Your password was changed",
                format!(
                    "Hi {},\n\nThe password for your account \"{username}\" was just changed.\n\n\
                     If this wasn't you, contact an administrator straight away.\n",
                    profile.display_name
                ),
            )
            .await;
        }
        Ok(None) => {}
        Err(error) => log::warn!("could not look up {username} for the password notice: {error}"),
    }

    Ok(changed_at)
}

pub async fn request_email_change(
    state: &AppState,
    user_id: &str,
    username: &str,
    new_email: &str,
    current_password: &str,
) -> Result<(), AccountError> {
    let new_email = new_email.trim();
    validate_email(new_email).map_err(AccountError::Rejected)?;

    let profile = state
        .lldap
        .get_user(username)
        .await
        .map_err(|error| internal("LLDAP lookup failed", error))?
        .ok_or_else(|| internal("LLDAP lookup failed", "user not found"))?;
    if profile.email.eq_ignore_ascii_case(new_email) {
        return Err(rejected("That is already your email address."));
    }

    verify_current_password(state, username, current_password).await?;

    if state
        .lldap
        .email_exists(new_email)
        .await
        .map_err(|error| internal("email lookup failed", error))?
    {
        return Err(rejected("That email address is already in use."));
    }

    let token = generate_token();
    let hours = state.config.mail.email_change_link_hours;
    let expires_at = (Utc::now() + Duration::hours(hours)).to_rfc3339();
    state
        .email_changes
        .create(user_id, new_email, &hash_token(&token), &expires_at)
        .await
        .map_err(|error| internal("storing email change failed", error))?;

    let link = format!(
        "{}/confirm-email?token={token}",
        state.config.server.public_url.trim_end_matches('/')
    );
    state
        .mailer
        .send(
            new_email,
            "Confirm your new email address",
            format!(
                "Hi {},\n\nConfirm that you want to use this address for your account \
                 \"{username}\" by opening the link below. It expires in {hours} hours.\n\n\
                 {link}\n\nIf you didn't ask for this, you can ignore this email.\n",
                profile.display_name
            ),
        )
        .await
        .map_err(|error| internal("sending confirmation email failed", error))?;

    state
        .audit
        .record_best_effort(
            Some(username),
            "email_change_requested",
            None,
            Some(serde_json::json!({ "new_email": new_email })),
        )
        .await;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingEmailChange {
    pub username: String,
    pub new_email: String,
}

pub async fn pending_email_change(
    state: &AppState,
    token: &str,
) -> Result<Option<PendingEmailChange>, AccountError> {
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }
    let Some(change) = state
        .email_changes
        .find_active(&hash_token(token), &now_rfc3339())
        .await
        .map_err(|error| internal("email change lookup failed", error))?
    else {
        return Ok(None);
    };
    let user = state
        .users
        .find_by_id(&change.user_id)
        .await
        .map_err(|error| internal("user lookup failed", error))?;
    Ok(user.map(|user| PendingEmailChange {
        username: user.username,
        new_email: change.new_email,
    }))
}

pub async fn confirm_email_change(
    state: &AppState,
    token: &str,
) -> Result<PendingEmailChange, AccountError> {
    let token = token.trim();
    let change = state
        .email_changes
        .find_active(&hash_token(token), &now_rfc3339())
        .await
        .map_err(|error| internal("email change lookup failed", error))?
        .ok_or_else(|| rejected(INVALID_EMAIL_LINK))?;
    let user = state
        .users
        .find_by_id(&change.user_id)
        .await
        .map_err(|error| internal("user lookup failed", error))?
        .ok_or_else(|| rejected(INVALID_EMAIL_LINK))?;

    if !state
        .email_changes
        .consume(&change.id, &now_rfc3339())
        .await
        .map_err(|error| internal("consuming email change failed", error))?
    {
        return Err(rejected(INVALID_EMAIL_LINK));
    }

    let apply = async {
        if state
            .lldap
            .email_exists(&change.new_email)
            .await
            .map_err(|error| internal("email lookup failed", error))?
        {
            return Err(rejected("That email address is now used by another account."));
        }
        let old_email = state
            .lldap
            .get_user(&user.username)
            .await
            .map_err(|error| internal("LLDAP lookup failed", error))?
            .map(|profile| profile.email);
        state
            .lldap
            .update_email(&user.username, &change.new_email)
            .await
            .map_err(|error| internal("updating email failed", error))?;
        Ok::<_, AccountError>(old_email)
    };

    let old_email = match apply.await {
        Ok(old_email) => old_email,
        Err(error) => {
            // Let the user retry the same link after a transient failure.
            if matches!(error, AccountError::Internal(_)) {
                let _ = state.email_changes.release(&change.id).await;
            }
            return Err(error);
        }
    };

    state
        .audit
        .record_best_effort(
            Some(&user.username),
            "email_changed",
            None,
            Some(serde_json::json!({ "old_email": old_email, "new_email": change.new_email })),
        )
        .await;

    if let Some(old_email) = old_email {
        notify(
            state,
            &old_email,
            "Your email address was changed",
            format!(
                "The email address for your account \"{}\" was changed to {}.\n\n\
                 If this wasn't you, contact an administrator straight away.\n",
                user.username, change.new_email
            ),
        )
        .await;
    }

    Ok(PendingEmailChange {
        username: user.username,
        new_email: change.new_email,
    })
}

#[cfg(test)]
mod tests {
    use super::{InviteStatus, generate_token, invite_status};
    use crate::repo::invites::InviteRecord;

    fn invite() -> InviteRecord {
        InviteRecord {
            id: "id".to_owned(),
            token_hash: "hash".to_owned(),
            groups_json: "[\"user\"]".to_owned(),
            created_at: "2026-01-01T00:00:00+00:00".to_owned(),
            expires_at: "2026-01-03T00:00:00+00:00".to_owned(),
            consumed_at: None,
            consumed_by: None,
            revoked_at: None,
        }
    }

    #[test]
    fn invite_status_reflects_lifecycle() {
        let now = "2026-01-02T00:00:00+00:00";
        assert_eq!(invite_status(&invite(), now), InviteStatus::Active);
        assert_eq!(
            invite_status(&invite(), "2026-01-03T00:00:00+00:00"),
            InviteStatus::Expired
        );
        let used = InviteRecord { consumed_at: Some(now.to_owned()), ..invite() };
        assert_eq!(invite_status(&used, now), InviteStatus::Used);
        let revoked = InviteRecord { revoked_at: Some(now.to_owned()), ..invite() };
        assert_eq!(invite_status(&revoked, now), InviteStatus::Revoked);
    }

    #[test]
    fn display_time_is_human_readable_utc() {
        assert_eq!(super::display_time("2026-10-15T19:07:31.123+00:00"), "2026-10-15 19:07 UTC");
        assert_eq!(super::display_time("not a time"), "not a time");
    }

    #[test]
    fn tokens_are_long_random_and_alphanumeric() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(a, b);
    }
}
