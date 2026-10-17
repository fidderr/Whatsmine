use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ToastLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ToastMessage {
    pub message: String,
    pub level: ToastLevel,
}

/// Bottom toast; dismisses itself after a timeout based on level, or on tap.
#[component]
pub fn Toast(msg: ToastMessage, on_dismiss: EventHandler<()>) -> Element {
    let level_class = match msg.level {
        ToastLevel::Info => "toast-info",
        ToastLevel::Warning => "toast-warning",
        ToastLevel::Error => "toast-error",
    };
    let timeout_ms = match msg.level {
        ToastLevel::Info => 3000u64,
        ToastLevel::Warning => 5000,
        ToastLevel::Error => 10000,
    };

    // Restart the timer when a new message replaces the current one.
    let text = msg.message.clone();
    use_effect(use_reactive!(|text| {
        let _ = &text;
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)).await;
            on_dismiss.call(());
        });
    }));

    rsx! {
        div { class: "toast-container",
            div {
                class: "toast {level_class}",
                onclick: move |_| on_dismiss.call(()),
                "{msg.message}"
            }
        }
    }
}
