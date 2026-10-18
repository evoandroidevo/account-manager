use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;

use super::{action_error, FormError};
use crate::{
    auth::{error_text, Logout},
    invites::{get_invite, InviteInfo, Signup},
};

/// Signup page reached from an invite link: /invite?code=...
#[component]
pub fn InvitePage() -> impl IntoView {
    let query = use_query_map();
    let code = move || query.read().get("code").unwrap_or_default();
    let logout = ServerAction::<Logout>::new();
    // Re-check after logging out here, since the URL doesn't change.
    let invite = Resource::new(
        move || (code(), logout.version().get()),
        |(code, _)| get_invite(code),
    );

    view! {
        <Title text="Create your account"/>
        <section class="card narrow">
            <Suspense fallback=|| view! { <p class="muted">"Checking invitation…"</p> }>
                {move || Suspend::new(async move {
                    match invite.await {
                        Ok(Some(InviteInfo { logged_in_as: Some(username), .. })) => {
                            view! { <LoggedInNotice username code=code() logout/> }.into_any()
                        }
                        Ok(Some(info)) => view! { <SignupForm code=code() info/> }.into_any(),
                        Ok(None) => {
                            view! {
                                <h1>"Invitation not valid"</h1>
                                <p class="form-error">
                                    "This invitation link is invalid, has expired, or was already used. "
                                    "Ask whoever sent it for a new one."
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
        </section>
    }
}

/// Shown instead of the form when someone is logged in, since signing up
/// would switch this browser to the new account.
#[component]
fn LoggedInNotice(username: String, code: String, logout: ServerAction<Logout>) -> impl IntoView {
    view! {
        <h1>"You're already logged in"</h1>
        <p>
            "You're logged in as " <strong>{username}</strong> ". This invitation creates a new "
            "account, so log out first to use it."
        </p>
        <ActionForm action=logout attr:class="stack">
            <input type="hidden" name="next" value=format!("/invite?code={code}")/>
            <button type="submit" disabled=move || logout.pending().get()>
                "Log out and continue"
            </button>
        </ActionForm>
        <p>
            <a href="/">"Back to my account"</a>
        </p>
    }
}

#[component]
fn SignupForm(code: String, info: InviteInfo) -> impl IntoView {
    let signup = ServerAction::<Signup>::new();

    view! {
        <h1>"Create your account"</h1>
        <p class="muted">
            "One account for every service here. This invitation expires " {info.expires_at} "."
        </p>
        <ActionForm action=signup attr:class="stack">
            <input type="hidden" name="code" value=code/>
            <label>
                "Username"
                <input
                    name="username"
                    autocomplete="username"
                    required
                    minlength="3"
                    maxlength="32"
                    pattern="[A-Za-z][A-Za-z0-9._\\-]*"
                    aria-describedby="username-hint"
                />
            </label>
            <small id="username-hint" class="muted hint">
                "3–32 characters: letters, numbers, periods, hyphens and underscores, starting with a letter."
            </small>
            <label>
                "Email"
                <input type="email" name="email" autocomplete="email" required/>
            </label>
            <div class="row">
                <label>
                    "First name"
                    <input name="first_name" autocomplete="given-name" required maxlength="64"/>
                </label>
                <label>
                    "Last name"
                    <input name="last_name" autocomplete="family-name" maxlength="64"/>
                </label>
            </div>
            <label>
                "Password"
                <input
                    type="password"
                    name="password"
                    autocomplete="new-password"
                    required
                    aria-describedby="password-hint"
                />
            </label>
            <small id="password-hint" class="muted hint">
                {info.password_hint}
            </small>
            <label>
                "Confirm password"
                <input type="password" name="password_confirm" autocomplete="new-password" required/>
            </label>
            <FormError message=action_error(signup.value())/>
            <button type="submit" disabled=move || signup.pending().get()>
                {move || if signup.pending().get() { "Creating account…" } else { "Create account" }}
            </button>
        </ActionForm>
    }
}
