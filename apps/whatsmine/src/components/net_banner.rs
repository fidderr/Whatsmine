use dioxus::prelude::*;
use whatsmine_core::NetStatus;

use crate::app::{Net, t, use_tr};

/// A slim notice while we are not connected to the network.
#[component]
pub fn NetBanner() -> Element {
    let tr = use_tr();
    let net = use_context::<Net>().0;
    let key = match net() {
        NetStatus::Online => return rsx! {},
        NetStatus::Connecting => "net.connecting",
        NetStatus::Offline => "net.offline",
    };
    rsx! {
        div { class: "net-banner", "{t(&tr, key)}" }
    }
}
