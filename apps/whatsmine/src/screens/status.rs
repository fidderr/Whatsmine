//! Posts that last a day: the list, a new one, who sees them, and looking at them.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use dioxus::prelude::*;
use whatsmine_core::status::{Audience, Backdrop, Post};
use whatsmine_core::{ContactKind, ContactState, now_ms};

use crate::app::{Rev, Route, ToastSig, Translations, t, toast, use_core, use_tr};
use crate::components::{
    Avatar, BottomNav, Chips, Header, Icon, IconName, Tab, Title, ToastLevel, change, use_settings,
};
use crate::fmt;
use crate::media;

/// How long a post stays on screen before the next.
const SHOWN_FOR_MS: u32 = 6_000;
const TICK_MS: u32 = 200;
const REACTIONS: [&str; 5] = ["👍", "❤️", "😂", "😮", "😢"];

// What is being written stays while someone goes to choose who sees it, or anywhere else.
static DRAFT_TEXT: GlobalSignal<String> = Signal::global(String::new);
static DRAFT_BACKDROP: GlobalSignal<Backdrop> = Signal::global(Backdrop::default);
static DRAFT_PHOTO: GlobalSignal<Option<(PathBuf, String)>> = Signal::global(|| None);

fn audience_line(tr: &Translations, audience: &Audience) -> String {
    match audience {
        Audience::All => t(tr, "status.sees_all"),
        Audience::Only(ids) => t(tr, "status.sees_only").replace("{n}", &ids.len().to_string()),
        Audience::Except(ids) => t(tr, "status.sees_except").replace("{n}", &ids.len().to_string()),
    }
}

/// Everyone's posts, a line per person.
struct Person {
    id: String,
    name: String,
    image: Option<String>,
    newest: i64,
}

#[component]
pub fn StatusScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let settings = use_settings();
    let _ = rev();
    let me = core.my_key();
    let posts = core.status_posts();
    let mine: Vec<&Post> = posts.iter().filter(|p| p.author == me).collect();
    let mut fresh: Vec<Person> = Vec::new();
    let mut seen: Vec<Person> = Vec::new();
    let mut done = BTreeSet::new();
    for post in posts.iter().filter(|p| p.author != me) {
        if !done.insert(post.author.clone()) {
            continue;
        }
        let of_them = || posts.iter().filter(|p| p.author == post.author);
        let contact = core.contact(&post.author);
        let person = Person {
            id: post.author.clone(),
            name: contact
                .as_ref()
                .map(|c| c.display_name())
                .unwrap_or_default(),
            image: contact.and_then(|c| c.avatar),
            newest: post.at,
        };
        if of_them().all(|p| p.seen) {
            seen.push(person);
        } else {
            fresh.push(person);
        }
    }
    let mine_line = if mine.is_empty() {
        format!(
            "{} {}",
            t(&tr, "status.mine_note"),
            audience_line(&tr, &settings().status_audience)
        )
    } else {
        let looked = mine
            .iter()
            .flat_map(|p| core.status_seen_by(&p.id))
            .collect::<BTreeSet<_>>()
            .len();
        t(&tr, "status.mine_count")
            .replace("{n}", &mine.len().to_string())
            .replace("{seen}", &looked.to_string())
    };
    let my_name = settings().display_name;
    let have_mine = !mine.is_empty();
    let my_id = me.clone();
    rsx! {
        div { class: "screen",
            Title { title: t(&tr, "status.title"),
                Link {
                    to: Route::StatusWho {},
                    class: "ib r",
                    "data-t": "status-who",
                    aria_label: "{t(&tr, \"status.who\")}",
                    Icon { name: IconName::Lock, size: 20 }
                }
            }
            div { class: "body",
                div { class: "group",
                    div { class: "row status-mine",
                        if have_mine {
                            Link {
                                to: Route::StatusView { id: my_id.clone() },
                                class: "status-mine-open",
                                "data-t": "status-mine",
                                Avatar { id: my_id.clone(), name: my_name.clone(), size: "medium" }
                                div { class: "stack",
                                    span { class: "row-t", "{t(&tr, \"status.mine\")}" }
                                    span { class: "sub", "{mine_line}" }
                                }
                            }
                        } else {
                            Link {
                                to: Route::StatusNew {},
                                class: "status-mine-open",
                                "data-t": "status-mine",
                                Avatar { id: my_id.clone(), name: my_name.clone(), size: "medium" }
                                div { class: "stack",
                                    span { class: "row-t", "{t(&tr, \"status.mine\")}" }
                                    span { class: "sub", "{mine_line}" }
                                }
                            }
                        }
                        Link {
                            to: Route::StatusNew {},
                            class: "ib",
                            "data-t": "status-new",
                            aria_label: "{t(&tr, \"status.new\")}",
                            Icon { name: IconName::Plus, size: 20 }
                        }
                    }
                }
                if !fresh.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"status.fresh\")}" }
                    div { class: "group",
                        for person in fresh {
                            PersonRow { key: "{person.id}", id: person.id.clone(), name: person.name.clone(), image: person.image.clone(), at: person.newest, new: true }
                        }
                    }
                }
                if !seen.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"status.seen\")}" }
                    div { class: "group",
                        for person in seen {
                            PersonRow { key: "{person.id}", id: person.id.clone(), name: person.name.clone(), image: person.image.clone(), at: person.newest, new: false }
                        }
                    }
                }
                p { class: "note", style: "padding:0 4px", "{t(&tr, \"status.limit\")}" }
            }
            BottomNav { active: Tab::People }
        }
    }
}

