//! Choosing a colour: a wheel with brightness, a code, soft colours to start from, and what was
//! used lately. The sliders also serve keyboards and screen readers, which cannot use a wheel.

use dioxus::prelude::*;
use whatsmine_core::look::{Rgb, SOFT};

use crate::app::{t, use_tr};

/// The wheel's size in pixels.
const WHEEL: f64 = 208.0;

fn soft_name(tr: &crate::app::Translations, family: &str, tint: usize) -> String {
    let tint = t(tr, ["soft.soft", "soft.mid", "soft.deep"][tint.min(2)]);
    t(tr, "soft.name")
        .replace("{tint}", &tint)
        .replace("{family}", &t(tr, &format!("soft.{family}")))
}

#[component]
pub fn ColorPicker(
    value: Rgb,
    #[props(default)] recent: Vec<Rgb>,
    on_change: EventHandler<Rgb>,
) -> Element {
    let tr = use_tr();
    let mut dragging = use_signal(|| false);
    // Hue and softness stay as they were when brightness goes to black or softness to white,
    // where the colour itself no longer says.
    let mut held = use_signal(|| value.to_hsv());
    let (hue, saturation, brightness) = {
        let (h, s, v) = value.to_hsv();
        let kept = held();
        let same = Rgb::from_hsv(kept.0, kept.1, kept.2) == value;
        if same { kept } else { (h, s, v) }
    };
    let code = value.hex();
    let angle = f64::from(hue).to_radians();
    let radius = f64::from(saturation) * 50.0;
    let thumb_x = 50.0 + radius * angle.sin();
    let thumb_y = 50.0 - radius * angle.cos();
    let dark = 1.0 - brightness;

    let mut pick = move |h: f32, s: f32, v: f32| {
        held.set((h, s, v));
        on_change.call(Rgb::from_hsv(h, s, v));
    };
    let mut at_point = move |x: f64, y: f64, v: f32| {
        let (dx, dy) = (x - WHEEL / 2.0, y - WHEEL / 2.0);
        let distance = (dx * dx + dy * dy).sqrt() / (WHEEL / 2.0);
        let h = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
        pick(h as f32, distance.min(1.0) as f32, v);
    };
    // A finger that leaves the wheel still moves the same thumb.
    use_effect(|| {
        document::eval(
            "const w = document.getElementById('color-wheel');\n\
             if (w) w.addEventListener('pointerdown', e => w.setPointerCapture(e.pointerId));",
        );
    });

    rsx! {
        div { class: "picker",
            div {
                id: "color-wheel",
                class: "wheel",
                role: "img",
                aria_label: "{t(&tr, \"look.wheel\")}",
                "data-t": "color-wheel",
                onpointerdown: move |e| {
                    dragging.set(true);
                    let p = e.element_coordinates();
                    at_point(p.x, p.y, brightness);
                },
                onpointermove: move |e| {
                    if dragging() {
                        let p = e.element_coordinates();
                        at_point(p.x, p.y, brightness);
                    }
                },
                onpointerup: move |_| dragging.set(false),
                onpointercancel: move |_| dragging.set(false),
                span { class: "wheel-dark", style: "opacity:{dark}" }
                span { class: "wheel-thumb", style: "left:{thumb_x}%;top:{thumb_y}%;background:{code}" }
            }
            label { class: "picker-line",
                span { class: "sub", "{t(&tr, \"look.brightness\")}" }
                input {
                    r#type: "range",
                    min: "0",
                    max: "100",
                    "data-t": "color-brightness",
                    value: "{(brightness * 100.0).round()}",
                    oninput: move |e| {
                        let v = e.value().parse::<f32>().unwrap_or(100.0) / 100.0;
                        pick(hue, saturation, v);
                    },
                }
            }
            // For who cannot use the wheel.
            label { class: "unseen",
                "{t(&tr, \"look.hue\")}"
                input {
                    r#type: "range",
                    min: "0",
                    max: "360",
                    value: "{hue.round()}",
                    oninput: move |e| {
                        let h = e.value().parse::<f32>().unwrap_or(0.0);
                        pick(h, saturation, brightness);
                    },
                }
            }
            label { class: "unseen",
                "{t(&tr, \"look.softness\")}"
                input {
                    r#type: "range",
                    min: "0",
                    max: "100",
                    value: "{(saturation * 100.0).round()}",
                    oninput: move |e| {
                        let s = e.value().parse::<f32>().unwrap_or(0.0) / 100.0;
                        pick(hue, s, brightness);
                    },
                }
            }
            div { class: "picker-code",
                span { class: "picker-now", style: "background:{code}", aria_hidden: "true" }
                label { class: "pill",
                    span { class: "sub", "{t(&tr, \"look.code\")}" }
                    input {
                        "data-t": "color-code",
                        value: "{code}",
                        maxlength: "7",
                        oninput: move |e| {
                            if let Some(color) = Rgb::parse(&e.value()) {
                                held.set(color.to_hsv());
                                on_change.call(color);
                            }
                        },
                    }
                }
            }
            h3 { class: "gt", "{t(&tr, \"look.soft\")}" }
            div { class: "soft-colors",
                for tint in 0..3 {
                    for (family, tints) in SOFT {
                        {
                            let color = tints[tint];
                            let name = soft_name(&tr, family, tint);
                            let on = color == value;
                            rsx! {
                                button {
                                    key: "{family}{tint}",
                                    class: if on { "soft on" } else { "soft" },
                                    style: "background:{color.hex()}",
                                    aria_label: "{name}",
                                    aria_pressed: "{on}",
                                    "data-t": "soft-{family}-{tint}",
                                    onclick: move |_| {
                                        held.set(color.to_hsv());
                                        on_change.call(color);
                                    },
                                }
                            }
                        }
                    }
                }
            }
            if !recent.is_empty() {
                h3 { class: "gt", "{t(&tr, \"look.recent\")}" }
                div { class: "swatches",
                    for color in recent {
                        button {
                            key: "{color.hex()}",
                            class: "soft",
                            style: "background:{color.hex()}",
                            aria_label: "{color.hex()}",
                            onclick: move |_| {
                                held.set(color.to_hsv());
                                on_change.call(color);
                            },
                        }
                    }
                }
            }
        }
    }
}
