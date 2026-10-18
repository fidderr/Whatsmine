//! The invisible layer around every screen: what the platform needs while the
//! app runs (tray and window on the desktop; Back, notification taps and
//! "is the app in front" on Android).

use dioxus::prelude::*;

use crate::app::{Route, SettingsSig, use_core};
use crate::back::BackStack;
use crate::links::{InviteDialog, InviteSig};
use crate::notify;

#[component]
pub fn Shell() -> Element {
    let core = use_core();
    let nav = use_navigator();
    let settings = use_context::<SettingsSig>().0;
    let stack = use_context::<BackStack>();
    let invite = use_context::<InviteSig>();
    let _ = (&settings, invite);

    // Is the app in front of the user? Then the system stays quiet.
    use_future(move || async move {
        let mut eval = document::eval(
            "const android = /Android/.test(navigator.userAgent);\n\
             const report = () => dioxus.send(document.visibilityState === 'visible' && (android || document.hasFocus()));\n\
             document.addEventListener('visibilitychange', report);\n\
             window.addEventListener('focus', report);\n\
             window.addEventListener('blur', report);\n\
             report();\n\
             await new Promise(() => {});",
        );
        while let Ok(active) = eval.recv::<bool>().await {
            notify::set_ui_active(active);
            #[cfg(target_os = "android")]
            if active {
                open_requested(nav, invite);
            }
        }
    });

    // Esc closes what is on top; the mouse's back button and Alt+Left go back.
    {
        let stack = stack.clone();
        use_future(move || {
            let stack = stack.clone();
            async move {
                let mut eval = document::eval(
                    "document.addEventListener('keydown', e => {\n\
                     if (e.key === 'Escape') dioxus.send('esc');\n\
                     else if (e.altKey && e.key === 'ArrowLeft') dioxus.send('back');\n\
                     });\n\
                     window.addEventListener('mouseup', e => {\n\
                     if (e.button === 3) { e.preventDefault(); dioxus.send('back'); }\n\
                     });\n\
                     await new Promise(() => {});",
                );
                while let Ok(what) = eval.recv::<String>().await {
                    if what == "esc" {
                        stack.close_top();
                    } else if !stack.close_top() && nav.can_go_back() {
                        nav.go_back();
                    }
                }
            }
        });
    }

    // Keep the background setting in force (and the texts in the right language).
    #[cfg(target_os = "android")]
    {
        use_effect(move || {
            let s = settings.read();
            crate::android::apply_texts(&s.locale);
            crate::android::apply_background(s.background);
        });
        // Android 13+ needs the user's yes before any notification shows:
        // ask once, a moment after the first screen is up.
        {
            let core = core.clone();
            use_hook(move || crate::android::ask_for_notifications_once(&core));
        }
        // A notification or a link may have started the app: open what it asked for.
        use_hook(|| {
            spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                // Also while the app is already in front (no visibility change then).
                loop {
                    open_requested(nav, invite);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
        });
        // Back closes a picture or menu, else goes up a screen, and from the
        // first screen to the background (or out of the app when it is not
        // allowed to run in the background).
        {
            let stack = stack.clone();
            use_future(move || {
                let stack = stack.clone();
                async move {
                    let Some(mut rx) = crate::android::take_back_events() else {
                        return;
                    };
                    while rx.recv().await.is_some() {
                        if stack.close_top() {
                            continue;
                        }
                        if nav.can_go_back() {
                            nav.go_back();
                        } else if settings.peek().background {
                            mobile_sentinel::background_service::move_to_back();
                        } else {
                            mobile_sentinel::background_service::exit_app();
                        }
                    }
                }
            });
        }
    }

    #[cfg(not(target_os = "android"))]
    let _ = &core;

    rsx! {
        Outlet::<Route> {}
        InviteDialog {}
    }
}

#[cfg(target_os = "android")]
fn open_requested(nav: dioxus::router::Navigator, invite: InviteSig) {
    if let Some(what) = mobile_sentinel::background_service::take_open_request() {
        if let Some(chat) = what.strip_prefix("chat:") {
            nav.push(Route::Chat {
                id: chat.to_string(),
            });
        } else if what == "requests" {
            nav.push(Route::Requests {});
        } else if let Some(link) = what.strip_prefix("link:") {
            crate::links::offer(invite, link);
        }
    }
}

/// The desktop side: tray, window, single instance. Mounted above everything,
/// onboarding included, so a hidden window can always be brought back.
#[cfg(feature = "desktop")]
pub fn desktop_shell(
    core: whatsmine_core::Core,
    settings: Signal<whatsmine_core::Settings>,
    invite: InviteSig,
) {
    use dioxus::desktop::tao::event::{Event, WindowEvent};
    use dioxus::desktop::trayicon::menu::{Menu, MenuItem, PredefinedMenuItem};
    use dioxus::desktop::{WindowCloseBehaviour, use_window, use_wry_event_handler};

    let window = use_window();

    // The tray icon: left click opens the window, the menu can open or quit.
    use_hook(|| {
        let menu = Menu::new();
        let strings = crate::i18n::load_translations(&core.settings().locale);
        let label = |key: &str, fallback: &str| {
            strings
                .get(key)
                .cloned()
                .unwrap_or_else(|| fallback.to_string())
        };
        let open = MenuItem::with_id("open", label("tray.open", "Open WhatsMine"), true, None);
        let quit = MenuItem::with_id("quit", label("tray.quit", "Quit"), true, None);
        let _ = menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit]);
        let tray = dioxus::desktop::trayicon::init_tray_icon(menu, tray_icon_image());
        let _ = tray.set_tooltip(Some("WhatsMine"));
        crate::desktop::promote_tray_icon();
        // Clicking a whatsmine:// link anywhere opens this app.
        crate::desktop::register_link_handler();
        // This copy was started by such a link.
        if let Some(link) = crate::desktop::take_startup_link() {
            crate::links::offer(invite, &link);
        }
    });
    {
        let (window, core) = (window.clone(), core.clone());
        dioxus::desktop::use_tray_menu_event_handler(move |event| match event.id.0.as_str() {
            "open" => show(&window),
            "quit" => quit(&core),
            _ => {}
        });
    }

    // Another launch of the app asks this one to show itself or quit.
    {
        let (window, core) = (window.clone(), core.clone());
        use_future(move || {
            let (window, core) = (window.clone(), core.clone());
            async move {
                let Some(mut rx) = crate::desktop::take_commands() else {
                    return;
                };
                while let Some(cmd) = rx.recv().await {
                    match cmd {
                        crate::desktop::Command::Show => show(&window),
                        crate::desktop::Command::Quit => quit(&core),
                        crate::desktop::Command::Open(link) => {
                            crate::links::offer(invite, &link);
                            show(&window);
                        }
                    }
                }
            }
        });
    }

    // A call rings: if the window is hidden or minimised, bring it up.
    {
        let (window, core) = (window.clone(), core.clone());
        use_future(move || {
            let (window, core) = (window.clone(), core.clone());
            async move {
                use tokio::sync::broadcast::error::RecvError;
                let mut rx = core.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(whatsmine_core::CoreEvent::Call(
                            whatsmine_core::calls::CallEvent::Incoming { .. },
                        )) => show(&window),
                        Ok(_) | Err(RecvError::Lagged(_)) => {}
                        Err(RecvError::Closed) => return,
                    }
                }
            }
        });
    }

    // Closing hides to the tray while the background setting is on.
    {
        let window = window.clone();
        use_effect(move || {
            let behaviour = if settings.read().background {
                WindowCloseBehaviour::WindowHides
            } else {
                WindowCloseBehaviour::WindowCloses
            };
            window.set_close_behavior(behaviour);
        });
    }
    // "Start with the PC" follows the setting (and repairs a moved exe path).
    use_effect(move || {
        let on = settings.read().start_with_system;
        let profile = std::env::var("WHATSMINE_PROFILE").ok();
        crate::desktop::set_autostart(on, profile.as_deref());
    });

    // Focus decides whether the system should notify; shutting down on a real close.
    {
        let core = core.clone();
        use_wry_event_handler(move |event, _| {
            if let Event::WindowEvent { event, .. } = event {
                match event {
                    WindowEvent::Focused(focused) => notify::set_ui_active(*focused),
                    WindowEvent::CloseRequested => {
                        if settings.peek().background {
                            notify::set_ui_active(false);
                        } else {
                            core.shutdown();
                        }
                    }
                    WindowEvent::Destroyed => core.shutdown(),
                    _ => {}
                }
            }
        });
    }
}

#[cfg(feature = "desktop")]
fn show(window: &dioxus::desktop::DesktopContext) {
    window.set_visible(true);
    window.set_minimized(false);
    window.set_focus();
    notify::set_ui_active(true);
}

#[cfg(feature = "desktop")]
fn quit(core: &whatsmine_core::Core) {
    core.shutdown();
    std::process::exit(0);
}

#[cfg(feature = "desktop")]
fn tray_icon_image() -> Option<dioxus::desktop::trayicon::Icon> {
    let img = image::load_from_memory(include_bytes!("../assets/icon.png")).ok()?;
    let img = img
        .resize(32, 32, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let (w, h) = img.dimensions();
    dioxus::desktop::trayicon::Icon::from_rgba(img.into_raw(), w, h).ok()
}
