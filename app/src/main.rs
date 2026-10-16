//! WhatsMine: chat without servers, accounts or phone numbers.

fn main() {
    #[cfg(feature = "desktop")]
    {
        use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
        dioxus::LaunchBuilder::new()
            .with_cfg(
                Config::new().with_menu(None).with_window(
                    WindowBuilder::new()
                        .with_title("WhatsMine")
                        .with_inner_size(LogicalSize::new(440.0, 780.0))
                        .with_min_inner_size(LogicalSize::new(340.0, 480.0)),
                ),
            )
            .launch(whatsmine::app::App);
    }

    #[cfg(not(feature = "desktop"))]
    dioxus::launch(whatsmine::app::App);
}
