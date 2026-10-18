use leptos::prelude::*;
use leptos_router::{
    components::{Outlet, Redirect, A},
    hooks::{use_location, use_navigate},
};

use crate::auth::{error_text, whoami, CurrentUser, Logout};

/// Wraps every page that needs a login: checks the session once, renders the
/// navigation, and redirects to /login when nobody is logged in.
#[component]
pub fn ProtectedLayout() -> impl IntoView {
    let user = Resource::new(|| (), |_| whoami());

    view! {
        <Suspense fallback=|| view! { <p class="muted">"Loading…"</p> }>
            {move || Suspend::new(async move {
                match user.await {
                    Ok(Some(user)) => view! { <Chrome user/> }.into_any(),
                    Ok(None) => view! { <Redirect path="/login"/> }.into_any(),
                    Err(error) => {
                        view! {
                            <section class="card narrow">
                                <h1>"Something went wrong"</h1>
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
fn Chrome(user: CurrentUser) -> impl IntoView {
    let logout = ServerAction::<Logout>::new();
    let CurrentUser { username, is_admin } = user.clone();
    provide_context(user);

    // The session is checked once when the layout loads; re-check on every
    // page change so a session ended elsewhere (e.g. by a password change on
    // another device) sends this browser to the login page.
    let location = use_location();
    let navigate = use_navigate();
    Effect::new(move |previous: Option<String>| {
        let path = location.pathname.get();
        if previous.is_some_and(|previous| previous != path) {
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                if let Ok(None) = whoami().await {
                    navigate("/login", Default::default());
                }
            });
        }
        path
    });

    view! {
        <header class="topbar">
            <span class="brand">"Account"</span>
            <nav class="nav">
                <A href="/" exact=true>
                    "Overview"
                </A>
                <A href="/security">"Password"</A>
                <A href="/email">"Email"</A>
                {is_admin.then(|| view! { <A href="/admin/invites">"Invites"</A> })}
            </nav>
            <span class="who">
                {username}
                {is_admin.then(|| view! { <span class="badge">"admin"</span> })}
            </span>
            <ActionForm action=logout>
                <button type="submit" class="link">
                    "Log out"
                </button>
            </ActionForm>
        </header>
        <Outlet/>
    }
}
