use dioxus::prelude::*;
use whatsmine_core::ContactState;
use whatsmine_core::identity::{invite_link, key_to_string, parse_invite};

use crate::app::{Route, SettingsSig, ToastSig, t, toast, use_core, use_tr};
use crate::components::scan::{self, ScanOverlay};
use crate::components::{Icon, IconName, QrCode, ToastLevel};
use crate::fmt;
use crate::js;
use crate::links::{self, InviteSig};

fn clipboard_label(tr: &crate::app::Translations, name: &str) -> String {
    if name.is_empty() {
        t(tr, "add.clipboard")
    } else {
        t(tr, "add.clipboard_named").replace("{name}", name)
    }
}

/// Show your invitation; add someone by theirs.
#[component]
pub fn AddContact() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let settings = use_context::<SettingsSig>().0;
    let invite = use_context::<InviteSig>();
    let mut toast_sig = use_context::<ToastSig>();
    let mut their_key = use_signal(String::new);
    let mut nickname = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut scanning = use_signal(|| false);
    let my_key = core.my_key();
    let link = invite_link(&my_key, &settings.read().display_name);

    // An invitation waiting on the clipboard is offered right away.
    let mut on_clipboard = use_signal(|| None::<(String, String)>);
    {
        let my_key = my_key.clone();
        let core = core.clone();
        use_hook(move || {
            spawn(async move {
                let Some(text) = js::paste_text().await else {
                    return;
                };
                let Some(found) = parse_invite(&text) else {
                    return;
                };
                let key = key_to_string(&found.key);
                let already = core
                    .contact(&key)
                    .is_some_and(|c| c.state != ContactState::PendingIn);
                if key != my_key && !already {
                    on_clipboard.set(Some((text, found.name.unwrap_or_default())));
                }
            });
        });
    }

    let submit = {
        let core = core.clone();
        move |_| {
            let typed = their_key.peek().clone();
            // A pasted link or message also works here.
            let key = parse_invite(&typed)
                .map(|i| key_to_string(&i.key))
                .unwrap_or(typed);
            match core.add_contact(&key, Some(nickname.peek().as_str())) {
                Ok(c) => {
                    nav.replace(Route::Chat { id: c.id });
                }
                Err(e) => error.set(Some(t(&tr, &format!("add.err.{e}")))),
            }
        }
    };

    let start_scan = move |_| {
        if scan::NATIVE_SCANNER {
            spawn(async move {
                if let Some(code) = scan::scan_native().await
                    && !links::offer(invite, &code)
                {
                    toast(
                        &mut toast_sig,
                        t(&tr, "add.scan_not_ours"),
                        ToastLevel::Warning,
                    );
                }
            });
        } else {
            scanning.set(true);
        }
    };

    let (copy_link, copy_key) = (link.clone(), my_key.clone());
    #[cfg(target_os = "android")]
    let share_text = t(&tr, "add.share_text").replace("{link}", &link);

    rsx! {
        div { class: "screen no-nav",
            div { class: "config-header",
                button { class: "back-btn", onclick: move |_| nav.go_back(),
                    Icon { name: IconName::ArrowLeft }
                }
                span { class: "config-title", "{t(&tr, \"add.title\")}" }
            }

            if let Some((text, name)) = on_clipboard() {
                div { class: "banner clipboard-invite",
                    span { "{clipboard_label(&tr, &name)}" }
                    button {
                        class: "secondary-btn",
                        onclick: move |_| {
                            on_clipboard.set(None);
                            links::offer(invite, &text);
                        },
                        "{t(&tr, \"add.clipboard_use\")}"
                    }
                }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"add.your_key\")}" }
                p { class: "setting-hint", "{t(&tr, \"add.your_key_hint\")}" }
                QrCode { data: link.clone() }
                div { class: "button-row",
                    button {
                        class: "secondary-btn grow",
                        onclick: move |_| {
                            js::copy_text(&copy_link);
                            toast(&mut toast_sig, t(&tr, "add.link_copied"), ToastLevel::Info);
                        },
                        Icon { name: IconName::Link, size: 18 }
                        "{t(&tr, \"add.copy_link\")}"
                    }
                    if cfg!(target_os = "android") {
                        button {
                            class: "secondary-btn grow",
                            onclick: move |_| {
                                #[cfg(target_os = "android")]
                                {
                                    let text = share_text.clone();
                                    let title = t(&tr, "add.share");
                                    std::thread::spawn(move || {
                                        let _ = mobile_sentinel::share::text(&text, Some(&title));
                                    });
                                }
                            },
                            Icon { name: IconName::Share, size: 18 }
                            "{t(&tr, \"add.share\")}"
                        }
                    }
                }
                div { class: "key-box", "{fmt::grouped_key(&my_key)}" }
                button {
                    class: "secondary-btn grow",
                    style: "width: 100%;",
                    onclick: move |_| {
                        js::copy_text(&copy_key);
                        toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                    },
                    Icon { name: IconName::Copy, size: 18 }
                    "{t(&tr, \"add.copy_key\")}"
                }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"add.their_key\")}" }
                button {
                    class: "save-btn scan-btn",
                    onclick: start_scan,
                    Icon { name: IconName::Scan, size: 20 }
                    "{t(&tr, \"add.scan\")}"
                }
                div { class: "field",
                    div { class: "input-row",
                        input {
                            r#type: "text",
                            placeholder: "{t(&tr, \"add.key_placeholder\")}",
                            value: "{their_key}",
                            autocomplete: "off",
                            spellcheck: "false",
                            oninput: move |e| {
                                their_key.set(e.value());
                                error.set(None);
                            },
                        }
                        button {
                            class: "secondary-btn",
                            title: "{t(&tr, \"add.paste\")}",
                            onclick: move |_| async move {
                                if let Some(text) = js::paste_text().await {
                                    their_key.set(text.trim().to_string());
                                    error.set(None);
                                }
                            },
                            Icon { name: IconName::Clipboard, size: 18 }
                        }
                    }
                    if let Some(err) = error() {
                        p { class: "field-error", "{err}" }
                    }
                }
                div { class: "field",
                    input {
                        r#type: "text",
                        maxlength: "64",
                        placeholder: "{t(&tr, \"add.nickname_placeholder\")}",
                        value: "{nickname}",
                        oninput: move |e| nickname.set(e.value()),
                    }
                    p { class: "setting-hint", "{t(&tr, \"add.nickname_hint\")}" }
                }
                button {
                    class: "save-btn",
                    disabled: their_key().trim().is_empty(),
                    onclick: submit,
                    "{t(&tr, \"add.send_request\")}"
                }
                p { class: "setting-hint", "{t(&tr, \"add.request_hint\")}" }
            }

            if scanning() {
                ScanOverlay {
                    on_code: move |code: String| {
                        scanning.set(false);
                        if !links::offer(invite, &code) {
                            toast(&mut toast_sig, t(&tr, "add.scan_not_ours"), ToastLevel::Warning);
                        }
                    },
                    on_close: move |_| scanning.set(false),
                }
            }
        }
    }
}
