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

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rand::{Rng, distr::Alphanumeric};
use serde_json::json;

use crate::{
    AppState,
    accounts::{AccountError, display_time},
    config::TierConfig,
    repo::whop::{PendingDowngrade, UpgradeCredit, WhopMembership},
    whop::{MembershipInfo, WhopClient, is_entitled},
};

/// Credits below this aren't worth a promo code.
pub const MIN_CREDIT_CENTS: i64 = 50;
/// How long an upgrade's promo code stays usable.
const PROMO_CODE_LIFETIME_HOURS: i64 = 2;
/// A code closer to expiry than this isn't reused, so it can't run out while
/// the user is on the checkout page.
const PROMO_CODE_REUSE_MARGIN_MINUTES: i64 = 30;
/// Promo codes one upgrade (user, current membership, target plan) can be
/// issued as. Whop can't tie a code to a buyer, so this bounds what handing
/// codes to other people can cost; past it, the upgrade is at full price.
const MAX_PROMO_CODES_PER_UPGRADE: i64 = 3;
/// Checkouts a user can start per `CHECKOUT_WINDOW`.
pub const CHECKOUT_MAX_ATTEMPTS: usize = 10;
pub const CHECKOUT_WINDOW: std::time::Duration = std::time::Duration::from_secs(10 * 60);

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
    /// `credit_used_up`: there would be a credit, but it was already spent or
    /// its promo codes ran out, so this is the full price.
    Upgrade { list_cents: i64, credit_cents: i64, from_tier: String, credit_used_up: bool },
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
                            (Some(plan), Some((renewal, start, end))) => {
                                let credit = credit_cents(renewal, start, end, Utc::now(), list);
                                let issued = state
                                    .whop
                                    .upgrade_credit(user_id, &plan.membership.membership_id, plan_id)
                                    .await
                                    .map_err(|error| internal("loading the upgrade credit failed", error))?;
                                let used_up = credit >= MIN_CREDIT_CENTS
                                    && issued.is_some_and(|issued| credit_used_up(&issued, Utc::now()));
                                PlanOption::Upgrade {
                                    list_cents: list,
                                    credit_cents: if used_up { 0 } else { credit },
                                    from_tier: plan.tier.name.clone(),
                                    credit_used_up: used_up,
                                }
                            }
                            (Some(plan), None) => PlanOption::Upgrade {
                                list_cents: list,
                                credit_cents: 0,
                                from_tier: plan.tier.name.clone(),
                                credit_used_up: false,
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

    let throttle_key = [format!("checkout:{user_id}")];
    if state.checkout_throttle.is_blocked(&throttle_key) {
        return Err(rejected("Too many checkout attempts. Try again in a few minutes."));
    }
    state.checkout_throttle.record_failure(&throttle_key);

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
            if credit >= MIN_CREDIT_CENTS
                && let Some(issued) =
                    upgrade_promo_code(state, client, user_id, &plan.membership.membership_id, plan_id, credit)
                        .await?
            {
                metadata["credit_cents"] = json!(issued.credit_cents);
                promo_code = Some(issued.promo_code);
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

/// Whether an upgrade can no longer get a discount: its credit was spent, or
/// the last code it's allowed has expired.
fn credit_used_up(issued: &UpgradeCredit, now: DateTime<Utc>) -> bool {
    issued.redeemed_by_membership.is_some()
        || (issued.issued_count >= MAX_PROMO_CODES_PER_UPGRADE
            && parse_time(Some(&issued.expires_at)).is_none_or(|expires| expires <= now))
}

/// The promo code carrying an upgrade's credit: the one already issued while
/// it has time left, otherwise a new one worth `credit`. None once the credit
/// was spent or its codes ran out.
async fn upgrade_promo_code(
    state: &AppState,
    client: &WhopClient,
    user_id: &str,
    from_membership_id: &str,
    plan_id: &str,
    credit: i64,
) -> Result<Option<UpgradeCredit>, AccountError> {
    let code: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(12)
        .map(char::from)
        .collect();
    let now = Utc::now();
    let timestamp = |time: DateTime<Utc>| time.to_rfc3339_opts(SecondsFormat::Secs, true);
    let new_credit = UpgradeCredit {
        user_id: user_id.to_owned(),
        from_membership_id: from_membership_id.to_owned(),
        to_plan_id: plan_id.to_owned(),
        promo_code: format!("up{}", code.to_ascii_lowercase()),
        credit_cents: credit,
        expires_at: timestamp(now + Duration::hours(PROMO_CODE_LIFETIME_HOURS)),
        redeemed_by_membership: None,
        issued_count: 1,
    };
    let reuse_until = timestamp(now + Duration::minutes(PROMO_CODE_REUSE_MARGIN_MINUTES));
    let (issued, claimed) = state
        .whop
        .claim_upgrade_credit(&new_credit, &reuse_until, MAX_PROMO_CODES_PER_UPGRADE)
        .await
        .map_err(|error| internal("recording the upgrade credit failed", error))?;
    if credit_used_up(&issued, now) {
        return Ok(None);
    }
    if claimed
        && let Err(error) = client
            .create_promo_code(&issued.promo_code, issued.credit_cents, plan_id, &issued.expires_at)
            .await
    {
        if let Err(error) = state.whop.release_upgrade_credit(&issued).await {
            log::warn!("releasing the upgrade credit failed: {error}");
        }
        return Err(internal("creating the upgrade credit failed", error));
    }
    Ok(Some(issued))
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
/// memberships it replaces, marks the upgrade credit towards it as used, and
/// clears a scheduled downgrade once the user has bought another plan.
/// Failures are logged; the next webhook or reconcile retries.
pub async fn on_membership_active(state: &AppState, user_id: &str, info: &MembershipInfo) {
    for old_id in replaced_memberships(state, user_id, info).await {
        if let Err(error) = retire_replaced(state, user_id, &old_id, &info.id).await {
            log::error!("retiring membership {old_id} after upgrade to {} failed: {error}", info.id);
        }
    }
    if let Err(error) = state.whop.redeem_upgrade_credits(user_id, &info.plan_id, &info.id).await {
        log::warn!("marking upgrade credits used by {} failed: {error}", info.id);
    }
    match state.whop.pending_downgrade(user_id).await {
        Ok(Some(pending)) if pending.membership_id != info.id && !info.cancel_at_period_end => {
            let _ = state.whop.clear_pending_downgrade(user_id).await;
        }
        Ok(_) => {}
        Err(error) => log::warn!("loading scheduled downgrade for {user_id} failed: {error}"),
    }
}

/// The memberships `info` replaces: the one its checkout named, plus the
/// user's other paid memberships on a lower tier, however `info` was bought
/// (a purchase outside our checkout carries no metadata). A trial or a plan
/// that's already ending doesn't replace anything by tier: the user would
/// lose the plan they keep paying for once it stops.
async fn replaced_memberships(state: &AppState, user_id: &str, info: &MembershipInfo) -> Vec<String> {
    let mut replaced: Vec<String> = info
        .replaces_membership
        .iter()
        .filter(|old| **old != info.id)
        .cloned()
        .collect();
    let config = &state.config;
    let new_tier = config.tier_for_whop_plan(&info.plan_id);
    let Some(new_tier) = new_tier.filter(|_| info.status == "active" && !info.cancel_at_period_end) else {
        return replaced;
    };
    let new_rank = config.tier_rank(&new_tier.id);
    match state.whop.for_user(user_id).await {
        Ok(memberships) => {
            for membership in memberships {
                let lower = config
                    .tier_for_whop_plan(&membership.plan_id)
                    .is_some_and(|tier| config.tier_rank(&tier.id) < new_rank);
                if lower
                    && membership.membership_id != info.id
                    && is_entitled(&membership.status)
                    && !replaced.contains(&membership.membership_id)
                {
                    replaced.push(membership.membership_id);
                }
            }
        }
        Err(error) => log::warn!("loading memberships for {user_id} failed: {error}"),
    }
    replaced
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

    mod upgrades {
        use chrono::{Duration, Utc};
        use serde_json::json;
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };

        use crate::{
            AppState,
            config::{AppConfig, TierConfig, WhopConfig},
            plan_change::{PlanOption, on_membership_active, plan_options, start_checkout},
            repo::whop::WhopMembership,
            whop::MembershipInfo,
        };

        fn tier(id: &str, plans: &[&str]) -> TierConfig {
            TierConfig {
                id: id.to_owned(),
                name: id.to_owned(),
                description: String::new(),
                price_label: None,
                lldap_groups: Vec::new(),
                whop_plan_ids: plans.iter().map(|plan| (*plan).to_owned()).collect(),
                jellyfin: None,
                seerr: None,
            }
        }

        /// A user paying for Plus, half-way through a 30-day period.
        async fn state(server: &MockServer) -> (AppState, String, std::path::PathBuf) {
            let path = std::env::temp_dir().join(format!("am-upgrade-{}.db", uuid::Uuid::new_v4()));
            let config = AppConfig {
                database: crate::config::DatabaseConfig {
                    url: format!("sqlite://{}", path.display()),
                },
                tiers: vec![
                    tier("free", &[]),
                    tier("plus", &["plan_plus"]),
                    tier("premium", &["plan_premium"]),
                ],
                whop: WhopConfig {
                    enabled: true,
                    api_url: server.uri(),
                    api_key: "key".to_owned(),
                    account_id: "biz_1".to_owned(),
                    webhook_secret: "ws_x".to_owned(),
                    grace_days: 3,
                },
                ..AppConfig::default()
            };
            let state = AppState::initialize(config).await.expect("state should initialize");
            let user = state.users.ensure("alice").await.unwrap();
            let now = Utc::now();
            state
                .whop
                .upsert(&WhopMembership {
                    membership_id: "mem_1".to_owned(),
                    user_id: Some(user.id.clone()),
                    whop_user_id: Some("user_1".to_owned()),
                    email: None,
                    plan_id: "plan_plus".to_owned(),
                    status: "active".to_owned(),
                    current_period_start: Some((now - Duration::days(15)).to_rfc3339()),
                    current_period_end: Some((now + Duration::days(15)).to_rfc3339()),
                    cancel_at_period_end: false,
                    manage_url: None,
                    updated_at: String::new(),
                })
                .await
                .unwrap();
            (state, user.id, path)
        }

        async fn mock_prices_and_checkout(server: &MockServer) {
            for (plan, price) in [("plan_plus", 5.0), ("plan_premium", 15.0)] {
                Mock::given(method("GET"))
                    .and(path(format!("/plans/{plan}")))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                        "id": plan, "initial_price": 0.0, "renewal_price": price
                    })))
                    .mount(server)
                    .await;
            }
            Mock::given(method("POST"))
                .and(path("/checkout_configurations"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "id": "ch_1", "purchase_url": "https://whop.com/checkout/ch_1/"
                })))
                .mount(server)
                .await;
        }

        /// Whop's answer to creating a promo code, expected `times` times.
        async fn mock_promo_codes(server: &MockServer, times: u64) {
            Mock::given(method("POST"))
                .and(path("/promo_codes"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": "ignored" })))
                .expect(times)
                .mount(server)
                .await;
        }

        /// Whop's answer to cancelling mem_1, expected `times` times.
        async fn mock_cancel(server: &MockServer, times: u64) {
            Mock::given(method("POST"))
                .and(path("/memberships/mem_1/cancel"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
                .expect(times)
                .mount(server)
                .await;
        }

        fn premium(status: &str, cancel_at_period_end: bool) -> MembershipInfo {
            MembershipInfo {
                id: "mem_2".to_owned(),
                plan_id: "plan_premium".to_owned(),
                status: status.to_owned(),
                whop_user_id: Some("user_1".to_owned()),
                email: None,
                metadata_user_id: None,
                replaces_membership: None,
                current_period_start: None,
                current_period_end: None,
                cancel_at_period_end,
                manage_url: None,
            }
        }

        #[tokio::test]
        async fn repeated_upgrades_reuse_one_promo_code() {
            let server = MockServer::start().await;
            mock_prices_and_checkout(&server).await;
            mock_promo_codes(&server, 1).await;
            let (state, user_id, db) = state(&server).await;

            let first = start_checkout(&state, &user_id, "alice", "premium").await.unwrap();
            let second = start_checkout(&state, &user_id, "alice", "premium").await.unwrap();
            assert!(first.contains("promoCode=up"), "{first}");
            assert_eq!(first, second);

            let _ = std::fs::remove_file(db);
        }

        #[tokio::test]
        async fn concurrent_upgrades_mint_one_promo_code() {
            let server = MockServer::start().await;
            mock_prices_and_checkout(&server).await;
            mock_promo_codes(&server, 1).await;
            let (state, user_id, db) = state(&server).await;

            let (first, second) = tokio::join!(
                start_checkout(&state, &user_id, "alice", "premium"),
                start_checkout(&state, &user_id, "alice", "premium"),
            );
            assert_eq!(first.unwrap(), second.unwrap());

            let _ = std::fs::remove_file(db);
        }

        #[tokio::test]
        async fn a_redeemed_credit_is_not_issued_again() {
            let server = MockServer::start().await;
            mock_prices_and_checkout(&server).await;
            mock_promo_codes(&server, 1).await;
            let (state, user_id, db) = state(&server).await;

            start_checkout(&state, &user_id, "alice", "premium").await.unwrap();
            state.whop.redeem_upgrade_credits(&user_id, "plan_premium", "mem_2").await.unwrap();
            let url = start_checkout(&state, &user_id, "alice", "premium").await.unwrap();
            assert!(!url.contains("promoCode"), "{url}");

            let _ = std::fs::remove_file(db);
        }

        /// Makes the upgrade's current code expire `minutes` from now.
        async fn set_expiry(state: &AppState, minutes: i64) {
            let expires = (Utc::now() + Duration::minutes(minutes))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            sqlx::query("UPDATE upgrade_credits SET expires_at = ?")
                .bind(expires)
                .execute(&state.pool)
                .await
                .unwrap();
        }

        fn promo_code(url: &str) -> Option<&str> {
            url.split("promoCode=").nth(1)
        }

        #[tokio::test]
        async fn promo_codes_run_out_and_the_upgrade_is_then_full_price() {
            let server = MockServer::start().await;
            mock_prices_and_checkout(&server).await;
            mock_promo_codes(&server, 3).await;
            let (state, user_id, db) = state(&server).await;
            let checkout = || start_checkout(&state, &user_id, "alice", "premium");

            // Each expired code is replaced, up to the limit.
            let first = checkout().await.unwrap();
            set_expiry(&state, -1).await;
            let second = checkout().await.unwrap();
            set_expiry(&state, -1).await;
            let third = checkout().await.unwrap();
            assert_ne!(promo_code(&first), promo_code(&second));
            assert_ne!(promo_code(&second), promo_code(&third));

            // The last code is still used close to its expiry, as there's
            // no other.
            set_expiry(&state, 10).await;
            assert_eq!(promo_code(&checkout().await.unwrap()), promo_code(&third));

            // Once it expires there's no discount, and the page says so.
            set_expiry(&state, -1).await;
            assert_eq!(promo_code(&checkout().await.unwrap()), None);
            let options = plan_options(&state, &user_id, "plus").await.unwrap();
            let premium = options.tiers.iter().find(|(tier, _)| tier.id == "premium").unwrap();
            assert_eq!(
                premium.1,
                PlanOption::Upgrade {
                    list_cents: 1500,
                    credit_cents: 0,
                    from_tier: "plus".to_owned(),
                    credit_used_up: true,
                }
            );

            let _ = std::fs::remove_file(db);
        }

        #[tokio::test]
        async fn a_failed_promo_code_can_be_retried() {
            let server = MockServer::start().await;
            mock_prices_and_checkout(&server).await;
            Mock::given(method("POST"))
                .and(path("/promo_codes"))
                .respond_with(ResponseTemplate::new(500))
                .up_to_n_times(1)
                .mount(&server)
                .await;
            mock_promo_codes(&server, 1).await;
            let (state, user_id, db) = state(&server).await;

            assert!(start_checkout(&state, &user_id, "alice", "premium").await.is_err());
            let url = start_checkout(&state, &user_id, "alice", "premium").await.unwrap();
            assert!(url.contains("promoCode=up"), "{url}");

            let _ = std::fs::remove_file(db);
        }

        #[tokio::test]
        async fn checkout_is_rate_limited_per_user() {
            let server = MockServer::start().await;
            mock_prices_and_checkout(&server).await;
            mock_promo_codes(&server, 1).await;
            let (state, user_id, db) = state(&server).await;

            for _ in 0..super::super::CHECKOUT_MAX_ATTEMPTS {
                start_checkout(&state, &user_id, "alice", "premium").await.unwrap();
            }
            assert!(start_checkout(&state, &user_id, "alice", "premium").await.is_err());

            let _ = std::fs::remove_file(db);
        }

        #[tokio::test]
        async fn a_higher_plan_retires_lower_ones_without_checkout_metadata() {
            let server = MockServer::start().await;
            mock_cancel(&server, 1).await;
            let (state, user_id, db) = state(&server).await;

            on_membership_active(&state, &user_id, &premium("active", false)).await;

            let _ = std::fs::remove_file(db);
        }

        #[tokio::test]
        async fn trials_and_ending_plans_do_not_retire_lower_ones() {
            let server = MockServer::start().await;
            mock_cancel(&server, 0).await;
            let (state, user_id, db) = state(&server).await;

            on_membership_active(&state, &user_id, &premium("trialing", false)).await;
            on_membership_active(&state, &user_id, &premium("active", true)).await;

            let _ = std::fs::remove_file(db);
        }
    }
}
