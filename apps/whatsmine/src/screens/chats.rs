use dioxus::prelude::*;
use whatsmine_core::{ChatSummary, ContactKind, ContactState, MediaKind, MessageStatus};

use crate::app::{Rev, Route, Typing, t, use_core, use_tr};
use crate::back::BackLayer;
use crate::components::{Avatar, BottomNav, ConfirmDialog, Icon, IconName, NetBanner, Tab};
use crate::fmt;

#[component]
pub fn Chats() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let nav = use_navigator();
    let mut query = use_signal(String::new);
    let mut menu: Signal<Option<ChatSummary>> = use_signal(|| None);

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
                        ChatRow {
                            key: "{c.contact.id}",
                            chat: c,
                            on_menu: move |c| menu.set(Some(c)),
                        }
                    }
                }
            }
            button {
                class: "fab",
                title: "{t(&tr, \"add.title\")}",
                onclick: move |_| {
                    nav.push(Route::AddContact {});
                },
                Icon { name: IconName::UserPlus, size: 24 }
            }
            button {
                class: "fab secondary-fab",
                title: "{t(&tr, \"group.new\")}",
                onclick: move |_| {
                    nav.push(Route::NewGroup {});
                },
                Icon { name: IconName::Users, size: 22 }
            }
            if let Some(chat) = menu() {
                ChatMenu { chat, on_close: move |_| menu.set(None) }
            }
            BottomNav { active: Tab::Chats }
        }
    }
}

/// What a long press (or right click) on a chat offers.
#[component]
fn ChatMenu(chat: ChatSummary, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut confirm = use_signal(|| false);
    let c = &chat.contact;
    let name = c.display_name();
    let (id, pinned, muted) = (c.id.clone(), c.pinned, c.muted);
    let (pin_core, mute_core, read_core, del_core) =
        (core.clone(), core.clone(), core.clone(), core.clone());
    let (pin_id, mute_id, read_id, del_id) = (id.clone(), id.clone(), id.clone(), id.clone());
    rsx! {
        div { class: "bottom-sheet-overlay", onclick: move |_| on_close.call(()),
            BackLayer {
                on_back: move |_| {
                    if confirm() {
                        confirm.set(false)
                    } else {
                        on_close.call(())
                    }
                },
            }
            div { class: "bottom-sheet", onclick: move |e| e.stop_propagation(),
                div { class: "sheet-title", "{name}" }
                if chat.unread > 0 {
                    button {
                        class: "sheet-option",
                        onclick: move |_| {
                            read_core.mark_read(&read_id);
                            crate::notify::mark_read(&read_id);
                            on_close.call(());
                        },
                        Icon { name: IconName::CheckDouble }
                        "{t(&tr, \"chats.mark_read\")}"
                    }
                }
                button {
                    class: "sheet-option",
                    onclick: move |_| {
                        pin_core.set_pinned(&pin_id, !pinned);
                        on_close.call(());
                    },
                    Icon { name: IconName::Pin }
                    if pinned {
                        "{t(&tr, \"chats.unpin\")}"
                    } else {
                        "{t(&tr, \"chats.pin\")}"
                    }
                }
                button {
                    class: "sheet-option",
                    onclick: move |_| {
                        mute_core.set_muted(&mute_id, !muted);
                        on_close.call(());
                    },
                    Icon {
                        name: if muted { IconName::Bell } else { IconName::BellOff },
                    }
                    if muted {
                        "{t(&tr, \"chats.unmute\")}"
                    } else {
                        "{t(&tr, \"chats.mute\")}"
                    }
                }
                button {
                    class: "sheet-option destructive",
                    onclick: move |_| confirm.set(true),
                    Icon { name: IconName::Trash }
                    "{t(&tr, \"chats.delete\")}"
                }
                button { class: "sheet-option cancel", onclick: move |_| on_close.call(()),
                    "{t(&tr, \"common.cancel\")}"
                }
            }
            if confirm() {
                ConfirmDialog {
                    title: t(&tr, "chats.delete_title").replace("{name}", &name),
                    message: t(&tr, "chats.delete_message").replace("{name}", &name),
                    confirm_label: t(&tr, "chats.delete"),
                    on_confirm: move |_| {
                        del_core.hide_chat(&del_id);
                        on_close.call(());
                    },
                    on_cancel: move |_| confirm.set(false),
                }
            }
        }
    }
}

#[component]
fn ChatRow(chat: ChatSummary, on_menu: EventHandler<ChatSummary>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    // Long press: a timer that any lift or movement cancels.
    let mut press = use_signal(|| 0u64);
    let mut long_fired = use_signal(|| false);
    let draft = core.draft(&chat.contact.id);
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
    let (open_id, press_chat, menu_chat) = (c.id.clone(), chat.clone(), chat.clone());

    rsx! {
        div {
            class: if c.pinned { "card pinned" } else { "card" },
            onclick: move |_| {
                if long_fired() {
                    long_fired.set(false);
                } else {
                    nav.push(Route::Chat {
                        id: open_id.clone(),
                    });
                }
            },
            oncontextmenu: move |e| {
                e.prevent_default();
                on_menu.call(menu_chat.clone());
            },
            ontouchstart: move |_| {
                let token = *press.peek() + 1;
                press.set(token);
                long_fired.set(false);
                let chat = press_chat.clone();
                spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    if *press.peek() == token {
                        long_fired.set(true);
                        on_menu.call(chat);
                    }
                });
            },
            ontouchend: move |_| *press.write() += 1,
            ontouchmove: move |_| *press.write() += 1,
            ontouchcancel: move |_| *press.write() += 1,
            Avatar {
                id: c.id.clone(),
                name: name.clone(),
                online,
                image: c.avatar.clone(),
                group: is_group,
                zoom: true,
            }
            div { class: "card-main",
                div { class: "card-row",
                    span { class: "card-title", "{name}" }
                    if c.muted {
                        span { class: "card-flag",
                            Icon { name: IconName::BellOff, size: 14 }
                        }
                    }
                    if c.pinned {
                        span { class: "card-flag",
                            Icon { name: IconName::Pin, size: 14 }
                        }
                    }
                    span { class: if unread > 0 { "card-time unread" } else { "card-time" }, "{time}" }
                }
                div { class: "card-row",
                    if is_typing {
                        span { class: "card-sub accent", "{t(&tr, \"chat.typing\")}" }
                    } else if !draft.trim().is_empty() {
                        span { class: "card-sub",
                            span { class: "draft-label", "{t(&tr, \"chats.draft\")} " }
                            "{draft}"
                        }
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
                        span { class: if c.muted { "unread-badge muted" } else { "unread-badge" }, "{unread}" }
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
        MessageStatus::NotDelivered | MessageStatus::Queued => (IconName::Clock, "ticks"),
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
