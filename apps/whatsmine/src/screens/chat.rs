use dioxus::prelude::*;
use whatsmine_core::{ContactState, Core, Message};

use crate::app::{
    MsgRev, Rev, Route, SettingsSig, ToastSig, Translations, Typing, t, toast, use_core, use_tr,
};
use crate::components::{
    AttachmentView, Avatar, CameraMode, CameraOverlay, Icon, IconName, ImageViewer, ToastLevel,
    VoiceBar, open_file, save_file,
};
use crate::fmt;
use crate::js;
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
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let msg_rev = use_context::<MsgRev>().0;
    let typing = use_context::<Typing>().0;
    let settings = use_context::<SettingsSig>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let nav = use_navigator();

    let mut limit = use_signal(|| PAGE);
    let mut draft = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.draft(&id))
    };
    let mut menu: Signal<Option<Message>> = use_signal(|| None);
    let mut viewer: Signal<Option<Message>> = use_signal(|| None);
    let mut sheet = use_signal(|| false);
    let mut recording = use_signal(|| false);
    let mut camera: Signal<Option<CameraMode>> = use_signal(|| None);

    // Read while open: on mount and whenever messages change.
    {
        let (core, id) = (core.clone(), id.clone());
        use_effect(move || {
            let _ = msg_rev();
            core.mark_read(&id);
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
    let online = core.is_online(&id);
    let is_typing = typing.read().contains_key(&id);
    let show_last_seen = settings.read().share_last_seen;

    let (status, live) = if is_typing {
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

    let pending_note = t(&tr, "chat.pending_note").replace("{name}", &name);
    let rows = draft().lines().count().clamp(1, 5);
    let contact_id = id.clone();
    let (voice_core, voice_id) = (core.clone(), id.clone());
    let (camera_core, camera_id) = (core.clone(), id.clone());
    let view_core = core.clone();

    rsx! {
        div { class: "chat",
            div { class: "chat-header",
                button { class: "back-btn", onclick: move |_| nav.go_back(),
                    Icon { name: IconName::ArrowLeft }
                }
                div {
                    class: "chat-peer",
                    onclick: move |_| {
                        nav.push(Route::ContactInfo { id: contact_id.clone() });
                    },
                    Avatar {
                        id: id.clone(),
                        name: name.clone(),
                        size: "small",
                        online,
                        image: contact.avatar.clone(),
                    }
                    div { class: "chat-peer-text",
                        span { class: "chat-peer-name", "{name}" }
                        if !status.is_empty() {
                            span { class: if live { "chat-peer-status live" } else { "chat-peer-status" }, "{status}" }
                        }
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
                            on_menu: move |m| menu.set(Some(m)),
                            on_view: {
                                let core = view_core.clone();
                                move |id: String| viewer.set(core.message(&id))
                            },
                        }
                    }
                }
            }
            if recording() {
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
            if let Some(m) = viewer() {
                ImageViewer { message: m, on_close: move |_| viewer.set(None) }
            }
            if let Some(m) = menu() {
                MessageMenu {
                    message: m,
                    on_close: move |_| menu.set(None),
                }
            }
        }
    }
}

#[component]
fn Bubble(
    message: Message,
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
            if has_media {
                AttachmentView { message: message.clone(), on_view }
                if !message.body.is_empty() {
                    div { class: "caption", "{message.body}" }
                }
            } else {
                "{message.body}"
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

#[component]
fn MessageMenu(message: Message, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let body = message.body.clone();
    let id = message.id.clone();
    let has_file = message
        .attachment
        .as_ref()
        .is_some_and(|a| a.path.is_some());
    let (for_open, for_save) = (message.clone(), message.clone());
    rsx! {
        div { class: "bottom-sheet-overlay", onclick: move |_| on_close.call(()),
            div { class: "bottom-sheet", onclick: move |e| e.stop_propagation(),
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
