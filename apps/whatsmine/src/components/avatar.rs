use dioxus::prelude::*;

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
) -> Element {
    let mut broken = use_signal(|| false);
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
    rsx! {
        div {
            class: "avatar {size}",
            style: "background: hsl({hue}, 50%, 42%);",
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
    }
}
