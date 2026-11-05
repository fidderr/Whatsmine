//! What the test scenarios (`devtest/`) drive the app with. Only in a build with the
//! `test-hooks` feature, never in one for users.
//!
//! The harness writes `<data dir>/debug_action.txt`, and the file goes once it was seen:
//!
//!   `js:<script>`: run in the page, the result is left in `debug_script_result.txt`.
//!     It is how the scenarios press buttons and read the screen, on any screen size.
//!   `report`: how the connection stands, in `debug_report.txt`.
//!   `core:<what>`: what only the core can do, for a test with two real windows. The answer is
//!     left in `debug_script_result.txt`: `invite`, `key`, `add <link>`, `accept <key>`,
//!     `online <key>`, `requests`.
//!
//! `<data dir>/debug_<name>.txt` holds a number to use instead of a wait that takes too long for
//! a test, see [`crate::platform::shortened`].
//!
//! `<data dir>/debug_seed.txt`, there before the app starts, sets up what a scenario needs, see
//! [`seed_if_asked`].

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

static SCRIPT: Mutex<Option<String>> = Mutex::new(None);
static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Set to `WIDTHxHEIGHT`: this PC is running tests. See [`test_window`].
pub const TEST_WINDOW: &str = "WHATSMINE_TEST_WINDOW";

/// Watch for what the harness writes.
pub fn start(dir: PathBuf, core: whatsmine_core::Core) {
    if DIR.set(dir.clone()).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        let mut records = None;
        loop {
            std::thread::sleep(Duration::from_millis(30));
            if let Ok(runtime) = &runtime
                && let Some(what) = dht_asked(&dir)
            {
                let answer = runtime.block_on(async {
                    if records.is_none() {
                        records = whatsmine_core::records::Records::join().ok();
                    }
                    match &records {
                        Some(records) => dht_check(records, &what).await,
                        None => "no DHT".to_owned(),
                    }
                });
                let unfinished = dir.join("debug_dht.tmp");
                if std::fs::write(&unfinished, answer).is_ok() {
                    let _ = std::fs::rename(unfinished, dir.join("debug_dht.txt"));
                }
            }
            if take_action(&dir, &core)
                && let Ok(runtime) = &runtime
            {
                let report = runtime.block_on(core.connection_report());
                write_report(&dir, report);
            }
        }
    });
}

fn dht_asked(dir: &Path) -> Option<String> {
    let what = std::fs::read_to_string(dir.join("debug_dht_ask.txt")).ok()?;
    let _ = std::fs::remove_file(dir.join("debug_dht_ask.txt"));
    Some(what)
}

/// `put <word> <bytes>` or `get <word>`: can this device reach the DHT, and can another one read
/// what it wrote? The word is what both sides agree on.
async fn dht_check(records: &whatsmine_core::records::Records, what: &str) -> String {
    let mut words = what.split_whitespace();
    let (verb, word) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    let mut secret = [0_u8; 32];
    let take = word.len().min(32);
    secret[..take].copy_from_slice(&word.as_bytes()[..take]);
    let slot = whatsmine_core::records::seed("devtest", &secret, 0, 0);
    let started = std::time::Instant::now();
    match verb {
        "put" => {
            let size = words.next().and_then(|n| n.parse().ok()).unwrap_or(10);
            let wrote = records.put(&slot, &vec![7; size]).await;
            format!("wrote={wrote} in {} ms", started.elapsed().as_millis())
        }
        "get" => {
            let read = records.get(&slot).await;
            format!(
                "read={:?} in {} ms",
                read.map(|value| value.len()),
                started.elapsed().as_millis()
            )
        }
        _ => "what?".to_owned(),
    }
}

/// Returns whether the harness asked for a report.
fn take_action(dir: &Path, core: &whatsmine_core::Core) -> bool {
    let path = dir.join("debug_action.txt");
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return false;
    };
    let contents = contents.trim();
    if let Some(script) = contents.strip_prefix("js:") {
        *SCRIPT.lock().unwrap_or_else(PoisonError::into_inner) = Some(script.to_owned());
    }
    if let Some(what) = contents.strip_prefix("core:") {
        let answer = core_action(core, what);
        let unfinished = dir.join("debug_script_result.tmp");
        if std::fs::write(&unfinished, format!("={answer}")).is_ok() {
            let _ = std::fs::rename(unfinished, dir.join("debug_script_result.txt"));
        }
    }
    let _ = std::fs::remove_file(&path);
    contents == "report"
}

