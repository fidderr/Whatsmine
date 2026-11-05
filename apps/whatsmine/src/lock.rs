//! The app lock: nothing shows until whoever holds the device proves it is them, when the app
//! starts and when it comes back after being away.

use std::sync::OnceLock;

use dioxus::prelude::*;
use whatsmine_core::model::now_ms;

use crate::app::{t, use_tr};
use crate::components::{Icon, IconName, use_settings};

/// Whether this device can ask at all. Without a fingerprint, a face or a screen lock there is
/// nothing to ask with, and a lock would only lock its owner out.
pub fn can_lock() -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(|| {
        if crate::platform::test_run() {
            return false;
        }
        #[cfg(target_os = "android")]
        {
            mobile_sentinel::biometric::can_ask_with_screen_lock()
        }
        #[cfg(windows)]
        {
            hello::available()
        }
        #[cfg(not(any(target_os = "android", windows)))]
        {
            false
        }
    })
}

/// Is the app in sight? On a PC, looking at another window for a moment is not being away.
#[cfg(not(feature = "desktop"))]
fn in_sight() -> bool {
    crate::notify::ui_active()
}

#[cfg(feature = "desktop")]
fn in_sight() -> bool {
    let window = dioxus::desktop::window();
    window.is_visible() && !window.is_minimized()
}

#[component]
pub fn LockScreen() -> Element {
    let tr = use_tr();
    let settings = use_settings();
    let mut open = use_signal(|| !can_lock());
    let mut asking = use_signal(|| false);
    let mut ask = move || {
        if *asking.peek() {
            return;
        }
        asking.set(true);
        spawn(async move {
            let passed = confirm(&t(&tr, "lock.reason")).await;
            asking.set(false);
            if passed {
                open.set(true);
            }
        });
    };
    use_hook(ask);
    // Back after a while: ask again.
    use_future(move || async move {
        let mut ask = ask;
        let mut away_since: Option<i64> = None;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            // The question itself may take the app out of sight.
            if *asking.peek() || !can_lock() {
                away_since = None;
                continue;
            }
            match (in_sight(), away_since) {
                (false, None) => away_since = Some(now_ms()),
                (true, Some(since)) => {
                    away_since = None;
                    let allowed = i64::from(settings.peek().lock_after_s) * 1000;
                    if now_ms() - since >= allowed && *open.peek() {
                        open.set(false);
                        ask();
                    }
                }
                _ => {}
            }
        }
    });
    if open() {
        return rsx! {};
    }
    rsx! {
        div { class: "lock-screen",
            Icon { name: IconName::Lock, size: 56 }
            p { class: "empty-title", "{t(&tr, \"lock.title\")}" }
            button { class: "btn main", onclick: move |_| ask(),
                "{t(&tr, \"lock.unlock\")}"
            }
        }
    }
}

/// Whoever holds the device proves it is them, where it can ask at all.
pub async fn prove(reason: &str) -> bool {
    !can_lock() || confirm(reason).await
}

async fn confirm(reason: &str) -> bool {
    #[cfg(target_os = "android")]
    {
        let reason = reason.to_owned();
        tokio::task::spawn_blocking(move || {
            mobile_sentinel::biometric::authenticate_or_screen_lock(&reason).is_ok()
        })
        .await
        .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let reason = reason.to_owned();
        tokio::task::spawn_blocking(move || hello::ask(&reason))
            .await
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "android", windows)))]
    {
        let _ = reason;
        true
    }
}

/// Windows Hello: a face, a fingerprint or the PIN of this PC.
#[cfg(windows)]
mod hello {
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    };
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
    use windows::core::{HSTRING, factory};
    use windows_future::IAsyncOperation;

    pub fn available() -> bool {
        UserConsentVerifier::CheckAvailabilityAsync()
            .and_then(|asked| asked.get())
            .is_ok_and(|answer| answer == UserConsentVerifierAvailability::Available)
    }

    /// Blocks until there is an answer. The question belongs to our window, without one it fails.
    pub fn ask(reason: &str) -> bool {
        let Some(window) = crate::platform::main_window() else {
            return false;
        };
        let answer = (|| -> windows::core::Result<UserConsentVerificationResult> {
            let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
            // SAFETY: the handle is this app's own main window, which lives as long as the process.
            let asked: IAsyncOperation<UserConsentVerificationResult> = unsafe {
                interop.RequestVerificationForWindowAsync(
                    HWND(window as *mut core::ffi::c_void),
                    &HSTRING::from(reason),
                )?
            };
            asked.get()
        })();
        answer.is_ok_and(|result| result == UserConsentVerificationResult::Verified)
    }
}
