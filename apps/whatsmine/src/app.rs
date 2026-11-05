// SECURITY: Dioxus RSX string interpolation ("{var}") escapes HTML. Message
// text and names from other people are only ever rendered that way; there is
// no `dangerous_inner_html` anywhere in this crate.

use std::collections::HashMap;

use dioxus::prelude::*;
use whatsmine_core::{Core, CoreEvent, DbRecovery, NetStatus, Settings, TransferState};

use crate::call::{self, CallOverlay, CallUi, Phase};
use crate::components::{Toast, ToastLevel, ToastMessage};
use crate::i18n;
use crate::screens::{
    AddContact, AddType, AppearanceScreen, Calls, Chat, ChatLook, Chats, ConnectionScreen,
    ContactInfo, DevicesScreen, FoldersScreen, GroupInfoScreen, HelpScreen, LookEdit, LooksScreen,
    NewGroup, NotificationsScreen, People, PrivacyScreen, PublicAddress, Requests, SavedScreen,
    SearchScreen, SettingsScreen, SharedMedia, SpeedScreen, Start, StorageScreen,
};
use crate::shell::Shell;

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
    #[layout(Shell)]
    #[route("/")]
    Chats {},
    #[route("/chat/:id")]
    Chat { id: String },
    #[route("/calls")]
    Calls {},
    #[route("/people")]
    People {},
    #[route("/add")]
    AddContact {},
    #[route("/add/type")]
    AddType {},
    #[route("/requests")]
    Requests {},
    #[route("/address")]
    PublicAddress {},
    #[route("/contact/:id")]
    ContactInfo { id: String },
    #[route("/look/:id")]
    ChatLook { id: String },
    #[route("/shared/:id")]
    SharedMedia { id: String },
    #[route("/search")]
    SearchScreen {},
    #[route("/saved")]
    SavedScreen {},
    #[route("/folders")]
    FoldersScreen {},
    #[route("/group/new")]
    NewGroup {},
    #[route("/group/:id")]
    GroupInfoScreen { id: String },
    #[route("/settings")]
    SettingsScreen {},
    #[route("/settings/privacy")]
    PrivacyScreen {},
    #[route("/settings/notifications")]
    NotificationsScreen {},
    #[route("/settings/appearance")]
    AppearanceScreen {},
    #[route("/settings/looks")]
    LooksScreen {},
    #[route("/settings/looks/:id")]
    LookEdit { id: String },
    #[route("/settings/connection")]
    ConnectionScreen {},
    #[route("/settings/speed")]
    SpeedScreen {},
    #[route("/settings/devices")]
    DevicesScreen {},
    #[route("/settings/storage")]
    StorageScreen {},
    #[route("/settings/help")]
    HelpScreen {},
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

/// In the binary and as a plain element: no file to load and nothing to wait for, so the first
/// frame already looks as it should.
const STYLESHEET: &str = include_str!("../assets/style.css");

/// Says when the window may have changed. Edge to edge, a keyboard changes nothing the page can
/// see, so it also looks for as long as something can be typed in, and keeps what is being typed
/// in out from under the keyboard. `--text-zoom` is how much the phone's font size setting
/// scales text, for what has to fit whatever it is set to.
#[cfg(target_os = "android")]
const WINDOW_WATCH: &str = "\
    const measureText = () => { \
        const probe = document.createElement('div'); \
        probe.style.cssText = 'position:absolute;visibility:hidden;font-size:100px;line-height:1'; \
        probe.textContent = '0'; \
        document.body.append(probe); \
        document.documentElement.style.setProperty('--text-zoom', probe.getBoundingClientRect().height / 100); \
        probe.remove(); \
    }; \
    const tell = () => { measureText(); dioxus.send(innerWidth); }; \
    addEventListener('resize', tell); \
    document.addEventListener('visibilitychange', tell); \
    matchMedia('(prefers-color-scheme: dark)').addEventListener('change', tell); \
    const typing = () => document.activeElement?.matches('input:not([type=range]):not([type=date]), textarea'); \
    const showField = () => { \
        const root = document.querySelector('.app-root'); \
        if (!root || !typing()) return; \
        const free = innerHeight - parseFloat(getComputedStyle(root).getPropertyValue('--safe-bottom')); \
        const box = document.activeElement.getBoundingClientRect(); \
        if (box.bottom > free || box.top < 0) document.activeElement.scrollIntoView({ block: 'center' }); \
    }; \
    let watching = 0; \
    document.addEventListener('focusin', () => { \
        if (watching) return; \
        let quiet = 0; \
        watching = setInterval(() => { \
            tell(); \
            showField(); \
            quiet = typing() ? 0 : quiet + 1; \
            if (quiet > 5) { clearInterval(watching); watching = 0; } \
        }, 200); \
    }); \
    tell();";