fn core_action(core: &whatsmine_core::Core, what: &str) -> String {
    let (verb, rest) = what.split_once(' ').unwrap_or((what, ""));
    match verb {
        "invite" => core.invite_link(),
        "key" => core.my_key(),
        // A test window never has the focus: it says so itself, to be treated as in use.
        "front" => {
            core.set_in_front(true);
            "ok".to_owned()
        }
        "add" => core
            .add_contact(rest, None)
            .map_or_else(|e| format!("error: {e}"), |c| c.id),
        "accept" => core
            .accept(rest)
            .map_or_else(|e| format!("error: {e}"), |()| "ok".to_owned()),
        "online" => core.is_online(rest).to_string(),
        // Turned on some time ago, and this person has been known for a long time.
        "control_on" => {
            let mut settings = core.settings();
            settings.control_on_since = Some(whatsmine_core::now_ms() - 60_000);
            core.save_settings(&settings)
                .map_or_else(|e| format!("error: {e}"), |()| "ok".to_owned())
        }
        "known_long" => {
            core.known_since_as_is(
                rest,
                whatsmine_core::now_ms() - whatsmine_core::control::KNOWN_FOR - 60_000,
            );
            "ok".to_owned()
        }
        // The owner's own hand on the mouse, in a test run where nothing looks at the real one.
        "control_busy" => {
            core.control_owner_busy();
            "ok".to_owned()
        }
        "control_stop" => {
            core.control_stop();
            "ok".to_owned()
        }
        "control_log" => core.control_log().len().to_string(),
        "requests" => core
            .requests()
            .iter()
            .map(|c| c.id.clone())
            .collect::<Vec<_>>()
            .join(" "),
        _ => "what?".to_owned(),
    }
}

/// In one go: the harness reads it as soon as it is there.
fn write_report(dir: &Path, report: Option<whatsmine_core::ConnectionReport>) {
    let text = report.map_or_else(
        || "no report".to_owned(),
        |r| {
            format!(
                "level={:?} up={} network={:?} reachable_direct={} on_relay={} ipv6={} hard_nat={:?} peers={}",
                r.level,
                r.endpoint_up,
                r.network,
                r.reachable_direct,
                r.on_relay,
                r.ipv6,
                r.hard_nat,
                r.peers.len()
            )
        },
    );
    let unfinished = dir.join("debug_report.tmp");
    if std::fs::write(&unfinished, text).is_ok() {
        let _ = std::fs::rename(unfinished, dir.join("debug_report.txt"));
    }
}

/// What the harness left for this start, in words: `demo` for a device that has been in use for
/// a while, and `name=`, `theme=`, `locale=`. Anything in it means past the first-run screen.
pub fn seed_if_asked(core: &whatsmine_core::Core) {
    let path = core.data_dir().join("debug_seed.txt");
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return;
    };
    let _ = std::fs::remove_file(&path);
    let mut settings = core.settings();
    settings.onboarded = true;
    for word in contents.split_whitespace() {
        match word.split_once('=') {
            Some(("name", value)) => settings.display_name = value.to_owned(),
            Some(("theme", value)) => settings.app_look = value.to_owned(),
            Some(("locale", value)) => settings.locale = value.to_owned(),
            None if word == "demo" => {
                if let Err(e) = core.seed_demo() {
                    log::warn!("seeding failed: {e:#}");
                }
            }
            _ => log::warn!("seed: what is {word}?"),
        }
    }
    if let Err(e) = core.save_settings(&settings) {
        log::warn!("seeding the settings failed: {e:#}");
    }
}

/// The number the harness left for `name`, if any.
pub fn test_value(name: &str) -> Option<u32> {
    let path = DIR.get()?.join(format!("debug_{name}.txt"));
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Run what the harness sent in the page.
pub async fn script_loop() {
    loop {
        tokio::time::sleep(Duration::from_millis(30)).await;
        let script = SCRIPT.lock().unwrap_or_else(PoisonError::into_inner).take();
        let Some(script) = script else {
            continue;
        };
        let mut eval = dioxus::document::eval(&format!(
            "try {{ dioxus.send(String((function () {{ {script} }})())); }} \
             catch (e) {{ dioxus.send('error: ' + e); }}"
        ));
        // An answer can get lost while the page is away: the next script must still run.
        let result = tokio::time::timeout(Duration::from_secs(2), eval.recv::<String>())
            .await
            .map_or_else(
                |_| "error: no answer".to_owned(),
                |answer| answer.unwrap_or_default(),
            );
        let Some(dir) = DIR.get() else {
            continue;
        };
        // In one go: the harness reads it as soon as it is there.
        let unfinished = dir.join("debug_script_result.tmp");
        if std::fs::write(&unfinished, format!("={result}")).is_ok() {
            let _ = std::fs::rename(unfinished, dir.join("debug_script_result.txt"));
        }
    }
}

/// With [`TEST_WINDOW`] set: a window of that size, off-screen, that never takes the focus and
/// has no button on the taskbar. Tests can run while the PC is in use.
#[cfg(feature = "desktop")]
pub fn test_window(window: dioxus::desktop::WindowBuilder) -> dioxus::desktop::WindowBuilder {
    use dioxus::desktop::{LogicalPosition, LogicalSize};
    let Ok(size) = std::env::var(TEST_WINDOW) else {
        return window;
    };
    let (width, height) = size
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((440.0, 780.0));
    let window = window
        .with_position(LogicalPosition::new(-8000.0, 0.0))
        .with_inner_size(LogicalSize::<f64>::new(width, height))
        .with_min_inner_size(LogicalSize::<f64>::new(200.0, 200.0))
        .with_focused(false);
    #[cfg(windows)]
    let window = {
        use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
        window.with_skip_taskbar(true)
    };
    window
}
