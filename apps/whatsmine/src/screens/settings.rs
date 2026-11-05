use dioxus::prelude::*;
use whatsmine_core::Settings;
use whatsmine_core::power::SpeedMode;

use crate::app::{Net, Rev, Route, ToastSig, Translations, t, toast, use_core, use_tr};
use crate::components::{
    Avatar, BottomNav, CameraMode, CameraOverlay, Chips, ConfirmDialog, Header, Icon, IconName,
    Notice, Row, Sheet, SheetItem, Swatches, SwitchRow, Tab, Title, ToastLevel, change,
    use_settings,
};
use crate::health;
use crate::i18n;
use crate::js;
use crate::look;
use crate::media::{self, Picked};
use crate::screens::contact::{disappear_label, disappear_options};

fn opt(tr: &Translations, pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, key)| ((*k).to_owned(), t(tr, key)))
        .collect()
}

/// The hub: who you are, what needs attention, and every part of the settings.
#[component]
pub fn SettingsScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let settings = use_settings();
    let looks = health::use_watch();
    let mut editing = use_signal(|| false);
    let _ = looks();
    let s = settings();
    let problems = health::check(&tr).len();
    let problem_label = t(&tr, "settings.problems").replace("{n}", &problems.to_string());
    let speed = match s.speed_mode {
        SpeedMode::Automatic => t(&tr, "speed.auto_short"),
        SpeedMode::AlwaysInstant => t(&tr, "speed.instant_short"),
        SpeedMode::BatterySaver => t(&tr, "speed.saver_short"),
    };

    rsx! {
        div { class: "screen",
            Title { title: t(&tr, "settings.title") }
            div { class: "body",
                button {
                    class: "group row tall",
                    "data-t": "profile",
                    onclick: move |_| editing.set(true),
                    Avatar {
                        id: "me".to_string(),
                        name: s.display_name.clone(),
                        image: s.avatar.clone(),
                    }
                    span { class: "stack",
                        span { class: "row-t b one-line", "{s.display_name}" }
                        span { class: "sub", "{t(&tr, \"settings.profile_note\")}" }
                    }
                    span { class: "dim", style: "display:flex",
                        Icon { name: IconName::ChevronRight, size: 16 }
                    }
                }
                if problems > 0 {
                    Notice {
                        warn: true,
                        icon: IconName::Warning,
                        text: problem_label,
                        to: Route::ConnectionScreen {},
                        tag: "problems",
                    }
                }
                div { class: "group",
                    Row { icon: IconName::Palette, title: t(&tr, "settings.appearance"), small: true, to: Route::AppearanceScreen {}, tag: "appearance" }
                    Row { icon: IconName::Folder, title: t(&tr, "settings.chats_folders"), small: true, to: Route::FoldersScreen {}, tag: "folders" }
                    Row { icon: IconName::Bookmark, title: t(&tr, "saved.title"), small: true, to: Route::SavedScreen {}, tag: "saved" }
                    Row { icon: IconName::Bell, title: t(&tr, "settings.notifications_title"), small: true, to: Route::NotificationsScreen {}, tag: "notifications" }
                    Row { icon: IconName::Lock, title: t(&tr, "settings.privacy"), small: true, to: Route::PrivacyScreen {}, tag: "privacy" }
                    Row { icon: IconName::Link, title: t(&tr, "address.title"), small: true, to: Route::PublicAddress {}, tag: "address" }
                    Row { icon: IconName::Battery, title: t(&tr, "speed.title"), value: speed, small: true, to: Route::SpeedScreen {}, tag: "speed" }
                    Row { icon: IconName::Wifi, title: t(&tr, "connection.title"), small: true, to: Route::ConnectionScreen {}, tag: "connection" }
                    Row { icon: IconName::Device, title: t(&tr, "devices.title"), small: true, to: Route::DevicesScreen {}, tag: "devices" }
                    Row { icon: IconName::Storage, title: t(&tr, "storage.title"), small: true, to: Route::StorageScreen {}, tag: "storage" }
                    Row { icon: IconName::Help, title: t(&tr, "help.title"), small: true, to: Route::HelpScreen {}, tag: "help" }
                }
            }
            if editing() {
                ProfileSheet { on_close: move |_| editing.set(false) }
            }
            BottomNav { active: Tab::Settings }
        }
    }
}

/// Your 12 words. While they are on screen, no picture and no shared screen can show them.
#[component]
fn WordsSheet(on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    use_hook(|| crate::guard::set(crate::guard::Reason::Words, true));
    use_drop(|| crate::guard::set(crate::guard::Reason::Words, false));
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "devices.words"), tag: "words-sheet",
            p { class: "note", "{t(&tr, \"devices.words_text\")}" }
            div { class: "words",
                for (i, word) in core.recovery_words().into_iter().enumerate() {
                    span { key: "{i}", "{i + 1}. {word}" }
                }
            }
        }
    }
}

/// Your name and photo.
#[component]
fn ProfileSheet(on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut settings = use_settings();
    let mut toast_sig = use_context::<ToastSig>();
    let mut name = use_signal(|| settings.peek().display_name.clone());
    let mut camera = use_signal(|| false);
    let s = settings();
    let apply_photo = {
        let core = core.clone();
        move |file: Picked| {
            let core = core.clone();
            spawn(async move {
                let ok = media::set_avatar(core.clone(), file).await;
                settings.set(core.settings());
                if !ok {
                    toast(
                        &mut toast_sig,
                        t(&tr, "settings.photo_failed"),
                        ToastLevel::Error,
                    );
                }
            });
        }
    };
    let (from_gallery, from_camera, camera_apply) =
        (apply_photo.clone(), apply_photo.clone(), apply_photo);
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "settings.profile"), tag: "profile-sheet",
            div { style: "align-self:center",
                Avatar {
                    id: "me".to_string(),
                    name: s.display_name.clone(),
                    size: "large",
                    image: s.avatar.clone(),
                    zoom: true,
                }
            }
            label { class: "field",
                span { class: "gt", "{t(&tr, \"onboarding.name_label\")}" }
                input {
                    r#type: "text",
                    maxlength: "64",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
            }
            p { class: "note", "{t(&tr, \"settings.name_hint\")}" }
            div { class: "sheet-list",
                SheetItem {
                    label: t(&tr, "settings.photo_choose"),
                    icon: IconName::Image,
                    on_tap: move |_| {
                        let apply = from_gallery.clone();
                        spawn(async move {
                            match media::pick(media::IMAGES, false).await {
                                None => toast(&mut toast_sig, t(&tr, "attach.unavailable"), ToastLevel::Warning),
                                Some(mut files) if !files.is_empty() => apply(files.remove(0)),
                                Some(_) => {}
                            }
                        });
                    },
                }
                SheetItem {
                    label: t(&tr, "settings.photo_take"),
                    icon: IconName::Camera,
                    on_tap: move |_| {
                        if media::NATIVE_CAMERA {
                            let apply = from_camera.clone();
                            spawn(async move {
                                if let Some(file) = media::capture(false).await {
                                    apply(file);
                                }
                            });
                        } else {
                            camera.set(true);
                        }
                    },
                }
                if s.avatar.is_some() {
                    SheetItem {
                        label: t(&tr, "settings.photo_remove"),
                        icon: IconName::Trash,
                        danger: true,
                        on_tap: move |_| {
                            let _ = core.clear_avatar();
                            settings.set(core.settings());
                        },
                    }
                }
            }
            button {
                class: "btn main",
                "data-t": "save",
                disabled: name().trim().is_empty(),
                onclick: move |_| {
                    let chosen: String = name.peek().trim().chars().take(64).collect();
                    change(settings, |s| s.display_name = chosen);
                    on_close.call(());
                },
                "{t(&tr, \"common.save\")}"
            }
        }
        if camera() {
            CameraOverlay {
                mode: CameraMode::Photo,
                on_done: move |file: Option<Picked>| {
                    camera.set(false);
                    if let Some(file) = file {
                        camera_apply(file);
                    }
                },
            }
        }
    }
}

