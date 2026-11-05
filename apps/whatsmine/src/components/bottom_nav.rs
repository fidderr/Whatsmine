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
    let settings = crate::components::use_settings();
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

    let order = tab_keys(&settings.read().tab_order);
    rsx! {
        for key in order {
            Fragment { key: "{key}",
                match key.as_str() {
                "calls" => rsx! {
                    Link { to: Route::Calls {}, class: class(Tab::Calls), "data-t": "tab-calls",
                        Icon { name: IconName::Phone, size: 24 }
                        "{t(&tr, \"nav.calls\")}"
                        if missed > 0 {
                            span { class: "nav-badge", "{missed}" }
                        }
                    }
                },
                "people" => rsx! {
                    Link { to: Route::People {}, class: class(Tab::People), "data-t": "tab-people",
                        Icon { name: IconName::People, size: 24 }
                        "{t(&tr, \"nav.people\")}"
                        if requests > 0 {
                            span { class: "nav-badge", "{requests}" }
                        }
                    }
                },
                "settings" => rsx! {
                    Link { to: Route::SettingsScreen {}, class: class(Tab::Settings), "data-t": "tab-settings",
                        Icon { name: IconName::Sliders, size: 24 }
                        "{t(&tr, \"nav.settings\")}"
                    }
                },
                _ => rsx! {
                    Link { to: Route::Chats {}, class: class(Tab::Chats), "data-t": "tab-chats",
                        Icon { name: IconName::Chat, size: 24 }
                        "{t(&tr, \"nav.chats\")}"
                        if unread > 0 {
                            span { class: "nav-badge", "{unread}" }
                        }
                    }
                },
                }
            }
        }
    }
}

/// The tabs in the order someone chose, whatever that list holds: each one once.
pub fn tab_keys(chosen: &[String]) -> Vec<String> {
    const ALL: [&str; 4] = ["chats", "calls", "people", "settings"];
    let mut out: Vec<String> = Vec::new();
    for key in chosen {
        if ALL.contains(&key.as_str()) && !out.contains(key) {
            out.push(key.clone());
        }
    }
    for key in ALL {
        if !out.iter().any(|k| k == key) {
            out.push(key.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::tab_keys;

    #[test]
    fn every_tab_is_there_once_whatever_was_saved() {
        assert_eq!(tab_keys(&[]), ["chats", "calls", "people", "settings"]);
        let odd = ["people", "people", "nonsense", "chats"].map(str::to_owned);
        assert_eq!(tab_keys(&odd), ["people", "chats", "calls", "settings"]);
    }
}
