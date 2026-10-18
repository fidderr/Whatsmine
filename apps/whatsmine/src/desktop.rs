//! Desktop (Windows): one running copy per profile, a tray icon, toasts and
//! "start with the PC". Closing the window hides it to the tray (when the
//! background setting is on) so messages keep arriving.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc;

use crate::notify::Notice;

const APP_ID: &str = "com.fidderr.whatsmine";

/// What another launch of the app can ask the running one to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Show,
    Quit,
    /// A `whatsmine://` link was clicked.
    Open(String),
}

static STARTUP_LINK: Mutex<Option<String>> = Mutex::new(None);

/// A link this copy was started with (it was not running when the link was clicked).
pub fn set_startup_link(link: String) {
    *STARTUP_LINK.lock().unwrap_or_else(|p| p.into_inner()) = Some(link);
}

pub fn take_startup_link() -> Option<String> {
    STARTUP_LINK
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
}

static COMMANDS: OnceLock<mpsc::UnboundedSender<Command>> = OnceLock::new();
static RECEIVER: Mutex<Option<mpsc::UnboundedReceiver<Command>>> = Mutex::new(None);

/// The commands from later launches, for the UI to act on. Taken once.
pub fn take_commands() -> Option<mpsc::UnboundedReceiver<Command>> {
    RECEIVER.lock().unwrap_or_else(|p| p.into_inner()).take()
}

/// A port per profile on the loopback interface: whoever holds it is the
/// running copy for that profile.
fn port_for(profile: Option<&str>) -> u16 {
    let mut h: u32 = 0x811c_9dc5;
    for b in profile.unwrap_or("").bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    47_000 + (h % 2_000) as u16
}

/// Becomes the running copy for `profile`, or (if there already is one)
/// tells it what to do and returns `false`: this process should then exit.
pub fn claim_instance(profile: Option<&str>, forward: Option<Command>) -> bool {
    let port = port_for(profile);
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            let (tx, rx) = mpsc::unbounded_channel();
            let _ = COMMANDS.set(tx.clone());
            *RECEIVER.lock().unwrap_or_else(|p| p.into_inner()) = Some(rx);
            std::thread::Builder::new()
                .name("whatsmine-instance".into())
                .spawn(move || {
                    for stream in listener.incoming().flatten() {
                        let mut line = String::new();
                        let mut reader = BufReader::new(&stream);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        if reader.read_line(&mut line).is_err() {
                            continue;
                        }
                        let _ = (&stream).write_all(b"whatsmine\n");
                        let line = line.trim();
                        match line {
                            "show" => {
                                let _ = tx.send(Command::Show);
                            }
                            "quit" => {
                                let _ = tx.send(Command::Quit);
                            }
                            _ => {
                                if let Some(link) = line.strip_prefix("open ") {
                                    let _ = tx.send(Command::Open(link.to_string()));
                                }
                            }
                        }
                    }
                })
                .ok();
            true
        }
        Err(_) => {
            let say = match forward {
                Some(Command::Quit) => "quit".to_string(),
                Some(Command::Show) => "show".to_string(),
                // One line only: nothing in a link may smuggle in a second command.
                Some(Command::Open(link)) => format!("open {}", link.replace(['\r', '\n'], "")),
                None => "ping".to_string(),
            };
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(mut s) => {
                    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = writeln!(s, "{say}");
                    let mut reply = String::new();
                    // A real copy answers; something else on that port does not,
                    // and then we simply go on as the running copy ourselves.
                    BufReader::new(&s).read_line(&mut reply).ok();
                    reply.trim() != "whatsmine"
                }
                Err(_) => true,
            }
        }
    }
}

// ---- Windows only ----

#[cfg(windows)]
fn run_key_name(profile: Option<&str>) -> String {
    match profile.filter(|p| !p.is_empty()) {
        Some(p) => format!("WhatsMine-{p}"),
        None => "WhatsMine".into(),
    }
}

