use dioxus::prelude::*;
use whatsmine_core::{Contact, ContactState};

use crate::app::{Rev, Route, t, use_core, use_tr};
use crate::components::{
    Avatar, ConfirmDialog, Header, IconName, Row, SwitchRow, change, use_settings,
};

/// People who want to connect, and requests we are waiting on.
#[component]
pub fn Requests() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let settings = use_settings();
    let _ = rev();
    let incoming = core.requests();
    let outgoing: Vec<Contact> = core
        .chats()
        .into_iter()
        .map(|c| c.contact)
        .filter(|c| c.state == ContactState::PendingOut)
        .collect();
    let quiet = settings.read().quiet_requests;

    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "requests.title") }
            div { class: "body g12",
                div { class: "group",
                    SwitchRow {
                        title: t(&tr, "requests.quiet"),
                        note: t(&tr, "requests.quiet_note"),
                        checked: quiet,
                        tag: "quiet",
                        on_change: move |on| change(settings, |s| s.quiet_requests = on),
                    }
                }
                if incoming.is_empty() && outgoing.is_empty() {
                    div { class: "empty-state",
                        p { class: "empty-title", "{t(&tr, \"requests.empty_title\")}" }
                        p { class: "empty-subtitle", "{t(&tr, \"requests.empty_subtitle\")}" }
                    }
                }
                for c in incoming.iter().cloned() {
                    IncomingRequest { key: "{c.id}", contact: c }
                }
                if !incoming.is_empty() {
                    p { class: "note", "{t(&tr, \"requests.limits\")}" }
                }
                if !outgoing.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"requests.outgoing\")}" }
                    div { class: "group",
                        for c in outgoing {
                            Link { key: "{c.id}", to: Route::Chat { id: c.id.clone() }, class: "row",
                                Avatar { id: c.id.clone(), name: c.display_name(), size: "medium", image: c.avatar.clone() }
                                span { class: "stack",
                                    span { class: "row-t one-line", "{c.display_name()}" }
                                    span { class: "sub accent-text", "{t(&tr, \"chats.waiting\")}" }
                                }
                            }
                        }
                    }
                    p { class: "note", "{t(&tr, \"requests.outgoing_hint\")}" }
                }
                div { class: "group",
                    Row {
                        icon: IconName::Link,
                        title: t(&tr, "requests.too_many"),
                        note: t(&tr, "requests.too_many_note"),
                        to: Route::PublicAddress {},
                        tag: "address",
                    }
                }
            }
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
    let keys = core.contact_keys(&contact.id);
    let via = if keys.via.is_empty() {
        t(&tr, "requests.via_main")
    } else {
        keys.via.clone()
    };
    let came = t(&tr, "requests.came_through").replace("{via}", &via);
    let (id1, id2, id3) = (contact.id.clone(), contact.id.clone(), contact.id.clone());
    let (c1, c2, c3) = (core.clone(), core.clone(), core.clone());

    rsx! {
        section { class: "panel", "data-t": "request",
            div { style: "display:flex;align-items:center;gap:12px",
                Avatar { id: contact.id.clone(), name: name.clone() }
                span { class: "stack",
                    span { class: "row-t b one-line", "{name}" }
                    span { class: "sub", "{came}" }
                }
            }
            if !keys.greeting.is_empty() {
                p { class: "plain-box", "{keys.greeting}" }
            }
            div { class: "btns",
                button {
                    class: "btn main",
                    "data-t": "accept",
                    onclick: move |_| {
                        if c1.accept(&id1).is_ok() {
                            nav.push(Route::Chat { id: id1.clone() });
                        }
                    },
                    "{t(&tr, \"requests.accept\")}"
                }
                button {
                    class: "btn",
                    onclick: move |_| {
                        let _ = c2.decline(&id2);
                    },
                    "{t(&tr, \"requests.decline\")}"
                }
                button { class: "btn", onclick: move |_| confirm_block.set(true),
                    "{t(&tr, \"contact.block\")}"
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
