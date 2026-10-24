use std::time::Duration;

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;

use super::{action_error, FormError};
use crate::{
    auth::error_text,
    billing::{
        billing_overview, BillingInfo, BillingTier, KeepCurrentPlan, PendingChange, ScheduleDowngrade,
        StartCheckout, TierOffer,
    },
};

/// Plans page: compare tiers, upgrade through Whop, manage a subscription.
#[component]
pub fn BillingPage() -> impl IntoView {
    let query = use_query_map();
    let back_from_checkout = move || query.read().get("checkout").as_deref() == Some("done");
    let refresh = RwSignal::new(0u32);
    let actions = PlanActions {
        checkout: ServerAction::<StartCheckout>::new(),
        downgrade: ServerAction::<ScheduleDowngrade>::new(),
        keep: ServerAction::<KeepCurrentPlan>::new(),
    };
    let billing = Resource::new(
        move || (refresh.get(), actions.downgrade.version().get(), actions.keep.version().get()),
        |_| billing_overview(),
    );

    // After checkout the plan changes when Whop's webhook arrives, usually
    // within seconds; re-check for a minute.
    Effect::new(move |_| {
        if back_from_checkout() {
            for step in 1..=20u32 {
                set_timeout(move || refresh.update(|n| *n += 1), Duration::from_secs(3 * u64::from(step)));
            }
        }
    });

    view! {
        <Title text="Plans"/>
        {move || {
            back_from_checkout()
                .then(|| {
                    view! {
                        <p class="notice" role="status">
                            "Thanks! Your plan changes as soon as Whop confirms the payment."
                        </p>
                    }
                })
        }}
        <Transition fallback=|| view! { <p class="muted">"Loading…"</p> }>
            {move || Suspend::new(async move {
                match billing.await {
                    Ok(info) => view! { <BillingView info actions/> }.into_any(),
                    Err(error) => {
                        view! {
                            <section class="card">
                                <p class="form-error">{error_text(&error)}</p>
                            </section>
                        }
                            .into_any()
                    }
                }
            })}
        </Transition>
    }
}

/// The plan-changing actions, shared by the summary and the tier cards.
#[derive(Clone, Copy)]
struct PlanActions {
    checkout: ServerAction<StartCheckout>,
    downgrade: ServerAction<ScheduleDowngrade>,
    keep: ServerAction<KeepCurrentPlan>,
}

impl PlanActions {
    fn busy(self) -> bool {
        self.checkout.pending().get() || self.downgrade.pending().get() || self.keep.pending().get()
    }
}

#[component]
fn BillingView(info: BillingInfo, actions: PlanActions) -> impl IntoView {
    let PlanActions { checkout, downgrade, keep } = actions;
    // Send the browser to Whop once the checkout exists.
    Effect::new(move |_| {
        if let Some(Ok(url)) = checkout.value().get() {
            let _ = window().location().set_href(&url);
        }
    });

    let BillingInfo { tiers, current_name, source, valid_until, manage_url, payments_enabled, pending } =
        info;
    let status = match (source.as_str(), &valid_until) {
        ("whop", Some(until)) => format!("Paid through Whop, active until {until}."),
        ("whop", None) => "Paid through Whop.".to_owned(),
        ("manual", Some(until)) => format!("Set by an administrator until {until}."),
        ("manual", None) => "Set by an administrator.".to_owned(),
        _ => "The standard plan.".to_owned(),
    };
    let cards = tiers
        .into_iter()
        .map(|tier| view! { <TierCard tier actions/> })
        .collect_view();

    view! {
        <section class="card" data-testid="current-plan">
            <h1>"Your plan: " {current_name}</h1>
            <p class="muted">{status}</p>
            {pending.map(|pending| view! { <PendingNotice pending actions/> })}
            {manage_url
                .map(|url| {
                    view! {
                        <p>
                            <a href=url target="_blank" rel="noopener">
                                "Manage or cancel your subscription on Whop"
                            </a>
                        </p>
                    }
                })}
            {(!payments_enabled)
                .then(|| view! { <p class="muted">"Online payments aren't available yet."</p> })}
            <FormError message=action_error(checkout.value())/>
            <FormError message=action_error(downgrade.value())/>
            <FormError message=action_error(keep.value())/>
        </section>
        <div class="tiers">{cards}</div>
    }
}

