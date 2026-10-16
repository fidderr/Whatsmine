use dioxus::prelude::*;

use crate::components::{Icon, IconName};

/// AlarmFree-style dropdown: a trigger button and a menu of options.
/// `options` are `(value, label)` pairs.
#[component]
pub fn Dropdown(
    value: String,
    options: Vec<(String, String)>,
    on_change: EventHandler<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let current = options
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, l)| l.clone())
        .unwrap_or_default();
    rsx! {
        div { class: "custom-dropdown",
            button {
                class: "dropdown-trigger",
                onclick: move |_| open.toggle(),
                span { "{current}" }
                Icon { name: IconName::ChevronDown, size: 16 }
            }
            if open() {
                div { class: "dropdown-menu",
                    for (v, label) in options.iter().cloned() {
                        button {
                            key: "{v}",
                            class: if v == value { "dropdown-item active" } else { "dropdown-item" },
                            onclick: move |_| {
                                open.set(false);
                                on_change.call(v.clone());
                            },
                            "{label}"
                        }
                    }
                }
            }
        }
    }
}
