//! Switching between paid plans without paying twice.
//!
//! Whop can't move a membership to another plan, so:
//! - Upgrades are a new checkout whose first payment is reduced by the unused
//!   value of the current paid membership (a single-use promo code, applied
//!   to the checkout link). When the new membership is active, the old one is
//!   cancelled immediately.
//! - Downgrades (to a cheaper plan or to the free tier) are scheduled: the
//!   current membership stops renewing and stays active until its period
//!   ends; then the user moves to the cheaper plan.

use chrono::{DateTime, Duration, Utc};
use rand::{Rng, distr::Alphanumeric};
use serde_json::json;

use crate::{
    AppState,
    accounts::{AccountError, display_time},
    config::TierConfig,
    repo::whop::{PendingDowngrade, WhopMembership},
    whop::{MembershipInfo, WhopClient, is_entitled},
};

/// Credits below this aren't worth a promo code.
pub const MIN_CREDIT_CENTS: i64 = 50;
/// How long an upgrade's promo code stays usable.
const PROMO_CODE_LIFETIME_HOURS: i64 = 2;

/// The unused value of a period: `renewal_cents` scaled by the time left,
/// rounded down to the cent, and never more than `cap_cents` (the new plan's
/// first payment).
pub fn credit_cents(
    renewal_cents: i64,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
    now: DateTime<Utc>,
    cap_cents: i64,
) -> i64 {
    let total = (period_end - period_start).num_seconds();
    let left = (period_end - now).num_seconds().clamp(0, total.max(0));
    if total <= 0 || renewal_cents <= 0 {
        return 0;
    }
    let credit = (renewal_cents as i128 * left as i128 / total as i128) as i64;
    credit.clamp(0, cap_cents.max(0))
}

fn parse_time(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|time| time.with_timezone(&Utc))
}

fn internal(context: &str, error: impl std::fmt::Display) -> AccountError {
    AccountError::Internal(format!("{context}: {error}"))
}

fn rejected(message: impl Into<String>) -> AccountError {
    AccountError::Rejected(message.into())
}

fn client(state: &AppState) -> Result<&WhopClient, AccountError> {
    state
        .whop_client
        .as_ref()
        .ok_or_else(|| rejected("Payments aren't set up yet."))
}

/// The user's current paid membership: their best active Whop membership.
#[derive(Clone, Debug)]
pub struct PaidPlan {
    pub membership: WhopMembership,
    pub tier: TierConfig,
}

pub async fn current_paid_plan(state: &AppState, user_id: &str) -> Result<Option<PaidPlan>, AccountError> {
    let memberships = state
        .whop
        .for_user(user_id)
        .await
        .map_err(|error| internal("loading Whop memberships failed", error))?;
    let config = &state.config;
    Ok(memberships
        .into_iter()
        .filter(|membership| is_entitled(&membership.status))
        .filter_map(|membership| {
            let tier = config.tier_for_whop_plan(&membership.plan_id)?.clone();
            Some(PaidPlan { membership, tier })
        })
        .max_by_key(|plan| config.tier_rank(&plan.tier.id)))
}

/// What the plans page offers for one tier.
#[derive(Clone, Debug, PartialEq)]
pub enum PlanOption {
    /// The user's current tier.
    Current,
    /// Buy it outright (no paid plan to credit).
    Buy { charge_cents: i64 },
    /// Switch now; the unused value of the current plan comes off the first payment.
    Upgrade { list_cents: i64, credit_cents: i64, from_tier: String },
    /// Switch when the current paid period ends.
    Downgrade { effective: String },
    /// Not available (no Whop plan, or payments disabled).
    Unavailable,
}

/// The plans page: the current paid plan, any scheduled downgrade, and what
/// each tier offers.
pub struct PlanOptions {
    pub paid: Option<PaidPlan>,
    pub pending: Option<PendingDowngrade>,
    pub tiers: Vec<(TierConfig, PlanOption)>,
}

