//! Small bridges to the WebView for things Rust cannot do directly.
//! Values are passed with `serde_json` so nothing is spliced into JS source.

use dioxus::prelude::*;

/// Copies text to the system clipboard. Done natively: the WebView's
/// clipboard API is blocked on the app's (non-https) origin.
pub fn copy_text(text: &str) {
    crate::clipboard::set(text);
}

/// Reads text from the system clipboard, if any.
pub async fn paste_text() -> Option<String> {
    crate::clipboard::get().filter(|s| !s.trim().is_empty())
}

/// Scrolls an element to its end after the next paint.
pub fn scroll_to_bottom(id: &str) {
    let arg = serde_json::to_string(id).unwrap_or_default();
    document::eval(&format!(
        "requestAnimationFrame(() => {{ const el = document.getElementById({arg}); if (el) el.scrollTop = el.scrollHeight; }});"
    ));
}

/// Screens start at the top; the router keeps the previous page's scroll.
pub fn use_scroll_top() {
    dioxus::prelude::use_hook(|| {
        document::eval("window.scrollTo(0, 0);");
    });
}

/// Moves keyboard focus to an element.
pub fn focus(id: &str) {
    let arg = serde_json::to_string(id).unwrap_or_default();
    document::eval(&format!(
        "requestAnimationFrame(() => {{ const el = document.getElementById({arg}); if (el) el.focus(); }});"
    ));
}

/// Scrolls to the end only if the reader is already near it, so a photo
/// finishing loading does not yank someone who scrolled up.
pub fn scroll_if_near_bottom(id: &str) {
    let arg = serde_json::to_string(id).unwrap_or_default();
    document::eval(&format!(
        "requestAnimationFrame(() => {{ const el = document.getElementById({arg}); if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 400) el.scrollTop = el.scrollHeight; }});"
    ));
}
