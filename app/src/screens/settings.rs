use dioxus::prelude::*;
use whatsmine_core::Settings;

use crate::app::{Net, Rev, SettingsSig, ToastSig, Translations, t, toast, use_core, use_tr};
use crate::components::{
    BottomNav, ConfirmDialog, Dropdown, Icon, IconName, Tab, ToastLevel, ToggleRow,
};
use crate::fmt;
use crate::i18n;
use crate::js;

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
