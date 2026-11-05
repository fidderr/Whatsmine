use dioxus::prelude::*;
use whatsmine_core::rich::{ChatPrefs, SearchIn};
use whatsmine_core::{ContactState, Trust};

use crate::app::{Rev, Route, SettingsSig, ToastSig, t, toast, use_core, use_tr};
use crate::call::{self, CallBridge, CallUi};
use crate::components::{
    Avatar, BackButton, Chips, ConfirmDialog, Icon, IconName, Row, Sheet, SwitchRow, ToastLevel,
};
use crate::fmt;
use crate::js;
use crate::screens::add::share_or_copy;
use crate::screens::people::TrustMark;

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    DeleteChat,
    Block,
    Remove,
}

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Notes,
    Nickname,
    Disappear,
    Words,
    Key,
    Sees,
    Labels,
}

/// How long messages can stay, for the chips that choose it.
pub fn disappear_options(tr: &crate::app::Translations) -> Vec<(String, String)> {
    vec![
        ("0".to_owned(), t(tr, "disappear.off")),
        ("3600000".to_owned(), t(tr, "disappear.hour")),
        ("86400000".to_owned(), t(tr, "disappear.day")),
        ("604800000".to_owned(), t(tr, "disappear.week")),
        ("2592000000".to_owned(), t(tr, "disappear.month")),
    ]
}

