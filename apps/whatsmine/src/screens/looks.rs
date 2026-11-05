//! Looks: the ones that are ready, the ones you made, changing one, and which one a chat has.

use dioxus::prelude::*;
use whatsmine_core::look::{
    Background, Base, Colors, Corners, Font, FontChoice, Look, MAX_LOOKS, Pattern, Rgb, contrast,
    palette,
};
use whatsmine_core::{ContactKind, ContactState};

use crate::app::{Rev, Route, ToastSig, Translations, t, toast, use_core, use_tr};
use crate::back::BackLayer;
use crate::components::{
    Avatar, Chips, ColorPicker, ConfirmDialog, Header, Icon, IconName, Row, Sheet, SheetItem,
    ToastLevel, change, use_settings,
};
use crate::look::{font_stack, look_style};
use crate::media;

/// What a look is called: a ready-made one in the language of the app.
pub fn look_name(tr: &Translations, look: &Look) -> String {
    if !look.is_ready_made() {
        return look.name.clone();
    }
    t(
        tr,
        match look.base {
            Base::System => "settings.theme_system",
            Base::Dark => "settings.theme_dark",
            Base::Light => "settings.theme_light",
            Base::Black => "settings.theme_black",
            Base::Colorblind => "settings.theme_colorblind",
            Base::ColorblindLight => "settings.theme_colorblind_light",
        },
    )
}

fn opt(tr: &Translations, pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, key)| ((*k).to_owned(), t(tr, key)))
        .collect()
}

/// A look as it comes out right now: the one that follows the device, as the device is.
fn as_shown(mut look: Look) -> Look {
    if look.base == Base::System {
        look.base = if crate::platform::system_dark() {
            Base::Dark
        } else {
            Base::Light
        };
    }
    look
}

/// A few messages in a look, to see what it does.
#[component]
pub fn LookPreview(look: Look) -> Element {
    let tr = use_tr();
    let style = look_style(&as_shown(look), true);
    // Its own name, for a rule that follows every change while the look is made.
    let id = use_hook(|| own_id("lp"));
    let preview_rule = format!(
        "{}{}",
        crate::look::rule(&format!("#{id}.look-preview"), &style.vars),
        crate::look::rule(&format!("#{id} > .chat-wall"), &style.wall)
    );
    rsx! {
        section {
            id: "{id}",
            class: "look-preview{style.class}",
            "data-t": "look-preview",
            aria_label: "{t(&tr, \"look.preview\")}",
            style { "{preview_rule}" }
            if !style.wall.is_empty() {
                div { class: "chat-wall" }
            }
            div { class: "bubble-wrap in",
                div { class: "bubble in", "{t(&tr, \"look.sample_in\")}" }
            }
            div { class: "bubble-wrap out",
                div { class: "bubble out", "{t(&tr, \"look.sample_out\")}" }
            }
        }
    }
}

/// A name of its own for an element that gets a rule of its own.
fn own_id(prefix: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    format!(
        "{prefix}{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// A look in small: a corner of chat with a bubble or two.
#[component]
fn LookMini(look: Look, tiny: bool) -> Element {
    let style = look_style(&as_shown(look), true);
    let id = use_hook(|| own_id("lm"));
    let mini_rule = format!(
        "{}{}",
        crate::look::rule(&format!("#{id}.look-mini"), &style.vars),
        crate::look::rule(&format!("#{id} > .chat-wall"), &style.wall)
    );
    rsx! {
        span {
            id: "{id}",
            class: if tiny { "look-mini tiny{style.class}" } else { "look-mini{style.class}" },
            aria_hidden: "true",
            style { "{mini_rule}" }
            if !style.wall.is_empty() {
                span { class: "chat-wall" }
            }
            if !tiny {
                span { class: "mini-in" }
            }
            span { class: "mini-out" }
        }
    }
}

/// A look as a small picture and its name.
#[component]
fn LookTile(look: Look, note: String, small: bool, on_tap: EventHandler<()>) -> Element {
    let tr = use_tr();
    let name = look_name(&tr, &look);
    rsx! {
        button {
            class: if small { "look-tile small" } else { "look-tile" },
            "data-t": "look-tile",
            onclick: move |_| on_tap.call(()),
            LookMini { look, tiny: false }
            span { class: "look-tile-text",
                span { class: "b one-line", "{name}" }
                if !note.is_empty() {
                    span { class: "small sub one-line", "{note}" }
                }
            }
        }
    }
}

#[component]
pub fn LooksScreen() -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let rev = use_context::<Rev>().0;
    let settings = use_settings();
    let mut toast_sig = use_context::<ToastSig>();
    let mut acting: Signal<Option<Look>> = use_signal(|| None);
    let _ = rev();
    let s = settings();
    let mine = core.looks();
    let note_for = |look: &Look| {
        if s.app_look == look.id {
            t(&tr, "look.is_app")
        } else if s.chat_look == look.id {
            t(&tr, "look.is_default")
        } else if let Some(from) = &look.from {
            t(&tr, "look.from").replace("{name}", from)
        } else {
            match core.chats_using_look(&look.id) {
                0 => String::new(),
                1 => t(&tr, "look.in_one_chat"),
                used => t(&tr, "look.in_chats").replace("{n}", &used.to_string()),
            }
        }
    };
    let new_core = core.clone();
    let full = mine.len() >= MAX_LOOKS;
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "look.yours") }
            div { class: "body",
                h2 { class: "gt", style: "padding-top:0", "{t(&tr, \"look.own\")}" }
                div { class: "look-grid",
                    for look in mine.clone() {
                        LookTile {
                            key: "{look.id}",
                            note: note_for(&look),
                            small: false,
                            look: look.clone(),
                            on_tap: move |_| acting.set(Some(look.clone())),
                        }
                    }
                    if !full {
                        button {
                            class: "look-tile new",
                            "data-t": "look-new",
                            onclick: move |_| {
                                let base = new_core.app_look();
                                let look = base.copied(new_core.new_look_id(), t(&tr, "look.new_name"));
                                match new_core.save_look(&look) {
                                    Ok(()) => {
                                        nav.push(Route::LookEdit { id: look.id });
                                    }
                                    Err(_) => toast(&mut toast_sig, t(&tr, "look.err_save"), ToastLevel::Error),
                                }
                            },
                            Icon { name: IconName::Plus }
                            "{t(&tr, \"look.new\")}"
                        }
                    }
                }
                h2 { class: "gt", "{t(&tr, \"look.ready\")}" }
                div { class: "look-grid three",
                    for look in Look::ready_made() {
                        LookTile {
                            key: "{look.id}",
                            note: String::new(),
                            small: true,
                            look: look.clone(),
                            on_tap: move |_| acting.set(Some(look.clone())),
                        }
                    }
                }
            }
            if let Some(look) = acting() {
                LookActions { look, on_close: move |_| acting.set(None) }
            }
        }
    }
}