#[component]
pub fn AppearanceScreen() -> Element {
    let mut tr = use_tr();
    crate::js::use_scroll_top();
    let settings = use_settings();
    let mut languages = use_signal(|| false);
    let s = settings();
    let themes = opt(
        &tr,
        &[
            ("system", "settings.theme_system"),
            ("dark", "settings.theme_dark"),
            ("light", "settings.theme_light"),
            ("black", "settings.theme_black"),
            ("colorblind", "settings.theme_colorblind"),
            ("colorblind-light", "settings.theme_colorblind_light"),
        ],
    );
    let sizes = opt(
        &tr,
        &[
            ("small", "look.small"),
            ("normal", "look.normal"),
            ("large", "look.large"),
            ("huge", "look.huge"),
        ],
    );
    let densities = opt(
        &tr,
        &[
            ("roomy", "look.roomy"),
            ("normal", "look.normal"),
            ("compact", "look.compact"),
            ("big", "look.big_photos"),
        ],
    );
    let accents: Vec<(String, String, String)> = look::ACCENTS
        .iter()
        .map(|(k, color, name)| {
            (
                (*k).to_owned(),
                (*color).to_owned(),
                t(&tr, &format!("look.{name}")),
            )
        })
        .collect();
    let language = i18n::LANGUAGES
        .iter()
        .find(|(c, _)| *c == s.locale)
        .map(|(_, n)| (*n).to_owned())
        .unwrap_or_default();
    let style = look::chat_style(&s, &Default::default());

    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "settings.appearance") }
            div { class: "body{style.class}",
                section { class: "panel", style: "background:var(--surface2);{style.messages}",
                    div { class: "bubble-wrap in",
                        div { class: "bubble in", "{t(&tr, \"look.sample_in\")}" }
                    }
                    div { class: "bubble-wrap out",
                        div { class: "bubble out", "{t(&tr, \"look.sample_out\")}" }
                    }
                }
                h2 { class: "gt", "{t(&tr, \"settings.theme\")}" }
                Chips {
                    options: themes,
                    value: s.theme.clone(),
                    on_change: move |v: String| change(settings, |s| s.theme = v),
                }
                h2 { class: "gt", "{t(&tr, \"look.accent\")}" }
                Swatches {
                    options: accents,
                    value: s.accent.clone(),
                    on_change: move |v: String| change(settings, |s| s.accent = v),
                }
                h2 { class: "gt", "{t(&tr, \"look.text_size\")}" }
                Chips {
                    options: sizes,
                    value: s.text_size.clone(),
                    on_change: move |v: String| change(settings, |s| s.text_size = v),
                }
                h2 { class: "gt", "{t(&tr, \"look.density\")}" }
                Chips {
                    options: densities,
                    value: s.density.clone(),
                    on_change: move |v: String| change(settings, |s| s.density = v),
                }
                h2 { class: "gt", "{t(&tr, \"look.all_chats\")}" }
                div { class: "group",
                    Row {
                        icon: IconName::Palette,
                        title: t(&tr, "look.title"),
                        small: true,
                        to: Route::ChatLook { id: "all".to_owned() },
                        tag: "look",
                    }
                    Row {
                        icon: IconName::Chat,
                        title: t(&tr, "settings.language"),
                        value: language,
                        small: true,
                        tag: "language",
                        on_tap: move |_| languages.set(true),
                    }
                }
            }
            if languages() {
                Sheet { on_close: move |_| languages.set(false), title: t(&tr, "settings.language"),
                    div { class: "sheet-list",
                        for (code, name) in i18n::LANGUAGES.iter().copied() {
                            button {
                                key: "{code}",
                                class: "sheet-item",
                                "data-t": "lang-{code}",
                                onclick: move |_| {
                                    tr.set(i18n::load_translations(code));
                                    change(settings, |s| s.locale = code.to_owned());
                                    languages.set(false);
                                },
                                "{name}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// How a chat looks, or every chat: `id` is "all" for the latter.
#[component]
pub fn ChatLook(id: String) -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let settings = use_settings();
    let every = id == "all";
    let mut prefs = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.chat_prefs(&id))
    };
    let s = settings();
    let p = prefs();
    let (wall, bubble, dim) = if every {
        (s.wallpaper.clone(), s.bubble_color.clone(), s.wallpaper_dim)
    } else {
        (p.wallpaper.clone(), p.bubble_color.clone(), p.wallpaper_dim)
    };
    let style = look::chat_style(&s, &p);
    let swatches: Vec<(String, String, String)> = look::ACCENTS
        .iter()
        .map(|(k, color, name)| {
            (
                (*k).to_owned(),
                (*color).to_owned(),
                t(&tr, &format!("look.{name}")),
            )
        })
        .collect();
    let shapes = opt(&tr, &[("round", "look.round"), ("square", "look.square")]);
    let layouts = opt(&tr, &[("bubbles", "look.bubbles"), ("flat", "look.flat")]);

    let set_look = {
        let (core, id) = (core.clone(), id.clone());
        move |f: &dyn Fn(&mut whatsmine_core::rich::ChatPrefs), g: &dyn Fn(&mut Settings)| {
            if id == "all" {
                let mut next = settings.peek().clone();
                g(&mut next);
                change(settings, |s| *s = next);
            } else {
                let mut next = prefs.peek().clone();
                f(&mut next);
                core.set_chat_prefs(&id, &next);
                prefs.set(next);
            }
        }
    };
    let (mut for_dim, mut for_bubble) = (set_look.clone(), set_look.clone());

    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "look.title") }
            div { class: "body{style.class}",
                section { class: "panel", style: "background:var(--surface2);{style.messages}",
                    div { class: "bubble-wrap in",
                        div { class: "bubble in", "{t(&tr, \"look.sample_in\")}" }
                    }
                    div { class: "bubble-wrap out",
                        div { class: "bubble out", "{t(&tr, \"look.sample_out\")}" }
                    }
                }
                h2 { class: "gt", "{t(&tr, \"look.wallpaper\")}" }
                div { style: "display:flex;gap:6px",
                    for (key, css) in look::WALLPAPERS.iter().copied() {
                        {
                            let on = wall == key;
                            let mut set = set_look.clone();
                            rsx! {
                                button {
                                    key: "{key}",
                                    class: if on { "wall on" } else { "wall" },
                                    style: if css.is_empty() { "background:var(--surface)".to_owned() } else { css.to_owned() },
                                    aria_pressed: "{on}",
                                    "data-t": "wall-{key}",
                                    onclick: move |_| set(&|p| p.wallpaper = key.to_owned(), &|s| s.wallpaper = key.to_owned()),
                                    if css.is_empty() {
                                        "{t(&tr, \"look.none\")}"
                                    }
                                }
                            }
                        }
                    }
                }
                label { class: "row sm",
                    span { class: "sub b", "{t(&tr, \"look.dim\")}" }
                    input {
                        r#type: "range",
                        min: "0",
                        max: "90",
                        value: "{dim}",
                        style: "flex:1;min-width:0;accent-color:var(--primary)",
                        oninput: move |e| {
                            let v = e.value().parse::<u8>().unwrap_or(0);
                            for_dim(&|p| p.wallpaper_dim = v, &|s| s.wallpaper_dim = v);
                        },
                    }
                }
                h2 { class: "gt", "{t(&tr, \"look.bubble_color\")}" }
                Swatches {
                    options: swatches,
                    value: bubble,
                    on_change: move |v: String| for_bubble(&|p| p.bubble_color = v.clone(), &|s| s.bubble_color = v.clone()),
                }
                if every {
                    h2 { class: "gt", "{t(&tr, \"look.shape\")}" }
                    Chips {
                        options: shapes,
                        value: if s.bubble_square { "square".to_owned() } else { "round".to_owned() },
                        on_change: move |v: String| change(settings, |s| s.bubble_square = v == "square"),
                    }
                    Chips {
                        options: layouts,
                        value: if s.flat_layout { "flat".to_owned() } else { "bubbles".to_owned() },
                        on_change: move |v: String| change(settings, |s| s.flat_layout = v == "flat"),
                    }
                }
            }
        }
    }
}

