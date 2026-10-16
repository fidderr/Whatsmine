use dioxus::prelude::*;

/// Round avatar: the first letter of the name on a colour derived from the
/// contact's key, so the same person always gets the same colour.
#[component]
pub fn Avatar(
    id: String,
    name: String,
    #[props(default)] size: String,
    #[props(default)] online: bool,
) -> Element {
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".into());
    let hue = id
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32))
        % 360;
    rsx! {
        div {
            class: "avatar {size}",
            style: "background: hsl({hue}, 50%, 42%);",
            "{initial}"
            if online {
                span { class: "online-dot" }
            }
        }
    }
}
