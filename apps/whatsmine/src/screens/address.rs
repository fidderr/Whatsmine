use dioxus::prelude::*;
use whatsmine_core::MyInvite;
use whatsmine_core::model::now_ms;

use crate::app::{ToastSig, t, toast, use_core, use_tr};
use crate::components::{
    Chips, Header, Icon, IconName, QrCode, Sheet, SwitchRow, ToastLevel, change, use_settings,
};
use crate::js;
use crate::screens::add::share_or_copy;

const DAY: i64 = 86_400_000;

/// What strangers can reach you with, and how to make all of it stop working.
#[component]
pub fn PublicAddress() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let mut version = use_signal(|| 0u32);
    let mut new_link = use_signal(|| false);
    let mut confirm_reset = use_signal(|| false);
    let _ = version();
    let handle = core.handle();
    let link = core.invite_link();
    // What is still out there. Codes to read out and in-person codes are gone in minutes.
    let mut invites: Vec<MyInvite> = core
        .invites()
        .into_iter()
        .filter(|i| {
            !i.revoked
                && i.met_token.is_none()
                && !whatsmine_core::codes::looks_like_code(&i.label)
                && i.expires_at.is_none_or(|at| at > now_ms())
                && i.max_uses.is_none_or(|most| i.uses < most)
        })
        .collect();
    invites.sort_by_key(|i| !i.main);
    let (copy_handle, share_link) = (handle.clone(), link.clone());
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "address.title") }
            div { class: "body g12",
                p { class: "note", "{t(&tr, \"address.intro\")}" }
                section { class: "panel", style: "flex-direction:row;align-items:center;gap:8px",
                    div { class: "qr mini",
                        QrCode { data: link.clone() }
                    }
                    span { class: "stack", style: "padding-left:4px",
                        span { class: "row-t b one-line", "{handle}" }
                        span { class: "sub", "{t(&tr, \"address.name_code\")}" }
                    }
                    button {
                        class: "ib",
                        aria_label: "{t(&tr, \"add.copy_handle\")}",
                        onclick: move |_| {
                            js::copy_text(&copy_handle);
                            toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                        },
                        Icon { name: IconName::Copy, size: 20 }
                    }
                    button {
                        class: "ib",
                        aria_label: "{t(&tr, \"add.share\")}",
                        onclick: move |_| {
                            let text = t(&tr, "add.share_text").replace("{link}", &share_link);
                            share_or_copy(tr, toast_sig, text, t(&tr, "add.share"));
                        },
                        Icon { name: IconName::Share, size: 20 }
                    }
                }
                h2 { class: "gt", "{t(&tr, \"address.links\")}" }
                div { class: "group",
                    for invite in invites {
                        InviteRow { key: "{invite.id}", invite, on_change: move |_| *version.write() += 1 }
                    }
                }
                button { class: "btn", "data-t": "new-link", onclick: move |_| new_link.set(true),
                    Icon { name: IconName::Plus, size: 20 }
                    "{t(&tr, \"address.new_link\")}"
                }
                section { class: "panel warn",
                    div {
                        h2 { class: "row-t b", "{t(&tr, \"address.bothered\")}" }
                        p { class: "sub", "{t(&tr, \"address.bothered_text\")}" }
                    }
                    button {
                        class: "btn warn",
                        "data-t": "reset",
                        onclick: move |_| confirm_reset.set(true),
                        "{t(&tr, \"address.reset\")}"
                    }
                }
            }
            if new_link() {
                NewLinkSheet {
                    on_close: move |_| {
                        new_link.set(false);
                        *version.write() += 1;
                    },
                }
            }
            if confirm_reset() {
                ResetSheet {
                    on_close: move |_| {
                        confirm_reset.set(false);
                        *version.write() += 1;
                    },
                }
            }
        }
    }
}