pub fn disappear_label(tr: &crate::app::Translations, ms: Option<i64>) -> String {
    let key = ms.unwrap_or(0).to_string();
    disappear_options(tr)
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, l)| l)
        .unwrap_or_else(|| t(tr, "disappear.off"))
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
    let bridge = use_context::<CallBridge>();
    let phase = use_context::<CallUi>().0;
    let mut confirm = use_signal(|| None::<Confirm>);
    let mut open = use_signal(|| None::<Open>);

    let _ = rev();
    let Some(contact) = core.contact(&id) else {
        return rsx! {
            div { class: "screen no-nav",
                header { class: "hdr", BackButton {} }
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
    let keys = core.contact_keys(&id);
    let prefs = core.chat_prefs(&id);
    let media = core.search_chat(&id, "", SearchIn::Media);
    let files = core.search_chat(&id, "", SearchIn::Files).len();
    let links = core.search_chat(&id, "", SearchIn::Links).len();
    let shared_count = media.len() + files + links;
    let shared_note = t(&tr, "contact.shared_count").replace("{n}", &shared_count.to_string());
    let notes_value: String = prefs.notes.chars().take(24).collect();
    let block_label = t(&tr, "contact.block_name").replace("{name}", &name);

    let save = {
        let (core, id) = (core.clone(), id.clone());
        move |p: &ChatPrefs| core.set_chat_prefs(&id, p)
    };
    let (save_fav, save_sheet) = (save.clone(), save);
    let (c_pin, c_mute, c_act, c_unblock) =
        (core.clone(), core.clone(), core.clone(), core.clone());
    let (id_pin, id_mute, id_act, id_unblock) = (id.clone(), id.clone(), id.clone(), id.clone());
    let (id_msg, id_search) = (id.clone(), id.clone());
    let (voice_core, voice_id, voice_bridge) = (core.clone(), id.clone(), bridge.clone());
    let (video_core, video_id, video_bridge) = (core.clone(), id.clone(), bridge);
    let card = core.contact_card(&id);
    let fav_prefs = prefs.clone();
    let favourite = prefs.favourite;

    rsx! {
        div { class: "screen no-nav",
            header { class: "hdr", style: "padding-bottom:0",
                BackButton {}
                span { style: "flex:1" }
                if let Some(card) = card {
                    button {
                        class: "ib",
                        aria_label: "{t(&tr, \"contact.share\")}",
                        onclick: move |_| {
                            share_or_copy(tr, toast_sig, card.clone(), t(&tr, "contact.share"));
                        },
                        Icon { name: IconName::Share, size: 22 }
                    }
                }
            }
            div { class: "body g12",
                div { class: "panel center", style: "background:transparent;padding:0;gap:4px",
                    Avatar { id: id.clone(), name: name.clone(), size: "big", online, image: contact.avatar.clone(), zoom: true }
                    h1 { style: "font-size:1.75rem;line-height:1.3;font-weight:700", "{name}" }
                    if !contact.their_name.is_empty() && contact.their_name != name {
                        span { class: "sub", "{calls_themselves}" }
                    }
                    if !presence.is_empty() {
                        span { class: "sub", "{presence}" }
                    }
                    TrustMark { trust: keys.trust }
                }
                if !blocked {
                    div { class: "tile-actions",
                        button {
                            class: "tile-action",
                            "data-t": "message",
                            onclick: move |_| {
                                nav.push(Route::Chat { id: id_msg.clone() });
                            },
                            Icon { name: IconName::Chat, size: 22 }
                            "{t(&tr, \"contact.message\")}"
                        }
                        button {
                            class: "tile-action",
                            onclick: move |_| {
                                call::start_call(&voice_core, &voice_id, false, phase, &voice_bridge, &mut toast_sig, &tr);
                            },
                            Icon { name: IconName::Phone, size: 22 }
                            "{t(&tr, \"call.voice_short\")}"
                        }
                        button {
                            class: "tile-action",
                            onclick: move |_| {
                                call::start_call(&video_core, &video_id, true, phase, &video_bridge, &mut toast_sig, &tr);
                            },
                            Icon { name: IconName::Video, size: 22 }
                            "{t(&tr, \"call.video_short\")}"
                        }
                        Link {
                            class: "tile-action",
                            to: Route::SharedMedia { id: id_search.clone() },
                            Icon { name: IconName::Search, size: 22 }
                            "{t(&tr, \"contact.search\")}"
                        }
                    }
                }
                div { class: "group",
                    Row {
                        icon: IconName::Image,
                        title: t(&tr, "contact.shared"),
                        value: shared_note,
                        small: true,
                        to: Route::SharedMedia { id: id.clone() },
                        tag: "shared",
                    }
                }
                div { class: "group",
                    SwitchRow {
                        title: t(&tr, "contact.favourite"),
                        small: true,
                        checked: favourite,
                        tag: "favourite",
                        on_change: move |on| {
                            let mut p = fav_prefs.clone();
                            p.favourite = on;
                            save_fav(&p);
                        },
                    }
                    SwitchRow {
                        title: t(&tr, "contact.pin"),
                        small: true,
                        checked: contact.pinned,
                        on_change: move |v| c_pin.set_pinned(&id_pin, v),
                    }
                    SwitchRow {
                        title: t(&tr, "contact.mute"),
                        note: t(&tr, "contact.mute_hint"),
                        checked: contact.muted,
                        on_change: move |v| c_mute.set_muted(&id_mute, v),
                    }
                }
                div { class: "group",
                    Row {
                        icon: IconName::Pencil,
                        title: t(&tr, "contact.nickname"),
                        value: contact.nickname.clone().unwrap_or_default(),
                        small: true,
                        tag: "nickname",
                        on_tap: move |_| open.set(Some(Open::Nickname)),
                    }
                    Row {
                        icon: IconName::Book,
                        title: t(&tr, "contact.notes"),
                        value: notes_value,
                        small: true,
                        tag: "notes",
                        on_tap: move |_| open.set(Some(Open::Notes)),
                    }
                    Row {
                        icon: IconName::Folder,
                        title: t(&tr, "contact.labels"),
                        value: prefs.labels.join(", "),
                        small: true,
                        tag: "labels",
                        on_tap: move |_| open.set(Some(Open::Labels)),
                    }
                    Row {
                        icon: IconName::Clock,
                        title: t(&tr, "contact.disappear"),
                        value: disappear_label(&tr, prefs.disappear_ms),
                        small: true,
                        tag: "disappear",
                        on_tap: move |_| open.set(Some(Open::Disappear)),
                    }
                    Row {
                        icon: IconName::Palette,
                        title: t(&tr, "look.title"),
                        small: true,
                        to: Route::ChatLook { id: id.clone() },
                        tag: "look",
                    }
                    Row {
                        icon: IconName::People,
                        title: t(&tr, "contact.sees").replace("{name}", &name),
                        small: true,
                        tag: "sees",
                        on_tap: move |_| open.set(Some(Open::Sees)),
                    }
                    Row {
                        icon: IconName::Shield,
                        title: t(&tr, "contact.words"),
                        value: if keys.trust == Trust::SafetyWords { t(&tr, "contact.words_done") } else { String::new() },
                        small: true,
                        tag: "words",
                        on_tap: move |_| open.set(Some(Open::Words)),
                    }
                    Row {
                        icon: IconName::Key,
                        title: t(&tr, "contact.key"),
                        small: true,
                        tag: "key",
                        on_tap: move |_| open.set(Some(Open::Key)),
                    }
                }
                div { class: "group",
                    if blocked {
                        Row {
                            icon: IconName::Check,
                            title: t(&tr, "contact.unblock"),
                            small: true,
                            on_tap: move |_| {
                                let _ = c_unblock.unblock(&id_unblock);
                            },
                        }
                    } else {
                        Row {
                            icon: IconName::Trash,
                            title: t(&tr, "contact.delete_chat"),
                            small: true,
                            danger: true,
                            on_tap: move |_| confirm.set(Some(Confirm::DeleteChat)),
                        }
                        Row {
                            icon: IconName::Ban,
                            title: block_label,
                            small: true,
                            danger: true,
                            tag: "block",
                            on_tap: move |_| confirm.set(Some(Confirm::Block)),
                        }
                    }
                    Row {
                        icon: IconName::Xmark,
                        title: t(&tr, "contact.delete"),
                        small: true,
                        danger: true,
                        on_tap: move |_| confirm.set(Some(Confirm::Remove)),
                    }
                }
            }
            match open() {
                Some(Open::Nickname) => rsx! {
                    NicknameSheet { id: id.clone(), their_name: contact.their_name.clone(), current: contact.nickname.clone().unwrap_or_default(), on_close: move |_| open.set(None) }
                },
                Some(Open::Notes) => rsx! {
                    NotesSheet { prefs: prefs.clone(), on_save: move |p: ChatPrefs| save_sheet(&p), on_close: move |_| open.set(None) }
                },
                Some(Open::Labels) => rsx! {
                    LabelsSheet { id: id.clone(), on_close: move |_| open.set(None) }
                },
                Some(Open::Disappear) => rsx! {
                    DisappearSheet { id: id.clone(), on_close: move |_| open.set(None) }
                },
                Some(Open::Words) => rsx! {
                    WordsSheet { id: id.clone(), name: name.clone(), on_close: move |_| open.set(None) }
                },
                Some(Open::Sees) => rsx! {
                    SeesSheet { id: id.clone(), name: name.clone(), on_close: move |_| open.set(None) }
                },
                Some(Open::Key) => rsx! {
                    Sheet { on_close: move |_| open.set(None), title: t(&tr, "contact.key"),
                        div { class: "key-box", "{fmt::grouped_key(&id)}" }
                        p { class: "note", "{t(&tr, \"contact.key_hint\")}" }
                        button {
                            class: "btn",
                            onclick: {
                                let id = id.clone();
                                move |_| {
                                    js::copy_text(&id);
                                    toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                                }
                            },
                            Icon { name: IconName::Copy, size: 18 }
                            "{t(&tr, \"common.copy\")}"
                        }
                    }
                },
                None => rsx! {},
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
                        Confirm::Remove => t(&tr, "contact.delete_message").replace("{name}", &name),
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
                                nav.replace(Route::People {});
                            }
                        }
                    },
                    on_cancel: move |_| confirm.set(None),
                }
            }
        }
    }
}

