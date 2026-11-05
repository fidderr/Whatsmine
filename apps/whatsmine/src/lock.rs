//! The app lock: nothing shows until whoever holds the phone proves it is them.

use dioxus::prelude::*;

use crate::app::{t, use_tr};
use crate::components::{Icon, IconName};

/// Whether this device can ask at all. On Windows there is nothing to ask with here.
pub const CAN_LOCK: bool = cfg!(target_os = "android");

#[component]
pub fn LockScreen() -> Element {
    let tr = use_tr();
    let mut open = use_signal(|| !CAN_LOCK);
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

async fn confirm(reason: &str) -> bool {
    #[cfg(target_os = "android")]
    {
        let reason = reason.to_owned();
        tokio::task::spawn_blocking(move || {
            matches!(
                mobile_sentinel::biometric::authenticate(&reason),
                Ok(mobile_sentinel::biometric::AuthResult::Success)
            )
        })
        .await
        .unwrap_or(false)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = reason;
        true
    }
}
