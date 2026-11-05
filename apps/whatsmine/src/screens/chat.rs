use dioxus::prelude::*;
use whatsmine_core::model::now_ms;
use whatsmine_core::rich::MessageExtra;
use whatsmine_core::{ContactKind, ContactState, Core, Message, MessageStatus};

use crate::app::{
    MsgRev, Rev, Route, SettingsSig, ToastSig, Translations, Typing, t, toast, use_core, use_tr,
};
use crate::call::{self, CallBridge, CallUi, Phase};
use crate::components::{
    AttachmentView, Avatar, CameraMode, CameraOverlay, ForwardSheet, Icon, IconName, ImageViewer,
    Sheet, SheetItem, ToastLevel, VoiceBar, open_file, save_file,
};
use crate::fmt;
use crate::js;
use crate::links::{self, InviteSig};
use crate::look;
use crate::media::{self, Picked, VoiceStart};
use crate::screens::chat_rich::{
    ComposeKind, ComposeSheet, DeliverySheet, RichView, SendLaterSheet,
};
use crate::screens::chats::{Preview, StatusTicks};
use crate::screens::contact::DisappearSheet;

const PAGE: u32 = 100;
const MOBILE: bool = cfg!(any(target_os = "android", target_os = "ios"));

/// Emoji for the full picker: the ones people use. The system keyboard has the rest.
const EMOJI: &[&str] = &[
    "😀", "😂", "🥲", "😍", "😘", "😎", "🤔", "😴", "😢", "😭", "😡", "😱", "🥳", "🤗", "🙄", "🤝",
    "👍", "👎", "👏", "🙏", "💪", "👋", "👀", "🎉", "❤️", "🧡", "💛", "💚", "💙", "💜", "🔥", "✨",
    "⭐", "✅", "❌", "☕", "🍕", "🍺", "🎂", "⚽",
];

#[derive(Clone, Copy, PartialEq)]
enum Attach {
    Gallery,
    File,
    Photo,
    Video,
}

#[component]
pub fn Chat(id: String) -> Element {
    // Keyed so switching chats starts with fresh state (draft, paging).
    rsx! {
        ChatView { key: "{id}", id: id.clone() }
    }
}

/// Sends files and reports the first problem, if any.
async fn send_and_report(
    core: Core,
    id: String,
    files: Vec<Picked>,
    caption: String,
    tr: Translations,
    mut toast_sig: ToastSig,
) {
    // Copying and hashing a big file takes a few seconds before it shows up
    // in the chat; say so, so it does not look like nothing happened.
    if media::total_size(&files) > 16 * 1024 * 1024 {
        let name = files.first().map(|f| f.name.clone()).unwrap_or_default();
        toast(
            &mut toast_sig,
            t(&tr, "attach.preparing").replace("{name}", &name),
            ToastLevel::Info,
        );
    }
    let errors = media::send_files(core, id, files, caption).await;
    if let Some(e) = errors.first() {
        toast(
            &mut toast_sig,
            t(&tr, &format!("chat.err.{e}")),
            ToastLevel::Error,
        );
    }
}

/// A chat as plain text, oldest first.
fn export_text(core: &Core, chat: &str, tr: &Translations) -> String {
    let me = core.settings().display_name;
    let them = core
        .contact(chat)
        .map(|c| c.display_name())
        .unwrap_or_default();
    let mut lines: Vec<String> = core
        .messages(chat, None, u32::MAX)
        .into_iter()
        .map(|m| {
            let who = if m.outgoing {
                me.clone()
            } else {
                m.sender
                    .as_ref()
                    .and_then(|s| core.contact(s))
                    .map_or_else(|| them.clone(), |c| c.display_name())
            };
            let what = match &m.attachment {
                Some(a) if m.body.is_empty() => format!("[{}]", a.name),
                Some(a) => format!("[{}] {}", a.name, m.body),
                None => m.body.clone(),
            };
            format!("{} {who}: {what}", fmt::when(tr, m.sent_at))
        })
        .collect();
    lines.push(String::new());
    lines.join("\n")
}

/// Who is named with an @ in this text.
fn mentions_in(text: &str, members: &[(String, String)]) -> Vec<String> {
    members
        .iter()
        .filter(|(_, name)| !name.is_empty() && text.contains(&format!("@{name}")))
        .map(|(id, _)| id.clone())
        .collect()
}

