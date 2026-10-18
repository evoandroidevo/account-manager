use leptos::{html, prelude::*};
use leptos_meta::Title;

use super::{action_error, action_succeeded, FormError, FormSuccess};
use crate::account::{password_policy_hint, ChangePassword};

#[component]
pub fn SecurityPage() -> impl IntoView {
    let action = ServerAction::<ChangePassword>::new();
    let hint = Resource::new(|| (), |_| password_policy_hint());
    let form = NodeRef::<html::Form>::new();
    let succeeded = action_succeeded(action.value());

    // Clear the password fields once the change went through.
    Effect::new(move |_| {
        if succeeded.get() {
            if let Some(form) = form.get() {
                form.reset();
            }
        }
    });

    view! {
        <Title text="Change password"/>
        <section class="card">
            <h1>"Change password"</h1>
            <p class="muted">
                "This is the password for every service you sign in to with this account. "
                "Changing it signs you out of this site on your other devices. Apps you're "
                "already signed in to, like Jellyfin, may keep you signed in until you sign "
                "out of them."
            </p>
            <ActionForm action node_ref=form attr:class="stack">
                <label>
                    "Current password"
                    <input
                        type="password"
                        name="current_password"
                        autocomplete="current-password"
                        required
                    />
                </label>
                <label>
                    "New password"
                    <input
                        type="password"
                        name="new_password"
                        autocomplete="new-password"
                        required
                        aria-describedby="password-hint"
                    />
                </label>
                <small id="password-hint" class="muted hint">
                    <Suspense>{move || hint.get().and_then(Result::ok)}</Suspense>
                </small>
                <label>
                    "Confirm new password"
                    <input
                        type="password"
                        name="new_password_confirm"
                        autocomplete="new-password"
                        required
                    />
                </label>
                <FormError message=action_error(action.value())/>
                <FormSuccess message=Signal::derive(move || {
                    succeeded
                        .get()
                        .then(|| "Password changed. You've been signed out of this site on your other devices.".to_owned())
                })/>
                <button type="submit" disabled=move || action.pending().get()>
                    "Change password"
                </button>
            </ActionForm>
        </section>
    }
}
