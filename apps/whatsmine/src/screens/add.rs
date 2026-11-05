use dioxus::prelude::*;
use whatsmine_core::ContactState;
use whatsmine_core::identity::{key_to_string, parse_invite};

use crate::app::{Route, SettingsSig, ToastSig, Translations, t, toast, use_core, use_tr};
use crate::components::scan::{self, ScanOverlay};
use crate::components::{Avatar, Header, Icon, IconName, QrCode, Row, ToastLevel};
use crate::js;
use crate::links::{self, InviteSig};

fn clipboard_label(tr: &Translations, name: &str) -> String {
    if name.is_empty() {
        t(tr, "add.clipboard")
    } else {
        t(tr, "add.clipboard_named").replace("{name}", name)
    }
}

/// Hands text to the system's share sheet, or copies it where there is none.
pub fn share_or_copy(tr: Translations, mut toast_sig: ToastSig, text: String, title: String) {
    #[cfg(target_os = "android")]
    {
        let _ = (tr, &mut toast_sig);
        std::thread::spawn(move || {
            let _ = mobile_sentinel::share::text(&text, Some(&title));
        });
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = title;
        js::copy_text(&text);
        toast(&mut toast_sig, t(&tr, "add.link_copied"), ToastLevel::Info);
    }
}

/// Starts the scanner, and offers what it found.
pub fn use_scan(invite: InviteSig) -> (Signal<bool>, Callback<()>) {
    let tr = use_tr();
    let mut toast_sig = use_context::<ToastSig>();
    let mut scanning = use_signal(|| false);
    let start = use_callback(move |()| {
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
    });
    (scanning, start)
}