#[component]
pub fn NotificationsScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let settings = use_settings();
    let looks = health::use_watch();
    let _ = looks();
    let s = settings();
    let items = health::check(&tr);
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "settings.notifications_title") }
            div { class: "body",
                if !items.is_empty() {
                    div { class: "group",
                        for item in items {
                            health::HealthRow { key: "{item.key}", item }
                        }
                    }
                }
                div { class: "group",
                    SwitchRow {
                        title: t(&tr, "settings.notifications"),
                        note: t(&tr, "settings.notifications_hint"),
                        checked: s.notifications,
                        tag: "notifications",
                        on_change: move |v| change(settings, |s| s.notifications = v),
                    }
                    SwitchRow {
                        title: t(&tr, "settings.previews"),
                        note: t(&tr, "settings.previews_hint"),
                        checked: s.notification_previews,
                        on_change: move |v| change(settings, |s| s.notification_previews = v),
                    }
                    SwitchRow {
                        title: t(&tr, "requests.quiet"),
                        note: t(&tr, "requests.quiet_note"),
                        checked: s.quiet_requests,
                        on_change: move |v| change(settings, |s| s.quiet_requests = v),
                    }
                }
                div { class: "group",
                    SwitchRow {
                        title: t(&tr, "settings.background"),
                        note: t(&tr, if cfg!(target_os = "android") { "settings.background_hint_android" } else { "settings.background_hint_desktop" }),
                        checked: s.background,
                        on_change: move |v| change(settings, |s| s.background = v),
                    }
                    if cfg!(not(target_os = "android")) {
                        SwitchRow {
                            title: t(&tr, "settings.autostart"),
                            note: t(&tr, "settings.autostart_hint"),
                            checked: s.start_with_system,
                            on_change: move |v| change(settings, |s| s.start_with_system = v),
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn PrivacyScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let settings = use_settings();
    let mut sheet_disappear = use_signal(|| false);
    let mut filter = use_signal(|| false);
    let mut show_hidden = use_signal(|| false);
    let _ = rev();
    let s = settings();
    let blocked = core.blocked();
    let hidden = core.hidden_chats().len();
    let can_lock = crate::lock::can_lock();
    let lock_after = opt(
        &tr,
        &[
            ("0", "privacy.lock_now"),
            ("60", "privacy.lock_1m"),
            ("300", "privacy.lock_5m"),
        ],
    );
    let default_label = disappear_label(
        &tr,
        (s.disappear_default_ms > 0).then_some(s.disappear_default_ms),
    );
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "settings.privacy") }
            div { class: "body",
                h2 { class: "gt", style: "padding-top:0", "{t(&tr, \"privacy.who_sees\")}" }
                div { class: "group",
                    SwitchRow {
                        title: t(&tr, "settings.last_seen"),
                        note: t(&tr, "settings.last_seen_hint"),
                        checked: s.share_last_seen,
                        on_change: move |v| change(settings, |s| s.share_last_seen = v),
                    }
                    SwitchRow {
                        title: t(&tr, "settings.read_receipts"),
                        note: t(&tr, "settings.read_receipts_hint"),
                        checked: s.read_receipts,
                        tag: "read-receipts",
                        on_change: move |v| change(settings, |s| s.read_receipts = v),
                    }
                    SwitchRow {
                        title: t(&tr, "settings.typing"),
                        note: t(&tr, "settings.typing_hint"),
                        checked: s.typing_indicators,
                        on_change: move |v| change(settings, |s| s.typing_indicators = v),
                    }
                }
                h2 { class: "gt", "{t(&tr, \"privacy.on_device\")}" }
                div { class: "group",
                    if can_lock {
                        SwitchRow {
                            title: t(&tr, "privacy.app_lock"),
                            note: t(&tr, "privacy.app_lock_note"),
                            checked: s.app_lock,
                            tag: "app-lock",
                            // Only someone who can answer may turn it on or off.
                            on_change: move |v| {
                                spawn(async move {
                                    if crate::lock::prove(&t(&tr, "lock.reason")).await {
                                        change(settings, |s| s.app_lock = v);
                                    }
                                });
                            },
                        }
                        if s.app_lock {
                            div { class: "row sm", style: "flex-direction:column;align-items:stretch",
                                span { class: "sub", "{t(&tr, \"privacy.lock_after\")}" }
                                Chips {
                                    options: lock_after,
                                    value: s.lock_after_s.to_string(),
                                    on_change: move |v: String| {
                                        let seconds = v.parse::<u32>().unwrap_or(60);
                                        change(settings, |s| s.lock_after_s = seconds);
                                    },
                                }
                            }
                        }
                    } else {
                        div { class: "row sm still",
                            span { class: "sub", "{t(&tr, \"privacy.no_lock\")}" }
                        }
                    }
                    if hidden > 0 {
                        Row {
                            title: t(&tr, "privacy.hidden_chats"),
                            value: hidden.to_string(),
                            small: true,
                            tag: "hidden-chats",
                            on_tap: move |_| {
                                spawn(async move {
                                    if crate::lock::prove(&t(&tr, "lock.reason")).await {
                                        show_hidden.set(true);
                                    }
                                });
                            },
                        }
                    }
                    SwitchRow {
                        title: t(&tr, "privacy.block_screenshots"),
                        small: true,
                        checked: s.block_screenshots,
                        on_change: move |v| change(settings, |s| s.block_screenshots = v),
                    }
                    SwitchRow {
                        title: t(&tr, "privacy.incognito_keyboard"),
                        small: true,
                        checked: s.incognito_keyboard,
                        on_change: move |v| change(settings, |s| s.incognito_keyboard = v),
                    }
                    Row {
                        title: t(&tr, "privacy.disappear_default"),
                        value: default_label,
                        small: true,
                        tag: "disappear-default",
                        on_tap: move |_| sheet_disappear.set(true),
                    }
                    Row {
                        title: t(&tr, "privacy.request_filter"),
                        note: t(&tr, "privacy.request_filter_note"),
                        tag: "request-filter",
                        on_tap: move |_| filter.set(true),
                    }
                }
                h2 { class: "gt", "{t(&tr, \"privacy.address\")}" }
                div { class: "group",
                    SwitchRow {
                        title: t(&tr, "privacy.relays"),
                        note: t(&tr, "privacy.relays_note"),
                        checked: s.use_relays,
                        on_change: move |v| change(settings, |s| s.use_relays = v),
                    }
                }
                if !blocked.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"settings.blocked\")}" }
                    p { class: "note", "{t(&tr, \"contacts.blocked_hint\")}" }
                    div { class: "group",
                        for c in blocked {
                            BlockedRow { key: "{c.id}", id: c.id.clone(), name: c.display_name() }
                        }
                    }
                }
            }
            if sheet_disappear() {
                Sheet { on_close: move |_| sheet_disappear.set(false), title: t(&tr, "privacy.disappear_default"),
                    Chips {
                        options: disappear_options(&tr),
                        value: s.disappear_default_ms.to_string(),
                        on_change: move |v: String| {
                            let ms = v.parse::<i64>().unwrap_or(0);
                            change(settings, |s| s.disappear_default_ms = ms);
                        },
                    }
                }
            }
            if filter() {
                FilterSheet { on_close: move |_| filter.set(false) }
            }
            if show_hidden() {
                HiddenSheet { on_close: move |_| show_hidden.set(false) }
            }
        }
    }
}

