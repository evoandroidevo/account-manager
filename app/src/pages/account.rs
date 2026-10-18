use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::Redirect;

use crate::auth::{account_overview, error_text, AccountProfile};

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
