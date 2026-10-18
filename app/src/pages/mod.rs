pub mod account;
pub mod admin_invites;
pub mod admin_users;
pub mod confirm_email;
pub mod email;
pub mod invite;
pub mod layout;
pub mod login;
pub mod security;

use leptos::prelude::*;

use crate::auth::error_text;

/// The error from an action's last run, as display text.
pub fn action_error<T, V>(value: V) -> Signal<Option<String>>
where
    V: With<Value = Option<Result<T, ServerFnError>>> + Send + Sync + 'static,
{
    Signal::derive(move || {
        value.with(|result| {
            result
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .map(error_text)
        })
    })
}

/// Whether an action's last run succeeded.
pub fn action_succeeded<T, V>(value: V) -> Signal<bool>
where
    V: With<Value = Option<Result<T, ServerFnError>>> + Send + Sync + 'static,
{
    Signal::derive(move || value.with(|result| matches!(result, Some(Ok(_)))))
}

#[component]
pub fn FormError(#[prop(into)] message: Signal<Option<String>>) -> impl IntoView {
    move || {
        message
            .get()
            .map(|message| view! { <p class="form-error" role="alert">{message}</p> })
    }
}

#[component]
pub fn FormSuccess(#[prop(into)] message: Signal<Option<String>>) -> impl IntoView {
    move || {
        message
            .get()
            .map(|message| view! { <p class="form-success" role="status">{message}</p> })
    }
}