pub async fn plan_options(
    state: &AppState,
    user_id: &str,
    effective_tier_id: &str,
) -> Result<PlanOptions, AccountError> {
    let config = &state.config;
    let paid = current_paid_plan(state, user_id).await?;
    let pending = live_pending_downgrade(state, user_id).await?;
    let paid_rank = paid.as_ref().map(|plan| config.tier_rank(&plan.tier.id));

    // The credit only depends on the current plan, so work it out once.
    let mut credit_source: Option<(i64, DateTime<Utc>, DateTime<Utc>)> = None;
    if let (Some(client), Some(plan)) = (&state.whop_client, &paid) {
        let period = (
            parse_time(plan.membership.current_period_start.as_deref()),
            parse_time(plan.membership.current_period_end.as_deref()),
        );
        if let (Some(start), Some(end)) = period
            && plan.membership.status != "trialing"
        {
            match client.plan_price(&plan.membership.plan_id).await {
                Ok(price) => credit_source = Some((price.renewal_cents(), start, end)),
                Err(error) => log::warn!("could not price the current plan: {error}"),
            }
        }
    }

    let mut tiers = Vec::new();
    for tier in &config.tiers {
        let rank = config.tier_rank(&tier.id);
        let option = if tier.id == effective_tier_id && paid_rank.is_none_or(|paid| paid == rank) {
            PlanOption::Current
        } else if let Some(plan) = paid.as_ref().filter(|_| paid_rank.is_some_and(|paid| rank < paid)) {
            PlanOption::Downgrade {
                effective: plan
                    .membership
                    .current_period_end
                    .as_deref()
                    .map(display_time)
                    .unwrap_or_else(|| "the end of the billing period".to_owned()),
            }
        } else if paid_rank.is_some_and(|paid| rank == paid) {
            PlanOption::Current
        } else {
            match (&state.whop_client, tier.whop_plan_ids.first()) {
                (Some(client), Some(plan_id)) => match client.plan_price(plan_id).await {
                    Ok(price) => {
                        let list = price.first_charge_cents();
                        match (&paid, credit_source) {
                            (Some(plan), Some((renewal, start, end))) => PlanOption::Upgrade {
                                list_cents: list,
                                credit_cents: credit_cents(renewal, start, end, Utc::now(), list),
                                from_tier: plan.tier.name.clone(),
                            },
                            (Some(plan), None) => PlanOption::Upgrade {
                                list_cents: list,
                                credit_cents: 0,
                                from_tier: plan.tier.name.clone(),
                            },
                            (None, _) => PlanOption::Buy { charge_cents: list },
                        }
                    }
                    Err(error) => {
                        log::warn!("could not price {plan_id}: {error}");
                        PlanOption::Unavailable
                    }
                },
                _ => PlanOption::Unavailable,
            }
        };
        tiers.push((tier.clone(), option));
    }
    Ok(PlanOptions { paid, pending, tiers })
}

/// A scheduled downgrade, dropped once it no longer needs showing: the user
/// moved to the free tier (nothing to buy) after the old membership ended.
async fn live_pending_downgrade(
    state: &AppState,
    user_id: &str,
) -> Result<Option<PendingDowngrade>, AccountError> {
    let Some(pending) = state
        .whop
        .pending_downgrade(user_id)
        .await
        .map_err(|error| internal("loading scheduled downgrade failed", error))?
    else {
        return Ok(None);
    };
    let still_running = state
        .whop
        .find(&pending.membership_id)
        .await
        .map_err(|error| internal("loading Whop membership failed", error))?
        .is_some_and(|membership| is_entitled(&membership.status));
    let to_paid_tier = state
        .config
        .tier(&pending.to_tier)
        .is_some_and(|tier| !tier.whop_plan_ids.is_empty());
    if !still_running && !to_paid_tier {
        let _ = state.whop.clear_pending_downgrade(user_id).await;
        return Ok(None);
    }
    Ok(Some(pending))
}

