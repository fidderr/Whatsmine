//! Small bridges to the WebView for things Rust cannot do directly.
//! Values are passed with `serde_json` so nothing is spliced into JS source.

use dioxus::prelude::*;

/// Copies text to the clipboard. Uses the async Clipboard API when allowed,
/// falling back to a hidden textarea + execCommand (older WebViews).
pub fn copy_text(text: &str) {
    let arg = serde_json::to_string(text).unwrap_or_default();
    document::eval(&format!(
        r#"
        const text = {arg};
        const fallback = () => {{
            const ta = document.createElement('textarea');
            ta.value = text;
            ta.style.position = 'fixed';
            ta.style.opacity = '0';
            document.body.appendChild(ta);
            ta.select();
            try {{ document.execCommand('copy'); }} catch (e) {{}}
            ta.remove();
        }};
        if (navigator.clipboard && window.isSecureContext) {{
            navigator.clipboard.writeText(text).catch(fallback);
        }} else {{
            fallback();
        }}
        "#
    ));
}

/// Reads text from the clipboard, if the platform allows it.
pub async fn paste_text() -> Option<String> {
    let mut ev = document::eval(
        r#"
        try {
            const t = await navigator.clipboard.readText();
            dioxus.send(t || "");
        } catch (e) {
            dioxus.send("");
        }
        "#,
    );
    ev.recv::<String>().await.ok().filter(|s| !s.is_empty())
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
