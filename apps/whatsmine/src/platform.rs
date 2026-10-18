//! Small platform specifics.

/// CSS variables for the space the system bars take. Android draws the app
/// edge-to-edge (under the status and navigation bars) and the WebView
/// reports `env(safe-area-inset-*)` as 0, so ask the window for its real
/// insets (the same way AlarmFree does). Elsewhere CSS `env()` works.
pub fn safe_area_style() -> String {
    #[cfg(target_os = "android")]
    if let Some(i) = mobile_sentinel::display::safe_area_insets() {
        return format!(
            "--safe-top: {}px; --safe-bottom: {}px; --safe-left: {}px; --safe-right: {}px;",
            i.top.round() as u32,
            i.bottom.round() as u32,
            i.left.round() as u32,
            i.right.round() as u32
        );
    }
    String::new()
}

/// Dark status/navigation bar icons (`true`, for a light app) or light ones.
pub fn set_light_system_bars(light: bool) {
    #[cfg(target_os = "android")]
    mobile_sentinel::display::set_light_system_bars(light);
    #[cfg(not(target_os = "android"))]
    let _ = light;
}