/// Starts a Whop checkout for `tier_id` and returns the URL to send the
/// browser to. When the user already pays for a lower plan, the unused value
/// of it comes off the first payment and the old membership is replaced.
pub async fn start_checkout(
    state: &AppState,
    user_id: &str,
    username: &str,
    tier_id: &str,
) -> Result<String, AccountError> {
    let client = client(state)?;
    let config = &state.config;
    let tier = config.tier(tier_id).ok_or_else(|| rejected("Unknown plan."))?;
    let plan_id = tier
        .whop_plan_ids
        .first()
        .ok_or_else(|| rejected("That plan can't be bought online."))?;

    let paid = current_paid_plan(state, user_id).await?;
    if let Some(plan) = &paid
        && config.tier_rank(&plan.tier.id) >= config.tier_rank(tier_id)
    {
        return Err(rejected(format!(
            "You already have {}. Switching to a cheaper plan happens at the end of your billing period.",
            plan.tier.name
        )));
    }

    let mut metadata = json!({ "user_id": user_id, "username": username });
    let mut promo_code = None;
    if let Some(plan) = &paid {
        metadata["replaces_membership"] = json!(plan.membership.membership_id);
        let period = (
            parse_time(plan.membership.current_period_start.as_deref()),
            parse_time(plan.membership.current_period_end.as_deref()),
        );
        if let (Some(start), Some(end)) = period
            && plan.membership.status != "trialing"
        {
            let current = client
                .plan_price(&plan.membership.plan_id)
                .await
                .map_err(|error| internal("pricing the current plan failed", error))?;
            let target = client
                .plan_price(plan_id)
                .await
                .map_err(|error| internal("pricing the new plan failed", error))?;
            let credit = credit_cents(
                current.renewal_cents(),
                start,
                end,
                Utc::now(),
                target.first_charge_cents(),
            );
            if credit >= MIN_CREDIT_CENTS {
                let code: String = rand::rng()
                    .sample_iter(&Alphanumeric)
                    .take(12)
                    .map(char::from)
                    .collect();
                let code = format!("up{}", code.to_ascii_lowercase());
                let expires = (Utc::now() + Duration::hours(PROMO_CODE_LIFETIME_HOURS)).to_rfc3339();
                let stored = client
                    .create_promo_code(&code, credit, plan_id, &expires)
                    .await
                    .map_err(|error| internal("creating the upgrade credit failed", error))?;
                metadata["credit_cents"] = json!(credit);
                promo_code = Some(stored);
            }
        }
    }

    let redirect = format!(
        "{}/billing?checkout=done",
        config.server.public_url.trim_end_matches('/')
    );
    let mut url = client
        .create_checkout(plan_id, &metadata, &redirect)
        .await
        .map_err(|error| internal("creating the Whop checkout failed", error))?;
    // Whop applies a promo code passed on the checkout link (not one set on
    // the checkout configuration).
    if let Some(code) = &promo_code {
        let separator = if url.contains('?') { '&' } else { '?' };
        url = format!("{url}{separator}promoCode={code}");
    }

    state
        .audit
        .record_best_effort(
            Some(username),
            "checkout_started",
            None,
            Some(json!({
                "tier": tier_id,
                "plan": plan_id,
                "replaces": paid.as_ref().map(|plan| &plan.membership.membership_id),
                "credit_cents": metadata["credit_cents"],
            })),
        )
        .await;
    Ok(url)
}

/// Schedules a move to a cheaper tier (or the free tier) for when the
/// current paid period ends. Returns when that is, for display.
pub async fn schedule_downgrade(
    state: &AppState,
    user_id: &str,
    username: &str,
    tier_id: &str,
) -> Result<String, AccountError> {
    let client = client(state)?;
    let config = &state.config;
    config.tier(tier_id).ok_or_else(|| rejected("Unknown plan."))?;
    let plan = current_paid_plan(state, user_id)
        .await?
        .ok_or_else(|| rejected("You don't have a paid plan to change."))?;
    if config.tier_rank(tier_id) >= config.tier_rank(&plan.tier.id) {
        return Err(rejected("That isn't a cheaper plan."));
    }

    let membership_id = &plan.membership.membership_id;
    if !plan.membership.cancel_at_period_end {
        client
            .cancel_membership(membership_id, true, &format!("Switching to {tier_id} at period end"))
            .await
            .map_err(|error| internal("scheduling the change failed", error))?;
        state
            .whop
            .set_cancel_at_period_end(membership_id, true)
            .await
            .map_err(|error| internal("recording the change failed", error))?;
    }
    state
        .whop
        .set_pending_downgrade(user_id, membership_id, tier_id)
        .await
        .map_err(|error| internal("recording the change failed", error))?;
    state
        .audit
        .record_best_effort(
            Some(username),
            "downgrade_scheduled",
            None,
            Some(json!({ "from": plan.tier.id, "to": tier_id, "membership": membership_id })),
        )
        .await;
    Ok(plan
        .membership
        .current_period_end
        .as_deref()
        .map(display_time)
        .unwrap_or_else(|| "the end of the billing period".to_owned()))
}

