use dioxus::prelude::*;
use whatsmine_core::{MediaKind, Message, TransferState};

use crate::app::{ToastSig, Transfers, t, toast, use_core, use_tr};
use crate::back::BackLayer;
use crate::components::{Icon, IconName, ToastLevel};
use crate::{fmt, js, media};

/// Open / save a received file. Shared by the file card, the viewer and the
/// message menu.
pub fn open_file(tr: crate::app::Translations, toast_sig: &mut ToastSig, message: &Message) {
    let Some(att) = message.attachment.as_ref() else {
        return;
    };
    let Some(path) = att.path.as_deref() else {
        return;
    };
    if !media::open(path, &att.mime) {
        toast(toast_sig, t(&tr, "file.open_failed"), ToastLevel::Warning);
    }
}

pub fn save_file(tr: crate::app::Translations, toast_sig: ToastSig, message: &Message) {
    let Some(att) = message.attachment.as_ref() else {
        return;
    };
    let Some(path) = att.path.clone() else {
        return;
    };
    let (name, mime) = (att.name.clone(), att.mime.clone());
    let mut toast_sig = toast_sig;
    spawn(async move {
        match media::save_to_downloads(path, name, mime).await {
            Some(place) => toast(
                &mut toast_sig,
                t(&tr, "file.saved").replace("{path}", &place),
                ToastLevel::Info,
            ),
            None => toast(
                &mut toast_sig,
                t(&tr, "file.save_failed"),
                ToastLevel::Error,
            ),
        }
    });
}

