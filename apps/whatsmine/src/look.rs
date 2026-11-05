//! From a look to what the page draws. Everything here is built from values and fixed choices, so
//! nothing of a look ever reaches the page as it came in.

use whatsmine_core::look::{
    Background, Base, Corners, Font, FontChoice, Look, Palette, Pattern, palette,
};

/// The root text size in pixels, which everything else scales with. A font that looks small or big
/// for its size is corrected, so the rest of the screen does not jump.
pub fn root_font_px(size: &str, font: &FontChoice) -> f32 {
    let base = match size {
        "small" => 14.0,
        "large" => 18.0,
        "huge" => 21.0,
        _ => 16.0,
    };
    base * correction(font)
}

fn correction(font: &FontChoice) -> f32 {
    match font {
        FontChoice::Bundled(Font::Inter) => 0.96,
        FontChoice::Bundled(Font::Lexend) => 0.95,
        FontChoice::Bundled(Font::JetBrainsMono) => 0.92,
        FontChoice::Bundled(Font::Caveat) => 1.25,
        FontChoice::Bundled(Font::OpenDyslexic) => 0.9,
        _ => 1.0,
    }
}

/// Only what can be part of a name in the page.
fn safe_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// The fonts of a look, as the page asks for them.
pub fn font_stack(font: &FontChoice) -> Option<String> {
    const REST: &str = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif";
    match font {
        FontChoice::System => None,
        FontChoice::Bundled(font) => Some(format!("'wm-{}', {REST}", font.key())),
        FontChoice::Own(id) => Some(format!("'wm-own-{}', {REST}", safe_id(id))),
    }
}

fn palette_vars(p: &Palette) -> String {
    let pairs = [
        ("bg", p.bg),
        ("surface", p.surface),
        ("surface-hover", p.surface_hover),
        ("surface2", p.surface2),
        ("line", p.line),
        ("border", p.line),
        ("primary", p.primary),
        ("primary-hover", p.primary_hover),
        ("on-primary", p.on_primary),
        ("primary-soft", p.primary_soft),
        ("text", p.text),
        ("text-secondary", p.text_secondary),
        ("error", p.error),
        ("on-error", p.on_error),
        ("success", p.success),
        ("on-success", p.on_success),
        ("warning", p.warning),
        ("warning-soft", p.warning_soft),
        ("disabled", p.disabled),
        ("bubble-out", p.bubble_out),
        ("bubble-out-text", p.bubble_out_text),
        ("bubble-out-meta", p.bubble_out_meta),
        ("read-tick", p.read_tick),
        ("bubble-in", p.bubble_in),
        ("bubble-in-text", p.bubble_in_text),
        ("bubble-in-meta", p.bubble_in_meta),
    ];
    let mut out = String::with_capacity(pairs.len() * 28);
    for (name, color) in pairs {
        out.push_str(&format!("--{name}:{};", color.hex()));
    }
    out.push_str(if p.dark {
        "--quote:rgba(255,255,255,.09);--scrim:rgba(0,0,0,.6);"
    } else {
        "--quote:rgba(0,0,0,.06);--scrim:rgba(0,0,0,.4);"
    });
    out
}

/// What is behind the messages.
fn wall_style(look: &Look) -> String {
    let mut css = match &look.background {
        Background::None => return String::new(),
        Background::Pattern(Pattern::Dots) => "background-color:var(--surface2);background-image:radial-gradient(var(--line) 1px,transparent 1.5px);background-size:14px 14px;".to_owned(),
        Background::Pattern(Pattern::Waves) => "background-color:var(--surface2);background-image:repeating-radial-gradient(circle at 0 100%,transparent 0 7px,var(--line) 7px 8px);".to_owned(),
        Background::Color(color) => format!("background-color:{};", color.hex()),
        Background::Gradient { from, to, angle } => format!(
            "background-color:{};background-image:linear-gradient({}deg,{},{});",
            from.hex(),
            angle.min(&360),
            from.hex(),
            to.hex()
        ),
        Background::Photo(id) => format!(
            "background-image:url(/look-photo/{});background-size:cover;background-position:center;",
            safe_id(id)
        ),
    };
    if look.dim > 0 {
        css.push_str(&format!(
            "box-shadow:inset 0 0 0 100vmax rgba(0,0,0,{:.2});",
            f32::from(look.dim.min(90)) / 100.0
        ));
    }
    if look.blur > 0 {
        let blur = look.blur.min(20);
        css.push_str(&format!("filter:blur({blur}px);inset:-{blur}px;"));
    }
    css
}

/// How a look comes out on the page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LookStyle {
    /// For the `style` of what it dresses.
    pub vars: String,
    /// Added to its classes.
    pub class: String,
    /// For what lies behind the messages. Empty for nothing.
    pub wall: String,
}

