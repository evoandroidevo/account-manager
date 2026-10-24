use leptos::prelude::*;
use leptos_meta::Title;

use super::{action_error, FormError};
use crate::{
    auth::error_text,
    billing::{payments_overview, AssignMembership, PaymentsInfo},
};

/// Admin page: Whop memberships that couldn't be matched to an account, and
/// recent webhook deliveries.
#[component]
pub fn AdminPaymentsPage() -> impl IntoView {
    let assign = ServerAction::<AssignMembership>::new();
    let payments = Resource::new(move || assign.version().get(), |_| payments_overview());

    view! {
        <Title text="Payments"/>
        <Transition fallback=|| view! { <p class="muted">"Loading…"</p> }>
            {move || Suspend::new(async move {
                match payments.await {
                    Ok(info) => view! { <PaymentsView info assign/> }.into_any(),
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

#[component]
fn PaymentsView(info: PaymentsInfo, assign: ServerAction<AssignMembership>) -> impl IntoView {
    let PaymentsInfo { payments_enabled, unmatched, events } = info;

    let unmatched_view = if unmatched.is_empty() {
        view! { <p class="muted">"Every membership is matched to an account."</p> }.into_any()
    } else {
        let rows = unmatched
            .into_iter()
            .map(|membership| {
                let row_id = membership.membership_id.clone();
                view! {
                    <tr data-membership=row_id>
                        <td>{membership.tier_name}</td>
                        <td>{membership.email.unwrap_or_else(|| "no email".to_owned())}</td>
                        <td>{membership.status}</td>
                        <td>
                            <ActionForm action=assign attr:class="inline-form">
                                <input
                                    type="hidden"
                                    name="membership_id"
                                    value=membership.membership_id
                                />
                                <input
                                    name="username"
                                    placeholder="username"
                                    aria-label="Username to assign"
                                    required
                                />
                                <button type="submit">"Assign"</button>
                            </ActionForm>
                        </td>
                    </tr>
                }
            })
            .collect_view();
        view! {
            <div class="table-wrap">
                <table class="users">
                    <thead>
                        <tr>
                            <th>"Plan"</th>
                            <th>"Whop email"</th>
                            <th>"Status"</th>
                            <th>"Assign to"</th>
                        </tr>
                    </thead>
                    <tbody>{rows}</tbody>
                </table>
            </div>
        }
            .into_any()
    };

    let events_view = if events.is_empty() {
        view! { <p class="muted">"No webhooks received yet."</p> }.into_any()
    } else {
        let rows = events
            .into_iter()
            .map(|event| {
                let outcome_class = format!("status status-{}", event.outcome);
                view! {
                    <tr>
                        <td>{event.received_at}</td>
                        <td>{event.event_type}</td>
                        <td>
                            <span
                                class=outcome_class
                                title=event.detail.unwrap_or_default()
                            >
                                {event.outcome}
                            </span>
                        </td>
                        <td class="muted">{event.membership_id.unwrap_or_default()}</td>
                    </tr>
                }
            })
            .collect_view();
        view! {
            <div class="table-wrap">
                <table class="users" data-testid="webhook-events">
                    <thead>
                        <tr>
                            <th>"Received"</th>
                            <th>"Event"</th>
                            <th>"Outcome"</th>
                            <th>"Membership"</th>
                        </tr>
                    </thead>
                    <tbody>{rows}</tbody>
                </table>
            </div>
        }
            .into_any()
    };

    view! {
        <section class="card">
            <h1>"Unmatched payments"</h1>
            {(!payments_enabled)
                .then(|| view! { <p class="muted">"Whop payments are turned off in the config."</p> })}
            <p class="muted">
                "Memberships bought without the account's checkout link and with an email "
                "that matches no account. Assign each to its account to apply the plan."
            </p>
            <FormError message=action_error(assign.value())/>
            {unmatched_view}
        </section>
        <section class="card">
            <h1>"Recent webhooks"</h1>
            {events_view}
        </section>
    }
}
