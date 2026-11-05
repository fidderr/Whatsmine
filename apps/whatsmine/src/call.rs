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
use whatsmine_core::calls::{CallEndReason, CallEvent, MediaKind, ScreenCodec};
use whatsmine_core::{Core, now_ms};

use crate::app::{ToastSig, Translations, t, toast, use_core, use_tr};
use crate::back::BackLayer;
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
        /// We are showing our screen.
        sharing: bool,
        /// Who is showing theirs.
        sharer: Option<String>,
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
                    "s" => {
                        let key = m.get("key").and_then(Value::as_bool).unwrap_or(false);
                        let codec = m.get("c").and_then(Value::as_u64).unwrap_or(0) as u8;
                        core.call_screen_frame(ts, key, ScreenCodec::from_bits(codec), bytes());
                    }
                    "screen" => {
                        let on = m.get("on").and_then(Value::as_bool).unwrap_or(false);
                        let size = |name| m.get(name).and_then(Value::as_u64).unwrap_or(0) as u32;
                        if let Phase::Active { sharing, .. } = &mut *phase.write() {
                            *sharing = on;
                        }
                        crate::guard::set(crate::guard::Reason::Sharing, on);
                        core.call_screen_on(on, size("w"), size("h"));
                    }
                    "askKey" => core.call_screen_key(),
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
                            "screen" | "screen-encoder" => "call.err_screen",
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
            let mut media = core.call_media();
            while let Some(m) = media.recv().await {
                let t = match m.kind {
                    MediaKind::Audio => "a",
                    MediaKind::Video => "v",
                    MediaKind::Screen => {
                        let codec = match m.codec {
                            ScreenCodec::Vp8 => 0,
                            ScreenCodec::Vp9 => 1,
                            ScreenCodec::H264 => 2,
                        };
                        bridge.send(json!({
                            "t": "s", "from": m.from, "n": m.number, "ts": m.ts, "key": m.key,
                            "c": codec, "d": B64.encode(&m.data)
                        }));
                        continue;
                    }
                };
                bridge.send(json!({
                    "t": t, "from": m.from, "ts": m.ts, "key": m.key, "d": B64.encode(&m.data)
                }));
            }
        }
    });

    bridge
}

/// On Android: the call goes on while the app is not in front. Only possible while it is.
fn keep_going(name: &str, tr: &Translations, video: bool) {
    #[cfg(target_os = "android")]
    {
        use mobile_sentinel::permissions::{PermissionState, status};
        let camera = video || status("android.permission.CAMERA") == PermissionState::Granted;
        mobile_sentinel::call_service::start(name, &t(tr, "call.in_call"), camera);
        mobile_sentinel::call_service::set_speaker(video);
    }
    #[cfg(not(target_os = "android"))]
    let _ = (name, tr, video);
}

fn call_over() {
    #[cfg(target_os = "android")]
    {
        mobile_sentinel::call_service::stop();
        mobile_sentinel::call_service::end_audio();
    }
}

