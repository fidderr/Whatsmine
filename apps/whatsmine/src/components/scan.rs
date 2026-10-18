//! Reading a QR code with the camera.
//!
//! Android has a scanner of its own (a system-style screen from
//! mobile-sentinel). Elsewhere the WebView opens the webcam and the page
//! looks for a code in every frame (jsQR, bundled).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dioxus::prelude::*;

use crate::app::{t, use_tr};
use crate::back::BackLayer;
use crate::components::{Icon, IconName};

const JSQR: &str = include_str!("../../assets/vendor/jsQR.js");

/// True where the system provides the scanner screen.
pub const NATIVE_SCANNER: bool = cfg!(target_os = "android");

/// Opens the system scanner and waits for a code. `None` if cancelled.
pub async fn scan_native() -> Option<String> {
    crate::media::blocking(|| mobile_sentinel::scanner::scan().ok())
        .await
        .flatten()
}

static LOADED: AtomicBool = AtomicBool::new(false);

fn script() -> String {
    // The library goes into the page once per run of the app.
    let lib = if LOADED.swap(true, Ordering::Relaxed) {
        String::new()
    } else {
        format!(
            "window.__jsqr = (function () {{ const module = {{ exports: {{}} }}; const exports = module.exports; {JSQR}\n return module.exports; }})();"
        )
    };
    format!(
        r#"
        {lib}
        let video = null;
        for (let i = 0; i < 60 && !(video = document.getElementById('scan-video')); i++) {{
            await new Promise(r => setTimeout(r, 50));
        }}
        let stream = null;
        try {{
            stream = await navigator.mediaDevices.getUserMedia({{
                video: {{ facingMode: 'environment', width: {{ ideal: 1280 }}, height: {{ ideal: 720 }} }}
            }});
        }} catch (e) {{
            dioxus.send('error:' + ((e && e.name) || 'unknown'));
            return;
        }}
        video.srcObject = stream;
        try {{ await video.play(); }} catch (e) {{}}
        dioxus.send('ready');
        const canvas = document.createElement('canvas');
        const ctx = canvas.getContext('2d', {{ willReadFrequently: true }});
        let open = true;
        dioxus.recv().then(() => {{ open = false; }});
        while (open) {{
            await new Promise(r => setTimeout(r, 150));
            if (video.readyState < 2 || !video.videoWidth) continue;
            const scale = Math.min(1, 720 / video.videoWidth);
            canvas.width = Math.round(video.videoWidth * scale);
            canvas.height = Math.round(video.videoHeight * scale);
            ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
            const img = ctx.getImageData(0, 0, canvas.width, canvas.height);
            const code = window.__jsqr(img.data, canvas.width, canvas.height, {{ inversionAttempts: 'attemptBoth' }});
            if (code && code.data) {{
                dioxus.send('code:' + code.data);
                break;
            }}
        }}
        stream.getTracks().forEach(t => t.stop());
        video.srcObject = null;
        "#
    )
}

/// Full-screen webcam view that reports the first QR code it sees.
#[component]
pub fn ScanOverlay(on_code: EventHandler<String>, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let mut failed = use_signal(|| false);
    let mut ready = use_signal(|| false);
    let page = use_hook(move || document::eval(&script()));

    use_future(move || async move {
        let mut page = page;
        while let Ok(msg) = page.recv::<String>().await {
            if msg == "ready" {
                ready.set(true);
            } else if let Some(code) = msg.strip_prefix("code:") {
                on_code.call(code.to_string());
                return;
            } else if msg.starts_with("error:") {
                failed.set(true);
            }
        }
    });
    // However the screen goes away, release the camera.
    use_drop(move || {
        let _ = page.send("close");
    });
    use_future(move || async move {
        // Give up politely if the camera never comes up.
        tokio::time::sleep(Duration::from_secs(20)).await;
        if !*ready.peek() {
            failed.set(true);
        }
    });

    rsx! {
        div { class: "camera scanner",
            BackLayer { on_back: move |_| on_close.call(()) }
            div { class: "camera-bar",
                button { class: "back-btn", onclick: move |_| on_close.call(()),
                    Icon { name: IconName::Xmark }
                }
                span { class: "viewer-name", "{t(&tr, \"add.scan_title\")}" }
            }
            video {
                id: "scan-video",
                class: "camera-video",
                autoplay: true,
                muted: true,
                playsinline: true,
            }
            div { class: "scan-frame" }
            if failed() {
                div { class: "camera-note error", "{t(&tr, \"camera.error\")}" }
            } else if !ready() {
                div { class: "camera-note", "{t(&tr, \"camera.starting\")}" }
            } else {
                div { class: "scan-hint", "{t(&tr, \"add.scan_hint\")}" }
            }
        }
    }
}
