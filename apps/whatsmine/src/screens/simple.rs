//! Simple mode: your people, big buttons, plain words, and nothing else in the way.

use dioxus::prelude::*;
use whatsmine_core::{ChatSummary, ContactKind, ContactState};

use crate::app::{Rev, Route, ToastSig, t, toast, use_core, use_tr};
use crate::call::{self, CallBridge, CallUi};
use crate::components::scan::ScanOverlay;
use crate::components::{Avatar, Icon, IconName, QrCode, Sheet, ToastLevel, change, use_settings};
use crate::links::{self, InviteSig};
use crate::screens::add::{share_or_copy, use_scan};

/// Your people, the ones you marked as favourites first.
#[component]
pub fn SimpleHome() -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let settings = use_settings();
    let mut adding = use_signal(|| false);
    let mut leaving = use_signal(|| false);
    let _ = rev();
    let mut people: Vec<ChatSummary> = core
        .chats()
        .into_iter()
        .filter(|c| {
            c.contact.kind == ContactKind::Person && c.contact.state == ContactState::Accepted
        })
        .collect();
    people.sort_by_key(|c| !c.prefs.favourite);
    if adding() {
        return rsx! { SimpleAdd { on_close: move |_| adding.set(false) } };
    }
    rsx! {
        div { class: "screen no-nav simple",
            div { class: "ttl", h1 { "{t(&tr, \"simple.your_people\")}" } }
            div { class: "body",
                if people.is_empty() {
                    p { class: "note", style: "font-size:1.25rem", "{t(&tr, \"simple.nobody_yet\")}" }
                }
                for chat in people {
                    BigPerson { key: "{chat.contact.id}", chat }
                }
                button {
                    class: "btn big",
                    "data-t": "simple-add",
                    onclick: move |_| adding.set(true),
                    Icon { name: IconName::UserPlus, size: 24 }
                    "{t(&tr, \"add.title\")}"
                }
                button {
                    class: "btn",
                    style: "background:transparent",
                    "data-t": "simple-settings",
                    onclick: move |_| leaving.set(true),
                    "{t(&tr, \"nav.settings\")}"
                }
            }
            if leaving() {
                Sheet { on_close: move |_| leaving.set(false), title: t(&tr, "simple.leave_title"), tag: "simple-leave",
                    p { class: "note", "{t(&tr, \"simple.leave_text\")}" }
                    button {
                        class: "btn main big",
                        "data-t": "simple-off",
                        onclick: move |_| change(settings, |s| s.simple_mode = false),
                        "{t(&tr, \"simple.turn_off\")}"
                    }
                }
            }
        }
    }
}

#[component]
fn BigPerson(chat: ChatSummary) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let bridge = use_context::<CallBridge>();
    let phase = use_context::<CallUi>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let c = chat.contact.clone();
    let name = c.display_name();
    let news = if chat.unread > 0 {
        t(&tr, "simple.new_messages").replace("{n}", &chat.unread.to_string())
    } else {
        String::new()
    };
    let (open_id, call_id) = (c.id.clone(), c.id.clone());
    rsx! {
        section { class: "big-person", "data-t": "big-person",
            Avatar { id: c.id.clone(), name: name.clone(), size: "big", image: c.avatar.clone() }
            div { class: "stack", style: "gap:8px",
                div { style: "display:flex;align-items:baseline;gap:8px;min-width:0",
                    span { class: "big-name one-line", "{name}" }
                    if !news.is_empty() {
                        span { class: "big-news", "{news}" }
                    }
                }
                div { class: "btns",
                    button {
                        class: "btn main tall",
                        "data-t": "big-message",
                        onclick: move |_| {
                            nav.push(Route::Chat { id: open_id.clone() });
                        },
                        Icon { name: IconName::Chat, size: 22 }
                        "{t(&tr, \"simple.message\")}"
                    }
                    button {
                        class: "btn tall",
                        onclick: move |_| {
                            call::start_call(&core, &call_id, false, phase, &bridge, &mut toast_sig, &tr);
                        },
                        Icon { name: IconName::Phone, size: 22 }
                        "{t(&tr, \"simple.call\")}"
                    }
                }
            }
        }
    }
}

/// Adding someone, with big words for each step.
#[component]
fn SimpleAdd(on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let invite = use_context::<InviteSig>();
    let mut toast_sig = use_context::<ToastSig>();
    let toast_share = toast_sig;
    let (mut scanning, start_scan) = use_scan(invite);
    let mut showing = use_signal(|| false);
    let link = core.invite_link();
    let code = core.in_person_code();
    let share_text = t(&tr, "add.share_text").replace("{link}", &link);
    rsx! {
        div { class: "screen no-nav simple",
            header { class: "hdr",
                button {
                    class: "btn tall",
                    style: "background:transparent",
                    "data-t": "back",
                    onclick: move |_| on_close.call(()),
                    Icon { name: IconName::ArrowLeft }
                    "{t(&tr, \"common.back\")}"
                }
            }
            crate::back::BackLayer { on_back: move |_| on_close.call(()) }
            div { class: "body", style: "gap:16px;padding:0 16px",
                h1 { style: "font-size:1.75rem;line-height:1.3;font-weight:700", "{t(&tr, \"add.title\")}" }
                if showing() {
                    div { class: "panel center",
                        QrCode { data: code.clone() }
                        p { style: "font-size:1.25rem;line-height:1.4", "{t(&tr, \"simple.show_text\")}" }
                    }
                } else {
                    ol { class: "steps",
                        li { b { "1" } span { "{t(&tr, \"simple.step_1\")}" } }
                        li { b { "2" } span { "{t(&tr, \"simple.step_2\")}" } }
                    }
                }
                button {
                    class: "btn main bigger",
                    "data-t": "simple-scan",
                    onclick: move |_| start_scan.call(()),
                    Icon { name: IconName::Camera, size: 32 }
                    "{t(&tr, \"simple.scan\")}"
                }
                button {
                    class: "btn big",
                    "data-t": "simple-show",
                    onclick: move |_| showing.toggle(),
                    Icon { name: IconName::Qr, size: 28 }
                    "{t(&tr, \"simple.show\")}"
                }
                button {
                    class: "btn big",
                    "data-t": "simple-share",
                    onclick: move |_| share_or_copy(tr, toast_share, share_text.clone(), t(&tr, "add.share")),
                    Icon { name: IconName::Share, size: 28 }
                    "{t(&tr, \"simple.send_link\")}"
                }
            }
            if scanning() {
                ScanOverlay {
                    on_code: move |found: String| {
                        scanning.set(false);
                        if !links::offer(invite, &found) {
                            toast(&mut toast_sig, t(&tr, "add.scan_not_ours"), ToastLevel::Warning);
                        }
                    },
                    on_close: move |_| scanning.set(false),
                }
            }
        }
    }
}
