//! A space: a group of groups, rooms and channels, with a way into each of them.

use dioxus::prelude::*;
use whatsmine_core::Core;
use whatsmine_core::groups::{ChildState, SpaceChildView};
use whatsmine_core::protocol::ChildKind;

use crate::app::{Rev, Route, ToastSig, t, toast, use_core, use_tr};
use crate::components::{Header, Icon, IconName, Row, Sheet, ToastLevel};

fn names_of(core: &Core, people: &[String]) -> String {
    people
        .iter()
        .map(|who| {
            core.contact(who)
                .map(|c| c.display_name())
                .unwrap_or_else(|| core.settings().display_name)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[component]
pub fn SpaceScreen(id: String) -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let mut adding = use_signal(|| false);
    let _ = rev();
    let Some(info) = core.group_info(&id) else {
        return rsx! {};
    };
    let children = core.space_children(&id);
    let of_kind = |kind: ChildKind| -> Vec<SpaceChildView> {
        children
            .iter()
            .filter(|c| c.child.kind == kind)
            .cloned()
            .collect()
    };
    let (groups, rooms, channels) = (
        of_kind(ChildKind::Group),
        of_kind(ChildKind::Room),
        of_kind(ChildKind::Channel),
    );
    let can_add = core.space_can_add(&id);
    let mine = info.i_am_admin || info.i_am_owner;
    let members = t(&tr, "space.members").replace("{n}", &info.members.len().to_string());
    let info_id = id.clone();
    let empty = children.is_empty();
    rsx! {
        div { class: "screen no-nav", "data-t": "space",
            Header { title: info.name.clone(),
                Link {
                    to: Route::GroupInfoScreen { id: info_id },
                    class: "ib r",
                    "data-t": "space-info",
                    aria_label: "{t(&tr, \"space.info\")}",
                    Icon { name: IconName::Gear, size: 20 }
                }
            }
            div { class: "body",
                p { class: "note", style: "padding:0 4px", "{members}" }
                if empty {
                    p { class: "note", style: "padding:0 4px", "{t(&tr, \"space.empty\")}" }
                }
                if !groups.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"space.groups\")}" }
                    div { class: "group",
                        for c in groups {
                            ChildRow { key: "{c.child.id}", space: id.clone(), view: c, can_remove: mine }
                        }
                    }
                }
                if !rooms.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"space.rooms\")}" }
                    div { class: "group",
                        for c in rooms {
                            ChildRow { key: "{c.child.id}", space: id.clone(), view: c, can_remove: mine }
                        }
                    }
                }
                if !channels.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"space.channels\")}" }
                    div { class: "group",
                        for c in channels {
                            ChildRow { key: "{c.child.id}", space: id.clone(), view: c, can_remove: mine }
                        }
                    }
                }
                if mine {
                    div { class: "group",
                        Row {
                            icon: IconName::Plus,
                            title: t(&tr, "space.add"),
                            small: true,
                            tag: "space-add",
                            on_tap: move |_| adding.set(true),
                        }
                    }
                }
            }
            if adding() {
                Sheet {
                    on_close: move |_| adding.set(false),
                    title: t(&tr, "space.add"),
                    tag: "space-add-sheet",
                    if can_add.is_empty() {
                        p { class: "note", "{t(&tr, \"space.add_none\")}" }
                    }
                    for (child, name) in can_add {
                        {
                            let (core, space, row_id) = (core.clone(), id.clone(), child.clone());
                            let mut toast_sig = use_context::<ToastSig>();
                            rsx! {
                                button {
                                    key: "{row_id}",
                                    class: "sheet-item",
                                    "data-t": "space-add-item",
                                    onclick: move |_| {
                                        adding.set(false);
                                        if core.space_add_child(&space, &child).is_err() {
                                            toast(&mut toast_sig, t(&tr, "space.failed"), ToastLevel::Error);
                                        }
                                    },
                                    Icon { name: IconName::Users }
                                    span { "{name}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ChildRow(space: String, view: SpaceChildView, can_remove: bool) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let mut toast_sig = use_context::<ToastSig>();
    let child = view.child.clone();
    let room = child.kind == ChildKind::Room;
    let icon = match child.kind {
        ChildKind::Group => IconName::Users,
        ChildKind::Room => IconName::Mic,
        ChildKind::Channel => IconName::Bell,
    };
    let note = match (&view.state, room) {
        (ChildState::Asked, _) => t(&tr, "space.asked"),
        (ChildState::In, true) if view.people.is_empty() => t(&tr, "room.nobody"),
        (ChildState::In, true) => {
            t(&tr, "room.who").replace("{names}", &names_of(&core, &view.people))
        }
        (ChildState::In, false) => t(&tr, "space.joined"),
        (ChildState::Out, true) => t(&tr, "space.room_out"),
        (ChildState::Out, false) => t(&tr, "space.not_joined"),
    };
    let action = match &view.state {
        ChildState::In => None,
        ChildState::Asked => None,
        ChildState::Out => Some(t(&tr, "space.join")),
    };
    let open_id = child.id.clone();
    let join_id = child.id.clone();
    let remove_id = child.id.clone();
    let (join_core, remove_core) = (core.clone(), core.clone());
    let (join_space, remove_space) = (space.clone(), space);
    let is_in = view.state == ChildState::In;
    let mut asking_remove = use_signal(|| false);
    let remove_title = t(&tr, "space.remove_title").replace("{name}", &child.name);
    rsx! {
        if asking_remove() {
            crate::components::ConfirmDialog {
                title: remove_title,
                message: t(&tr, "space.remove_note"),
                confirm_label: t(&tr, "space.remove"),
                on_confirm: move |_| {
                    asking_remove.set(false);
                    let _ = remove_core.space_remove_child(&remove_space, &remove_id);
                },
                on_cancel: move |_| asking_remove.set(false),
            }
        }
        div { class: "row", "data-t": "space-child",
            button {
                class: "space-open",
                "data-t": "space-child-open",
                disabled: !is_in,
                onclick: move |_| {
                    nav.push(Route::Chat { id: open_id.clone() });
                },
                Icon { name: icon, size: 20 }
                div { class: "stack",
                    span { class: "row-t", "{child.name}" }
                    span { class: "sub", "{note}" }
                }
            }
            if let Some(word) = action {
                button {
                    class: "btn small",
                    "data-t": "space-child-join",
                    onclick: move |_| {
                        if join_core.space_open_child(&join_space, &join_id).is_err() {
                            toast(&mut toast_sig, t(&tr, "space.failed"), ToastLevel::Error);
                        }
                    },
                    "{word}"
                }
            }
            if can_remove {
                button {
                    class: "ib",
                    "data-t": "space-child-remove",
                    aria_label: "{t(&tr, \"space.remove\")}",
                    onclick: move |_| asking_remove.set(true),
                    Icon { name: IconName::Xmark, size: 18 }
                }
            }
        }
    }
}
