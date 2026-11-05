use dioxus::prelude::*;
use whatsmine_core::{Contact, ContactState, Trust};

use crate::app::{Rev, Route, t, use_core, use_tr};
use crate::components::{Avatar, BottomNav, Icon, IconName, NetBanner, Notice, Row, Tab, Title};

/// A person's mark: how sure you are who they are.
pub fn trust_label(tr: &crate::app::Translations, trust: Trust) -> String {
    match trust {
        Trust::Link => t(tr, "trust.link"),
        Trust::MetInPerson => t(tr, "trust.met"),
        Trust::SafetyWords => t(tr, "trust.words"),
    }
}

#[component]
pub fn TrustMark(trust: Trust) -> Element {
    let tr = use_tr();
    let (icon, class) = match trust {
        Trust::Link => (IconName::Link, "mark sub"),
        _ => (IconName::Shield, "mark sub ok-text"),
    };
    rsx! {
        span { class,
            Icon { name: icon, size: 14 }
            "{trust_label(&tr, trust)}"
        }
    }
}

/// Everyone you have added, and who wants to add you.
#[component]
pub fn People() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let mut query = use_signal(String::new);
    let mut searching = use_signal(|| false);

    let _ = rev();
    let all = core.address_book();
    let blocked = core.blocked().len();
    let requests = core.requests().len();
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
    let favourites: Vec<Contact> = core
        .chats()
        .into_iter()
        .filter(|c| c.prefs.favourite)
        .map(|c| c.contact)
        .collect();
    let want_label = t(&tr, "chats.want_to_add").replace("{n}", &requests.to_string());

    rsx! {
        div { class: "screen",
            Title { title: t(&tr, "people.title"),
                button {
                    class: "ib r",
                    "data-t": "people-search",
                    aria_label: "{t(&tr, \"contacts.search\")}",
                    onclick: move |_| searching.toggle(),
                    Icon { name: IconName::Search, size: 20 }
                }
                Link {
                    to: Route::AddContact {},
                    class: "ib r",
                    "data-t": "people-add",
                    aria_label: "{t(&tr, \"add.title\")}",
                    Icon { name: IconName::UserPlus, size: 20 }
                }
            }
            div { class: "body",
                NetBanner {}
                if searching() {
                    label { class: "search-box",
                        Icon { name: IconName::Search, size: 18 }
                        input {
                            r#type: "search",
                            autofocus: true,
                            placeholder: "{t(&tr, \"contacts.search\")}",
                            value: "{query}",
                            oninput: move |e| query.set(e.value()),
                        }
                    }
                }
                if requests > 0 {
                    Notice {
                        icon: IconName::UserPlus,
                        text: want_label,
                        to: Route::Requests {},
                        tag: "requests-notice",
                    }
                }
                div { class: "group",
                    Row {
                        icon: IconName::UserPlus,
                        title: t(&tr, "add.title"),
                        small: true,
                        to: Route::AddContact {},
                        tag: "add",
                    }
                    Row {
                        icon: IconName::Users,
                        title: t(&tr, "group.new"),
                        small: true,
                        to: Route::NewGroup {},
                        tag: "new-group",
                    }
                }
                if !favourites.is_empty() && q.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"people.favourites\")}" }
                    Faces { people: favourites, to_call: false }
                }
                if all.is_empty() {
                    div { class: "empty-state",
                        Icon { name: IconName::People, size: 56 }
                        p { class: "empty-title", "{t(&tr, \"contacts.empty_title\")}" }
                        p { class: "empty-subtitle", "{t(&tr, \"contacts.empty_subtitle\")}" }
                    }
                } else if people.is_empty() {
                    div { class: "empty-state",
                        p { class: "empty-subtitle", "{t(&tr, \"chats.no_match\")}" }
                    }
                } else {
                    h2 { class: "gt", "{t(&tr, \"people.everyone\")}" }
                    div { class: "group",
                        for c in people {
                            PersonRow { key: "{c.id}", contact: c }
                        }
                    }
                }
                if blocked > 0 {
                    div { class: "group",
                        Row {
                            icon: IconName::Ban,
                            title: t(&tr, "settings.blocked"),
                            value: blocked.to_string(),
                            small: true,
                            to: Route::PrivacyScreen {},
                        }
                    }
                }
            }
            BottomNav { active: Tab::People }
        }
    }
}

/// A row of faces: favourite people.
#[component]
pub fn Faces(people: Vec<Contact>, to_call: bool) -> Element {
    let core = use_core();
    let nav = use_navigator();
    let bridge = use_context::<crate::call::CallBridge>();
    let phase = use_context::<crate::call::CallUi>().0;
    let toast_sig = use_context::<crate::app::ToastSig>();
    let tr = use_tr();
    rsx! {
        div { class: "faces",
            for c in people {
                {
                    let id = c.id.clone();
                    let (core, bridge) = (core.clone(), bridge.clone());
                    rsx! {
                        button {
                            key: "{c.id}",
                            class: "face",
                            onclick: move |_| {
                                if to_call {
                                    let mut sig = toast_sig;
                                    crate::call::start_call(&core, &id, false, phase, &bridge, &mut sig, &tr);
                                } else {
                                    nav.push(Route::ContactInfo { id: id.clone() });
                                }
                            },
                            Avatar {
                                id: c.id.clone(),
                                name: c.display_name(),
                                size: "face-size",
                                image: c.avatar.clone(),
                            }
                            span { "{c.display_name()}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PersonRow(contact: Contact) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let name = contact.display_name();
    let online = core.is_online(&contact.id);
    let trust = core.contact_keys(&contact.id).trust;
    let id = contact.id.clone();
    rsx! {
        button {
            class: "row",
            onclick: move |_| {
                nav.push(Route::ContactInfo { id: id.clone() });
            },
            Avatar {
                id: contact.id.clone(),
                name: name.clone(),
                size: "medium",
                online,
                image: contact.avatar.clone(),
            }
            span { class: "stack",
                span { class: "row-t one-line", "{name}" }
                if contact.state == ContactState::PendingOut {
                    span { class: "sub accent-text", "{t(&tr, \"chats.waiting\")}" }
                } else {
                    TrustMark { trust }
                }
            }
        }
    }
}
