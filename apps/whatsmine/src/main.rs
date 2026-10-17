//! WhatsMine: chat without servers, accounts or phone numbers.

/// WebView2 flags (Windows). The page is our own `http://dioxus.index.html`,
/// which the browser engine does not call a "secure context", so it would
/// hide the camera from it. We declare that one origin trusted, and skip the
/// per-use camera prompt: the page only ever runs our own code, and the user
/// started the camera by tapping the button.
#[cfg(all(feature = "desktop", windows))]
const WEBVIEW2_ARGS: &str = "--unsafely-treat-insecure-origin-as-secure=http://dioxus.index.html --use-fake-ui-for-media-stream";

fn main() {
    #[cfg(feature = "desktop")]
    {
        use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
        let config = Config::new().with_menu(None).with_window(
            WindowBuilder::new()
                .with_title("WhatsMine")
                .with_inner_size(LogicalSize::new(440.0, 780.0))
                .with_min_inner_size(LogicalSize::new(340.0, 480.0)),
        );
        let config = match dioxus::desktop::icon_from_memory(include_bytes!("../assets/icon.png")) {
            Ok(icon) => config.with_icon(icon),
            Err(_) => config,
        };
        #[cfg(windows)]
        let config = {
            // Keep honouring the standard WebView2 variable (used for debugging).
            let extra = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
            config.with_windows_browser_args(format!("{WEBVIEW2_ARGS} {extra}"))
        };
        dioxus::LaunchBuilder::new()
            .with_cfg(config)
            .launch(whatsmine::app::App);
    }

    #[cfg(not(feature = "desktop"))]
    dioxus::launch(whatsmine::app::App);
}
