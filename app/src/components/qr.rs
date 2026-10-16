use dioxus::prelude::*;

/// A QR code drawn as one SVG path (one 1×1 square per dark module).
#[component]
pub fn QrCode(data: String) -> Element {
    let Ok(code) = qrcode::QrCode::new(data.as_bytes()) else {
        return rsx! {};
    };
    let width = code.width();
    let mut d = String::new();
    for (i, color) in code.to_colors().into_iter().enumerate() {
        if color == qrcode::Color::Dark {
            let (x, y) = (i % width, i / width);
            d.push_str(&format!("M{x} {y}h1v1h-1z"));
        }
    }
    rsx! {
        div { class: "qr",
            svg {
                view_box: "0 0 {width} {width}",
                "shape-rendering": "crispEdges",
                path { d: "{d}", fill: "#000" }
            }
        }
    }
}
