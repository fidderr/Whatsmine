use dioxus::prelude::*;
use whatsmine_core::Settings;

use crate::app::{Net, Rev, SettingsSig, ToastSig, Translations, t, toast, use_core, use_tr};
use crate::components::{
    Avatar, BottomNav, CameraMode, CameraOverlay, ConfirmDialog, Dropdown, Icon, IconName, Tab,
    ToastLevel, ToggleRow,
};
use crate::fmt;
use crate::i18n;
use crate::js;
use crate::media::{self, Picked};

#[component]
pub fn SettingsScreen() -> Element {
    let mut tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let net = use_context::<Net>().0;
    let mut settings = use_context::<SettingsSig>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let mut name = use_signal(|| settings.peek().display_name.clone());
    let mut confirm_wipe = use_signal(|| false);
    let mut camera = use_signal(|| false);

    let save_core = core.clone();
    let update = use_callback(move |f: Box<dyn Fn(&mut Settings)>| {
        let mut s = settings.peek().clone();
        f(&mut s);
        if save_core.save_settings(&s).is_ok() {
            settings.set(s);
        }
    });

    let s = settings();
    let _ = rev();
    let blocked = core.blocked();
    let my_key = core.my_key();
    let net_key = match net() {
        whatsmine_core::NetStatus::Online => "net.status_online",
        whatsmine_core::NetStatus::Connecting => "net.connecting",
        whatsmine_core::NetStatus::Offline => "net.offline",
    };

    let theme_options = vec![
        ("system".to_string(), t(&tr, "settings.theme_system")),
        ("dark".to_string(), t(&tr, "settings.theme_dark")),
        ("light".to_string(), t(&tr, "settings.theme_light")),
    ];
    let language_options: Vec<(String, String)> = i18n::LANGUAGES
        .iter()
        .map(|(c, n)| (c.to_string(), n.to_string()))
        .collect();

    let copy_key = my_key.clone();
    let wipe_core = core.clone();

    let size_options: Vec<(String, String)> = [
        ("100", "settings.size_100"),
        ("500", "settings.size_500"),
        ("2048", "settings.size_2048"),
        ("10240", "settings.size_10240"),
        ("0", "settings.size_0"),
    ]
    .iter()
    .map(|(v, k)| (v.to_string(), t(&tr, k)))
    .collect();

    // Profile picture: from the gallery, from the camera, or removed.
    let apply_photo = {
        let core = core.clone();
        move |file: Picked| {
            let core = core.clone();
            spawn(async move {
                let ok = media::set_avatar(core.clone(), file).await;
                settings.set(core.settings());
                if !ok {
                    toast(
                        &mut toast_sig,
                        t(&tr, "settings.photo_failed"),
                        ToastLevel::Error,
                    );
                }
            });
        }
    };
    let choose_photo = {
        let apply = apply_photo.clone();
        move |_| {
            let apply = apply.clone();
            spawn(async move {
                match media::pick(media::IMAGES, false).await {
                    None => toast(
                        &mut toast_sig,
                        t(&tr, "attach.unavailable"),
                        ToastLevel::Warning,
                    ),
                    Some(mut files) if !files.is_empty() => apply(files.remove(0)),
                    Some(_) => {}
                }
            });
        }
    };
    let take_photo = {
        let apply = apply_photo.clone();
        move |_| {
            if media::NATIVE_CAMERA {
                let apply = apply.clone();
                spawn(async move {
                    if let Some(file) = media::capture(false).await {
                        apply(file);
                    }
                });
            } else {
                camera.set(true);
            }
        }
    };
    let remove_core = core.clone();
    let camera_apply = apply_photo.clone();
    rsx! {
        div { class: "screen",
            div { class: "header",
                h1 { "{t(&tr, \"settings.title\")}" }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"settings.profile\")}" }
                div { class: "input-row",
                    input {
                        r#type: "text",
                        maxlength: "64",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                    button {
                        class: "secondary-btn",
                        onclick: move |_| {
                            let n: String = name.peek().trim().chars().take(64).collect();
                            update.call(Box::new(move |s: &mut Settings| s.display_name = n.clone()));
                            toast(&mut toast_sig, t(&tr, "common.saved"), ToastLevel::Info);
                        },
                        "{t(&tr, \"common.save\")}"
                    }
                }
                p { class: "setting-hint", "{t(&tr, \"settings.name_hint\")}" }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"settings.photo\")}" }
                div { class: "profile-photo",
                    Avatar {
                        id: "me".to_string(),
                        name: s.display_name.clone(),
                        size: "large",
                        image: s.avatar.clone(),
                    }
                    div { class: "photo-actions",
                        button { class: "secondary-btn", onclick: choose_photo,
                            Icon { name: IconName::Image, size: 18 }
                            "{t(&tr, \"settings.photo_choose\")}"
                        }
                        button { class: "secondary-btn", onclick: take_photo,
                            Icon { name: IconName::Camera, size: 18 }
                            "{t(&tr, \"settings.photo_take\")}"
                        }
                        if s.avatar.is_some() {
                            button {
                                class: "secondary-btn danger",
                                onclick: move |_| {
                                    let _ = remove_core.clear_avatar();
                                    settings.set(remove_core.settings());
                                },
                                Icon { name: IconName::Trash, size: 18 }
                                "{t(&tr, \"settings.photo_remove\")}"
                            }
                        }
                    }
                }
                p { class: "setting-hint", "{t(&tr, \"settings.photo_hint\")}" }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"add.your_key\")}" }
                div { class: "key-box", "{fmt::grouped_key(&my_key)}" }
                button {
                    class: "secondary-btn",
                    style: "margin-top: 10px;",
                    onclick: move |_| {
                        js::copy_text(&copy_key);
                        toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                    },
                    Icon { name: IconName::Copy, size: 18 }
                    "{t(&tr, \"add.copy_key\")}"
                }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"settings.appearance\")}" }
                div { class: "field",
                    p { class: "setting-hint", style: "margin: 0 0 6px;", "{t(&tr, \"settings.theme\")}" }
                    Dropdown {
                        value: s.theme.clone(),
                        options: theme_options,
                        on_change: move |v: String| update.call(Box::new(move |s: &mut Settings| s.theme = v.clone())),
                    }
                }
                div { class: "field",
                    p { class: "setting-hint", style: "margin: 0 0 6px;", "{t(&tr, \"settings.language\")}" }
                    Dropdown {
                        value: s.locale.clone(),
                        options: language_options,
                        on_change: move |v: String| {
                            tr.set(i18n::load_translations(&v));
                            update.call(Box::new(move |s: &mut Settings| s.locale = v.clone()));
                        },
                    }
                }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"settings.privacy\")}" }
                ToggleRow {
                    label: t(&tr, "settings.read_receipts"),
                    hint: t(&tr, "settings.read_receipts_hint"),
                    checked: s.read_receipts,
                    on_change: move |v| update.call(Box::new(move |s: &mut Settings| s.read_receipts = v)),
                }
                ToggleRow {
                    label: t(&tr, "settings.last_seen"),
                    hint: t(&tr, "settings.last_seen_hint"),
                    checked: s.share_last_seen,
                    on_change: move |v| update.call(Box::new(move |s: &mut Settings| s.share_last_seen = v)),
                }
                ToggleRow {
                    label: t(&tr, "settings.typing"),
                    hint: t(&tr, "settings.typing_hint"),
                    checked: s.typing_indicators,
                    on_change: move |v| update.call(Box::new(move |s: &mut Settings| s.typing_indicators = v)),
                }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"settings.files\")}" }
                p { class: "setting-hint", style: "margin: 0 0 6px;", "{t(&tr, \"settings.max_file\")}" }
                Dropdown {
                    value: s.max_file_mb.to_string(),
                    options: size_options,
                    on_change: move |v: String| {
                        let mb = v.parse::<u64>().unwrap_or(2048);
                        update.call(Box::new(move |s: &mut Settings| s.max_file_mb = mb));
                    },
                }
                p { class: "setting-hint", "{t(&tr, \"settings.max_file_hint\")}" }
            }

            if !blocked.is_empty() {
                div { class: "settings-section",
                    label { class: "section-label", "{t(&tr, \"settings.blocked\")}" }
                    div { class: "list",
                        for c in blocked {
                            BlockedRow { key: "{c.id}", id: c.id.clone(), name: c.display_name() }
                        }
                    }
                }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"settings.about\")}" }
                div { class: "about-info",
                    span { "WhatsMine {env!(\"CARGO_PKG_VERSION\")}" }
                    span { "{t(&tr, \"settings.network\")}: {t(&tr, net_key)}" }
                    span { "{t(&tr, \"settings.about_text\")}" }
                    span { "{t(&tr, \"settings.about_limits\")}" }
                }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"settings.danger\")}" }
                button { class: "delete-btn", onclick: move |_| confirm_wipe.set(true),
                    "{t(&tr, \"settings.wipe\")}"
                }
                p { class: "setting-hint", "{t(&tr, \"settings.wipe_hint\")}" }
            }

            if camera() {
                CameraOverlay {
                    mode: CameraMode::Photo,
                    on_done: move |file: Option<Picked>| {
                        camera.set(false);
                        if let Some(file) = file {
                            camera_apply(file);
                        }
                    },
                }
            }

            if confirm_wipe() {
                ConfirmDialog {
                    title: t(&tr, "settings.wipe_title"),
                    message: t(&tr, "settings.wipe_message"),
                    confirm_label: t(&tr, "settings.wipe_confirm"),
                    on_confirm: move |_| {
                        let _ = wipe_core.wipe_everything();
                        quit();
                    },
                    on_cancel: move |_| confirm_wipe.set(false),
                }
            }

            BottomNav { active: Tab::Settings }
        }
    }
}

/// After wiping, the old identity must not linger in memory: close the app.
fn quit() {
    std::process::exit(0);
}

#[component]
fn BlockedRow(id: String, name: String) -> Element {
    let tr: Translations = use_tr();
    let core = use_core();
    rsx! {
        div { class: "card static",
            crate::components::Avatar { id: id.clone(), name: name.clone(), size: "small" }
            div { class: "card-main",
                span { class: "card-title", "{name}" }
            }
            button {
                class: "secondary-btn",
                onclick: move |_| {
                    let _ = core.unblock(&id);
                },
                "{t(&tr, \"contact.unblock\")}"
            }
        }
    }
}
