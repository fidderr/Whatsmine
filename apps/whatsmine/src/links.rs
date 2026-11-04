//! Invitations: a `whatsmine://add/...` link (clicked, scanned or pasted)
//! asks "Add this person?" before anything happens.

use dioxus::prelude::*;
use whatsmine_core::ContactState;
use whatsmine_core::identity::{Invite, key_to_string, parse_invite};

use crate::app::{Route, ToastSig, t, toast, use_core, use_tr};
use crate::back::BackLayer;
use crate::components::{Avatar, ToastLevel};
use crate::fmt;

/// The invitation waiting for an answer, if any.
#[derive(Clone, Copy)]
pub struct InviteSig(pub Signal<Option<Invite>>);

/// Looks for an invitation in `text` and, if there is one, asks about it.
/// False if the text holds none.
pub fn offer(mut sig: InviteSig, text: &str) -> bool {
    match parse_invite(text) {
        Some(invite) => {
            sig.0.set(Some(invite));
            true
        }
        None => false,
    }
}

/// The question: add this person?
#[component]
pub fn InviteDialog() -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let mut sig = use_context::<InviteSig>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let Some(invite) = sig.read().clone() else {
        return rsx! {};
    };

    let Some(person) = invite.key() else {
        return rsx! {};
    };
    let key = key_to_string(&person);
    let invite_text = invite.link();
    let me = core.my_key() == key;
    let known = core
        .contact(&key)
        .filter(|c| c.state != ContactState::Blocked && c.state != ContactState::PendingIn);
    let blocked = core
        .contact(&key)
        .is_some_and(|c| c.state == ContactState::Blocked);
    let name = invite
        .name()
        .map(str::to_owned)
        .or_else(|| core.contact(&key).map(|c| c.display_name()))
        .unwrap_or_else(|| t(&tr, "invite.someone"));

    let title = if me {
        t(&tr, "invite.me")
    } else if known.is_some() {
        t(&tr, "invite.known").replace("{name}", &name)
    } else if blocked {
        t(&tr, "invite.blocked").replace("{name}", &name)
    } else {
        t(&tr, "invite.title").replace("{name}", &name)
    };
    let body = if me || known.is_some() || blocked {
        String::new()
    } else {
        t(&tr, "invite.body")
    };

    let open_key = key.clone();
    let (add_name, add_core) = (invite.name.clone(), core.clone());
    rsx! {
        div { class: "confirm-dialog-overlay", onclick: move |_| sig.set(None),
            BackLayer { on_back: move |_| sig.set(None) }
            div { class: "confirm-dialog invite-dialog", onclick: move |e| e.stop_propagation(),
                div { class: "invite-avatar",
                    Avatar { id: key.clone(), name: name.clone(), size: "large" }
                }
                h3 { "{title}" }
                if !body.is_empty() {
                    p { "{body}" }
                }
                div { class: "key-box small", "{fmt::grouped_key(&key)}" }
                div { class: "confirm-actions",
                    button { class: "btn-cancel", onclick: move |_| sig.set(None),
                        "{t(&tr, \"common.cancel\")}"
                    }
                    if known.is_some() {
                        button {
                            class: "btn-confirm",
                            onclick: move |_| {
                                sig.set(None);
                                nav.push(Route::Chat { id: open_key.clone() });
                            },
                            "{t(&tr, \"invite.open\")}"
                        }
                    } else if !me && !blocked {
                        button {
                            class: "btn-confirm",
                            onclick: move |_| {
                                sig.set(None);
                                match add_core.add_contact(&invite_text, (!add_name.is_empty()).then_some(add_name.as_str())) {
                                    Ok(c) => {
                                        nav.push(Route::Chat { id: c.id });
                                    }
                                    Err(e) => toast(
                                        &mut toast_sig,
                                        t(&tr, &format!("add.err.{e}")),
                                        ToastLevel::Error,
                                    ),
                                }
                            },
                            "{t(&tr, \"invite.add\")}"
                        }
                    }
                }
            }
        }
    }
}
