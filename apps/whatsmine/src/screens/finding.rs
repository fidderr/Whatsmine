//! Finding things again: search, your own list, shared media, and folders.

use dioxus::prelude::*;
use whatsmine_core::Message;
use whatsmine_core::rich::{Folder, SearchHit, SearchIn};

use crate::app::{Rev, Route, t, use_core, use_tr};
use crate::components::{
    Avatar, BackButton, Chips, Header, Icon, IconName, Row, Sheet, SwitchRow, change, use_settings,
};
use crate::fmt;

fn message_line(m: &Message) -> String {
    match &m.attachment {
        Some(a) if m.body.is_empty() => a.name.clone(),
        _ => m.body.clone(),
    }
}

#[component]
pub fn SearchScreen() -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut query = use_signal(String::new);
    let mut within = use_signal(|| "all".to_owned());
    let options = vec![
        ("all".to_owned(), t(&tr, "search.all")),
        ("people".to_owned(), t(&tr, "search.people")),
        ("media".to_owned(), t(&tr, "search.media")),
        ("files".to_owned(), t(&tr, "search.files")),
        ("links".to_owned(), t(&tr, "search.links")),
    ];
    let kind = match within().as_str() {
        "people" => SearchIn::People,
        "media" => SearchIn::Media,
        "files" => SearchIn::Files,
        "links" => SearchIn::Links,
        _ => SearchIn::All,
    };
    let text = query();
    let hits = if text.trim().is_empty() && matches!(kind, SearchIn::All | SearchIn::People) {
        Vec::new()
    } else {
        core.search(&text, kind)
    };
    rsx! {
        div { class: "screen no-nav",
            header { class: "hdr",
                BackButton {}
                label { class: "search-box",
                    Icon { name: IconName::Search, size: 18 }
                    input {
                        r#type: "search",
                        autofocus: true,
                        "data-t": "search-input",
                        placeholder: "{t(&tr, \"chats.search\")}",
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                    }
                }
            }
            div { class: "body",
                Chips {
                    options,
                    value: within(),
                    on_change: move |v: String| within.set(v),
                }
                if hits.is_empty() && !text.trim().is_empty() {
                    div { class: "empty-state",
                        p { class: "empty-subtitle", "{t(&tr, \"chats.no_match\")}" }
                    }
                } else if !hits.is_empty() {
                    div { class: "group", "data-t": "search-hits",
                        for hit in hits {
                            match hit {
                                SearchHit::Person(id) => rsx! {
                                    PersonHit { key: "{id}", id }
                                },
                                SearchHit::Message { chat, id } => rsx! {
                                    MessageHit { key: "{id}", chat, id }
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PersonHit(id: String) -> Element {
    let core = use_core();
    let contact = core.contact(&id);
    let name = contact
        .as_ref()
        .map(|c| c.display_name())
        .unwrap_or_default();
    rsx! {
        Link { class: "row", to: Route::ContactInfo { id: id.clone() },
            Avatar {
                id: id.clone(),
                name: name.clone(),
                size: "medium",
                image: contact.as_ref().and_then(|c| c.avatar.clone()),
            }
            span { class: "row-t stack one-line", "{name}" }
        }
    }
}

#[component]
fn MessageHit(chat: String, id: String) -> Element {
    let tr = use_tr();
    let core = use_core();
    let Some(m) = core.message(&id) else {
        return rsx! {};
    };
    let contact = core.contact(&chat);
    let name = contact
        .as_ref()
        .map(|c| c.display_name())
        .unwrap_or_default();
    rsx! {
        Link { class: "row", to: Route::Chat { id: chat.clone() },
            Avatar {
                id: chat.clone(),
                name: name.clone(),
                size: "medium",
                image: contact.as_ref().and_then(|c| c.avatar.clone()),
            }
            span { class: "stack",
                span { class: "chat-line",
                    span { class: "chat-name", "{name}" }
                    span { class: "chat-time", "{fmt::list_time(&tr, m.sent_at)}" }
                }
                span { class: "sub one-line", "{message_line(&m)}" }
            }
        }
    }
}

/// Your own list: what you saved and what you asked to be reminded of.
#[component]
pub fn SavedScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let rev = use_context::<Rev>().0;
    let _ = rev();
    let saved = core.saved_messages();
    let reminders = core.reminders();
    let scheduled = core.scheduled();
    let mut tags: Vec<String> = saved.iter().flat_map(|m| m.extra.tags.clone()).collect();
    tags.sort();
    tags.dedup();
    let untagged: Vec<Message> = saved
        .iter()
        .filter(|m| m.extra.tags.is_empty())
        .cloned()
        .collect();
    let all_empty = saved.is_empty() && reminders.is_empty() && scheduled.is_empty();
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "saved.title") }
            div { class: "body",
                div { class: "group",
                    Row {
                        icon: IconName::Pencil,
                        title: t(&tr, "saved.notes"),
                        note: t(&tr, "saved.notes_note"),
                        tag: "notes-chat",
                        on_tap: {
                            let core = core.clone();
                            move |_| {
                                let id = core.notes_chat(&t(&tr, "saved.notes"));
                                nav.push(Route::Chat { id });
                            }
                        },
                    }
                }
                if all_empty {
                    div { class: "empty-state",
                        Icon { name: IconName::Bookmark, size: 56 }
                        p { class: "empty-title", "{t(&tr, \"saved.empty_title\")}" }
                        p { class: "empty-subtitle", "{t(&tr, \"saved.empty_text\")}" }
                    }
                }
                if !reminders.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"saved.reminders\")}" }
                    div { class: "group",
                        for (id, at) in reminders {
                            if let Some(m) = core.message(&id) {
                                SavedRow { key: "{id}", message: m, note: t(&tr, "saved.remind_at").replace("{when}", &fmt::when(&tr, at)) }
                            }
                        }
                    }
                }
                if !scheduled.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"saved.scheduled\")}" }
                    div { class: "group",
                        for m in scheduled {
                            SavedRow { key: "{m.id}", note: t(&tr, "saved.goes_at").replace("{when}", &fmt::when(&tr, m.sent_at)), message: m }
                        }
                    }
                }
                for tag in tags {
                    Fragment { key: "{tag}",
                        h2 { class: "gt", "{tag}" }
                        div { class: "group",
                            for m in saved.iter().filter(|m| m.extra.tags.contains(&tag)).cloned() {
                                SavedRow { key: "{m.id}", note: String::new(), message: m }
                            }
                        }
                    }
                }
                if !untagged.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"saved.saved\")}" }
                    div { class: "group",
                        for m in untagged {
                            SavedRow { key: "{m.id}", note: String::new(), message: m }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SavedRow(message: Message, note: String) -> Element {
    let tr = use_tr();
    let core = use_core();
    let name = core
        .contact(&message.contact_id)
        .map(|c| c.display_name())
        .unwrap_or_default();
    let shown = if note.is_empty() {
        format!("{name}, {}", fmt::list_time(&tr, message.sent_at))
    } else {
        format!("{name}, {note}")
    };
    rsx! {
        Link { class: "row", to: Route::Chat { id: message.contact_id.clone() },
            span { class: "stack",
                span { class: "row-t one-line", "{message_line(&message)}" }
                span { class: "sub one-line", "{shown}" }
            }
        }
    }
}

/// What was shared in one chat.
#[component]
pub fn SharedMedia(id: String) -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let mut query = use_signal(String::new);
    let mut within = use_signal(|| "media".to_owned());
    let options = vec![
        ("media".to_owned(), t(&tr, "search.media")),
        ("files".to_owned(), t(&tr, "search.files")),
        ("links".to_owned(), t(&tr, "search.links")),
        ("all".to_owned(), t(&tr, "search.all")),
    ];
    let kind = match within().as_str() {
        "files" => SearchIn::Files,
        "links" => SearchIn::Links,
        "all" => SearchIn::All,
        _ => SearchIn::Media,
    };
    let items = core.search_chat(&id, &query(), kind);
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "contact.shared") }
            div { class: "body",
                label { class: "search-box",
                    Icon { name: IconName::Search, size: 18 }
                    input {
                        r#type: "search",
                        placeholder: "{t(&tr, \"chats.search\")}",
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                    }
                }
                Chips {
                    options,
                    value: within(),
                    on_change: move |v: String| within.set(v),
                }
                if items.is_empty() {
                    div { class: "empty-state",
                        p { class: "empty-subtitle", "{t(&tr, \"chats.no_match\")}" }
                    }
                } else {
                    div { class: "group",
                        for m in items {
                            SavedRow { key: "{m.id}", note: String::new(), message: m }
                        }
                    }
                }
            }
        }
    }
}

