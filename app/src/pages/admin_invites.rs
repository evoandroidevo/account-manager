use leptos::{ev::SubmitEvent, prelude::*, web_sys};
use leptos_meta::Title;

use super::{action_error, FormError};
use crate::{
    auth::error_text,
    invites::{
        get_invite_defaults, list_invites, CreateInvite, InviteDefaults, InviteSummary, RevokeInvite,
    },
};

/// Admin page: create single-use invite links and manage recent ones.
#[component]
pub fn AdminInvitesPage() -> impl IntoView {
    let create = ServerAction::<CreateInvite>::new();
    let revoke = ServerAction::<RevokeInvite>::new();
    let defaults = Resource::new(|| (), |_| get_invite_defaults());
    // Refetch the list after every create or revoke.
    let invites = Resource::new(
        move || (create.version().get(), revoke.version().get()),
        |_| list_invites(),
    );

    view! {
        <Title text="Invites"/>
        <section class="card">
            <h1>"Create an invite"</h1>
            <Suspense fallback=|| view! { <p class="muted">"Loading…"</p> }>
                {move || Suspend::new(async move {
                    match defaults.await {
                        Ok(defaults) => view! { <CreateInviteForm defaults action=create/> }.into_any(),
                        Err(error) => view! { <p class="form-error">{error_text(&error)}</p> }.into_any(),
                    }
                })}
            </Suspense>
            {move || {
                create
                    .value()
                    .get()
                    .and_then(Result::ok)
                    .map(|invite| {
                        view! {
                            <div class="created" data-testid="created-invite">
                                <label>
                                    "Invite link — copy it now, it won't be shown again"
                                    <input
                                        readonly
                                        value=invite.url
                                        on:focus=|ev| {
                                            event_target::<web_sys::HtmlInputElement>(&ev).select()
                                        }
                                    />
                                </label>
                                <p class="muted">
                                    "Single use. Groups: " {invite.groups.join(", ")} ". Expires "
                                    {invite.expires_at} "."
                                </p>
                            </div>
                        }
                    })
            }}
        </section>

        <section class="card">
            <h1>"Recent invites"</h1>
            <FormError message=action_error(revoke.value())/>
            <Transition fallback=|| view! { <p class="muted">"Loading…"</p> }>
                {move || Suspend::new(async move {
                    match invites.await {
                        Ok(list) if list.is_empty() => {
                            view! { <p class="muted">"No invites yet."</p> }.into_any()
                        }
                        Ok(list) => view! { <InviteTable list revoke/> }.into_any(),
                        Err(error) => view! { <p class="form-error">{error_text(&error)}</p> }.into_any(),
                    }
                })}
            </Transition>
        </section>
    }
}

#[component]
fn CreateInviteForm(defaults: InviteDefaults, action: ServerAction<CreateInvite>) -> impl IntoView {
    let selected = RwSignal::new(defaults.default_groups.clone());
    let hours = RwSignal::new(defaults.expiration_hours);

    let submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        action.dispatch(CreateInvite {
            groups: selected.get(),
            expiration_hours: hours.get(),
        });
    };

    let checkboxes = defaults
        .groups
        .into_iter()
        .map(|group| {
            let is_checked = {
                let group = group.clone();
                move || selected.read().contains(&group)
            };
            let toggle = {
                let group = group.clone();
                move |ev| {
                    let on = event_target_checked(&ev);
                    selected.update(|groups| {
                        groups.retain(|g| g != &group);
                        if on {
                            groups.push(group.clone());
                        }
                    });
                }
            };
            view! {
                <label class="check">
                    <input type="checkbox" prop:checked=is_checked on:change=toggle/>
                    <span>{group}</span>
                </label>
            }
        })
        .collect_view();

    view! {
        <form class="stack" on:submit=submit>
            <fieldset class="groups">
                <legend>"Groups the new account joins"</legend>
                {checkboxes}
            </fieldset>
            <label>
                "Link expires after (hours)"
                <input
                    type="number"
                    min="1"
                    max="720"
                    required
                    prop:value=move || hours.get().to_string()
                    on:input=move |ev| {
                        if let Ok(value) = event_target_value(&ev).parse() {
                            hours.set(value);
                        }
                    }
                />
            </label>
            <FormError message=action_error(action.value())/>
            <button type="submit" disabled=move || action.pending().get()>
                "Create invite"
            </button>
        </form>
    }
}

#[component]
fn InviteTable(list: Vec<InviteSummary>, revoke: ServerAction<RevokeInvite>) -> impl IntoView {
    let rows = list
        .into_iter()
        .map(|invite| {
            let InviteSummary { id, groups, created_at, expires_at, status, used_by } = invite;
            let detail = match (status.as_str(), used_by) {
                ("used", Some(user)) => format!("by {user}"),
                ("active", _) | ("expired", _) => format!("expires {expires_at}"),
                _ => String::new(),
            };
            let can_revoke = status == "active";
            let status_class = format!("status status-{status}");
            view! {
                <tr>
                    <td>
                        <span class=status_class>{status}</span>
                    </td>
                    <td>{groups.join(", ")}</td>
                    <td>{created_at}</td>
                    <td class="muted">{detail}</td>
                    <td>
                        {can_revoke
                            .then(|| {
                                view! {
                                    <button
                                        type="button"
                                        class="link danger"
                                        disabled=move || revoke.pending().get()
                                        on:click=move |_| {
                                            revoke.dispatch(RevokeInvite { id: id.clone() });
                                        }
                                    >
                                        "Revoke"
                                    </button>
                                }
                            })}
                    </td>
                </tr>
            }
        })
        .collect_view();

    view! {
        <div class="table-wrap">
            <table class="invites">
                <thead>
                    <tr>
                        <th>"Status"</th>
                        <th>"Groups"</th>
                        <th>"Created"</th>
                        <th></th>
                        <th></th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
    }
}