#[component]
fn InviteRow(invite: MyInvite, on_change: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let toast_sig = use_context::<ToastSig>();
    let label = if invite.main {
        t(&tr, "address.main_link")
    } else {
        invite.label.clone()
    };
    let expiry = match (invite.max_uses, invite.expires_at) {
        (Some(1), _) => t(&tr, "address.works_once"),
        (_, Some(at)) => t(&tr, "address.expires_in")
            .replace("{n}", &((at - now_ms()).max(0) / DAY + 1).to_string()),
        _ => t(&tr, "address.never_expires"),
    };
    let note = t(&tr, "address.used")
        .replace("{what}", &expiry)
        .replace("{n}", &invite.uses.to_string());
    let id = invite.id.clone();
    let link = core.invite_link_for(&invite.id);
    rsx! {
        div { class: "row still",
            span { class: "stack",
                span { class: "row-t one-line", "{label}" }
                span { class: "sub", "{note}" }
            }
            if let Some(link) = link {
                button {
                    class: "ib",
                    aria_label: "{t(&tr, \"add.share\")}",
                    onclick: move |_| {
                        let text = t(&tr, "add.share_text").replace("{link}", &link);
                        share_or_copy(tr, toast_sig, text, t(&tr, "add.share"));
                    },
                    Icon { name: IconName::Share, size: 20 }
                }
            }
            if !invite.main {
                button {
                    class: "btn small-btn",
                    "data-t": "revoke",
                    onclick: move |_| {
                        core.revoke_invite(&id);
                        on_change.call(());
                    },
                    "{t(&tr, \"address.revoke\")}"
                }
            }
        }
    }
}

#[component]
fn NewLinkSheet(on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let toast_sig = use_context::<ToastSig>();
    let mut label = use_signal(String::new);
    let mut lasts = use_signal(|| "forever".to_owned());
    let options = vec![
        ("forever".to_owned(), t(&tr, "address.forever")),
        ("week".to_owned(), t(&tr, "address.a_week")),
        ("once".to_owned(), t(&tr, "address.once")),
    ];
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "address.new_link"),
            label { class: "field",
                span { class: "gt", "{t(&tr, \"address.link_name\")}" }
                input {
                    r#type: "text",
                    maxlength: "40",
                    placeholder: "{t(&tr, \"address.link_name_placeholder\")}",
                    value: "{label}",
                    oninput: move |e| label.set(e.value()),
                }
            }
            h3 { "{t(&tr, \"address.lasts\")}" }
            Chips {
                options,
                value: lasts(),
                on_change: move |v: String| lasts.set(v),
            }
            button {
                class: "btn main",
                "data-t": "make-link",
                disabled: label().trim().is_empty(),
                onclick: move |_| {
                    let (expires, uses) = match lasts.peek().as_str() {
                        "week" => (Some(now_ms() + 7 * DAY), None),
                        "once" => (None, Some(1)),
                        _ => (None, None),
                    };
                    let made = core.new_invite(label.peek().trim(), expires, uses);
                    let text = t(&tr, "add.share_text").replace("{link}", &made);
                    share_or_copy(tr, toast_sig, text, t(&tr, "add.share"));
                    on_close.call(());
                },
                "{t(&tr, \"address.make_link\")}"
            }
        }
    }
}

#[component]
fn ResetSheet(on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let settings = use_settings();
    let mut also_name = use_signal(|| false);
    let mut new_name = use_signal(String::new);
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "address.reset_title"), tag: "reset-sheet",
            for key in ["address.reset_1", "address.reset_2", "address.reset_3", "address.reset_4"] {
                div { key: "{key}", class: "mark", style: "align-items:flex-start;gap:12px",
                    span { class: "accent-text", style: "display:flex",
                        Icon { name: IconName::Check, size: 20 }
                    }
                    span { "{t(&tr, key)}" }
                }
            }
            SwitchRow {
                title: t(&tr, "address.reset_name"),
                checked: also_name(),
                on_change: move |on| also_name.set(on),
            }
            if also_name() {
                input {
                    r#type: "text",
                    maxlength: "64",
                    placeholder: "{t(&tr, \"onboarding.name_placeholder\")}",
                    value: "{new_name}",
                    oninput: move |e| new_name.set(e.value()),
                }
            }
            div { class: "btns",
                button { class: "btn", onclick: move |_| on_close.call(()),
                    "{t(&tr, \"common.cancel\")}"
                }
                button {
                    class: "btn danger",
                    "data-t": "reset-yes",
                    onclick: move |_| {
                        if *also_name.peek() {
                            let _ = core.clear_avatar();
                            let name = new_name.peek().trim().to_owned();
                            change(settings, |s| {
                                s.avatar = None;
                                if !name.is_empty() {
                                    s.display_name = name;
                                }
                            });
                        }
                        core.reset_public_address();
                        toast(&mut toast_sig, t(&tr, "address.reset_done"), ToastLevel::Info);
                        on_close.call(());
                    },
                    "{t(&tr, \"address.reset_short\")}"
                }
            }
        }
    }
}