#[component]
fn HiddenSheet(on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let rev = use_context::<Rev>().0;
    let _ = rev();
    let chats = core.hidden_chats();
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "privacy.hidden_chats"), tag: "hidden-sheet",
            div { class: "sheet-list",
                for c in chats {
                    {
                        let (open_id, show_id, core) = (c.contact.id.clone(), c.contact.id.clone(), core.clone());
                        let prefs = c.prefs.clone();
                        rsx! {
                            div { key: "{c.contact.id}", class: "sheet-item",
                                Avatar { id: c.contact.id.clone(), name: c.contact.display_name(), size: "tiny", image: c.contact.avatar.clone() }
                                button {
                                    class: "stack one-line row-t",
                                    style: "border:none;background:none;color:inherit;font:inherit;text-align:left;cursor:pointer",
                                    onclick: move |_| {
                                        on_close.call(());
                                        nav.push(Route::Chat { id: open_id.clone() });
                                    },
                                    "{c.contact.display_name()}"
                                }
                                button {
                                    class: "btn small-btn",
                                    onclick: move |_| {
                                        let mut p = prefs.clone();
                                        p.locked = false;
                                        core.set_chat_prefs(&show_id, &p);
                                    },
                                    "{t(&tr, \"privacy.show_again\")}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FilterSheet(on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let settings = use_settings();
    let mut text = use_signal(|| settings.peek().request_filter.join(", "));
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "privacy.request_filter"),
            p { class: "note", "{t(&tr, \"privacy.request_filter_note\")}" }
            input {
                r#type: "text",
                placeholder: "{t(&tr, \"privacy.request_filter_placeholder\")}",
                value: "{text}",
                oninput: move |e| text.set(e.value()),
            }
            button {
                class: "btn main",
                onclick: move |_| {
                    let words: Vec<String> = text
                        .peek()
                        .split(',')
                        .map(|w| w.trim().to_lowercase())
                        .filter(|w| !w.is_empty())
                        .collect();
                    change(settings, |s| s.request_filter = words);
                    on_close.call(());
                },
                "{t(&tr, \"common.save\")}"
            }
        }
    }
}

#[component]
fn BlockedRow(id: String, name: String) -> Element {
    let tr: Translations = use_tr();
    let core = use_core();
    rsx! {
        div { class: "row still",
            Avatar { id: id.clone(), name: name.clone(), size: "medium" }
            span { class: "row-t stack one-line", "{name}" }
            button {
                class: "btn small-btn",
                onclick: move |_| {
                    let _ = core.unblock(&id);
                },
                "{t(&tr, \"contact.unblock\")}"
            }
        }
    }
}

/// The connection in plain words, from what is known right now.
#[component]
pub fn ConnectionScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let settings = use_settings();
    let net = use_context::<Net>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let looks = health::use_watch();
    let _ = looks();
    let mut turn = use_signal(|| 0u32);
    let report = {
        let core = core.clone();
        use_resource(move || {
            let core = core.clone();
            let _ = turn();
            async move { core.connection_report().await }
        })
    };
    let items = health::check(&tr);
    let s = settings();
    let _ = net();
    let shown = report.read().clone().flatten();

    use whatsmine_core::power::Network;
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "connection.title") }
            div { class: "body",
                if let Some(r) = shown.clone() {
                    {
                        let (mark, icon, title, note) = if r.network == Network::None {
                            ("status-mark off", IconName::Xmark, "connection.offline", "connection.offline_note")
                        } else if !r.endpoint_up {
                            ("status-mark", IconName::Moon, "connection.resting", "connection.resting_note")
                        } else if r.reachable_direct {
                            ("status-mark", IconName::Check, "connection.reachable", "connection.reachable_note")
                        } else if r.on_relay {
                            ("status-mark", IconName::Check, "connection.relayed", "connection.relayed_note")
                        } else {
                            ("status-mark warn", IconName::Warning, "connection.hard", "connection.hard_note")
                        };
                        rsx! {
                            section { class: "panel", style: "flex-direction:row;align-items:center",
                                span { class: mark, Icon { name: icon, size: 24 } }
                                span { class: "stack",
                                    span { style: "font-size:1.25rem;line-height:1.4;font-weight:500", "{t(&tr, title)}" }
                                    span { class: "sub", "{t(&tr, note)}" }
                                }
                            }
                        }
                    }
                }
                if !items.is_empty() {
                    div { class: "group",
                        for item in items {
                            health::HealthRow { key: "{item.key}", item }
                        }
                    }
                }
                if let Some(r) = shown.clone() {
                    h2 { class: "gt", "{t(&tr, \"connection.network\")}" }
                    div { class: "group",
                        Row {
                            title: t(&tr, "connection.network"),
                            small: true,
                            value: t(&tr, match r.network {
                                Network::Wifi => "connection.wifi",
                                Network::Wired => "connection.wired",
                                Network::Mobile => "connection.mobile",
                                Network::None => "connection.none",
                            }),
                        }
                        Row {
                            title: "IPv6".to_owned(),
                            small: true,
                            value: t(&tr, if r.ipv6 { "connection.works" } else { "connection.not_here" }),
                        }
                        if let Some(hard) = r.hard_nat {
                            Row {
                                title: t(&tr, "connection.router"),
                                small: true,
                                value: t(&tr, if hard { "connection.router_hard" } else { "connection.router_easy" }),
                            }
                        }
                    }
                    if !r.peers.is_empty() {
                        h2 { class: "gt", "{t(&tr, \"connection.contacts_now\")}" }
                        div { class: "group",
                            for p in r.peers.clone() {
                                PeerRow { key: "{p.contact_id}", path: p }
                            }
                        }
                    }
                }
                div { class: "group",
                    SwitchRow {
                        title: t(&tr, "privacy.relays"),
                        small: true,
                        checked: s.use_relays,
                        on_change: move |v| change(settings, |s| s.use_relays = v),
                    }
                }
                div { class: "btns",
                    button {
                        class: "btn",
                        style: "background:var(--surface)",
                        "data-t": "test-now",
                        onclick: {
                            let core = core.clone();
                            move |_| {
                                core.network_changed();
                                *turn.write() += 1;
                            }
                        },
                        Icon { name: IconName::Restart, size: 20 }
                        "{t(&tr, \"connection.test\")}"
                    }
                    button {
                        class: "btn",
                        style: "background:var(--surface)",
                        "data-t": "copy-report",
                        onclick: move |_| {
                            if let Some(r) = shown.as_ref() {
                                js::copy_text(&report_text(r));
                                toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                            }
                        },
                        Icon { name: IconName::Copy, size: 20 }
                        "{t(&tr, \"connection.copy_report\")}"
                    }
                }
            }
        }
    }
}

