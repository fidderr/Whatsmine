use dioxus::prelude::*;

#[component]
pub fn ToggleRow(
    label: String,
    #[props(default)] hint: String,
    checked: bool,
    on_change: EventHandler<bool>,
) -> Element {
    rsx! {
        label { class: "toggle-row",
            div { class: "toggle-text",
                span { "{label}" }
                if !hint.is_empty() {
                    span { class: "setting-hint", "{hint}" }
                }
            }
            span { class: "toggle-switch",
                input {
                    r#type: "checkbox",
                    checked,
                    onchange: move |e| on_change.call(e.checked()),
                }
                span { class: "toggle-slider" }
            }
        }
    }
}