/// What can be done with a look.
#[component]
fn LookActions(look: Look, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let nav = use_navigator();
    let settings = use_settings();
    let mut toast_sig = use_context::<ToastSig>();
    let mut sharing = use_signal(|| false);
    let mut deleting = use_signal(|| false);
    let s = settings();
    let name = look_name(&tr, &look);
    let ready = look.is_ready_made();
    let is_default = s.chat_look == look.id;
    let (app_id, default_id, edit_id, copy, share, delete_id) = (
        look.id.clone(),
        look.id.clone(),
        look.id.clone(),
        look.clone(),
        look.clone(),
        look.id.clone(),
    );
    let (copy_core, delete_core) = (core.clone(), core.clone());
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: name.clone(), tag: "look-actions",
            div { class: "sheet-list",
                SheetItem {
                    label: t(&tr, "look.use_app"),
                    icon: IconName::Palette,
                    tag: "look-use-app",
                    on_tap: move |_| {
                        let id = app_id.clone();
                        change(settings, |s| s.app_look = id);
                        on_close.call(());
                    },
                }
                SheetItem {
                    label: t(&tr, if is_default { "look.stop_default" } else { "look.use_default" }),
                    icon: IconName::Chat,
                    tag: "look-use-default",
                    on_tap: move |_| {
                        let id = if is_default { String::new() } else { default_id.clone() };
                        change(settings, |s| s.chat_look = id);
                        on_close.call(());
                    },
                }
                if !ready {
                    SheetItem {
                        label: t(&tr, "look.change"),
                        icon: IconName::Pencil,
                        tag: "look-change",
                        on_tap: move |_| {
                            on_close.call(());
                            nav.push(Route::LookEdit { id: edit_id.clone() });
                        },
                    }
                }
                SheetItem {
                    label: t(&tr, "look.copy"),
                    icon: IconName::Copy,
                    tag: "look-copy",
                    on_tap: move |_| {
                        let name = if copy.is_ready_made() {
                            t(&tr, "look.new_name")
                        } else {
                            t(&tr, "look.copy_of").replace("{name}", &copy.name)
                        };
                        let new = copy.copied(copy_core.new_look_id(), name);
                        match copy_core.save_look(&new) {
                            Ok(()) => {
                                on_close.call(());
                                nav.push(Route::LookEdit { id: new.id });
                            }
                            Err(_) => toast(&mut toast_sig, t(&tr, "look.err_save"), ToastLevel::Error),
                        }
                    },
                }
                if !ready {
                    SheetItem {
                        label: t(&tr, "look.share"),
                        icon: IconName::Share,
                        tag: "look-share",
                        on_tap: move |_| sharing.set(true),
                    }
                    SheetItem {
                        label: t(&tr, "common.delete"),
                        icon: IconName::Trash,
                        danger: true,
                        tag: "look-delete",
                        on_tap: move |_| deleting.set(true),
                    }
                }
            }
        }
        if sharing() {
            ShareLookSheet { look: share.clone(), on_close: move |_| { sharing.set(false); on_close.call(()); } }
        }
        if deleting() {
            ConfirmDialog {
                title: t(&tr, "look.delete_title").replace("{name}", &name),
                message: t(&tr, "look.delete_text"),
                confirm_label: t(&tr, "common.delete"),
                on_confirm: move |_| {
                    delete_core.delete_look(&delete_id);
                    deleting.set(false);
                    on_close.call(());
                },
                on_cancel: move |_| deleting.set(false),
            }
        }
    }
}

