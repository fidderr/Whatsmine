// SECURITY: Dioxus RSX string interpolation ("{var}") escapes HTML. Message
// text and names from other people are only ever rendered that way; there is
// no `dangerous_inner_html` anywhere in this crate.

use std::collections::HashMap;

use dioxus::prelude::*;
use whatsmine_core::{
    Core, CoreEvent, DbRecovery, NetMode, NetStatus, Settings, TransferState, paths,
};

use crate::components::{Toast, ToastLevel, ToastMessage};
use crate::i18n;
use crate::screens::{AddContact, Chat, Chats, ContactInfo, Onboarding, Requests, SettingsScreen};

pub type Translations = Signal<HashMap<String, String>>;

/// Looks up a translation; falls back to the key itself.
pub fn t(translations: &Translations, key: &str) -> String {
    translations
        .read()
        .get(key)
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

#[derive(Clone, Routable, Debug, PartialEq)]
pub enum Route {
    #[route("/")]
    Chats {},
    #[route("/chat/:id")]
    Chat { id: String },
    #[route("/add")]
    AddContact {},
    #[route("/requests")]
    Requests {},
    #[route("/contact/:id")]
    ContactInfo { id: String },
    #[route("/settings")]
    SettingsScreen {},
}

/// The core handle, shared through context.
#[derive(Clone)]
pub struct AppCore(pub Core);

/// Bumped whenever contacts, chats or presence change.
#[derive(Clone, Copy)]
pub struct Rev(pub Signal<u64>);

/// Bumped whenever a message is added or changes status.
#[derive(Clone, Copy)]
pub struct MsgRev(pub Signal<u64>);

/// Contacts currently typing (contact id -> when we heard it).
#[derive(Clone, Copy)]
pub struct Typing(pub Signal<HashMap<String, i64>>);

/// File transfers in progress: message id -> (bytes done, bytes total).
#[derive(Clone, Copy)]
pub struct Transfers(pub Signal<HashMap<String, (u64, u64)>>);

#[derive(Clone, Copy)]
pub struct Net(pub Signal<NetStatus>);

#[derive(Clone, Copy)]
pub struct SettingsSig(pub Signal<Settings>);

pub type ToastSig = Signal<Option<ToastMessage>>;

pub fn use_core() -> Core {
    use_context::<AppCore>().0
}

pub fn use_tr() -> Translations {
    use_context::<Translations>()
}

/// Shows a toast from anywhere.
pub fn toast(sig: &mut ToastSig, message: String, level: ToastLevel) {
    sig.set(Some(ToastMessage { message, level }));
}

const TYPING_SHOWN_MS: u64 = 5000;

#[component]
pub fn App() -> Element {
    let started = use_hook(|| {
        #[cfg(target_os = "android")]
        mobile_sentinel::init(mobile_sentinel::InitConfig {
            log_tag: "WhatsMine".to_string(),
            ..Default::default()
        });
        let profile = std::env::var("WHATSMINE_PROFILE").ok();
        paths::data_dir(profile.as_deref())
            .and_then(|dir| Core::start(&dir, NetMode::Internet))
            .map_err(|e| format!("{e:#}"))
    });

    match started {
        Ok(core) => rsx! { Main { core: AppCore(core) } },
        Err(e) => rsx! {
            document::Link { rel: "stylesheet", href: asset!("/assets/style.css") }
            div { class: "app-root",
                div { class: "screen no-nav",
                    div { class: "empty-state",
                        p { class: "empty-title", "WhatsMine could not start" }
                        p { class: "empty-subtitle", "{e}" }
                    }
                }
            }
        },
    }
}

impl PartialEq for AppCore {
    fn eq(&self, other: &Self) -> bool {
        self.0.my_key() == other.0.my_key()
    }
}

#[component]
fn Main(core: AppCore) -> Element {
    let initial = core.0.settings();
    let settings = use_signal(|| initial.clone());
    let translations: Translations = use_signal(|| i18n::load_translations(&initial.locale));
    let mut rev = use_signal(|| 0u64);
    let mut msg_rev = use_signal(|| 0u64);
    let mut typing = use_signal(HashMap::<String, i64>::new);
    let mut net = use_signal(|| core.0.net_status());
    let mut transfers = use_signal(HashMap::<String, (u64, u64)>::new);
    let mut toast_sig: ToastSig = use_signal(|| None);

    use_context_provider(|| core.clone());
    use_context_provider(|| translations);
    use_context_provider(|| SettingsSig(settings));
    use_context_provider(|| Rev(rev));
    use_context_provider(|| MsgRev(msg_rev));
    use_context_provider(|| Typing(typing));
    use_context_provider(|| Net(net));
    use_context_provider(|| Transfers(transfers));
    use_context_provider(|| toast_sig);

    crate::media_server::use_media_server(core.0.clone());

    // Tell the user if the database had to be reset or cannot persist.
    use_hook(|| {
        let key = match core.0.db_recovery() {
            DbRecovery::Healthy => return,
            DbRecovery::Recovered { .. } => "db.recovered",
            DbRecovery::Volatile => "db.volatile",
        };
        toast(&mut toast_sig, t(&translations, key), ToastLevel::Error);
    });

    // Pump core events into signals.
    let pump_core = core.0.clone();
    use_future(move || {
        let core = pump_core.clone();
        async move {
            use tokio::sync::broadcast::error::RecvError;
            let mut rx = core.subscribe();
            loop {
                match rx.recv().await {
                    Ok(CoreEvent::MessageAdded(m)) => {
                        // Their message arrived: they are done typing it.
                        if !m.outgoing {
                            typing.write().remove(&m.contact_id);
                        }
                        *msg_rev.write() += 1;
                        *rev.write() += 1;
                    }
                    Ok(CoreEvent::MessageStatus { .. }) => {
                        *msg_rev.write() += 1;
                        *rev.write() += 1;
                    }
                    Ok(CoreEvent::MessageUpdated(m)) => {
                        if m.attachment
                            .as_ref()
                            .is_some_and(|a| a.state != TransferState::Transferring)
                        {
                            transfers.write().remove(&m.id);
                        }
                        *msg_rev.write() += 1;
                        *rev.write() += 1;
                    }
                    Ok(CoreEvent::Transfer {
                        message_id,
                        done,
                        total,
                        ..
                    }) => {
                        transfers.write().insert(message_id, (done, total));
                    }
                    Ok(CoreEvent::Typing { contact_id }) => {
                        let now = whatsmine_core::now_ms();
                        typing.write().insert(contact_id.clone(), now);
                        spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(TYPING_SHOWN_MS))
                                .await;
                            let mut map = typing.write();
                            if map.get(&contact_id) == Some(&now) {
                                map.remove(&contact_id);
                            }
                        });
                    }
                    Ok(CoreEvent::RequestReceived { name, .. }) => {
                        let who = if name.is_empty() {
                            t(&translations, "requests.someone")
                        } else {
                            name
                        };
                        toast(
                            &mut toast_sig,
                            t(&translations, "requests.toast").replace("{name}", &who),
                            ToastLevel::Info,
                        );
                        *rev.write() += 1;
                    }
                    Ok(CoreEvent::Presence {
                        contact_id, online, ..
                    }) => {
                        if !online {
                            typing.write().remove(&contact_id);
                        }
                        *rev.write() += 1;
                    }
                    Ok(CoreEvent::Net(status)) => net.set(status),
                    Ok(_) => *rev.write() += 1,
                    Err(RecvError::Lagged(_)) => {
                        *rev.write() += 1;
                        *msg_rev.write() += 1;
                    }
                    Err(RecvError::Closed) => return,
                }
            }
        }
    });

    let s = settings.read();
    let theme_class = match s.theme.as_str() {
        "light" => "theme-light",
        "dark" => "theme-dark",
        _ => "theme-system",
    };
    let onboarded = s.onboarded;
    drop(s);
    let safe_area = use_hook(crate::platform::safe_area_style);

    // Closing the window: hang up properly so contacts see us go offline
    // right away instead of after the heartbeat timeout.
    #[cfg(feature = "desktop")]
    {
        let core = core.0.clone();
        dioxus::desktop::use_wry_event_handler(move |event, _| {
            use dioxus::desktop::tao::event::{Event, WindowEvent};
            if let Event::WindowEvent {
                event: WindowEvent::CloseRequested | WindowEvent::Destroyed,
                ..
            } = event
            {
                core.shutdown();
            }
        });
    }

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/style.css") }
        document::Meta {
            name: "viewport",
            content: "width=device-width, initial-scale=1, viewport-fit=cover, interactive-widget=resizes-content",
        }
        div { class: "app-root {theme_class}", style: "{safe_area}",
            div { class: "status-bar-bg" }
            if onboarded {
                Router::<Route> {}
            } else {
                Onboarding {}
            }
            if let Some(msg) = toast_sig.read().clone() {
                Toast {
                    msg,
                    on_dismiss: move |_| toast_sig.set(None),
                }
            }
        }
    }
}
