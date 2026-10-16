use leptos::prelude::*;
use leptos_meta::Title;

use crate::auth::{error_text, Login};

#[component]
pub fn LoginPage() -> impl IntoView {
    let login = ServerAction::<Login>::new();
    let error = move || {
        login
            .value()
            .get()
            .and_then(|result| result.err())
            .map(|error| view! { <p class="form-error" role="alert">{error_text(&error)}</p> })
    };

    view! {
        <Title text="Log in"/>
        <section class="card narrow">
            <h1>"Log in"</h1>
            <p class="muted">"Use the same username and password as your other services."</p>
            <ActionForm action=login attr:class="stack">
                <label>
                    "Username"
                    <input name="username" autocomplete="username" required autofocus/>
                </label>
                <label>
                    "Password"
                    <input
                        type="password"
                        name="password"
                        autocomplete="current-password"
                        required
                    />
                </label>
                {error}
                <button type="submit" disabled=move || login.pending().get()>
                    {move || if login.pending().get() { "Logging in…" } else { "Log in" }}
                </button>
            </ActionForm>
        </section>
    }
}
