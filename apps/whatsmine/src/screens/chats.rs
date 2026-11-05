use dioxus::prelude::*;
use whatsmine_core::model::now_ms;
use whatsmine_core::rich::Folder;
use whatsmine_core::{ChatSummary, ContactKind, ContactState, MediaKind, MessageStatus};

use crate::app::{Rev, Route, Typing, t, use_core, use_tr};
use crate::components::{
    Avatar, BottomNav, ConfirmDialog, Icon, IconName, NetBanner, Notice, Sheet, SheetItem, Tab,
    Title, use_settings,
};
use crate::fmt;
use crate::screens::simple::SimpleHome;

const HOUR: i64 = 3_600_000;

/// What the list shows: chips above it choose.
#[derive(Clone, PartialEq)]
enum Filter {
    All,
    Unread,
    Groups,
    Archived,
    Folder(Folder),
}

#[component]
pub fn Chats() -> Element {
    let settings = use_settings();
    let wide = use_context::<crate::shell::Wide>().0;
    crate::js::use_scroll_top();
    if settings.read().simple_mode {
        return rsx! { SimpleHome {} };
    }
    // Side by side, the list is already there: this part says what to do.
    if wide() {
        let tr = use_tr();
        return rsx! {
            div { class: "wide-empty",
                Icon { name: IconName::Chat, size: 56 }
                p { class: "empty-title", "{t(&tr, \"wide.pick\")}" }
                p { class: "empty-subtitle", "{t(&tr, \"wide.pick_text\")}" }
            }
        };
    }
    rsx! {
        div { class: "screen",
            ChatList { current: None::<String> }
            BottomNav { active: Tab::Chats }
        }
    }
}

/// Whether a chat belongs in the plain list right now.
fn on_the_list(c: &ChatSummary, now: i64) -> bool {
    !c.prefs.archived && !c.prefs.locked && c.prefs.snoozed_until.is_none_or(|until| until <= now)
}

/// The chat list, with what goes on top of it.
#[component]
pub fn ChatList(current: Option<String>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let nav = use_navigator();
    let mut filter = use_signal(|| Filter::All);
    let mut menu: Signal<Option<ChatSummary>> = use_signal(|| None);

    let _ = rev();
    let all = core.chats();
    let now = now_ms();
    let folders = core.folders();
    let unread_count = all
        .iter()
        .filter(|c| on_the_list(c, now) && (c.unread > 0 || c.prefs.marked_unread))
        .count();
    let archived_count = all.iter().filter(|c| c.prefs.archived).count();
    let requests = core.requests().len();
    let chosen = filter();
    let chats: Vec<ChatSummary> = match &chosen {
        Filter::All => all
            .iter()
            .filter(|c| on_the_list(c, now))
            .cloned()
            .collect(),
        Filter::Unread => all
            .iter()
            .filter(|c| on_the_list(c, now) && (c.unread > 0 || c.prefs.marked_unread))
            .cloned()
            .collect(),
        Filter::Groups => all
            .iter()
            .filter(|c| on_the_list(c, now) && c.contact.kind == ContactKind::Group)
            .cloned()
            .collect(),
        Filter::Archived => all.iter().filter(|c| c.prefs.archived).cloned().collect(),
        Filter::Folder(folder) => core
            .chats_in(folder)
            .into_iter()
            .filter(|c| on_the_list(c, now))
            .collect(),
    };
    let chip = |f: &Filter| {
        if *f == chosen { "chip on" } else { "chip" }
    };
    let want_label = t(&tr, "chats.want_to_add").replace("{n}", &requests.to_string());

    rsx! {
        Title { title: t(&tr, "chats.title"),
            Link {
                to: Route::SearchScreen {},
                class: "ib r",
                "data-t": "search",
                aria_label: "{t(&tr, \"chats.search\")}",
                Icon { name: IconName::Search, size: 20 }
            }
            Link {
                to: Route::FoldersScreen {},
                class: "ib r",
                "data-t": "folders",
                aria_label: "{t(&tr, \"folders.title\")}",
                Icon { name: IconName::Folder, size: 20 }
            }
        }
        div { class: "chips scroll",
            button { class: chip(&Filter::All), onclick: move |_| filter.set(Filter::All),
                "{t(&tr, \"chats.filter_all\")}"
            }
            button { class: chip(&Filter::Unread), onclick: move |_| filter.set(Filter::Unread),
                "{t(&tr, \"chats.filter_unread\")}"
                if unread_count > 0 {
                    span { class: if chosen == Filter::Unread { "" } else { "accent-text" }, "{unread_count}" }
                }
            }
            button { class: chip(&Filter::Groups), onclick: move |_| filter.set(Filter::Groups),
                "{t(&tr, \"chats.filter_groups\")}"
            }
            for folder in folders {
                {
                    let f = Filter::Folder(folder.clone());
                    let class = chip(&f);
                    rsx! {
                        button {
                            key: "{folder.name}",
                            class,
                            onclick: move |_| filter.set(f.clone()),
                            "{folder.name}"
                        }
                    }
                }
            }
        }
        div { class: "body",
            NetBanner {}
            if requests > 0 {
                Notice {
                    icon: IconName::UserPlus,
                    text: want_label,
                    to: Route::Requests {},
                    tag: "requests-notice",
                }
            }
            if all.is_empty() {
                div { class: "empty-state",
                    Icon { name: IconName::Chat, size: 56 }
                    p { class: "empty-title", "{t(&tr, \"chats.empty_title\")}" }
                    p { class: "empty-subtitle", "{t(&tr, \"chats.empty_subtitle\")}" }
                    Link { to: Route::AddContact {}, class: "btn main", "{t(&tr, \"add.title\")}" }
                }
            } else if chats.is_empty() {
                div { class: "empty-state",
                    p { class: "empty-subtitle", "{t(&tr, \"chats.no_match\")}" }
                }
            } else {
                div { class: "group chat-list",
                    for c in chats {
                        ChatRow {
                            key: "{c.contact.id}",
                            current: current.as_deref() == Some(c.contact.id.as_str()),
                            chat: c,
                            on_menu: move |c| menu.set(Some(c)),
                        }
                    }
                }
            }
            if archived_count > 0 && chosen != Filter::Archived {
                div { class: "group",
                    button {
                        class: "row sm",
                        "data-t": "archive",
                        onclick: move |_| filter.set(Filter::Archived),
                        span { class: "dim", style: "display:flex",
                            Icon { name: IconName::Archive, size: 20 }
                        }
                        span { class: "row-t stack", "{t(&tr, \"chats.archive\")}" }
                        span { class: "val", "{archived_count}" }
                    }
                }
            }
        }
        button {
            class: "fab",
            title: "{t(&tr, \"chats.new\")}",
            "data-t": "fab",
            onclick: move |_| {
                nav.push(Route::People {});
            },
            Icon { name: IconName::Plus, size: 24 }
        }
        if let Some(chat) = menu() {
            ChatMenu { chat, on_close: move |_| menu.set(None) }
        }
    }
}

