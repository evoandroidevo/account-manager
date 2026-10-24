//! Which tier an account is on, and changing it.
//!
//! The effective tier is the first active one of: a manual override set by an
//! admin, a paid Whop membership (phase 4), then `default_tier`. Changing it
//! only records the decision and queues sync jobs; the worker applies it to
//! each service.

use chrono::{DateTime, Utc};

use crate::{
    AppState,
    accounts::{AccountError, now_rfc3339},
    config::{AppConfig, TierConfig},
    repo::entitlements::{Entitlement, SOURCE_MANUAL, SOURCE_WHOP},
};

pub const SOURCE_DEFAULT: &str = "default";

#[derive(Clone, Debug, PartialEq)]
pub struct EffectiveTier {
    pub tier: TierConfig,
    /// "manual", "whop" or "default".
    pub source: String,
    pub valid_until: Option<String>,
}

/// Picks the effective tier from a user's entitlements at time `now`
/// (RFC 3339). Expired entitlements and ones naming a tier that no longer
/// exists are ignored.
pub fn resolve(config: &AppConfig, entitlements: &[Entitlement], now: &str) -> EffectiveTier {
    for source in [SOURCE_MANUAL, SOURCE_WHOP] {
        let Some(entitlement) = entitlements.iter().find(|e| e.source == source) else {
            continue;
        };
        let active = entitlement
            .valid_until
            .as_deref()
            .is_none_or(|until| until > now);
        if !active {
            continue;
        }
        match config.tier(&entitlement.tier) {
            Some(tier) => {
                return EffectiveTier {
                    tier: tier.clone(),
                    source: source.to_owned(),
                    valid_until: entitlement.valid_until.clone(),
                };
            }
            None => log::warn!(
                "user {} has a {source} entitlement for unknown tier \"{}\"",
                entitlement.user_id,
                entitlement.tier
            ),
        }
    }
    EffectiveTier {
        tier: config
            .tier(&config.default_tier)
            .expect("default_tier is checked at startup")
            .clone(),
        source: SOURCE_DEFAULT.to_owned(),
        valid_until: None,
    }
}

/// Parses an end date from a form ("YYYY-MM-DD", blank for none) as the end
/// of that day in UTC.
pub fn parse_end_date(value: &str) -> Result<Option<DateTime<Utc>>, AccountError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(23, 59, 59))
        .map(|time| Some(time.and_utc()))
        .ok_or_else(|| AccountError::Rejected("Enter the end date as YYYY-MM-DD.".to_owned()))
}

fn internal(context: &str, error: impl std::fmt::Display) -> AccountError {
    AccountError::Internal(format!("{context}: {error}"))
}

pub async fn effective_tier(state: &AppState, user_id: &str) -> Result<EffectiveTier, AccountError> {
    let entitlements = state
        .entitlements
        .for_user(user_id)
        .await
        .map_err(|error| internal("loading entitlements failed", error))?;
    Ok(resolve(&state.config, &entitlements, &now_rfc3339()))
}

/// Queues a sync of every enabled service for the user, to run now.
pub async fn queue_sync(state: &AppState, user_id: &str) -> Result<(), AccountError> {
    let now = now_rfc3339();
    for service in state.services.enabled() {
        state
            .provisioning
            .enqueue(user_id, service.as_str(), &now)
            .await
            .map_err(|error| internal("queueing sync failed", error))?;
    }
    Ok(())
}

/// Sets an admin override. `valid_until` (RFC 3339) must be in the future;
/// `None` means it never expires.
pub async fn set_manual_tier(
    state: &AppState,
    user_id: &str,
    tier_id: &str,
    valid_until: Option<DateTime<Utc>>,
    note: Option<&str>,
    actor: &str,
) -> Result<(), AccountError> {
    if state.config.tier(tier_id).is_none() {
        return Err(AccountError::Rejected(format!("Unknown tier \"{tier_id}\".")));
    }
    if valid_until.is_some_and(|until| until <= Utc::now()) {
        return Err(AccountError::Rejected("The end date must be in the future.".to_owned()));
    }
    let valid_until = valid_until.map(|until| until.to_rfc3339());
    let note = note.map(str::trim).filter(|note| !note.is_empty());
    state
        .entitlements
        .set(user_id, SOURCE_MANUAL, tier_id, valid_until.as_deref(), note, Some(actor))
        .await
        .map_err(|error| internal("saving tier failed", error))?;
    state
        .audit
        .record_best_effort(
            Some(actor),
            "tier_set",
            Some(user_id),
            Some(serde_json::json!({ "tier": tier_id, "valid_until": valid_until, "note": note })),
        )
        .await;
    queue_sync(state, user_id).await
}

