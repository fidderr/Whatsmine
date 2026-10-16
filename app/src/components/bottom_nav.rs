use dioxus::prelude::*;

use crate::app::{Rev, Route, t, use_core, use_tr};
use crate::components::{Icon, IconName};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    Chats,
    Requests,
    Settings,
}

/// The one shared bottom navigation bar.
#[component]
pub fn BottomNav(active: Tab) -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let _ = rev();
    let unread: u32 = core.chats().iter().map(|c| c.unread).sum();
    let requests = core.requests().len();

    let class = |tab: Tab| {
        if tab == active {
            "nav-item active"
        } else {
            "nav-item"
        }
    };

    rsx! {
        nav { class: "bottom-nav",
            Link { to: Route::Chats {}, class: class(Tab::Chats),
                Icon { name: IconName::Chat, size: 22 }
                "{t(&tr, \"nav.chats\")}"
                if unread > 0 {
                    span { class: "nav-badge", "{unread}" }
                }
            }
            Link { to: Route::Requests {}, class: class(Tab::Requests),
                Icon { name: IconName::Users, size: 22 }
                "{t(&tr, \"nav.requests\")}"
                if requests > 0 {
                    span { class: "nav-badge", "{requests}" }
                }
            }
            Link { to: Route::SettingsScreen {}, class: class(Tab::Settings),
                Icon { name: IconName::Gear, size: 22 }
                "{t(&tr, \"nav.settings\")}"
            }
        }
    }
}
