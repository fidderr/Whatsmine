use dioxus::prelude::*;
use whatsmine_core::{ChatSummary, ContactKind, ContactState, MediaKind, MessageStatus};

use crate::app::{Rev, Route, Typing, t, use_core, use_tr};
use crate::components::{Avatar, BottomNav, Icon, IconName, NetBanner, Tab};
use crate::fmt;

#[component]
pub fn Chats() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let nav = use_navigator();
    let mut query = use_signal(String::new);

    let _ = rev();
    let all = core.chats();
    let q = query().trim().to_lowercase();
    let chats: Vec<ChatSummary> = all
        .iter()
        .filter(|c| {
            q.is_empty()
                || c.contact.display_name().to_lowercase().contains(&q)
                || c.contact.their_name.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();

    rsx! {
        div { class: "screen",
            div { class: "header",
                h1 { "{t(&tr, \"chats.title\")}" }
            }
            NetBanner {}
            if all.len() > 3 {
                div { class: "search",
                    Icon { name: IconName::Search, size: 18 }
                    input {
                        r#type: "search",
                        placeholder: "{t(&tr, \"chats.search\")}",
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                    }
                }
            }
            if all.is_empty() {
                div { class: "empty-state",
                    Icon { name: IconName::Chat, size: 56 }
                    p { class: "empty-title", "{t(&tr, \"chats.empty_title\")}" }
                    p { class: "empty-subtitle", "{t(&tr, \"chats.empty_subtitle\")}" }
                }
            } else if chats.is_empty() {
                div { class: "empty-state",
                    p { class: "empty-subtitle", "{t(&tr, \"chats.no_match\")}" }
                }
            } else {
                div { class: "list",
                    for c in chats {
                        ChatRow { key: "{c.contact.id}", chat: c }
                    }
                }
            }
            button {
                class: "fab secondary-fab",
                title: "{t(&tr, \"group.new\")}",
                onclick: move |_| {
                    nav.push(Route::NewGroup {});
                },
                Icon { name: IconName::Users, size: 22 }
            }
            button {
                class: "fab",
                title: "{t(&tr, \"add.title\")}",
                onclick: move |_| {
                    nav.push(Route::AddContact {});
                },
                Icon { name: IconName::UserPlus, size: 24 }
            }
            BottomNav { active: Tab::Chats }
        }
    }
}

#[component]
fn ChatRow(chat: ChatSummary) -> Element {
    let tr = use_tr();
    let core = use_core();
    let typing = use_context::<Typing>().0;
    let c = &chat.contact;
    let name = c.display_name();
    let is_group = c.kind == ContactKind::Group;
    let online = !is_group && core.is_online(&c.id);
    let is_typing = typing.read().contains_key(&c.id);
    // In a group, say who wrote the last message.
    let author = chat
        .last_message
        .as_ref()
        .and_then(|m| m.sender.as_ref())
        .map(|s| {
            core.contact(s)
                .map(|c| c.display_name())
                .unwrap_or_default()
        })
        .filter(|n| !n.is_empty());
    let time = chat
        .last_message
        .as_ref()
        .map(|m| fmt::list_time(&tr, m.sent_at))
        .unwrap_or_default();
    let unread = chat.unread;

    rsx! {
        Link { to: Route::Chat { id: c.id.clone() }, class: "card",
            Avatar {
                id: c.id.clone(),
                name: name.clone(),
                online,
                image: c.avatar.clone(),
                group: is_group,
            }
            div { class: "card-main",
                div { class: "card-row",
                    span { class: "card-title", "{name}" }
                    span { class: if unread > 0 { "card-time unread" } else { "card-time" }, "{time}" }
                }
                div { class: "card-row",
                    if is_typing {
                        span { class: "card-sub accent", "{t(&tr, \"chat.typing\")}" }
                    } else if c.state == ContactState::PendingOut && chat.last_message.is_none() {
                        span { class: "card-sub accent", "{t(&tr, \"chats.waiting\")}" }
                    } else if let Some(m) = chat.last_message.clone() {
                        span { class: "card-sub",
                            if m.outgoing {
                                StatusTicks { status: m.status }
                            }
                            if let Some(a) = author.clone() {
                                "{a}: "
                            }
                            Preview { message: m.clone() }
                        }
                    }
                    if unread > 0 {
                        span { class: "unread-badge", "{unread}" }
                    }
                }
            }
        }
    }
}

/// Clock (queued), ✓ (sent), ✓✓ (delivered), blue ✓✓ (read).
#[component]
pub fn StatusTicks(status: MessageStatus) -> Element {
    let (icon, class) = match status {
        MessageStatus::Queued => (IconName::Clock, "ticks"),
        MessageStatus::Sent => (IconName::Check, "ticks"),
        MessageStatus::Delivered => (IconName::CheckDouble, "ticks"),
        MessageStatus::Read => (IconName::CheckDouble, "ticks read"),
    };
    rsx! {
        span { class,
            Icon { name: icon, size: 15 }
        }
    }
}

/// One line for the chat list: the text, or an icon and name for a file.
#[component]
fn Preview(message: whatsmine_core::Message) -> Element {
    let tr = use_tr();
    let Some(att) = message.attachment.as_ref() else {
        return rsx! { "{message.body}" };
    };
    let (icon, label) = match att.kind() {
        MediaKind::Image => (IconName::Image, t(&tr, "media.photo")),
        MediaKind::Video => (IconName::Video, t(&tr, "media.video")),
        MediaKind::Audio => (IconName::Mic, t(&tr, "media.voice")),
        MediaKind::File => (IconName::File, att.name.clone()),
    };
    let text = if message.body.is_empty() {
        label
    } else {
        message.body.clone()
    };
    rsx! {
        Icon { name: icon, size: 15 }
        "{text}"
    }
}