/// `own_colors`: this look brings its colours itself, because the ones around it are another
/// look's. Otherwise the theme class on the page's root has them already, if it can.
pub fn look_style(look: &Look, own_colors: bool) -> LookStyle {
    let mut vars = String::new();
    // A look that follows the device can only say so with the classes of the root.
    let follows_device = look.base == Base::System && look.colors.is_empty();
    if !follows_device && (own_colors || !look.colors.is_empty()) {
        vars.push_str(&palette_vars(&palette(look)));
    }
    if let Some(stack) = font_stack(&look.app_font) {
        vars.push_str(&format!("--font-app:{stack};"));
    }
    if let Some(stack) = font_stack(&look.message_font) {
        vars.push_str(&format!("--font-msg:{stack};"));
    }
    let size = f32::from(look.message_size.clamp(80, 150)) / 100.0 * correction(&look.message_font);
    if (size - 1.0).abs() > 0.001 {
        vars.push_str(&format!("--msg-scale:{size:.3};"));
    }
    match look.bubble_corners {
        Corners::Round => {}
        Corners::Soft => vars.push_str("--bubble-r:10px;--bubble-r-tail:4px;"),
        Corners::Square => vars.push_str("--bubble-r:3px;--bubble-r-tail:3px;"),
    }
    match look.app_corners {
        Corners::Round => {}
        Corners::Soft => vars.push_str("--radius:10px;--radius-md:8px;--radius-sm:6px;"),
        Corners::Square => vars.push_str("--radius:3px;--radius-md:3px;--radius-sm:2px;"),
    }
    if look.square_photos {
        vars.push_str("--photo-r:22%;");
    }
    let wall = wall_style(look);
    let mut class = String::new();
    if look.flat {
        class.push_str(" flat");
    }
    if !wall.is_empty() {
        class.push_str(" has-wall");
    }
    LookStyle { vars, class, wall }
}

/// The class on the page's root that a look starts from.
pub fn theme_class(base: Base) -> &'static str {
    match base {
        Base::System => "theme-system",
        Base::Dark => "theme-dark",
        Base::Light => "theme-light",
        Base::Black => "theme-black",
        Base::Colorblind => "theme-colorblind",
        Base::ColorblindLight => "theme-colorblind-light",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatsmine_core::look::{Colors, Rgb};

    const STYLE: &str = include_str!("../assets/style.css");

    /// The values of one block of the stylesheet.
    fn block(selector: &str) -> std::collections::HashMap<String, String> {
        let at = STYLE.find(selector).expect("the selector");
        let open = at + STYLE[at..].find('{').expect("its block");
        let close = open + STYLE[open..].find('}').expect("its end");
        STYLE[open + 1..close]
            .lines()
            .filter_map(|line| line.trim().strip_suffix(';')?.split_once(": "))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .collect()
    }

    #[test]
    fn the_ready_made_looks_are_what_the_stylesheet_says() {
        let dark = block(":root");
        let light = block(".theme-light,");
        let black = block(".theme-black");
        for (base, own) in [
            (Base::Dark, &dark),
            (Base::Light, &light),
            (Base::Black, &black),
        ] {
            let look = Look {
                base,
                ..Look::default()
            };
            let vars = palette_vars(&palette(&look));
            for (name, value) in own {
                if !name.starts_with("--") || !value.starts_with('#') || name == "--secondary" {
                    continue;
                }
                let want = format!("{name}:{value};");
                assert!(vars.contains(&want), "{base:?}: {want} is not in {vars}");
            }
        }
    }

    #[test]
    fn a_look_without_changes_leaves_the_page_alone() {
        let style = look_style(&Look::ready_made()[0], true);
        assert_eq!(style, LookStyle::default());
        let dark = Look::ready_made()[1].clone();
        assert_eq!(look_style(&dark, false), LookStyle::default());
        assert!(look_style(&dark, true).vars.contains("--bg:#101218"));
    }

    #[test]
    fn only_values_reach_the_page() {
        let look = Look {
            colors: Colors {
                background: Some(Rgb::new(0x11_22_33)),
                ..Colors::default()
            },
            app_font: FontChoice::Own("x');}body{display:none".into()),
            background: Background::Photo("../../secret);}".into()),
            message_size: 255,
            ..Look::default()
        };
        let style = look_style(&look, false);
        assert!(style.vars.contains("--bg:#112233;"));
        assert!(style.vars.contains("'wm-own-xbodydisplaynone'"));
        assert!(style.wall.contains("/look-photo/secret)"));
        assert!(style.vars.contains("--msg-scale:1.500;"));
        for text in [&style.vars, &style.wall] {
            assert!(!text.contains("../"));
            assert!(!text.contains('}'));
        }
    }

    #[test]
    fn sizes_follow_the_choice() {
        assert_eq!(root_font_px("normal", &FontChoice::System), 16.0);
        assert_eq!(root_font_px("huge", &FontChoice::System), 21.0);
        assert!(root_font_px("normal", &FontChoice::Bundled(Font::Caveat)) > 16.0);
    }
}
