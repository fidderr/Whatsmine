//! A voice message: play, how far along, how fast. The bars are the sound itself, worked out from
//! the file on this device.

use dioxus::prelude::*;

use crate::components::{Icon, IconName};
use crate::fmt;

const SPEEDS: [(&str, f64); 3] = [("1x", 1.0), ("1.5x", 1.5), ("2x", 2.0)];

#[component]
pub fn VoicePlayer(
    id: String,
    src: String,
    #[props(default)] duration_ms: Option<u64>,
    on_broken: EventHandler<()>,
) -> Element {
    let dom = format!("voice-{id}");
    let mut playing = use_signal(|| false);
    let mut speed = use_signal(|| 0usize);
    {
        let dom = dom.clone();
        use_future(move || {
            let dom = dom.clone();
            async move {
                let mut eval = document::eval(&format!(
                    "const a = document.getElementById('{dom}');\n\
                     const wave = document.getElementById('{dom}-wave');\n\
                     if (a && wave) {{\n\
                       const paint = () => {{\n\
                         const done = a.duration ? a.currentTime / a.duration : 0;\n\
                         [...wave.children].forEach((b, i) => {{\n\
                           b.classList.toggle('on', (i + 0.5) / wave.children.length <= done);\n\
                         }});\n\
                       }};\n\
                       a.addEventListener('timeupdate', paint);\n\
                       a.addEventListener('play', () => dioxus.send(true));\n\
                       a.addEventListener('pause', () => dioxus.send(false));\n\
                       a.addEventListener('ended', () => {{ a.currentTime = 0; paint(); dioxus.send(false); }});\n\
                       wave.addEventListener('click', e => {{\n\
                         const r = wave.getBoundingClientRect();\n\
                         if (a.duration) a.currentTime = a.duration * (e.clientX - r.left) / r.width;\n\
                         paint();\n\
                       }});\n\
                       try {{\n\
                         const AC = window.AudioContext || window.webkitAudioContext;\n\
                         const data = await (await fetch(a.currentSrc || a.src)).arrayBuffer();\n\
                         const ctx = new AC();\n\
                         const sound = await ctx.decodeAudioData(data);\n\
                         ctx.close();\n\
                         const samples = sound.getChannelData(0);\n\
                         const bars = wave.children.length;\n\
                         const step = Math.max(1, Math.floor(samples.length / bars));\n\
                         const peaks = [];\n\
                         for (let i = 0; i < bars; i++) {{\n\
                           let peak = 0;\n\
                           for (let j = i * step; j < Math.min((i + 1) * step, samples.length); j += 64) {{\n\
                             peak = Math.max(peak, Math.abs(samples[j]));\n\
                           }}\n\
                           peaks.push(peak);\n\
                         }}\n\
                         const top = Math.max(...peaks, 0.01);\n\
                         peaks.forEach((p, i) => {{\n\
                           wave.children[i].style.height = Math.max(12, Math.round(p / top * 100)) + '%';\n\
                         }});\n\
                       }} catch (e) {{}}\n\
                     }}\n\
                     await new Promise(() => {{}});"
                ));
                while let Ok(now) = eval.recv::<bool>().await {
                    playing.set(now);
                }
            }
        });
    }
    let (toggle_dom, speed_dom) = (dom.clone(), dom.clone());
    let (label, _) = SPEEDS[speed()];
    let length = duration_ms.map(fmt::duration).unwrap_or_default();
    rsx! {
        div { class: "voice-player", "data-t": "voice-player",
            audio {
                id: "{dom}",
                src: "{src}",
                preload: "metadata",
                onerror: move |_| on_broken.call(()),
            }
            button {
                class: "voice-play",
                "data-t": "voice-play",
                aria_label: "Play",
                onclick: move |_| {
                    document::eval(&format!(
                        "const a = document.getElementById('{toggle_dom}'); if (a) {{ if (a.paused) a.play(); else a.pause(); }}"
                    ));
                },
                Icon { name: if playing() { IconName::Stop } else { IconName::Send }, size: 20 }
            }
            div { class: "voice-side",
                div { class: "voice-wave", id: "{dom}-wave", aria_hidden: "true",
                    for i in 0..30 {
                        span { key: "{i}" }
                    }
                }
                span { class: "small dim", "{length}" }
            }
            button {
                class: "voice-speed",
                "data-t": "voice-speed",
                aria_label: "Speed",
                onclick: move |_| {
                    let next = (speed() + 1) % SPEEDS.len();
                    speed.set(next);
                    let rate = SPEEDS[next].1;
                    document::eval(&format!(
                        "const a = document.getElementById('{speed_dom}'); if (a) a.playbackRate = {rate};"
                    ));
                },
                "{label}"
            }
        }
    }
}