#[component]
fn ChatView(id: String) -> Element {
    // While it is open, whoever it is with stays worth trying.
    {
        let (core, open) = (use_core(), id.clone());
        use_hook(move || core.set_chat_open(Some(&open)));
        let core = use_core();
        use_drop(move || core.set_chat_open(None));
    }
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let msg_rev = use_context::<MsgRev>().0;
    let typing = use_context::<Typing>().0;
    let settings = use_context::<SettingsSig>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let bridge = use_context::<CallBridge>();
    let call_phase = use_context::<CallUi>().0;
    let nav = use_navigator();

    let mut limit = use_signal(|| PAGE);
    let mut draft = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.draft(&id))
    };
    let mut menu: Signal<Option<Message>> = use_signal(|| None);
    let mut viewer: Signal<Option<String>> = use_signal(|| None);
    let mut forward: Signal<Option<Message>> = use_signal(|| None);
    let mut sheet = use_signal(|| false);
    let mut more = use_signal(|| false);
    let mut recording = use_signal(|| false);
    let mut camera: Signal<Option<CameraMode>> = use_signal(|| None);
    let mut reply_to: Signal<Option<Message>> = use_signal(|| None);
    let mut editing: Signal<Option<Message>> = use_signal(|| None);
    let mut compose: Signal<Option<ComposeKind>> = use_signal(|| None);
    let mut delivery: Signal<Option<String>> = use_signal(|| None);
    let mut send_later = use_signal(|| false);
    let mut show_scheduled = use_signal(|| false);
    let mut searching = use_signal(|| false);
    let mut disappear = use_signal(|| false);
    let mut query = use_signal(String::new);
    let mut selection: Signal<Option<Vec<String>>> = use_signal(|| None);
    let mut away_from_newest = use_signal(|| false);
    use_future(move || async move {
        let mut eval = document::eval(
            "const m = document.getElementById('msgs');\n\
             if (m) m.addEventListener('scroll', () => dioxus.send(m.scrollHeight - m.scrollTop - m.clientHeight > 400));\n\
             await new Promise(() => {});",
        );
        while let Ok(away) = eval.recv::<bool>().await {
            if away != *away_from_newest.peek() {
                away_from_newest.set(away);
            }
        }
    });

    // Read while open: on mount and whenever messages change.
    {
        let (core, id) = (core.clone(), id.clone());
        use_effect(move || {
            let _ = msg_rev();
            core.mark_read(&id);
            crate::notify::mark_read(&id);
            let mut prefs = core.chat_prefs(&id);
            if prefs.marked_unread {
                prefs.marked_unread = false;
                core.set_chat_prefs(&id, &prefs);
            }
        });
    }
    // Keep the newest message in view (not when paging back).
    use_effect(move || {
        let _ = msg_rev();
        if *limit.peek() == PAGE {
            js::scroll_to_bottom("msgs");
        }
    });
    use_hook(|| {
        if !MOBILE {
            js::focus("composer");
        }
    });

    let _ = rev();
    let Some(contact) = core.contact(&id) else {
        return rsx! {
            div { class: "screen no-nav",
                header { class: "hdr", crate::components::BackButton {} }
                div { class: "empty-state",
                    p { class: "empty-title", "{t(&tr, \"chat.not_found\")}" }
                }
            }
        };
    };
    let _ = msg_rev();
    let messages = core.messages(&id, None, limit());
    let has_more = messages.len() as u32 >= limit();
    let name = contact.display_name();
    let is_group = contact.kind == ContactKind::Group;
    let group_info = if is_group { core.group_info(&id) } else { None };
    let can_write = !is_group || group_info.as_ref().is_some_and(|g| g.active);
    let only_admins = group_info
        .as_ref()
        .is_some_and(|g| g.active && g.rules.only_admins_send && !g.i_am_admin);
    let call_ready = if is_group {
        can_write
    } else {
        contact.state == ContactState::Accepted && id != core.my_key()
    };
    let ongoing_call = if is_group { core.group_call(&id) } else { None };
    let group_status = group_info.as_ref().map(|g| {
        if g.active {
            t(&tr, "group.status")
                .replace("{n}", &g.members.len().to_string())
                .replace(
                    "{online}",
                    &g.members
                        .iter()
                        .filter(|m| m.online || m.is_me)
                        .count()
                        .to_string(),
                )
        } else {
            t(&tr, "group.inactive_short")
        }
    });
    let online = !is_group && core.is_online(&id);
    let is_typing = typing.read().contains_key(&id);
    let s = settings();
    let show_last_seen = s.share_last_seen;
    let prefs = core.chat_prefs(&id);
    // Its own look brings its own colours. Without one, it has what the app has.
    let own_look = core.chat_look(&id);
    let has_own_look = own_look.is_some();
    let style = look::look_style(&own_look.unwrap_or_else(|| core.app_look()), has_own_look);
    let scheduled: Vec<Message> = core
        .scheduled()
        .into_iter()
        .filter(|m| m.contact_id == id)
        .collect();
    let pinned: Option<Message> = prefs
        .pinned_messages
        .iter()
        .rev()
        .find_map(|p| core.message(p));

    let (status, live) = if let Some(g) = group_status {
        (g, false)
    } else if is_typing {
        (t(&tr, "chat.typing"), true)
    } else if contact.state == ContactState::PendingOut {
        (t(&tr, "chat.waiting"), false)
    } else if online {
        (t(&tr, "chat.online"), true)
    } else if let (true, Some(ts)) = (show_last_seen, contact.last_seen) {
        (fmt::last_seen(&tr, ts), false)
    } else {
        (String::new(), false)
    };

    let members: Vec<(String, String)> = group_info
        .as_ref()
        .map(|g| {
            g.members
                .iter()
                .filter(|m| !m.is_me)
                .map(|m| (m.id.clone(), m.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    // Who the @ being typed could be.
    let mention_word = draft
        .read()
        .rsplit([' ', '\n'])
        .next()
        .filter(|w| w.starts_with('@'))
        .map(|w| w[1..].to_lowercase());
    let mention_options: Vec<String> = match &mention_word {
        Some(word) => members
            .iter()
            .filter(|(_, n)| n.to_lowercase().starts_with(word.as_str()) && !n.is_empty())
            .map(|(_, n)| n.clone())
            .collect(),
        None => Vec::new(),
    };

    let send = {
        let (core, id, tr, members) = (core.clone(), id.clone(), tr, members.clone());
        move |silent: bool, at: Option<i64>| {
            let text = draft.peek().clone();
            if text.trim().is_empty() {
                return;
            }
            let being_edited = editing.peek().clone();
            if let Some(m) = being_edited {
                if let Err(e) = core.edit_message(&m.id, &text) {
                    toast(
                        &mut toast_sig,
                        t(&tr, &format!("chat.err.{e}")),
                        ToastLevel::Error,
                    );
                }
                editing.set(None);
                draft.set(String::new());
                core.set_draft(&id, "");
                return;
            }
            let extra = MessageExtra {
                reply_to: reply_to.peek().as_ref().map(|m| m.id.clone()),
                mentions: mentions_in(&text, &members),
                silent,
                ..MessageExtra::default()
            };
            match core.send_with(&id, &text, extra, at) {
                Ok(_) => {
                    draft.set(String::new());
                    reply_to.set(None);
                    limit.set(PAGE);
                }
                Err(e) => toast(
                    &mut toast_sig,
                    t(&tr, &format!("chat.err.{e}")),
                    ToastLevel::Error,
                ),
            }
        }
    };
    let mut send_click = send.clone();
    let mut send_key = send.clone();
    let mut send_quiet = send.clone();
    let mut send_at = send;

    // Whatever is typed goes along as the caption of the first file.
    let take_caption = {
        let (core, id) = (core.clone(), id.clone());
        move || {
            let caption = draft.peek().clone();
            if !caption.trim().is_empty() {
                draft.set(String::new());
                core.set_draft(&id, "");
            }
            limit.set(PAGE);
            caption
        }
    };

    let attach = {
        let (core, id, take_caption) = (core.clone(), id.clone(), take_caption.clone());
        move |what: Attach| {
            sheet.set(false);
            let (core, id, mut take_caption) = (core.clone(), id.clone(), take_caption.clone());
            match what {
                Attach::Gallery | Attach::File => {
                    let mimes = if what == Attach::Gallery {
                        media::GALLERY
                    } else {
                        media::ANY_FILE
                    };
                    spawn(async move {
                        match media::pick(mimes, true).await {
                            None => toast(
                                &mut toast_sig,
                                t(&tr, "attach.unavailable"),
                                ToastLevel::Warning,
                            ),
                            Some(files) if files.is_empty() => {}
                            Some(files) => {
                                let caption = take_caption();
                                send_and_report(core, id, files, caption, tr, toast_sig).await;
                            }
                        }
                    });
                }
                Attach::Photo | Attach::Video if media::NATIVE_CAMERA => {
                    spawn(async move {
                        let got = media::capture(what == Attach::Video).await;
                        if let Some(file) = got {
                            let caption = take_caption();
                            send_and_report(core, id, vec![file], caption, tr, toast_sig).await;
                        }
                    });
                }
                Attach::Photo => camera.set(Some(CameraMode::Photo)),
                Attach::Video => camera.set(Some(CameraMode::Video)),
            }
        }
    };
    let (attach_gallery, attach_file, attach_photo, attach_video, attach_camera) = (
        attach.clone(),
        attach.clone(),
        attach.clone(),
        attach.clone(),
        attach,
    );

    let start_voice = {
        let core = core.clone();
        move |_| {
            let core = core.clone();
            spawn(async move {
                match media::start_voice(&core).await {
                    VoiceStart::Started => recording.set(true),
                    VoiceStart::NoPermission => toast(
                        &mut toast_sig,
                        t(&tr, "voice.no_permission"),
                        ToastLevel::Warning,
                    ),
                    VoiceStart::Failed => {
                        toast(&mut toast_sig, t(&tr, "voice.failed"), ToastLevel::Error)
                    }
                }
            });
        }
    };

    // The photos of this chat, in order, for the viewer to page through.
    let image_ids: Vec<String> = messages
        .iter()
        .filter(|m| {
            m.attachment.as_ref().is_some_and(|a| {
                a.path.is_some()
                    && !m.extra.view_once
                    && a.state != whatsmine_core::TransferState::Declined
                    && a.kind() == whatsmine_core::MediaKind::Image
            })
        })
        .map(|m| m.id.clone())
        .collect();
    let pending_note = t(&tr, "chat.pending_note").replace("{name}", &name);
    let rows = draft().lines().count().clamp(1, 5);
    let contact_id = id.clone();
    let (voice_core, voice_id) = (core.clone(), id.clone());
    let (camera_core, camera_id) = (core.clone(), id.clone());
    let has_text = !draft().trim().is_empty();
    let placeholder = if is_group {
        t(&tr, "chat.placeholder_in").replace("{name}", &name)
    } else {
        t(&tr, "chat.placeholder")
    };
    let spellcheck = if s.incognito_keyboard {
        "false"
    } else {
        "true"
    };
    let autocomplete = if s.incognito_keyboard { "off" } else { "on" };
    let hits: Vec<Message> = if searching() && !query().trim().is_empty() {
        core.search_chat(&id, &query(), whatsmine_core::rich::SearchIn::All)
    } else {
        Vec::new()
    };
    let (shared_id, look_id) = (id.clone(), id.clone());
    let quick = s.quick_replies.clone();
    let disappear_line = t(&tr, "chat.disappear_on").replace(
        "{time}",
        &crate::screens::contact::disappear_label(&tr, prefs.disappear_ms),
    );
    let scheduled_label =
        t(&tr, "chat.scheduled_count").replace("{n}", &scheduled.len().to_string());

    rsx! {
        div { class: "chat{style.class}", style: "{style.vars}",
            if !style.wall.is_empty() {
                div { class: "chat-wall", style: "{style.wall}" }
            }
            header { class: "hdr s",
                button { class: "ib back-btn", aria_label: "{t(&tr, \"common.back\")}", onclick: move |_| nav.go_back(),
                    Icon { name: IconName::ArrowLeft }
                }
                div {
                    class: "chat-peer",
                    "data-t": "chat-peer",
                    onclick: move |_| {
                        if is_group {
                            nav.push(Route::GroupInfoScreen { id: contact_id.clone() });
                        } else {
                            nav.push(Route::ContactInfo { id: contact_id.clone() });
                        }
                    },
                    Avatar {
                        id: id.clone(),
                        name: name.clone(),
                        size: "small",
                        online,
                        image: contact.avatar.clone(),
                        group: is_group,
                    }
                    div { style: "min-width:0",
                        span { class: "chat-peer-name", "{name}" }
                        if !status.is_empty() {
                            span { class: if live { "chat-peer-status live" } else { "chat-peer-status" }, "{status}" }
                        }
                    }
                }
                if call_ready {
                    button {
                        class: "ib call-start-video",
                        "data-t": "call-video",
                        aria_label: "{t(&tr, \"call.video\")}",
                        onclick: {
                            let (core, id, bridge) = (core.clone(), id.clone(), bridge.clone());
                            move |_| {
                                call::start_call(&core, &id, true, call_phase, &bridge, &mut toast_sig, &tr)
                            }
                        },
                        Icon { name: IconName::Video, size: 22 }
                    }
                    button {
                        class: "ib call-start",
                        "data-t": "call-voice",
                        aria_label: "{t(&tr, \"call.voice\")}",
                        onclick: {
                            let (core, id, bridge) = (core.clone(), id.clone(), bridge.clone());
                            move |_| {
                                call::start_call(&core, &id, false, call_phase, &bridge, &mut toast_sig, &tr)
                            }
                        },
                        Icon { name: IconName::Phone, size: 22 }
                    }
                }
                button {
                    class: "ib",
                    "data-t": "chat-more",
                    aria_label: "{t(&tr, \"chat.more\")}",
                    onclick: move |_| more.set(true),
                    Icon { name: IconName::More, size: 22 }
                }
            }
            if ongoing_call.is_some() && *call_phase.read() == Phase::Idle {
                div { class: "call-banner",
                    span { "{t(&tr, \"group.call_going\")}" }
                    button {
                        class: "call-join",
                        onclick: {
                            let (core, id) = (core.clone(), id.clone());
                            move |_| call::join_group_call(&core, &id, &toast_sig, &tr)
                        },
                        Icon { name: IconName::Phone, size: 16 }
                        "{t(&tr, \"group.join_call\")}"
                    }
                }
            }
            if searching() {
                div { class: "bar-note",
                    label { class: "search-box", style: "background:var(--surface2)",
                        Icon { name: IconName::Search, size: 18 }
                        input {
                            r#type: "search",
                            autofocus: true,
                            "data-t": "chat-search",
                            placeholder: "{t(&tr, \"chat.search\")}",
                            value: "{query}",
                            oninput: move |e| query.set(e.value()),
                        }
                    }
                    button {
                        class: "ib",
                        aria_label: "{t(&tr, \"common.cancel\")}",
                        onclick: move |_| {
                            searching.set(false);
                            query.set(String::new());
                        },
                        Icon { name: IconName::Xmark, size: 20 }
                    }
                }
                if !hits.is_empty() {
                    div { class: "group", style: "flex:none;max-height:40vh;overflow-y:auto;border-radius:0",
                        for hit in hits {
                            {
                                let target = hit.id.clone();
                                rsx! {
                                    button {
                                        key: "{hit.id}",
                                        class: "row sm",
                                        onclick: move |_| {
                                            limit.set(u32::MAX);
                                            js::scroll_to_element(&format!("msg-{target}"));
                                        },
                                        span { class: "stack",
                                            span { class: "one-line", Preview { message: hit.clone() } }
                                            span { class: "sub small", "{fmt::list_time(&tr, hit.sent_at)}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(p) = pinned {
                {
                    let target = p.id.clone();
                    rsx! {
                        button {
                            class: "bar-note",
                            style: "border:none;color:inherit;font:inherit;text-align:left;cursor:pointer;width:100%",
                            "data-t": "pinned",
                            onclick: move |_| {
                                limit.set(u32::MAX);
                                js::scroll_to_element(&format!("msg-{target}"));
                            },
                            Icon { name: IconName::Pin, size: 18 }
                            span { class: "stack one-line", Preview { message: p.clone() } }
                        }
                    }
                }
            }
            if !scheduled.is_empty() {
                button {
                    class: "bar-note",
                    style: "border:none;color:inherit;font:inherit;text-align:left;cursor:pointer;width:100%",
                    "data-t": "scheduled",
                    onclick: move |_| show_scheduled.set(true),
                    Icon { name: IconName::Clock, size: 18 }
                    span { class: "stack", "{scheduled_label}" }
                    Icon { name: IconName::ChevronRight, size: 16 }
                }
            }
            div { id: "msgs", class: "messages",
                if has_more {
                    button {
                        class: "btn small-btn",
                        style: "align-self:center",
                        onclick: move |_| limit += PAGE,
                        "{t(&tr, \"chat.load_older\")}"
                    }
                }
                if contact.state == ContactState::PendingOut {
                    div { class: "sys-line", "{pending_note}" }
                }
                if messages.is_empty() && contact.state == ContactState::Accepted {
                    div { class: "sys-line", "{t(&tr, \"chat.empty\")}" }
                }
                if prefs.disappear_ms.is_some() {
                    div { class: "sys-line", "{disappear_line}" }
                }
                for (i, m) in messages.iter().enumerate() {
                    Fragment { key: "{m.id}",
                        if i == 0 || fmt::day_of(messages[i - 1].sent_at) != fmt::day_of(m.sent_at) {
                            div { class: "date-sep", "{fmt::day_label(&tr, m.sent_at)}" }
                        }
                        Bubble {
                            message: m.clone(),
                            selected: selection.read().as_ref().map(|s| s.contains(&m.id)),
                            on_select: move |id: String| {
                                if let Some(list) = selection.write().as_mut() {
                                    if let Some(at) = list.iter().position(|x| *x == id) {
                                        list.remove(at);
                                    } else {
                                        list.push(id);
                                    }
                                }
                            },
                            sender: m.sender.as_ref().map(|s| {
                                let name = group_info
                                    .as_ref()
                                    .and_then(|g| g.members.iter().find(|x| x.id == *s))
                                    .map(|x| x.name.clone())
                                    .or_else(|| core.contact(s).map(|c| c.display_name()))
                                    .unwrap_or_default();
                                (s.clone(), name)
                            }),
                            on_menu: move |m| menu.set(Some(m)),
                            on_view: move |id: String| viewer.set(Some(id)),
                            on_delivery: move |id: String| delivery.set(Some(id)),
                        }
                    }
                }
            }
            if away_from_newest() {
                button {
                    class: "round jump-newest",
                    "data-t": "jump-newest",
                    aria_label: "{t(&tr, \"chat.to_newest\")}",
                    onclick: move |_| js::scroll_to_bottom("msgs"),
                    Icon { name: IconName::ChevronDown, size: 24 }
                }
            }
            if let Some(chosen) = selection() {
                SelectBar {
                    chosen,
                    on_close: move |_| selection.set(None),
                }
            } else if !can_write {
                div { class: "sys-line", style: "margin:8px", "{t(&tr, \"group.inactive\")}" }
            } else if only_admins {
                div { class: "sys-line", style: "margin:8px", "{t(&tr, \"group.only_admins\")}" }
            } else if recording() {
                VoiceBar {
                    on_send: move |file: Picked| {
                        recording.set(false);
                        limit.set(PAGE);
                        spawn(send_and_report(
                            voice_core.clone(),
                            voice_id.clone(),
                            vec![file],
                            String::new(),
                            tr,
                            toast_sig,
                        ));
                    },
                    on_cancel: move |_| recording.set(false),
                }
            } else {
                if let Some(m) = reply_to() {
                    div { class: "bar-note", "data-t": "reply-bar",
                        Icon { name: IconName::Reply, size: 18 }
                        span { class: "stack",
                            span { class: "b small", "{t(&tr, \"chat.replying\")}" }
                            span { class: "sub one-line", Preview { message: m.clone() } }
                        }
                        button { class: "ib", aria_label: "{t(&tr, \"common.cancel\")}", onclick: move |_| reply_to.set(None),
                            Icon { name: IconName::Xmark, size: 18 }
                        }
                    }
                }
                if editing().is_some() {
                    div { class: "bar-note", "data-t": "edit-bar",
                        Icon { name: IconName::Pencil, size: 18 }
                        span { class: "stack b", "{t(&tr, \"chat.editing\")}" }
                        button {
                            class: "ib",
                            aria_label: "{t(&tr, \"common.cancel\")}",
                            onclick: move |_| {
                                editing.set(None);
                                draft.set(String::new());
                            },
                            Icon { name: IconName::Xmark, size: 18 }
                        }
                    }
                }
                if !mention_options.is_empty() {
                    div { class: "chips scroll",
                        for option in mention_options {
                            button {
                                key: "{option}",
                                class: "chip",
                                onclick: move |_| {
                                    let mut text = draft.peek().clone();
                                    if let Some(at) = text.rfind('@') {
                                        text.truncate(at);
                                    }
                                    text.push_str(&format!("@{option} "));
                                    draft.set(text);
                                    js::focus("composer");
                                },
                                "@{option}"
                            }
                        }
                    }
                }
                if !quick.is_empty() && !has_text {
                    div { class: "chips scroll",
                        for reply in quick {
                            button {
                                key: "{reply}",
                                class: "chip",
                                onclick: move |_| draft.set(reply.clone()),
                                "{reply}"
                            }
                        }
                    }
                }
                div { class: "composer",
                    button {
                        class: "round",
                        "data-t": "attach",
                        title: "{t(&tr, \"chat.attach\")}",
                        onclick: move |_| sheet.set(true),
                        Icon { name: IconName::Plus, size: 24 }
                    }
                    label { class: "pill",
                        textarea {
                            id: "composer",
                            rows: "{rows}",
                            maxlength: "65536",
                            spellcheck,
                            autocomplete,
                            placeholder: "{placeholder}",
                            value: "{draft}",
                            oninput: {
                                let (core, id) = (core.clone(), id.clone());
                                move |e: FormEvent| {
                                    let v = e.value();
                                    let v = if v.trim().is_empty() { String::new() } else { v };
                                    core.set_draft(&id, &v);
                                    if !v.is_empty() {
                                        core.typing(&id);
                                    }
                                    draft.set(v);
                                }
                            },
                            onkeydown: move |e: KeyboardEvent| {
                                if !MOBILE && e.key() == Key::Enter && !e.modifiers().shift() {
                                    e.prevent_default();
                                    send_key(false, None);
                                }
                            },
                        }
                        if !has_text {
                            button {
                                class: "ib",
                                "data-t": "camera",
                                aria_label: "{t(&tr, \"attach.photo\")}",
                                onclick: move |_| {
                                    let mut a = attach_camera.clone();
                                    a(Attach::Photo)
                                },
                                Icon { name: IconName::Camera, size: 20 }
                            }
                        }
                    }
                    if has_text {
                        button {
                            class: "send-btn",
                            "data-t": "send",
                            title: "{t(&tr, \"chat.send\")}",
                            onclick: move |_| send_click(false, None),
                            Icon { name: IconName::Send, size: 20 }
                        }
                    } else {
                        button {
                            class: "send-btn",
                            "data-t": "record",
                            title: "{t(&tr, \"chat.record\")}",
                            onclick: start_voice,
                            Icon { name: IconName::Mic, size: 22 }
                        }
                    }
                }
            }
            if sheet() {
                Sheet { on_close: move |_| sheet.set(false), tag: "attach-sheet",
                    div { class: "sheet-tiles",
                        AttachTile { label: t(&tr, "attach.gallery"), icon: IconName::Image, on_tap: {
                            let mut a = attach_gallery;
                            move |_| a(Attach::Gallery)
                        } }
                        AttachTile { label: t(&tr, "attach.photo"), icon: IconName::Camera, on_tap: {
                            let mut a = attach_photo;
                            move |_| a(Attach::Photo)
                        } }
                        AttachTile { label: t(&tr, "attach.video"), icon: IconName::Video, on_tap: {
                            let mut a = attach_video;
                            move |_| a(Attach::Video)
                        } }
                        AttachTile { label: t(&tr, "attach.file"), icon: IconName::File, on_tap: {
                            let mut a = attach_file;
                            move |_| a(Attach::File)
                        } }
                        AttachTile { label: t(&tr, "attach.once"), icon: IconName::Lock, tag: "new-once", on_tap: {
                            let (core, id) = (core.clone(), id.clone());
                            move |_| {
                                sheet.set(false);
                                let (core, id) = (core.clone(), id.clone());
                                spawn(async move {
                                    let Some(mut files) = media::pick(media::IMAGES, false).await else {
                                        return;
                                    };
                                    if files.is_empty() {
                                        return;
                                    }
                                    let file = files.remove(0);
                                    let done = media::blocking(move || {
                                        core.send_view_once(&id, &file.path, &file.name, &file.mime)
                                            .map(|_| ())
                                            .map_err(|e| e.to_string())
                                    })
                                    .await;
                                    if let Some(Err(e)) = done {
                                        let mut sig = toast_sig;
                                        toast(&mut sig, t(&tr, &format!("chat.err.{e}")), ToastLevel::Error);
                                    }
                                });
                            }
                        } }
                        AttachTile { label: t(&tr, "rich.poll"), icon: IconName::Poll, tag: "new-poll", on_tap: move |_| {
                            sheet.set(false);
                            compose.set(Some(ComposeKind::Poll));
                        } }
                        AttachTile { label: t(&tr, "rich.list"), icon: IconName::ListCheck, tag: "new-list", on_tap: move |_| {
                            sheet.set(false);
                            compose.set(Some(ComposeKind::List));
                        } }
                        AttachTile { label: t(&tr, "rich.plan"), icon: IconName::Calendar, tag: "new-plan", on_tap: move |_| {
                            sheet.set(false);
                            compose.set(Some(ComposeKind::Plan));
                        } }
                        AttachTile { label: t(&tr, "rich.place"), icon: IconName::Place, tag: "new-place", on_tap: move |_| {
                            sheet.set(false);
                            compose.set(Some(ComposeKind::Place));
                        } }
                        AttachTile { label: t(&tr, "rich.contact"), icon: IconName::Card, tag: "new-card", on_tap: move |_| {
                            sheet.set(false);
                            compose.set(Some(ComposeKind::Contact));
                        } }
                    }
                    if has_text {
                        div { class: "sheet-list",
                            SheetItem {
                                label: t(&tr, "chat.send_later"),
                                icon: IconName::Clock,
                                tag: "send-later",
                                on_tap: move |_| {
                                    sheet.set(false);
                                    send_later.set(true);
                                },
                            }
                            SheetItem {
                                label: t(&tr, "chat.send_silent"),
                                icon: IconName::BellOff,
                                tag: "send-silent",
                                on_tap: move |_| {
                                    sheet.set(false);
                                    send_quiet(true, None);
                                },
                            }
                        }
                    }
                }
            }
            if more() {
                Sheet { on_close: move |_| more.set(false), title: name.clone(), tag: "chat-more-sheet",
                    div { class: "sheet-list",
                        SheetItem {
                            label: t(&tr, "chat.search"),
                            icon: IconName::Search,
                            tag: "search-chat",
                            on_tap: move |_| {
                                more.set(false);
                                searching.set(true);
                            },
                        }
                        SheetItem {
                            label: t(&tr, "contact.shared"),
                            icon: IconName::Image,
                            tag: "shared",
                            on_tap: move |_| {
                                more.set(false);
                                nav.push(Route::SharedMedia { id: shared_id.clone() });
                            },
                        }
                        SheetItem {
                            label: t(&tr, "contact.disappear"),
                            icon: IconName::Clock,
                            tag: "disappear",
                            on_tap: move |_| {
                                more.set(false);
                                disappear.set(true);
                            },
                        }
                        SheetItem {
                            label: t(&tr, "look.title"),
                            icon: IconName::Palette,
                            tag: "look",
                            on_tap: move |_| {
                                more.set(false);
                                nav.push(Route::ChatLook { id: look_id.clone() });
                            },
                        }
                        SheetItem {
                            label: t(&tr, "chat.export"),
                            icon: IconName::Download,
                            tag: "export",
                            on_tap: {
                                let (core, id, name) = (core.clone(), id.clone(), name.clone());
                                move |_| {
                                    more.set(false);
                                    let text = export_text(&core, &id, &tr);
                                    let (core, name) = (core.clone(), name.clone());
                                    spawn(async move {
                                        let saved = crate::screens::settings::save_text_file(
                                            &core,
                                            &format!("{name}.txt"),
                                            "text/plain",
                                            text.into_bytes(),
                                        )
                                        .await;
                                        let mut sig = toast_sig;
                                        match saved {
                                            Some(place) => toast(&mut sig, t(&tr, "file.saved").replace("{path}", &place), ToastLevel::Info),
                                            None => toast(&mut sig, t(&tr, "file.save_failed"), ToastLevel::Error),
                                        }
                                    });
                                }
                            },
                        }
                    }
                }
            }
            if disappear() {
                DisappearSheet { id: id.clone(), on_close: move |_| disappear.set(false) }
            }
            if show_scheduled() {
                ScheduledSheet { list: scheduled.clone(), on_close: move |_| show_scheduled.set(false) }
            }
            if send_later() {
                SendLaterSheet {
                    on_close: move |_| send_later.set(false),
                    on_pick: move |at: i64| {
                        send_later.set(false);
                        send_at(false, Some(at));
                    },
                }
            }
            if let Some(kind) = compose() {
                ComposeSheet {
                    kind,
                    chat: id.clone(),
                    on_close: move |_| compose.set(None),
                }
            }
            if let Some(mid) = delivery() {
                DeliverySheet { message_id: mid, on_close: move |_| delivery.set(None) }
            }
            if let Some(mode) = camera() {
                CameraOverlay {
                    mode,
                    on_done: move |file: Option<Picked>| {
                        camera.set(None);
                        if let Some(file) = file {
                            let core = camera_core.clone();
                            let id = camera_id.clone();
                            let mut take_caption = take_caption.clone();
                            let caption = take_caption();
                            spawn(send_and_report(core, id, vec![file], caption, tr, toast_sig));
                        }
                    },
                }
            }
            if let Some(start) = viewer() {
                {
                    let once = core.message(&start).is_some_and(|m| m.extra.view_once);
                    let ids = if once { vec![start.clone()] } else { image_ids.clone() };
                    let (seen_core, seen_id) = (core.clone(), start.clone());
                    rsx! {
                        ImageViewer {
                            ids,
                            start,
                            on_close: move |_| {
                                viewer.set(None);
                                if once {
                                    seen_core.viewed_once(&seen_id);
                                }
                            },
                        }
                    }
                }
            }
            if let Some(m) = menu() {
                MessageMenu {
                    message: m,
                    on_close: move |_| menu.set(None),
                    on_forward: move |m| forward.set(Some(m)),
                    on_reply: move |m| {
                        reply_to.set(Some(m));
                        js::focus("composer");
                    },
                    on_edit: move |m: Message| {
                        draft.set(m.body.clone());
                        editing.set(Some(m));
                        js::focus("composer");
                    },
                    on_delivery: move |id: String| delivery.set(Some(id)),
                    on_select: move |id: String| selection.set(Some(vec![id])),
                }
            }
            if let Some(m) = forward() {
                ForwardSheet { message: m, on_close: move |_| forward.set(None) }
            }
        }
    }
}

/// Several messages at once: what can be done with all of them.
#[component]
fn SelectBar(chosen: Vec<String>, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let count = t(&tr, "chat.selected").replace("{n}", &chosen.len().to_string());
    let (copy_core, delete_core) = (core.clone(), core);
    let (copy_list, delete_list) = (chosen.clone(), chosen);
    rsx! {
        div { class: "bar-note", "data-t": "select-bar", style: "padding-bottom:calc(4px + var(--safe-bottom))",
            button { class: "ib", aria_label: "{t(&tr, \"common.cancel\")}", onclick: move |_| on_close.call(()),
                Icon { name: IconName::Xmark, size: 20 }
            }
            span { class: "stack b", "{count}" }
            button {
                class: "ib",
                "data-t": "select-copy",
                aria_label: "{t(&tr, \"chat.copy\")}",
                onclick: move |_| {
                    // In the order they were written, whichever order they were tapped.
                    let mut found: Vec<Message> = copy_list.iter().filter_map(|id| copy_core.message(id)).collect();
                    found.sort_by_key(|m| m.sent_at);
                    let text: Vec<String> = found.into_iter().map(|m| m.body).filter(|b| !b.is_empty()).collect();
                    js::copy_text(&text.join("\n"));
                    toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                    on_close.call(());
                },
                Icon { name: IconName::Copy, size: 20 }
            }
            button {
                class: "ib danger-text",
                "data-t": "select-delete",
                aria_label: "{t(&tr, \"chat.delete_for_me\")}",
                onclick: move |_| {
                    for id in &delete_list {
                        delete_core.delete_message(id);
                    }
                    on_close.call(());
                },
                Icon { name: IconName::Trash, size: 20 }
            }
        }
    }
}

#[component]
fn AttachTile(
    label: String,
    icon: IconName,
    on_tap: EventHandler<()>,
    #[props(default)] tag: String,
) -> Element {
    rsx! {
        button { class: "sheet-tile", "data-t": "{tag}", onclick: move |_| on_tap.call(()),
            span { Icon { name: icon, size: 24 } }
            span { "{label}" }
        }
    }
}

#[component]
fn ScheduledSheet(list: Vec<Message>, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "saved.scheduled"), tag: "scheduled-sheet",
            for m in list {
                {
                    let (now_core, cancel_core) = (core.clone(), core.clone());
                    let (now_id, cancel_id) = (m.id.clone(), m.id.clone());
                    let goes = t(&tr, "saved.goes_at").replace("{when}", &fmt::when(&tr, m.sent_at));
                    rsx! {
                        div { key: "{m.id}", class: "plain-box", style: "display:flex;flex-direction:column;gap:8px",
                            span { "{m.body}" }
                            span { class: "sub", "{goes}" }
                            div { class: "btns",
                                button {
                                    class: "btn small-btn",
                                    style: "background:var(--surface)",
                                    "data-t": "send-now",
                                    onclick: move |_| {
                                        now_core.send_scheduled_now(&now_id);
                                        on_close.call(());
                                    },
                                    "{t(&tr, \"chat.send_now\")}"
                                }
                                button {
                                    class: "btn small-btn",
                                    style: "background:var(--surface)",
                                    "data-t": "cancel-scheduled",
                                    onclick: move |_| {
                                        cancel_core.cancel_scheduled(&cancel_id);
                                        on_close.call(());
                                    },
                                    "{t(&tr, \"common.cancel\")}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn Bubble(
    message: Message,
    #[props(default)] sender: Option<(String, String)>,
    /// While choosing several: whether this one is chosen.
    #[props(default)]
    selected: Option<bool>,
    on_select: EventHandler<String>,
    on_menu: EventHandler<Message>,
    on_view: EventHandler<String>,
    on_delivery: EventHandler<String>,
) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut press = use_signal(|| 0u64);
    let m = message.clone();
    let more = message.clone();
    let held = message.clone();
    let e = &message.extra;
    let has_media = message.attachment.is_some();
    let dir = if message.outgoing { "out" } else { "in" };
    let class = if has_media {
        format!("bubble {dir} has-media")
    } else {
        format!("bubble {dir}")
    };
    let quoted = e.reply_to.as_ref().and_then(|r| core.message(r));
    let quoted_name = quoted.as_ref().map(|q| {
        if q.outgoing {
            t(&tr, "chat.you")
        } else {
            let who = q.sender.clone().unwrap_or_else(|| q.contact_id.clone());
            core.contact(&who)
                .map(|c| c.display_name())
                .unwrap_or_default()
        }
    });
    let me = core.my_key();
    // One line per emoji, with how many.
    let mut counted: Vec<(String, usize, bool)> = Vec::new();
    for (who, emoji) in &e.reactions {
        match counted.iter_mut().find(|(x, _, _)| x == emoji) {
            Some(entry) => {
                entry.1 += 1;
                entry.2 |= *who == me;
            }
            None => counted.push((emoji.clone(), 1, *who == me)),
        }
    }
    let delivery_id = message.id.clone();
    let tick_status = message.status;
    let mentions: Vec<String> = e
        .mentions
        .iter()
        .filter_map(|id| {
            if *id == me {
                Some(core.settings().display_name)
            } else {
                core.contact(id).map(|c| c.their_name)
            }
        })
        .collect();
    let tap_id = message.id.clone();

    rsx! {
        div {
            class: if selected == Some(true) { "bubble-wrap {dir} chosen" } else { "bubble-wrap {dir}" },
            id: "msg-{message.id}",
            onclick: move |_| {
                if selected.is_some() {
                    on_select.call(tap_id.clone());
                }
            },
            div {
                class,
                "data-t": "bubble",
                oncontextmenu: move |e| {
                    e.prevent_default();
                    on_menu.call(m.clone());
                },
                ontouchstart: move |_| {
                    let token = *press.peek() + 1;
                    press.set(token);
                    let message = held.clone();
                    spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        if *press.peek() == token {
                            on_menu.call(message);
                        }
                    });
                },
                ontouchend: move |_| *press.write() += 1,
                ontouchmove: move |_| *press.write() += 1,
                ontouchcancel: move |_| *press.write() += 1,
                if let Some((sid, sname)) = sender.clone() {
                    span {
                        class: "bubble-sender",
                        style: "color: hsl({sender_hue(&sid)}, 60%, 60%);",
                        "{sname}"
                    }
                }
                if e.forwarded {
                    span { class: "tag-line",
                        Icon { name: IconName::Forward, size: 12 }
                        "{t(&tr, \"chat.forwarded\")}"
                    }
                }
                if let Some(q) = quoted {
                    button {
                        class: "quote",
                        onclick: {
                            let target = q.id.clone();
                            move |_| js::scroll_to_element(&format!("msg-{target}"))
                        },
                        b { "{quoted_name.clone().unwrap_or_default()}" }
                        Preview { message: q.clone() }
                    }
                }
                if e.unsent {
                    span { class: "gone-text", "{t(&tr, \"chat.unsent\")}" }
                } else if let Some(rich) = e.rich.clone() {
                    RichView { message_id: message.id.clone(), rich }
                } else if e.view_once {
                    OnceCard { message: message.clone(), on_view }
                } else if has_media {
                    AttachmentView { message: message.clone(), on_view }
                    if !message.body.is_empty() {
                        div { class: "caption",
                            MessageText { body: message.body.clone(), mentions: mentions.clone() }
                        }
                    }
                } else {
                    MessageText { body: message.body.clone(), mentions: mentions.clone() }
                }
                if e.unknown {
                    span { class: "tag-line", "{t(&tr, \"chat.update_to_see\")}" }
                }
                span { class: "bubble-meta",
                    if e.saved {
                        Icon { name: IconName::Bookmark, size: 12 }
                    }
                    if e.disappear_ms.is_some() {
                        Icon { name: IconName::Clock, size: 12 }
                    }
                    if e.edited && !e.unsent {
                        "{t(&tr, \"chat.edited\")} "
                    }
                    "{fmt::clock(message.sent_at)}"
                    if message.outgoing && !e.unsent {
                        span {
                            "data-t": "ticks",
                            style: "cursor:pointer;display:inline-flex",
                            onclick: move |e| {
                                e.stop_propagation();
                                on_delivery.call(delivery_id.clone());
                            },
                            StatusTicks { status: tick_status }
                        }
                    }
                    button {
                        class: "bubble-more",
                        aria_label: "{t(&tr, \"chat.more\")}",
                        onclick: move |e| {
                            e.stop_propagation();
                            on_menu.call(more.clone());
                        },
                        Icon { name: IconName::ChevronDown, size: 14 }
                    }
                }
            }
            if !counted.is_empty() {
                div { class: "reactions",
                    for (emoji, count, mine) in counted {
                        {
                            let (core, id, tap) = (core.clone(), message.id.clone(), emoji.clone());
                            rsx! {
                                button {
                                    key: "{emoji}",
                                    class: if mine { "reaction mine" } else { "reaction" },
                                    onclick: move |_| {
                                        if mine {
                                            core.react(&id, None);
                                        } else {
                                            core.react(&id, Some(&tap));
                                        }
                                    },
                                    if count > 1 {
                                        "{emoji} {count}"
                                    } else {
                                        "{emoji}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A photo that can be opened once: nothing shows until then, and nothing stays after.
#[component]
fn OnceCard(message: Message, on_view: EventHandler<String>) -> Element {
    let tr = use_tr();
    let can_open = !message.outgoing
        && !message.extra.viewed
        && message
            .attachment
            .as_ref()
            .is_some_and(|a| a.path.is_some());
    let key = if message.outgoing {
        "chat.once_sent"
    } else if message.extra.viewed {
        "chat.once_seen"
    } else if can_open {
        "chat.once_open"
    } else {
        "chat.once_coming"
    };
    let id = message.id.clone();
    rsx! {
        button {
            class: "place-card",
            "data-t": "once",
            disabled: !can_open,
            onclick: move |_| on_view.call(id.clone()),
            span { class: "place-pin", Icon { name: IconName::Image, size: 24 } }
            span { class: "row-t", "{t(&tr, key)}" }
        }
    }
}

/// A message's text, with web addresses and invitations clickable and the marks people type.
#[component]
fn MessageText(body: String, #[props(default)] mentions: Vec<String>) -> Element {
    let tr = use_tr();
    let invite = use_context::<InviteSig>();
    let mut toast_sig = use_context::<ToastSig>();
    let pieces = fmt::pieces(&body);
    rsx! {
        for (i, piece) in pieces.into_iter().enumerate() {
            match piece {
                fmt::Piece::Text(text) => rsx! {
                    for (j, styled) in fmt::styled(&text).into_iter().enumerate() {
                        StyledText { key: "{i}-{j}", styled, mentions: mentions.clone() }
                    }
                },
                fmt::Piece::Link(url) => rsx! {
                    a {
                        key: "{i}",
                        class: "msg-link",
                        href: "{url}",
                        onclick: {
                            let url = url.clone();
                            move |e| {
                                e.prevent_default();
                                e.stop_propagation();
                                if url.starts_with("whatsmine://") {
                                    links::offer(invite, &url);
                                } else {
                                    let web = if url.starts_with("www.") {
                                        format!("https://{url}")
                                    } else {
                                        url.clone()
                                    };
                                    if !media::open_url(&web) {
                                        toast(&mut toast_sig, t(&tr, "file.open_failed"), ToastLevel::Warning);
                                    }
                                }
                            }
                        },
                        "{url}"
                    }
                },
            }
        }
    }
}

#[component]
fn StyledText(styled: fmt::Styled, mentions: Vec<String>) -> Element {
    let mut hidden = use_signal(|| true);
    match styled {
        fmt::Styled::Plain(text) => {
            let parts = fmt::with_mentions(&text, &mentions);
            rsx! {
                for (i, (part, named)) in parts.into_iter().enumerate() {
                    span { key: "{i}", class: if named { "mention" } else { "" }, "{part}" }
                }
            }
        }
        fmt::Styled::Bold(text) => rsx! { b { "{text}" } },
        fmt::Styled::Italic(text) => rsx! { i { "{text}" } },
        fmt::Styled::Strike(text) => rsx! { s { "{text}" } },
        fmt::Styled::Code(text) => rsx! { code { "{text}" } },
        fmt::Styled::Spoiler(text) => rsx! {
            span {
                style: if hidden() { "background:currentColor;border-radius:4px;cursor:pointer" } else { "" },
                onclick: move |_| hidden.set(false),
                "{text}"
            }
        },
    }
}

/// A stable colour for each person in a group chat.
fn sender_hue(id: &str) -> u32 {
    id.bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32))
        % 360
}

#[component]
fn MessageMenu(
    message: Message,
    on_close: EventHandler<()>,
    on_forward: EventHandler<Message>,
    on_reply: EventHandler<Message>,
    on_edit: EventHandler<Message>,
    on_delivery: EventHandler<String>,
    on_select: EventHandler<String>,
) -> Element {
    let tr = use_tr();
    let core = use_core();
    let settings = use_context::<SettingsSig>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let mut more_emoji = use_signal(|| false);
    let mut remind = use_signal(|| false);
    let body = message.body.clone();
    let id = message.id.clone();
    let has_file = !message.extra.view_once
        && message
            .attachment
            .as_ref()
            .is_some_and(|a| a.path.is_some());
    let unsent = message.extra.unsent;
    let can_forward = (!body.is_empty() || has_file) && !unsent;
    let can_edit =
        message.outgoing && !unsent && message.attachment.is_none() && message.extra.rich.is_none();
    let me = core.my_key();
    let mine: Option<String> = message
        .extra
        .reactions
        .iter()
        .find(|(who, _)| *who == me)
        .map(|(_, e)| e.clone());
    let shortlist = settings.read().reactions.clone();
    let (for_open, for_save, for_forward, for_reply, for_edit) = (
        message.clone(),
        message.clone(),
        message.clone(),
        message.clone(),
        message.clone(),
    );
    let chat = message.contact_id.clone();
    let prefs = core.chat_prefs(&chat);
    let pinned = prefs.pinned_messages.contains(&id);
    let saved = message.extra.saved;
    let failed = message.status == MessageStatus::NotDelivered;

    let (save_core, pin_core, unsend_core, delete_core, retry_core) = (
        core.clone(),
        core.clone(),
        core.clone(),
        core.clone(),
        core.clone(),
    );
    let (save_id, pin_id, unsend_id, delete_id, retry_id, delivery_id) = (
        id.clone(),
        id.clone(),
        id.clone(),
        id.clone(),
        id.clone(),
        id.clone(),
    );

    if remind() {
        return rsx! {
            RemindSheet { message_id: id.clone(), outgoing: message.outgoing, on_close: move |_| on_close.call(()) }
        };
    }

    let react = {
        let (core, id, mine) = (core.clone(), id.clone(), mine.clone());
        move |emoji: &str| {
            if mine.as_deref() == Some(emoji) {
                core.react(&id, None);
            } else {
                core.react(&id, Some(emoji));
            }
            on_close.call(());
        }
    };
    let react_more = react.clone();
    let from_them = !message.outgoing;

    rsx! {
        Sheet { on_close: move |_| on_close.call(()), tag: "message-menu",
            if !unsent {
                div { class: "reaction-bar", role: "group", aria_label: "{t(&tr, \"chat.react\")}",
                    for emoji in shortlist {
                        {
                            let react = react.clone();
                            let tap = emoji.clone();
                            rsx! {
                                button {
                                    key: "{emoji}",
                                    class: if mine.as_deref() == Some(emoji.as_str()) { "on" } else { "" },
                                    "data-t": "react",
                                    onclick: move |_| react(&tap),
                                    "{emoji}"
                                }
                            }
                        }
                    }
                    button {
                        "data-t": "react-more",
                        aria_label: "{t(&tr, \"chat.more_reactions\")}",
                        onclick: move |_| more_emoji.toggle(),
                        Icon { name: IconName::Plus, size: 20 }
                    }
                }
            }
            if more_emoji() {
                div { class: "emoji-grid",
                    for emoji in EMOJI.iter().copied() {
                        {
                            let react = react_more.clone();
                            rsx! {
                                button { key: "{emoji}", onclick: move |_| react(emoji), "{emoji}" }
                            }
                        }
                    }
                }
            }
            div { class: "sheet-list",
                if failed {
                    SheetItem {
                        label: t(&tr, "delivery.try_now"),
                        icon: IconName::Restart,
                        tag: "retry",
                        on_tap: move |_| {
                            retry_core.retry_message(&retry_id);
                            on_close.call(());
                        },
                    }
                }
                if !unsent {
                    SheetItem {
                        label: t(&tr, "chat.reply"),
                        icon: IconName::Reply,
                        tag: "reply",
                        on_tap: move |_| {
                            on_reply.call(for_reply.clone());
                            on_close.call(());
                        },
                    }
                }
                if can_forward {
                    SheetItem {
                        label: t(&tr, "forward.action"),
                        icon: IconName::Forward,
                        tag: "forward",
                        on_tap: move |_| {
                            on_forward.call(for_forward.clone());
                            on_close.call(());
                        },
                    }
                }
                if !body.is_empty() && from_them {
                    SheetItem {
                        label: t(&tr, "chat.read_aloud"),
                        icon: IconName::Volume,
                        tag: "read-aloud",
                        on_tap: {
                            let text = body.clone();
                            move |_| {
                                js::speak(&text);
                                on_close.call(());
                            }
                        },
                    }
                }
                if !body.is_empty() {
                    SheetItem {
                        label: t(&tr, "chat.copy"),
                        icon: IconName::Copy,
                        tag: "copy",
                        on_tap: move |_| {
                            js::copy_text(&body);
                            toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                            on_close.call(());
                        },
                    }
                }
                if can_edit {
                    SheetItem {
                        label: t(&tr, "chat.edit"),
                        icon: IconName::Pencil,
                        tag: "edit",
                        on_tap: move |_| {
                            on_edit.call(for_edit.clone());
                            on_close.call(());
                        },
                    }
                }
                if !unsent {
                    SheetItem {
                        label: t(&tr, if saved { "chat.unsave" } else { "chat.save" }),
                        icon: IconName::Bookmark,
                        tag: "save",
                        on_tap: move |_| {
                            if saved {
                                save_core.unsave_message(&save_id);
                            } else {
                                save_core.save_message(&save_id, &[]);
                            }
                            on_close.call(());
                        },
                    }
                    SheetItem {
                        label: t(&tr, if pinned { "chat.unpin" } else { "chat.pin" }),
                        icon: IconName::Pin,
                        tag: "pin",
                        on_tap: move |_| {
                            let mut p = pin_core.chat_prefs(&chat);
                            if pinned {
                                p.pinned_messages.retain(|x| *x != pin_id);
                            } else {
                                p.pinned_messages.push(pin_id.clone());
                            }
                            pin_core.set_chat_prefs(&chat, &p);
                            on_close.call(());
                        },
                    }
                    SheetItem {
                        label: t(&tr, "chat.remind"),
                        icon: IconName::Bell,
                        tag: "remind",
                        on_tap: move |_| remind.set(true),
                    }
                }
                if message.outgoing && !unsent {
                    SheetItem {
                        label: t(&tr, "delivery.title"),
                        icon: IconName::Info,
                        tag: "delivery",
                        on_tap: move |_| {
                            on_delivery.call(delivery_id.clone());
                            on_close.call(());
                        },
                    }
                }
                if has_file && !unsent {
                    SheetItem {
                        label: t(&tr, "file.open"),
                        icon: IconName::External,
                        on_tap: move |_| {
                            open_file(tr, &mut toast_sig, &for_open);
                            on_close.call(());
                        },
                    }
                    SheetItem {
                        label: t(&tr, "file.save"),
                        icon: IconName::Download,
                        on_tap: move |_| {
                            save_file(tr, toast_sig, &for_save);
                            on_close.call(());
                        },
                    }
                }
                SheetItem {
                    label: t(&tr, "chat.select"),
                    icon: IconName::CheckDouble,
                    tag: "select",
                    on_tap: {
                        let id = id.clone();
                        move |_| {
                            on_select.call(id.clone());
                            on_close.call(());
                        }
                    },
                }
                if message.outgoing && !unsent {
                    SheetItem {
                        label: t(&tr, "chat.delete_for_all"),
                        icon: IconName::Trash,
                        danger: true,
                        tag: "unsend",
                        on_tap: move |_| {
                            let _ = unsend_core.unsend_message(&unsend_id);
                            on_close.call(());
                        },
                    }
                }
                SheetItem {
                    label: t(&tr, "chat.delete_for_me"),
                    icon: IconName::Trash,
                    danger: true,
                    tag: "delete",
                    on_tap: move |_| {
                        delete_core.delete_message(&delete_id);
                        on_close.call(());
                    },
                }
            }
        }
    }
}

#[component]
fn RemindSheet(message_id: String, outgoing: bool, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    const HOUR: i64 = 3_600_000;
    let choices: [(&str, i64); 3] = [
        ("chat.remind_hour", HOUR),
        ("chat.remind_tomorrow", 24 * HOUR),
        ("chat.remind_week", 7 * 24 * HOUR),
    ];
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "chat.remind"), tag: "remind-sheet",
            div { class: "sheet-list",
                for (key, wait) in choices {
                    {
                        let (core, id) = (core.clone(), message_id.clone());
                        rsx! {
                            SheetItem {
                                key: "{key}",
                                label: t(&tr, key),
                                icon: IconName::Bell,
                                tag: "remind-{wait}",
                                on_tap: move |_| {
                                    core.remind_me(&id, now_ms() + wait);
                                    toast(&mut toast_sig, t(&tr, "chat.remind_set"), ToastLevel::Info);
                                    on_close.call(());
                                },
                            }
                        }
                    }
                }
                if outgoing {
                    SheetItem {
                        label: t(&tr, "chat.remind_no_answer"),
                        icon: IconName::Reply,
                        tag: "remind-no-answer",
                        on_tap: {
                            let (core, id) = (core.clone(), message_id.clone());
                            move |_| {
                                core.remind_if_no_answer(&id, now_ms() + 24 * HOUR);
                                toast(&mut toast_sig, t(&tr, "chat.remind_set"), ToastLevel::Info);
                                on_close.call(());
                            }
                        },
                    }
                }
            }
        }
    }
}
