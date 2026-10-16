use dioxus::prelude::*;

use crate::app::{Route, ToastSig, t, toast, use_core, use_tr};
use crate::components::{Icon, IconName, QrCode, ToastLevel};
use crate::fmt;
use crate::js;

/// Show your key; add someone by theirs.
#[component]
pub fn AddContact() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let mut toast_sig = use_context::<ToastSig>();
    let mut their_key = use_signal(String::new);
    let mut nickname = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let my_key = core.my_key();

    let copy_key = my_key.clone();
    let submit = move |_| match core.add_contact(&their_key.peek(), Some(nickname.peek().as_str()))
    {
        Ok(c) => {
            nav.replace(Route::Chat { id: c.id });
        }
        Err(e) => error.set(Some(t(&tr, &format!("add.err.{e}")))),
    };

    rsx! {
        div { class: "screen no-nav",
            div { class: "config-header",
                button { class: "back-btn", onclick: move |_| nav.go_back(),
                    Icon { name: IconName::ArrowLeft }
                }
                span { class: "config-title", "{t(&tr, \"add.title\")}" }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"add.your_key\")}" }
                div { class: "key-box", "{fmt::grouped_key(&my_key)}" }
                p { class: "setting-hint", "{t(&tr, \"add.your_key_hint\")}" }
                QrCode { data: format!("whatsmine:{my_key}") }
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
        }
    }
}