/// A scheduled downgrade: undo it while the paid plan runs, or buy the
/// cheaper plan once it has ended.
#[component]
fn PendingNotice(pending: PendingChange, actions: PlanActions) -> impl IntoView {
    let PendingChange { to_id, to_name, from_name, effective, awaiting_purchase } = pending;
    if awaiting_purchase {
        let label = format!("Continue with {to_name}");
        return view! {
            <div class="notice" role="status" data-testid="pending-change">
                <p>{format!("Your previous plan has ended. You chose to move to {to_name}.")}</p>
                <button
                    type="button"
                    disabled=move || actions.busy()
                    on:click=move |_| {
                        actions.checkout.dispatch(StartCheckout { tier_id: to_id.clone() });
                    }
                >
                    {label}
                </button>
            </div>
        }
            .into_any();
    }
    let from_name = from_name.unwrap_or_else(|| "your plan".to_owned());
    let when = effective.unwrap_or_else(|| "the end of the billing period".to_owned());
    view! {
        <div class="notice" role="status" data-testid="pending-change">
            <p>{format!("You'll move to {to_name} on {when}. Until then you keep {from_name}.")}</p>
            <button
                type="button"
                class="secondary"
                disabled=move || actions.busy()
                on:click=move |_| {
                    actions.keep.dispatch(KeepCurrentPlan {});
                }
            >
                {format!("Keep {from_name}")}
            </button>
        </div>
    }
        .into_any()
}

#[component]
fn TierCard(tier: BillingTier, actions: PlanActions) -> impl IntoView {
    let BillingTier { id, name, description, price_label, offer } = tier;
    let current = offer == TierOffer::Current;
    let tier_id = id.clone();
    let checkout_button = move |label: String, id: String| {
        view! {
            <button
                type="button"
                disabled=move || actions.busy()
                on:click=move |_| {
                    actions.checkout.dispatch(StartCheckout { tier_id: id.clone() });
                }
            >
                {label}
            </button>
        }
    };
    let action = match offer {
        TierOffer::Current => view! { <span class="status status-ok">"Current plan"</span> }.into_any(),
        TierOffer::Buy { charge: _ } => checkout_button(format!("Get {name}"), id).into_any(),
        TierOffer::Upgrade { charge, list, credit, from } => {
            let quote = match credit {
                Some(credit) => format!("{charge} today ({list} minus {credit} for the unused part of {from})"),
                None => format!("{charge} today"),
            };
            view! {
                <p class="quote" data-testid="upgrade-quote">{quote}</p>
                {checkout_button(format!("Upgrade to {name}"), id)}
            }
                .into_any()
        }
        TierOffer::Downgrade { effective, scheduled: true } => {
            view! { <span class="status status-pending">{format!("Starts {effective}")}</span> }.into_any()
        }
        TierOffer::Downgrade { effective, scheduled: false } => {
            let label = format!("Switch to {name} on {effective}");
            view! {
                <button
                    type="button"
                    class="secondary"
                    disabled=move || actions.busy()
                    on:click=move |_| {
                        actions.downgrade.dispatch(ScheduleDowngrade { tier_id: id.clone() });
                    }
                >
                    {label}
                </button>
            }
                .into_any()
        }
        TierOffer::Unavailable => ().into_any(),
    };

    view! {
        <section class=if current { "card tier current" } else { "card tier" } data-tier=tier_id>
            <h2>{name}</h2>
            {price_label.map(|price| view! { <p class="price">{price}</p> })}
            {(!description.is_empty()).then(|| view! { <p class="muted">{description}</p> })}
            {action}
        </section>
    }
}
