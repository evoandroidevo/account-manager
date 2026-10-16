use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::Redirect;

use crate::auth::{account_overview, error_text, AccountProfile, Logout};

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
                            <section class="card narrow">
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
    let logout = ServerAction::<Logout>::new();
    let AccountProfile { username, display_name, email, member_since, groups, is_admin } =
        account;

    view! {
        <header class="topbar">
            <span class="brand">"Account"</span>
            <span class="who">
                {username.clone()}
                {is_admin.then(|| view! { <span class="badge">"admin"</span> })}
            </span>
            <ActionForm action=logout>
                <button type="submit" class="link">"Log out"</button>
            </ActionForm>
        </header>

        <section class="card">
            <h1>{display_name}</h1>
            <dl class="details">
                <dt>"Username"</dt>
                <dd>{username}</dd>
                <dt>"Email"</dt>
                <dd>{email}</dd>
                <dt>"Member since"</dt>
                <dd>{member_since}</dd>
                <dt>"Groups"</dt>
                <dd>
                    <ul class="chips">
                        {groups.into_iter().map(|group| view! { <li>{group}</li> }).collect_view()}
                    </ul>
                </dd>
            </dl>
        </section>
    }
}
