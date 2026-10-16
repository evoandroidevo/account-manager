use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    SsrMode, StaticSegment,
};

pub mod auth;
mod pages;
#[cfg(feature = "ssr")]
pub mod server;

use pages::{account::AccountPage, login::LoginPage};

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    // Provides context that manages stylesheets, titles, meta tags, etc.
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/account-manager.css"/>
        <Title text="Account"/>

        <Router>
            <main>
                // Protected pages render with SsrMode::Async so a logged-out
                // visitor gets a real HTTP redirect before any HTML is sent.
                <Routes fallback=|| "Page not found.".into_view()>
                    <Route path=StaticSegment("") view=AccountPage ssr=SsrMode::Async/>
                    <Route path=StaticSegment("login") view=LoginPage/>
                </Routes>
            </main>
        </Router>
    }
}
