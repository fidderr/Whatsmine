//! Android: what the background service, notifications and the Back button
//! need from the app. The service (see mobile-sentinel's `background_service`)
//! keeps the process alive and calls [`Java_com_mobilesentinel_SentinelBackground_nativeStart`]
//! to bring the core up even when no screen exists.

use std::path::Path;
use std::sync::{Mutex, Once, OnceLock};

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use mobile_sentinel::background_service as bg;
use tokio::sync::mpsc;

use crate::i18n;
use crate::notify::{Kind, Notice};

/// Logging and the sentinel's process hooks, once per process (the service
/// and the UI both come through here).
pub fn init_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        mobile_sentinel::init(mobile_sentinel::InitConfig {
            log_tag: "WhatsMine".to_string(),
            log_level: log::LevelFilter::Info,
            ..Default::default()
        });
    });
}

/// Keeps the core told of what only Android knows: the network, the battery, and when to wake.
/// Android's own callbacks run on its threads, and each only passes the news on.
pub fn watch_device(core: whatsmine_core::Core) {
    use mobile_sentinel::battery;
    use mobile_sentinel::network::{self, ConnectionType};
    use std::sync::Arc;
    use whatsmine_core::power::{Device, Network};

    let network_of = |kind: ConnectionType| match kind {
        ConnectionType::Wifi => Network::Wifi,
        ConnectionType::Cellular => Network::Mobile,
        ConnectionType::Other => Network::Wired,
        ConnectionType::None => Network::None,
    };
    let power = battery::power_state();
    let device = Arc::new(Mutex::new(Device {
        network: network_of(network::connection_type()),
        charging: power.charging,
        power_save: power.power_save,
        low_battery: power.low,
    }));
    core.set_device(*device.lock().unwrap_or_else(|p| p.into_inner()));

    {
        let (core, device) = (core.clone(), device.clone());
        network::on_change(move |state| {
            let mut d = device.lock().unwrap_or_else(|p| p.into_inner());
            d.network = network_of(state.kind);
            core.set_device(*d);
            core.network_changed();
        });
    }
    {
        let core = core.clone();
        network::on_radio_active(move || core.radio_active());
    }
    {
        let core = core.clone();
        battery::on_power_change(move |state| {
            let mut d = device.lock().unwrap_or_else(|p| p.into_inner());
            d.charging = state.charging;
            d.power_save = state.power_save;
            d.low_battery = state.low;
            core.set_device(*d);
        });
    }
    {
        let core = core.clone();
        bg::on_wake(move || core.wake());
    }
    // What the core asks for goes to the system's alarm: its own timers stop in Doze.
    let mut events = core.subscribe();
    std::thread::Builder::new()
        .name("whatsmine-wake".into())
        .spawn(move || {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match events.blocking_recv() {
                    Ok(whatsmine_core::CoreEvent::WakeIn { ms }) => {
                        bg::wake_in(ms);
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                }
            }
        })
        .ok();
}

/// What to open when a notification is tapped: `chat:<id>` or `requests`.
fn open_string(n: &Notice) -> String {
    match (&n.chat, n.kind) {
        (Some(c), _) => format!("chat:{c}"),
        (None, Kind::Request) => "requests".into(),
        _ => String::new(),
    }
}

pub fn post(n: &Notice) {
    let kind = match n.kind {
        Kind::Call => bg::Kind::Call,
        Kind::Request => bg::Kind::Request,
        Kind::Message | Kind::Missed => bg::Kind::Message,
    };
    bg::post(&n.tag, &n.title, &n.body, &open_string(n), kind);
}

pub fn cancel(tag: &str) {
    bg::cancel(tag);
}

pub fn cancel_all() {
    bg::cancel_all();
}

/// Tells the service notification and the channels what language to speak.
pub fn apply_texts(locale: &str) {
    let t = i18n::load_translations(locale);
    let get = |k: &str, fallback: &str| t.get(k).cloned().unwrap_or_else(|| fallback.to_string());
    bg::set_texts(
        &get("notify.service_title", "WhatsMine is running"),
        &get("notify.service_text", "Waiting for messages"),
        &get("notify.channel_messages", "Messages"),
        &get("notify.channel_calls", "Calls"),
        &get("notify.channel_requests", "Requests"),
    );
}

