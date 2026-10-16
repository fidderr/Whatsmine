use dioxus::prelude::*;

use crate::app::{SettingsSig, t, use_core, use_tr};
use crate::components::{Icon, IconName};

/// First run: pick a name, learn that the key is your "number".
#[component]
pub fn Onboarding() -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut settings = use_context::<SettingsSig>().0;
    let mut name = use_signal(String::new);

    let finish = use_callback(move |()| {
        let mut s = settings.peek().clone();
        s.display_name = name.peek().trim().chars().take(64).collect();
        s.onboarded = true;
        if core.save_settings(&s).is_ok() {
            settings.set(s);
        }
    });

    rsx! {
        div { class: "onboarding",
            div {
                div { class: "brand", "Whats", span { "Mine" } }
                p { class: "tagline", "{t(&tr, \"onboarding.tagline\")}" }
            }
            div { class: "points",
                div { class: "point",
                    Icon { name: IconName::Server }
                    span { "{t(&tr, \"onboarding.no_servers\")}" }
                }
                div { class: "point",
                    Icon { name: IconName::Key }
                    span { "{t(&tr, \"onboarding.key\")}" }
                }
                div { class: "point",
                    Icon { name: IconName::Lock }
                    span { "{t(&tr, \"onboarding.encrypted\")}" }
                }
                div { class: "point",
                    Icon { name: IconName::Shield }
                    span { "{t(&tr, \"onboarding.yours\")}" }
                }
            }
            div {
                label { class: "section-label", "{t(&tr, \"onboarding.name_label\")}" }
                input {
                    r#type: "text",
                    maxlength: "64",
                    placeholder: "{t(&tr, \"onboarding.name_placeholder\")}",
                    value: "{name}",
                    autofocus: true,
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter && !name.peek().trim().is_empty() {
                            finish.call(());
                        }
                    },
                }
                p { class: "setting-hint", "{t(&tr, \"onboarding.name_hint\")}" }
            }
            button {
                class: "save-btn",
                disabled: name().trim().is_empty(),
                onclick: move |_| finish.call(()),
                "{t(&tr, \"onboarding.start\")}"
            }
        }
    }
}
