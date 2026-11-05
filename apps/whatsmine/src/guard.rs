//! Keeps this app out of screenshots, shared screens and the list of recent apps. Several things
//! can ask for that at once, and it lasts until none of them does.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy)]
pub enum Reason {
    /// The setting.
    Asked = 1,
    /// Your 12 words are on screen.
    Words = 2,
    /// You are sharing your screen: chats and words stay out of it.
    Sharing = 4,
    /// With the app lock on, the list of recent apps must not show a chat.
    Lock = 8,
}

static REASONS: AtomicU8 = AtomicU8::new(0);

pub fn set(reason: Reason, on: bool) {
    let bit = reason as u8;
    let before = if on {
        REASONS.fetch_or(bit, Ordering::Relaxed)
    } else {
        REASONS.fetch_and(!bit, Ordering::Relaxed)
    };
    let now = if on { before | bit } else { before & !bit };
    if (before != 0) != (now != 0) {
        apply(now != 0);
    }
}

/// What the window is: to be told again when there is a window.
pub fn reapply() {
    apply(REASONS.load(Ordering::Relaxed) != 0);
}

#[cfg(target_os = "android")]
fn apply(on: bool) {
    mobile_sentinel::display::set_secure(on);
}

#[cfg(windows)]
fn apply(on: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };
    let Some(window) = crate::platform::main_window() else {
        return;
    };
    let affinity = if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    // SAFETY: the handle is this app's own main window, which lives as long as the process.
    if unsafe { SetWindowDisplayAffinity(window as _, affinity) } == 0 {
        log::warn!("could not change what the window shows in pictures");
    }
}

#[cfg(not(any(target_os = "android", windows)))]
fn apply(_on: bool) {}
