use dioxus::prelude::*;

use crate::back::BackLayer;

/// A cache-busting version for a picture file: its modification time.
pub fn picture_version(path: &str) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Round avatar: the contact's picture when they have one, else the first
/// letter of the name on a colour derived from their key, so the same person
/// always gets the same colour. `image` is the local picture file (only used
/// to know whether there is one and when it changed); `id` is a contact id,
/// or `"me"` for our own picture.
#[component]
pub fn Avatar(
    id: String,
    name: String,
    #[props(default)] size: String,
    #[props(default)] online: bool,
    #[props(default)] image: Option<String>,
    #[props(default)] group: bool,
    /// Tapping the picture shows it large.
    #[props(default)]
    zoom: bool,
) -> Element {
    let mut broken = use_signal(|| false);
    let mut open = use_signal(|| false);
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".into());
    let hue = id
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32))
        % 360;
    let src = image
        .as_deref()
        .map(|p| format!("/avatar/{id}?v={}", picture_version(p)));
    let big_src = src.clone().filter(|_| !broken());
    let big_name = name.clone();
    rsx! {
        div {
            class: if zoom { "avatar {size} zoomable" } else { "avatar {size}" },
            style: "background: hsl({hue}, 50%, 42%);",
            onclick: move |e| {
                if zoom {
                    // Not the row's tap, and not a link's navigation.
                    e.stop_propagation();
                    e.prevent_default();
                    open.set(true);
                }
            },
            if group {
                super::Icon { name: super::IconName::Users, size: if size == "large" { 44 } else { 22 } }
            } else {
                "{initial}"
            }
            if let (Some(src), false) = (src, broken()) {
                img {
                    class: "avatar-img",
                    src: "{src}",
                    alt: "",
                    onerror: move |_| broken.set(true),
                }
            }
            if online {
                span { class: "online-dot" }
            }
        }
        if open() {
            div {
                class: "viewer",
                onclick: move |e| {
                    e.stop_propagation();
                    open.set(false);
                },
                BackLayer { on_back: move |_| open.set(false) }
                div { class: "viewer-bar",
                    button {
                        class: "back-btn",
                        onclick: move |e| {
                            e.stop_propagation();
                            open.set(false);
                        },
                        super::Icon { name: super::IconName::Xmark }
                    }
                    span { class: "viewer-name", "{big_name}" }
                }
                if let Some(src) = big_src {
                    img { class: "viewer-img avatar-big", src: "{src}", alt: "{big_name}" }
                } else {
                    div {
                        class: "avatar-big placeholder",
                        style: "background: hsl({hue}, 50%, 42%);",
                        if group {
                            super::Icon { name: super::IconName::Users, size: 96 }
                        } else {
                            "{initial}"
                        }
                    }
                }
            }
        }
    }
}
