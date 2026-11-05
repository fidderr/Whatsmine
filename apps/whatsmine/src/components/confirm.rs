use dioxus::prelude::*;

use crate::app::{t, use_tr};
use crate::back::BackLayer;

#[component]
pub fn ConfirmDialog(
    title: String,
    message: String,
    confirm_label: String,
    #[props(default = true)] danger: bool,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let tr = use_tr();
    let confirm_class = if danger { "btn danger" } else { "btn main" };
    rsx! {
        div {
            class: "scrim center",
            onclick: move |_| on_cancel.call(()),
            BackLayer { on_back: move |_| on_cancel.call(()) }
            div {
                class: "confirm-dialog",
                onclick: move |e| e.stop_propagation(),
                h3 { "{title}" }
                p { "{message}" }
                div { class: "btns",
                    button { class: "btn", onclick: move |_| on_cancel.call(()),
                        "{t(&tr, \"common.cancel\")}"
                    }
                    button { class: confirm_class, onclick: move |_| on_confirm.call(()),
                        "{confirm_label}"
                    }
                }
            }
        }
    }
}
