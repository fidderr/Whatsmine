use std::collections::HashSet;

use dioxus::prelude::*;
use whatsmine_core::{ContactKind, ContactState};

use crate::app::{Rev, Route, ToastSig, t, toast, use_core, use_tr};
use crate::call::{self, CallBridge, CallUi};
use crate::components::{
    Avatar, BackButton, ConfirmDialog, Header, Icon, IconName, Row, Sheet, ToastLevel,
};
use crate::screens::contact::{DisappearSheet, disappear_label};

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
            Header { title: t(&tr, "group.new") }
            div { class: "body g12",
                label { class: "field",
                    span { class: "gt", "{t(&tr, \"group.name\")}" }
                    input {
                        r#type: "text",
                        maxlength: "64",
                        placeholder: "{t(&tr, \"group.name_placeholder\")}",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                h2 { class: "gt", "{pick_label}" }
                if contacts.is_empty() {
                    p { class: "note", "{t(&tr, \"group.no_contacts\")}" }
                } else {
                    div { class: "group",
                        for c in contacts {
                            {
                                let id = c.id.clone();
                                let on = picked.read().contains(&id);
                                let label = c.display_name();
                                rsx! {
                                    button {
                                        key: "{c.id}",
                                        class: "row",
                                        role: "checkbox",
                                        aria_checked: "{on}",
                                        onclick: move |_| {
                                            let mut p = picked.write();
                                            if !p.remove(&id) {
                                                p.insert(id.clone());
                                            }
                                        },
                                        Avatar {
                                            id: c.id.clone(),
                                            name: label.clone(),
                                            size: "medium",
                                            image: c.avatar.clone(),
                                        }
                                        span { class: "row-t stack one-line", "{label}" }
                                        if on {
                                            span { class: "accent-text", style: "display:flex",
                                                Icon { name: IconName::Check, size: 20 }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                button {
                    class: "btn main",
                    "data-t": "create-group",
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

/// Members, and (for the admin) rename, add and remove.
#[component]
pub fn GroupInfoScreen(id: String) -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let rev = use_context::<Rev>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let bridge = use_context::<CallBridge>();
    let phase = use_context::<CallUi>().0;
    let mut confirm = use_signal(|| None::<Confirm>);
    let mut target = use_signal(String::new);
    let mut renaming = use_signal(|| false);
    let mut adding = use_signal(|| false);
    let mut disappear = use_signal(|| false);
    let mut new_name = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.group_info(&id).map(|g| g.name).unwrap_or_default())
    };

    let _ = rev();
    let Some(info) = core.group_info(&id) else {
        return rsx! {
            div { class: "screen no-nav",
                header { class: "hdr", BackButton {} }
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
    let prefs = core.chat_prefs(&id);
    let id_shared = id.clone();
    let (call_core, call_id) = (core.clone(), id.clone());
    let id_look = id.clone();

    rsx! {
        div { class: "screen no-nav",
            header { class: "hdr", style: "padding-bottom:0",
                BackButton {}
                span { style: "flex:1" }
                if info.i_am_admin && info.active {
                    button {
                        class: "ib",
                        "data-t": "edit-group",
                        aria_label: "{t(&tr, \"group.rename\")}",
                        onclick: move |_| renaming.set(true),
                        Icon { name: IconName::Pencil, size: 22 }
                    }
                }
            }
            div { class: "body",
                div { class: "panel center", style: "background:transparent;padding:0;gap:4px",
                    Avatar { id: id.clone(), name: name.clone(), size: "big", group: true, zoom: true }
                    h1 { style: "font-size:1.75rem;line-height:1.3;font-weight:700", "{name}" }
                    span { class: "sub", "{subtitle}" }
                    if !info.active {
                        span { class: "sub", "{t(&tr, \"group.inactive\")}" }
                    }
                }
                div { class: "tile-actions",
                    if !addable.is_empty() {
                        button {
                            class: "tile-action",
                            "data-t": "add-member",
                            onclick: move |_| adding.set(true),
                            Icon { name: IconName::UserPlus, size: 22 }
                            "{t(&tr, \"group.add\")}"
                        }
                    }
                    if info.active {
                        button {
                            class: "tile-action",
                            onclick: move |_| {
                                call::start_call(&call_core, &call_id, false, phase, &bridge, &mut toast_sig, &tr);
                            },
                            Icon { name: IconName::Phone, size: 22 }
                            "{t(&tr, \"call.voice_short\")}"
                        }
                    }
                    Link {
                        class: "tile-action",
                        to: Route::SharedMedia { id: id_shared.clone() },
                        Icon { name: IconName::Search, size: 22 }
                        "{t(&tr, \"contact.search\")}"
                    }
                }
                h2 { class: "gt", "{t(&tr, \"group.members\")}" }
                div { class: "group",
                    for m in info.members.clone() {
                        div { key: "{m.id}", class: "row still",
                            Avatar {
                                id: m.id.clone(),
                                name: m.name.clone(),
                                size: "medium",
                                online: m.online || m.is_me,
                                image: core.contact(&m.id).and_then(|c| c.avatar),
                                zoom: true,
                            }
                            span { class: "stack",
                                span { class: "row-t one-line",
                                    "{m.name}"
                                    if m.is_me {
                                        " ({you_label})"
                                    }
                                }
                                if m.is_admin {
                                    span { class: "sub accent-text", "{t(&tr, \"group.admin\")}" }
                                } else if !m.is_contact && !m.is_me {
                                    span { class: "sub", "{t(&tr, \"group.not_contact\")}" }
                                }
                            }
                            if info.i_am_admin && info.active && !m.is_me {
                                button {
                                    class: "ib danger-text",
                                    "data-t": "remove-member",
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
                div { class: "group",
                    Row {
                        icon: IconName::Clock,
                        title: t(&tr, "contact.disappear"),
                        value: disappear_label(&tr, prefs.disappear_ms),
                        small: true,
                        on_tap: move |_| disappear.set(true),
                    }
                    Row {
                        icon: IconName::Palette,
                        title: t(&tr, "look.title"),
                        small: true,
                        to: Route::ChatLook { id: id_look.clone() },
                    }
                    if info.active {
                        Row {
                            icon: IconName::Xmark,
                            title: t(&tr, "group.leave"),
                            small: true,
                            danger: true,
                            tag: "leave",
                            on_tap: move |_| confirm.set(Some(Confirm::Leave)),
                        }
                    }
                    Row {
                        icon: IconName::Trash,
                        title: t(&tr, "group.delete"),
                        small: true,
                        danger: true,
                        on_tap: move |_| confirm.set(Some(Confirm::Delete)),
                    }
                }
            }
            if disappear() {
                DisappearSheet { id: id.clone(), on_close: move |_| disappear.set(false) }
            }
            if renaming() {
                Sheet { on_close: move |_| renaming.set(false), title: t(&tr, "group.rename"),
                    input {
                        r#type: "text",
                        maxlength: "64",
                        value: "{new_name}",
                        oninput: move |e| new_name.set(e.value()),
                    }
                    button {
                        class: "btn main",
                        onclick: {
                            let (core, id) = (core.clone(), id.clone());
                            move |_| {
                                match core.group_rename(&id, &new_name.peek()) {
                                    Ok(()) => toast(&mut toast_sig, t(&tr, "common.saved"), ToastLevel::Info),
                                    Err(e) => toast(&mut toast_sig, t(&tr, &format!("group.err.{e}")), ToastLevel::Error),
                                }
                                renaming.set(false);
                            }
                        },
                        "{t(&tr, \"common.save\")}"
                    }
                }
            }
            if adding() {
                Sheet { on_close: move |_| adding.set(false), title: t(&tr, "group.add_member"),
                    div { class: "sheet-list",
                        for c in addable {
                            button {
                                key: "{c.id}",
                                class: "sheet-item",
                                onclick: {
                                    let (core, gid, cid) = (core.clone(), id.clone(), c.id.clone());
                                    move |_| {
                                        if let Err(e) = core.group_add(&gid, &cid) {
                                            toast(&mut toast_sig, t(&tr, &format!("group.err.{e}")), ToastLevel::Error);
                                        }
                                        adding.set(false);
                                    }
                                },
                                Avatar { id: c.id.clone(), name: c.display_name(), size: "tiny", image: c.avatar.clone() }
                                span { "{c.display_name()}" }
                            }
                        }
                    }
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
