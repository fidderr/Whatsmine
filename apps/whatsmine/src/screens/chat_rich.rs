//! Polls, lists, plans, places and contact cards: how they look in a chat, how they are made, and
//! the sheet that says where a message is.

use chrono::{Local, TimeZone};
use dioxus::prelude::*;
use whatsmine_core::model::now_ms;
use whatsmine_core::rich::{ChecklistItem, PlanAnswer, Rich, RichChange};
use whatsmine_core::{ContactKind, ContactState, MessageStatus};

use crate::app::{MsgRev, Route, ToastSig, t, toast, use_core, use_tr};
use crate::components::{Icon, IconName, Sheet, SheetItem, SwitchRow, ToastLevel};
use crate::fmt;
use crate::links::{self, InviteSig};
use crate::media;

#[derive(Clone, Copy, PartialEq)]
pub enum ComposeKind {
    Poll,
    List,
    Plan,
    Place,
    Contact,
}

#[component]
pub fn RichView(message_id: String, rich: Rich) -> Element {
    match rich {
        Rich::Look(look) => {
            let core = use_core();
            let m = core.message(&message_id);
            let (outgoing, chat) = m
                .map(|m| (m.outgoing, m.contact_id))
                .unwrap_or((true, String::new()));
            rsx! {
                crate::screens::looks::SharedLookCard { message_id, look: *look, outgoing, chat }
            }
        }
        Rich::Poll {
            question,
            options,
            votes,
            multiple,
        } => rsx! { PollView { message_id, question, options, votes, multiple } },
        Rich::Checklist { title, items } => rsx! { ListView { message_id, title, items } },
        Rich::Plan {
            title,
            at,
            place,
            answers,
        } => rsx! { PlanView { message_id, title, at, place, answers } },
        Rich::Place {
            lat,
            lon,
            label,
            live_until,
        } => rsx! { PlaceView { lat, lon, label, live_until } },
        Rich::ContactCard { name, invite } => rsx! { CardView { name, invite } },
        Rich::Note(text) => rsx! { div { class: "chat-note", "{text}" } },
    }
}