/// Folders appear as chips above the chats.
#[component]
pub fn FoldersScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let _ = rev();
    let mut editing = use_signal(|| None::<(usize, Folder)>);
    let settings = use_settings();
    let mut quick = use_signal(|| false);
    let mut reacting = use_signal(|| false);
    let folders = core.folders();
    let archived = core.chats().iter().filter(|c| c.prefs.archived).count();
    let count = folders.len();
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "folders.title") }
            div { class: "body",
                p { class: "note", "{t(&tr, \"folders.intro\")}" }
                if !folders.is_empty() {
                    div { class: "group",
                        for (i, folder) in folders.iter().cloned().enumerate() {
                            {
                                let chats = core.chats_in(&folder).len();
                                let edit = folder.clone();
                                rsx! {
                                    Row {
                                        key: "{folder.name}",
                                        icon: IconName::Folder,
                                        title: folder.name.clone(),
                                        value: chats.to_string(),
                                        tag: "folder",
                                        on_tap: move |_| editing.set(Some((i, edit.clone()))),
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "group",
                    Row {
                        icon: IconName::Reply,
                        title: t(&tr, "folders.quick_replies"),
                        note: t(&tr, "folders.quick_replies_note"),
                        tag: "quick-replies",
                        on_tap: move |_| quick.set(true),
                    }
                    Row {
                        icon: IconName::Smile,
                        title: t(&tr, "folders.reactions"),
                        value: settings.read().reactions.join(" "),
                        tag: "reactions",
                        on_tap: move |_| reacting.set(true),
                    }
                }
                button {
                    class: "btn",
                    "data-t": "new-folder",
                    onclick: move |_| editing.set(Some((count, Folder::default()))),
                    Icon { name: IconName::Plus, size: 20 }
                    "{t(&tr, \"folders.new\")}"
                }
                if archived > 0 {
                    div { class: "group",
                        Row {
                            icon: IconName::Archive,
                            title: t(&tr, "chats.archive"),
                            value: archived.to_string(),
                            small: true,
                            to: Route::Chats {},
                        }
                    }
                }
            }
            if let Some((index, folder)) = editing() {
                FolderSheet { index, folder, on_close: move |_| editing.set(None) }
            }
            if quick() {
                WordsSheet {
                    title: t(&tr, "folders.quick_replies"),
                    note: t(&tr, "folders.quick_replies_note"),
                    start: settings.read().quick_replies.join("\n"),
                    split_on: "\n",
                    on_save: move |lines: Vec<String>| change(settings, |s| s.quick_replies = lines),
                    on_close: move |_| quick.set(false),
                }
            }
            if reacting() {
                WordsSheet {
                    title: t(&tr, "folders.reactions"),
                    note: t(&tr, "folders.reactions_note"),
                    start: settings.read().reactions.join(" "),
                    split_on: " ",
                    on_save: move |lines: Vec<String>| {
                        if !lines.is_empty() {
                            change(settings, |s| s.reactions = lines);
                        }
                    },
                    on_close: move |_| reacting.set(false),
                }
            }
        }
    }
}

/// A short list a person writes themselves.
#[component]
fn WordsSheet(
    title: String,
    note: String,
    start: String,
    split_on: &'static str,
    on_save: EventHandler<Vec<String>>,
    on_close: EventHandler<()>,
) -> Element {
    let tr = use_tr();
    let mut text = use_signal(|| start);
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title,
            p { class: "note", "{note}" }
            textarea {
                rows: "4",
                autofocus: true,
                "data-t": "words-text",
                value: "{text}",
                oninput: move |e| text.set(e.value()),
            }
            button {
                class: "btn main",
                "data-t": "save",
                onclick: move |_| {
                    let lines: Vec<String> = text
                        .peek()
                        .split(split_on)
                        .map(|l| l.trim().to_owned())
                        .filter(|l| !l.is_empty())
                        .collect();
                    on_save.call(lines);
                    on_close.call(());
                },
                "{t(&tr, \"common.save\")}"
            }
        }
    }
}