#[component]
fn PersonRow(id: String, name: String, image: Option<String>, at: i64, new: bool) -> Element {
    let tr = use_tr();
    let when = fmt::list_time(&tr, at);
    rsx! {
        Link {
            to: Route::StatusView { id: id.clone() },
            class: "row",
            "data-t": "status-person",
            span { class: if new { "status-ring new" } else { "status-ring" },
                Avatar { id: id.clone(), name: name.clone(), size: "medium", image }
            }
            div { class: "stack",
                span { class: "row-t", "{name}" }
                span { class: "sub", "{when}" }
            }
        }
    }
}

#[component]
pub fn StatusNew() -> Element {
    let tr = use_tr();
    let core = use_core();
    let settings = use_settings();
    let nav = use_navigator();
    let mut toast_sig = use_context::<ToastSig>();
    let mut sending = use_signal(|| false);
    let preview_core = core.clone();
    let can_post = !sending() && (!DRAFT_TEXT().trim().is_empty() || DRAFT_PHOTO().is_some());
    let area_class = format!("status-new-area st-{}", DRAFT_BACKDROP().word());
    let photo_url = DRAFT_PHOTO().map(|(_, url)| url);
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "status.new") }
            div { class: "body",
                div { class: "{area_class}",
                    if let Some(url) = &photo_url {
                        img { class: "status-new-photo", src: "{url}", alt: "" }
                    }
                    textarea {
                        "data-t": "status-text",
                        aria_label: "{t(&tr, \"status.placeholder\")}",
                        placeholder: "{t(&tr, \"status.placeholder\")}",
                        maxlength: "700",
                        value: "{DRAFT_TEXT}",
                        oninput: move |e| *DRAFT_TEXT.write() = e.value(),
                    }
                }
                h2 { class: "gt", "{t(&tr, \"status.backdrop\")}" }
                div { class: "status-backdrops",
                    for choice in Backdrop::ALL {
                        button {
                            key: "{choice.word()}",
                            class: format!("status-backdrop st-{}{}", choice.word(), if choice == DRAFT_BACKDROP() { " on" } else { "" }),
                            "data-t": "status-backdrop",
                            aria_label: t(&tr, &format!("status.bg_{}", choice.word())),
                            aria_pressed: "{choice == DRAFT_BACKDROP()}",
                            onclick: move |_| *DRAFT_BACKDROP.write() = choice,
                        }
                    }
                    button {
                        class: "status-backdrop photo",
                        "data-t": "status-photo",
                        aria_label: "{t(&tr, \"status.photo\")}",
                        onclick: move |_| {
                            let core = preview_core.clone();
                            spawn(async move {
                                let Some(files) = media::pick(media::IMAGES, false).await else {
                                    toast(&mut toast_sig, t(&tr, "attach.unavailable"), ToastLevel::Warning);
                                    return;
                                };
                                let Some(file) = files.into_iter().next() else {
                                    return;
                                };
                                let path = file.path.clone();
                                let looked = tokio::task::spawn_blocking(move || core.status_photo_preview(&file.path)).await;
                                match looked {
                                    Ok(Ok(jpeg)) => {
                                        let url = format!(
                                            "data:image/jpeg;base64,{}",
                                            base64::engine::general_purpose::STANDARD.encode(jpeg)
                                        );
                                        *DRAFT_PHOTO.write() = Some((path, url));
                                    }
                                    _ => toast(&mut toast_sig, t(&tr, "status.photo_failed"), ToastLevel::Error),
                                }
                            });
                        },
                        Icon { name: IconName::Image, size: 20 }
                    }
                }
                div { class: "group",
                    crate::components::Row {
                        icon: IconName::Users,
                        title: t(&tr, "status.who_row"),
                        value: audience_line(&tr, &settings().status_audience),
                        small: true,
                        to: Route::StatusWho {},
                        tag: "status-who-row",
                    }
                }
                button {
                    class: "btn main",
                    "data-t": "status-post",
                    disabled: !can_post,
                    onclick: move |_| {
                        sending.set(true);
                        let core = core.clone();
                        let words = DRAFT_TEXT();
                        let choice = DRAFT_BACKDROP();
                        let path = DRAFT_PHOTO().map(|(path, _)| path);
                        spawn(async move {
                            let posted = tokio::task::spawn_blocking(move || {
                                core.status_post(&words, choice, path.as_deref())
                            })
                            .await;
                            sending.set(false);
                            if matches!(posted, Ok(Ok(_))) {
                                *DRAFT_TEXT.write() = String::new();
                                *DRAFT_PHOTO.write() = None;
                                nav.go_back();
                            } else {
                                toast(&mut toast_sig, t(&tr, "status.post_failed"), ToastLevel::Error);
                            }
                        });
                    },
                    "{t(&tr, \"status.post\")}"
                }
            }
        }
    }
}