#[component]
fn PollView(
    message_id: String,
    question: String,
    options: Vec<String>,
    votes: Vec<(String, u32)>,
    multiple: bool,
) -> Element {
    let core = use_core();
    let tr = use_tr();
    let me = core.my_key();
    let total = votes.len().max(1);
    rsx! {
        div { class: "rich", "data-t": "poll",
            span { class: "rich-title", "{question}" }
            if multiple {
                span { class: "sub small", "{t(&tr, \"rich.poll_many\")}" }
            }
            for (i, option) in options.into_iter().enumerate() {
                {
                    let number = i as u32;
                    let count = votes.iter().filter(|(_, o)| *o == number).count();
                    let mine = votes.iter().any(|(who, o)| *who == me && *o == number);
                    let percent = count * 100 / total;
                    let (core, id) = (core.clone(), message_id.clone());
                    rsx! {
                        button {
                            key: "{i}",
                            class: if mine { "poll-option on" } else { "poll-option" },
                            "data-t": "poll-option",
                            aria_pressed: "{mine}",
                            onclick: move |_| core.change_rich(&id, RichChange::Vote { option: number, on: !mine }),
                            span { class: "poll-fill", style: "width:{percent}%" }
                            span { "{option}" }
                            span { "{count}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ListView(message_id: String, title: String, items: Vec<ChecklistItem>) -> Element {
    let core = use_core();
    rsx! {
        div { class: "rich", "data-t": "checklist",
            span { class: "rich-title", "{title}" }
            for (i, item) in items.into_iter().enumerate() {
                {
                    let (core, id) = (core.clone(), message_id.clone());
                    let done = item.done;
                    rsx! {
                        button {
                            key: "{i}",
                            class: if done { "check-item done" } else { "check-item" },
                            role: "checkbox",
                            aria_checked: "{done}",
                            "data-t": "check-item",
                            onclick: move |_| core.change_rich(&id, RichChange::Check { item: i as u32, done: !done }),
                            span { class: "check-box",
                                if done {
                                    Icon { name: IconName::Check, size: 16 }
                                }
                            }
                            span { "{item.text}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlanView(
    message_id: String,
    title: String,
    at: i64,
    place: String,
    answers: Vec<(String, PlanAnswer)>,
) -> Element {
    let core = use_core();
    let tr = use_tr();
    let me = core.my_key();
    let local = Local.timestamp_millis_opt(at).single();
    let weekday = local
        .map(|d| d.format("%a").to_string().to_uppercase())
        .unwrap_or_default();
    let day = local
        .map(|d| d.format("%d").to_string())
        .unwrap_or_default();
    let going = answers
        .iter()
        .filter(|(_, a)| *a == PlanAnswer::Going)
        .count();
    let going_text = t(&tr, "rich.going_count").replace("{n}", &going.to_string());
    let line = if place.is_empty() {
        format!("{}. {going_text}", fmt::clock(at))
    } else {
        format!("{}, {place}. {going_text}", fmt::clock(at))
    };
    let mine = answers.iter().find(|(who, _)| *who == me).map(|(_, a)| *a);
    let choices = [
        (PlanAnswer::Going, "rich.going"),
        (PlanAnswer::Maybe, "rich.maybe"),
        (PlanAnswer::No, "rich.cant"),
    ];
    rsx! {
        div { class: "rich", "data-t": "plan",
            div { class: "plan-head",
                span { class: "plan-date",
                    "{weekday}"
                    b { "{day}" }
                }
                span { class: "stack",
                    span { class: "rich-title", "{title}" }
                    span { class: "sub", "{line}" }
                }
            }
            div { class: "chips",
                for (answer, key) in choices {
                    {
                        let (core, id) = (core.clone(), message_id.clone());
                        rsx! {
                            button {
                                key: "{key}",
                                class: if mine == Some(answer) { "chip on" } else { "chip" },
                                "data-t": "plan-answer",
                                aria_pressed: "{mine == Some(answer)}",
                                onclick: move |_| core.change_rich(&id, RichChange::Answer(answer)),
                                "{t(&tr, key)}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PlaceView(lat: f64, lon: f64, label: String, live_until: Option<i64>) -> Element {
    let tr = use_tr();
    let mut toast_sig = use_context::<ToastSig>();
    let url = if lat == 0.0 && lon == 0.0 {
        let query: String = label
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_string()
                } else {
                    "+".to_owned()
                }
            })
            .collect();
        format!("https://www.openstreetmap.org/search?query={query}")
    } else if cfg!(target_os = "android") {
        format!("geo:{lat},{lon}?q={lat},{lon}")
    } else {
        format!("https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=16/{lat}/{lon}")
    };
    let note = match live_until {
        Some(until) if until > now_ms() => t(&tr, "rich.place_live"),
        _ => t(&tr, "rich.place_note"),
    };
    rsx! {
        button {
            class: "place-card rich",
            style: "flex-direction:row",
            "data-t": "place",
            onclick: move |_| {
                if !media::open_url(&url) {
                    toast(&mut toast_sig, t(&tr, "file.open_failed"), ToastLevel::Warning);
                }
            },
            span { class: "place-pin", Icon { name: IconName::Place, size: 24 } }
            span { class: "stack",
                span { class: "rich-title", "{label}" }
                span { class: "sub", "{note}" }
            }
        }
    }
}

#[component]
fn CardView(name: String, invite: String) -> Element {
    let tr = use_tr();
    let offer = use_context::<InviteSig>();
    rsx! {
        div { class: "rich", "data-t": "contact-card",
            div { class: "plan-head",
                span { class: "place-pin", Icon { name: IconName::Card, size: 24 } }
                span { class: "stack",
                    span { class: "rich-title", "{name}" }
                    span { class: "sub", "{t(&tr, \"rich.card_note\")}" }
                }
            }
            button {
                class: "btn small-btn",
                "data-t": "card-add",
                onclick: move |_| {
                    links::offer(offer, &invite);
                },
                "{t(&tr, \"rich.card_add\")}"
            }
        }
    }
}

// ---- making them ----

#[component]
pub fn ComposeSheet(kind: ComposeKind, chat: String, on_close: EventHandler<()>) -> Element {
    match kind {
        ComposeKind::Poll => rsx! { PollSheet { chat, on_close } },
        ComposeKind::List => rsx! { ListSheet { chat, on_close } },
        ComposeKind::Plan => rsx! { PlanSheet { chat, on_close } },
        ComposeKind::Place => rsx! { PlaceSheet { chat, on_close } },
        ComposeKind::Contact => rsx! { CardSheet { chat, on_close } },
    }
}

/// Fills in lines one by one: there is always an empty one more.
#[component]
fn Lines(placeholder: String, lines: Signal<Vec<String>>) -> Element {
    let shown = lines();
    rsx! {
        for (i, line) in shown.iter().cloned().enumerate() {
            input {
                key: "{i}",
                r#type: "text",
                maxlength: "200",
                "data-t": "line",
                placeholder: "{placeholder}",
                value: "{line}",
                oninput: move |e| {
                    let mut all = lines.write();
                    all[i] = e.value();
                    if all.last().is_some_and(|l| !l.trim().is_empty()) && all.len() < 20 {
                        all.push(String::new());
                    }
                },
            }
        }
    }
}

fn filled(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}

#[component]
fn PollSheet(chat: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let mut question = use_signal(String::new);
    let lines = use_signal(|| vec![String::new(), String::new()]);
    let mut multiple = use_signal(|| false);
    let ready = !question().trim().is_empty() && filled(&lines()).len() >= 2;
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "rich.poll"), tag: "poll-sheet",
            input {
                r#type: "text",
                maxlength: "200",
                autofocus: true,
                "data-t": "question",
                placeholder: "{t(&tr, \"rich.poll_question\")}",
                value: "{question}",
                oninput: move |e| question.set(e.value()),
            }
            Lines { placeholder: t(&tr, "rich.poll_option"), lines }
            SwitchRow {
                title: t(&tr, "rich.poll_allow_many"),
                small: true,
                checked: multiple(),
                on_change: move |on| multiple.set(on),
            }
            button {
                class: "btn main",
                "data-t": "send-rich",
                disabled: !ready,
                onclick: move |_| {
                    let rich = Rich::Poll {
                        question: question.peek().trim().to_owned(),
                        options: filled(&lines.peek()),
                        votes: Vec::new(),
                        multiple: *multiple.peek(),
                    };
                    if let Err(e) = core.send_rich(&chat, rich) {
                        toast(&mut toast_sig, t(&tr, &format!("chat.err.{e}")), ToastLevel::Error);
                    }
                    on_close.call(());
                },
                "{t(&tr, \"chat.send\")}"
            }
        }
    }
}

#[component]
fn ListSheet(chat: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let mut title = use_signal(String::new);
    let lines = use_signal(|| vec![String::new()]);
    let ready = !title().trim().is_empty() && !filled(&lines()).is_empty();
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "rich.list"), tag: "list-sheet",
            input {
                r#type: "text",
                maxlength: "200",
                autofocus: true,
                "data-t": "question",
                placeholder: "{t(&tr, \"rich.list_title\")}",
                value: "{title}",
                oninput: move |e| title.set(e.value()),
            }
            Lines { placeholder: t(&tr, "rich.list_item"), lines }
            button {
                class: "btn main",
                "data-t": "send-rich",
                disabled: !ready,
                onclick: move |_| {
                    let rich = Rich::Checklist {
                        title: title.peek().trim().to_owned(),
                        items: filled(&lines.peek())
                            .into_iter()
                            .map(|text| ChecklistItem { text, done: false })
                            .collect(),
                    };
                    if let Err(e) = core.send_rich(&chat, rich) {
                        toast(&mut toast_sig, t(&tr, &format!("chat.err.{e}")), ToastLevel::Error);
                    }
                    on_close.call(());
                },
                "{t(&tr, \"chat.send\")}"
            }
        }
    }
}

/// Turns what a date and time field holds into a moment.
pub fn parse_local(text: &str) -> Option<i64> {
    let naive = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M").ok()?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|d| d.timestamp_millis())
}

/// What a date and time field starts with: this long from now.
fn field_time(from_now_ms: i64) -> String {
    Local
        .timestamp_millis_opt(now_ms() + from_now_ms)
        .single()
        .map(|d| d.format("%Y-%m-%dT%H:%M").to_string())
        .unwrap_or_default()
}

#[component]
fn PlanSheet(chat: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let mut title = use_signal(String::new);
    let mut place = use_signal(String::new);
    let mut when = use_signal(|| field_time(86_400_000));
    let moment = parse_local(&when());
    let ready = !title().trim().is_empty() && moment.is_some();
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "rich.plan"), tag: "plan-sheet",
            input {
                r#type: "text",
                maxlength: "200",
                autofocus: true,
                "data-t": "question",
                placeholder: "{t(&tr, \"rich.plan_title\")}",
                value: "{title}",
                oninput: move |e| title.set(e.value()),
            }
            input {
                r#type: "datetime-local",
                value: "{when}",
                oninput: move |e| when.set(e.value()),
            }
            input {
                r#type: "text",
                maxlength: "200",
                placeholder: "{t(&tr, \"rich.plan_place\")}",
                value: "{place}",
                oninput: move |e| place.set(e.value()),
            }
            button {
                class: "btn main",
                "data-t": "send-rich",
                disabled: !ready,
                onclick: move |_| {
                    let Some(at) = parse_local(&when.peek()) else { return };
                    let rich = Rich::Plan {
                        title: title.peek().trim().to_owned(),
                        at,
                        place: place.peek().trim().to_owned(),
                        answers: Vec::new(),
                    };
                    if let Err(e) = core.send_rich(&chat, rich) {
                        toast(&mut toast_sig, t(&tr, &format!("chat.err.{e}")), ToastLevel::Error);
                    }
                    on_close.call(());
                },
                "{t(&tr, \"chat.send\")}"
            }
        }
    }
}

/// Where you are right now, if this device can say.
async fn my_location() -> Option<(f64, f64)> {
    #[cfg(target_os = "android")]
    {
        tokio::task::spawn_blocking(|| {
            if !crate::android::request_blocking("android.permission.ACCESS_FINE_LOCATION") {
                return None;
            }
            mobile_sentinel::location::current()
                .ok()
                .map(|c| (c.latitude, c.longitude))
        })
        .await
        .ok()
        .flatten()
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

#[component]
fn PlaceSheet(chat: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let mut label = use_signal(String::new);
    let mut spot = use_signal(|| None::<(f64, f64)>);
    let mut looking = use_signal(|| false);
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "rich.place"), tag: "place-sheet",
            input {
                r#type: "text",
                maxlength: "200",
                "data-t": "question",
                placeholder: "{t(&tr, \"rich.place_label\")}",
                value: "{label}",
                oninput: move |e| label.set(e.value()),
            }
            p { class: "note", "{t(&tr, \"rich.place_private\")}" }
            if cfg!(target_os = "android") {
                button {
                    class: "btn",
                    disabled: looking(),
                    onclick: move |_| {
                        looking.set(true);
                        spawn(async move {
                            let found = my_location().await;
                            looking.set(false);
                            if found.is_none() {
                                toast(&mut toast_sig, t(&tr, "rich.place_failed"), ToastLevel::Warning);
                            }
                            spot.set(found);
                        });
                    },
                    Icon { name: IconName::Place, size: 20 }
                    if spot().is_some() {
                        "{t(&tr, \"rich.place_found\")}"
                    } else {
                        "{t(&tr, \"rich.place_mine\")}"
                    }
                }
            }
            button {
                class: "btn main",
                "data-t": "send-rich",
                disabled: label().trim().is_empty() && spot().is_none(),
                onclick: move |_| {
                    let (lat, lon) = spot.peek().unwrap_or((0.0, 0.0));
                    let typed = label.peek().trim().to_owned();
                    let rich = Rich::Place {
                        lat,
                        lon,
                        label: if typed.is_empty() { t(&tr, "rich.place_here") } else { typed },
                        live_until: None,
                    };
                    if let Err(e) = core.send_rich(&chat, rich) {
                        toast(&mut toast_sig, t(&tr, &format!("chat.err.{e}")), ToastLevel::Error);
                    }
                    on_close.call(());
                },
                "{t(&tr, \"chat.send\")}"
            }
        }
    }
}

#[component]
fn CardSheet(chat: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let people: Vec<_> = core
        .chats()
        .into_iter()
        .map(|c| c.contact)
        .filter(|c| {
            c.kind == ContactKind::Person
                && c.state == ContactState::Accepted
                && c.id != chat
                && core.contact_card(&c.id).is_some()
        })
        .collect();
    let my_name = core.settings().display_name;
    let (mine_core, mine_chat) = (core.clone(), chat.clone());
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "rich.contact"), tag: "card-sheet",
            div { class: "sheet-list",
                SheetItem {
                    label: t(&tr, "rich.card_mine"),
                    icon: IconName::Card,
                    tag: "card-mine",
                    on_tap: move |_| {
                        let rich = Rich::ContactCard {
                            name: my_name.clone(),
                            invite: mine_core.invite_link(),
                        };
                        if let Err(e) = mine_core.send_rich(&mine_chat, rich) {
                            toast(&mut toast_sig, t(&tr, &format!("chat.err.{e}")), ToastLevel::Error);
                        }
                        on_close.call(());
                    },
                }
                for c in people {
                    {
                        let (core, chat, name, id) = (core.clone(), chat.clone(), c.their_name.clone(), c.id.clone());
                        rsx! {
                            SheetItem {
                                key: "{c.id}",
                                label: c.display_name(),
                                icon: IconName::UserPlus,
                                on_tap: move |_| {
                                    if let Some(invite) = core.contact_card(&id) {
                                        let rich = Rich::ContactCard { name: name.clone(), invite };
                                        if let Err(e) = core.send_rich(&chat, rich) {
                                            toast(&mut toast_sig, t(&tr, &format!("chat.err.{e}")), ToastLevel::Error);
                                        }
                                    }
                                    on_close.call(());
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---- when and where ----

#[component]
pub fn SendLaterSheet(on_close: EventHandler<()>, on_pick: EventHandler<i64>) -> Element {
    let tr = use_tr();
    let mut when = use_signal(|| field_time(3_600_000));
    let moment = parse_local(&when()).filter(|at| *at > now_ms());
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "chat.send_later"), tag: "later-sheet",
            p { class: "note", "{t(&tr, \"chat.send_later_note\")}" }
            input {
                r#type: "datetime-local",
                "data-t": "later-time",
                value: "{when}",
                oninput: move |e| when.set(e.value()),
            }
            button {
                class: "btn main",
                "data-t": "later-yes",
                disabled: moment.is_none(),
                onclick: move |_| {
                    if let Some(at) = moment {
                        on_pick.call(at);
                    }
                },
                "{t(&tr, \"chat.send_later\")}"
            }
        }
    }
}

/// Where a message is, why it waits, and when the next try is.
#[component]
pub fn DeliverySheet(message_id: String, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let msg_rev = use_context::<MsgRev>().0;
    let mut tick = use_signal(|| 0u32);
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            *tick.write() += 1;
        }
    });
    let _ = (msg_rev(), tick());
    let Some(message) = core.message(&message_id) else {
        return rsx! {};
    };
    let info = core.delivery_info(&message_id);
    let name = core
        .contact(&message.contact_id)
        .map(|c| c.display_name())
        .unwrap_or_default();
    let status = message.status;
    let is_group = whatsmine_core::groups::is_group_id(&message.contact_id);
    let reached = t(&tr, "delivery.reached").replace("{name}", &name);
    let opened = t(&tr, "delivery.read").replace("{name}", &name);
    // What is known about where it is, one step per line.
    let mut steps: Vec<(IconName, String, String, bool)> = vec![(
        IconName::Check,
        t(&tr, "delivery.saved"),
        fmt::when(&tr, message.sent_at),
        true,
    )];
    match status {
        MessageStatus::Scheduled => steps.push((
            IconName::Clock,
            t(&tr, "delivery.scheduled"),
            fmt::when(&tr, message.sent_at),
            false,
        )),
        MessageStatus::NotDelivered => steps.push((
            IconName::Warning,
            t(&tr, "delivery.stopped"),
            t(&tr, "delivery.stopped_note"),
            false,
        )),
        MessageStatus::Queued | MessageStatus::Sent => {
            if let Some(i) = &info {
                let note = match i.next_try_in_ms {
                    Some(ms) if ms > 0 => {
                        t(&tr, "delivery.next_try").replace("{time}", &fmt::wait_time(&tr, ms))
                    }
                    _ if i.connected => t(&tr, "delivery.on_its_way"),
                    _ => t(&tr, "delivery.looking"),
                };
                steps.push((IconName::Clock, t(&tr, "delivery.trying"), note, false));
                if i.knocked {
                    steps.push((
                        IconName::Bell,
                        t(&tr, "delivery.knocked"),
                        t(&tr, "delivery.knocked_note"),
                        true,
                    ));
                }
            }
        }
        MessageStatus::Delivered => {
            steps.push((IconName::CheckDouble, reached, String::new(), true));
        }
        MessageStatus::Read => {
            steps.push((IconName::CheckDouble, reached, String::new(), true));
            steps.push((IconName::CheckDouble, opened, String::new(), true));
        }
    }
    let acks_line = if is_group {
        let (got, of) = core.group_acks(&message_id);
        t(&tr, "delivery.group")
            .replace("{got}", &got.to_string())
            .replace("{of}", &of.to_string())
    } else {
        String::new()
    };
    let (retry_core, delete_core) = (core.clone(), core.clone());
    let (retry_id, delete_id) = (message_id.clone(), message_id.clone());
    let can_retry = matches!(
        status,
        MessageStatus::NotDelivered | MessageStatus::Queued | MessageStatus::Sent
    );
    let legend = [
        (IconName::Clock, "delivery.legend_clock"),
        (IconName::Check, "delivery.legend_one"),
        (IconName::CheckDouble, "delivery.legend_two"),
        (IconName::Warning, "delivery.legend_failed"),
    ];
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "delivery.title"), tag: "delivery-sheet",
            if !message.body.is_empty() {
                div { class: "plain-box one-line", "{message.body}" }
            }
            for (i, (icon, title, note, done)) in steps.into_iter().enumerate() {
                div { key: "{i}", class: "mark", style: "align-items:flex-start;gap:12px",
                    span { class: if done { "ok-text" } else { "warn-text" }, style: "display:flex",
                        Icon { name: icon, size: 20 }
                    }
                    span { class: "stack",
                        span { class: "row-t", "{title}" }
                        if !note.is_empty() {
                            span { class: "sub", "{note}" }
                        }
                    }
                }
            }
            if !acks_line.is_empty() {
                p { class: "sub", "{acks_line}" }
            }
            if can_retry {
                div { class: "btns",
                    button {
                        class: "btn",
                        "data-t": "try-now",
                        onclick: move |_| {
                            retry_core.retry_message(&retry_id);
                            retry_core.wake();
                        },
                        "{t(&tr, \"delivery.try_now\")}"
                    }
                    button {
                        class: "btn",
                        "data-t": "delete-it",
                        onclick: move |_| {
                            delete_core.delete_message(&delete_id);
                            on_close.call(());
                        },
                        "{t(&tr, \"delivery.delete\")}"
                    }
                }
            }
            h3 { "{t(&tr, \"delivery.legend\")}" }
            for (icon, key) in legend {
                span { key: "{key}", class: "mark sub",
                    Icon { name: icon, size: 16 }
                    "{t(&tr, key)}"
                }
            }
            button {
                class: "row sm",
                style: "padding:0",
                "data-t": "check-connection",
                onclick: move |_| {
                    on_close.call(());
                    nav.push(Route::ConnectionScreen {});
                },
                span { class: "stack row-t", "{t(&tr, \"delivery.check\")}" }
                Icon { name: IconName::ChevronRight, size: 16 }
            }
        }
    }
}
