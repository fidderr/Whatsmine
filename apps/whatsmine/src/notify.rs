//! Notifications for what happens while you are not looking.
//!
//! A small hub listens to the core's events for the whole life of the
//! process (not just while a window exists, so it keeps working when the
//! window is hidden in the tray or the Android activity is gone) and decides
//! what, if anything, to show. The decision is a pure function so it can be
//! tested; showing it is per platform: Windows toasts, an Android
//! notification through mobile-sentinel.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

use whatsmine_core::calls::{CallEndReason, CallEvent};
use whatsmine_core::{Core, CoreEvent, MediaKind, Message, Settings};

use crate::i18n;

/// Is a window of ours in front of the user right now? Then the app itself
/// shows what is happening and the system stays quiet.
static UI_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_ui_active(active: bool) {
    let was = UI_ACTIVE.swap(active, Ordering::Relaxed);
    if active && !was {
        platform::cancel_all();
    }
}

pub fn ui_active() -> bool {
    UI_ACTIVE.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Message,
    Request,
    Call,
    Missed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Notifications with the same tag replace each other.
    pub tag: String,
    pub kind: Kind,
    pub title: String,
    pub body: String,
    /// The conversation to open when it is tapped.
    pub chat: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Post(Notice),
    Cancel(String),
}

/// What the decision needs to know about the outside world.
pub trait Lookup {
    fn name(&self, id: &str) -> String;
    fn text(&self, key: &str) -> String;
}

#[derive(Default)]
pub struct Hub {
    /// Calls we were rung for and have not answered or declined yet.
    ringing: HashSet<String>,
    /// Unread lines per chat, newest last, for a notification that shows a few.
    recent: HashMap<String, Vec<String>>,
}

const MAX_LINES: usize = 5;

impl Hub {
    pub fn decide(
        &mut self,
        ev: &CoreEvent,
        settings: &Settings,
        ui_active: bool,
        look: &dyn Lookup,
    ) -> Vec<Action> {
        if !settings.notifications {
            return Vec::new();
        }
        let generic = |look: &dyn Lookup, key: &str| (look.text("app.name"), look.text(key));
        match ev {
            CoreEvent::MessageAdded(m) if !m.outgoing => {
                if ui_active {
                    return Vec::new();
                }
                let tag = format!("chat:{}", m.contact_id);
                let (title, body) = if settings.notification_previews {
                    let line = preview(m, look);
                    let lines = self.recent.entry(m.contact_id.clone()).or_default();
                    lines.push(line);
                    if lines.len() > MAX_LINES {
                        lines.remove(0);
                    }
                    (look.name(&m.contact_id), lines.join("\n"))
                } else {
                    generic(look, "notify.new_message")
                };
                vec![Action::Post(Notice {
                    tag,
                    kind: Kind::Message,
                    title,
                    body,
                    chat: Some(m.contact_id.clone()),
                })]
            }
            CoreEvent::RequestReceived { contact_id, name } => {
                if ui_active {
                    return Vec::new();
                }
                let who = if name.is_empty() {
                    look.text("requests.someone")
                } else {
                    name.clone()
                };
                let body = look.text("notify.request").replace("{name}", &who);
                vec![Action::Post(Notice {
                    tag: format!("req:{contact_id}"),
                    kind: Kind::Request,
                    title: look.text("app.name"),
                    body,
                    chat: None,
                })]
            }
            CoreEvent::Call(CallEvent::Incoming {
                call_id,
                contact_id,
                from,
                video,
            }) => {
                self.ringing.insert(call_id.clone());
                if ui_active {
                    return Vec::new();
                }
                let key = if *video {
                    "call.incoming_video"
                } else {
                    "call.incoming_voice"
                };
                let (title, body) = if settings.notification_previews {
                    let mut body = look.text(key);
                    if contact_id != from {
                        body = format!("{body} · {}", look.name(from));
                    }
                    (look.name(contact_id), body)
                } else {
                    (look.text("app.name"), look.text("notify.incoming_call"))
                };
                vec![Action::Post(Notice {
                    tag: "call".into(),
                    kind: Kind::Call,
                    title,
                    body,
                    chat: Some(contact_id.clone()),
                })]
            }
            CoreEvent::Call(CallEvent::Connected { call_id, .. }) => {
                self.ringing.remove(call_id);
                vec![Action::Cancel("call".into())]
            }
            CoreEvent::Call(CallEvent::Ended {
                call_id,
                contact_id,
                reason,
                ..
            }) => {
                let was_ringing = self.ringing.remove(call_id);
                let mut out = vec![Action::Cancel("call".into())];
                if was_ringing && *reason == CallEndReason::Missed && !ui_active {
                    let (title, body) = if settings.notification_previews {
                        let name = look.name(contact_id);
                        (
                            name.clone(),
                            look.text("call.missed").replace("{name}", &name),
                        )
                    } else {
                        generic(look, "notify.missed_call")
                    };
                    out.push(Action::Post(Notice {
                        tag: format!("missed:{call_id}"),
                        kind: Kind::Missed,
                        title,
                        body,
                        chat: Some(contact_id.clone()),
                    }));
                }
                out
            }
            _ => Vec::new(),
        }
    }