/// Starts (or stops starting) the app, hidden in the tray, when the user logs in.
pub fn set_autostart(on: bool, profile: Option<&str>) -> bool {
    #[cfg(windows)]
    {
        use windows_registry::CURRENT_USER;
        let Ok(key) = CURRENT_USER.create(r"Software\Microsoft\Windows\CurrentVersion\Run") else {
            return false;
        };
        let name = run_key_name(profile);
        if on {
            let Ok(exe) = std::env::current_exe() else {
                return false;
            };
            key.set_string(&name, format!("\"{}\" --background", exe.display()))
                .is_ok()
        } else {
            let _ = key.remove_value(&name);
            true
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (on, profile);
        false
    }
}

pub fn autostart_enabled(profile: Option<&str>) -> bool {
    #[cfg(windows)]
    {
        use windows_registry::CURRENT_USER;
        CURRENT_USER
            .open(r"Software\Microsoft\Windows\CurrentVersion\Run")
            .and_then(|k| k.get_string(run_key_name(profile)))
            .is_ok()
    }
    #[cfg(not(windows))]
    {
        let _ = profile;
        false
    }
}

/// Makes `whatsmine://` links open this app (current user only).
pub fn register_link_handler() {
    #[cfg(windows)]
    std::thread::spawn(|| {
        use windows_registry::CURRENT_USER;
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let Ok(root) = CURRENT_USER.create(r"Software\Classes\whatsmine") else {
            return;
        };
        let _ = root.set_string("", "URL:WhatsMine");
        let _ = root.set_string("URL Protocol", "");
        if let Ok(cmd) = CURRENT_USER.create(r"Software\Classes\whatsmine\shell\open\command") {
            let _ = cmd.set_string("", format!("\"{}\" \"%1\"", exe.display()));
        }
    });
}

/// Gives toasts a name and icon of our own instead of "Windows PowerShell".
#[cfg(windows)]
fn register_app_id(icon_png: &std::path::Path) {
    use windows_registry::CURRENT_USER;
    if let Ok(key) = CURRENT_USER.create(format!(r"Software\Classes\AppUserModelId\{APP_ID}")) {
        let _ = key.set_string("DisplayName", "WhatsMine");
        let _ = key.set_string("IconUri", icon_png.display().to_string());
        let _ = key.set_string("IconBackgroundColor", "FF0B1220");
    }
}

/// Windows 11 tucks new tray icons into the overflow menu. Ask for ours to be
/// shown next to the clock (the user can still hide it in Windows settings).
pub fn promote_tray_icon() {
    #[cfg(windows)]
    std::thread::spawn(|| {
        use windows_registry::CURRENT_USER;
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let exe = exe.display().to_string().to_lowercase();
        // Windows records the icon a moment after it is created.
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(500));
            let Ok(root) = CURRENT_USER.open(r"Control Panel\NotifyIconSettings") else {
                continue;
            };
            let Ok(names) = root.keys() else { continue };
            let mut done = false;
            for name in names {
                let Ok(key) =
                    CURRENT_USER.create(format!(r"Control Panel\NotifyIconSettings\{name}"))
                else {
                    continue;
                };
                let path = key
                    .get_string("ExecutablePath")
                    .unwrap_or_default()
                    .to_lowercase();
                if path == exe {
                    if key.get_u32("IsPromoted").unwrap_or(0) != 1 {
                        let _ = key.set_u32("IsPromoted", 1);
                    }
                    done = true;
                }
            }
            if done {
                return;
            }
        }
    });
}

/// Shows a Windows toast. False if the system refused it.
pub fn toast(n: &Notice) -> bool {
    #[cfg(windows)]
    {
        static ICON: OnceLock<()> = OnceLock::new();
        ICON.get_or_init(|| {
            let path = std::env::temp_dir().join("whatsmine-toast-icon.png");
            if std::fs::write(&path, include_bytes!("../assets/icon.png")).is_ok() {
                register_app_id(&path);
            }
        });
        let result = notify_rust::Notification::new()
            .app_id(APP_ID)
            .summary(&n.title)
            .body(&n.body)
            .show();
        if let Err(e) = &result {
            log::warn!("toast failed: {e}");
        }
        result.is_ok()
    }
    #[cfg(not(windows))]
    {
        let _ = n;
        false
    }
}
