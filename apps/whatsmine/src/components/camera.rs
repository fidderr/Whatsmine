//! The in-app camera, for platforms without a system camera app (Windows).
//!
//! The page asks the WebView for the webcam (and microphone for video),
//! shows the live picture, and records with the browser's own encoders. The
//! finished photo or video is posted to the `capture` handler (see
//! `media_server`), which stores it and answers with its path.

use std::time::Duration;

use dioxus::prelude::*;

use crate::app::{t, use_tr};
use crate::back::BackLayer;
use crate::components::{Icon, IconName};
use crate::fmt;
use crate::media::Picked;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CameraMode {
    Photo,
    Video,
}

#[derive(Clone, PartialEq)]
enum Cam {
    Starting,
    Ready,
    Recording,
    Saving,
    Failed,
}

fn script(mode: CameraMode) -> String {
    let mode = match mode {
        CameraMode::Photo => "photo",
        CameraMode::Video => "video",
    };
    format!(
        r#"
        let video = null;
        for (let i = 0; i < 60 && !(video = document.getElementById('cam-video')); i++) {{
            await new Promise(r => setTimeout(r, 50));
        }}
        const mode = '{mode}';
        let stream = null, rec = null, chunks = [];
        try {{
            stream = await navigator.mediaDevices.getUserMedia({{
                video: {{ width: {{ ideal: 1280 }}, height: {{ ideal: 720 }} }},
                audio: mode === 'video'
            }});
        }} catch (e) {{
            dioxus.send('error:' + ((e && e.name) || 'unknown'));
            return;
        }}
        video.srcObject = stream;
        try {{ await video.play(); }} catch (e) {{}}
        dioxus.send('ready');
        const upload = async (blob, ext) => {{
            const r = await fetch('/capture/' + ext, {{ method: 'POST', body: blob }});
            if (!r.ok) throw new Error('upload ' + r.status);
            return await r.text();
        }};
        while (true) {{
            const cmd = await dioxus.recv();
            try {{
                if (cmd === 'snap') {{
                    const c = document.createElement('canvas');
                    c.width = video.videoWidth;
                    c.height = video.videoHeight;
                    c.getContext('2d').drawImage(video, 0, 0);
                    const blob = await new Promise(res => c.toBlob(res, 'image/jpeg', 0.92));
                    dioxus.send('done:' + await upload(blob, 'jpg'));
                }} else if (cmd === 'rec-start') {{
                    chunks = [];
                    const type = ['video/webm;codecs=vp8,opus', 'video/webm']
                        .find(t => MediaRecorder.isTypeSupported(t)) || '';
                    rec = new MediaRecorder(stream, type ? {{ mimeType: type }} : undefined);
                    rec.ondataavailable = e => {{ if (e.data && e.data.size) chunks.push(e.data); }};
                    rec.onstop = async () => {{
                        try {{
                            dioxus.send('done:' + await upload(new Blob(chunks, {{ type: 'video/webm' }}), 'webm'));
                        }} catch (e) {{ dioxus.send('error:upload'); }}
                    }};
                    rec.start(500);
                    dioxus.send('recording');
                }} else if (cmd === 'rec-stop') {{
                    rec.stop();
                }} else if (cmd === 'close') {{
                    break;
                }}
            }} catch (e) {{
                dioxus.send('error:' + ((e && e.message) || 'failed'));
            }}
        }}
        if (rec && rec.state !== 'inactive') {{ try {{ rec.stop(); }} catch (e) {{}} }}
        stream.getTracks().forEach(t => t.stop());
        video.srcObject = null;
        "#
    )
}

/// Full-screen camera. Calls `on_done` with the captured file, or `None`
/// when closed without capturing.
#[component]
pub fn CameraOverlay(mode: CameraMode, on_done: EventHandler<Option<Picked>>) -> Element {
    let tr = use_tr();
    let mut state = use_signal(|| Cam::Starting);
    let mut seconds = use_signal(|| 0u64);
    let page = use_hook(move || document::eval(&script(mode)));

    use_future(move || async move {
        let mut page = page;
        while let Ok(msg) = page.recv::<String>().await {
            if msg == "ready" {
                state.set(Cam::Ready);
            } else if msg == "recording" {
                seconds.set(0);
                state.set(Cam::Recording);
            } else if let Some(path) = msg.strip_prefix("done:") {
                let _ = page.send("close");
                let (name, mime) = match mode {
                    CameraMode::Photo => ("photo.jpg", "image/jpeg"),
                    CameraMode::Video => ("video.webm", "video/webm"),
                };
                on_done.call(Some(Picked {
                    path: path.into(),
                    name: name.into(),
                    mime: mime.into(),
                    duration_ms: matches!(mode, CameraMode::Video).then(|| *seconds.peek() * 1000),
                }));
                return;
            } else if msg.starts_with("error:") {
                state.set(Cam::Failed);
            }
        }
    });
    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if *state.peek() == Cam::Recording {
                *seconds.write() += 1;
            }
        }
    });
    // However the screen goes away, release the camera.
    use_drop(move || {
        let _ = page.send("close");
    });

    let close = move |_: ()| {
        let _ = page.send("close");
        on_done.call(None);
    };
    rsx! {
        div { class: "camera",
            BackLayer { on_back: move |_| close(()) }
            div { class: "camera-bar",
                button { class: "back-btn", onclick: move |_| close(()), Icon { name: IconName::Xmark } }
                if state() == Cam::Recording {
                    span { class: "rec-dot" }
                    span { class: "rec-time", "{fmt::duration(seconds() * 1000)}" }
                }
            }
            video {
                id: "cam-video",
                class: "camera-video",
                autoplay: true,
                muted: true,
                playsinline: true,
            }
            match state() {
                Cam::Starting => rsx! {
                    div { class: "camera-note", "{t(&tr, \"camera.starting\")}" }
                },
                Cam::Failed => rsx! {
                    div { class: "camera-note error", "{t(&tr, \"camera.error\")}" }
                },
                Cam::Saving => rsx! {
                    div { class: "camera-note", "{t(&tr, \"camera.saving\")}" }
                },
                Cam::Ready | Cam::Recording => rsx! {},
            }
            div { class: "camera-controls",
                if mode == CameraMode::Photo {
                    button {
                        class: "shutter",
                        disabled: state() != Cam::Ready,
                        title: "{t(&tr, \"camera.snap\")}",
                        onclick: move |_| {
                            state.set(Cam::Saving);
                            let _ = page.send("snap");
                        },
                    }
                } else if state() == Cam::Recording {
                    button {
                        class: "shutter recording",
                        title: "{t(&tr, \"camera.stop\")}",
                        onclick: move |_| {
                            state.set(Cam::Saving);
                            let _ = page.send("rec-stop");
                        },
                    }
                } else {
                    button {
                        class: "shutter video",
                        disabled: state() != Cam::Ready,
                        title: "{t(&tr, \"camera.record\")}",
                        onclick: move |_| {
                            let _ = page.send("rec-start");
                        },
                    }
                }
            }
        }
    }
}
