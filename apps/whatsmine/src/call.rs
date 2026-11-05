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
use whatsmine_core::control::{ASK_WAITS, Ended, Input, MouseButton, READ_FIRST, Refusal};
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
        /// They could be controlled, if they agree.
        sharer_can_control: bool,
        control: ControlView,
    },
}

/// Who is in control of what, during a call.
#[derive(Clone, PartialEq, Debug, Default)]
pub enum ControlView {
    #[default]
    Nothing,
    /// Someone asks to control this PC, since this moment.
    Asked { from: String, at: i64 },
    /// Someone is in control of this PC.
    Owner { who: String },
    /// We are in control of what someone shows.
    Driving,
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
                        let word = |name| m.get(name).and_then(Value::as_str).unwrap_or_default();
                        let can_control = crate::control_win::shown_now(
                            on,
                            word("label"),
                            word("surface"),
                            size("w"),
                            size("h"),
                        );
                        core.call_screen_on(on, size("w"), size("h"), can_control);
                    }
                    "askKey" => core.call_screen_key(),
                    "ctl" => core.control_send(read_input(&m)),
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
            for_state.call_screen_on(state.is_some(), w, h, false);
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
                sharer_can_control: false,
                control: ControlView::Nothing,
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
                sharer_can_control: false,
                control: ControlView::Nothing,
            });
        }
        CallEvent::PeerScreen {
            from,
            on,
            can_control,
            ..
        } => {
            if let Phase::Active {
                sharer,
                sharer_can_control,
                ..
            } = &mut *phase.write()
            {
                if on {
                    *sharer = Some(from.clone());
                    *sharer_can_control = can_control;
                } else if sharer.as_deref() == Some(from.as_str()) {
                    *sharer = None;
                    *sharer_can_control = false;
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
        CallEvent::ControlAsked { from, .. } => {
            set_control(phase, ControlView::Asked { from, at: now_ms() });
            come_forward();
        }
        CallEvent::ControlAskedAnyway { from, why, .. } => {
            let key = match why {
                Refusal::TurnedOff => Some("control.asked_off"),
                Refusal::TooNew => Some("control.asked_new"),
                Refusal::NotOneToOne | Refusal::NotSharing | Refusal::NotPossible => {
                    Some("control.asked_not_now")
                }
                Refusal::Busy | Refusal::SaidNo => None,
            };
            if let Some(key) = key {
                let line = t(tr, key).replace("{name}", &name_of(core, &from));
                toast(toast_sig, line, ToastLevel::Info);
            }
        }
        CallEvent::ControlRefused { from, why, .. } => {
            let key = match why {
                Refusal::SaidNo => "control.said_no",
                Refusal::Busy => "control.busy",
                _ => "control.cannot",
            };
            let line = t(tr, key).replace("{name}", &name_of(core, &from));
            toast(toast_sig, line, ToastLevel::Info);
        }
        CallEvent::ControlOn {
            who, driving: true, ..
        } => {
            let _ = who;
            set_control(phase, ControlView::Driving);
            bridge.send(json!({ "t": "control", "on": true, "keys": t(tr, "control.keyboard") }));
        }
        CallEvent::ControlOn {
            who,
            driving: false,
            until,
            ..
        } => {
            let name = name_of(core, &who);
            crate::control_win::on(
                core,
                until.unwrap_or_else(now_ms),
                crate::control_win::BarWords {
                    who: t(tr, "control.in_control").replace("{name}", &name),
                    more: t(tr, "control.more_time"),
                    stop: t(tr, "control.stop"),
                },
            );
            set_control(phase, ControlView::Owner { who });
        }
        CallEvent::ControlOff {
            who, driving, why, ..
        } => {
            if driving {
                bridge.send(json!({ "t": "control", "on": false }));
            } else {
                crate::control_win::off();
            }
            let asked_only = !driving
                && matches!(
                    &*phase.peek(),
                    Phase::Active {
                        control: ControlView::Asked { .. },
                        ..
                    }
                );
            set_control(phase, ControlView::Nothing);
            if !asked_only {
                let name = name_of(core, &who);
                let note = if driving {
                    "control.note_driver"
                } else {
                    "control.note_owner"
                };
                core.add_chat_note(&who, &t(tr, note).replace("{name}", &name));
                if !matches!(why, Ended::CallOrSharing) {
                    let line = t(tr, ended_key(why)).replace("{name}", &name);
                    toast(toast_sig, line, ToastLevel::Info);
                }
            }
        }
        CallEvent::Ended {
            contact_id, reason, ..
        } => {
            let was_incoming = matches!(&*phase.read(), Phase::Incoming { .. });
            crate::guard::set(crate::guard::Reason::Sharing, false);
            crate::control_win::shown_now(false, "", "", 0, 0);
            crate::control_win::off();
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

fn set_control(mut phase: Signal<Phase>, view: ControlView) {
    if let Phase::Active { control, .. } = &mut *phase.write() {
        *control = view;
    }
}

fn ended_key(why: Ended) -> &'static str {
    match why {
        Ended::Owner => "control.ended_owner",
        Ended::Them => "control.ended_them",
        Ended::TimeUp => "control.ended_time",
        Ended::Silence => "control.ended_silence",
        Ended::CallOrSharing => "control.ended_sharing",
        Ended::NotSafeAnyMore => "control.ended_unsafe",
        Ended::Misbehaved => "control.ended_wrong",
    }
}

/// The owner may be looking at what they are showing. This must not go unseen.
fn come_forward() {
    #[cfg(feature = "desktop")]
    if !crate::platform::test_run() {
        dioxus::desktop::window().set_focus();
    }
}

/// What the page sends while someone is in control. Whatever does not fit is left out.
fn read_input(m: &Value) -> Vec<Input> {
    let Some(events) = m.get("e").and_then(Value::as_array) else {
        return Vec::new();
    };
    events.iter().filter_map(one_input).collect()
}

fn one_input(event: &Value) -> Option<Input> {
    let number = |at: usize| event.get(at).and_then(Value::as_i64);
    let down = |at: usize| number(at).map(|n| n != 0);
    match event.get(0)?.as_str()? {
        "m" => Some(Input::Move {
            x: u16::try_from(number(1)?).ok()?,
            y: u16::try_from(number(2)?).ok()?,
        }),
        "b" => Some(Input::Button {
            which: match number(1)? {
                0 => MouseButton::Left,
                1 => MouseButton::Middle,
                2 => MouseButton::Right,
                _ => return None,
            },
            down: down(2)?,
        }),
        "w" => Some(Input::Wheel {
            dx: i16::try_from(number(1)?).ok()?,
            dy: i16::try_from(number(2)?).ok()?,
        }),
        "k" => Some(Input::Key {
            code: u16::try_from(number(1)?).ok()?,
            extended: down(2)?,
            down: down(3)?,
        }),
        "t" => {
            let text = event.get(1)?.as_str()?;
            Some(Input::Text(
                text.chars()
                    .take(whatsmine_core::control::MOST_TEXT)
                    .collect(),
            ))
        }
        _ => None,
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
    let mut control_tick = use_signal(|| false);
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
    let control_view = match &current {
        Phase::Active { control, .. } => control.clone(),
        _ => ControlView::Nothing,
    };
    let can_ask = matches!(
        &current,
        Phase::Active {
            sharer: Some(_),
            sharer_can_control: true,
            group: false,
            control: ControlView::Nothing,
            ..
        }
    );
    let driving = control_view == ControlView::Driving;
    let in_control_line = match &control_view {
        ControlView::Owner { who } => {
            Some(t(&tr, "control.in_control").replace("{name}", &name_of(&core, who)))
        }
        _ => None,
    };
    let asked = match &control_view {
        ControlView::Asked { from, at } if now() - at <= ASK_WAITS => {
            Some((from.clone(), now() - at))
        }
        _ => None,
    };
    if asked.is_none() && control_tick() {
        control_tick.set(false);
    }
    // What the owner is about to allow: what is being shown is what can be used.
    let shown_scope = crate::control_win::shown().map(crate::control_win::Target::scope);
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
                if let Some(line) = &in_control_line {
                    div { class: "call-in-control", "data-t": "control-owner-line",
                        span { "{line}" }
                        button {
                            class: "btn danger",
                            "data-t": "control-owner-stop",
                            onclick: {
                                let core = core.clone();
                                move |_| core.control_stop()
                            },
                            "{t(&tr, \"control.stop\")}"
                        }
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
                    Phase::Active { muted, .. } if driving => {
                        let mute_label = t(&tr, if muted { "call.unmute" } else { "call.mute" });
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
                                Icon { name: if muted { IconName::MicOff } else { IconName::Mic }, size: 22 }
                            }
                            button {
                                class: "call-btn toggle",
                                "data-t": "control-stop",
                                aria_label: "{t(&tr, \"control.stop_mine\")}",
                                onclick: {
                                    let core = core.clone();
                                    move |_| core.control_stop()
                                },
                                Icon { name: IconName::Mouse, size: 22 }
                            }
                            button {
                                class: "call-btn decline",
                                "data-t": "call-hangup",
                                aria_label: "{t(&tr, \"call.hangup\")}",
                                onclick: {
                                    let core = core.clone();
                                    move |_| core.call_end()
                                },
                                Icon { name: IconName::PhoneOff, size: 22 }
                            }
                        }
                    }
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
                            if can_ask {
                                button {
                                    class: "call-btn toggle on",
                                    "data-t": "control-ask",
                                    aria_label: "{t(&tr, \"control.ask\")}",
                                    "data-label": "{t(&tr, \"control.ask_short\")}",
                                    onclick: {
                                        let core = core.clone();
                                        let mut toast_sig = toast_sig;
                                        let who = someone_name.clone().unwrap_or_default();
                                        move |_| {
                                            core.control_ask();
                                            let line = t(&tr, "control.asking").replace("{name}", &who);
                                            toast(&mut toast_sig, line, ToastLevel::Info);
                                        }
                                    },
                                    Icon { name: IconName::Mouse, size: 26 }
                                }
                            }
                            if driving {
                                button {
                                    class: "call-btn toggle",
                                    "data-t": "control-stop",
                                    aria_label: "{t(&tr, \"control.stop_mine\")}",
                                    "data-label": "{t(&tr, \"control.stop\")}",
                                    onclick: {
                                        let core = core.clone();
                                        move |_| core.control_stop()
                                    },
                                    Icon { name: IconName::Mouse, size: 26 }
                                }
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
            if let Some((from, waited)) = &asked {
                {
                    let who = name_of(&core, from);
                    let title = t(&tr, "control.asked_title").replace("{name}", &who);
                    let wait_left = (READ_FIRST - waited + 999) / 1000;
                    let ready = wait_left <= 0 && control_tick();
                    let allow_word = if wait_left > 0 {
                        t(&tr, "control.allow_wait").replace("{n}", &wait_left.to_string())
                    } else {
                        t(&tr, "control.allow")
                    };
                    let scope_key = match shown_scope {
                        Some(whatsmine_core::control::Scope::Window) => "control.scope_window",
                        _ => "control.scope_screen",
                    };
                    let no_core = core.clone();
                    let close_core = core.clone();
                    let allow_core = core.clone();
                    rsx! {
                        crate::components::Sheet {
                            // Away from it means no.
                            on_close: move |_| {
                                close_core.control_answer(false, whatsmine_core::control::Scope::Window);
                                set_control(phase, ControlView::Nothing);
                            },
                            title,
                            tag: "control-sheet",
                            p { class: "control-warning", "data-t": "control-warning", "{t(&tr, \"control.warning\")}" }
                            p { class: "note", "{t(&tr, scope_key)}" }
                            p { class: "note", "{t(&tr, \"control.what_stays\")}" }
                            label { class: "control-tick",
                                input {
                                    r#type: "checkbox",
                                    "data-t": "control-tick",
                                    checked: control_tick(),
                                    onchange: move |e| control_tick.set(e.checked()),
                                }
                                span { "{t(&tr, \"control.tick\")}" }
                            }
                            div { class: "btns",
                                button {
                                    class: "btn",
                                    "data-t": "control-allow",
                                    disabled: !ready,
                                    onclick: move |_| {
                                        if let Some(scope) = shown_scope {
                                            crate::control_win::prepare(allow_core.control_input());
                                            allow_core.control_answer(true, scope);
                                        }
                                    },
                                    "{allow_word}"
                                }
                                button {
                                    class: "btn main",
                                    "data-t": "control-no",
                                    onclick: move |_| {
                                        no_core.control_answer(false, whatsmine_core::control::Scope::Window);
                                        set_control(phase, ControlView::Nothing);
                                    },
                                    "{t(&tr, \"control.no\")}"
                                }
                            }
                        }
                    }
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
    use super::{clock, read_input};
    use serde_json::json;
    use whatsmine_core::control::{Input, MouseButton};

    #[test]
    fn what_the_page_sends_becomes_input_and_the_rest_does_not() {
        let m = json!({ "e": [
            ["m", 10, 20], ["b", 2, 1], ["w", 0, -120], ["k", 30, 0, 1], ["t", "hello"],
            ["m", 70000, 0], ["m", -1, 0], ["b", 9, 1], ["k", 1], ["x"], [], 5, "m", null,
            ["t", 5], ["w", 40000, 0]
        ]});
        assert_eq!(
            read_input(&m),
            [
                Input::Move { x: 10, y: 20 },
                Input::Button {
                    which: MouseButton::Right,
                    down: true
                },
                Input::Wheel { dx: 0, dy: -120 },
                Input::Key {
                    code: 30,
                    extended: false,
                    down: true
                },
                Input::Text("hello".into()),
            ]
        );
        assert!(read_input(&json!({})).is_empty());
        assert!(read_input(&json!({ "e": "nonsense" })).is_empty());
    }

    #[test]
    fn clock_formats() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(59_999), "0:59");
        assert_eq!(clock(61_000), "1:01");
        assert_eq!(clock(3_725_000), "1:02:05");
        assert_eq!(clock(-5), "0:00");
    }
}