#[component]
fn FolderSheet(index: usize, folder: Folder, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut draft = use_signal(|| folder.clone());
    let mut labels = use_signal(|| folder.labels.join(", "));
    let current = draft();
    let chats = core.chats();
    let (save_core, del_core) = (core.clone(), core.clone());
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "folders.edit"), tag: "folder-sheet",
            label { class: "field",
                span { class: "gt", "{t(&tr, \"folders.name\")}" }
                input {
                    r#type: "text",
                    maxlength: "24",
                    "data-t": "folder-name",
                    value: "{current.name}",
                    oninput: move |e| draft.write().name = e.value(),
                }
            }
            h3 { "{t(&tr, \"folders.when\")}" }
            div { class: "sheet-list",
                SwitchRow {
                    title: t(&tr, "folders.unread"),
                    small: true,
                    checked: current.unread,
                    on_change: move |on| draft.write().unread = on,
                }
                SwitchRow {
                    title: t(&tr, "folders.groups"),
                    small: true,
                    checked: current.groups,
                    on_change: move |on| draft.write().groups = on,
                }
                SwitchRow {
                    title: t(&tr, "folders.people"),
                    small: true,
                    checked: current.people,
                    on_change: move |on| draft.write().people = on,
                }
            }
            label { class: "field",
                span { class: "gt", "{t(&tr, \"folders.labels\")}" }
                input {
                    r#type: "text",
                    placeholder: "{t(&tr, \"folders.labels_placeholder\")}",
                    value: "{labels}",
                    oninput: move |e| labels.set(e.value()),
                }
            }
            h3 { "{t(&tr, \"folders.also\")}" }
            div { class: "sheet-list",
                for c in chats {
                    {
                        let id = c.contact.id.clone();
                        let on = current.chats.contains(&id);
                        rsx! {
                            button {
                                key: "{id}",
                                class: "sheet-item",
                                role: "checkbox",
                                aria_checked: "{on}",
                                onclick: move |_| {
                                    let mut d = draft.write();
                                    if on {
                                        d.chats.retain(|x| *x != id);
                                    } else {
                                        d.chats.push(id.clone());
                                    }
                                },
                                Avatar { id: c.contact.id.clone(), name: c.contact.display_name(), size: "tiny", image: c.contact.avatar.clone() }
                                span { class: "stack one-line", "{c.contact.display_name()}" }
                                if on {
                                    span { class: "accent-text", style: "display:flex",
                                        Icon { name: IconName::Check, size: 20 }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "btns",
                if index < save_core.folders().len() {
                    button {
                        class: "btn",
                        onclick: move |_| {
                            let mut all = del_core.folders();
                            if index < all.len() {
                                all.remove(index);
                            }
                            del_core.save_folders(&all);
                            on_close.call(());
                        },
                        "{t(&tr, \"folders.delete\")}"
                    }
                }
                button {
                    class: "btn main",
                    "data-t": "save-folder",
                    disabled: current.name.trim().is_empty(),
                    onclick: move |_| {
                        let mut done = draft.peek().clone();
                        done.name = done.name.trim().to_owned();
                        done.labels = labels
                            .peek()
                            .split(',')
                            .map(|l| l.trim().to_owned())
                            .filter(|l| !l.is_empty())
                            .collect();
                        let mut all = save_core.folders();
                        if index < all.len() {
                            all[index] = done;
                        } else {
                            all.push(done);
                        }
                        save_core.save_folders(&all);
                        on_close.call(());
                    },
                    "{t(&tr, \"common.save\")}"
                }
            }
        }
    }
}
