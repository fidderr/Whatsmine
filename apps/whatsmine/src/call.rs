//! Voice and video calls in the app, 1:1 and group.
//!
//! The core does signalling and moves opaque media packets over the network.
//! The page (see `call.js`) captures, encodes, decodes and plays them. This
//! module is the bridge between the two, plus the call screen.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use dioxus::document::Eval;
use dioxus::prelude::*;
use serde_json::{Value, json};
use whatsmine_core::calls::{CallEndReason, CallEvent, MediaKind};
use whatsmine_core::{Core, now_ms};

use crate::app::{ToastSig, Translations, t, toast, use_core, use_tr};
use crate::components::{Avatar, Icon, IconName, ToastLevel};

const JS: &str = include_str!("call.js");

#[derive(Clone, PartialEq, Debug)]
pub enum Phase {
    Idle,
    Outgoing {
        contact_id: String,
        video: bool,
    },
    Incoming {
        contact_id: String,
        from: String,
        video: bool,
    },
    Active {
        contact_id: String,
        since: i64,
        my_cam: bool,
        /// Everyone else who is in the call.
        peers: Vec<String>,
        /// Those of them whose camera is on.
        cams: Vec<String>,
        muted: bool,
        group: bool,
    },
}

#[derive(Clone, Copy)]
pub struct CallUi(pub Signal<Phase>);

/// The channel to the page's media pipeline.
#[derive(Clone)]
pub struct CallBridge(Eval);

impl CallBridge {
    pub fn send(&self, msg: Value) {
        let _ = self.0.send(msg);
    }
}

/// Loads the page side of calls and starts moving packets in both
/// directions. Call once, from the root component.
pub fn use_call_bridge(
    core: &Core,
    mut phase: Signal<Phase>,
    mut toast_sig: ToastSig,
    tr: Translations,
) -> CallBridge {
    let bridge = use_hook(|| {
        CallBridge(document::eval(&format!(
            "{JS}\nawait window.wmCall.attach(dioxus);"
        )))
    });

    // Packets from the page: out to the network.
    let (up_core, up_bridge) = (core.clone(), bridge.clone());
    use_future(move || {
        let (core, bridge) = (up_core.clone(), up_bridge.clone());
        let mut eval = bridge.0;
        async move {
            while let Ok(m) = eval.recv::<Value>().await {
                let kind = m.get("t").and_then(Value::as_str).unwrap_or_default();
                let ts = m.get("ts").and_then(Value::as_u64).unwrap_or(0) as u32;
                let bytes = || {
                    m.get("d")
                        .and_then(Value::as_str)
                        .and_then(|s| B64.decode(s).ok())
                        .unwrap_or_default()
                };
                match kind {
                    "a" => core.call_audio(ts, bytes()),
                    "v" => {
                        let key = m.get("key").and_then(Value::as_bool).unwrap_or(false);
                        core.call_video(ts, key, bytes());
                    }
                    "cam" => {
                        let on = m.get("on").and_then(Value::as_bool).unwrap_or(false);
                        if let Phase::Active { my_cam, .. } = &mut *phase.write() {
                            *my_cam = on;
                        }
                        core.call_video_on(on);
                    }
                    "error" => {
                        let what = m.get("what").and_then(Value::as_str).unwrap_or_default();
                        log::warn!(
                            "call media: {what}: {}",
                            m.get("msg").and_then(Value::as_str).unwrap_or("")
                        );
                        let key = match what {
                            "microphone" => "call.err.mic",
                            "camera" => "call.err.cam",
                            _ => "call.err.media",
                        };
                        toast(&mut toast_sig, t(&tr, key), ToastLevel::Error);
                        if what == "microphone" {
                            core.call_end();
                        }
                    }
                    _ => {}
                }
            }
        }
    });

    // Packets from the network: into the page.
    let (down_core, down_bridge) = (core.clone(), bridge.clone());
    use_future(move || {
        let (core, bridge) = (down_core.clone(), down_bridge.clone());
        async move {
            let Some(mut media) = core.take_call_media() else {
                return;
            };
            while let Some(m) = media.recv().await {
                let t = match m.kind {
                    MediaKind::Audio => "a",
                    MediaKind::Video => "v",
                };
                bridge.send(json!({
                    "t": t, "from": m.from, "ts": m.ts, "key": m.key, "d": B64.encode(&m.data)
                }));
            }
        }
    });

    bridge
}

fn name_of(core: &Core, contact_id: &str) -> String {
    core.contact(contact_id)
        .map(|c| c.display_name())
        .unwrap_or_default()
}