#[component]
fn NicknameSheet(
    id: String,
    their_name: String,
    current: String,
    on_close: EventHandler<()>,
) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut text = use_signal(|| current);
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "contact.nickname"),
            input {
                r#type: "text",
                maxlength: "64",
                autofocus: true,
                placeholder: "{their_name}",
                value: "{text}",
                oninput: move |e| text.set(e.value()),
            }
            p { class: "note", "{t(&tr, \"contact.nickname_hint\")}" }
            button {
                class: "btn main",
                "data-t": "save",
                onclick: move |_| {
                    core.rename(&id, &text.peek());
                    on_close.call(());
                },
                "{t(&tr, \"common.save\")}"
            }
        }
    }
}

#[component]
fn NotesSheet(
    prefs: ChatPrefs,
    on_save: EventHandler<ChatPrefs>,
    on_close: EventHandler<()>,
) -> Element {
    let tr = use_tr();
    let mut text = use_signal(|| prefs.notes.clone());
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "contact.notes"),
            textarea {
                rows: "4",
                maxlength: "2000",
                autofocus: true,
                value: "{text}",
                oninput: move |e| text.set(e.value()),
            }
            p { class: "note", "{t(&tr, \"contact.notes_hint\")}" }
            button {
                class: "btn main",
                "data-t": "save",
                onclick: move |_| {
                    let mut p = prefs.clone();
                    p.notes = text.peek().clone();
                    on_save.call(p);
                    on_close.call(());
                },
                "{t(&tr, \"common.save\")}"
            }
        }
    }
}

