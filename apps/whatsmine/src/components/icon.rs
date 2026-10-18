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
    Paperclip,
    Mic,
    Camera,
    Video,
    Image,
    File,
    Download,
    External,
    Stop,
    Phone,
    PhoneOff,
    MicOff,
    VideoOff,
    Book,
    Pin,
    Bell,
    BellOff,
    Share,
    Link,
    Scan,
    Forward,
    More,
    ChevronLeft,
    Info,
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
        IconName::Paperclip => &[
            "M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48",
        ],
        IconName::Mic => &[
            "M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z",
            "M19 10v2a7 7 0 0 1-14 0v-2",
            "M12 19v4",
            "M8 23h8",
        ],
        IconName::Camera => &[
            "M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z",
            "M12 9a4 4 0 1 0 0 8 4 4 0 1 0 0-8z",
        ],
        IconName::Video => &[
            "M23 7l-7 5 7 5V7z",
            "M3 5h11a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z",
        ],
        IconName::Image => &[
            "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z",
            "M8.5 7a1.5 1.5 0 1 0 0 3 1.5 1.5 0 1 0 0-3z",
            "M21 15l-5-5L5 21",
        ],
        IconName::File => &[
            "M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z",
            "M13 2v7h7",
        ],
        IconName::Download => &[
            "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4",
            "M7 10l5 5 5-5",
            "M12 15V3",
        ],
        IconName::External => &[
            "M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6",
            "M15 3h6v6",
            "M10 14L21 3",
        ],
        IconName::Stop => &["M6 6h12v12H6z"],
        IconName::Phone => &[
            "M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6 19.79 19.79 0 0 1-3.07-8.67A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72 12.84 12.84 0 0 0 .7 2.81 2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45 12.84 12.84 0 0 0 2.81.7A2 2 0 0 1 22 16.92z",
        ],
        IconName::PhoneOff => &[
            "M10.68 13.31a16 16 0 0 0 3.41 2.6l1.27-1.27a2 2 0 0 1 2.11-.45 12.84 12.84 0 0 0 2.81.7 2 2 0 0 1 1.72 2v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.42 19.42 0 0 1-3.33-2.67m-2.67-3.34a19.79 19.79 0 0 1-3.07-8.63A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72 12.84 12.84 0 0 0 .7 2.81 2 2 0 0 1-.45 2.11L8.09 9.91",
            "M23 1 1 23",
        ],
        IconName::MicOff => &[
            "M1 1l22 22",
            "M9 9v3a3 3 0 0 0 5.12 2.12M15 9.34V4a3 3 0 0 0-5.94-.6",
            "M17 16.95A7 7 0 0 1 5 12v-2m14 0v2a7 7 0 0 1-.11 1.23",
            "M12 19v4",
            "M8 23h8",
        ],
        IconName::VideoOff => &[
            "M16 16v1a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2h2m5.66 0H14a2 2 0 0 1 2 2v3.34l1 1L23 7v10",
            "M1 1l22 22",
        ],
        IconName::Book => &[
            "M4 19.5A2.5 2.5 0 0 1 6.5 17H20",
            "M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z",
        ],
        IconName::Pin => &[
            "M12 17v5",
            "M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z",
        ],
        IconName::Bell => &[
            "M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9",
            "M13.73 21a2 2 0 0 1-3.46 0",
        ],
        IconName::BellOff => &[
            "M13.73 21a2 2 0 0 1-3.46 0",
            "M18.63 13A17.89 17.89 0 0 1 18 8",
            "M6.26 6.26A5.86 5.86 0 0 0 6 8c0 7-3 9-3 9h14",
            "M18 8a6 6 0 0 0-9.33-5",
            "M1 1l22 22",
        ],
        IconName::Share => &[
            "M18 2a3 3 0 1 0 0 6 3 3 0 1 0 0-6z",
            "M6 9a3 3 0 1 0 0 6 3 3 0 1 0 0-6z",
            "M18 16a3 3 0 1 0 0 6 3 3 0 1 0 0-6z",
            "M8.59 13.51l6.83 3.98",
            "M15.41 6.51l-6.82 3.98",
        ],
        IconName::Link => &[
            "M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71",
            "M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71",
        ],
        IconName::Scan => &[
            "M3 7V5a2 2 0 0 1 2-2h2",
            "M17 3h2a2 2 0 0 1 2 2v2",
            "M21 17v2a2 2 0 0 1-2 2h-2",
            "M7 21H5a2 2 0 0 1-2-2v-2",
            "M7 12h10",
        ],
        IconName::Forward => &["M15 14l5-5-5-5", "M4 20v-7a4 4 0 0 1 4-4h12"],
        IconName::More => &["M12 5h.01", "M12 12h.01", "M12 19h.01"],
        IconName::ChevronLeft => &["M15 18l-6-6 6-6"],
        IconName::Info => &[
            "M12 2a10 10 0 1 0 0 20 10 10 0 1 0 0-20z",
            "M12 16v-4",
            "M12 8h.01",
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