/// Reacts to a call event from the core.
pub fn on_event(
    ev: CallEvent,
    core: &Core,
    mut phase: Signal<Phase>,
    bridge: &CallBridge,
    toast_sig: &mut ToastSig,
    tr: &Translations,
) {
    match ev {
        CallEvent::Incoming {
            contact_id,
            from,
            video,
            ..
        } => {
            bridge.send(json!({ "t": "ring", "kind": "in" }));
            phase.set(Phase::Incoming {
                contact_id,
                from,
                video,
            });
        }
        CallEvent::Connected {
            call_id,
            contact_id,
        } => {
            let video = match &*phase.read() {
                Phase::Outgoing { video, .. } | Phase::Incoming { video, .. } => *video,
                _ => false,
            };
            let group = contact_id.starts_with("g:");
            bridge.send(json!({ "t": "ring", "kind": Value::Null }));
            bridge.send(json!({ "t": "start", "call": call_id, "video": video, "group": group }));
            phase.set(Phase::Active {
                contact_id,
                since: now_ms(),
                my_cam: false,
                peers: Vec::new(),
                cams: Vec::new(),
                muted: false,
                group,
            });
        }
        CallEvent::Participants { peers: now, .. } => {
            let named: Vec<Value> = now
                .iter()
                .map(|p| json!({ "id": p, "name": name_of(core, p) }))
                .collect();
            if let Phase::Active { peers, cams, .. } = &mut *phase.write() {
                cams.retain(|c| now.contains(c));
                *peers = now;
            }
            bridge.send(json!({ "t": "peers", "peers": named }));
        }
        CallEvent::PeerVideo { from, on, .. } => {
            if let Phase::Active { cams, .. } = &mut *phase.write() {
                cams.retain(|c| *c != from);
                if on {
                    cams.push(from.clone());
                }
            }
            bridge.send(json!({ "t": "peerVideo", "from": from, "on": on }));
        }
        CallEvent::GroupCall { .. } => {}
        CallEvent::Ended {
            contact_id, reason, ..
        } => {
            let was_incoming = matches!(&*phase.read(), Phase::Incoming { .. });
            bridge.send(json!({ "t": "stop" }));
            phase.set(Phase::Idle);
            let name = name_of(core, &contact_id);
            let key = match reason {
                CallEndReason::Missed if was_incoming => Some("call.missed"),
                CallEndReason::Missed => Some("call.no_answer"),
                CallEndReason::Declined => Some("call.declined"),
                CallEndReason::Busy => Some("call.busy"),
                CallEndReason::Lost => Some("call.lost"),
                CallEndReason::Hangup => Some("call.ended"),
                CallEndReason::Cancelled => None,
            };
            if let Some(key) = key {
                toast(
                    toast_sig,
                    t(tr, key).replace("{name}", &name),
                    ToastLevel::Info,
                );
            }
        }
    }
}

/// Rings a contact, or everyone online in a group.
pub fn start_call(
    core: &Core,
    contact_id: &str,
    video: bool,
    mut phase: Signal<Phase>,
    bridge: &CallBridge,
    toast_sig: &mut ToastSig,
    tr: &Translations,
) {
    if *phase.read() != Phase::Idle {
        return;
    }
    match core.call_invite(contact_id, video) {
        Ok(_) => {
            bridge.send(json!({ "t": "ring", "kind": "out" }));
            phase.set(Phase::Outgoing {
                contact_id: contact_id.to_string(),
                video,
            });
        }
        Err(e) => {
            log::info!("call not started: {e:#}");
            toast(toast_sig, t(tr, "call.unavailable"), ToastLevel::Info);
        }
    }
}

