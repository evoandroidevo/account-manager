use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;

use super::{action_error, FormError};
use crate::{
    account::{get_email_change, ConfirmEmailChange},
    auth::error_text,
};

/// Landing page for the link in the confirmation email. The change itself
/// needs a button press (a POST), so link prefetchers can't apply it.
#[component]
pub fn ConfirmEmailPage() -> impl IntoView {
    let query = use_query_map();
    let token = move || query.read().get("token").unwrap_or_default();
    let change = Resource::new(token, get_email_change);
    let confirm = ServerAction::<ConfirmEmailChange>::new();

    view! {
        <Title text="Confirm email change"/>
        <section class="card narrow">
            {move || {
                confirm
                    .value()
                    .get()
                    .and_then(Result::ok)
                    .map(|done| {
                        view! {
                            <h1>"Email updated"</h1>
                            <p>
                                "The email address for " <strong>{done.username}</strong> " is now "
                                <strong>{done.new_email}</strong> "."
                            </p>
                            <p>
                                <a href="/">"Go to your account"</a>
                            </p>
                        }
                    })
            }}
            <Show when=move || !matches!(confirm.value().get(), Some(Ok(_)))>
                <Suspense fallback=|| view! { <p class="muted">"Checking link…"</p> }>
                    {move || Suspend::new(async move {
                        match change.await {
                            Ok(Some(info)) => {
                                view! {
                                    <h1>"Confirm new email"</h1>
                                    <p>
                                        "Change the email address for " <strong>{info.username}</strong>
                                        " to " <strong>{info.new_email}</strong> "?"
                                    </p>
                                    <ActionForm action=confirm attr:class="stack">
                                        <input type="hidden" name="token" value=token()/>
                                        <FormError message=action_error(confirm.value())/>
                                        <button type="submit" disabled=move || confirm.pending().get()>
                                            "Confirm change"
                                        </button>
                                    </ActionForm>
                                }
                                    .into_any()
                            }
                            Ok(None) => {
                                view! {
                                    <h1>"Link not valid"</h1>
                                    <p class="form-error">
                                        "This confirmation link is invalid, expired, or already used. "
                                        "Request a new one from your account's email page."
                                    </p>
                                }
                                    .into_any()
                            }
                            Err(error) => {
                                view! {
                                    <h1>"Something went wrong"</h1>
                                    <p class="form-error">{error_text(&error)}</p>
                                }
                                    .into_any()
                            }
                        }
                    })}
                </Suspense>
            </Show>
        </section>
    }
}