/// Turns the background service on or off to match the setting.
pub fn apply_background(on: bool) {
    if on {
        bg::start();
    } else {
        bg::stop();
    }
}

/// Asks for the notification permission the first time the app runs (the
/// system dialog), and never again on its own: after that it is the button in
/// Settings.
pub fn ask_for_notifications_once(core: &whatsmine_core::Core) {
    use mobile_sentinel::permissions::{PermissionState, request, status};
    const PERMISSION: &str = "android.permission.POST_NOTIFICATIONS";
    let Some(dir) = core.media_dir().parent().map(Path::to_path_buf) else {
        return;
    };
    let marker = dir.join("asked-notifications");
    if marker.exists() || status(PERMISSION) != PermissionState::Denied {
        return;
    }
    let _ = std::fs::write(&marker, b"1");
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let _ = request(PERMISSION);
    });
}

/// Makes sure the microphone is allowed (and asks for the camera if the call
/// has video) before a call starts, so its first seconds are not silent.
/// Blocks while the system dialogs are up: call it off the UI thread.
pub fn ensure_call_permissions(video: bool) -> bool {
    let mic = request_blocking("android.permission.RECORD_AUDIO");
    if video {
        // Without the camera the call simply goes on without video.
        let _ = request_blocking("android.permission.CAMERA");
    }
    mic
}

/// Asks for a permission and waits for the user's answer. (The system call
/// only shows the dialog and returns at once, so this watches the activity:
/// it goes away while the dialog is up and comes back when it is answered.)
/// Never call this on the UI thread.
pub fn request_blocking(permission: &str) -> bool {
    use mobile_sentinel::permissions::{PermissionState, request, status};
    use std::time::{Duration, Instant};
    if status(permission) == PermissionState::Granted {
        return true;
    }
    request(permission);
    let started = Instant::now();
    let mut dialog_seen = false;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        if status(permission) == PermissionState::Granted {
            return true;
        }
        if !bg::is_foreground() {
            dialog_seen = true;
        } else if dialog_seen {
            // Back from the dialog and still not granted: the answer was no.
            return false;
        } else if started.elapsed() > Duration::from_secs(3) {
            // No dialog came up (denied for good earlier).
            return false;
        }
        if started.elapsed() > Duration::from_secs(120) {
            return false;
        }
    }
}

// ---- the Back button ----

/// Android sends Back on its own thread; the UI takes the one receiver once.
type BackChannel = (
    mpsc::UnboundedSender<()>,
    Mutex<Option<mpsc::UnboundedReceiver<()>>>,
);

static BACK: OnceLock<BackChannel> = OnceLock::new();

fn back_channel() -> &'static BackChannel {
    BACK.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        (tx, Mutex::new(Some(rx)))
    })
}

/// The Back presses, for the screen to act on. One listener: the screen.
pub fn take_back_events() -> Option<mpsc::UnboundedReceiver<()>> {
    back_channel()
        .1
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
}

// ---- JNI hooks the service and the activity call ----

/// The service started the process (or the UI did): bring the core up.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_mobilesentinel_SentinelBackground_nativeStart(
    mut env: JNIEnv,
    _class: JClass,
    data_dir: JString,
) {
    init_once();
    bg::warm_up();
    let Ok(dir) = env.get_string(&data_dir).map(String::from) else {
        return;
    };
    match crate::boot::core_in(Path::new(&dir)) {
        Ok(core) => {
            let settings = core.settings();
            apply_texts(&settings.locale);
            log::info!("background service: core is up");
        }
        Err(e) => log::error!("background service: core did not start: {e}"),
    }
}

/// Back was pressed; the screen decides what that means.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_mobilesentinel_SentinelBackground_nativeOnBack(
    _env: JNIEnv,
    _class: JClass,
) {
    let _ = back_channel().0.send(());
}
