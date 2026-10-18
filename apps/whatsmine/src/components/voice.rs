use std::time::{Duration, Instant};

use dioxus::prelude::*;

use crate::app::{t, use_tr};
use crate::back::BackLayer;
use crate::components::{Icon, IconName};
use crate::fmt;
use crate::media::{self, Picked};

/// Shown in place of the text box while a voice note is being recorded.
/// The recording itself was started by the caller; this stops or discards it.
#[component]
pub fn VoiceBar(on_send: EventHandler<Picked>, on_cancel: EventHandler<()>) -> Element {
    let tr = use_tr();
    let mut elapsed = use_signal(|| 0u64);
    let mut level = use_signal(|| 0f32);
    let mut stopping = use_signal(|| false);
    let started = use_hook(Instant::now);

    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_millis(90)).await;
            elapsed.set(started.elapsed().as_millis() as u64);
            level.set(mobile_sentinel::recorder::level());
        }
    });
    // Leaving the screen mid-recording throws it away rather than leaving
    // the microphone open.
    use_drop(mobile_sentinel::recorder::cancel);

    let pct = (level() * 140.0).clamp(4.0, 100.0);
    let discard = move |_: ()| {
        spawn(async move {
            media::cancel_voice().await;
            on_cancel.call(());
        });
    };
    rsx! {
        div { class: "voice-bar",
            BackLayer { on_back: move |_| discard(()) }
            button {
                class: "icon-btn danger",
                title: "{t(&tr, \"voice.discard\")}",
                disabled: stopping(),
                onclick: move |_| discard(()),
                Icon { name: IconName::Trash, size: 22 }
            }
            span { class: "rec-dot" }
            span { class: "rec-time", "{fmt::duration(elapsed())}" }
            div { class: "voice-level",
                div { class: "voice-level-fill", style: "width: {pct}%;" }
            }
            button {
                class: "send-btn",
                title: "{t(&tr, \"voice.send\")}",
                disabled: stopping(),
                onclick: move |_| {
                    stopping.set(true);
                    spawn(async move {
                        match media::finish_voice().await {
                            Some(p) => on_send.call(p),
                            None => on_cancel.call(()),
                        }
                    });
                },
                Icon { name: IconName::Send, size: 20 }
            }
        }
    }
}