fn quit() {
    std::process::exit(0);
}

/// For someone helping you: facts only, nothing about who or what.
fn report_text(r: &whatsmine_core::ConnectionReport) -> String {
    let direct = r.peers.iter().filter(|p| p.direct).count();
    format!(
        "WhatsMine {}\nnetwork: {:?}\nlevel: {:?}\nendpoint up: {}\nreachable directly: {}\non relay: {}\nipv6: {}\nhard router: {:?}\nconnections: {} ({} direct)\n",
        env!("CARGO_PKG_VERSION"),
        r.network,
        r.level,
        r.endpoint_up,
        r.reachable_direct,
        r.on_relay,
        r.ipv6,
        r.hard_nat,
        r.peers.len(),
        direct,
    )
}

#[component]
fn PeerRow(path: whatsmine_core::PeerPath) -> Element {
    let tr = use_tr();
    let core = use_core();
    let contact = core.contact(&path.contact_id);
    let name = contact
        .as_ref()
        .map(|c| c.display_name())
        .unwrap_or_default();
    let (state, class) = if path.direct {
        (t(&tr, "connection.direct"), "val ok-text")
    } else {
        (t(&tr, "connection.through_relay"), "val")
    };
    rsx! {
        div { class: "row sm still",
            Avatar {
                id: path.contact_id.clone(),
                name: name.clone(),
                size: "tiny",
                image: contact.as_ref().and_then(|c| c.avatar.clone()),
            }
            span { class: "row-t stack one-line", "{name}" }
            span { class, "{state}" }
        }
    }
}

#[component]
pub fn SpeedScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let settings = use_settings();
    let core = use_core();
    let looks = health::use_watch();
    let _ = looks();
    let s = settings();
    let days = opt(
        &tr,
        &[
            ("1", "speed.days_1"),
            ("7", "speed.days_7"),
            ("30", "speed.days_30"),
            ("0", "speed.forever"),
        ],
    );
    let items = health::check(&tr);
    let modes = [
        (SpeedMode::Automatic, "speed.auto", "speed.auto_text"),
        (
            SpeedMode::AlwaysInstant,
            "speed.instant",
            "speed.instant_text",
        ),
        (SpeedMode::BatterySaver, "speed.saver", "speed.saver_text"),
    ];
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "speed.title") }
            div { class: "body",
                for (mode, title, text) in modes {
                    {
                        let on = s.speed_mode == mode;
                        rsx! {
                            button {
                                key: "{title}",
                                class: "group row",
                                role: "radio",
                                aria_checked: "{on}",
                                "data-t": "{title}",
                                style: if on { "outline:2px solid var(--primary);outline-offset:-2px;align-items:flex-start;padding:12px" } else { "align-items:flex-start;padding:12px" },
                                onclick: move |_| change(settings, |s| s.speed_mode = mode),
                                span { style: if on { "flex:none;width:22px;height:22px;border-radius:11px;border:7px solid var(--primary)" } else { "flex:none;width:22px;height:22px;border-radius:11px;border:2px solid var(--text-secondary)" } }
                                span { class: "stack",
                                    span { class: "row-t b", "{t(&tr, title)}" }
                                    span { class: "sub", "{t(&tr, text)}" }
                                }
                            }
                        }
                    }
                }
                h2 { class: "gt", "{t(&tr, \"speed.keep_trying\")}" }
                Chips {
                    options: days,
                    value: s.keep_trying_days.to_string(),
                    on_change: move |v: String| {
                        let d = v.parse::<u32>().unwrap_or(7);
                        change(settings, |s| s.keep_trying_days = d);
                        core.wake();
                    },
                }
                if !items.is_empty() {
                    div { class: "group",
                        for item in items {
                            health::HealthRow { key: "{item.key}", item }
                        }
                    }
                }
            }
        }
    }
}