    /// The chat was opened: its unread lines are no longer news.
    pub fn read(&mut self, chat: &str) {
        self.recent.remove(chat);
    }
}

/// One line for a message: the text, or what kind of file it is. In a group,
/// who wrote it.
fn preview(m: &Message, look: &dyn Lookup) -> String {
    let what = match &m.attachment {
        None => m.body.clone(),
        Some(a) => {
            let label = match a.kind() {
                MediaKind::Image => look.text("media.photo"),
                MediaKind::Video => look.text("media.video"),
                MediaKind::Audio => look.text("media.voice"),
                MediaKind::File => a.name.clone(),
            };
            if m.body.is_empty() {
                label
            } else {
                format!("{label}: {}", m.body)
            }
        }
    };
    match &m.sender {
        Some(s) => format!("{}: {what}", look.name(s)),
        None => what,
    }
}

struct CoreLookup<'a> {
    core: &'a Core,
    strings: HashMap<String, String>,
}

impl Lookup for CoreLookup<'_> {
    fn name(&self, id: &str) -> String {
        self.core
            .contact(id)
            .map(|c| c.display_name())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.chars().take(8).collect())
    }
    fn text(&self, key: &str) -> String {
        self.strings
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }
}

static HUB: LazyLock<Mutex<Hub>> = LazyLock::new(|| Mutex::new(Hub::default()));

/// A chat was opened: forget its unread lines and remove its notification.
pub fn mark_read(chat: &str) {
    HUB.lock().unwrap_or_else(|p| p.into_inner()).read(chat);
    platform::cancel(&format!("chat:{chat}"));
}

/// Starts the hub for the life of the process.
pub fn start(core: Core) {
    std::thread::Builder::new()
        .name("whatsmine-notify".into())
        .spawn(move || {
            use tokio::sync::broadcast::error::RecvError;
            let mut rx = core.subscribe();
            loop {
                let ev = match rx.blocking_recv() {
                    Ok(ev) => ev,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                };
                let settings = core.settings();
                let mut strings = i18n::load_translations(&settings.locale);
                strings.insert("app.name".into(), "WhatsMine".into());
                let look = CoreLookup {
                    core: &core,
                    strings,
                };
                let actions = HUB.lock().unwrap_or_else(|p| p.into_inner()).decide(
                    &ev,
                    &settings,
                    ui_active(),
                    &look,
                );
                for action in actions {
                    match action {
                        Action::Post(n) => platform::post(&n),
                        Action::Cancel(tag) => platform::cancel(&tag),
                    }
                }
            }
        })
        .ok();
}