/// Your code for whoever is next to you, and every other way of adding someone.
#[component]
pub fn AddContact() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let settings = use_context::<SettingsSig>().0;
    let invite = use_context::<InviteSig>();
    let mut toast_sig = use_context::<ToastSig>();
    let my_key = core.my_key();
    let name = settings.read().display_name.clone();
    let photo = settings.read().avatar.clone();
    let link = core.invite_link();
    let handle = core.handle();
    let (mut scanning, start_scan) = use_scan(invite);
    // Scanned right here, in person: both are added without asking.
    let in_person = use_signal({
        let core = core.clone();
        move || core.in_person_code()
    });

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
                let Some(person) = found.key() else {
                    return;
                };
                let key = key_to_string(&person);
                let already = core
                    .contact(&key)
                    .is_some_and(|c| c.state != ContactState::PendingIn);
                if key != my_key && !already {
                    on_clipboard.set(Some((text, found.name.clone())));
                }
            });
        });
    }

    let copy_handle = handle.clone();
    let share_text = t(&tr, "add.share_text").replace("{link}", &link);

    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "add.title") }
            div { class: "body g12",
                if let Some((text, found)) = on_clipboard() {
                    div { class: "notice still",
                        Icon { name: IconName::Clipboard }
                        span { class: "stack b", "{clipboard_label(&tr, &found)}" }
                        button {
                            class: "btn small-btn",
                            onclick: move |_| {
                                on_clipboard.set(None);
                                links::offer(invite, &text);
                            },
                            "{t(&tr, \"add.clipboard_use\")}"
                        }
                    }
                }
                section { class: "panel center", style: "gap:4px",
                    QrCode { data: in_person() }
                    span { class: "mark", style: "padding-top:8px",
                        Avatar { id: "me".to_string(), name: name.clone(), size: "tiny", image: photo }
                        span { style: "font-size:1.25rem;font-weight:500", "{name}" }
                    }
                    button {
                        class: "row sm",
                        style: "justify-content:center;width:auto",
                        "data-t": "copy-handle",
                        aria_label: "{t(&tr, \"add.copy_handle\")}",
                        onclick: move |_| {
                            js::copy_text(&copy_handle);
                            toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                        },
                        span { class: "dim", "{handle}" }
                        Icon { name: IconName::Copy, size: 18 }
                    }
                }
                p { class: "note", "{t(&tr, \"add.your_key_hint\")}" }
                div { class: "btns",
                    button {
                        class: "btn main",
                        "data-t": "scan",
                        onclick: move |_| start_scan.call(()),
                        Icon { name: IconName::Camera, size: 20 }
                        "{t(&tr, \"add.scan\")}"
                    }
                    button {
                        class: "btn",
                        "data-t": "share",
                        onclick: move |_| share_or_copy(tr, toast_sig, share_text.clone(), t(&tr, "add.share")),
                        Icon { name: IconName::Share, size: 20 }
                        "{t(&tr, \"add.share\")}"
                    }
                }
                div { class: "group",
                    Row {
                        icon: IconName::Pencil,
                        title: t(&tr, "add.type"),
                        note: t(&tr, "add.type_note"),
                        to: Route::AddType {},
                        tag: "type",
                    }
                    Row {
                        icon: IconName::Link,
                        title: t(&tr, "address.title"),
                        note: t(&tr, "address.row_note"),
                        to: Route::PublicAddress {},
                        tag: "address",
                    }
                }
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

/// Type what they gave you: a link, a name code, or the code they read out.
#[component]
pub fn AddType() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let mut toast_sig = use_context::<ToastSig>();
    let mut typed = use_signal(String::new);
    let mut greeting = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut code = use_signal(|| None::<String>);

    let submit = {
        let core = core.clone();
        move |_| {
            let text = typed.peek().trim().to_owned();
            let hello = greeting.peek().trim().to_owned();
            let core = core.clone();
            busy.set(true);
            error.set(None);
            spawn(async move {
                let result = core.add_by_text(&text, &hello).await;
                busy.set(false);
                match result {
                    Ok(c) => {
                        nav.replace(Route::Chat { id: c.id });
                    }
                    Err(e) => error.set(Some(t(&tr, &format!("add.err.{e}")))),
                }
            });
        }
    };

    let new_code = {
        let core = core.clone();
        move |_| {
            let core = core.clone();
            spawn(async move {
                code.set(Some(core.read_out_code().await));
            });
        }
    };
    let new_code_again = new_code.clone();

    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "add.type") }
            div { class: "body g12",
                label { class: "field",
                    span { class: "gt", "{t(&tr, \"add.their_code\")}" }
                    div { class: "input-row",
                        input {
                            r#type: "text",
                            placeholder: "{t(&tr, \"add.key_placeholder\")}",
                            value: "{typed}",
                            autocomplete: "off",
                            spellcheck: "false",
                            oninput: move |e| {
                                typed.set(e.value());
                                error.set(None);
                            },
                        }
                        button {
                            class: "btn",
                            title: "{t(&tr, \"add.paste\")}",
                            onclick: move |_| async move {
                                if let Some(text) = js::paste_text().await {
                                    typed.set(text.trim().to_string());
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
                label { class: "field",
                    span { class: "gt", "{t(&tr, \"add.greeting\")}" }
                    textarea {
                        rows: "2",
                        maxlength: "120",
                        placeholder: "{t(&tr, \"add.greeting_placeholder\")}",
                        value: "{greeting}",
                        oninput: move |e| greeting.set(e.value()),
                    }
                }
                button {
                    class: "btn main",
                    "data-t": "send-request",
                    disabled: typed().trim().is_empty() || busy(),
                    onclick: submit,
                    if busy() {
                        "{t(&tr, \"add.looking\")}"
                    } else {
                        "{t(&tr, \"add.send_request\")}"
                    }
                }
                p { class: "note", "{t(&tr, \"add.request_hint\")}" }
                h2 { class: "gt", "{t(&tr, \"add.on_the_phone\")}" }
                section { class: "panel center",
                    if let Some(shown) = code() {
                        span { class: "code-big", "{shown}" }
                        span { class: "sub", "{t(&tr, \"add.read_out_note\")}" }
                        div { class: "btns", style: "align-self:stretch",
                            button {
                                class: "btn",
                                onclick: move |_| {
                                    js::copy_text(&shown);
                                    toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                                },
                                Icon { name: IconName::Copy, size: 20 }
                                "{t(&tr, \"common.copy\")}"
                            }
                            button { class: "btn", onclick: new_code_again,
                                Icon { name: IconName::Restart, size: 20 }
                                "{t(&tr, \"add.new_code\")}"
                            }
                        }
                    } else {
                        span { class: "sub", "{t(&tr, \"add.read_out_text\")}" }
                        button { class: "btn", "data-t": "read-out", onclick: new_code,
                            Icon { name: IconName::Phone, size: 20 }
                            "{t(&tr, \"add.read_out\")}"
                        }
                    }
                }
            }
        }
    }
}
