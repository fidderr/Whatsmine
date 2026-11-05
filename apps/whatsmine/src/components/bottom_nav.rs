use dioxus::prelude::*;

use crate::app::{Rev, Route, t, use_core, use_tr};
use crate::components::{Icon, IconName};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    Chats,
    Calls,
    People,
    Settings,
}

/// The tabs: at the bottom on a phone, at the side on a wide window.
#[component]
pub fn BottomNav(active: Tab) -> Element {
    rsx! {
        nav { class: "bottom-nav", Tabs { active } }
    }
}

#[component]
pub fn Tabs(active: Tab) -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let _ = rev();
    let unread: u32 = core
        .chats()
        .iter()
        .filter(|c| !c.contact.muted && !c.prefs.archived)
        .map(|c| c.unread)
        .sum();
    let requests = core.requests().len();
    let missed = core
        .call_records()
        .iter()
        .take_while(|c| c.at > crate::screens::calls_seen_at())
        .filter(|c| c.missed && !c.outgoing)
        .count();

    let class = |tab: Tab| {
        if tab == active {
            "nav-item active"
        } else {
            "nav-item"
        }
    };

    rsx! {
        Link { to: Route::Chats {}, class: class(Tab::Chats), "data-t": "tab-chats",
            Icon { name: IconName::Chat, size: 24 }
            "{t(&tr, \"nav.chats\")}"
            if unread > 0 {
                span { class: "nav-badge", "{unread}" }
            }
        }
        Link { to: Route::Calls {}, class: class(Tab::Calls), "data-t": "tab-calls",
            Icon { name: IconName::Phone, size: 24 }
            "{t(&tr, \"nav.calls\")}"
            if missed > 0 {
                span { class: "nav-badge", "{missed}" }
            }
        }
        Link { to: Route::People {}, class: class(Tab::People), "data-t": "tab-people",
            Icon { name: IconName::People, size: 24 }
            "{t(&tr, \"nav.people\")}"
            if requests > 0 {
                span { class: "nav-badge", "{requests}" }
            }
        }
        Link { to: Route::SettingsScreen {}, class: class(Tab::Settings), "data-t": "tab-settings",
            Icon { name: IconName::Sliders, size: 24 }
            "{t(&tr, \"nav.settings\")}"
        }
    }
}
