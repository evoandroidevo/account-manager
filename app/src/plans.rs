//! Server functions for tiers: the user's own plan, and the admin users page.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use crate::server::{account_error, ctx, internal};

/// How a service account looks to its owner. Error details stay admin-only.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ServiceHealth {
    Ready,
    SettingUp,
    Problem,
    NotManaged,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ServiceStatus {
    pub name: String,
    pub health: ServiceHealth,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlanInfo {
    pub tier_name: String,
    pub description: String,
    /// Display date, when the plan ends.
    pub valid_until: Option<String>,
    /// Jellyfin and Seerr, when enabled.
    pub services: Vec<ServiceStatus>,
}

#[cfg(feature = "ssr")]
fn health(state: Option<&str>) -> ServiceHealth {
    use backend::repo::provisioning::{STATE_ERROR, STATE_OK, STATE_SKIPPED};
    match state {
        Some(state) if state == STATE_OK => ServiceHealth::Ready,
        Some(state) if state == STATE_SKIPPED => ServiceHealth::NotManaged,
        Some(state) if state == STATE_ERROR => ServiceHealth::Problem,
        _ => ServiceHealth::SettingUp,
    }
}

/// `Ok(None)` when nobody is logged in.
#[server]
pub async fn my_plan() -> Result<Option<PlanInfo>, ServerFnError> {
    use backend::{accounts::display_time, services::Service, tiers};

    let ctx = ctx()?;
    let Some(user) = ctx.user().await? else {
        return Ok(None);
    };
    let state = &ctx.state;
    let effective = tiers::effective_tier(state, &user.user_id)
        .await
        .map_err(account_error)?;
    let accounts = state
        .provisioning
        .accounts_for_user(&user.user_id)
        .await
        .map_err(|error| internal(format!("loading service accounts failed: {error}")))?;

    let services = state
        .services
        .enabled()
        .into_iter()
        .filter(|service| *service != Service::Lldap)
        .map(|service| ServiceStatus {
            name: service.label().to_owned(),
            health: health(
                accounts
                    .iter()
                    .find(|account| account.service == service.as_str())
                    .map(|account| account.state.as_str()),
            ),
        })
        .collect();

    Ok(Some(PlanInfo {
        tier_name: effective.tier.name,
        description: effective.tier.description,
        valid_until: effective.valid_until.as_deref().map(display_time),
        services,
    }))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TierOption {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AdminServiceStatus {
    pub name: String,
    /// pending, ok, error or skipped.
    pub state: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub tier_id: String,
    pub tier_name: String,
    /// "manual", "whop" or "default".
    pub source: String,
    pub valid_until: Option<String>,
    /// The manual override's end date as YYYY-MM-DD, for the edit form.
    pub manual_until_date: Option<String>,
    pub note: Option<String>,
    pub disabled: bool,
    pub services: Vec<AdminServiceStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UsersPage {
    pub tiers: Vec<TierOption>,
    pub default_tier: String,
    pub users: Vec<UserRow>,
}

#[server]
pub async fn list_users() -> Result<UsersPage, ServerFnError> {
    use std::collections::HashMap;

    use backend::{
        accounts::{display_time, now_rfc3339},
        repo::entitlements::{Entitlement, SOURCE_MANUAL},
        services::Service,
        tiers,
    };

    let ctx = ctx()?;
    ctx.require_admin().await?;
    let state = &ctx.state;
    fn load_error(error: impl std::fmt::Display) -> ServerFnError {
        internal(format!("loading users failed: {error}"))
    }
    let users = state.users.all().await.map_err(load_error)?;
    let entitlements = state.entitlements.all().await.map_err(load_error)?;
    let accounts = state.provisioning.all_accounts().await.map_err(load_error)?;

    let mut by_user: HashMap<&str, Vec<Entitlement>> = HashMap::new();
    for entitlement in &entitlements {
        by_user.entry(entitlement.user_id.as_str()).or_default().push(entitlement.clone());
    }
    let now = now_rfc3339();
    let enabled = state.services.enabled();

    let rows = users
        .into_iter()
        .map(|user| {
            let user_entitlements = by_user.get(user.id.as_str()).map(Vec::as_slice).unwrap_or_default();
            let effective = tiers::resolve(&state.config, user_entitlements, &now);
            let manual = user_entitlements.iter().find(|e| e.source == SOURCE_MANUAL);
            let services = enabled
                .iter()
                .map(|service: &Service| {
                    let account = accounts
                        .iter()
                        .find(|a| a.user_id == user.id && a.service == service.as_str());
                    AdminServiceStatus {
                        name: service.label().to_owned(),
                        state: account.map_or("pending".to_owned(), |a| a.state.clone()),
                        detail: account.and_then(|a| a.detail.clone()),
                    }
                })
                .collect();
            UserRow {
                tier_id: effective.tier.id,
                tier_name: effective.tier.name,
                source: effective.source,
                valid_until: effective.valid_until.as_deref().map(display_time),
                manual_until_date: manual
                    .and_then(|e| e.valid_until.as_deref())
                    .and_then(|until| until.get(..10))
                    .map(str::to_owned),
                note: manual.and_then(|e| e.note.clone()),
                disabled: user.disabled_at.is_some(),
                services,
                username: user.username,
                id: user.id,
            }
        })
        .collect();

    Ok(UsersPage {
        tiers: state
            .config
            .tiers
            .iter()
            .map(|tier| TierOption { id: tier.id.clone(), name: tier.name.clone() })
            .collect(),
        default_tier: state.config.default_tier.clone(),
        users: rows,
    })
}

/// Sets an admin override. `valid_until` is YYYY-MM-DD or blank for no end.
#[server]
pub async fn set_user_tier(
    user_id: String,
    tier_id: String,
    valid_until: String,
    note: String,
) -> Result<(), ServerFnError> {
    use backend::tiers;

    let ctx = ctx()?;
    let admin = ctx.require_admin().await?;
    let until = tiers::parse_end_date(&valid_until).map_err(account_error)?;
    tiers::set_manual_tier(&ctx.state, &user_id, &tier_id, until, Some(&note), &admin.username)
        .await
        .map_err(account_error)
}

#[server]
pub async fn clear_user_tier(user_id: String) -> Result<(), ServerFnError> {
    let ctx = ctx()?;
    let admin = ctx.require_admin().await?;
    backend::tiers::clear_manual_tier(&ctx.state, &user_id, &admin.username)
        .await
        .map_err(account_error)
}

/// Re-applies the user's tier to every service now.
#[server]
pub async fn resync_user(user_id: String) -> Result<(), ServerFnError> {
    let ctx = ctx()?;
    let admin = ctx.require_admin().await?;
    backend::tiers::queue_sync(&ctx.state, &user_id)
        .await
        .map_err(account_error)?;
    ctx.state
        .audit
        .record_best_effort(Some(&admin.username), "resync", Some(&user_id), None)
        .await;
    Ok(())
}