fn clock(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The full-screen call UI. Mounted once at the root; renders nothing while idle.
#[component]
pub fn CallOverlay() -> Element {
    let core = use_core();
    let tr = use_tr();
    let CallUi(mut phase) = use_context::<CallUi>();
    let bridge = use_context::<CallBridge>();
    let mut now = use_signal(now_ms);

    // Tick the call timer while connected.
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if matches!(&*phase.read(), Phase::Active { .. }) {
                now.set(now_ms());
            }
        }
    });

    let current = phase.read().clone();
    let (contact_id, status, video_ui) = match &current {
        Phase::Idle => return rsx! {},
        Phase::Outgoing { contact_id, .. } => (contact_id.clone(), t(&tr, "call.calling"), false),
        Phase::Incoming {
            contact_id,
            from,
            video,
        } => {
            let mut s = t(
                &tr,
                if *video {
                    "call.incoming_video"
                } else {
                    "call.incoming_voice"
                },
            );
            if contact_id != from {
                s = format!("{s} · {}", name_of(&core, from));
            }
            (contact_id.clone(), s, false)
        }
        Phase::Active {
            contact_id,
            since,
            my_cam,
            cams,
            peers,
            group,
            ..
        } => {
            let mut s = clock(now() - since);
            if *group {
                s = format!("{s} · {}", peers.len() + 1);
            }
            (contact_id.clone(), s, *group || *my_cam || !cams.is_empty())
        }
    };
    let name = name_of(&core, &contact_id);
    let image = core.contact(&contact_id).and_then(|c| c.avatar);
    let is_group = contact_id.starts_with("g:");
    let hide_avatar =
        matches!(&current, Phase::Active { cams, group, .. } if *group || !cams.is_empty());
    let class = if video_ui {
        "call-overlay video-live"
    } else {
        "call-overlay"
    };

    rsx! {
        div { class: "{class}",
            div { class: "call-top",
                if !hide_avatar {
                    div { class: "call-avatar",
                        Avatar {
                            id: contact_id.clone(),
                            name: name.clone(),
                            size: "large",
                            image,
                            group: is_group,
                        }
                    }
                }
                div { class: "call-name", "{name}" }
                div { class: "call-status", "{status}" }
            }
            div { class: "call-controls",
                match current.clone() {
                    Phase::Incoming { .. } => rsx! {
                        button {
                            class: "call-btn decline",
                            aria_label: "{t(&tr, \"call.decline\")}",
                            onclick: {
                                let (core, bridge) = (core.clone(), bridge.clone());
                                move |_| {
                                    bridge.send(json!({ "t": "ring", "kind": Value::Null }));
                                    core.call_answer(false);
                                }
                            },
                            Icon { name: IconName::PhoneOff, size: 28 }
                        }
                        button {
                            class: "call-btn accept",
                            aria_label: "{t(&tr, \"call.accept\")}",
                            onclick: {
                                let (core, bridge) = (core.clone(), bridge.clone());
                                move |_| {
                                    bridge.send(json!({ "t": "ring", "kind": Value::Null }));
                                    core.call_answer(true);
                                }
                            },
                            Icon { name: IconName::Phone, size: 28 }
                        }
                    },
                    Phase::Outgoing { .. } => rsx! {
                        button {
                            class: "call-btn decline",
                            aria_label: "{t(&tr, \"call.hangup\")}",
                            onclick: {
                                let (core, bridge) = (core.clone(), bridge.clone());
                                move |_| {
                                    bridge.send(json!({ "t": "ring", "kind": Value::Null }));
                                    core.call_end();
                                }
                            },
                            Icon { name: IconName::PhoneOff, size: 28 }
                        }
                    },
                    Phase::Active { my_cam, muted, .. } => {
                        let mute_label = t(&tr, if muted { "call.unmute" } else { "call.mute" });
                        let cam_label = t(&tr, if my_cam { "call.camera_off" } else { "call.camera_on" });
                        rsx! {
                            button {
                                class: if muted { "call-btn toggle on" } else { "call-btn toggle" },
                                aria_label: "{mute_label}",
                                onclick: {
                                    let bridge = bridge.clone();
                                    move |_| {
                                        let on = !muted;
                                        if let Phase::Active { muted, .. } = &mut *phase.write() {
                                            *muted = on;
                                        }
                                        bridge.send(json!({ "t": "mute", "on": on }));
                                    }
                                },
                                Icon { name: if muted { IconName::MicOff } else { IconName::Mic }, size: 26 }
                            }
                            button {
                                class: if my_cam { "call-btn toggle" } else { "call-btn toggle on" },
                                aria_label: "{cam_label}",
                                onclick: {
                                    let bridge = bridge.clone();
                                    move |_| bridge.send(json!({ "t": "cam", "on": !my_cam }))
                                },
                                Icon { name: if my_cam { IconName::Video } else { IconName::VideoOff }, size: 26 }
                            }
                            button {
                                class: "call-btn decline",
                                aria_label: "{t(&tr, \"call.hangup\")}",
                                onclick: {
                                    let core = core.clone();
                                    move |_| core.call_end()
                                },
                                Icon { name: IconName::PhoneOff, size: 28 }
                            }
                        }
                    }
                    Phase::Idle => rsx! {},
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clock;

    #[test]
    fn clock_formats() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(59_999), "0:59");
        assert_eq!(clock(61_000), "1:01");
        assert_eq!(clock(3_725_000), "1:02:05");
        assert_eq!(clock(-5), "0:00");
    }
}
