use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum IconName {
    Chat,
    Send,
    Gear,
    UserPlus,
    Users,
    Search,
    Copy,
    Clipboard,
    ArrowLeft,
    Check,
    CheckDouble,
    Clock,
    Xmark,
    Trash,
    Ban,
    Pencil,
    Shield,
    Key,
    Lock,
    ChevronDown,
    ChevronRight,
    Server,
}

/// Inline stroke icon (24×24 grid). No fonts or CDN.
#[component]
pub fn Icon(name: IconName, #[props(default = 20)] size: u32) -> Element {
    let paths: &[&str] = match name {
        IconName::Chat => &[
            "M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z",
        ],
        IconName::Send => &["M22 2 11 13", "M22 2 15 22 11 13 2 9 22 2z"],
        IconName::Gear => &[
            "M12 9a3 3 0 1 0 0 6 3 3 0 1 0 0-6z",
            "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z",
        ],
        IconName::UserPlus => &[
            "M16 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2",
            "M8.5 3a4 4 0 1 0 0 8 4 4 0 1 0 0-8z",
            "M20 8v6",
            "M23 11h-6",
        ],
        IconName::Users => &[
            "M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2",
            "M9 3a4 4 0 1 0 0 8 4 4 0 1 0 0-8z",
            "M23 21v-2a4 4 0 0 0-3-3.87",
            "M16 3.13a4 4 0 0 1 0 7.75",
        ],
        IconName::Search => &["M11 3a8 8 0 1 0 0 16 8 8 0 1 0 0-16z", "M21 21l-4.35-4.35"],
        IconName::Copy => &[
            "M20 9h-9a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2v-9a2 2 0 0 0-2-2z",
            "M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1",
        ],
        IconName::Clipboard => &[
            "M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2",
            "M9 2h6a1 1 0 0 1 1 1v2a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z",
        ],
        IconName::ArrowLeft => &["M19 12H5", "M12 19l-7-7 7-7"],
        IconName::Check => &["M20 6 9 17l-5-5"],
        IconName::CheckDouble => &["M17 6 7 17l-5-5", "M22 6 12 17l-1.5-1.5"],
        IconName::Clock => &["M12 2a10 10 0 1 0 0 20 10 10 0 1 0 0-20z", "M12 6v6l4 2"],
        IconName::Xmark => &["M18 6 6 18", "M6 6l12 12"],
        IconName::Trash => &[
            "M3 6h18",
            "M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6",
            "M10 11v6",
            "M14 11v6",
            "M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2",
        ],
        IconName::Ban => &[
            "M12 2a10 10 0 1 0 0 20 10 10 0 1 0 0-20z",
            "M4.93 4.93l14.14 14.14",
        ],
        IconName::Pencil => &[
            "M12 20h9",
            "M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z",
        ],
        IconName::Shield => &["M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"],
        IconName::Key => &[
            "M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.78 7.78 5.5 5.5 0 0 1 7.78-7.78zm0 0L15.5 7.5m0 0 3 3L22 7l-3-3m-3.5 3.5L19 4",
        ],
        IconName::Lock => &[
            "M5 11h14a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2z",
            "M7 11V7a5 5 0 0 1 10 0v4",
        ],
        IconName::ChevronDown => &["M6 9l6 6 6-6"],
        IconName::ChevronRight => &["M9 18l6-6-6-6"],
        IconName::Server => &[
            "M4 2h16a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z",
            "M4 14h16a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-4a2 2 0 0 1 2-2z",
            "M6 6h.01",
            "M6 18h.01",
        ],
    };

    rsx! {
        svg {
            class: "icon",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            for d in paths.iter() {
                path { d: "{d}" }
            }
        }
    }
}