#[component]
pub fn App() -> Element {
    let started = use_hook(|| {
        #[cfg(target_os = "android")]
        crate::android::init_once();
        // One core for the whole process: it outlives this window.
        crate::boot::core()
    });
    // On Android the UI's loop can lose a wake when another screen closes, and then everything
    // here waits for a touch. And the page applies changes at once: nothing runs until the last
    // change is applied, and a WebView that is not drawing has no animation frames for that.
    use_hook(mobile_sentinel::keep_event_loop_awake);
    #[cfg(target_os = "android")]
    use_hook(|| {
        document::eval("window.interpreter.headless = true;");
    });

    match started {
        Ok(core) => rsx! { Main { core: AppCore(core) } },
        Err(e) => rsx! {
            style { "{STYLESHEET}" }
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
    use_context_provider(crate::back::BackStack::default);
    let invite = use_signal(|| None);
    use_context_provider(|| crate::links::InviteSig(invite));

    let call_phase = use_signal(|| Phase::Idle);
    use_context_provider(|| CallUi(call_phase));
    let bridge = call::use_call_bridge(&core.0, call_phase, toast_sig, translations);
    use_context_provider(|| bridge.clone());
    use_hook(|| call::restore(&core.0, call_phase, &bridge));
    #[cfg(feature = "desktop")]
    crate::shell::desktop_shell(core.0.clone(), settings, crate::links::InviteSig(invite));

    crate::media_server::use_media_server(core.0.clone());
    use_hook(|| {
        let core = core.0.clone();
        std::thread::spawn(move || crate::media::clean_scratch(&core));
    });

    // Tell the user if the old data had to go, once, or if the database had to be reset or
    // cannot persist.
    use_hook(|| {
        if core.0.started_clean() {
            toast(
                &mut toast_sig,
                t(&translations, "db.clean_start"),
                ToastLevel::Info,
            );
            return;
        }
        let key = match core.0.db_recovery() {
            DbRecovery::Healthy => return,
            DbRecovery::Recovered { .. } => "db.recovered",
            DbRecovery::Volatile => "db.volatile",
        };
        toast(&mut toast_sig, t(&translations, key), ToastLevel::Error);
    });

    // Pump core events into signals.
    let pump_core = core.0.clone();
    let pump_bridge = bridge.clone();
    use_future(move || {
        let core = pump_core.clone();
        let bridge = pump_bridge.clone();
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
                    Ok(CoreEvent::Call(ev)) => {
                        call::on_event(
                            ev,
                            &core,
                            call_phase,
                            &bridge,
                            &mut toast_sig,
                            &translations,
                        );
                        *rev.write() += 1;
                    }
                    Ok(CoreEvent::Net(status)) => net.set(status),
                    Ok(CoreEvent::ReadElsewhere { contact_id }) => {
                        crate::notify::mark_read(&contact_id);
                        *rev.write() += 1;
                    }
                    Ok(CoreEvent::SettingsChanged) => {
                        let mut s = settings;
                        s.set(core.settings());
                    }
                    // Nothing of this person is left here: the next start is a new beginning.
                    Ok(CoreEvent::RemovedFromPerson) => std::process::exit(0),
                    Ok(CoreEvent::MessageGone { .. }) => {
                        *msg_rev.write() += 1;
                        *rev.write() += 1;
                    }
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
    let density_class = match s.density.as_str() {
        "roomy" => "density-roomy",
        "compact" => "density-compact",
        "big" => "density-big",
        _ => "",
    };
    let onboarded = s.onboarded;
    let locked_app = s.app_lock;
    let text_size = s.text_size.clone();
    drop(s);
    // How the app looks: when it is chosen, and when the look itself was changed.
    let look_core = core.0.clone();
    let app_look = use_memo(move || {
        let _ = settings.read().app_look.len();
        let _ = rev();
        look_core.app_look()
    });
    let look = app_look();
    let theme_class = crate::look::theme_class(look.base);
    let look_style = crate::look::look_style(&look, false);
    let app_font = look.app_font.clone();
    // Text follows the size chosen, and the font: everything is in rem.
    use_effect(use_reactive!(|text_size, app_font| {
        document::eval(&format!(
            "document.documentElement.style.fontSize = '{:.2}px';",
            crate::look::root_font_px(&text_size, &app_font)
        ));
    }));
    // Fonts someone added themselves.
    let fonts_core = core.0.clone();
    let own_fonts = use_memo(move || {
        let _ = rev();
        fonts_core
            .own_fonts()
            .iter()
            .map(|f| {
                let id: String = f
                    .id
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                format!("@font-face{{font-family:'wm-own-{id}';src:url(/font/own-{id});font-display:swap}}")
            })
            .collect::<String>()
    });
    // System bars and keyboard: how far they overlap us (rotation and window changes alter it),
    // and the bars' colour.
    let mut safe_area = use_signal(crate::platform::safe_area_style);
    #[cfg(target_os = "android")]
    {
        let mut system_dark = use_signal(crate::platform::system_dark);
        let read_window = move || {
            let now = crate::platform::safe_area_style();
            if now != *safe_area.peek() {
                safe_area.set(now);
            }
            let dark = crate::platform::system_dark();
            if dark != *system_dark.peek() {
                system_dark.set(dark);
            }
        };
        // Right after the start the window does not know them yet.
        use_future(move || async move {
            let mut read_window = read_window;
            for delay_ms in [100, 300, 600, 1200, 2500] {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                read_window();
            }
        });
        use_future(move || async move {
            let mut read_window = read_window;
            let mut changes = document::eval(WINDOW_WATCH);
            while changes.recv::<i32>().await.is_ok() {
                read_window();
            }
        });
        use_effect(move || {
            let look = app_look();
            let dark = crate::platform::is_dark(&look, *system_dark.read());
            crate::platform::set_system_bars(dark, crate::platform::page_color(&look, dark));
        });
    }
    #[cfg(not(target_os = "android"))]
    let _ = &mut safe_area;

    // What the test scenarios drive the app with.
    #[cfg(feature = "test-hooks")]
    {
        let dir = core.0.data_dir().to_path_buf();
        let hook_core = core.0.clone();
        use_hook(move || crate::debug_hooks::start(dir, hook_core));
        use_future(crate::debug_hooks::script_loop);
    }

    rsx! {
        style { "{STYLESHEET}" }
        style { "{own_fonts}" }
        document::Meta {
            name: "viewport",
            content: "width=device-width, initial-scale=1, viewport-fit=cover, interactive-widget=resizes-content",
        }
        div { class: "app-root {theme_class} {density_class}", style: "{safe_area.read()}{look_style.vars}",
            div { class: "status-bar-bg" }
            if onboarded {
                Router::<Route> {}
            } else {
                Start {}
            }
            if onboarded && locked_app {
                crate::lock::LockScreen {}
            }
            CallOverlay {}
            if let Some(msg) = toast_sig.read().clone() {
                Toast {
                    msg,
                    on_dismiss: move |_| toast_sig.set(None),
                }
            }
        }
    }
}