/// Words of your own for this chat, for folders to go by.
#[component]
fn LabelsSheet(id: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut text = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.chat_prefs(&id).labels.join(", "))
    };
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "contact.labels"),
            input {
                r#type: "text",
                autofocus: true,
                placeholder: "{t(&tr, \"folders.labels_placeholder\")}",
                value: "{text}",
                oninput: move |e| text.set(e.value()),
            }
            p { class: "note", "{t(&tr, \"contact.labels_note\")}" }
            button {
                class: "btn main",
                onclick: move |_| {
                    let mut p = core.chat_prefs(&id);
                    p.labels = text
                        .peek()
                        .split(',')
                        .map(|l| l.trim().to_owned())
                        .filter(|l| !l.is_empty())
                        .collect();
                    core.set_chat_prefs(&id, &p);
                    on_close.call(());
                },
                "{t(&tr, \"common.save\")}"
            }
        }
    }
}

/// How long messages stay in this chat. Goes for both sides.
#[component]
pub fn DisappearSheet(id: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut prefs = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.chat_prefs(&id))
    };
    let value = prefs.read().disappear_ms.unwrap_or(0).to_string();
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "contact.disappear"), tag: "disappear-sheet",
            p { class: "note", "{t(&tr, \"contact.disappear_note\")}" }
            Chips {
                options: disappear_options(&tr),
                value,
                on_change: move |v: String| {
                    let ms = v.parse::<i64>().unwrap_or(0);
                    let mut p = prefs.peek().clone();
                    p.disappear_ms = (ms > 0).then_some(ms);
                    core.set_chat_prefs(&id, &p);
                    prefs.set(p);
                },
            }
        }
    }
}

/// Compare these with them, on a call or face to face: they are sure it is you, and you them.
#[component]
fn WordsSheet(id: String, name: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let words = core.safety_words(&id);
    let compared = core.contact_keys(&id).trust == Trust::SafetyWords;
    let intro = t(&tr, "contact.words_text").replace("{name}", &name);
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "contact.words"), tag: "words-sheet",
            p { class: "note", "{intro}" }
            div { class: "words",
                for word in words {
                    span { key: "{word}", "{word}" }
                }
            }
            if compared {
                span { class: "mark ok-text",
                    Icon { name: IconName::Shield, size: 16 }
                    "{t(&tr, \"trust.words\")}"
                }
            } else {
                button {
                    class: "btn main",
                    "data-t": "words-same",
                    onclick: move |_| {
                        core.mark_safety_words(&id);
                        on_close.call(());
                    },
                    "{t(&tr, \"contact.words_same\")}"
                }
            }
        }
    }
}

/// What this one person gets to see of you.
#[component]
fn SeesSheet(id: String, name: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut prefs = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.chat_prefs(&id))
    };
    let mut my_name = use_signal(|| prefs.peek().my_name.clone().unwrap_or_default());
    let current = prefs();
    let save_core = core.clone();
    let save_id = id.clone();
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "contact.sees").replace("{name}", &name),
            div { class: "sheet-list",
                SwitchRow {
                    title: t(&tr, "contact.hide_presence"),
                    note: t(&tr, "contact.hide_presence_note"),
                    checked: current.hide_presence,
                    on_change: move |on| {
                        let mut p = prefs.peek().clone();
                        p.hide_presence = on;
                        core.set_chat_prefs(&id, &p);
                        prefs.set(p);
                    },
                }
            }
            label { class: "field",
                span { class: "gt", "{t(&tr, \"contact.my_name\")}" }
                input {
                    r#type: "text",
                    maxlength: "64",
                    placeholder: "{t(&tr, \"contact.my_name_placeholder\")}",
                    value: "{my_name}",
                    oninput: move |e| my_name.set(e.value()),
                }
            }
            button {
                class: "btn main",
                onclick: move |_| {
                    let mut p = prefs.peek().clone();
                    let name = my_name.peek().trim().to_owned();
                    p.my_name = (!name.is_empty()).then_some(name);
                    save_core.set_chat_prefs(&save_id, &p);
                    on_close.call(());
                },
                "{t(&tr, \"common.save\")}"
            }
        }
    }
}
