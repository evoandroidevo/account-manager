use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::Redirect;

use crate::{
    auth::{account_overview, error_text, AccountProfile},
    plans::{my_plan, PlanInfo, ServiceHealth},
};

#[component]
pub fn AccountPage() -> impl IntoView {
    let overview = Resource::new(|| (), |_| account_overview());

    view! {
        <Title text="Your account"/>
        <Suspense fallback=|| view! { <p class="muted">"Loading…"</p> }>
            {move || Suspend::new(async move {
                match overview.await {
                    Ok(Some(account)) => view! { <AccountView account/> }.into_any(),
                    Ok(None) => view! { <Redirect path="/login"/> }.into_any(),
                    Err(error) => {
                        view! {
                            <section class="card">
                                <h1>"Account unavailable"</h1>
                                <p class="form-error">{error_text(&error)}</p>
                            </section>
                        }
                            .into_any()
                    }
                }
            })}
        </Suspense>
        <PlanCard/>
    }
}

#[component]
fn PlanCard() -> impl IntoView {
    let plan = Resource::new(|| (), |_| my_plan());

    view! {
        <Suspense>
            {move || Suspend::new(async move {
                plan.await.ok().flatten().map(|plan| view! { <PlanView plan/> })
            })}
        </Suspense>
    }
}

#[component]
fn PlanView(plan: PlanInfo) -> impl IntoView {
    let PlanInfo { tier_name, description, valid_until, services } = plan;
    let services = (!services.is_empty()).then(|| {
        let items = services
            .into_iter()
            .map(|service| {
                let (class, label) = match service.health {
                    ServiceHealth::Ready => ("status status-ok", "Ready"),
                    ServiceHealth::SettingUp => ("status status-pending", "Setting up…"),
                    ServiceHealth::Problem => ("status status-error", "Needs attention"),
                    ServiceHealth::NotManaged => ("status status-skipped", "Managed separately"),
                };
                view! {
                    <li>
                        <span>{service.name}</span>
                        <span class=class>{label}</span>
                    </li>
                }
            })
            .collect_view();
        view! { <ul class="services">{items}</ul> }
    });

    view! {
        <section class="card" data-testid="plan">
            <h2>"Your plan"</h2>
            <p class="plan-name">{tier_name}</p>
            {(!description.is_empty()).then(|| view! { <p class="muted">{description}</p> })}
            {valid_until.map(|until| view! { <p class="muted">"Ends " {until} "."</p> })}
            {services}
        </section>
    }
}

#[component]
fn AccountView(account: AccountProfile) -> impl IntoView {
    let AccountProfile { username, display_name, email, member_since, .. } = account;

    view! {
        <section class="card">
            <h1>{display_name}</h1>
            <dl class="details">
                <dt>"Username"</dt>
                <dd>{username}</dd>
                <dt>"Email"</dt>
                <dd>{email}</dd>
                <dt>"Member since"</dt>
                <dd>{member_since}</dd>
            </dl>
        </section>
    }
}
