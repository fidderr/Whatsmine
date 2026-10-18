use dioxus::prelude::*;
use whatsmine_core::{Contact, ContactState};

use crate::app::{Rev, Route, SettingsSig, t, use_core, use_tr};
use crate::components::{Avatar, BottomNav, Icon, IconName, NetBanner, Tab};
use crate::fmt;

/// Everyone you have added, including people whose chat you deleted, and
/// the people you blocked.
#[component]
pub fn ContactsScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let nav = use_navigator();
    let mut query = use_signal(String::new);

    let _ = rev();
    let all = core.address_book();
    let blocked = core.blocked();
    let q = query().trim().to_lowercase();
    let people: Vec<Contact> = all
        .iter()
        .filter(|c| {
            q.is_empty()
                || c.display_name().to_lowercase().contains(&q)
                || c.their_name.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    let blocked: Vec<Contact> = blocked
        .into_iter()
        .filter(|c| q.is_empty() || c.display_name().to_lowercase().contains(&q))
        .collect();

    let count_label = t(&tr, "contacts.count").replace("{n}", &all.len().to_string());
    let blocked_label = t(&tr, "contacts.blocked").replace("{n}", &blocked.len().to_string());

    rsx! {
        div { class: "screen",
            div { class: "header",
                h1 { "{t(&tr, \"contacts.title\")}" }
            }
            NetBanner {}
            div { class: "search",
                Icon { name: IconName::Search, size: 18 }
                input {
                    r#type: "search",
                    placeholder: "{t(&tr, \"contacts.search\")}",
                    value: "{query}",
                    oninput: move |e| query.set(e.value()),
                }
            }
            div { class: "list",
                button {
                    class: "card action-card",
                    onclick: move |_| {
                        nav.push(Route::AddContact {});
                    },
                    div { class: "action-icon",
                        Icon { name: IconName::UserPlus, size: 20 }
                    }
                    div { class: "card-main",
                        span { class: "card-title", "{t(&tr, \"add.title\")}" }
                    }
                }
                button {
                    class: "card action-card",
                    onclick: move |_| {
                        nav.push(Route::NewGroup {});
                    },
                    div { class: "action-icon",
                        Icon { name: IconName::Users, size: 20 }
                    }
                    div { class: "card-main",
                        span { class: "card-title", "{t(&tr, \"group.new\")}" }
                    }
                }
            }
            if all.is_empty() {
                div { class: "empty-state",
                    Icon { name: IconName::Book, size: 56 }
                    p { class: "empty-title", "{t(&tr, \"contacts.empty_title\")}" }
                    p { class: "empty-subtitle", "{t(&tr, \"contacts.empty_subtitle\")}" }
                }
            } else if people.is_empty() && blocked.is_empty() {
                div { class: "empty-state",
                    p { class: "empty-subtitle", "{t(&tr, \"chats.no_match\")}" }
                }
            } else {
                if !people.is_empty() {
                    label { class: "section-label list-label", "{count_label}" }
                    div { class: "list",
                        for c in people {
                            ContactRow { key: "{c.id}", contact: c }
                        }
                    }
                }
            }
            if !blocked.is_empty() {
                label { class: "section-label list-label", "{blocked_label}" }
                p { class: "setting-hint list-hint", "{t(&tr, \"contacts.blocked_hint\")}" }
                div { class: "list",
                    for c in blocked {
                        BlockedRow { key: "{c.id}", contact: c }
                    }
                }
            }
            BottomNav { active: Tab::Contacts }
        }
    }
}

#[component]
fn ContactRow(contact: Contact) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let settings = use_context::<SettingsSig>().0;
    let name = contact.display_name();
    let online = core.is_online(&contact.id);
    let sub = if contact.state == ContactState::PendingOut {
        t(&tr, "chats.waiting")
    } else if online {
        t(&tr, "chat.online")
    } else if let (true, Some(ts)) = (settings.read().share_last_seen, contact.last_seen) {
        fmt::last_seen(&tr, ts)
    } else if !contact.their_name.is_empty() && contact.their_name != name {
        contact.their_name.clone()
    } else {
        String::new()
    };
    let (open_id, info_id) = (contact.id.clone(), contact.id.clone());
    rsx! {
        div {
            class: "card",
            onclick: move |_| {
                nav.push(Route::Chat { id: open_id.clone() });
            },
            Avatar {
                id: contact.id.clone(),
                name: name.clone(),
                online,
                image: contact.avatar.clone(),
                zoom: true,
            }
            div { class: "card-main",
                div { class: "card-row",
                    span { class: "card-title", "{name}" }
                }
                if !sub.is_empty() {
                    div { class: "card-row",
                        span { class: if online { "card-sub accent" } else { "card-sub" }, "{sub}" }
                    }
                }
            }
            button {
                class: "icon-btn",
                title: "{t(&tr, \"contact.title\")}",
                onclick: move |e| {
                    e.stop_propagation();
                    nav.push(Route::ContactInfo { id: info_id.clone() });
                },
                Icon { name: IconName::Info, size: 20 }
            }
        }
    }
}

#[component]
fn BlockedRow(contact: Contact) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let name = contact.display_name();
    let (id, info_id) = (contact.id.clone(), contact.id.clone());
    rsx! {
        div {
            class: "card",
            onclick: move |_| {
                nav.push(Route::ContactInfo { id: info_id.clone() });
            },
            Avatar {
                id: contact.id.clone(),
                name: name.clone(),
                image: contact.avatar.clone(),
            }
            div { class: "card-main",
                div { class: "card-row",
                    span { class: "card-title", "{name}" }
                }
            }
            button {
                class: "secondary-btn",
                onclick: move |e| {
                    e.stop_propagation();
                    let _ = core.unblock(&id);
                },
                "{t(&tr, \"contact.unblock\")}"
            }
        }
    }
}