/// Cancels a scheduled downgrade: the current membership renews again.
pub async fn keep_current_plan(state: &AppState, user_id: &str, username: &str) -> Result<(), AccountError> {
    let client = client(state)?;
    let pending = state
        .whop
        .pending_downgrade(user_id)
        .await
        .map_err(|error| internal("loading scheduled downgrade failed", error))?
        .ok_or_else(|| rejected("There's no plan change to undo."))?;
    let still_running = state
        .whop
        .find(&pending.membership_id)
        .await
        .map_err(|error| internal("loading Whop membership failed", error))?
        .is_some_and(|membership| is_entitled(&membership.status));
    if !still_running {
        return Err(rejected("That plan has already ended; choose a plan to buy instead."));
    }
    client
        .resume_membership(&pending.membership_id)
        .await
        .map_err(|error| internal("undoing the change failed", error))?;
    state
        .whop
        .set_cancel_at_period_end(&pending.membership_id, false)
        .await
        .map_err(|error| internal("recording the change failed", error))?;
    state
        .whop
        .clear_pending_downgrade(user_id)
        .await
        .map_err(|error| internal("recording the change failed", error))?;
    state
        .audit
        .record_best_effort(Some(username), "downgrade_cancelled", None, None)
        .await;
    Ok(())
}

/// Called when one of a user's memberships is active: retires the
/// membership an upgrade replaced, and clears a scheduled downgrade once the
/// user has bought another plan. Failures are logged; the next webhook or
/// reconcile retries.
pub async fn on_membership_active(state: &AppState, user_id: &str, info: &MembershipInfo) {
    if let Some(old_id) = info.replaces_membership.as_deref().filter(|old| *old != info.id)
        && let Err(error) = retire_replaced(state, user_id, old_id, &info.id).await
    {
        log::error!("retiring membership {old_id} after upgrade to {} failed: {error}", info.id);
    }
    match state.whop.pending_downgrade(user_id).await {
        Ok(Some(pending)) if pending.membership_id != info.id && !info.cancel_at_period_end => {
            let _ = state.whop.clear_pending_downgrade(user_id).await;
        }
        Ok(_) => {}
        Err(error) => log::warn!("loading scheduled downgrade for {user_id} failed: {error}"),
    }
}

async fn retire_replaced(
    state: &AppState,
    user_id: &str,
    old_id: &str,
    new_id: &str,
) -> Result<(), String> {
    let Some(old) = state
        .whop
        .find(old_id)
        .await
        .map_err(|error| format!("loading replaced membership failed: {error}"))?
    else {
        return Ok(());
    };
    // Only ever cancel the same user's own membership, and only once.
    if old.user_id.as_deref() != Some(user_id) || !is_entitled(&old.status) {
        return Ok(());
    }
    let client = state.whop_client.as_ref().ok_or("whop is disabled")?;
    client
        .cancel_membership(old_id, false, &format!("Upgraded to membership {new_id}"))
        .await?;
    state
        .audit
        .record_best_effort(
            Some("whop"),
            "membership_replaced",
            Some(user_id),
            Some(json!({ "old": old_id, "new": new_id })),
        )
        .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, Utc};

    use super::credit_cents;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn credits_the_unused_part_of_the_period() {
        let start = at("2026-11-01T00:00:00Z");
        let end = start + Duration::days(30);
        // Half-way through a $5 period: $2.50 back.
        assert_eq!(credit_cents(500, start, end, start + Duration::days(15), 1500), 250);
        // Rounded down to the cent.
        assert_eq!(credit_cents(500, start, end, start + Duration::days(10), 1500), 333);
        // Nothing used yet / all used.
        assert_eq!(credit_cents(500, start, end, start, 1500), 500);
        assert_eq!(credit_cents(500, start, end, end + Duration::days(1), 1500), 0);
    }

    #[test]
    fn never_credits_more_than_the_new_first_payment() {
        let start = at("2026-11-01T00:00:00Z");
        let end = start + Duration::days(30);
        assert_eq!(credit_cents(2000, start, end, start, 1500), 1500);
    }

    #[test]
    fn handles_broken_periods_and_free_plans() {
        let start = at("2026-11-01T00:00:00Z");
        assert_eq!(credit_cents(500, start, start, start, 1500), 0);
        assert_eq!(credit_cents(500, start, start - Duration::days(1), start, 1500), 0);
        assert_eq!(credit_cents(0, start, start + Duration::days(30), start, 1500), 0);
    }
}