/// Removes an admin override, returning the user to their paid or default tier.
pub async fn clear_manual_tier(state: &AppState, user_id: &str, actor: &str) -> Result<(), AccountError> {
    let removed = state
        .entitlements
        .clear(user_id, SOURCE_MANUAL)
        .await
        .map_err(|error| internal("clearing tier failed", error))?;
    if !removed {
        return Err(AccountError::Rejected("That user has no manual tier.".to_owned()));
    }
    state
        .audit
        .record_best_effort(Some(actor), "tier_cleared", Some(user_id), None)
        .await;
    queue_sync(state, user_id).await
}

#[cfg(test)]
mod tests {
    use super::{SOURCE_DEFAULT, resolve};
    use crate::{
        config::{AppConfig, TierConfig},
        repo::entitlements::{Entitlement, SOURCE_MANUAL, SOURCE_WHOP},
    };

    fn tier(id: &str) -> TierConfig {
        TierConfig {
            id: id.to_owned(),
            name: id.to_uppercase(),
            description: String::new(),
            price_label: None,
            lldap_groups: Vec::new(),
            whop_plan_ids: Vec::new(),
            jellyfin: None,
            seerr: None,
        }
    }

    fn config() -> AppConfig {
        AppConfig {
            default_tier: "free".to_owned(),
            tiers: vec![tier("free"), tier("plus"), tier("premium")],
            ..AppConfig::default()
        }
    }

    fn entitlement(source: &str, tier: &str, valid_until: Option<&str>) -> Entitlement {
        Entitlement {
            user_id: "u".to_owned(),
            source: source.to_owned(),
            tier: tier.to_owned(),
            valid_until: valid_until.map(str::to_owned),
            note: None,
            updated_at: String::new(),
            updated_by: None,
        }
    }

    const NOW: &str = "2026-06-01T00:00:00+00:00";

    #[test]
    fn end_dates_mean_the_end_of_that_day_in_utc() {
        use super::parse_end_date;
        assert_eq!(parse_end_date("  ").unwrap(), None);
        assert_eq!(
            parse_end_date("2026-12-31").unwrap().unwrap().to_rfc3339(),
            "2026-12-31T23:59:59+00:00"
        );
        assert!(parse_end_date("31/12/2026").is_err());
    }

    #[test]
    fn falls_back_to_the_default_tier() {
        let effective = resolve(&config(), &[], NOW);
        assert_eq!(effective.tier.id, "free");
        assert_eq!(effective.source, SOURCE_DEFAULT);
    }

    #[test]
    fn manual_beats_whop_and_whop_beats_default() {
        let both = [
            entitlement(SOURCE_WHOP, "plus", None),
            entitlement(SOURCE_MANUAL, "premium", None),
        ];
        assert_eq!(resolve(&config(), &both, NOW).tier.id, "premium");
        let whop = [entitlement(SOURCE_WHOP, "plus", None)];
        assert_eq!(resolve(&config(), &whop, NOW).source, SOURCE_WHOP);
    }

    #[test]
    fn expired_and_unknown_entitlements_are_ignored() {
        let entitlements = [
            entitlement(SOURCE_MANUAL, "premium", Some("2026-05-31T23:59:59+00:00")),
            entitlement(SOURCE_WHOP, "gold", None),
        ];
        assert_eq!(resolve(&config(), &entitlements, NOW).tier.id, "free");

        let future = [entitlement(SOURCE_MANUAL, "plus", Some("2026-06-02T00:00:00+00:00"))];
        let effective = resolve(&config(), &future, NOW);
        assert_eq!(effective.tier.id, "plus");
        assert_eq!(effective.valid_until.as_deref(), Some("2026-06-02T00:00:00+00:00"));
    }
}
