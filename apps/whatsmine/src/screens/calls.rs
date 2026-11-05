use std::sync::atomic::{AtomicI64, Ordering};

use dioxus::prelude::*;
use whatsmine_core::model::now_ms;
use whatsmine_core::rich::CallRecord;

use crate::app::{Rev, ToastSig, t, use_core, use_tr};
use crate::call::{self, CallBridge, CallUi};
use crate::components::{Avatar, BottomNav, Icon, IconName, Tab, Title};
use crate::fmt;
use crate::screens::people::Faces;

/// Until when the missed calls were seen, for the mark on the tab. Here and now only: a restart
/// shows them again, and that is fine.
static SEEN_AT: AtomicI64 = AtomicI64::new(0);

pub fn calls_seen_at() -> i64 {
    SEEN_AT.load(Ordering::Relaxed)
}

/// Calls that were, and favourite people to call.
#[component]
pub fn Calls() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    use_hook(|| SEEN_AT.store(now_ms(), Ordering::Relaxed));
    use_drop(|| SEEN_AT.store(now_ms(), Ordering::Relaxed));
    let _ = rev();
    let calls = core.call_records();
    let favourites: Vec<_> = core
        .chats()
        .into_iter()
        .filter(|c| c.prefs.favourite)
        .map(|c| c.contact)
        .collect();

    rsx! {
        div { class: "screen",
            Title { title: t(&tr, "calls.title") }
            div { class: "body",
                if !favourites.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"people.favourites\")}" }
                    Faces { people: favourites, to_call: true }
                }
                if calls.is_empty() {
                    div { class: "empty-state",
                        Icon { name: IconName::Phone, size: 56 }
                        p { class: "empty-title", "{t(&tr, \"calls.empty_title\")}" }
                        p { class: "empty-subtitle", "{t(&tr, \"calls.empty_subtitle\")}" }
                    }
                } else {
                    h2 { class: "gt", "{t(&tr, \"calls.recent\")}" }
                    div { class: "group",
                        for call in calls {
                            CallRow { key: "{call.id}", call }
                        }
                    }
                }
            }
            BottomNav { active: Tab::Calls }
        }
    }
}

#[component]
fn CallRow(call: CallRecord) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let bridge = use_context::<CallBridge>();
    let phase = use_context::<CallUi>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let contact = core.contact(&call.contact_id);
    let name = contact
        .as_ref()
        .map(|c| c.display_name())
        .unwrap_or_else(|| t(&tr, "calls.someone"));
    let is_group = whatsmine_core::groups::is_group_id(&call.contact_id);
    let missed_in = call.missed && !call.outgoing;
    let when = fmt::list_time(&tr, call.at);
    let note = if missed_in {
        t(&tr, "calls.tried").replace("{when}", &when)
    } else if call.missed {
        t(&tr, "calls.no_answer").replace("{when}", &when)
    } else {
        t(&tr, "calls.lasted")
            .replace("{when}", &when)
            .replace("{length}", &fmt::length(&tr, call.duration_ms))
    };
    let (open_id, call_id) = (call.contact_id.clone(), call.contact_id.clone());
    let video = call.video;
    let dir = if call.outgoing {
        IconName::CallOut
    } else {
        IconName::CallIn
    };
    rsx! {
        div { class: "row",
            div {
                class: "stack",
                style: "flex-direction:row;align-items:center;gap:12px;cursor:pointer",
                onclick: move |_| {
                    nav.push(crate::app::Route::Chat { id: open_id.clone() });
                },
                Avatar {
                    id: call.contact_id.clone(),
                    name: name.clone(),
                    size: "medium",
                    group: is_group,
                    image: contact.as_ref().and_then(|c| c.avatar.clone()),
                }
                span { class: "stack",
                    span { class: if missed_in { "row-t one-line danger-text" } else { "row-t one-line" }, "{name}" }
                    span { class: if missed_in { "mark sub danger-text" } else { "mark sub" },
                        Icon { name: dir, size: 14 }
                        span { class: "one-line", "{note}" }
                    }
                }
            }
            button {
                class: "ib accent-text",
                aria_label: "{t(&tr, \"calls.call_back\")}",
                onclick: move |_| {
                    call::start_call(&core, &call_id, video, phase, &bridge, &mut toast_sig, &tr);
                },
                Icon { name: if video { IconName::Video } else { IconName::Phone }, size: 22 }
            }
        }
    }
}