/// Who gets a copy.
#[component]
fn ShareLookSheet(look: Look, on_close: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let chats: Vec<_> = core
        .chats()
        .into_iter()
        .filter(|c| {
            c.contact.state == ContactState::Accepted || c.contact.kind == ContactKind::Group
        })
        .collect();
    rsx! {
        Sheet { on_close: move |_| on_close.call(()), title: t(&tr, "look.share_to"), tag: "look-share-sheet",
            div { class: "sheet-list",
                for chat in chats {
                    {
                        let (id, core, look) = (chat.contact.id.clone(), core.clone(), look.clone());
                        rsx! {
                            button {
                                key: "{chat.contact.id}",
                                class: "sheet-item",
                                "data-t": "share-to",
                                onclick: move |_| {
                                    let (id, core, look) = (id.clone(), core.clone(), look.clone());
                                    on_close.call(());
                                    dioxus::core::spawn_forever(async move {
                                        let sent = tokio::task::spawn_blocking(move || core.share_look(&id, &look).is_ok()).await;
                                        let key = if matches!(sent, Ok(true)) { "look.shared" } else { "look.err_share" };
                                        toast(&mut toast_sig, t(&tr, key), ToastLevel::Info);
                                    });
                                },
                                Avatar { id: chat.contact.id.clone(), name: chat.contact.display_name(), size: "tiny", image: chat.contact.avatar.clone() }
                                span { class: "one-line", "{chat.contact.display_name()}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Which colour is being chosen.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    Background,
    Cards,
    Accent,
    Text,
    MyBubble,
    MyText,
    TheirBubble,
    TheirText,
    WallColor,
    WallFrom,
    WallTo,
}

impl Slot {
    const LOOK: [Self; 8] = [
        Self::Background,
        Self::Cards,
        Self::Accent,
        Self::Text,
        Self::MyBubble,
        Self::MyText,
        Self::TheirBubble,
        Self::TheirText,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Background => "look.c_background",
            Self::Cards => "look.c_cards",
            Self::Accent => "look.c_accent",
            Self::Text => "look.c_text",
            Self::MyBubble => "look.c_my_bubble",
            Self::MyText => "look.c_my_text",
            Self::TheirBubble => "look.c_their_bubble",
            Self::TheirText => "look.c_their_text",
            Self::WallColor => "look.hue",
            Self::WallFrom => "look.c_wall_from",
            Self::WallTo => "look.c_wall_to",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Background => "c-background",
            Self::Cards => "c-cards",
            Self::Accent => "c-accent",
            Self::Text => "c-text",
            Self::MyBubble => "c-my-bubble",
            Self::MyText => "c-my-text",
            Self::TheirBubble => "c-their-bubble",
            Self::TheirText => "c-their-text",
            Self::WallColor | Self::WallFrom => "c-wall-from",
            Self::WallTo => "c-wall-to",
        }
    }

    /// The chosen colour, if there is one.
    fn chosen(self, look: &Look) -> Option<Rgb> {
        let c = &look.colors;
        match self {
            Self::Background => c.background,
            Self::Cards => c.cards,
            Self::Accent => c.accent,
            Self::Text => c.text,
            Self::MyBubble => c.my_bubble,
            Self::MyText => c.my_text,
            Self::TheirBubble => c.their_bubble,
            Self::TheirText => c.their_text,
            Self::WallColor => match look.background {
                Background::Color(color) => Some(color),
                _ => None,
            },
            Self::WallFrom => match look.background {
                Background::Gradient { from, .. } => Some(from),
                _ => None,
            },
            Self::WallTo => match look.background {
                Background::Gradient { to, .. } => Some(to),
                _ => None,
            },
        }
    }

    /// What it is at the moment, chosen or not.
    fn showing(self, look: &Look) -> Rgb {
        let p = palette(look);
        self.chosen(look).unwrap_or(match self {
            Self::Background | Self::WallColor | Self::WallFrom | Self::WallTo => p.bg,
            Self::Cards => p.surface,
            Self::Accent => p.primary,
            Self::Text => p.text,
            Self::MyBubble => p.bubble_out,
            Self::MyText => p.bubble_out_text,
            Self::TheirBubble => p.bubble_in,
            Self::TheirText => p.bubble_in_text,
        })
    }

    fn set(self, look: &mut Look, color: Option<Rgb>) {
        let c: &mut Colors = &mut look.colors;
        match self {
            Self::Background => c.background = color,
            Self::Cards => c.cards = color,
            Self::Accent => c.accent = color,
            Self::Text => c.text = color,
            Self::MyBubble => c.my_bubble = color,
            Self::MyText => c.my_text = color,
            Self::TheirBubble => c.their_bubble = color,
            Self::TheirText => c.their_text = color,
            Self::WallColor => {
                if let Some(color) = color {
                    look.background = Background::Color(color);
                }
            }
            Self::WallFrom => {
                if let (Some(color), Background::Gradient { from, .. }) =
                    (color, &mut look.background)
                {
                    *from = color;
                }
            }
            Self::WallTo => {
                if let (Some(color), Background::Gradient { to, .. }) =
                    (color, &mut look.background)
                {
                    *to = color;
                }
            }
        }
    }

    /// For text someone picks: what it has to read on.
    fn text_on(self, look: &Look) -> Option<(Rgb, Rgb)> {
        let p = palette(look);
        match self {
            Self::Text => Some((p.text, p.bg)),
            Self::MyText => Some((p.bubble_out_text, p.bubble_out)),
            Self::TheirText => Some((p.bubble_in_text, p.bubble_in)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Main,
    Color(Slot),
    Fonts,
    Background,
    Shapes,
}

#[component]
pub fn LookEdit(id: String) -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let mut toast_sig = use_context::<ToastSig>();
    let settings = use_settings();
    let mut draft = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.look(&id))
    };
    let mut part = use_signal(|| Part::Main);
    let Some(look) = draft() else {
        return rsx! { div { class: "screen no-nav", Header { title: t(&tr, "look.yours") } } };
    };
    let title = match part() {
        Part::Main => look_name(&tr, &look),
        Part::Color(slot) => t(&tr, slot.key()),
        Part::Fonts => t(&tr, "look.fonts"),
        Part::Background => t(&tr, "look.background"),
        Part::Shapes => t(&tr, "look.shapes"),
    };
    let used = core.chats_using_look(&look.id);
    let used_note = t(&tr, "look.used_note").replace("{n}", &used.to_string());
    let bases = opt(
        &tr,
        &[
            ("dark", "settings.theme_dark"),
            ("light", "settings.theme_light"),
            ("black", "settings.theme_black"),
            ("colorblind", "settings.theme_colorblind"),
            ("colorblind-light", "settings.theme_colorblind_light"),
        ],
    );
    let save_core = core.clone();
    let copy_core = core.clone();
    let inner = match part() {
        Part::Color(slot) => rsx! {
            BackLayer { on_back: move |_| part.set(Part::Main) }
            ColorPart { slot, draft, on_done: move |_| part.set(Part::Main) }
        },
        Part::Fonts => rsx! {
            BackLayer { on_back: move |_| part.set(Part::Main) }
            FontsPart { draft }
        },
        Part::Background => rsx! {
            BackLayer { on_back: move |_| part.set(Part::Main) }
            BackgroundPart { draft, on_color: move |slot| part.set(Part::Color(slot)) }
        },
        Part::Shapes => rsx! {
            BackLayer { on_back: move |_| part.set(Part::Main) }
            ShapesPart { draft }
        },
        Part::Main => rsx! {
            label { class: "group row sm",
                span { class: "row-t", style: "flex:none", "{t(&tr, \"look.name\")}" }
                input {
                    class: "plain-input",
                    "data-t": "look-name",
                    maxlength: "40",
                    value: "{look.name}",
                    oninput: move |e| {
                        if let Some(l) = draft.write().as_mut() {
                            l.name = e.value();
                        }
                    },
                }
            }
            h2 { class: "gt", "{t(&tr, \"look.starts_from\")}" }
            Chips {
                options: bases,
                value: look.base.key().to_owned(),
                on_change: move |v: String| {
                    if let Some(l) = draft.write().as_mut() {
                        l.base = match v.as_str() {
                            "light" => Base::Light,
                            "black" => Base::Black,
                            "colorblind" => Base::Colorblind,
                            "colorblind-light" => Base::ColorblindLight,
                            _ => Base::Dark,
                        };
                    }
                },
            }
            h2 { class: "gt", "{t(&tr, \"look.colors\")}" }
            div { class: "group",
                for slot in Slot::LOOK {
                    {
                        let color = slot.showing(&look).hex();
                        let auto = slot.chosen(&look).is_none();
                        rsx! {
                            button {
                                key: "{slot.tag()}",
                                class: "row sm",
                                "data-t": "{slot.tag()}",
                                onclick: move |_| part.set(Part::Color(slot)),
                                span { class: "color-dot", style: "background:{color}", aria_hidden: "true" }
                                span { class: "row-t stack", "{t(&tr, slot.key())}" }
                                if auto {
                                    span { class: "val", "{t(&tr, \"look.auto\")}" }
                                }
                                span { class: "dim", style: "display:flex",
                                    Icon { name: IconName::ChevronRight, size: 16 }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "group",
                Row { icon: IconName::TextSize, title: t(&tr, "look.fonts"), small: true, tag: "look-fonts", on_tap: move |_| part.set(Part::Fonts) }
                Row { icon: IconName::Image, title: t(&tr, "look.background"), small: true, tag: "look-background", on_tap: move |_| part.set(Part::Background) }
                Row { icon: IconName::Sliders, title: t(&tr, "look.shapes"), small: true, tag: "look-shapes", on_tap: move |_| part.set(Part::Shapes) }
            }
            p { class: "note", "{t(&tr, \"look.readable_note\")}" }
            if used > 1 {
                p { class: "note", "{used_note}" }
            }
            div { class: "btns",
                if used > 0 {
                    button {
                        class: "btn",
                        "data-t": "look-save-copy",
                        onclick: move |_| {
                            let Some(l) = draft.peek().clone() else { return };
                            let name = l.name.clone();
                            let copy = l.copied(copy_core.new_look_id(), name);
                            if copy_core.save_look(&copy).is_err() {
                                toast(&mut toast_sig, t(&tr, "look.err_save"), ToastLevel::Error);
                            } else {
                                nav.go_back();
                            }
                        },
                        "{t(&tr, \"look.save_copy\")}"
                    }
                }
                button {
                    class: "btn main",
                    "data-t": "look-save",
                    onclick: move |_| {
                        let Some(l) = draft.peek().clone() else { return };
                        if save_core.save_look(&l).is_err() {
                            toast(&mut toast_sig, t(&tr, "look.err_save"), ToastLevel::Error);
                        } else {
                            change(settings, |_| {});
                            nav.go_back();
                        }
                    },
                    "{t(&tr, \"common.save\")}"
                }
            }
        },
    };
    rsx! {
        div { class: "screen no-nav",
            header { class: "hdr",
                button {
                    class: "ib back-btn",
                    "data-t": "back",
                    aria_label: "{t(&tr, \"common.back\")}",
                    onclick: move |_| {
                        if part() == Part::Main {
                            nav.go_back();
                        } else {
                            part.set(Part::Main);
                        }
                    },
                    Icon { name: IconName::ArrowLeft }
                }
                h1 { "{title}" }
            }
            div { class: "body",
                LookPreview { look: look.clone() }
                {inner}
            }
        }
    }
}

#[component]
fn ColorPart(slot: Slot, draft: Signal<Option<Look>>, on_done: EventHandler<()>) -> Element {
    let tr = use_tr();
    let settings = use_settings();
    let Some(look) = draft() else {
        return rsx! {};
    };
    let value = slot.showing(&look);
    let can_be_auto = Slot::LOOK.contains(&slot);
    let recent: Vec<Rgb> = settings().recent_colors;
    // Someone may choose text that reads badly. It stays theirs, with a word about it.
    let weak = slot
        .chosen(&look)
        .and(slot.text_on(&look))
        .is_some_and(|(text, back)| contrast(text, back) < 4.5);
    rsx! {
        ColorPicker {
            value,
            recent,
            on_change: move |color| {
                if let Some(l) = draft.write().as_mut() {
                    slot.set(l, Some(color));
                }
            },
        }
        if weak {
            div { class: "notice warn still", "data-t": "look-weak",
                Icon { name: IconName::Warning }
                span { class: "stack", "{t(&tr, \"look.weak\")}" }
                button {
                    class: "btn small-btn",
                    "data-t": "look-fix",
                    onclick: move |_| {
                        if let Some(l) = draft.write().as_mut() {
                            slot.set(l, None);
                        }
                    },
                    "{t(&tr, \"look.fix\")}"
                }
            }
        }
        div { class: "btns",
            if can_be_auto {
                button {
                    class: "btn",
                    "data-t": "color-auto",
                    onclick: move |_| {
                        if let Some(l) = draft.write().as_mut() {
                            slot.set(l, None);
                        }
                        on_done.call(());
                    },
                    "{t(&tr, \"look.auto\")}"
                }
            }
            button {
                class: "btn main",
                "data-t": "color-done",
                onclick: move |_| {
                    if let Some(color) = draft.peek().as_ref().and_then(|l| slot.chosen(l)) {
                        change(settings, |s| {
                            s.recent_colors.retain(|c| *c != color);
                            s.recent_colors.insert(0, color);
                            s.recent_colors.truncate(8);
                        });
                    }
                    on_done.call(());
                },
                "{t(&tr, \"common.done\")}"
            }
        }
    }
}

#[component]
fn FontsPart(draft: Signal<Option<Look>>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let mut for_messages = use_signal(|| true);
    let _ = rev();
    let Some(look) = draft() else {
        return rsx! {};
    };
    let current = if for_messages() {
        look.message_font.clone()
    } else {
        look.app_font.clone()
    };
    let mut set = move |font: FontChoice| {
        if let Some(l) = draft.write().as_mut() {
            if for_messages() {
                l.message_font = font;
            } else {
                l.app_font = font;
            }
        }
    };
    let mut choices: Vec<(FontChoice, String)> =
        vec![(FontChoice::System, t(&tr, "look.font_system"))];
    for font in Font::ALL {
        choices.push((FontChoice::Bundled(font), font.name().to_owned()));
    }
    for own in core.own_fonts() {
        choices.push((FontChoice::Own(own.id), own.name));
    }
    let add_core = core.clone();
    rsx! {
        div { class: "chips", role: "radiogroup",
            button {
                class: if for_messages() { "chip" } else { "chip on" },
                "data-t": "font-app",
                onclick: move |_| for_messages.set(false),
                "{t(&tr, \"look.font_app\")}"
            }
            button {
                class: if for_messages() { "chip on" } else { "chip" },
                "data-t": "font-messages",
                onclick: move |_| for_messages.set(true),
                "{t(&tr, \"look.font_messages\")}"
            }
        }
        div { class: "group", role: "radiogroup",
            for (i, (font, name)) in choices.into_iter().enumerate() {
                {
                    let on = font == current;
                    let family = font_stack(&font).unwrap_or_else(|| "inherit".to_owned());
                    rsx! {
                        button {
                            key: "{i}",
                            class: "row sm",
                            role: "radio",
                            aria_checked: "{on}",
                            "data-t": "font-choice",
                            onclick: move |_| set(font.clone()),
                            span { class: "row-t stack", style: "font-family:{family}", "{name}" }
                            if on {
                                span { class: "accent-text", style: "display:flex",
                                    Icon { name: IconName::Check, size: 20 }
                                }
                            }
                        }
                    }
                }
            }
        }
        div { class: "group",
            Row {
                icon: IconName::File,
                title: t(&tr, "look.font_file"),
                note: t(&tr, "look.font_file_note"),
                tag: "font-file",
                on_tap: move |_| {
                    let core = add_core.clone();
                    spawn(async move {
                        let Some(files) = media::pick(media::ANY_FILE, false).await else {
                            toast(&mut toast_sig, t(&tr, "attach.unavailable"), ToastLevel::Warning);
                            return;
                        };
                        let Some(file) = files.into_iter().next() else {
                            return;
                        };
                        let added = tokio::task::spawn_blocking(move || core.add_own_font(&file.path, &file.name)).await;
                        if !matches!(added, Ok(Ok(_))) {
                            toast(&mut toast_sig, t(&tr, "look.err_font"), ToastLevel::Error);
                        }
                    });
                },
            }
        }
        label { class: "group row sm",
            span { class: "row-t", style: "flex:none", "{t(&tr, \"look.message_size\")}" }
            input {
                r#type: "range",
                min: "80",
                max: "150",
                "data-t": "message-size",
                value: "{look.message_size}",
                style: "flex:1;min-width:0",
                oninput: move |e| {
                    if let Some(l) = draft.write().as_mut() {
                        l.message_size = e.value().parse().unwrap_or(100);
                    }
                },
            }
        }
    }
}

#[component]
fn BackgroundPart(draft: Signal<Option<Look>>, on_color: EventHandler<Slot>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut toast_sig = use_context::<ToastSig>();
    let Some(look) = draft() else {
        return rsx! {};
    };
    let kind = match &look.background {
        Background::None => "none",
        Background::Pattern(_) => "pattern",
        Background::Color(_) => "color",
        Background::Gradient { .. } => "gradient",
        Background::Photo(_) => "photo",
    };
    let kinds = opt(
        &tr,
        &[
            ("none", "look.none"),
            ("pattern", "look.pattern"),
            ("color", "look.one_color"),
            ("gradient", "look.two_colors"),
            ("photo", "look.photo"),
        ],
    );
    let bg = palette(&look).bg;
    let mut set = move |background: Background| {
        if let Some(l) = draft.write().as_mut() {
            l.background = background;
        }
    };
    let photo_core = core.clone();
    let choose_photo = move || {
        let core = photo_core.clone();
        spawn(async move {
            let Some(files) = media::pick(media::IMAGES, false).await else {
                toast(
                    &mut toast_sig,
                    t(&tr, "attach.unavailable"),
                    ToastLevel::Warning,
                );
                return;
            };
            let Some(file) = files.into_iter().next() else {
                return;
            };
            let stored =
                tokio::task::spawn_blocking(move || core.store_look_photo(&file.path)).await;
            match stored {
                Ok(Ok(id)) => {
                    if let Some(l) = draft.write().as_mut() {
                        l.background = Background::Photo(id);
                    }
                }
                _ => toast(&mut toast_sig, t(&tr, "look.err_photo"), ToastLevel::Error),
            }
        });
    };
    let choose_for_tap = choose_photo.clone();
    let choose_for_row = choose_photo;
    rsx! {
        Chips {
            options: kinds,
            value: kind.to_owned(),
            on_change: move |v: String| match v.as_str() {
                "pattern" => set(Background::Pattern(Pattern::Dots)),
                "color" => set(Background::Color(bg)),
                "gradient" => set(Background::Gradient {
                    from: Rgb::new(0x1f_5a_4d),
                    to: Rgb::new(0x8f_c3_e8),
                    angle: 160,
                }),
                "photo" => choose_for_tap(),
                _ => set(Background::None),
            },
        }
        if let Background::Pattern(pattern) = look.background {
            Chips {
                options: opt(&tr, &[("dots", "look.dots"), ("waves", "look.waves")]),
                value: if pattern == Pattern::Waves { "waves".to_owned() } else { "dots".to_owned() },
                on_change: move |v: String| {
                    set(Background::Pattern(if v == "waves" { Pattern::Waves } else { Pattern::Dots }));
                },
            }
        }
        if let Background::Color(color) = look.background {
            div { class: "group",
                button {
                    class: "row sm",
                    "data-t": "c-wall-from",
                    onclick: move |_| on_color.call(Slot::WallColor),
                    span { class: "color-dot", style: "background:{color.hex()}", aria_hidden: "true" }
                    span { class: "row-t stack", "{t(&tr, \"look.hue\")}" }
                }
            }
        }
        if let Background::Gradient { from, to, angle } = look.background {
            div { class: "group",
                button {
                    class: "row sm",
                    "data-t": "c-wall-from",
                    onclick: move |_| on_color.call(Slot::WallFrom),
                    span { class: "color-dot", style: "background:{from.hex()}", aria_hidden: "true" }
                    span { class: "row-t stack", "{t(&tr, \"look.c_wall_from\")}" }
                }
                button {
                    class: "row sm",
                    "data-t": "c-wall-to",
                    onclick: move |_| on_color.call(Slot::WallTo),
                    span { class: "color-dot", style: "background:{to.hex()}", aria_hidden: "true" }
                    span { class: "row-t stack", "{t(&tr, \"look.c_wall_to\")}" }
                }
                label { class: "row sm",
                    span { class: "row-t", style: "flex:none", "{t(&tr, \"look.angle\")}" }
                    input {
                        r#type: "range",
                        min: "0",
                        max: "360",
                        value: "{angle}",
                        style: "flex:1;min-width:0",
                        oninput: move |e| {
                            let turn = e.value().parse().unwrap_or(0);
                            set(Background::Gradient { from, to, angle: turn });
                        },
                    }
                }
            }
        }
        if kind == "photo" {
            div { class: "group",
                Row { icon: IconName::Image, title: t(&tr, "look.photo_other"), small: true, tag: "look-photo", on_tap: move |_| choose_for_row() }
            }
        }
        if kind != "none" {
            div { class: "group",
                label { class: "row sm",
                    span { class: "row-t", style: "flex:none", "{t(&tr, \"look.dim\")}" }
                    input {
                        r#type: "range",
                        min: "0",
                        max: "90",
                        "data-t": "look-dim",
                        value: "{look.dim}",
                        style: "flex:1;min-width:0",
                        oninput: move |e| {
                            if let Some(l) = draft.write().as_mut() {
                                l.dim = e.value().parse().unwrap_or(0);
                            }
                        },
                    }
                }
                label { class: "row sm",
                    span { class: "row-t", style: "flex:none", "{t(&tr, \"look.blur\")}" }
                    input {
                        r#type: "range",
                        min: "0",
                        max: "20",
                        "data-t": "look-blur",
                        value: "{look.blur}",
                        style: "flex:1;min-width:0",
                        oninput: move |e| {
                            if let Some(l) = draft.write().as_mut() {
                                l.blur = e.value().parse().unwrap_or(0);
                            }
                        },
                    }
                }
            }
        }
        if kind == "photo" {
            p { class: "note", "{t(&tr, \"look.photo_note\")}" }
        }
    }
}

#[component]
fn ShapesPart(draft: Signal<Option<Look>>) -> Element {
    let tr = use_tr();
    let Some(look) = draft() else {
        return rsx! {};
    };
    let corner_key = |c: Corners| match c {
        Corners::Round => "round",
        Corners::Soft => "soft",
        Corners::Square => "square",
    };
    let corner = |v: &str| match v {
        "soft" => Corners::Soft,
        "square" => Corners::Square,
        _ => Corners::Round,
    };
    let corners = opt(
        &tr,
        &[
            ("round", "look.round"),
            ("soft", "look.soft_corners"),
            ("square", "look.square"),
        ],
    );
    rsx! {
        h2 { class: "gt", "{t(&tr, \"look.bubble_corners\")}" }
        Chips {
            options: corners.clone(),
            value: corner_key(look.bubble_corners).to_owned(),
            on_change: move |v: String| {
                if let Some(l) = draft.write().as_mut() {
                    l.bubble_corners = corner(&v);
                }
            },
        }
        h2 { class: "gt", "{t(&tr, \"look.messages\")}" }
        Chips {
            options: opt(&tr, &[("bubbles", "look.bubbles"), ("flat", "look.flat")]),
            value: if look.flat { "flat".to_owned() } else { "bubbles".to_owned() },
            on_change: move |v: String| {
                if let Some(l) = draft.write().as_mut() {
                    l.flat = v == "flat";
                }
            },
        }
        h2 { class: "gt", "{t(&tr, \"look.app_corners\")}" }
        Chips {
            options: corners,
            value: corner_key(look.app_corners).to_owned(),
            on_change: move |v: String| {
                if let Some(l) = draft.write().as_mut() {
                    l.app_corners = corner(&v);
                }
            },
        }
        h2 { class: "gt", "{t(&tr, \"look.people_photos\")}" }
        Chips {
            options: opt(&tr, &[("circle", "look.circle"), ("square", "look.rounded_square")]),
            value: if look.square_photos { "square".to_owned() } else { "circle".to_owned() },
            on_change: move |v: String| {
                if let Some(l) = draft.write().as_mut() {
                    l.square_photos = v == "square";
                }
            },
        }
    }
}

/// The look of one chat.
#[component]
pub fn ChatLook(id: String) -> Element {
    let tr = use_tr();
    crate::js::use_scroll_top();
    let core = use_core();
    let nav = use_navigator();
    let rev = use_context::<Rev>().0;
    let settings = use_settings();
    let mut toast_sig = use_context::<ToastSig>();
    let mut prefs = {
        let (core, id) = (core.clone(), id.clone());
        use_signal(move || core.chat_prefs(&id))
    };
    let _ = rev();
    let s = settings();
    let own = prefs.read().look.clone();
    let shown = core.chat_look(&id).unwrap_or_else(|| core.app_look());
    let default_name = core
        .look(&s.chat_look)
        .map(|l| look_name(&tr, &l))
        .unwrap_or_else(|| t(&tr, "look.same_as_app"));
    let mut looks = core.looks();
    // Following the device is something only the app as a whole can do.
    looks.extend(
        Look::ready_made()
            .into_iter()
            .filter(|l| l.base != Base::System),
    );
    let name = core
        .contact(&id)
        .map(|c| c.display_name())
        .unwrap_or_default();
    let set_core = core.clone();
    let share_chat = id.clone();
    let set = move |look: String| {
        let mut next = prefs.peek().clone();
        next.look = look;
        set_core.set_chat_prefs(&id, &next);
        prefs.set(next);
    };
    let mut set_default = set.clone();
    let (make_core, share_look, share_core, share_id) =
        (core.clone(), shown.clone(), core.clone(), share_chat);
    rsx! {
        div { class: "screen no-nav",
            Header { title: t(&tr, "look.this_chat") }
            div { class: "body",
                LookPreview { look: shown.clone() }
                div { class: "group", role: "radiogroup",
                    button {
                        class: "row sm",
                        role: "radio",
                        aria_checked: "{own.is_empty()}",
                        "data-t": "chat-look-default",
                        onclick: move |_| set_default(String::new()),
                        span { class: "stack",
                            span { class: "row-t", "{t(&tr, \"look.same_as_chats\")}" }
                            span { class: "sub", "{default_name}" }
                        }
                        if own.is_empty() {
                            span { class: "accent-text", style: "display:flex",
                                Icon { name: IconName::Check, size: 20 }
                            }
                        }
                    }
                    for look in looks {
                        {
                            let on = own == look.id;
                            let label = look_name(&tr, &look);
                            let mut set = set.clone();
                            let choose = look.id.clone();
                            rsx! {
                                button {
                                    key: "{look.id}",
                                    class: "row sm",
                                    role: "radio",
                                    aria_checked: "{on}",
                                    "data-t": "chat-look-choice",
                                    onclick: move |_| set(choose.clone()),
                                    LookMini { look: look.clone(), tiny: true }
                                    span { class: "row-t stack one-line", "{label}" }
                                    if on {
                                        span { class: "accent-text", style: "display:flex",
                                            Icon { name: IconName::Check, size: 20 }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "group",
                    Row {
                        icon: IconName::Plus,
                        title: t(&tr, "look.make_from"),
                        small: true,
                        tag: "chat-look-new",
                        on_tap: move |_| {
                            let copy = shown.copied(make_core.new_look_id(), t(&tr, "look.new_name"));
                            match make_core.save_look(&copy) {
                                Ok(()) => {
                                    nav.push(Route::LookEdit { id: copy.id });
                                }
                                Err(_) => toast(&mut toast_sig, t(&tr, "look.err_save"), ToastLevel::Error),
                            }
                        },
                    }
                    if !share_look.is_ready_made() {
                        Row {
                            icon: IconName::Share,
                            title: t(&tr, "look.send_to").replace("{name}", &name),
                            small: true,
                            tag: "chat-look-send",
                            on_tap: move |_| {
                                let (core, id, look) = (share_core.clone(), share_id.clone(), share_look.clone());
                                dioxus::core::spawn_forever(async move {
                                    let sent = tokio::task::spawn_blocking(move || core.share_look(&id, &look).is_ok()).await;
                                    let key = if matches!(sent, Ok(true)) { "look.shared" } else { "look.err_share" };
                                    toast(&mut toast_sig, t(&tr, key), ToastLevel::Info);
                                });
                            },
                        }
                    }
                }
                p { class: "note", "{t(&tr, \"look.only_you\")}" }
            }
        }
    }
}

/// A look someone sent, in a chat.
#[component]
pub fn SharedLookCard(message_id: String, look: Look, outgoing: bool, chat: String) -> Element {
    let tr = use_tr();
    let core = use_core();
    let rev = use_context::<Rev>().0;
    let mut toast_sig = use_context::<ToastSig>();
    let _ = rev();
    let kept = core
        .looks()
        .into_iter()
        .find(|l| l.from_message.as_deref() == Some(message_id.as_str()));
    let mut shown = look.clone();
    // The picture travels as the message's file: until it is here there is nothing to show.
    if matches!(shown.background, Background::Photo(_)) {
        shown.background = Background::None;
    }
    let (use_core_, keep_core, use_id, keep_id) = (
        core.clone(),
        core.clone(),
        message_id.clone(),
        message_id.clone(),
    );
    rsx! {
        div { class: "shared-look", "data-t": "shared-look",
            LookPreview { look: shown }
            span { class: "b", "{look.name}" }
            span { class: "small", style: "opacity:.8", "{t(&tr, \"look.card_note\")}" }
            if !outgoing {
                div { class: "btns",
                    button {
                        class: "btn main",
                        "data-t": "look-use-here",
                        onclick: move |_| {
                            let (core, id, chat) = (use_core_.clone(), use_id.clone(), chat.clone());
                            spawn(async move {
                                let taken = {
                                    let core = core.clone();
                                    tokio::task::spawn_blocking(move || core.keep_shared_look(&id)).await
                                };
                                match taken {
                                    Ok(Ok(look)) => {
                                        let mut prefs = core.chat_prefs(&chat);
                                        prefs.look = look.id;
                                        core.set_chat_prefs(&chat, &prefs);
                                    }
                                    _ => toast(&mut toast_sig, t(&tr, "look.err_save"), ToastLevel::Error),
                                }
                            });
                        },
                        "{t(&tr, \"look.use_here\")}"
                    }
                    button {
                        class: "btn",
                        "data-t": "look-keep",
                        disabled: kept.is_some(),
                        onclick: move |_| {
                            let (core, id) = (keep_core.clone(), keep_id.clone());
                            spawn(async move {
                                let taken = tokio::task::spawn_blocking(move || core.keep_shared_look(&id)).await;
                                let key = if matches!(taken, Ok(Ok(_))) { "look.kept" } else { "look.err_save" };
                                toast(&mut toast_sig, t(&tr, key), ToastLevel::Info);
                            });
                        },
                        if kept.is_some() {
                            "{t(&tr, \"look.kept_short\")}"
                        } else {
                            "{t(&tr, \"look.keep\")}"
                        }
                    }
                }
            }
        }
    }
}
