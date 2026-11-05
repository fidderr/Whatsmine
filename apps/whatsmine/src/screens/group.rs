use std::collections::HashSet;

use dioxus::prelude::*;
use whatsmine_core::{ContactKind, ContactState};

use crate::app::{Rev, Route, ToastSig, t, toast, use_core, use_tr};
use crate::call::{self, CallBridge, CallUi};
use crate::components::{
    Avatar, BackButton, Chips, ConfirmDialog, Header, Icon, IconName, Row, Sheet, ToastLevel,
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
    let mut room = use_signal(|| false);

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
            let made = if *room.peek() {
                core.create_room(&name.peek(), &members)
            } else {
                core.create_group(&name.peek(), &members)
            };
            match made {
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
                div { class: "group",
                    crate::components::SwitchRow {
                        title: t(&tr, "room.switch"),
                        note: t(&tr, "room.switch_note"),
                        checked: room(),
                        tag: "group-room",
                        on_change: move |v: bool| room.set(v),
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
    End,
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
    let mut rules_open = use_signal(|| false);
    let mut link_open = use_signal(|| false);
    let mut member_menu = use_signal(|| None::<whatsmine_core::GroupMember>);
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
    let may_add = info.active && (info.i_am_admin || !info.rules.only_admins_add);
    let may_edit = info.active && (info.i_am_admin || !info.rules.only_admins_edit);
    let asking = if info.i_am_admin {
        core.group_link_state(&id).asking
    } else {
        Vec::new()
    };
    let addable: Vec<_> = if may_add {
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
                if may_edit {
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
                    if !info.rules.about.is_empty() {
                        span { class: "sub", "{info.rules.about}" }
                    }
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
                if !asking.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"group.asking\")}" }
                    div { class: "group",
                        for a in asking {
                            div { key: "{a.id}", class: "row still", "data-t": "asking",
                                span { class: "stack",
                                    span { class: "row-t one-line", "{a.name}" }
                                    if !a.greeting.is_empty() {
                                        span { class: "sub", "{a.greeting}" }
                                    }
                                }
                                button {
                                    class: "btn small-btn",
                                    onclick: {
                                        let (core, gid, who) = (core.clone(), id.clone(), a.id.clone());
                                        move |_| {
                                            let _ = core.group_answer_join(&gid, &who, false);
                                        }
                                    },
                                    "{t(&tr, \"requests.decline\")}"
                                }
                                button {
                                    class: "btn small-btn main",
                                    "data-t": "asking-yes",
                                    onclick: {
                                        let (core, gid, who) = (core.clone(), id.clone(), a.id.clone());
                                        move |_| {
                                            if let Err(e) = core.group_answer_join(&gid, &who, true) {
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
                                if m.is_owner {
                                    span { class: "sub accent-text", "{t(&tr, \"group.owner\")}" }
                                } else if m.is_admin {
                                    span { class: "sub accent-text", "{t(&tr, \"group.admin\")}" }
                                } else if !m.is_contact && !m.is_me {
                                    span { class: "sub", "{t(&tr, \"group.not_contact\")}" }
                                }
                            }
                            if info.i_am_admin && info.active && !m.is_me && (info.i_am_owner || !m.is_admin) {
                                button {
                                    class: "ib",
                                    "data-t": "member-menu",
                                    aria_label: "{t(&tr, \"chat.more\")}",
                                    onclick: {
                                        let member = m.clone();
                                        move |_| member_menu.set(Some(member.clone()))
                                    },
                                    Icon { name: IconName::More, size: 20 }
                                }
                            }
                        }
                    }
                }
                if info.i_am_admin && info.active {
                    div { class: "group",
                        Row {
                            icon: IconName::Link,
                            title: t(&tr, "group.link"),
                            small: true,
                            tag: "group-link",
                            on_tap: move |_| link_open.set(true),
                        }
                        Row {
                            icon: IconName::Shield,
                            title: t(&tr, "group.rules"),
                            small: true,
                            tag: "group-rules",
                            on_tap: move |_| rules_open.set(true),
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
                    if info.i_am_owner && info.active {
                        Row {
                            icon: IconName::Ban,
                            title: t(&tr, "group.end"),
                            small: true,
                            danger: true,
                            tag: "end-group",
                            on_tap: move |_| confirm.set(Some(Confirm::End)),
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
            if rules_open() {
                RulesSheet { id: id.clone(), on_close: move |_| rules_open.set(false) }
            }
            if link_open() {
                LinkSheet { id: id.clone(), on_close: move |_| link_open.set(false) }
            }
            if let Some(member) = member_menu() {
                MemberSheet {
                    id: id.clone(),
                    member,
                    owner: info.i_am_owner,
                    on_remove: move |who: String| {
                        target.set(who);
                        confirm.set(Some(Confirm::Remove));
                    },
                    on_close: move |_| member_menu.set(None),
                }
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
                        Confirm::End => t(&tr, "group.end_title").replace("{name}", &name),
                    },
                    message: match which {
                        Confirm::Leave if info.i_am_admin => t(&tr, "group.leave_admin_message"),
                        Confirm::Leave => t(&tr, "group.leave_message"),
                        Confirm::Delete => t(&tr, "group.delete_message"),
                        Confirm::Remove => t(&tr, "group.remove_message"),
                        Confirm::End => t(&tr, "group.end_message"),
                    },
                    confirm_label: match which {
                        Confirm::Leave => t(&tr, "group.leave"),
                        Confirm::Delete => t(&tr, "group.delete"),
                        Confirm::Remove => t(&tr, "group.remove"),
                        Confirm::End => t(&tr, "group.end"),
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
                                Confirm::End => {
                                    let _ = core.group_close(&id);
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

/// What the group allows.
#[component]
fn RulesSheet(id: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let _ = rev();
    let Some(info) = core.group_info(&id) else {
        return rsx! {};
    };
    let r = info.rules.clone();
    let who = vec![
        ("all".to_owned(), t(&tr, "group.who_everyone")),
        ("admins".to_owned(), t(&tr, "group.who_admins")),
    ];
    let pick = |only: bool| if only { "admins" } else { "all" }.to_owned();
    let slow = vec![
        ("0".to_owned(), t(&tr, "group.slow_off")),
        ("30000".to_owned(), t(&tr, "group.slow_30s")),
        ("60000".to_owned(), t(&tr, "group.slow_1m")),
        ("300000".to_owned(), t(&tr, "group.slow_5m")),
    ];
    let mut about = use_signal(|| r.about.clone());
    let (c1, c2, c3, c4, c5) = (core.clone(), core.clone(), core.clone(), core.clone(), core);
    let (i1, i2, i3, i4, i5) = (id.clone(), id.clone(), id.clone(), id.clone(), id);
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "group.rules"), tag: "rules-sheet",
            h3 { "{t(&tr, \"group.who_sends\")}" }
            Chips {
                options: who.clone(),
                value: pick(r.only_admins_send),
                on_change: move |v: String| {
                    let _ = c1.group_set_rules(&i1, |r| r.only_admins_send = v == "admins");
                },
            }
            h3 { "{t(&tr, \"group.who_adds\")}" }
            Chips {
                options: who.clone(),
                value: pick(r.only_admins_add),
                on_change: move |v: String| {
                    let _ = c2.group_set_rules(&i2, |r| r.only_admins_add = v == "admins");
                },
            }
            h3 { "{t(&tr, \"group.who_edits\")}" }
            Chips {
                options: who,
                value: pick(r.only_admins_edit),
                on_change: move |v: String| {
                    let _ = c3.group_set_rules(&i3, |r| r.only_admins_edit = v == "admins");
                },
            }
            h3 { "{t(&tr, \"group.slow\")}" }
            Chips {
                options: slow,
                value: r.slow_ms.to_string(),
                on_change: move |v: String| {
                    let ms = v.parse::<u32>().unwrap_or(0);
                    let _ = c4.group_set_rules(&i4, |r| r.slow_ms = ms);
                },
            }
            label { class: "field",
                span { class: "gt", "{t(&tr, \"group.about\")}" }
                input {
                    r#type: "text",
                    maxlength: "140",
                    value: "{about}",
                    oninput: move |e| about.set(e.value()),
                }
            }
            button {
                class: "btn main",
                onclick: move |_| {
                    let text = about.peek().trim().to_owned();
                    let _ = c5.group_set_rules(&i5, |r| r.about = text);
                    on_close.call(());
                },
                "{t(&tr, \"common.save\")}"
            }
        }
    }
}

/// The link anyone can ask to join with.
#[component]
fn LinkSheet(id: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let toast_sig = use_context::<ToastSig>();
    let rev = use_context::<Rev>().0;
    let _ = rev();
    let mut link = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.group_link(&id).unwrap_or_default())
    };
    let approve = core.group_link_state(&id).approve;
    let (approve_core, reset_core, approve_id, reset_id) = (core.clone(), core, id.clone(), id);
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "group.link"), tag: "link-sheet",
            div { style: "align-self:center",
                crate::components::QrCode { data: link() }
            }
            p { class: "note", "{t(&tr, \"group.link_note\")}" }
            div { class: "sheet-list",
                crate::components::SwitchRow {
                    title: t(&tr, "group.approve"),
                    note: t(&tr, "group.approve_note"),
                    checked: approve,
                    on_change: move |on| {
                        let _ = approve_core.group_link_approve(&approve_id, on);
                    },
                }
            }
            div { class: "btns",
                button {
                    class: "btn",
                    onclick: move |_| {
                        if let Ok(fresh) = reset_core.group_link_reset(&reset_id) {
                            link.set(fresh);
                        }
                    },
                    Icon { name: IconName::Restart, size: 20 }
                    "{t(&tr, \"group.link_new\")}"
                }
                button {
                    class: "btn main",
                    "data-t": "share-group-link",
                    onclick: move |_| {
                        crate::screens::add::share_or_copy(tr, toast_sig, link.peek().clone(), t(&tr, "add.share"));
                    },
                    Icon { name: IconName::Share, size: 20 }
                    "{t(&tr, \"add.share\")}"
                }
            }
        }
    }
}

/// What can be done with one member.
#[component]
fn MemberSheet(
    id: String,
    member: whatsmine_core::GroupMember,
    owner: bool,
    on_remove: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    let tr = use_tr();
    let core = use_core();
    let (admin_core, owner_core) = (core.clone(), core);
    let (admin_id, owner_id) = (id.clone(), id);
    let (who_admin, who_owner, who_remove) =
        (member.id.clone(), member.id.clone(), member.id.clone());
    let is_admin = member.is_admin;
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: member.name.clone(), tag: "member-sheet",
            div { class: "sheet-list",
                if owner {
                    crate::components::SheetItem {
                        label: t(&tr, if is_admin { "group.admin_off" } else { "group.admin_on" }),
                        icon: IconName::Shield,
                        tag: "make-admin",
                        on_tap: move |_| {
                            let _ = admin_core.group_set_admin(&admin_id, &who_admin, !is_admin);
                            on_close.call(());
                        },
                    }
                    crate::components::SheetItem {
                        label: t(&tr, "group.hand_over"),
                        icon: IconName::Key,
                        tag: "hand-over",
                        on_tap: move |_| {
                            let _ = owner_core.group_hand_over(&owner_id, &who_owner);
                            on_close.call(());
                        },
                    }
                }
                crate::components::SheetItem {
                    label: t(&tr, "group.remove"),
                    icon: IconName::Xmark,
                    danger: true,
                    tag: "remove-member",
                    on_tap: move |_| {
                        on_remove.call(who_remove.clone());
                        on_close.call(());
                    },
                }
            }
        }
    }
}
