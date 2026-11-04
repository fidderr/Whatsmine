use dioxus::prelude::*;
use whatsmine_core::{ContactKind, ContactState, Core, Message};

use crate::app::{
    MsgRev, Rev, Route, SettingsSig, ToastSig, Translations, Typing, t, toast, use_core, use_tr,
};
use crate::back::BackLayer;
use crate::call::{self, CallBridge, CallUi, Phase};
use crate::components::{
    AttachmentView, Avatar, CameraMode, CameraOverlay, ForwardSheet, Icon, IconName, ImageViewer,
    ToastLevel, VoiceBar, open_file, save_file,
};
use crate::fmt;
use crate::js;
use crate::links::{self, InviteSig};
use crate::media::{self, Picked, VoiceStart};
use crate::screens::chats::StatusTicks;

const PAGE: u32 = 100;
const MOBILE: bool = cfg!(any(target_os = "android", target_os = "ios"));

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
    let mut recording = use_signal(|| false);
    let mut camera: Signal<Option<CameraMode>> = use_signal(|| None);

    // Read while open: on mount and whenever messages change.
    {
        let (core, id) = (core.clone(), id.clone());
        use_effect(move || {
            let _ = msg_rev();
            core.mark_read(&id);
            crate::notify::mark_read(&id);
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
                div { class: "config-header",
                    button { class: "back-btn", onclick: move |_| nav.go_back(),
                        Icon { name: IconName::ArrowLeft }
                    }
                }
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
    let call_ready = if is_group {
        can_write
    } else {
        contact.state == ContactState::Accepted
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
    let show_last_seen = settings.read().share_last_seen;

    let (status, live) = if let Some(s) = group_status {
        (s, false)
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

    let send = {
        let (core, id, tr) = (core.clone(), id.clone(), tr);
        move || {
            let text = draft.peek().clone();
            if text.trim().is_empty() {
                return;
            }
            match core.send(&id, &text) {
                Ok(_) => {
                    draft.set(String::new());
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
    let mut send_key = send;

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
    let (attach_gallery, attach_file, attach_photo, attach_video) = (
        attach.clone(),
        attach.clone(),
        attach.clone(),
        attach.clone(),
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

    rsx! {
        div { class: "chat",
            div { class: "chat-header",
                button { class: "back-btn", onclick: move |_| nav.go_back(),
                    Icon { name: IconName::ArrowLeft }
                }
                div {
                    class: "chat-peer",
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
                        zoom: true,
                    }
                    div { class: "chat-peer-text",
                        span { class: "chat-peer-name", "{name}" }
                        if !status.is_empty() {
                            span { class: if live { "chat-peer-status live" } else { "chat-peer-status" }, "{status}" }
                        }
                    }
                }
                if call_ready {
                    button {
                        class: "header-icon-btn call-start",
                        aria_label: "{t(&tr, \"call.voice\")}",
                        onclick: {
                            let (core, id, bridge) = (core.clone(), id.clone(), bridge.clone());
                            move |_| {
                                call::start_call(&core, &id, false, call_phase, &bridge, &mut toast_sig, &tr)
                            }
                        },
                        Icon { name: IconName::Phone, size: 20 }
                    }
                    button {
                        class: "header-icon-btn call-start-video",
                        aria_label: "{t(&tr, \"call.video\")}",
                        onclick: {
                            let (core, id, bridge) = (core.clone(), id.clone(), bridge.clone());
                            move |_| {
                                call::start_call(&core, &id, true, call_phase, &bridge, &mut toast_sig, &tr)
                            }
                        },
                        Icon { name: IconName::Video, size: 20 }
                    }
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
            div { id: "msgs", class: "messages",
                if has_more {
                    button {
                        class: "secondary-btn load-older",
                        onclick: move |_| limit += PAGE,
                        "{t(&tr, \"chat.load_older\")}"
                    }
                }
                if contact.state == ContactState::PendingOut {
                    div { class: "pending-note", "{pending_note}" }
                }
                if messages.is_empty() && contact.state == ContactState::Accepted {
                    div { class: "pending-note", "{t(&tr, \"chat.empty\")}" }
                }
                for (i, m) in messages.iter().enumerate() {
                    Fragment { key: "{m.id}",
                        if i == 0 || fmt::day_of(messages[i - 1].sent_at) != fmt::day_of(m.sent_at) {
                            div { class: "date-sep", "{fmt::day_label(&tr, m.sent_at)}" }
                        }
                        Bubble {
                            message: m.clone(),
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
                        }
                    }
                }
            }
            if !can_write {
                div { class: "pending-note", "{t(&tr, \"group.inactive\")}" }
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
                div { class: "composer",
                    button {
                        class: "icon-btn",
                        title: "{t(&tr, \"chat.attach\")}",
                        onclick: move |_| sheet.set(true),
                        Icon { name: IconName::Paperclip, size: 22 }
                    }
                    textarea {
                        id: "composer",
                        rows: "{rows}",
                        maxlength: "65536",
                        placeholder: "{t(&tr, \"chat.placeholder\")}",
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
                                send_key();
                            }
                        },
                    }
                    if draft().trim().is_empty() {
                        button {
                            class: "send-btn",
                            title: "{t(&tr, \"chat.record\")}",
                            onclick: start_voice,
                            Icon { name: IconName::Mic, size: 20 }
                        }
                    } else {
                        button {
                            class: "send-btn",
                            title: "{t(&tr, \"chat.send\")}",
                            onclick: move |_| send_click(),
                            Icon { name: IconName::Send, size: 20 }
                        }
                    }
                }
            }
            if sheet() {
                div { class: "bottom-sheet-overlay", onclick: move |_| sheet.set(false),
                    BackLayer { on_back: move |_| sheet.set(false) }
                    div { class: "bottom-sheet", onclick: move |e| e.stop_propagation(),
                        button {
                            class: "sheet-option",
                            onclick: {
                                let mut a = attach_gallery;
                                move |_| a(Attach::Gallery)
                            },
                            Icon { name: IconName::Image }
                            "{t(&tr, \"attach.gallery\")}"
                        }
                        button {
                            class: "sheet-option",
                            onclick: {
                                let mut a = attach_photo;
                                move |_| a(Attach::Photo)
                            },
                            Icon { name: IconName::Camera }
                            "{t(&tr, \"attach.photo\")}"
                        }
                        button {
                            class: "sheet-option",
                            onclick: {
                                let mut a = attach_video;
                                move |_| a(Attach::Video)
                            },
                            Icon { name: IconName::Video }
                            "{t(&tr, \"attach.video\")}"
                        }
                        button {
                            class: "sheet-option",
                            onclick: {
                                let mut a = attach_file;
                                move |_| a(Attach::File)
                            },
                            Icon { name: IconName::File }
                            "{t(&tr, \"attach.file\")}"
                        }
                        button { class: "sheet-option cancel", onclick: move |_| sheet.set(false),
                            "{t(&tr, \"common.cancel\")}"
                        }
                    }
                }
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
                ImageViewer {
                    ids: image_ids.clone(),
                    start,
                    on_close: move |_| viewer.set(None),
                }
            }
            if let Some(m) = menu() {
                MessageMenu {
                    message: m,
                    on_close: move |_| menu.set(None),
                    on_forward: move |m| forward.set(Some(m)),
                }
            }
            if let Some(m) = forward() {
                ForwardSheet { message: m, on_close: move |_| forward.set(None) }
            }
        }
    }
}

#[component]
fn Bubble(
    message: Message,
    #[props(default)] sender: Option<(String, String)>,
    on_menu: EventHandler<Message>,
    on_view: EventHandler<String>,
) -> Element {
    let m = message.clone();
    let more = message.clone();
    let has_media = message.attachment.is_some();
    let class = match (message.outgoing, has_media) {
        (true, false) => "bubble out",
        (false, false) => "bubble in",
        (true, true) => "bubble out has-media",
        (false, true) => "bubble in has-media",
    };
    rsx! {
        div {
            class,
            oncontextmenu: move |e| {
                e.prevent_default();
                on_menu.call(m.clone());
            },
            if let Some((sid, sname)) = sender.clone() {
                div {
                    class: "bubble-sender",
                    style: "color: hsl({sender_hue(&sid)}, 60%, 55%);",
                    "{sname}"
                }
            }
            if has_media {
                AttachmentView { message: message.clone(), on_view }
                if !message.body.is_empty() {
                    div { class: "caption", "{message.body}" }
                }
            } else {
                MessageText { body: message.body.clone() }
            }
            span { class: "bubble-meta",
                "{fmt::clock(message.sent_at)}"
                if message.outgoing {
                    StatusTicks { status: message.status }
                }
                button {
                    class: "bubble-more",
                    onclick: move |e| {
                        e.stop_propagation();
                        on_menu.call(more.clone());
                    },
                    Icon { name: IconName::ChevronDown, size: 14 }
                }
            }
        }
    }
}

/// A message's text, with web addresses and invitations clickable.
#[component]
fn MessageText(body: String) -> Element {
    let tr = use_tr();
    let invite = use_context::<InviteSig>();
    let mut toast_sig = use_context::<ToastSig>();
    let pieces = fmt::pieces(&body);
    if pieces.len() == 1 && matches!(pieces[0], fmt::Piece::Text(_)) {
        return rsx! { "{body}" };
    }
    rsx! {
        for (i, piece) in pieces.into_iter().enumerate() {
            match piece {
                fmt::Piece::Text(text) => rsx! {
                    span { key: "{i}", "{text}" }
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
) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let body = message.body.clone();
    let id = message.id.clone();
    let has_file = message
        .attachment
        .as_ref()
        .is_some_and(|a| a.path.is_some());
    let (for_open, for_save, for_forward) = (message.clone(), message.clone(), message.clone());
    let can_forward = !body.is_empty() || has_file;
    rsx! {
        div { class: "bottom-sheet-overlay", onclick: move |_| on_close.call(()),
            BackLayer { on_back: move |_| on_close.call(()) }
            div { class: "bottom-sheet", onclick: move |e| e.stop_propagation(),
                if can_forward {
                    button {
                        class: "sheet-option",
                        onclick: move |_| {
                            on_forward.call(for_forward.clone());
                            on_close.call(());
                        },
                        Icon { name: IconName::Forward }
                        "{t(&tr, \"forward.action\")}"
                    }
                }
                if has_file {
                    button {
                        class: "sheet-option",
                        onclick: move |_| {
                            open_file(tr, &mut toast_sig, &for_open);
                            on_close.call(());
                        },
                        Icon { name: IconName::External }
                        "{t(&tr, \"file.open\")}"
                    }
                    button {
                        class: "sheet-option",
                        onclick: move |_| {
                            save_file(tr, toast_sig, &for_save);
                            on_close.call(());
                        },
                        Icon { name: IconName::Download }
                        "{t(&tr, \"file.save\")}"
                    }
                }
                if !body.is_empty() {
                    button {
                        class: "sheet-option",
                        onclick: move |_| {
                            js::copy_text(&body);
                            toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                            on_close.call(());
                        },
                        Icon { name: IconName::Copy }
                        "{t(&tr, \"chat.copy\")}"
                    }
                }
                button {
                    class: "sheet-option destructive",
                    onclick: move |_| {
                        core.delete_message(&id);
                        on_close.call(());
                    },
                    Icon { name: IconName::Trash }
                    "{t(&tr, \"chat.delete_for_me\")}"
                }
                button { class: "sheet-option cancel", onclick: move |_| on_close.call(()),
                    "{t(&tr, \"common.cancel\")}"
                }
            }
        }
    }
}