/// On Android the frames of a shown screen come from the system, straight to the core. What the
/// page does on a PC.
#[cfg(target_os = "android")]
fn show_screen(core: &Core, phase: Signal<Phase>, tr: &Translations) {
    use mobile_sentinel::call_service::{ScreenCodec as Sdk, ScreenFrame, show_screen};
    let (for_state, for_frames) = (core.clone(), core.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<bool>();
    let shown = show_screen(
        &t(tr, "call.share_stop"),
        move |state: Option<(u32, u32)>| {
            let (w, h) = state.unwrap_or((0, 0));
            crate::guard::set(crate::guard::Reason::Sharing, state.is_some());
            for_state.call_screen_on(state.is_some(), w, h);
            let _ = tx.send(state.is_some());
        },
        move |frame: ScreenFrame| {
            let codec = match frame.codec {
                Sdk::H264 => ScreenCodec::H264,
                Sdk::Vp8 => ScreenCodec::Vp8,
            };
            for_frames.call_screen_frame(frame.time_us as u32, frame.key, codec, frame.data);
        },
    );
    if !shown {
        return;
    }
    let mut phase = phase;
    spawn(async move {
        while let Some(on) = rx.recv().await {
            if let Phase::Active { sharing, .. } = &mut *phase.write() {
                *sharing = on;
            }
            if !on {
                break;
            }
        }
    });
}

/// A screen that opens in the middle of a call (the app was in the
/// background, or a notification brought it up) picks the call up where it is.
pub fn restore(core: &Core, mut phase: Signal<Phase>, bridge: &CallBridge) {
    use whatsmine_core::calls::CallPhase;
    let Some(c) = core.call_snapshot() else {
        return;
    };
    match c.phase {
        CallPhase::RingingIn => {
            bridge.send(json!({ "t": "ring", "kind": "in" }));
            phase.set(Phase::Incoming {
                contact_id: c.contact_id,
                from: c.from,
                video: c.video,
            });
        }
        CallPhase::RingingOut => {
            bridge.send(json!({ "t": "ring", "kind": "out" }));
            phase.set(Phase::Outgoing {
                contact_id: c.contact_id,
                video: c.video,
            });
        }
        CallPhase::Active => {
            let group = c.contact_id.starts_with("g:");
            let named: Vec<Value> = c
                .peers
                .iter()
                .map(|p| json!({ "id": p, "name": name_of(core, p) }))
                .collect();
            bridge
                .send(json!({ "t": "start", "call": c.call_id, "video": c.video, "group": group }));
            bridge.send(json!({ "t": "peers", "peers": named }));
            if let Some(sharer) = &c.sharer {
                bridge.send(json!({ "t": "peerScreen", "from": sharer, "on": true }));
            }
            phase.set(Phase::Active {
                contact_id: c.contact_id,
                since: c.connected_since.unwrap_or_else(now_ms),
                my_cam: false,
                peers: c.peers,
                cams: Vec::new(),
                muted: false,
                group,
                sharing: false,
                sharer: c.sharer,
            });
        }
    }
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
                sharing: false,
                sharer: None,
            });
        }
        CallEvent::PeerScreen { from, on, .. } => {
            if let Phase::Active { sharer, .. } = &mut *phase.write() {
                if on {
                    *sharer = Some(from.clone());
                } else if sharer.as_deref() == Some(from.as_str()) {
                    *sharer = None;
                }
            }
            bridge.send(json!({ "t": "peerScreen", "from": from, "on": on }));
        }
        CallEvent::ScreenKeyWanted { .. } => {
            #[cfg(target_os = "android")]
            mobile_sentinel::call_service::want_whole_picture();
            bridge.send(json!({ "t": "screenKey" }));
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
            crate::guard::set(crate::guard::Reason::Sharing, false);
            call_over();
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

/// On Android the microphone must be allowed before a call begins (the
/// system asks once); elsewhere there is nothing to ask.
pub async fn ensure_permissions(video: bool) -> bool {
    #[cfg(target_os = "android")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::android::ensure_call_permissions(video));
        });
        let granted = rx.await.unwrap_or(false);
        // The system dialog is still closing: the answer comes back before the
        // app is in front again, and screen updates sent to a WebView that is
        // not back yet can be lost. Let it come back first.
        for _ in 0..30 {
            if crate::notify::ui_active() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        granted
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = video;
        true
    }
}