/// What a long press (or right click) on a chat offers.
#[component]
fn ChatMenu(chat: ChatSummary, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut confirm = use_signal(|| false);
    let mut snoozing = use_signal(|| false);
    let c = &chat.contact;
    let name = c.display_name();
    let (id, pinned, muted) = (c.id.clone(), c.pinned, c.muted);
    let unread = chat.unread > 0 || chat.prefs.marked_unread;
    let archived = chat.prefs.archived;
    let prefs = chat.prefs.clone();

    let with_prefs = {
        let (core, id, prefs) = (core.clone(), id.clone(), prefs.clone());
        move |f: &dyn Fn(&mut whatsmine_core::rich::ChatPrefs)| {
            let mut p = prefs.clone();
            f(&mut p);
            core.set_chat_prefs(&id, &p);
        }
    };
    let (for_unread, for_archive, for_snooze, for_hide) = (
        with_prefs.clone(),
        with_prefs.clone(),
        with_prefs.clone(),
        with_prefs,
    );
    let (read_core, pin_core, mute_core, del_core) =
        (core.clone(), core.clone(), core.clone(), core.clone());
    let (read_id, pin_id, mute_id, del_id) = (id.clone(), id.clone(), id.clone(), id.clone());

    if snoozing() {
        let hours: [(&str, i64); 3] = [
            ("chats.snooze_hour", 1),
            ("chats.snooze_evening", hours_until(20)),
            ("chats.snooze_tomorrow", hours_until(8)),
        ];
        return rsx! {
            Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "chats.snooze"),
                div { class: "sheet-list",
                    for (key, h) in hours {
                        {
                            let snooze = for_snooze.clone();
                            rsx! {
                                SheetItem {
                                    key: "{key}",
                                    label: t(&tr, key),
                                    icon: IconName::Moon,
                                    on_tap: move |_| {
                                        snooze(&|p| p.snoozed_until = Some(now_ms() + h * HOUR));
                                        on_close.call(());
                                    },
                                }
                            }
                        }
                    }
                }
            }
        };
    }

    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: name.clone(),
            div { class: "sheet-list",
                if unread {
                    SheetItem {
                        label: t(&tr, "chats.mark_read"),
                        icon: IconName::CheckDouble,
                        on_tap: move |_| {
                            read_core.mark_read(&read_id);
                            crate::notify::mark_read(&read_id);
                            for_unread(&|p| p.marked_unread = false);
                            on_close.call(());
                        },
                    }
                } else {
                    SheetItem {
                        label: t(&tr, "chats.mark_unread"),
                        icon: IconName::Chat,
                        on_tap: move |_| {
                            for_unread(&|p| p.marked_unread = true);
                            on_close.call(());
                        },
                    }
                }
                SheetItem {
                    label: t(&tr, if pinned { "chats.unpin" } else { "chats.pin" }),
                    icon: IconName::Pin,
                    tag: "chat-pin",
                    on_tap: move |_| {
                        pin_core.set_pinned(&pin_id, !pinned);
                        on_close.call(());
                    },
                }
                SheetItem {
                    label: t(&tr, if muted { "chats.unmute" } else { "chats.mute" }),
                    icon: if muted { IconName::Bell } else { IconName::BellOff },
                    on_tap: move |_| {
                        mute_core.set_muted(&mute_id, !muted);
                        on_close.call(());
                    },
                }
                SheetItem {
                    label: t(&tr, "chats.snooze"),
                    icon: IconName::Moon,
                    on_tap: move |_| snoozing.set(true),
                }
                SheetItem {
                    label: t(&tr, if archived { "chats.unarchive" } else { "chats.to_archive" }),
                    icon: IconName::Archive,
                    on_tap: move |_| {
                        for_archive(&|p| p.archived = !archived);
                        on_close.call(());
                    },
                }
                SheetItem {
                    label: t(&tr, "chats.hide"),
                    icon: IconName::Lock,
                    tag: "chat-hide",
                    on_tap: move |_| {
                        for_hide(&|p| p.locked = true);
                        on_close.call(());
                    },
                }
                SheetItem {
                    label: t(&tr, "chats.delete"),
                    icon: IconName::Trash,
                    danger: true,
                    on_tap: move |_| confirm.set(true),
                }
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

/// How many hours until this hour of the day comes around.
fn hours_until(hour: u32) -> i64 {
    use chrono::{Local, Timelike};
    let now = Local::now().hour() as i64;
    let wait = (i64::from(hour) - now).rem_euclid(24);
    if wait == 0 { 24 } else { wait }
}

#[component]
fn ChatRow(chat: ChatSummary, current: bool, on_menu: EventHandler<ChatSummary>) -> Element {
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
    let marked = chat.prefs.marked_unread && chat.unread == 0;
    let unread = chat.unread;
    let highlight = unread > 0 || marked;
    let (open_id, press_chat, menu_chat) = (c.id.clone(), chat.clone(), chat.clone());

    rsx! {
        button {
            class: if current { "chat-row current" } else { "chat-row" },
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
            }
            div { class: "stack",
                div { class: "chat-line",
                    span { class: "chat-name", "{name}" }
                    span { class: if highlight { "chat-time unread" } else { "chat-time" }, "{time}" }
                }
                div { class: "chat-line",
                    if is_typing {
                        span { class: "chat-preview live", span { "{t(&tr, \"chat.typing\")}" } }
                    } else if !draft.trim().is_empty() {
                        span { class: "chat-preview",
                            span {
                                span { class: "draft-mark", "{t(&tr, \"chats.draft\")} " }
                                "{draft}"
                            }
                        }
                    } else if c.state == ContactState::PendingOut && chat.last_message.is_none() {
                        span { class: "chat-preview live", span { "{t(&tr, \"chats.waiting\")}" } }
                    } else if let Some(m) = chat.last_message.clone() {
                        span { class: "chat-preview",
                            if m.outgoing {
                                StatusTicks { status: m.status }
                            }
                            span {
                                if let Some(a) = author.clone() {
                                    "{a}: "
                                }
                                Preview { message: m.clone() }
                            }
                        }
                    } else {
                        span { class: "chat-preview" }
                    }
                    if c.muted {
                        span { class: "chat-flag",
                            Icon { name: IconName::BellOff, size: 14 }
                        }
                    }
                    if c.pinned {
                        span { class: "chat-flag",
                            Icon { name: IconName::Pin, size: 14 }
                        }
                    }
                    if unread > 0 {
                        span { class: if c.muted { "badge muted" } else { "badge" }, "{unread}" }
                    } else if marked {
                        span { class: "badge" }
                    }
                }
            }
        }
    }
}

