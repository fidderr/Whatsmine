use dioxus::prelude::*;

use crate::app::{ToastSig, t, toast, use_core, use_tr};
use crate::components::{
    Avatar, CameraMode, CameraOverlay, Icon, IconName, Row, Sheet, SheetItem, ToastLevel, change,
    use_settings,
};
use crate::health;
use crate::media::{self, Picked};

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Welcome,
    Mode,
    Allow,
}

/// The first run: a name, how you want it, and what the phone has to allow.
#[component]
pub fn Start() -> Element {
    let mut step = use_signal(|| Step::Welcome);
    match step() {
        Step::Welcome => rsx! { Welcome { on_next: move |_| step.set(Step::Mode) } },
        Step::Mode => rsx! { Mode { on_next: move |_| step.set(Step::Allow) } },
        Step::Allow => rsx! { Allow {} },
    }
}

#[component]
fn Welcome(on_next: EventHandler<()>) -> Element {
    let tr = use_tr();
    let core = use_core();
    let mut settings = use_settings();
    let mut toast_sig = use_context::<ToastSig>();
    let mut name = use_signal(String::new);
    let mut choosing = use_signal(|| false);
    let mut camera = use_signal(|| false);
    let mut restoring = use_signal(|| false);
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

    let finish = move || {
        let chosen: String = name.peek().trim().chars().take(64).collect();
        if chosen.is_empty() {
            return;
        }
        change(settings, |s| s.display_name = chosen);
        on_next.call(());
    };
    let finish_key = finish;

    rsx! {
        div { class: "start",
            h1 { "{t(&tr, \"start.title\")}" }
            p { "{t(&tr, \"start.text\")}" }
            button {
                class: "face-picker",
                aria_label: "{t(&tr, \"settings.photo\")}",
                onclick: move |_| choosing.set(true),
                if s.avatar.is_some() {
                    Avatar {
                        id: "me".to_string(),
                        name: s.display_name.clone(),
                        size: "huge",
                        image: s.avatar.clone(),
                    }
                } else {
                    Icon { name: IconName::Camera, size: 32 }
                }
            }
            label { class: "field",
                span { class: "gt", "{t(&tr, \"onboarding.name_label\")}" }
                input {
                    r#type: "text",
                    maxlength: "64",
                    placeholder: "{t(&tr, \"onboarding.name_placeholder\")}",
                    value: "{name}",
                    autofocus: true,
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            finish_key();
                        }
                    },
                }
            }
            p { class: "note", "{t(&tr, \"start.name_note\")}" }
            div { class: "grow" }
            button {
                class: "btn",
                style: "background:transparent",
                "data-t": "start-restore",
                onclick: move |_| restoring.set(true),
                "{t(&tr, \"start.used_before\")}"
            }
            button {
                class: "btn main",
                "data-t": "start-next",
                disabled: name().trim().is_empty(),
                onclick: move |_| finish(),
                "{t(&tr, \"start.continue\")}"
            }
            if restoring() {
                crate::screens::settings::RestoreSheet { on_close: move |_| restoring.set(false) }
            }
            if choosing() {
                Sheet { on_close: move |_| choosing.set(false),
                    div { class: "sheet-list",
                        SheetItem {
                            label: t(&tr, "settings.photo_choose"),
                            icon: IconName::Image,
                            on_tap: move |_| {
                                choosing.set(false);
                                let apply = from_gallery.clone();
                                spawn(async move {
                                    if let Some(mut files) = media::pick(media::IMAGES, false).await
                                        && !files.is_empty()
                                    {
                                        apply(files.remove(0));
                                    }
                                });
                            },
                        }
                        SheetItem {
                            label: t(&tr, "settings.photo_take"),
                            icon: IconName::Camera,
                            on_tap: move |_| {
                                choosing.set(false);
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
                    }
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
}

#[component]
fn Mode(on_next: EventHandler<()>) -> Element {
    let tr = use_tr();
    let settings = use_settings();
    rsx! {
        div { class: "start",
            h1 { "{t(&tr, \"start.mode_title\")}" }
            button {
                class: "mode-card",
                "data-t": "mode-simple",
                onclick: move |_| {
                    change(settings, |s| s.simple_mode = true);
                    on_next.call(());
                },
                div { class: "mode-preview",
                    span { "A" }
                    span { "B" }
                }
                span { class: "mode-title", "{t(&tr, \"start.simple\")}" }
                span { class: "sub", "{t(&tr, \"start.simple_text\")}" }
            }
            button {
                class: "mode-card",
                "data-t": "mode-full",
                onclick: move |_| {
                    change(settings, |s| s.simple_mode = false);
                    on_next.call(());
                },
                div { class: "mode-preview lines",
                    span {}
                    span {}
                    span {}
                }
                span { class: "mode-title", "{t(&tr, \"start.full\")}" }
                span { class: "sub", "{t(&tr, \"start.full_text\")}" }
            }
            p { class: "note", "{t(&tr, \"start.mode_note\")}" }
        }
    }
}

/// What the phone has to allow. Only what is not allowed yet has a button.
#[component]
fn Allow() -> Element {
    let tr = use_tr();
    let settings = use_settings();
    let looks = health::use_watch();
    let _ = looks();
    let items = health::check(&tr);
    rsx! {
        div { class: "start",
            h1 { "{t(&tr, \"start.allow_title\")}" }
            p { "{t(&tr, \"start.allow_text\")}" }
            if !items.is_empty() {
                div { class: "group",
                    for item in items {
                        health::HealthRow { key: "{item.key}", item }
                    }
                }
            } else {
                div { class: "group",
                    Row {
                        icon: IconName::Check,
                        title: t(&tr, "health.all_good"),
                        note: t(&tr, "start.nothing_to_allow"),
                    }
                }
            }
            p { class: "note", "{t(&tr, \"start.allow_note\")}" }
            div { class: "grow" }
            button {
                class: "btn main",
                "data-t": "start-done",
                onclick: move |_| change(settings, |s| s.onboarded = true),
                "{t(&tr, \"start.begin\")}"
            }
        }
    }
}
