use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{ParentRoute, Route, Router, Routes},
    SsrMode, StaticSegment,
};

pub mod account;
pub mod auth;
mod pages;
#[cfg(feature = "ssr")]
pub mod server;

use pages::{
    account::AccountPage, confirm_email::ConfirmEmailPage, email::EmailPage,
    layout::ProtectedLayout, login::LoginPage, security::SecurityPage,
};

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
                <Routes fallback=|| "Page not found.".into_view()>
                    // Public pages.
                    <Route path=StaticSegment("login") view=LoginPage/>
                    <Route path=StaticSegment("confirm-email") view=ConfirmEmailPage/>

                    // Pages behind a login. SsrMode::Async waits for the session
                    // check, so logged-out visitors get a real HTTP redirect.
                    <ParentRoute path=StaticSegment("") view=ProtectedLayout ssr=SsrMode::Async>
                        <Route path=StaticSegment("") view=AccountPage ssr=SsrMode::Async/>
                        <Route path=StaticSegment("security") view=SecurityPage ssr=SsrMode::Async/>
                        <Route path=StaticSegment("email") view=EmailPage ssr=SsrMode::Async/>
                    </ParentRoute>
                </Routes>
            </main>
        </Router>
    }
}
