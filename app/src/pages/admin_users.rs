use leptos::prelude::*;
use leptos_meta::Title;

use super::{action_error, FormError};
use crate::{
    auth::error_text,
    plans::{list_users, ClearUserTier, ResyncUser, SetUserTier, TierOption, UserRow},
};

/// Admin page: every user's plan and service status, with manual tier
/// overrides and resync.
#[component]
pub fn AdminUsersPage() -> impl IntoView {
    let set = ServerAction::<SetUserTier>::new();
    let clear = ServerAction::<ClearUserTier>::new();
    let resync = ServerAction::<ResyncUser>::new();
    let refresh = RwSignal::new(0u32);
    let page = Resource::new(
        move || {
            (
                set.version().get(),
                clear.version().get(),
                resync.version().get(),
                refresh.get(),
            )
        },
        |_| list_users(),
    );
    // Which user's editor is open.
    let editing = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        if matches!(set.value().get(), Some(Ok(()))) || matches!(clear.value().get(), Some(Ok(())))
        {
            editing.set(None);
        }
    });

    view! {
        <Title text="Users"/>
        <section class="card">
            <div class="card-header">
                <h1>"Users"</h1>
                <button type="button" class="link" on:click=move |_| refresh.update(|n| *n += 1)>
                    "Refresh"
                </button>
            </div>
            <p class="muted">
                "Plans are applied in the background: groups first, then Jellyfin, then Seerr. "
                "Hover a status to see details."
            </p>
            <FormError message=action_error(set.value())/>
            <FormError message=action_error(clear.value())/>
            <FormError message=action_error(resync.value())/>
            <Transition fallback=|| view! { <p class="muted">"Loading…"</p> }>
                {move || Suspend::new(async move {
                    match page.await {
                        Ok(page) => {
                            let tiers = page.tiers;
                            let rows = page
                                .users
                                .into_iter()
                                .map(|user| {
                                    view! {
                                        <UserRows
                                            user
                                            tiers=tiers.clone()
                                            editing
                                            set
                                            clear
                                            resync
                                        />
                                    }
                                })
                                .collect_view();
                            view! {
                                <div class="table-wrap">
                                    <table class="users">
                                        <thead>
                                            <tr>
                                                <th>"User"</th>
                                                <th>"Plan"</th>
                                                <th>"Services"</th>
                                                <th></th>
                                            </tr>
                                        </thead>
                                        <tbody>{rows}</tbody>
                                    </table>
                                </div>
                            }
                                .into_any()
                        }
                        Err(error) => view! { <p class="form-error">{error_text(&error)}</p> }.into_any(),
                    }
                })}
            </Transition>
        </section>
    }
}

#[component]
fn UserRows(
    user: UserRow,
    tiers: Vec<TierOption>,
    editing: RwSignal<Option<String>>,
    set: ServerAction<SetUserTier>,
    clear: ServerAction<ClearUserTier>,
    resync: ServerAction<ResyncUser>,
) -> impl IntoView {
    let UserRow {
        id,
        username,
        tier_id,
        tier_name,
        source,
        valid_until,
        manual_until_date,
        note,
        disabled,
        services,
    } = user;
    let plan_detail = match (source.as_str(), &valid_until) {
        ("manual", Some(until)) => format!("set by admin, ends {until}"),
        ("manual", None) => "set by admin".to_owned(),
        ("whop", Some(until)) => format!("paid, renews {until}"),
        ("whop", None) => "paid".to_owned(),
        _ => "standard".to_owned(),
    };
    let has_override = source == "manual";
    let is_open = {
        let id = id.clone();
        Signal::derive(move || editing.get().as_deref() == Some(id.as_str()))
    };
    let toggle = {
        let id = id.clone();
        move |_| {
            editing.update(|open| {
                *open = if open.as_deref() == Some(id.as_str()) {
                    None
                } else {
                    Some(id.clone())
                }
            })
        }
    };
    let badges = services
        .into_iter()
        .map(|service| {
            view! {
                <span
                    class=format!("status status-{}", service.state)
                    title=service.detail.unwrap_or_else(|| service.state.clone())
                >
                    {service.name}
                </span>
            }
        })
        .collect_view();

    let editor = {
        let id = id.clone();
        let username = username.clone();
        move || {
            if !is_open.get() {
                return None;
            }
            let (id, username, tier_id) = (id.clone(), username.clone(), tier_id.clone());
            let (manual_until_date, note) = (manual_until_date.clone(), note.clone());
            {
                let options = tiers
                    .iter()
                    .map(|tier| {
                        view! {
                            <option value=tier.id.clone() selected=tier.id == tier_id>
                                {tier.name.clone()}
                            </option>
                        }
                    })
                    .collect_view();
                let (clear_id, resync_id) = (id.clone(), id.clone());
                let row = view! {
                    <tr class="editor">
                        <td colspan="4">
                            <ActionForm action=set attr:class="editor-form">
                                <input type="hidden" name="user_id" value=id/>
                                <label>
                                    {format!("Plan for {username}")}
                                    <select name="tier_id">{options}</select>
                                </label>
                                <label>
                                    "Ends (optional)"
                                    <input
                                        type="date"
                                        name="valid_until"
                                        value=manual_until_date.unwrap_or_default()
                                    />
                                </label>
                                <label>
                                    "Note"
                                    <input
                                        name="note"
                                        maxlength="200"
                                        value=note.unwrap_or_default()
                                    />
                                </label>
                                <div class="editor-actions">
                                    <button type="submit" disabled=move || set.pending().get()>
                                        "Save plan"
                                    </button>
                                    {has_override
                                        .then(|| {
                                            view! {
                                                <button
                                                    type="button"
                                                    class="link danger"
                                                    on:click=move |_| {
                                                        clear.dispatch(ClearUserTier { user_id: clear_id.clone() });
                                                    }
                                                >
                                                    "Remove override"
                                                </button>
                                            }
                                        })}
                                    <button
                                        type="button"
                                        class="link"
                                        on:click=move |_| {
                                            resync.dispatch(ResyncUser { user_id: resync_id.clone() });
                                        }
                                    >
                                        "Resync now"
                                    </button>
                                </div>
                            </ActionForm>
                        </td>
                    </tr>
                };
                Some(row)
            }
        }
    };

    view! {
        <tr data-username=username.clone()>
            <td>
                {username.clone()}
                {disabled.then(|| view! { <span class="muted">" (disabled)"</span> })}
            </td>
            <td>
                <strong>{tier_name}</strong>
                <div class="muted small">{plan_detail}</div>
            </td>
            <td>
                <div class="badges">{badges}</div>
            </td>
            <td>
                <button type="button" class="link" on:click=toggle>
                    {move || if is_open.get() { "Close" } else { "Edit" }}
                </button>
            </td>
        </tr>
        {editor}
    }
}