/// Everything platform-specific lives here. Test runs can also have every
/// notification written to a file (`WHATSMINE_NOTIFY_LOG`), which is how the
/// end-to-end tests see them.
mod platform {
    use super::Notice;

    fn log_line(what: &str, tag: &str, n: Option<&Notice>) {
        let Ok(path) = std::env::var("WHATSMINE_NOTIFY_LOG") else {
            return;
        };
        let line = serde_json::json!({
            "what": what,
            "tag": tag,
            "title": n.map(|n| n.title.as_str()),
            "body": n.map(|n| n.body.as_str()),
            "chat": n.and_then(|n| n.chat.as_deref()),
        });
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn post(n: &Notice) {
        log_line("post", &n.tag, Some(n));
        #[cfg(windows)]
        {
            let ok = crate::desktop::toast(n);
            log_line(
                if ok { "toast_ok" } else { "toast_failed" },
                &n.tag,
                Some(n),
            );
        }
        #[cfg(target_os = "android")]
        crate::android::post(n);
    }

    pub fn cancel(tag: &str) {
        log_line("cancel", tag, None);
        #[cfg(target_os = "android")]
        crate::android::cancel(tag);
        #[cfg(not(target_os = "android"))]
        let _ = tag;
    }

    pub fn cancel_all() {
        #[cfg(target_os = "android")]
        crate::android::cancel_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatsmine_core::{Attachment, MessageStatus, TransferState};

    struct Fake;
    impl Lookup for Fake {
        fn name(&self, id: &str) -> String {
            match id {
                "bob" => "Bobby".into(),
                "grp" => "Crew".into(),
                other => other.into(),
            }
        }
        fn text(&self, key: &str) -> String {
            match key {
                "call.missed" => "Missed call from {name}".into(),
                "notify.request" => "{name} wants to connect".into(),
                other => other.into(),
            }
        }
    }

    fn msg(chat: &str, body: &str, sender: Option<&str>) -> Message {
        Message {
            id: format!("m-{body}"),
            contact_id: chat.into(),
            outgoing: false,
            body: body.into(),
            sent_at: 1,
            received_at: 1,
            status: MessageStatus::Delivered,
            read: false,
            attachment: None,
            sender: sender.map(Into::into),
        }
    }

    fn posted(actions: Vec<Action>) -> Vec<Notice> {
        actions
            .into_iter()
            .filter_map(|a| match a {
                Action::Post(n) => Some(n),
                Action::Cancel(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_message_while_you_are_away_notifies_and_stacks_per_chat() {
        let mut hub = Hub::default();
        let s = Settings::default();
        let a = posted(hub.decide(
            &CoreEvent::MessageAdded(msg("bob", "hi", None)),
            &s,
            false,
            &Fake,
        ));
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].title.as_str(), a[0].body.as_str()), ("Bobby", "hi"));
        assert_eq!(a[0].chat.as_deref(), Some("bob"));
        let b = posted(hub.decide(
            &CoreEvent::MessageAdded(msg("bob", "you there?", None)),
            &s,
            false,
            &Fake,
        ));
        assert_eq!(
            b[0].tag, a[0].tag,
            "same chat replaces the earlier notification"
        );
        assert_eq!(b[0].body, "hi\nyou there?");
        hub.read("bob");
        let c = posted(hub.decide(
            &CoreEvent::MessageAdded(msg("bob", "again", None)),
            &s,
            false,
            &Fake,
        ));
        assert_eq!(c[0].body, "again", "reading the chat starts the stack over");
    }

    #[test]
    fn nothing_is_shown_while_the_app_is_in_front_or_when_switched_off() {
        let mut hub = Hub::default();
        let ev = CoreEvent::MessageAdded(msg("bob", "hi", None));
        assert!(
            hub.decide(&ev, &Settings::default(), true, &Fake)
                .is_empty()
        );
        let off = Settings {
            notifications: false,
            ..Settings::default()
        };
        assert!(hub.decide(&ev, &off, false, &Fake).is_empty());
        // Our own messages never notify.
        let mut mine = msg("bob", "me", None);
        mine.outgoing = true;
        assert!(
            hub.decide(
                &CoreEvent::MessageAdded(mine),
                &Settings::default(),
                false,
                &Fake
            )
            .is_empty()
        );
    }

    #[test]
    fn previews_can_be_hidden_and_groups_name_the_writer() {
        let mut hub = Hub::default();
        let hidden = Settings {
            notification_previews: false,
            ..Settings::default()
        };
        let n = posted(hub.decide(
            &CoreEvent::MessageAdded(msg("bob", "secret", None)),
            &hidden,
            false,
            &Fake,
        ));
        assert_eq!(
            (n[0].title.as_str(), n[0].body.as_str()),
            ("WhatsMine", "notify.new_message")
        );
        let g = posted(hub.decide(
            &CoreEvent::MessageAdded(msg("grp", "plan?", Some("bob"))),
            &Settings::default(),
            false,
            &Fake,
        ));
        assert_eq!(
            (g[0].title.as_str(), g[0].body.as_str()),
            ("Crew", "Bobby: plan?")
        );
    }

    #[test]
    fn files_show_what_they_are() {
        let mut hub = Hub::default();
        let mut m = msg("bob", "", None);
        m.attachment = Some(Attachment {
            name: "report.pdf".into(),
            mime: "application/pdf".into(),
            size: 1,
            sha256: String::new(),
            path: None,
            state: TransferState::Transferring,
            duration_ms: None,
        });
        let n = posted(hub.decide(
            &CoreEvent::MessageAdded(m),
            &Settings::default(),
            false,
            &Fake,
        ));
        assert_eq!(n[0].body, "report.pdf");
    }

    #[test]
    fn a_ringing_call_becomes_a_missed_call_unless_answered() {
        let mut hub = Hub::default();
        let s = Settings::default();
        let incoming = CoreEvent::Call(CallEvent::Incoming {
            call_id: "c1".into(),
            contact_id: "bob".into(),
            from: "bob".into(),
            video: false,
        });
        let n = posted(hub.decide(&incoming, &s, false, &Fake));
        assert_eq!(
            (n[0].kind, n[0].tag.as_str(), n[0].title.as_str()),
            (Kind::Call, "call", "Bobby")
        );
        let ended = |reason| {
            CoreEvent::Call(CallEvent::Ended {
                call_id: "c1".into(),
                contact_id: "bob".into(),
                reason,
                duration_ms: 0,
            })
        };
        let out = hub.decide(&ended(CallEndReason::Missed), &s, false, &Fake);
        assert!(out.contains(&Action::Cancel("call".into())));
        let missed = posted(out);
        assert_eq!(missed[0].kind, Kind::Missed);
        assert_eq!(missed[0].body, "Missed call from Bobby");

        // Answered: no missed-call notice later.
        hub.decide(&incoming, &s, false, &Fake);
        hub.decide(
            &CoreEvent::Call(CallEvent::Connected {
                call_id: "c1".into(),
                contact_id: "bob".into(),
            }),
            &s,
            false,
            &Fake,
        );
        assert!(posted(hub.decide(&ended(CallEndReason::Hangup), &s, false, &Fake)).is_empty());
        // A call we placed that nobody answered is not a missed call.
        assert!(posted(hub.decide(&ended(CallEndReason::Missed), &s, false, &Fake)).is_empty());
    }

    #[test]
    fn contact_requests_notify() {
        let mut hub = Hub::default();
        let ev = CoreEvent::RequestReceived {
            contact_id: "k".into(),
            name: "Zed".into(),
        };
        let n = posted(hub.decide(&ev, &Settings::default(), false, &Fake));
        assert_eq!(n[0].body, "Zed wants to connect");
    }
}