/// The content of a file message: image, video, audio or a file card, with
/// transfer progress and failure states.
#[component]
pub fn AttachmentView(message: Message, on_view: EventHandler<String>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let transfers = use_context::<Transfers>().0;
    let toast_sig = use_context::<ToastSig>();
    let mut broken = use_signal(|| false);

    let Some(att) = message.attachment.clone() else {
        return rsx! {};
    };
    let id = message.id.clone();
    let progress = transfers.read().get(&id).copied();
    let have_file = att.path.is_some();
    // Media the host cannot stream inline (huge files on Android) and media the
    // player rejected both fall back to a file card with Open / Save.
    let kind = if broken() || !crate::media_server::inline_ok(att.size) {
        MediaKind::File
    } else {
        att.kind()
    };

    // What to say about the transfer, if anything.
    let status: Option<String> = match (att.state, message.outgoing) {
        (TransferState::Declined, true) => Some(t(&tr, "file.declined")),
        (TransferState::Declined, false) => Some(t(&tr, "file.over_limit")),
        (TransferState::Failed, _) => Some(t(&tr, "file.failed")),
        (TransferState::Waiting, _) => Some(t(&tr, "file.waiting")),
        (TransferState::Transferring, outgoing) => {
            let (done, total) = progress.unwrap_or((0, att.size));
            let key = if outgoing {
                "file.sending"
            } else {
                "file.receiving"
            };
            if progress.is_none()
                && outgoing
                && message.status < whatsmine_core::MessageStatus::Sent
            {
                Some(t(&tr, "file.waiting"))
            } else {
                Some(
                    t(&tr, key)
                        .replace("{pct}", &fmt::percent(done, total).to_string())
                        .replace("{done}", &fmt::bytes(done))
                        .replace("{total}", &fmt::bytes(total)),
                )
            }
        }
        (TransferState::Done, _) => None,
    };
    let pct = progress
        .map(|(d, t)| fmt::percent(d, t))
        .filter(|_| att.state == TransferState::Transferring);
    let failed = att.state == TransferState::Failed && !message.outgoing;
    let src = format!("/media/{id}");
    let for_open = message.clone();
    let for_save = message.clone();
    let retry_id = id.clone();
    let retry_core = core.clone();

    rsx! {
        div { class: "attachment",
            if have_file && att.state != TransferState::Declined {
                match kind {
                    MediaKind::Image => rsx! {
                        img {
                            class: "media-img",
                            src: "{src}",
                            alt: "{att.name}",
                            onclick: {
                                let id = id.clone();
                                move |_| on_view.call(id.clone())
                            },
                            onload: move |_| js::scroll_if_near_bottom("msgs"),
                            onerror: move |_| broken.set(true),
                        }
                    },
                    MediaKind::Video => rsx! {
                        video {
                            class: "media-video",
                            src: "{src}",
                            controls: true,
                            preload: "metadata",
                            playsinline: true,
                            onloadedmetadata: move |_| js::scroll_if_near_bottom("msgs"),
                            onerror: move |_| broken.set(true),
                        }
                    },
                    MediaKind::Audio => rsx! {
                        crate::components::VoicePlayer {
                            id: message.id.clone(),
                            src: src.clone(),
                            duration_ms: att.duration_ms,
                            on_broken: move |_| broken.set(true),
                        }
                    },
                    MediaKind::File => rsx! {
                        FileCard {
                            name: att.name.clone(),
                            size: att.size,
                            actions: att.state == TransferState::Done || message.outgoing,
                            on_open: {
                                let mut toast_sig = toast_sig;
                                move |_| open_file(tr, &mut toast_sig, &for_open)
                            },
                            on_save: move |_| save_file(tr, toast_sig, &for_save),
                        }
                    },
                }
            } else {
                FileCard {
                    name: att.name.clone(),
                    size: att.size,
                    actions: false,
                    on_open: |_| {},
                    on_save: |_| {},
                }
            }
            if let Some(p) = pct {
                div { class: "progress",
                    div { class: "progress-bar", style: "width: {p}%;" }
                }
            }
            if let Some(s) = status {
                div { class: "file-status",
                    span { "{s}" }
                    if failed {
                        button {
                            class: "link-btn",
                            onclick: move |_| retry_core.retry_file(&retry_id),
                            "{t(&tr, \"file.retry\")}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FileCard(
    name: String,
    size: u64,
    actions: bool,
    on_open: EventHandler<()>,
    on_save: EventHandler<()>,
) -> Element {
    let tr = use_tr();
    rsx! {
        div { class: "file-card",
            div { class: "file-icon",
                Icon { name: IconName::File, size: 26 }
            }
            div { class: "file-info",
                span { class: "file-name", "{name}" }
                span { class: "file-size", "{fmt::bytes(size)}" }
            }
            if actions {
                div { class: "file-actions",
                    button {
                        class: "icon-btn",
                        title: "{t(&tr, \"file.open\")}",
                        onclick: move |_| on_open.call(()),
                        Icon { name: IconName::External, size: 18 }
                    }
                    button {
                        class: "icon-btn",
                        title: "{t(&tr, \"file.save\")}",
                        onclick: move |_| on_save.call(()),
                        Icon { name: IconName::Download, size: 18 }
                    }
                }
            }
        }
    }
}

/// Full-screen pictures: swipe or use the arrows to go through the chat's
/// photos, tap a picture to zoom, save from the bar. Back and Esc close it.
#[component]
pub fn ImageViewer(ids: Vec<String>, start: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let toast_sig = use_context::<ToastSig>();
    let mut index = use_signal(|| ids.iter().position(|i| *i == start).unwrap_or(0));
    let mut zoomed = use_signal(|| false);
    let mut touch_x = use_signal(|| None::<f64>);
    use_hook(|| crate::js::focus("viewer"));
    // Seen once is once, however this ends: closing it, going elsewhere, or the app being killed.
    let once_core = core.clone();
    let first = start.clone();
    use_hook(move || {
        if once_core.message(&first).is_some_and(|m| m.extra.view_once) {
            once_core.view_once_opening(&first);
        }
    });
    let done_core = core.clone();
    use_drop(move || done_core.finish_view_once());

    let total = ids.len();
    let at = index().min(total.saturating_sub(1));
    let Some(message) = ids.get(at).and_then(|id| core.message(id)) else {
        return rsx! {
            BackLayer { on_back: move |_| on_close.call(()) }
        };
    };
    let name = message
        .attachment
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let src = format!("/media/{}", message.id);
    let mut step = move |delta: isize| {
        let next = index() as isize + delta;
        if next >= 0 && (next as usize) < total {
            index.set(next as usize);
            zoomed.set(false);
        }
    };
    rsx! {
        div {
            id: "viewer",
            class: "viewer",
            tabindex: "0",
            onclick: move |_| on_close.call(()),
            onkeydown: move |e: KeyboardEvent| match e.key() {
                Key::ArrowLeft => step(-1),
                Key::ArrowRight => step(1),
                _ => {}
            },
            ontouchstart: move |e| {
                touch_x.set(e.touches().first().map(|t| t.client_coordinates().x));
            },
            ontouchend: move |e| {
                let from = touch_x.take();
                let to = e.touches_changed().first().map(|t| t.client_coordinates().x);
                if let (Some(a), Some(b), false) = (from, to, zoomed()) {
                    if b - a > 70.0 {
                        step(-1);
                    } else if a - b > 70.0 {
                        step(1);
                    }
                }
            },
            BackLayer {
                on_back: move |_| {
                    if zoomed() {
                        zoomed.set(false);
                    } else {
                        on_close.call(());
                    }
                },
            }
            div { class: "viewer-bar", onclick: move |e| e.stop_propagation(),
                button { class: "back-btn", onclick: move |_| on_close.call(()),
                    Icon { name: IconName::Xmark }
                }
                span { class: "viewer-name", "{name}" }
                if total > 1 {
                    span { class: "viewer-count", "{at + 1} / {total}" }
                }
                if !message.extra.view_once {
                    button {
                        class: "header-icon-btn",
                        title: "{t(&tr, \"file.save\")}",
                        onclick: {
                            let message = message.clone();
                            move |_| save_file(tr, toast_sig, &message)
                        },
                        Icon { name: IconName::Download }
                    }
                }
            }
            div { class: if zoomed() { "viewer-body zoomed" } else { "viewer-body" },
                img {
                    class: "viewer-img",
                    src: "{src}",
                    alt: "{name}",
                    onclick: move |e| {
                        e.stop_propagation();
                        zoomed.toggle();
                    },
                }
            }
            if total > 1 && at > 0 {
                button {
                    class: "viewer-nav prev",
                    onclick: move |e| {
                        e.stop_propagation();
                        step(-1);
                    },
                    Icon { name: IconName::ChevronLeft, size: 28 }
                }
            }
            if total > 1 && at + 1 < total {
                button {
                    class: "viewer-nav next",
                    onclick: move |e| {
                        e.stop_propagation();
                        step(1);
                    },
                    Icon { name: IconName::ChevronRight, size: 28 }
                }
            }
        }
    }
}