#[component]
pub fn StatusWho() -> Element {
    let tr = use_tr();
    let core = use_core();
    let settings = use_settings();
    let audience = settings().status_audience;
    let (kind, chosen): (&str, Vec<String>) = match &audience {
        Audience::All => ("all", Vec::new()),
        Audience::Only(ids) => ("only", ids.clone()),
        Audience::Except(ids) => ("except", ids.clone()),
    };
    let options = vec![
        ("all".to_owned(), t(&tr, "status.who_all")),
        ("only".to_owned(), t(&tr, "status.who_only")),
        ("except".to_owned(), t(&tr, "status.who_except")),
    ];
    let keep = chosen.clone();
    let people: Vec<(String, String)> = core
        .address_book()
        .into_iter()
        .filter(|c| c.state == ContactState::Accepted && c.kind == ContactKind::Person)
        .map(|c| (c.id.clone(), c.display_name()))
        .collect();
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "status.who") }
            div { class: "body",
                Chips {
                    options,
                    value: kind.to_owned(),
                    on_change: move |v: String| {
                        let ids = keep.clone();
                        change(settings, |s| {
                            s.status_audience = match v.as_str() {
                                "only" => Audience::Only(ids),
                                "except" => Audience::Except(ids),
                                _ => Audience::All,
                            };
                        });
                    },
                }
                if kind == "all" {
                    p { class: "note", style: "padding:0 4px", "{t(&tr, \"status.who_all_note\")}" }
                } else {
                    div { class: "group",
                        for (id, name) in people {
                            {
                                let on = chosen.contains(&id);
                                let toggle_id = id.clone();
                                let row_id = id.clone();
                                rsx! {
                                    label { key: "{row_id}", class: "row sm status-pick", "data-t": "status-pick",
                                        Avatar { id: id.clone(), name: name.clone(), size: "small" }
                                        span { class: "row-t", "{name}" }
                                        input {
                                            r#type: "checkbox",
                                            checked: on,
                                            onchange: move |e| {
                                                let turn_on = e.checked();
                                                let id = toggle_id.clone();
                                                change(settings, |s| {
                                                    let list = match &mut s.status_audience {
                                                        Audience::Only(ids) | Audience::Except(ids) => ids,
                                                        Audience::All => return,
                                                    };
                                                    list.retain(|i| *i != id);
                                                    if turn_on {
                                                        list.push(id);
                                                    }
                                                });
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Looking at someone's posts, oldest first, starting where we left off.
#[component]
pub fn StatusView(id: String) -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let nav = use_navigator();
    let mut toast_sig = use_context::<ToastSig>();
    let _ = rev();
    let mine = id == core.my_key();
    let mut posts: Vec<Post> = core
        .status_posts()
        .into_iter()
        .filter(|p| p.author == id)
        .collect();
    posts.reverse();
    let start = posts.iter().position(|p| !p.seen).unwrap_or(0);
    let mut at = use_signal(move || start);
    let mut shown_ms = use_signal(|| 0u32);
    let mut typing = use_signal(|| false);
    let mut draft = use_signal(String::new);

    // On to the next by itself, unless someone is typing.
    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_millis(u64::from(TICK_MS))).await;
            if typing() {
                continue;
            }
            let shown = *shown_ms.peek() + TICK_MS;
            if shown >= SHOWN_FOR_MS {
                shown_ms.set(0);
                *at.write() += 1;
            } else {
                shown_ms.set(shown);
            }
        }
    });

    let index = at();
    let shown_id = posts.get(index).map(|p| p.id.clone());
    let seen_core = core.clone();
    use_effect(use_reactive!(|shown_id| {
        match &shown_id {
            Some(id) => seen_core.status_seen(id),
            // Nothing (more) to look at.
            None => {
                nav.go_back();
            }
        }
    }));
    let Some(post) = posts.get(index).cloned() else {
        return rsx! {};
    };
    let contact = core.contact(&id);
    let name = if mine {
        t(&tr, "status.mine")
    } else {
        contact
            .as_ref()
            .map(|c| c.display_name())
            .unwrap_or_default()
    };
    let image = contact.and_then(|c| c.avatar);
    let when = fmt::list_time(&tr, post.at);
    let left_h = ((post.until - now_ms()).max(0) / 3_600_000).to_string();
    let left_line = t(&tr, "status.left").replace("{h}", &left_h);
    let count = posts.len();
    let progress = f64::from(shown_ms()) / f64::from(SHOWN_FOR_MS) * 100.0;
    let seen_by: Vec<String> = core
        .status_seen_by(&post.id)
        .iter()
        .map(|who| {
            core.contact(who)
                .map(|c| c.display_name())
                .unwrap_or_default()
        })
        .collect();
    let seen_line = if seen_by.is_empty() {
        t(&tr, "status.seen_nobody")
    } else {
        t(&tr, "status.seen_by").replace("{names}", &seen_by.join(", "))
    };
    let placeholder = t(&tr, "status.reply_to").replace("{name}", &name);
    let photo_url = post
        .has_photo()
        .then(|| format!("/status-photo/{}", post.id));
    let class = format!("status-view st-{}", post.backdrop.word());
    let post_id = post.id.clone();
    let delete_id = post.id.clone();
    let delete_core = core.clone();
    let send_core = core.clone();
    let send_id = post.id.clone();
    let react_id = post.id.clone();
    let react_core = core.clone();
    let mut send_reply = move |words: String| {
        if words.trim().is_empty() {
            return;
        }
        match send_core.status_reply(&send_id, &words) {
            Ok(_) => {
                draft.set(String::new());
                toast(
                    &mut toast_sig,
                    t(&tr, "status.reply_sent"),
                    ToastLevel::Info,
                );
            }
            Err(_) => toast(
                &mut toast_sig,
                t(&tr, "status.reply_failed"),
                ToastLevel::Error,
            ),
        }
    };
    let mut send_on_enter = send_reply.clone();
    rsx! {
        div { class: "{class}", "data-t": "status-view",
            if let Some(url) = &photo_url {
                img { class: "status-view-photo", src: "{url}", alt: "" }
            }
            div { class: "status-bars", role: "img",
                for n in 0..count {
                    span { key: "{n}", class: "status-bar",
                        span {
                            class: "status-bar-fill",
                            style: if n < index { "width:100%".to_owned() } else if n == index { format!("width:{progress:.0}%") } else { "width:0".to_owned() },
                        }
                    }
                }
            }
            header { class: "status-head",
                button {
                    class: "ib",
                    "data-t": "status-close",
                    aria_label: "{t(&tr, \"common.close\")}",
                    onclick: move |_| nav.go_back(),
                    Icon { name: IconName::Xmark }
                }
                Avatar { id: id.clone(), name: name.clone(), size: "small", image }
                div { class: "stack",
                    span { class: "row-t", "{name}" }
                    span { class: "small", "{when}" }
                }
                if mine {
                    button {
                        class: "ib",
                        "data-t": "status-delete",
                        aria_label: "{t(&tr, \"status.delete\")}",
                        onclick: move |_| delete_core.status_delete(&delete_id),
                        Icon { name: IconName::Trash, size: 20 }
                    }
                }
            }
            button {
                class: "status-tap back",
                aria_label: "{t(&tr, \"status.previous\")}",
                onclick: move |_| {
                    shown_ms.set(0);
                    let now = *at.peek();
                    at.set(now.saturating_sub(1));
                },
            }
            button {
                class: "status-tap on",
                "data-t": "status-next",
                aria_label: "{t(&tr, \"status.next\")}",
                onclick: move |_| {
                    shown_ms.set(0);
                    *at.write() += 1;
                },
            }
            if !post.text.is_empty() {
                div { class: if photo_url.is_some() { "status-text under" } else { "status-text" }, "data-t": "status-words", "{post.text}" }
            }
            if mine {
                div { class: "status-foot note",
                    span { "data-t": "status-seen-by", "{seen_line}" }
                    span { "{left_line}" }
                }
            } else {
                div { class: "status-foot",
                    div { class: "status-reactions",
                        for emoji in REACTIONS {
                            {
                                let react_core = react_core.clone();
                                let react_id = react_id.clone();
                                let mut toast_sig = toast_sig;
                                rsx! {
                                    button {
                                        key: "{emoji}",
                                        class: "status-reaction",
                                        "data-t": "status-react",
                                        aria_label: "{emoji}",
                                        onclick: move |_| {
                                            if react_core.status_reply(&react_id, emoji).is_ok() {
                                                toast(&mut toast_sig, t(&tr, "status.reply_sent"), ToastLevel::Info);
                                            }
                                        },
                                        "{emoji}"
                                    }
                                }
                            }
                        }
                    }
                    div { class: "composer",
                        label { class: "pill",
                            input {
                                "data-t": "status-reply",
                                aria_label: "{placeholder}",
                                placeholder: "{placeholder}",
                                value: "{draft}",
                                onfocusin: move |_| typing.set(true),
                                onfocusout: move |_| typing.set(false),
                                oninput: move |e| draft.set(e.value()),
                                onkeydown: move |e| {
                                    if e.key() == Key::Enter {
                                        send_on_enter(draft());
                                    }
                                },
                            }
                        }
                        button {
                            class: "round main",
                            "data-t": "status-send",
                            aria_label: "{t(&tr, \"status.send\")}",
                            onclick: move |_| send_reply(draft()),
                            Icon { name: IconName::Send, size: 20 }
                        }
                    }
                }
            }
            span { hidden: true, "data-id": "{post_id}" }
        }
    }
}