/// Clock (still on its way), ✓ (sent), ✓✓ (delivered), blue ✓✓ (read), a warning (stopped trying).
#[component]
pub fn StatusTicks(status: MessageStatus) -> Element {
    let (icon, class) = match status {
        MessageStatus::NotDelivered => (IconName::Warning, "ticks failed"),
        MessageStatus::Scheduled | MessageStatus::Queued => (IconName::Clock, "ticks"),
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
pub fn Preview(message: whatsmine_core::Message) -> Element {
    let tr = use_tr();
    if message.extra.unsent {
        return rsx! { "{t(&tr, \"chat.unsent_short\")}" };
    }
    let Some(att) = message.attachment.as_ref() else {
        let line = fmt::plain(&message.body);
        return rsx! { "{line}" };
    };
    let (key, name) = match att.kind() {
        MediaKind::Image => ("chats.photo", None),
        MediaKind::Video => ("chats.video", None),
        MediaKind::Audio => ("chats.audio", None),
        MediaKind::File => ("chats.file", Some(att.name.clone())),
    };
    let label = name.unwrap_or_else(|| t(&tr, key));
    if message.body.is_empty() {
        rsx! { "{label}" }
    } else {
        let line = fmt::plain(&message.body);
        rsx! { "{label}: {line}" }
    }
}