/// Puts bytes in a file where the person can find it, and says where.
pub async fn save_text_file(
    core: &whatsmine_core::Core,
    name: &str,
    mime: &str,
    bytes: Vec<u8>,
) -> Option<String> {
    let dir = core.data_dir().join("outgoing.tmp");
    std::fs::create_dir_all(&dir).ok()?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || ".-_ ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = dir.join(&safe);
    std::fs::write(&path, bytes).ok()?;
    let saved =
        media::save_to_downloads(path.to_string_lossy().into_owned(), safe, mime.to_owned()).await;
    let _ = std::fs::remove_file(&path);
    saved
}

/// Makes a backup and saves it. What to tell the person, or nothing if it worked by itself.
pub async fn back_up_now(core: whatsmine_core::Core, settings: Signal<Settings>) -> Option<String> {
    let made = {
        let core = core.clone();
        media::blocking(move || core.make_backup().ok())
            .await
            .flatten()
    }?;
    let name = format!(
        "whatsmine-backup-{}.wmbackup",
        chrono::Local::now().format("%Y-%m-%d")
    );
    let place = save_text_file(&core, &name, "application/octet-stream", made).await?;
    change(settings, |s| {
        s.last_backup_at = whatsmine_core::model::now_ms()
    });
    Some(place)
}

#[component]
pub fn DevicesScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let settings = use_settings();
    let mut toast_sig = use_context::<ToastSig>();
    let mut show_words = use_signal(|| false);
    let mut restoring = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut linking = use_signal(|| false);
    let mut confirm_remove = use_signal(|| false);
    let rev = use_context::<Rev>().0;
    let _ = rev();
    let device = core.device_state();
    let s = settings();
    let last = if s.last_backup_at > 0 {
        t(&tr, "devices.last_backup").replace("{when}", &crate::fmt::when(&tr, s.last_backup_at))
    } else {
        t(&tr, "devices.no_backup")
    };
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "devices.title") }
            div { class: "body",
                div { class: "group",
                    Row {
                        icon: if cfg!(target_os = "android") { IconName::Device } else { IconName::Monitor },
                        title: t(&tr, "devices.this"),
                        note: t(&tr, "devices.active_now"),
                    }
                    if device.linked {
                        Row {
                            icon: if cfg!(target_os = "android") { IconName::Monitor } else { IconName::Device },
                            title: t(&tr, "devices.other"),
                            note: t(&tr, if device.removing { "devices.removing" } else { "devices.other_note" }),
                        }
                    }
                }
                if device.linked {
                    if !device.removing {
                        button {
                            class: "btn",
                            "data-t": "remove-device",
                            onclick: move |_| confirm_remove.set(true),
                            "{t(&tr, \"devices.remove\")}"
                        }
                    }
                } else {
                    button {
                        class: "btn main",
                        "data-t": "link-device",
                        onclick: move |_| linking.set(true),
                        Icon { name: IconName::Qr, size: 20 }
                        "{t(&tr, \"devices.link\")}"
                    }
                    p { class: "note", "{t(&tr, \"devices.link_note\")}" }
                }
                h2 { class: "gt", "{t(&tr, \"devices.lost\")}" }
                div { class: "group",
                    Row {
                        icon: IconName::Lock,
                        title: t(&tr, "devices.words"),
                        note: t(&tr, "devices.words_note"),
                        tag: "words",
                        // Whoever has these words is you.
                        on_tap: move |_| {
                            spawn(async move {
                                if crate::lock::prove(&t(&tr, "lock.reason")).await {
                                    show_words.set(true);
                                }
                            });
                        },
                    }
                    SwitchRow {
                        title: t(&tr, "devices.auto_backup"),
                        note: t(&tr, "devices.auto_backup_note"),
                        checked: s.auto_backup,
                        tag: "auto-backup",
                        on_change: move |on| change(settings, |s| s.auto_backup = on),
                    }
                    Row { title: last, small: true }
                }
                p { class: "note", "{t(&tr, \"devices.backup_note\")}" }
                div { class: "btns",
                    button {
                        class: "btn",
                        style: "background:var(--surface)",
                        "data-t": "back-up",
                        disabled: busy(),
                        onclick: {
                            let core = core.clone();
                            move |_| {
                                let core = core.clone();
                                busy.set(true);
                                spawn(async move {
                                    match back_up_now(core, settings).await {
                                        Some(place) => toast(&mut toast_sig, t(&tr, "file.saved").replace("{path}", &place), ToastLevel::Info),
                                        None => toast(&mut toast_sig, t(&tr, "file.save_failed"), ToastLevel::Error),
                                    }
                                    busy.set(false);
                                });
                            }
                        },
                        Icon { name: IconName::Download, size: 20 }
                        "{t(&tr, \"devices.back_up\")}"
                    }
                    button {
                        class: "btn",
                        style: "background:var(--surface)",
                        "data-t": "restore",
                        onclick: move |_| restoring.set(true),
                        Icon { name: IconName::Restart, size: 20 }
                        "{t(&tr, \"devices.restore\")}"
                    }
                }
            }
            if show_words() {
                WordsSheet { on_close: move |_| show_words.set(false) }
            }
            if restoring() {
                RestoreSheet { on_close: move |_| restoring.set(false) }
            }
            if linking() {
                LinkSheet { on_close: move |_| linking.set(false) }
            }
            if confirm_remove() {
                ConfirmDialog {
                    title: t(&tr, "devices.remove_title"),
                    message: t(&tr, "devices.remove_text"),
                    confirm_label: t(&tr, "devices.remove"),
                    on_confirm: {
                        let core = core.clone();
                        move |_| {
                            core.remove_other_device();
                            confirm_remove.set(false);
                        }
                    },
                    on_cancel: move |_| confirm_remove.set(false),
                }
            }
        }
    }
}

