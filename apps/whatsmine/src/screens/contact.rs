use dioxus::prelude::*;
use whatsmine_core::ContactState;

use crate::app::{Rev, Route, SettingsSig, ToastSig, t, toast, use_core, use_tr};
use crate::components::{Avatar, ConfirmDialog, Icon, IconName, ToastLevel, ToggleRow};
use crate::fmt;
use crate::js;

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    DeleteChat,
    Block,
    Remove,
}

#[component]
pub fn ContactInfo(id: String) -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let rev = use_context::<Rev>().0;
    let settings = use_context::<SettingsSig>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let mut confirm = use_signal(|| None::<Confirm>);
    let mut nickname = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || {
            core.contact(&id)
                .and_then(|c| c.nickname)
                .unwrap_or_default()
        })
    };

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
    let name = contact.display_name();
    let online = core.is_online(&id);
    let presence = if online {
        t(&tr, "chat.online")
    } else if let (true, Some(ts)) = (settings.read().share_last_seen, contact.last_seen) {
        fmt::last_seen(&tr, ts)
    } else {
        String::new()
    };
    let blocked = contact.state == ContactState::Blocked;
    let calls_themselves =
        t(&tr, "contact.calls_themselves").replace("{name}", &contact.their_name);

    let (c_save, c_act, c_unblock) = (core.clone(), core.clone(), core.clone());
    let (c_pin, c_mute) = (core.clone(), core.clone());
    let (id_save, id_act, id_unblock, id_copy) = (id.clone(), id.clone(), id.clone(), id.clone());
    let (id_pin, id_mute, id_chat) = (id.clone(), id.clone(), id.clone());

    rsx! {
        div { class: "screen no-nav",
            div { class: "config-header",
                button { class: "back-btn", onclick: move |_| nav.go_back(),
                    Icon { name: IconName::ArrowLeft }
                }
                span { class: "config-title", "{t(&tr, \"contact.title\")}" }
            }
            div { class: "profile-head",
                Avatar { id: id.clone(), name: name.clone(), size: "large", online, image: contact.avatar.clone(), zoom: true }
                span { class: "profile-name", "{name}" }
                if !contact.their_name.is_empty() && contact.their_name != name {
                    span { class: "profile-sub", "{calls_themselves}" }
                }
                if !presence.is_empty() {
                    span { class: "profile-sub", "{presence}" }
                }
            }

            div { class: "profile-actions",
                button {
                    class: "profile-action",
                    onclick: move |_| {
                        nav.push(Route::Chat { id: id_chat.clone() });
                    },
                    Icon { name: IconName::Chat, size: 22 }
                    "{t(&tr, \"contact.message\")}"
                }
            }

            div { class: "settings-section",
                ToggleRow {
                    label: t(&tr, "contact.pin"),
                    checked: contact.pinned,
                    on_change: move |v| c_pin.set_pinned(&id_pin, v),
                }
                ToggleRow {
                    label: t(&tr, "contact.mute"),
                    hint: t(&tr, "contact.mute_hint"),
                    checked: contact.muted,
                    on_change: move |v| c_mute.set_muted(&id_mute, v),
                }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"contact.nickname\")}" }
                div { class: "input-row",
                    input {
                        r#type: "text",
                        maxlength: "64",
                        placeholder: "{contact.their_name}",
                        value: "{nickname}",
                        oninput: move |e| nickname.set(e.value()),
                    }
                    button {
                        class: "secondary-btn",
                        onclick: move |_| {
                            c_save.rename(&id_save, &nickname.peek());
                            toast(&mut toast_sig, t(&tr, "common.saved"), ToastLevel::Info);
                        },
                        "{t(&tr, \"common.save\")}"
                    }
                }
                p { class: "setting-hint", "{t(&tr, \"contact.nickname_hint\")}" }
            }

            div { class: "settings-section",
                label { class: "section-label", "{t(&tr, \"contact.key\")}" }
                div { class: "key-box", "{fmt::grouped_key(&id)}" }
                p { class: "setting-hint", "{t(&tr, \"contact.key_hint\")}" }
                button {
                    class: "secondary-btn",
                    style: "margin-top: 10px;",
                    onclick: move |_| {
                        js::copy_text(&id_copy);
                        toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                    },
                    Icon { name: IconName::Copy, size: 18 }
                    "{t(&tr, \"common.copy\")}"
                }
            }

            div { class: "settings-section",
                if blocked {
                    button {
                        class: "save-btn",
                        onclick: move |_| {
                            let _ = c_unblock.unblock(&id_unblock);
                        },
                        "{t(&tr, \"contact.unblock\")}"
                    }
                } else {
                    button { class: "delete-btn", onclick: move |_| confirm.set(Some(Confirm::DeleteChat)),
                        "{t(&tr, \"contact.delete_chat\")}"
                    }
                    button { class: "delete-btn", onclick: move |_| confirm.set(Some(Confirm::Block)),
                        "{t(&tr, \"contact.block\")}"
                    }
                }
                button { class: "delete-btn", onclick: move |_| confirm.set(Some(Confirm::Remove)),
                    "{t(&tr, \"contact.delete\")}"
                }
            }

            if let Some(which) = confirm() {
                ConfirmDialog {
                    title: match which {
                        Confirm::DeleteChat => t(&tr, "contact.delete_chat_title").replace("{name}", &name),
                        Confirm::Block => t(&tr, "contact.block_title").replace("{name}", &name),
                        Confirm::Remove => t(&tr, "contact.delete_title").replace("{name}", &name),
                    },
                    message: match which {
                        Confirm::DeleteChat => t(&tr, "contact.delete_chat_message").replace("{name}", &name),
                        Confirm::Block => t(&tr, "contact.block_message"),
                        Confirm::Remove => t(&tr, "contact.delete_message"),
                    },
                    confirm_label: match which {
                        Confirm::DeleteChat => t(&tr, "contact.delete_chat"),
                        Confirm::Block => t(&tr, "contact.block"),
                        Confirm::Remove => t(&tr, "contact.delete"),
                    },
                    on_confirm: move |_| {
                        confirm.set(None);
                        match which {
                            Confirm::DeleteChat => c_act.hide_chat(&id_act),
                            Confirm::Block => {
                                let _ = c_act.block(&id_act);
                            }
                            Confirm::Remove => {
                                let _ = c_act.delete_contact(&id_act);
                                nav.replace(Route::ContactsScreen {});
                            }
                        }
                    },
                    on_cancel: move |_| confirm.set(None),
                }
            }
        }
    }
}
