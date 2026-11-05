//! What the phone itself can do to delay messages: told plainly, with one tap to fix it.

use dioxus::prelude::*;

use crate::app::{Translations, t, use_tr};
use crate::components::{Icon, IconName};

/// One thing that needs a yes.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: &'static str,
    pub title: String,
    pub note: String,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub enum Action {
    Notifications,
    Battery,
    AppPage,
}

/// Looks again every so often while the screen is open: the system's dialogs answer later.
pub fn use_watch() -> Signal<u32> {
    let mut looks = use_signal(|| 0u32);
    #[cfg(target_os = "android")]
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            *looks.write() += 1;
        }
    });
    #[cfg(not(target_os = "android"))]
    let _ = &mut looks;
    looks
}

/// What is wrong right now. Nothing, on a system that has none of this.
pub fn check(tr: &Translations) -> Vec<Item> {
    #[cfg(target_os = "android")]
    {
        use mobile_sentinel::battery;
        use mobile_sentinel::permissions::{PermissionState, status};
        let mut out = Vec::new();
        let mut add = |bad: bool, key, title, note, action| {
            if bad {
                out.push(Item {
                    key,
                    title: t(tr, title),
                    note: t(tr, note),
                    action,
                });
            }
        };
        add(
            status("android.permission.POST_NOTIFICATIONS") == PermissionState::Denied,
            "notifications",
            "health.notifications",
            "health.notifications_note",
            Action::Notifications,
        );
        add(
            !battery::is_exempt(),
            "battery",
            "health.battery",
            "health.battery_note",
            Action::Battery,
        );
        add(
            !battery::background_allowed(),
            "background",
            "health.restricted",
            "health.restricted_note",
            Action::AppPage,
        );
        add(
            !battery::kept_when_unused(),
            "unused",
            "health.revoke",
            "health.revoke_note",
            Action::AppPage,
        );
        out
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = tr;
        Vec::new()
    }
}

fn act(action: Action) {
    #[cfg(target_os = "android")]
    match action {
        Action::Notifications => {
            std::thread::spawn(|| {
                if !crate::android::request_blocking("android.permission.POST_NOTIFICATIONS") {
                    mobile_sentinel::permissions::open_app_settings();
                }
            });
        }
        Action::Battery => mobile_sentinel::battery::request_exemption(),
        Action::AppPage => mobile_sentinel::permissions::open_app_settings(),
    }
    #[cfg(not(target_os = "android"))]
    let _ = action;
}

#[component]
pub fn HealthRow(item: Item) -> Element {
    let tr = use_tr();
    let action = item.action;
    rsx! {
        div { class: "row still",
            span { class: "warn-text", style: "display:flex",
                Icon { name: IconName::Warning, size: 20 }
            }
            span { class: "stack",
                span { class: "row-t", "{item.title}" }
                span { class: "sub", "{item.note}" }
            }
            button {
                class: "btn small-btn",
                onclick: move |_| act(action),
                "{t(&tr, \"health.allow\")}"
            }
        }
    }
}
