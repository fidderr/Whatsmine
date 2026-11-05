//! What every screen is built from, so they all look and behave the same.

use dioxus::prelude::*;
use whatsmine_core::Settings;

use crate::app::{Route, SettingsSig};
use crate::back::BackLayer;
use crate::components::{Icon, IconName};

/// Changes a setting: kept at once, and shown at once.
pub fn change(mut settings: Signal<Settings>, f: impl FnOnce(&mut Settings)) {
    let mut s = settings.peek().clone();
    f(&mut s);
    if let Ok(core) = crate::boot::core()
        && core.save_settings(&s).is_ok()
    {
        settings.set(s);
    }
}

pub fn use_settings() -> Signal<Settings> {
    use_context::<SettingsSig>().0
}

/// The big title of a tab.
#[component]
pub fn Title(title: String, #[props(default)] children: Element) -> Element {
    rsx! {
        div { class: "ttl",
            h1 { "{title}" }
            {children}
        }
    }
}

/// A screen you came to from somewhere: back, and its name.
#[component]
pub fn Header(title: String, #[props(default)] children: Element) -> Element {
    rsx! {
        header { class: "hdr",
            BackButton {}
            h1 { "{title}" }
            {children}
        }
    }
}

#[component]
pub fn BackButton() -> Element {
    let nav = use_navigator();
    rsx! {
        button {
            class: "ib back-btn",
            aria_label: "Back",
            onclick: move |_| nav.go_back(),
            Icon { name: IconName::ArrowLeft }
        }
    }
}

/// One line in a group: goes somewhere or does something.
#[component]
pub fn Row(
    title: String,
    #[props(default)] icon: Option<IconName>,
    #[props(default)] note: String,
    #[props(default)] value: String,
    #[props(default)] to: Option<Route>,
    #[props(default)] on_tap: Option<EventHandler<()>>,
    #[props(default)] danger: bool,
    #[props(default)] small: bool,
    /// For test scripts, and whoever needs to find it.
    #[props(default)]
    tag: String,
) -> Element {
    let class = format!(
        "row{}{}",
        if small { " sm" } else { "" },
        if danger { " danger" } else { "" }
    );
    let chevron = !danger && (to.is_some() || on_tap.is_some());
    let inner = rsx! {
        if let Some(name) = icon {
            span { class: "dim", style: "display:flex",
                Icon { name, size: 20 }
            }
        }
        span { class: "stack",
            span { class: "row-t", "{title}" }
            if !note.is_empty() {
                span { class: "sub", "{note}" }
            }
        }
        if !value.is_empty() {
            span { class: "val", "{value}" }
        }
        if chevron {
            span { class: "dim", style: "display:flex",
                Icon { name: IconName::ChevronRight, size: 16 }
            }
        }
    };
    match (to, on_tap) {
        (Some(to), _) => rsx! {
            Link { to, class, "data-t": "{tag}", {inner} }
        },
        (None, Some(on_tap)) => rsx! {
            button { class, "data-t": "{tag}", onclick: move |_| on_tap.call(()), {inner} }
        },
        (None, None) => rsx! {
            div { class: "{class} still", "data-t": "{tag}", {inner} }
        },
    }
}

/// One line with a switch.
#[component]
pub fn SwitchRow(
    title: String,
    #[props(default)] note: String,
    checked: bool,
    on_change: EventHandler<bool>,
    #[props(default)] small: bool,
    #[props(default)] tag: String,
) -> Element {
    rsx! {
        button {
            class: if small { "row sm" } else { "row" },
            role: "switch",
            aria_checked: "{checked}",
            "data-t": "{tag}",
            onclick: move |_| on_change.call(!checked),
            span { class: "stack",
                span { class: "row-t", "{title}" }
                if !note.is_empty() {
                    span { class: "sub", "{note}" }
                }
            }
            span { class: if checked { "sw on" } else { "sw" },
                span {}
            }
        }
    }
}

/// Choose one of a few.
#[component]
pub fn Chips(
    options: Vec<(String, String)>,
    value: String,
    on_change: EventHandler<String>,
) -> Element {
    rsx! {
        div { class: "chips", role: "radiogroup",
            for (key, label) in options {
                button {
                    key: "{key}",
                    class: if key == value { "chip on" } else { "chip" },
                    role: "radio",
                    aria_checked: "{key == value}",
                    onclick: move |_| on_change.call(key.clone()),
                    "{label}"
                }
            }
        }
    }
}

/// What slides up from the bottom. Back and a tap beside it close it.
#[component]
pub fn Sheet(
    on_close: EventHandler<()>,
    #[props(default)] title: String,
    #[props(default)] tag: String,
    children: Element,
) -> Element {
    rsx! {
        div { class: "scrim", onclick: move |_| on_close.call(()),
            BackLayer { on_back: move |_| on_close.call(()) }
            div {
                class: "sheet",
                role: "dialog",
                "data-t": "{tag}",
                onclick: move |e| e.stop_propagation(),
                span { class: "grab", aria_hidden: "true" }
                if !title.is_empty() {
                    h2 { class: "one-line", "{title}" }
                }
                {children}
            }
        }
    }
}

/// One thing a sheet offers.
#[component]
pub fn SheetItem(
    label: String,
    icon: IconName,
    on_tap: EventHandler<()>,
    #[props(default)] danger: bool,
    #[props(default)] tag: String,
) -> Element {
    rsx! {
        button {
            class: if danger { "sheet-item danger" } else { "sheet-item" },
            "data-t": "{tag}",
            onclick: move |_| on_tap.call(()),
            Icon { name: icon }
            span { "{label}" }
        }
    }
}

/// One line that asks for attention, and usually goes somewhere.
#[component]
pub fn Notice(
    text: String,
    icon: IconName,
    #[props(default)] warn: bool,
    #[props(default)] to: Option<Route>,
    #[props(default)] tag: String,
) -> Element {
    let class = if warn { "notice warn" } else { "notice" };
    let inner = rsx! {
        Icon { name: icon }
        span { class: "stack b", "{text}" }
        if to.is_some() {
            Icon { name: IconName::ChevronRight, size: 16 }
        }
    };
    match to {
        Some(to) => rsx! {
            Link { to, class, "data-t": "{tag}", {inner} }
        },
        None => rsx! {
            div { class: "{class} still", "data-t": "{tag}", {inner} }
        },
    }
}