/// Joins the call going on in a group.
pub fn join_group_call(core: &Core, group_id: &str, toast_sig: &ToastSig, tr: &Translations) {
    let (core, id, mut toast_sig, tr) = (core.clone(), group_id.to_string(), *toast_sig, *tr);
    spawn(async move {
        if ensure_permissions(false).await {
            core.call_join(&id);
        } else {
            toast(&mut toast_sig, t(&tr, "call.err.mic"), ToastLevel::Error);
        }
    });
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
    let (core, id, bridge, mut toast_sig, tr) = (
        core.clone(),
        contact_id.to_string(),
        bridge.clone(),
        *toast_sig,
        *tr,
    );
    spawn(async move {
        if !ensure_permissions(video).await {
            toast(&mut toast_sig, t(&tr, "call.err.mic"), ToastLevel::Error);
            return;
        }
        match core.call_invite(&id, video) {
            Ok(_) => {
                keep_going(&name_of(&core, &id), &tr, video);
                bridge.send(json!({ "t": "ring", "kind": "out" }));
                phase.set(Phase::Outgoing {
                    contact_id: id,
                    video,
                });
            }
            Err(e) => {
                log::info!("call not started: {e:#}");
                toast(&mut toast_sig, t(&tr, "call.unavailable"), ToastLevel::Info);
            }
        }
    });
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
    let toast_sig = use_context::<ToastSig>();
    let mut now = use_signal(now_ms);
    let mut asking_share = use_signal(|| false);
    let mut speaker = use_signal(|| false);
    let mut path_turn = use_signal(|| 0u32);
    let path_report = {
        let core = core.clone();
        use_resource(move || {
            let core = core.clone();
            let _ = path_turn();
            async move { core.connection_report().await }
        })
    };
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if matches!(&*phase.peek(), Phase::Active { .. }) {
                *path_turn.write() += 1;
            }
        }
    });

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
            sharer,
            ..
        } => {
            let mut s = clock(now() - since);
            if *group {
                s = format!("{s} · {}", peers.len() + 1);
            }
            (
                contact_id.clone(),
                s,
                *group || *my_cam || !cams.is_empty() || sharer.is_some(),
            )
        }
    };
    let name = name_of(&core, &contact_id);
    let image = core.contact(&contact_id).and_then(|c| c.avatar);
    let is_group = contact_id.starts_with("g:");
    // How the connection runs, in words, while there is one.
    let path_note = match (&current, path_report.read().clone().flatten()) {
        (Phase::Active { .. }, Some(report)) => report
            .peers
            .iter()
            .find(|p| p.contact_id == contact_id)
            .map(|p| {
                t(
                    &tr,
                    if p.direct {
                        "call.path_direct"
                    } else {
                        "call.path_relay"
                    },
                )
            })
            .unwrap_or_default(),
        _ => String::new(),
    };
    let hide_avatar = matches!(
        &current,
        Phase::Active { cams, group, sharer, .. }
            if *group || !cams.is_empty() || sharer.is_some()
    );
    let (sharing_now, someone_sharing) = match &current {
        Phase::Active {
            sharing, sharer, ..
        } => (*sharing, sharer.clone()),
        _ => (false, None),
    };
    let watching_line = someone_sharing
        .as_deref()
        .map(|who| t(&tr, "call.peer_sharing").replace("{name}", &name_of(&core, who)));
    let share_title = t(&tr, "call.share_title").replace("{name}", &name);
    let class = if video_ui {
        "call-overlay video-live"
    } else {
        "call-overlay"
    };

    rsx! {
        div { class: "{class}",
            // Back does not leave the call screen; hanging up does.
            BackLayer { on_back: move |_| {} }
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
                if !path_note.is_empty() {
                    div { class: "call-path", "{path_note}" }
                }
                if let Some(line) = &watching_line {
                    div { class: "call-path", "data-t": "call-watching", "{line}" }
                }
                if sharing_now {
                    div { class: "call-sharing", "data-t": "call-sharing",
                        Icon { name: IconName::Monitor, size: 20 }
                        span { "{t(&tr, \"call.sharing\")}" }
                    }
                }
            }
            div { class: "call-controls",
                match current.clone() {
                    Phase::Incoming { .. } => rsx! {
                        button {
                            class: "call-btn decline",
                            "data-t": "call-decline",
                            aria_label: "{t(&tr, \"call.decline\")}",
                            "data-label": "{t(&tr, \"call.decline\")}",
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
                            "data-t": "call-accept",
                            aria_label: "{t(&tr, \"call.accept\")}",
                            "data-label": "{t(&tr, \"call.accept\")}",
                            onclick: {
                                let (core, bridge, video) = (core.clone(), bridge.clone(), matches!(current, Phase::Incoming { video: true, .. }));
                                let mut toast_sig = toast_sig;
                                let answer_name = name.clone();
                                move |_| {
                                    bridge.send(json!({ "t": "ring", "kind": Value::Null }));
                                    let core = core.clone();
                                    let answer_name = answer_name.clone();
                                    spawn(async move {
                                        if ensure_permissions(video).await {
                                            keep_going(&answer_name, &tr, video);
                                            core.call_answer(true);
                                        } else {
                                            core.call_answer(false);
                                            toast(&mut toast_sig, t(&tr, "call.err.mic"), ToastLevel::Error);
                                        }
                                    });
                                }
                            },
                            Icon { name: IconName::Phone, size: 28 }
                        }
                    },
                    Phase::Outgoing { .. } => rsx! {
                        button {
                            class: "call-btn decline",
                            aria_label: "{t(&tr, \"call.hangup\")}",
                            "data-label": "{t(&tr, \"call.hangup\")}",
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
                        let share_label = t(&tr, if sharing_now { "call.share_stop" } else { "call.share" });
                        let someone_name = someone_sharing.as_deref().map(|who| name_of(&core, who));
                        rsx! {
                            button {
                                class: if muted { "call-btn toggle on" } else { "call-btn toggle" },
                                aria_label: "{mute_label}",
                                "data-label": "{mute_label}",
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
                            if cfg!(target_os = "android") {
                                button {
                                    class: if speaker() { "call-btn toggle" } else { "call-btn toggle on" },
                                    "data-t": "call-speaker",
                                    aria_label: "{t(&tr, \"call.speaker\")}",
                                    "data-label": "{t(&tr, \"call.speaker\")}",
                                    aria_pressed: "{speaker()}",
                                    onclick: move |_| {
                                        let on = !speaker();
                                        speaker.set(on);
                                        #[cfg(target_os = "android")]
                                        mobile_sentinel::call_service::set_speaker(on);
                                    },
                                    Icon { name: IconName::Volume, size: 26 }
                                }
                            }
                            button {
                                class: if my_cam { "call-btn toggle" } else { "call-btn toggle on" },
                                aria_label: "{cam_label}",
                                "data-label": "{t(&tr, \"call.l_camera\")}",
                                onclick: {
                                    let bridge = bridge.clone();
                                    move |_| bridge.send(json!({ "t": "cam", "on": !my_cam }))
                                },
                                Icon { name: if my_cam { IconName::Video } else { IconName::VideoOff }, size: 26 }
                            }
                            button {
                                class: if sharing_now { "call-btn toggle" } else { "call-btn toggle on" },
                                "data-t": "call-share",
                                aria_label: "{share_label}",
                                "data-label": "{t(&tr, \"call.l_screen\")}",
                                onclick: {
                                    let bridge = bridge.clone();
                                    let mut toast_sig = toast_sig;
                                    let name = someone_name.clone();
                                    move |_| {
                                        if sharing_now {
                                            #[cfg(target_os = "android")]
                                            mobile_sentinel::call_service::stop_screen();
                                            bridge.send(json!({ "t": "share", "on": false }));
                                        } else if let Some(name) = &name {
                                            toast(
                                                &mut toast_sig,
                                                t(&tr, "call.share_busy").replace("{name}", name),
                                                ToastLevel::Info,
                                            );
                                        } else {
                                            asking_share.set(true);
                                        }
                                    }
                                },
                                Icon { name: IconName::Monitor, size: 26 }
                            }
                            button {
                                class: "call-btn decline",
                                "data-t": "call-hangup",
                                aria_label: "{t(&tr, \"call.hangup\")}",
                            "data-label": "{t(&tr, \"call.hangup\")}",
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
            if asking_share() {
                crate::components::Sheet {
                    on_close: move |_| asking_share.set(false),
                    title: share_title,
                    tag: "share-sheet",
                    p { class: "note", "{t(&tr, \"call.share_text\")}" }
                    div { class: "btns",
                        button {
                            class: "btn",
                            onclick: move |_| asking_share.set(false),
                            "{t(&tr, \"common.cancel\")}"
                        }
                        button {
                            class: "btn main",
                            "data-t": "share-go",
                            onclick: {
                                let bridge = bridge.clone();
                                let core = core.clone();
                                move |_| {
                                    asking_share.set(false);
                                    #[cfg(target_os = "android")]
                                    {
                                        let _ = &bridge;
                                        show_screen(&core, phase, &tr);
                                    }
                                    #[cfg(not(target_os = "android"))]
                                    {
                                        let _ = &core;
                                        bridge.send(json!({ "t": "share", "on": true }));
                                    }
                                }
                            },
                            "{t(&tr, \"call.share_go\")}"
                        }
                    }
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
