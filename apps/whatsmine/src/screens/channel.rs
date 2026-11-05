//! Channels: the list, one channel, and what there is to know about it.

use dioxus::prelude::*;
use whatsmine_core::channel::{self, Channel, ChannelPost};

use crate::app::{Rev, Route, ToastSig, Translations, t, toast, use_core, use_tr};
use crate::components::{
    Avatar, BottomNav, Chips, Header, Icon, IconName, Row, Sheet, SwitchRow, Tab, Title,
    ToastLevel, change, use_settings,
};
use crate::fmt;
use crate::media;

fn followers_line(tr: &Translations, c: &Channel) -> String {
    t(tr, "channel.about_followers").replace("{n}", &channel::about(c.followers).to_string())
}

#[component]
pub fn ChannelsScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let nav = use_navigator();
    let mut toast_sig = use_context::<ToastSig>();
    let mut naming = use_signal(|| false);
    let mut name = use_signal(String::new);
    let mut pasting = use_signal(|| false);
    let mut pasted = use_signal(String::new);
    let _ = rev();
    let channels = core.channels();
    let create_core = core.clone();
    rsx! {
        div { class: "screen", "data-t": "channels",
            Title { title: t(&tr, "channel.title") }
            div { class: "body",
                div { class: "group",
                    Row {
                        icon: IconName::Plus,
                        title: t(&tr, "channel.new"),
                        small: true,
                        tag: "channel-new",
                        on_tap: move |_| naming.set(true),
                    }
                    Row {
                        icon: IconName::Link,
                        title: t(&tr, "channel.have_link"),
                        small: true,
                        tag: "channel-paste",
                        on_tap: move |_| pasting.set(true),
                    }
                }
                if channels.is_empty() {
                    p { class: "note", style: "padding:0 4px", "{t(&tr, \"channel.none\")}" }
                } else {
                    div { class: "group",
                        for c in channels {
                            {
                                let line = if c.runs_it() {
                                    t(&tr, "channel.yours")
                                } else {
                                    followers_line(&tr, &c)
                                };
                                rsx! {
                                    Link {
                                        key: "{c.id}",
                                        to: Route::ChannelView { id: c.id.clone() },
                                        class: "row",
                                        "data-t": "channel-row",
                                        Avatar { id: c.id.clone(), name: c.name.clone(), size: "medium" }
                                        div { class: "stack",
                                            span { class: "row-t", "{c.name}" }
                                            span { class: "sub", "{line}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                p { class: "note", style: "padding:0 4px", "{t(&tr, \"channel.what\")}" }
            }
            BottomNav { active: Tab::People }
            if naming() {
                Sheet {
                    on_close: move |_| naming.set(false),
                    title: t(&tr, "channel.new"),
                    tag: "channel-new-sheet",
                    label { class: "field",
                        span { class: "gt", "{t(&tr, \"channel.name\")}" }
                        input {
                            r#type: "text",
                            "data-t": "channel-name",
                            maxlength: "64",
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }
                    p { class: "note", "{t(&tr, \"channel.link_note\")}" }
                    div { class: "btns",
                        button { class: "btn", onclick: move |_| naming.set(false), "{t(&tr, \"common.cancel\")}" }
                        button {
                            class: "btn main",
                            "data-t": "channel-create",
                            disabled: name().trim().is_empty(),
                            onclick: move |_| {
                                naming.set(false);
                                match create_core.channel_create(&name()) {
                                    Ok(c) => {
                                        name.set(String::new());
                                        nav.push(Route::ChannelView { id: c.id });
                                    }
                                    Err(_) => toast(&mut toast_sig, t(&tr, "space.failed"), ToastLevel::Error),
                                }
                            },
                            "{t(&tr, \"channel.create\")}"
                        }
                    }
                }
            }
            if pasting() {
                Sheet {
                    on_close: move |_| pasting.set(false),
                    title: t(&tr, "channel.have_link"),
                    tag: "channel-paste-sheet",
                    label { class: "field",
                        span { class: "gt", "{t(&tr, \"channel.link\")}" }
                        input {
                            r#type: "text",
                            "data-t": "channel-link-in",
                            value: "{pasted}",
                            oninput: move |e| pasted.set(e.value()),
                        }
                    }
                    div { class: "btns",
                        button { class: "btn", onclick: move |_| pasting.set(false), "{t(&tr, \"common.cancel\")}" }
                        button {
                            class: "btn main",
                            "data-t": "channel-link-go",
                            disabled: channel::parse_link(&pasted()).is_none(),
                            onclick: move |_| {
                                pasting.set(false);
                                crate::links::offer_channel(&pasted());
                                pasted.set(String::new());
                            },
                            "{t(&tr, \"channel.continue\")}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PostCard(post: ChannelPost) -> Element {
    let tr = use_tr();
    let core = use_core();
    let when = fmt::list_time(&tr, post.at);
    let waiting = post.more.is_some() && !post.has_body;
    let number = post.number;
    let channel_id = post.channel.clone();
    let photo = (post.has_photo() && post.has_body)
        .then(|| format!("/channel-photo/{}/{}", post.channel, post.number));
    let counts = post.reactions.clone();
    let mine = post.mine.clone();
    rsx! {
        div { class: "channel-post", "data-t": "channel-post",
            if let Some(url) = &photo {
                img { class: "channel-photo", src: "{url}", alt: "" }
            }
            if !post.text.is_empty() {
                div { class: "channel-text", "{post.text}" }
            }
            if waiting {
                div { class: "note", "data-t": "channel-waiting", "{t(&tr, \"channel.more_coming\")}" }
            }
            div { class: "channel-meta", "{when}" }
            div { class: "channel-reactions",
                for (n, emoji) in channel::EMOJI.iter().enumerate() {
                    {
                        let count = counts.get(n).copied().unwrap_or(0);
                        let chosen = mine.contains(&(n as u8));
                        let (core, id) = (core.clone(), channel_id.clone());
                        let shown = channel::about(count);
                        rsx! {
                            button {
                                key: "{n}",
                                class: if chosen { "channel-react on" } else { "channel-react" },
                                "data-t": "channel-react",
                                disabled: chosen,
                                aria_label: "{emoji}",
                                onclick: move |_| core.channel_react(&id, number, n),
                                span { "{emoji}" }
                                if shown > 0 {
                                    span { "{shown}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn ChannelView(id: String) -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let mut draft = use_signal(String::new);
    let mut sending = use_signal(|| false);
    let _ = rev();
    let Some(c) = core.channel(&id) else {
        return rsx! {
            div { class: "screen no-nav",
                Header { title: t(&tr, "channel.title") }
                div { class: "body", p { class: "note", "{t(&tr, \"channel.gone\")}" } }
            }
        };
    };
    let posts = core.channel_posts(&id);
    let line = if c.runs_it() {
        format!("{}. {}", t(&tr, "channel.yours"), followers_line(&tr, &c))
    } else {
        followers_line(&tr, &c)
    };
    let link = c.link();
    let muted = c.muted;
    let mute_id = id.clone();
    let mute_core = core.clone();
    let write_core = core.clone();
    let photo_core = core.clone();
    let (write_id, photo_id, info_id) = (id.clone(), id.clone(), id.clone());
    let runs_it = c.runs_it();
    let can_send = !draft().trim().is_empty() && !sending();
    let empty_line = t(
        &tr,
        if runs_it {
            "channel.write_first"
        } else {
            "channel.nothing_yet"
        },
    );
    rsx! {
        div { class: "screen no-nav channel-screen", "data-t": "channel",
            Header { title: c.name.clone(),
                button {
                    class: "ib r",
                    "data-t": "channel-mute",
                    aria_label: t(&tr, if muted { "channel.unmute" } else { "channel.mute" }),
                    aria_pressed: "{muted}",
                    onclick: move |_| mute_core.channel_set_muted(&mute_id, !muted),
                    Icon { name: if muted { IconName::BellOff } else { IconName::Bell }, size: 20 }
                }
                Link {
                    to: Route::ChannelInfo { id: info_id },
                    class: "ib r",
                    "data-t": "channel-info",
                    aria_label: "{t(&tr, \"channel.info\")}",
                    Icon { name: IconName::Info, size: 20 }
                }
            }
            div { class: "body channel-body",
                p { class: "note", style: "padding:0 4px", "{line}" }
                if posts.is_empty() {
                    p { class: "note", style: "padding:0 4px", "{empty_line}" }
                }
                for post in posts {
                    PostCard { key: "{post.number}", post }
                }
            }
            if runs_it {
                div { class: "composer channel-composer",
                    button {
                        class: "round",
                        "data-t": "channel-photo-pick",
                        aria_label: "{t(&tr, \"status.photo\")}",
                        onclick: move |_| {
                            let (core, id) = (photo_core.clone(), photo_id.clone());
                            spawn(async move {
                                let Some(files) = media::pick(media::IMAGES, false).await else {
                                    toast(&mut toast_sig, t(&tr, "attach.unavailable"), ToastLevel::Warning);
                                    return;
                                };
                                let Some(file) = files.into_iter().next() else {
                                    return;
                                };
                                let words = draft();
                                let written = tokio::task::spawn_blocking(move || {
                                    core.channel_write(&id, &words, Some(&file.path))
                                })
                                .await;
                                if matches!(written, Ok(Ok(_))) {
                                    draft.set(String::new());
                                } else {
                                    toast(&mut toast_sig, t(&tr, "status.photo_failed"), ToastLevel::Error);
                                }
                            });
                        },
                        Icon { name: IconName::Image, size: 20 }
                    }
                    label { class: "pill",
                        textarea {
                            "data-t": "channel-draft",
                            rows: "1",
                            aria_label: "{t(&tr, \"channel.write\")}",
                            placeholder: "{t(&tr, \"channel.write\")}",
                            value: "{draft}",
                            oninput: move |e| draft.set(e.value()),
                        }
                    }
                    button {
                        class: "round main",
                        "data-t": "channel-send",
                        disabled: !can_send,
                        aria_label: "{t(&tr, \"status.send\")}",
                        onclick: move |_| {
                            sending.set(true);
                            if write_core.channel_write(&write_id, &draft(), None).is_ok() {
                                draft.set(String::new());
                            } else {
                                toast(&mut toast_sig, t(&tr, "space.failed"), ToastLevel::Error);
                            }
                            sending.set(false);
                        },
                        Icon { name: IconName::Send, size: 20 }
                    }
                }
            } else {
                div { class: "channel-share",
                    button {
                        class: "btn",
                        "data-t": "channel-share",
                        onclick: move |_| {
                            crate::js::copy_text(&link);
                            toast(&mut toast_sig, t(&tr, "channel.link_copied"), ToastLevel::Info);
                        },
                        Icon { name: IconName::Share, size: 20 }
                        "{t(&tr, \"channel.share\")}"
                    }
                }
            }
        }
    }
}

#[component]
pub fn ChannelInfo(id: String) -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let nav = use_navigator();
    let settings = use_settings();
    let mut toast_sig = use_context::<ToastSig>();
    let mut leaving = use_signal(|| false);
    let _ = rev();
    let Some(c) = core.channel(&id) else {
        return rsx! {};
    };
    let link = c.link();
    let keep_options: Vec<(String, String)> = [7u32, 30, 90]
        .iter()
        .map(|d| {
            (
                d.to_string(),
                t(&tr, "channel.days").replace("{n}", &d.to_string()),
            )
        })
        .collect();
    let (keep_core, keep_id) = (core.clone(), id.clone());
    let (leave_core, leave_id) = (core.clone(), id.clone());
    let s = settings();
    let runs_it = c.runs_it();
    let leave_label = t(
        &tr,
        if runs_it {
            "channel.stop_running"
        } else {
            "channel.leave"
        },
    );
    let leave_title = leave_label.clone();
    rsx! {
        div { class: "screen no-nav", "data-t": "channel-info-screen",
            Header { title: c.name.clone() }
            div { class: "body",
                p { class: "note", style: "padding:0 4px", "{followers_line(&tr, &c)}" }
                div { class: "group",
                    Row {
                        icon: IconName::Link,
                        title: t(&tr, "channel.copy_link"),
                        note: t(&tr, "channel.link_note"),
                        small: true,
                        tag: "channel-copy",
                        on_tap: move |_| {
                            crate::js::copy_text(&link);
                            toast(&mut toast_sig, t(&tr, "channel.link_copied"), ToastLevel::Info);
                        },
                    }
                }
                h2 { class: "gt", "{t(&tr, \"channel.keep\")}" }
                Chips {
                    options: keep_options,
                    value: c.keep_days.to_string(),
                    on_change: move |v: String| {
                        keep_core.channel_set_keep_days(&keep_id, v.parse().unwrap_or(30));
                    },
                }
                h2 { class: "gt", "{t(&tr, \"channel.passing_on\")}" }
                div { class: "group",
                    SwitchRow {
                        title: t(&tr, "channel.pass_on"),
                        note: t(&tr, "channel.pass_on_note"),
                        checked: s.channel_pass_on,
                        tag: "channel-pass-on",
                        on_change: move |v: bool| change(settings, |s| s.channel_pass_on = v),
                    }
                    SwitchRow {
                        title: t(&tr, "channel.relays"),
                        note: t(&tr, "channel.relays_note"),
                        checked: s.channel_relays_only,
                        tag: "channel-relays",
                        on_change: move |v: bool| change(settings, |s| s.channel_relays_only = v),
                    }
                }
                p { class: "note", style: "padding:0 4px", "{t(&tr, \"channel.honest\")}" }
                div { class: "group",
                    Row {
                        title: leave_label.clone(),
                        small: true,
                        danger: true,
                        tag: "channel-leave",
                        on_tap: move |_| leaving.set(true),
                    }
                }
            }
            if leaving() {
                crate::components::ConfirmDialog {
                    title: leave_title,
                    message: t(&tr, if runs_it { "channel.stop_note" } else { "channel.leave_note" }),
                    confirm_label: leave_label,
                    on_confirm: move |_| {
                        leaving.set(false);
                        leave_core.channel_leave(&leave_id);
                        nav.replace(Route::ChannelsScreen {});
                    },
                    on_cancel: move |_| leaving.set(false),
                }
            }
        }
    }
}
