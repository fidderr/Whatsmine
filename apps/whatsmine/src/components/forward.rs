use dioxus::prelude::*;
use whatsmine_core::{Contact, ContactKind, ContactState, Message};

use crate::app::{ToastSig, t, toast, use_core, use_tr};
use crate::back::BackLayer;
use crate::components::{Avatar, Icon, IconName, ToastLevel};
use crate::media::{self, Picked};

/// "Forward to…": pick who gets a copy of a message (text or file).
#[component]
pub fn ForwardSheet(message: Message, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let mut query = use_signal(String::new);

    // Chats first (most recent), then everyone else in the address book.
    let mut targets: Vec<Contact> = core
        .chats()
        .into_iter()
        .map(|c| c.contact)
        .filter(|c| c.state == ContactState::Accepted)
        .collect();
    for c in core.address_book() {
        if c.state == ContactState::Accepted && !targets.iter().any(|t| t.id == c.id) {
            targets.push(c);
        }
    }
    let q = query().trim().to_lowercase();
    targets.retain(|c| q.is_empty() || c.display_name().to_lowercase().contains(&q));

    let send_to = std::rc::Rc::new(move |contact: Contact| {
        let (core, message) = (core.clone(), message.clone());
        on_close.call(());
        dioxus::core::spawn_forever(async move {
            let name = contact.display_name();
            let errors = match message.attachment.as_ref().and_then(|a| {
                a.path.as_ref().map(|p| Picked {
                    path: p.into(),
                    name: a.name.clone(),
                    mime: a.mime.clone(),
                    duration_ms: a.duration_ms,
                })
            }) {
                Some(file) => {
                    media::send_files(core, contact.id, vec![file], message.body.clone()).await
                }
                None => match core.send(&contact.id, &message.body) {
                    Ok(_) => Vec::new(),
                    Err(e) => vec![e.to_string()],
                },
            };
            match errors.first() {
                None => toast(
                    &mut toast_sig,
                    t(&tr, "forward.done").replace("{name}", &name),
                    ToastLevel::Info,
                ),
                Some(e) => toast(
                    &mut toast_sig,
                    t(&tr, &format!("chat.err.{e}")),
                    ToastLevel::Error,
                ),
            }
        });
    });

    rsx! {
        div { class: "bottom-sheet-overlay", onclick: move |_| on_close.call(()),
            BackLayer { on_back: move |_| on_close.call(()) }
            div { class: "bottom-sheet forward-sheet", onclick: move |e| e.stop_propagation(),
                div { class: "sheet-title", "{t(&tr, \"forward.title\")}" }
                div { class: "search",
                    Icon { name: IconName::Search, size: 18 }
                    input {
                        r#type: "search",
                        placeholder: "{t(&tr, \"contacts.search\")}",
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                    }
                }
                div { class: "forward-list",
                    if targets.is_empty() {
                        p { class: "empty-subtitle", "{t(&tr, \"chats.no_match\")}" }
                    }
                    for c in targets {
                        button {
                            key: "{c.id}",
                            class: "sheet-option",
                            onclick: {
                                let (c, send) = (c.clone(), send_to.clone());
                                move |_| send(c.clone())
                            },
                            Avatar {
                                id: c.id.clone(),
                                name: c.display_name(),
                                size: "small",
                                image: c.avatar.clone(),
                                group: c.kind == ContactKind::Group,
                            }
                            "{c.display_name()}"
                        }
                    }
                }
                button { class: "sheet-option cancel", onclick: move |_| on_close.call(()),
                    "{t(&tr, \"common.cancel\")}"
                }
            }
        }
    }
}
