use dioxus::prelude::*;
use whatsmine_core::{Contact, ContactState};

use crate::app::{Rev, Route, t, use_core, use_tr};
use crate::components::{Avatar, BottomNav, ConfirmDialog, Icon, IconName, NetBanner, Tab};

/// People who want to connect, and requests we are waiting on.
#[component]
pub fn Requests() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let _ = rev();
    let incoming = core.requests();
    let outgoing: Vec<Contact> = core
        .chats()
        .into_iter()
        .map(|c| c.contact)
        .filter(|c| c.state == ContactState::PendingOut)
        .collect();

    rsx! {
        div { class: "screen",
            div { class: "header",
                h1 { "{t(&tr, \"requests.title\")}" }
            }
            NetBanner {}
            if incoming.is_empty() && outgoing.is_empty() {
                div { class: "empty-state",
                    Icon { name: IconName::Users, size: 56 }
                    p { class: "empty-title", "{t(&tr, \"requests.empty_title\")}" }
                    p { class: "empty-subtitle", "{t(&tr, \"requests.empty_subtitle\")}" }
                }
            }
            if !incoming.is_empty() {
                div { class: "settings-section",
                    label { class: "section-label", "{t(&tr, \"requests.incoming\")}" }
                    div { class: "list",
                        for c in incoming {
                            IncomingRequest { key: "{c.id}", contact: c }
                        }
                    }
                }
            }
            if !outgoing.is_empty() {
                div { class: "settings-section",
                    label { class: "section-label", "{t(&tr, \"requests.outgoing\")}" }
                    div { class: "list",
                        for c in outgoing {
                            Link { key: "{c.id}", to: Route::Chat { id: c.id.clone() }, class: "card",
                                Avatar { id: c.id.clone(), name: c.display_name(), image: c.avatar.clone() }
                                div { class: "card-main",
                                    span { class: "card-title", "{c.display_name()}" }
                                    span { class: "card-sub accent", "{t(&tr, \"chats.waiting\")}" }
                                }
                            }
                        }
                    }
                    p { class: "setting-hint", "{t(&tr, \"requests.outgoing_hint\")}" }
                }
            }
            BottomNav { active: Tab::Requests }
        }
    }
}

#[component]
fn IncomingRequest(contact: Contact) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let mut confirm_block = use_signal(|| false);
    let name = contact.display_name();
    let short: String = contact.id.chars().take(8).collect();
    let (id1, id2, id3) = (contact.id.clone(), contact.id.clone(), contact.id.clone());
    let (c1, c2, c3) = (core.clone(), core.clone(), core.clone());

    rsx! {
        div { class: "card static", style: "flex-direction: column; align-items: stretch;",
            div { style: "display: flex; gap: 12px; align-items: center;",
                Avatar { id: contact.id.clone(), name: name.clone(), image: contact.avatar.clone() }
                div { class: "card-main",
                    span { class: "card-title", "{name}" }
                    span { class: "card-sub", "{t(&tr, \"requests.key_starts\")} {short}" }
                }
            }
            div { class: "card-actions",
                button {
                    class: "secondary-btn primary grow",
                    onclick: move |_| {
                        if c1.accept(&id1).is_ok() {
                            nav.push(Route::Chat { id: id1.clone() });
                        }
                    },
                    Icon { name: IconName::Check, size: 18 }
                    "{t(&tr, \"requests.accept\")}"
                }
                button {
                    class: "secondary-btn grow",
                    onclick: move |_| {
                        let _ = c2.decline(&id2);
                    },
                    "{t(&tr, \"requests.decline\")}"
                }
                button {
                    class: "secondary-btn danger",
                    title: "{t(&tr, \"contact.block\")}",
                    onclick: move |_| confirm_block.set(true),
                    Icon { name: IconName::Ban, size: 18 }
                }
            }
        }
        if confirm_block() {
            ConfirmDialog {
                title: t(&tr, "contact.block_title").replace("{name}", &name),
                message: t(&tr, "contact.block_message"),
                confirm_label: t(&tr, "contact.block"),
                on_confirm: move |_| {
                    let _ = c3.block(&id3);
                    confirm_block.set(false);
                },
                on_cancel: move |_| confirm_block.set(false),
            }
        }
    }
}
