use std::collections::HashSet;

use dioxus::prelude::*;
use whatsmine_core::{ContactKind, ContactState};

use crate::app::{Rev, Route, ToastSig, t, toast, use_core, use_tr};
use crate::components::{Avatar, ConfirmDialog, Icon, IconName, ToastLevel};

/// Pick contacts and a name; you become the group's admin.
#[component]
pub fn NewGroup() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let rev = use_context::<Rev>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let mut name = use_signal(String::new);
    let mut picked = use_signal(HashSet::<String>::new);

    let _ = rev();
    let contacts: Vec<_> = core
        .chats()
        .into_iter()
        .map(|c| c.contact)
        .filter(|c| c.kind == ContactKind::Person && c.state == ContactState::Accepted)
        .collect();
    let count = picked.read().len();
    let can_create = count > 0 && !name.read().trim().is_empty();
    let pick_label = t(&tr, "group.pick_members").replace("{n}", &count.to_string());

    let create = {
        let core = core.clone();
        move |_| {
            let members: Vec<String> = picked.peek().iter().cloned().collect();
            match core.create_group(&name.peek(), &members) {
                Ok(g) => {
                    nav.replace(Route::Chat { id: g.id });
                }
                Err(e) => toast(
                    &mut toast_sig,
                    t(&tr, &format!("group.err.{e}")),
                    ToastLevel::Error,
                ),
            }
        }
    };

    rsx! {
        div { class: "screen no-nav",
            div { class: "config-header",
                button { class: "back-btn", onclick: move |_| nav.go_back(),
                    Icon { name: IconName::ArrowLeft }
                }
                span { class: "config-title", "{t(&tr, \"group.new\")}" }
            }
            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"group.name\")}" }
                input {
                    r#type: "text",
                    maxlength: "64",
                    placeholder: "{t(&tr, \"group.name_placeholder\")}",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
            }
            div { class: "settings-section",
                label { class: "section-label", "{pick_label}" }
                if contacts.is_empty() {
                    p { class: "setting-hint", "{t(&tr, \"group.no_contacts\")}" }
                }
                div { class: "list",
                    for c in contacts {
                        {
                            let id = c.id.clone();
                            let on = picked.read().contains(&id);
                            let label = c.display_name();
                            rsx! {
                                div {
                                    key: "{c.id}",
                                    class: if on { "card pick on" } else { "card pick" },
                                    onclick: {
                                        let id = id.clone();
                                        move |_| {
                                            let mut p = picked.write();
                                            if !p.remove(&id) {
                                                p.insert(id.clone());
                                            }
                                        }
                                    },
                                    Avatar {
                                        id: c.id.clone(),
                                        name: label.clone(),
                                        size: "small",
                                        image: c.avatar.clone(),
                                    }
                                    div { class: "card-main",
                                        span { class: "card-title", "{label}" }
                                    }
                                    if on {
                                        Icon { name: IconName::Check, size: 20 }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "settings-section",
                button {
                    class: "save-btn",
                    disabled: !can_create,
                    onclick: create,
                    "{t(&tr, \"group.create\")}"
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    Leave,
    Delete,
    Remove,
}

/// Members, and (for the admin) rename / add / remove.
#[component]
pub fn GroupInfoScreen(id: String) -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let rev = use_context::<Rev>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let mut confirm = use_signal(|| None::<Confirm>);
    let mut target = use_signal(String::new);
    let mut new_name = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.group_info(&id).map(|g| g.name).unwrap_or_default())
    };

    let _ = rev();
    let Some(info) = core.group_info(&id) else {
        return rsx! {
            div { class: "screen no-nav",
                div { class: "config-header",
                    button { class: "back-btn", onclick: move |_| nav.go_back(),
                        Icon { name: IconName::ArrowLeft }
                    }
                }
                div { class: "empty-state",
                    p { class: "empty-title", "{t(&tr, \"chat.not_found\")}" }
                }
            }
        };
    };
    let member_ids: HashSet<String> = info.members.iter().map(|m| m.id.clone()).collect();
    let addable: Vec<_> = if info.i_am_admin && info.active {
        core.chats()
            .into_iter()
            .map(|c| c.contact)
            .filter(|c| {
                c.kind == ContactKind::Person
                    && c.state == ContactState::Accepted
                    && !member_ids.contains(&c.id)
            })
            .collect()
    } else {
        Vec::new()
    };
    let subtitle = t(&tr, "group.members_count").replace("{n}", &info.members.len().to_string());
    let name = info.name.clone();
    let you_label = t(&tr, "group.you");
    let removing = info
        .members
        .iter()
        .find(|m| m.id == target())
        .map(|m| m.name.clone())
        .unwrap_or_default();

    rsx! {
        div { class: "screen no-nav",
            div { class: "config-header",
                button { class: "back-btn", onclick: move |_| nav.go_back(),
                    Icon { name: IconName::ArrowLeft }
                }
                span { class: "config-title", "{t(&tr, \"group.info\")}" }
            }
            div { class: "profile-head",
                Avatar { id: id.clone(), name: name.clone(), size: "large", group: true }
                span { class: "profile-name", "{name}" }
                span { class: "profile-sub", "{subtitle}" }
                if !info.active {
                    span { class: "profile-sub", "{t(&tr, \"group.inactive\")}" }
                }
            }

            if info.i_am_admin && info.active {
                div { class: "settings-section",
                    label { class: "section-label", "{t(&tr, \"group.rename\")}" }
                    div { class: "input-row",
                        input {
                            r#type: "text",
                            maxlength: "64",
                            value: "{new_name}",
                            oninput: move |e| new_name.set(e.value()),
                        }
                        button {
                            class: "secondary-btn",
                            onclick: {
                                let (core, id) = (core.clone(), id.clone());
                                move |_| match core.group_rename(&id, &new_name.peek()) {
                                    Ok(()) => toast(&mut toast_sig, t(&tr, "common.saved"), ToastLevel::Info),
                                    Err(e) => toast(&mut toast_sig, t(&tr, &format!("group.err.{e}")), ToastLevel::Error),
                                }
                            },
                            "{t(&tr, \"common.save\")}"
                        }
                    }
                }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"group.members\")}" }
                div { class: "list",
                    for m in info.members.clone() {
                        div { key: "{m.id}", class: "card member",
                            Avatar {
                                id: m.id.clone(),
                                name: m.name.clone(),
                                size: "small",
                                online: m.online || m.is_me,
                                image: core.contact(&m.id).and_then(|c| c.avatar),
                            }
                            div { class: "card-main",
                                span { class: "card-title",
                                    "{m.name}"
                                    if m.is_me {
                                        " ({you_label})"
                                    }
                                }
                                if m.is_admin {
                                    span { class: "card-sub accent", "{t(&tr, \"group.admin\")}" }
                                } else if !m.is_contact && !m.is_me {
                                    span { class: "card-sub", "{t(&tr, \"group.not_contact\")}" }
                                }
                            }
                            if info.i_am_admin && info.active && !m.is_me {
                                button {
                                    class: "icon-btn remove-member",
                                    title: "{t(&tr, \"group.remove\")}",
                                    onclick: {
                                        let mid = m.id.clone();
                                        move |_| {
                                            target.set(mid.clone());
                                            confirm.set(Some(Confirm::Remove));
                                        }
                                    },
                                    Icon { name: IconName::Xmark, size: 18 }
                                }
                            }
                        }
                    }
                }
            }

            if !addable.is_empty() {
                div { class: "settings-section",
                    label { class: "section-label", "{t(&tr, \"group.add_member\")}" }
                    div { class: "list",
                        for c in addable {
                            div { key: "{c.id}", class: "card",
                                Avatar {
                                    id: c.id.clone(),
                                    name: c.display_name(),
                                    size: "small",
                                    image: c.avatar.clone(),
                                }
                                div { class: "card-main",
                                    span { class: "card-title", "{c.display_name()}" }
                                }
                                button {
                                    class: "secondary-btn add-member",
                                    onclick: {
                                        let (core, gid, cid) = (core.clone(), id.clone(), c.id.clone());
                                        move |_| {
                                            if let Err(e) = core.group_add(&gid, &cid) {
                                                toast(&mut toast_sig, t(&tr, &format!("group.err.{e}")), ToastLevel::Error);
                                            }
                                        }
                                    },
                                    "{t(&tr, \"group.add\")}"
                                }
                            }
                        }
                    }
                }
            }

            div { class: "settings-section",
                if info.active {
                    button { class: "delete-btn", onclick: move |_| confirm.set(Some(Confirm::Leave)),
                        "{t(&tr, \"group.leave\")}"
                    }
                }
                button { class: "delete-btn", onclick: move |_| confirm.set(Some(Confirm::Delete)),
                    "{t(&tr, \"group.delete\")}"
                }
            }

            if let Some(which) = confirm() {
                ConfirmDialog {
                    title: match which {
                        Confirm::Leave => t(&tr, "group.leave_title").replace("{name}", &name),
                        Confirm::Delete => t(&tr, "group.delete_title").replace("{name}", &name),
                        Confirm::Remove => t(&tr, "group.remove_title").replace("{name}", &removing),
                    },
                    message: match which {
                        Confirm::Leave if info.i_am_admin => t(&tr, "group.leave_admin_message"),
                        Confirm::Leave => t(&tr, "group.leave_message"),
                        Confirm::Delete => t(&tr, "group.delete_message"),
                        Confirm::Remove => t(&tr, "group.remove_message"),
                    },
                    confirm_label: match which {
                        Confirm::Leave => t(&tr, "group.leave"),
                        Confirm::Delete => t(&tr, "group.delete"),
                        Confirm::Remove => t(&tr, "group.remove"),
                    },
                    on_confirm: {
                        let (core, id) = (core.clone(), id.clone());
                        move |_| {
                            confirm.set(None);
                            match which {
                                Confirm::Leave => {
                                    let _ = core.leave_group(&id);
                                }
                                Confirm::Delete => {
                                    let _ = core.delete_group(&id);
                                    nav.replace(Route::Chats {});
                                }
                                Confirm::Remove => {
                                    let _ = core.group_remove(&id, &target.peek());
                                }
                            }
                        }
                    },
                    on_cancel: move |_| confirm.set(None),
                }
            }
        }
    }
}