/// The code for the other device. It closes by itself once that one has everything.
#[component]
fn LinkSheet(on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let code = {
        let core = core.clone();
        use_resource(move || {
            let core = core.clone();
            async move { core.start_link().await }
        })
    };
    let _ = rev();
    if core.device_state().linked {
        on_close.call(());
        return rsx! {};
    }
    let shown = code.read().clone();
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "devices.link"), tag: "link-sheet",
            ol { class: "steps", style: "font-size:1rem;gap:8px",
                li { b { style: "width:28px;height:28px;font-size:0.875rem", "1" } span { "{t(&tr, \"devices.link_step_1\")}" } }
                li { b { style: "width:28px;height:28px;font-size:0.875rem", "2" } span { "{t(&tr, \"devices.link_step_2\")}" } }
            }
            if let Some(text) = shown {
                div { style: "align-self:center",
                    crate::components::QrCode { data: text.clone() }
                }
                button {
                    class: "btn",
                    "data-t": "copy-link-code",
                    onclick: move |_| {
                        js::copy_text(&text);
                        toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                    },
                    Icon { name: IconName::Copy, size: 20 }
                    "{t(&tr, \"common.copy\")}"
                }
            } else {
                p { class: "note", "{t(&tr, \"devices.link_ready\")}" }
            }
            p { class: "note", "{t(&tr, \"devices.link_files\")}" }
        }
    }
}

/// Becoming the other device: the code from the first one.
#[component]
pub fn JoinDeviceSheet(on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let invite = use_context::<crate::links::InviteSig>();
    let mut text = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut done = use_signal(|| false);
    let mut scanning = use_signal(|| false);
    let _ = invite;
    if done() {
        return rsx! {
            Sheet { on_close: move |_| quit(), title: t(&tr, "restore.done_title"), tag: "restore-done",
                p { class: "note", "{t(&tr, \"restore.done_text\")}" }
                button { class: "btn main", "data-t": "restore-close", onclick: move |_| quit(),
                    "{t(&tr, \"restore.close\")}"
                }
            }
        };
    }
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "devices.join"), tag: "join-device-sheet",
            p { class: "note", "{t(&tr, \"devices.join_text\")}" }
            if crate::components::scan::NATIVE_SCANNER {
                button {
                    class: "btn",
                    "data-t": "join-scan",
                    onclick: move |_| {
                        spawn(async move {
                            if let Some(code) = crate::components::scan::scan_native().await {
                                text.set(code);
                            }
                        });
                    },
                    Icon { name: IconName::Camera, size: 20 }
                    "{t(&tr, \"add.scan\")}"
                }
            } else {
                button {
                    class: "btn",
                    "data-t": "join-scan",
                    onclick: move |_| scanning.set(true),
                    Icon { name: IconName::Camera, size: 20 }
                    "{t(&tr, \"add.scan\")}"
                }
            }
            input {
                r#type: "text",
                "data-t": "join-code",
                placeholder: "{t(&tr, \"devices.join_placeholder\")}",
                autocomplete: "off",
                spellcheck: "false",
                value: "{text}",
                oninput: move |e| {
                    text.set(e.value());
                    error.set(None);
                },
            }
            if let Some(e) = error() {
                p { class: "field-error", "{e}" }
            }
            p { class: "note", "{t(&tr, \"restore.replaces\")}" }
            button {
                class: "btn main",
                "data-t": "join-go",
                disabled: text().trim().is_empty() || busy(),
                onclick: move |_| {
                    let code = text.peek().trim().to_owned();
                    let core = core.clone();
                    busy.set(true);
                    spawn(async move {
                        match core.join_device(&code).await {
                            Ok(()) => done.set(true),
                            Err(e) => error.set(Some(t(&tr, &format!("add.err.{e}")))),
                        }
                        busy.set(false);
                    });
                },
                if busy() {
                    "{t(&tr, \"add.looking\")}"
                } else {
                    "{t(&tr, \"devices.join\")}"
                }
            }
        }
        if scanning() {
            crate::components::scan::ScanOverlay {
                on_code: move |code: String| {
                    scanning.set(false);
                    text.set(code);
                },
                on_close: move |_| scanning.set(false),
            }
        }
    }
}

/// Coming back: the 12 words, and a backup if there is one.
#[component]
pub fn RestoreSheet(on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut words = use_signal(String::new);
    let mut file: Signal<Option<Picked>> = use_signal(|| None);
    let mut error = use_signal(|| None::<String>);
    let mut done = use_signal(|| false);
    let mut join = use_signal(|| false);
    if done() {
        return rsx! {
            Sheet { on_close: move |_| quit(), title: t(&tr, "restore.done_title"), tag: "restore-done",
                p { class: "note", "{t(&tr, \"restore.done_text\")}" }
                button { class: "btn main", "data-t": "restore-close", onclick: move |_| quit(),
                    "{t(&tr, \"restore.close\")}"
                }
            }
        };
    }
    if join() {
        return rsx! { JoinDeviceSheet { on_close: move |_| join.set(false) } };
    }
    let file_label = file()
        .map(|f| f.name)
        .unwrap_or_else(|| t(&tr, "restore.pick"));
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "devices.restore"), tag: "restore-sheet",
            button {
                class: "btn",
                "data-t": "use-other-device",
                onclick: move |_| join.set(true),
                Icon { name: IconName::Device, size: 20 }
                "{t(&tr, \"devices.join\")}"
            }
            p { class: "note", "{t(&tr, \"restore.text\")}" }
            label { class: "field",
                span { class: "gt", "{t(&tr, \"devices.words\")}" }
                textarea {
                    rows: "3",
                    "data-t": "restore-words",
                    spellcheck: "false",
                    autocomplete: "off",
                    value: "{words}",
                    oninput: move |e| {
                        words.set(e.value());
                        error.set(None);
                    },
                }
            }
            button {
                class: "btn",
                "data-t": "restore-pick",
                onclick: move |_| {
                    spawn(async move {
                        if let Some(mut picked) = media::pick(media::ANY_FILE, false).await
                            && !picked.is_empty()
                        {
                            file.set(Some(picked.remove(0)));
                            error.set(None);
                        }
                    });
                },
                Icon { name: IconName::File, size: 20 }
                span { class: "one-line", "{file_label}" }
            }
            if let Some(e) = error() {
                p { class: "field-error", "{e}" }
            }
            p { class: "note", "{t(&tr, \"restore.replaces\")}" }
            button {
                class: "btn main",
                "data-t": "restore-go",
                disabled: words().split_whitespace().count() != 12,
                onclick: move |_| {
                    let bytes = match file.peek().as_ref() {
                        Some(f) => match std::fs::read(&f.path) {
                            Ok(bytes) => Some(bytes),
                            Err(_) => {
                                error.set(Some(t(&tr, "restore.err.wrong_backup")));
                                return;
                            }
                        },
                        None => None,
                    };
                    match core.restore_on_next_start(words.peek().trim(), bytes) {
                        Ok(()) => done.set(true),
                        Err(e) => error.set(Some(t(&tr, &format!("restore.err.{e}")))),
                    }
                },
                "{t(&tr, \"devices.restore\")}"
            }
        }
    }
}

