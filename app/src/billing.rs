//! Server functions for paid plans (Whop): the billing page and the admin
//! payments page.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use crate::server::{account_error, ctx, internal};

/// What the plans page offers for a tier, relative to the current plan.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum TierOffer {
    Current,
    /// Buy it; `charge` is the first payment, e.g. "$5.00".
    Buy { charge: String },
    /// Switch now, paying `charge` (`list` minus `credit` for the unused
    /// part of `from`).
    Upgrade { charge: String, list: String, credit: Option<String>, from: String },
    /// Switch when the current paid period ends, on `effective`.
    Downgrade { effective: String, scheduled: bool },
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BillingTier {
    pub id: String,
    pub name: String,
    pub description: String,
    pub price_label: Option<String>,
    pub offer: TierOffer,
}

/// A downgrade the user scheduled.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PendingChange {
    pub to_id: String,
    pub to_name: String,
    /// The current paid plan, while it is still running.
    pub from_name: Option<String>,
    pub effective: Option<String>,
    /// The paid plan has ended and `to` still needs buying.
    pub awaiting_purchase: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BillingInfo {
    pub tiers: Vec<BillingTier>,
    pub current_name: String,
    /// "manual", "whop" or "default".
    pub source: String,
    pub valid_until: Option<String>,
    /// Whop's page for changing or cancelling the subscription.
    pub manage_url: Option<String>,
    pub payments_enabled: bool,
    pub pending: Option<PendingChange>,
}

#[cfg(feature = "ssr")]
fn money(cents: i64) -> String {
    format!("${}.{:02}", cents / 100, cents % 100)
}

#[server]
pub async fn billing_overview() -> Result<BillingInfo, ServerFnError> {
    use backend::{
        accounts::display_time,
        plan_change::{self, PlanOption, MIN_CREDIT_CENTS},
        tiers,
    };

    let ctx = ctx()?;
    let user = ctx.require_user().await?;
    let state = &ctx.state;
    let effective = tiers::effective_tier(state, &user.user_id)
        .await
        .map_err(account_error)?;
    let options = plan_change::plan_options(state, &user.user_id, &effective.tier.id)
        .await
        .map_err(account_error)?;
    let pending_to = options.pending.as_ref().map(|pending| pending.to_tier.clone());

    let pending = options.pending.as_ref().and_then(|pending| {
        let to = state.config.tier(&pending.to_tier)?;
        let running = options
            .paid
            .as_ref()
            .filter(|plan| plan.membership.membership_id == pending.membership_id);
        Some(PendingChange {
            to_id: to.id.clone(),
            to_name: to.name.clone(),
            from_name: running.map(|plan| plan.tier.name.clone()),
            effective: running
                .and_then(|plan| plan.membership.current_period_end.as_deref())
                .map(display_time),
            awaiting_purchase: running.is_none(),
        })
    });

    Ok(BillingInfo {
        tiers: options
            .tiers
            .into_iter()
            .map(|(tier, option)| {
                let offer = match option {
                    PlanOption::Current => TierOffer::Current,
                    PlanOption::Buy { charge_cents } => TierOffer::Buy { charge: money(charge_cents) },
                    PlanOption::Upgrade { list_cents, credit_cents, from_tier } => {
                        let credit = (credit_cents >= MIN_CREDIT_CENTS).then_some(credit_cents);
                        TierOffer::Upgrade {
                            charge: money(list_cents - credit.unwrap_or(0)),
                            list: money(list_cents),
                            credit: credit.map(money),
                            from: from_tier,
                        }
                    }
                    PlanOption::Downgrade { effective } => TierOffer::Downgrade {
                        effective,
                        scheduled: pending_to.as_deref() == Some(tier.id.as_str()),
                    },
                    PlanOption::Unavailable => TierOffer::Unavailable,
                };
                BillingTier {
                    id: tier.id,
                    name: tier.name,
                    description: tier.description,
                    price_label: tier.price_label,
                    offer,
                }
            })
            .collect(),
        current_name: effective.tier.name,
        source: effective.source,
        valid_until: effective.valid_until.as_deref().map(display_time),
        manage_url: options.paid.and_then(|plan| plan.membership.manage_url),
        payments_enabled: state.whop_client.is_some(),
        pending,
    })
}

/// Creates a Whop checkout for the tier (with any upgrade credit applied)
/// and returns its URL; the page then sends the browser there.
#[server]
pub async fn start_checkout(tier_id: String) -> Result<String, ServerFnError> {
    let ctx = ctx()?;
    let user = ctx.require_user().await?;
    backend::plan_change::start_checkout(&ctx.state, &user.user_id, &user.username, &tier_id)
        .await
        .map_err(account_error)
}

/// Moves to a cheaper plan when the current paid period ends.
#[server]
pub async fn schedule_downgrade(tier_id: String) -> Result<String, ServerFnError> {
    let ctx = ctx()?;
    let user = ctx.require_user().await?;
    backend::plan_change::schedule_downgrade(&ctx.state, &user.user_id, &user.username, &tier_id)
        .await
        .map_err(account_error)
}

/// Undoes a scheduled downgrade.
#[server]
pub async fn keep_current_plan() -> Result<(), ServerFnError> {
    let ctx = ctx()?;
    let user = ctx.require_user().await?;
    backend::plan_change::keep_current_plan(&ctx.state, &user.user_id, &user.username)
        .await
        .map_err(account_error)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UnmatchedMembership {
    pub membership_id: String,
    pub tier_name: String,
    pub email: Option<String>,
    pub status: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WebhookRow {
    pub received_at: String,
    pub event_type: String,
    pub outcome: String,
    pub membership_id: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PaymentsInfo {
    pub payments_enabled: bool,
    pub unmatched: Vec<UnmatchedMembership>,
    pub events: Vec<WebhookRow>,
}

#[server]
pub async fn payments_overview() -> Result<PaymentsInfo, ServerFnError> {
    use backend::accounts::display_time;

    let ctx = ctx()?;
    ctx.require_admin().await?;
    let state = &ctx.state;
    let unmatched = state
        .whop
        .unmatched()
        .await
        .map_err(|error| internal(format!("loading memberships failed: {error}")))?;
    let events = state
        .whop
        .recent_events(50)
        .await
        .map_err(|error| internal(format!("loading webhook events failed: {error}")))?;

    Ok(PaymentsInfo {
        payments_enabled: state.whop_client.is_some(),
        unmatched: unmatched
            .into_iter()
            .map(|membership| UnmatchedMembership {
                tier_name: state
                    .config
                    .tier_for_whop_plan(&membership.plan_id)
                    .map_or_else(|| format!("unknown plan {}", membership.plan_id), |tier| tier.name.clone()),
                email: membership.email,
                status: membership.status,
                updated_at: display_time(&membership.updated_at),
                membership_id: membership.membership_id,
            })
            .collect(),
        events: events
            .into_iter()
            .map(|event| WebhookRow {
                received_at: display_time(&event.received_at),
                event_type: event.event_type,
                outcome: event.outcome,
                membership_id: event.membership_id,
                detail: event.detail,
            })
            .collect(),
    })
}

/// Links an unmatched Whop membership to an account and applies it.
#[server]
pub async fn assign_membership(membership_id: String, username: String) -> Result<(), ServerFnError> {
    let ctx = ctx()?;
    let admin = ctx.require_admin().await?;
    backend::whop::assign_membership(&ctx.state, &membership_id, &username, &admin.username)
        .await
        .map_err(account_error)
}
