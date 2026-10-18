use leptos::{html, prelude::*};
use leptos_meta::Title;

use super::{action_error, FormError, FormSuccess};
use crate::{account::RequestEmailChange, auth::account_overview};

#[component]
pub fn EmailPage() -> impl IntoView {
    let action = ServerAction::<RequestEmailChange>::new();
    let profile = Resource::new(|| (), |_| account_overview());
    let form = NodeRef::<html::Form>::new();
    let sent = Signal::derive(move || action.value().get().and_then(Result::ok));

    Effect::new(move |_| {
        if sent.get().is_some() {
            if let Some(form) = form.get() {
                form.reset();
            }
        }
    });

    view! {
        <Title text="Change email"/>
        <section class="card">
            <h1>"Change email"</h1>
            <Suspense>
                {move || {
                    profile
                        .get()
                        .and_then(Result::ok)
                        .flatten()
                        .map(|profile| {
                            view! {
                                <p class="muted">
                                    "Your current address is " <strong>{profile.email}</strong> "."
                                </p>
                            }
                        })
                }}
            </Suspense>
            <p class="muted">
                "We'll email a confirmation link to the new address. "
                "Nothing changes until you open it."
            </p>
            <ActionForm action node_ref=form attr:class="stack">
                <label>
                    "New email address"
                    <input type="email" name="new_email" autocomplete="email" required/>
                </label>
                <label>
                    "Current password"
                    <input
                        type="password"
                        name="current_password"
                        autocomplete="current-password"
                        required
                    />
                </label>
                <FormError message=action_error(action.value())/>
                <FormSuccess message=sent/>
                <button type="submit" disabled=move || action.pending().get()>
                    "Send confirmation link"
                </button>
            </ActionForm>
        </section>
    }
}