#[component]
pub fn StorageScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let settings = use_settings();
    let _ = rev();
    let s = settings();
    let report = core.storage_report();
    let total = report.media + report.files + report.voice;
    let part = |n: u64| {
        (n * 100)
            .checked_div(total)
            .unwrap_or(0)
            .max(u64::from(n > 0))
    };
    let mb = |n: u64| format!("{n} MB");
    let wifi_options = vec![
        ("0".to_owned(), t(&tr, "storage.nothing")),
        ("16".to_owned(), mb(16)),
        ("100".to_owned(), mb(100)),
        ("2048".to_owned(), t(&tr, "storage.everything")),
    ];
    let mobile_options = vec![
        ("0".to_owned(), t(&tr, "storage.nothing")),
        ("2".to_owned(), mb(2)),
        ("16".to_owned(), mb(16)),
        ("100".to_owned(), mb(100)),
    ];
    let largest = opt(
        &tr,
        &[
            ("100", "settings.size_100"),
            ("500", "settings.size_500"),
            ("2048", "settings.size_2048"),
            ("10240", "settings.size_10240"),
            ("0", "settings.size_0"),
        ],
    );
    let total_text = crate::fmt::bytes(total);
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "storage.title") }
            div { class: "body",
                section { class: "panel",
                    div { style: "display:flex;align-items:baseline;gap:8px",
                        span { style: "font-size:1.75rem;line-height:1.3;font-weight:700", "{total_text}" }
                        span { class: "sub", "{t(&tr, \"storage.used\")}" }
                    }
                    div { class: "usage-bar", role: "img",
                        span { style: "flex:{part(report.media)};background:var(--primary)" }
                        span { style: "flex:{part(report.files)};background:var(--secondary)" }
                        span { style: "flex:{part(report.voice)};background:var(--success)" }
                    }
                    div { class: "chips", style: "gap:12px",
                        span { class: "mark sub",
                            span { class: "usage-key", style: "background:var(--primary)" }
                            "{t(&tr, \"storage.media\")}, {crate::fmt::bytes(report.media)}"
                        }
                        span { class: "mark sub",
                            span { class: "usage-key", style: "background:var(--secondary)" }
                            "{t(&tr, \"storage.files\")}, {crate::fmt::bytes(report.files)}"
                        }
                        span { class: "mark sub",
                            span { class: "usage-key", style: "background:var(--success)" }
                            "{t(&tr, \"storage.voice\")}, {crate::fmt::bytes(report.voice)}"
                        }
                    }
                }
                if !report.chats.is_empty() {
                    h2 { class: "gt", "{t(&tr, \"storage.by_chat\")}" }
                    div { class: "group",
                        for (chat, size) in report.chats.clone() {
                            {
                                let contact = core.contact(&chat);
                                let name = contact.as_ref().map(|c| c.display_name()).unwrap_or_default();
                                rsx! {
                                    Link {
                                        key: "{chat}",
                                        class: "row sm",
                                        to: Route::SharedMedia { id: chat.clone() },
                                        Avatar {
                                            id: chat.clone(),
                                            name: name.clone(),
                                            size: "tiny",
                                            image: contact.as_ref().and_then(|c| c.avatar.clone()),
                                        }
                                        span { class: "row-t stack one-line", "{name}" }
                                        span { class: "val", "{crate::fmt::bytes(size)}" }
                                    }
                                }
                            }
                        }
                    }
                }
                h2 { class: "gt", "{t(&tr, \"storage.on_wifi\")}" }
                Chips {
                    options: wifi_options,
                    value: s.auto_download_wifi_mb.to_string(),
                    on_change: move |v: String| {
                        let n = v.parse::<u64>().unwrap_or(0);
                        change(settings, |s| s.auto_download_wifi_mb = n);
                    },
                }
                h2 { class: "gt", "{t(&tr, \"storage.on_mobile\")}" }
                Chips {
                    options: mobile_options,
                    value: s.auto_download_mobile_mb.to_string(),
                    on_change: move |v: String| {
                        let n = v.parse::<u64>().unwrap_or(0);
                        change(settings, |s| s.auto_download_mobile_mb = n);
                    },
                }
                h2 { class: "gt", "{t(&tr, \"settings.max_file\")}" }
                Chips {
                    options: largest,
                    value: s.max_file_mb.to_string(),
                    on_change: move |v: String| {
                        let n = v.parse::<u64>().unwrap_or(2048);
                        change(settings, |s| s.max_file_mb = n);
                    },
                }
                p { class: "note", "{t(&tr, \"settings.max_file_hint\")}" }
            }
        }
    }
}

#[component]
pub fn HelpScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let net = use_context::<Net>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let mut confirm_wipe = use_signal(|| false);
    let net_key = match net() {
        whatsmine_core::NetStatus::Online => "net.status_online",
        whatsmine_core::NetStatus::Connecting => "net.connecting",
        whatsmine_core::NetStatus::Offline => "net.offline",
    };
    let my_key = core.my_key();
    let copy_key = my_key.clone();
    let wipe_core = core.clone();
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "help.title") }
            div { class: "body",
                section { class: "panel",
                    span { class: "row-t b", "WhatsMine {env!(\"CARGO_PKG_VERSION\")}" }
                    span { class: "sub", "{t(&tr, \"settings.network\")}: {t(&tr, net_key)}" }
                    span { class: "sub", "{t(&tr, \"settings.about_text\")}" }
                    span { class: "sub", "{t(&tr, \"settings.about_limits\")}" }
                }
                h2 { class: "gt", "{t(&tr, \"add.your_key\")}" }
                section { class: "panel",
                    div { class: "key-box", "{crate::fmt::grouped_key(&my_key)}" }
                    button {
                        class: "btn",
                        onclick: move |_| {
                            js::copy_text(&copy_key);
                            toast(&mut toast_sig, t(&tr, "common.copied"), ToastLevel::Info);
                        },
                        Icon { name: IconName::Copy, size: 18 }
                        "{t(&tr, \"add.copy_key\")}"
                    }
                }
                h2 { class: "gt", "{t(&tr, \"settings.danger\")}" }
                div { class: "group",
                    Row {
                        icon: IconName::Trash,
                        title: t(&tr, "settings.wipe"),
                        danger: true,
                        tag: "wipe",
                        on_tap: move |_| confirm_wipe.set(true),
                    }
                }
                p { class: "note", "{t(&tr, \"settings.wipe_hint\")}" }
            }
            if confirm_wipe() {
                ConfirmDialog {
                    title: t(&tr, "settings.wipe_title"),
                    message: t(&tr, "settings.wipe_message"),
                    confirm_label: t(&tr, "settings.wipe_confirm"),
                    on_confirm: move |_| {
                        let _ = wipe_core.wipe_everything();
                        // The old identity must not linger in memory: close the app.
                        quit();
                    },
                    on_cancel: move |_| confirm_wipe.set(false),
                }
            }
        }
    }
}
