//! A look: colours, fonts, a chat background and shapes, under a name. The same look can dress the
//! app, one chat, or all chats that have none of their own, and can be sent to someone as a copy.
//!
//! A look holds values and fixed choices, never anything that is executed or spliced as it is, so
//! one from someone else cannot do more than change how a chat looks here. The colours people did
//! not choose follow from the ones they did, and whatever is chosen stays readable.

use serde::{Deserialize, Serialize};

/// A colour, as three bytes. Nothing else can come through here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn new(hex: u32) -> Self {
        Self((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// `#rrggbb` or `rrggbb`, and nothing else.
    pub fn parse(text: &str) -> Option<Self> {
        let digits = text.trim().strip_prefix('#').unwrap_or(text.trim());
        if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(digits, 16).ok().map(Self::new)
    }

    /// From hue in degrees, saturation and value from 0 to 1.
    pub fn from_hsv(hue: f32, saturation: f32, value: f32) -> Self {
        let h = hue.rem_euclid(360.0) / 60.0;
        let (s, v) = (saturation.clamp(0.0, 1.0), value.clamp(0.0, 1.0));
        let c = v * s;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        let byte = |channel: f32| ((channel + m) * 255.0).round() as u8;
        Self(byte(r), byte(g), byte(b))
    }

    /// Hue in degrees, saturation and value from 0 to 1.
    pub fn to_hsv(self) -> (f32, f32, f32) {
        let (r, g, b) = (
            f32::from(self.0) / 255.0,
            f32::from(self.1) / 255.0,
            f32::from(self.2) / 255.0,
        );
        let max = r.max(g).max(b);
        let delta = max - r.min(g).min(b);
        let hue = if delta <= 0.0 {
            0.0
        } else if (max - r).abs() < f32::EPSILON {
            60.0 * ((g - b) / delta).rem_euclid(6.0)
        } else if (max - g).abs() < f32::EPSILON {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        let saturation = if max <= 0.0 { 0.0 } else { delta / max };
        (hue, saturation, max)
    }

    fn linear(channel: u8) -> f32 {
        let c = f32::from(channel) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// How much light it gives, as WCAG counts it.
    pub fn luminance(self) -> f32 {
        0.2126 * Self::linear(self.0)
            + 0.7152 * Self::linear(self.1)
            + 0.0722 * Self::linear(self.2)
    }

    /// Black or near it counts as a dark background, for what goes with it.
    pub fn is_dark(self) -> bool {
        self.luminance() < 0.18
    }
}

/// The WCAG contrast ratio: 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (a.luminance(), b.luminance());
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

// ---- working with colours: OKLab, where equal steps look equal ----

#[derive(Clone, Copy)]
struct Lab(f32, f32, f32);

fn to_lab(c: Rgb) -> Lab {
    let (r, g, b) = (Rgb::linear(c.0), Rgb::linear(c.1), Rgb::linear(c.2));
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    Lab(
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    )
}

fn gamma(channel: f32) -> u8 {
    let c = if channel <= 0.003_130_8 {
        12.92 * channel
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    };
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn linear_rgb(lab: Lab) -> (f32, f32, f32) {
    let l = lab.0 + 0.396_337_78 * lab.1 + 0.215_803_76 * lab.2;
    let m = lab.0 - 0.105_561_346 * lab.1 - 0.063_854_17 * lab.2;
    let s = lab.0 - 0.089_484_18 * lab.1 - 1.291_485_5 * lab.2;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    (
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    )
}

/// Back to a colour a screen has: less vivid until it fits, so the hue stays.
fn from_lab(lab: Lab) -> Rgb {
    let mut chroma = 1.0_f32;
    loop {
        let (r, g, b) = linear_rgb(Lab(lab.0, lab.1 * chroma, lab.2 * chroma));
        let fits = |c: f32| (-0.000_5..=1.000_5).contains(&c);
        if chroma <= 0.0 || (fits(r) && fits(g) && fits(b)) {
            return Rgb(gamma(r), gamma(g), gamma(b));
        }
        chroma -= 0.05;
    }
}

/// `to` by this share, in steps that look equal.
fn mix(from: Rgb, to: Rgb, share: f32) -> Rgb {
    let (a, b) = (to_lab(from), to_lab(to));
    from_lab(Lab(
        a.0 + (b.0 - a.0) * share,
        a.1 + (b.1 - a.1) * share,
        a.2 + (b.2 - a.2) * share,
    ))
}

/// Lighter or darker by this much, of 1.
fn shift(color: Rgb, by: f32) -> Rgb {
    let lab = to_lab(color);
    from_lab(Lab((lab.0 + by).clamp(0.0, 1.0), lab.1, lab.2))
}

/// The nearest colour of this hue that reads against all of `against`.
fn nudge(color: Rgb, against: &[Rgb], least: f32) -> Rgb {
    let reads = |c: Rgb| against.iter().all(|other| contrast(c, *other) >= least);
    if reads(color) {
        return color;
    }
    for step in 1..=50 {
        let by = step as f32 * 0.02;
        for candidate in [shift(color, by), shift(color, -by)] {
            if reads(candidate) {
                return candidate;
            }
        }
    }
    readable_on(against.first().copied().unwrap_or(color))
}

const INK: Rgb = Rgb::new(0x14_16_1c);
const PAPER: Rgb = Rgb::new(0xf2_f3_f7);

/// Text for on this colour. Black or white always reads on anything, so this always can.
pub fn readable_on(background: Rgb) -> Rgb {
    let soft = if contrast(INK, background) >= contrast(PAPER, background) {
        INK
    } else {
        PAPER
    };
    if contrast(soft, background) >= 4.5 {
        return soft;
    }
    if contrast(Rgb::new(0), background) >= contrast(Rgb::new(0xff_ff_ff), background) {
        Rgb::new(0)
    } else {
        Rgb::new(0xff_ff_ff)
    }
}

// ---- what a look is ----

/// What a look starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Base {
    /// Follows the device. Only for the ready-made look.
    System,
    #[default]
    Dark,
    Light,
    Black,
    Colorblind,
    ColorblindLight,
}

impl Base {
    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Black => "black",
            Self::Colorblind => "colorblind",
            Self::ColorblindLight => "colorblind-light",
        }
    }
}

/// The colours someone chose. What is left out follows from the rest.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Colors {
    pub background: Option<Rgb>,
    pub cards: Option<Rgb>,
    pub accent: Option<Rgb>,
    pub text: Option<Rgb>,
    pub my_bubble: Option<Rgb>,
    pub my_text: Option<Rgb>,
    pub their_bubble: Option<Rgb>,
    pub their_text: Option<Rgb>,
}

impl Colors {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The fonts that come with the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Font {
    Inter,
    Atkinson,
    Lexend,
    Nunito,
    Lora,
    JetBrainsMono,
    Caveat,
    OpenDyslexic,
}

impl Font {
    pub const ALL: [Self; 8] = [
        Self::Inter,
        Self::Atkinson,
        Self::Lexend,
        Self::Nunito,
        Self::Lora,
        Self::JetBrainsMono,
        Self::Caveat,
        Self::OpenDyslexic,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Inter => "inter",
            Self::Atkinson => "atkinson",
            Self::Lexend => "lexend",
            Self::Nunito => "nunito",
            Self::Lora => "lora",
            Self::JetBrainsMono => "jetbrains",
            Self::Caveat => "caveat",
            Self::OpenDyslexic => "opendyslexic",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Inter => "Inter",
            Self::Atkinson => "Atkinson Hyperlegible",
            Self::Lexend => "Lexend",
            Self::Nunito => "Nunito",
            Self::Lora => "Lora",
            Self::JetBrainsMono => "JetBrains Mono",
            Self::Caveat => "Caveat",
            Self::OpenDyslexic => "OpenDyslexic",
        }
    }
}

/// A font someone added from a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnFont {
    pub id: String,
    pub name: String,
    pub ext: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FontChoice {
    /// What the device uses.
    #[default]
    System,
    Bundled(Font),
    /// A file of their own, on this device only.
    Own(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Pattern {
    #[default]
    Dots,
    Waves,
}

/// What lies behind the messages.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Background {
    #[default]
    None,
    Pattern(Pattern),
    Color(Rgb),
    Gradient {
        from: Rgb,
        to: Rgb,
        /// 0 to 360 degrees.
        angle: u16,
    },
    /// A photo, kept sealed with the looks.
    Photo(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Corners {
    #[default]
    Round,
    Soft,
    Square,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub id: String,
    pub name: String,
    pub base: Base,
    pub colors: Colors,
    pub app_font: FontChoice,
    pub message_font: FontChoice,
    /// Size of messages in percent, 80 to 150.
    pub message_size: u8,
    pub background: Background,
    /// Over the background, 0 to 90 percent.
    pub dim: u8,
    /// Of the background, 0 to 20 pixels.
    pub blur: u8,
    pub bubble_corners: Corners,
    /// Messages as plain lines, without bubbles.
    pub flat: bool,
    pub app_corners: Corners,
    pub square_photos: bool,
    /// Who sent this copy.
    pub from: Option<String>,
    /// The message it came with.
    pub from_message: Option<String>,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            base: Base::Dark,
            colors: Colors::default(),
            app_font: FontChoice::System,
            message_font: FontChoice::System,
            message_size: 100,
            background: Background::None,
            dim: 0,
            blur: 0,
            bubble_corners: Corners::Round,
            flat: false,
            app_corners: Corners::Round,
            square_photos: false,
            from: None,
            from_message: None,
        }
    }
}

/// Keeps a look's name in bounds, and on one line.
fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect::<String>()
        .trim()
        .to_owned()
}

pub const MAX_NAME: usize = 40;
/// How many looks someone can keep.
pub const MAX_LOOKS: usize = 50;

impl Look {
    /// The ready-made looks. Their id is the key of what they start from.
    pub fn ready_made() -> Vec<Self> {
        [
            Base::System,
            Base::Dark,
            Base::Light,
            Base::Black,
            Base::Colorblind,
            Base::ColorblindLight,
        ]
        .into_iter()
        .map(|base| Self {
            id: base.key().to_owned(),
            base,
            ..Self::default()
        })
        .collect()
    }

    pub fn is_ready_made(&self) -> bool {
        self.id == self.base.key() && self.colors.is_empty()
    }

    /// A look that can be changed, from this one. The device's own colours cannot be known here,
    /// so one that follows the device starts out dark.
    pub fn copied(&self, id: String, name: String) -> Self {
        let mut copy = self.clone();
        copy.id = id;
        copy.name = clean_name(&name);
        if copy.base == Base::System {
            copy.base = Base::Dark;
        }
        copy
    }

    /// Everything in bounds, whatever was saved or sent.
    pub fn in_bounds(mut self) -> Self {
        self.name = clean_name(&self.name);
        self.message_size = self.message_size.clamp(80, 150);
        self.dim = self.dim.min(90);
        self.blur = self.blur.min(20);
        if let Background::Gradient { angle, .. } = &mut self.background {
            *angle %= 361;
        }
        self
    }

    /// What goes to someone else: not what only means something here.
    pub fn for_sharing(&self) -> Self {
        let mut copy = self.clone().in_bounds();
        for font in [&mut copy.app_font, &mut copy.message_font] {
            if matches!(font, FontChoice::Own(_)) {
                *font = FontChoice::System;
            }
        }
        copy.from = None;
        copy.from_message = None;
        copy
    }

    /// What came from someone else, as a look of ours: a new id, so nothing of ours is overwritten,
    /// nothing that only means something on their side, and text colours that follow from the rest
    /// so it cannot be made unreadable.
    pub fn from_someone(&self, id: String, who: &str, message: &str) -> Self {
        let mut copy = self.for_sharing();
        copy.id = id;
        if copy.name.is_empty() {
            copy.name = clean_name(who);
        }
        if copy.base == Base::System {
            copy.base = Base::Dark;
        }
        copy.colors.text = None;
        copy.colors.my_text = None;
        copy.colors.their_text = None;
        // The photo comes separately, if at all.
        if matches!(copy.background, Background::Photo(_)) {
            copy.background = Background::None;
        }
        copy.from = Some(clean_name(who));
        copy.from_message = Some(message.to_owned());
        copy
    }
}

// ---- the colours everything uses ----

/// Every colour the app draws with, all worked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Rgb,
    pub surface: Rgb,
    pub surface_hover: Rgb,
    pub surface2: Rgb,
    pub line: Rgb,
    pub primary: Rgb,
    pub primary_hover: Rgb,
    pub on_primary: Rgb,
    pub primary_soft: Rgb,
    pub text: Rgb,
    pub text_secondary: Rgb,
    pub error: Rgb,
    pub on_error: Rgb,
    pub success: Rgb,
    pub on_success: Rgb,
    pub warning: Rgb,
    pub warning_soft: Rgb,
    pub disabled: Rgb,
    pub bubble_out: Rgb,
    pub bubble_out_text: Rgb,
    pub bubble_out_meta: Rgb,
    pub read_tick: Rgb,
    pub bubble_in: Rgb,
    pub bubble_in_text: Rgb,
    pub bubble_in_meta: Rgb,
}

const fn c(hex: u32) -> Rgb {
    Rgb::new(hex)
}

/// The same values as the themes in the stylesheet. A test holds them together.
const DARK: Palette = Palette {
    dark: true,
    bg: c(0x10_12_18),
    surface: c(0x1b_1e_27),
    surface_hover: c(0x27_2b_36),
    surface2: c(0x27_2b_36),
    line: c(0x2e_33_40),
    primary: c(0x7c_b0_ff),
    primary_hover: c(0x94_bf_ff),
    on_primary: c(0x0b_15_26),
    primary_soft: c(0x1e_2c_45),
    text: c(0xf2_f3_f7),
    text_secondary: c(0xa9_af_bd),
    error: c(0xff_8a_7a),
    on_error: c(0x2b_0a_06),
    success: c(0x6f_d6_a0),
    on_success: c(0x07_26_1a),
    warning: c(0xff_b4_54),
    warning_soft: c(0x33_27_1a),
    disabled: c(0x6b_74_88),
    bubble_out: c(0x27_40_69),
    bubble_out_text: c(0xf2_f3_f7),
    bubble_out_meta: c(0xb9_c8_e2),
    read_tick: c(0x7c_b0_ff),
    bubble_in: c(0x1b_1e_27),
    bubble_in_text: c(0xf2_f3_f7),
    bubble_in_meta: c(0xa9_af_bd),
};

const LIGHT: Palette = Palette {
    dark: false,
    bg: c(0xf5_f6_f9),
    surface: c(0xff_ff_ff),
    surface_hover: c(0xe9_eb_f1),
    surface2: c(0xe9_eb_f1),
    line: c(0xe3_e5_ec),
    primary: c(0x1f_5f_d6),
    primary_hover: c(0x2b_6c_e6),
    on_primary: c(0xff_ff_ff),
    primary_soft: c(0xdf_e9_fb),
    text: c(0x14_16_1c),
    text_secondary: c(0x58_5e_6e),
    error: c(0xb3_26_1e),
    on_error: c(0xff_ff_ff),
    success: c(0x1b_7a_4b),
    on_success: c(0xff_ff_ff),
    warning: c(0x8f_54_00),
    warning_soft: c(0xfb_ef_dc),
    disabled: c(0xa6_ad_bb),
    bubble_out: c(0xd8_e6_fd),
    bubble_out_text: c(0x14_16_1c),
    bubble_out_meta: c(0x45_55_6f),
    read_tick: c(0x1f_5f_d6),
    bubble_in: c(0xff_ff_ff),
    bubble_in_text: c(0x14_16_1c),
    bubble_in_meta: c(0x58_5e_6e),
};

const fn black() -> Palette {
    let mut p = DARK;
    p.bg = c(0);
    p.surface = c(0x101116);
    p.surface_hover = c(0x1c_1e_25);
    p.surface2 = c(0x1c_1e_25);
    p.line = c(0x23_26_2e);
    p.primary_soft = c(0x16_23_3a);
    p.warning_soft = c(0x2a_20_14);
    p.bubble_out = c(0x1d_31_54);
    p.bubble_in = c(0x101116);
    p
}

const fn colorblind() -> Palette {
    let mut p = DARK;
    p.success = c(0x7c_b0_ff);
    p.on_success = c(0x0b_15_26);
    p.error = c(0xff_b4_54);
    p.on_error = c(0x2b_1a_05);
    p
}

const fn colorblind_light() -> Palette {
    let mut p = LIGHT;
    p.success = c(0x1f_5f_d6);
    p.on_success = c(0xff_ff_ff);
    p.error = c(0x8f_54_00);
    p.on_error = c(0xff_ff_ff);
    p
}

fn base_palette(base: Base) -> Palette {
    match base {
        Base::System | Base::Dark => DARK,
        Base::Light => LIGHT,
        Base::Black => black(),
        Base::Colorblind => colorblind(),
        Base::ColorblindLight => colorblind_light(),
    }
}

/// The least contrast for text, and for what only has to be seen.
const TEXT: f32 = 4.5;
const SEEN: f32 = 3.0;

/// All the colours of a look. Nothing chosen gives what it starts from, exactly. What is chosen
/// stays as chosen wherever it reads, and moves as little as it takes where it does not.
pub fn palette(look: &Look) -> Palette {
    let base = base_palette(look.base);
    let chosen = &look.colors;
    if chosen.is_empty() {
        return base;
    }
    let bg = chosen.background.unwrap_or(base.bg);
    let dark = bg.is_dark();
    // The same colours of the other kind when the background changed sides.
    let family = if dark == base.dark {
        base.clone()
    } else if dark {
        DARK
    } else {
        LIGHT
    };
    let away = if dark { 1.0 } else { -1.0 };

    // Text you chose yourself may read less well, but never so badly that Settings is out of reach.
    let (text, least) = match chosen.text {
        Some(mine) => (nudge(mine, &[bg], SEEN), SEEN),
        None => (readable_on(bg), TEXT),
    };
    let surface = chosen.cards.unwrap_or_else(|| {
        if chosen.background.is_some() {
            shift(bg, if dark { 0.05 } else { 0.03 })
        } else {
            family.surface
        }
    });
    let surface = nudge(surface, &[text], least);
    let surface2 = nudge(shift(surface, away * 0.05), &[text], least);
    let surface_hover = surface2;
    let line = shift(surface, away * 0.09);

    let primary = nudge(
        chosen.accent.unwrap_or(family.primary),
        &[bg, surface],
        SEEN,
    );
    let on_primary = readable_on(primary);
    let primary_hover = shift(primary, away * 0.05);
    let primary_soft = nudge(mix(bg, primary, 0.22), &[text], least);

    let text_secondary = nudge(mix(text, bg, 0.3), &[bg, surface, surface2], least);
    let disabled = mix(text, bg, 0.55);

    let error = nudge(family.error, &[bg, surface], SEEN);
    let success = nudge(family.success, &[bg, surface], SEEN);
    let warning = nudge(family.warning, &[bg, surface], SEEN);
    let warning_soft = nudge(mix(bg, warning, 0.15), &[warning], SEEN);

    let bubble_out = chosen.my_bubble.unwrap_or_else(|| {
        if chosen.accent.is_some() || chosen.background.is_some() {
            mix(bg, primary, 0.35)
        } else {
            family.bubble_out
        }
    });
    let bubble_text = |mine: Option<Rgb>, bubble: Rgb| match mine {
        Some(mine) => (nudge(mine, &[bubble], SEEN), SEEN),
        None => (readable_on(bubble), TEXT),
    };
    let (bubble_out_text, out_least) = bubble_text(chosen.my_text, bubble_out);
    let bubble_out_meta = nudge(
        mix(bubble_out_text, bubble_out, 0.2),
        &[bubble_out],
        out_least,
    );
    let read_tick = nudge(primary, &[bubble_out], SEEN);

    let bubble_in = chosen.their_bubble.unwrap_or(surface);
    let (bubble_in_text, in_least) = bubble_text(chosen.their_text, bubble_in);
    let bubble_in_meta = nudge(mix(bubble_in_text, bubble_in, 0.2), &[bubble_in], in_least);

    Palette {
        dark,
        bg,
        surface,
        surface_hover,
        surface2,
        line,
        primary,
        primary_hover,
        on_primary,
        primary_soft,
        text,
        text_secondary,
        error,
        on_error: readable_on(error),
        success,
        on_success: readable_on(success),
        warning,
        warning_soft,
        disabled,
        bubble_out,
        bubble_out_text,
        bubble_out_meta,
        read_tick,
        bubble_in,
        bubble_in_text,
        bubble_in_meta,
    }
}

// ---- soft colours to start from ----

/// Eight families, each soft, mid and deep. Our own colours, under plain names.
pub const SOFT: [(&str, [Rgb; 3]); 8] = [
    ("rose", [c(0xf6_d9_de), c(0xe3_a3_af), c(0xa8_5a_6c)]),
    ("peach", [c(0xfb_e0_cf), c(0xf2_b0_8e), c(0xb9_6f_4a)]),
    ("sand", [c(0xf3_e7_c9), c(0xdc_c4_8a), c(0x9a_7f_45)]),
    ("sage", [c(0xdc_e8_d6), c(0xa9_c4_a0), c(0x5f_7f_5a)]),
    ("sea", [c(0xcf_ea_e6), c(0x8c_c9_c1), c(0x3f_7f_78)]),
    ("sky", [c(0xd5_e6_f5), c(0x9b_c2_e6), c(0x4f_7f_a8)]),
    ("lilac", [c(0xe4_dc_f4), c(0xb9_a7_e0), c(0x75_61_9f)]),
    ("slate", [c(0xe1_e4_ea), c(0xa9_b0_bd), c(0x5b_64_74)]),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The same numbers every run.
    struct Dice(u64);

    impl Dice {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn color(&mut self) -> Rgb {
            let n = self.next();
            // Mostly anything, and now and then the extremes people do pick.
            match n >> 60 {
                0 => Rgb::new(0),
                1 => Rgb::new(0xff_ff_ff),
                2 => {
                    let v = (n & 0xff) as u8;
                    Rgb(v, v, v)
                }
                _ => Rgb(
                    (n & 0xff) as u8,
                    ((n >> 8) & 0xff) as u8,
                    ((n >> 16) & 0xff) as u8,
                ),
            }
        }

        fn maybe(&mut self) -> Option<Rgb> {
            let color = self.color();
            self.next().is_multiple_of(2).then_some(color)
        }
    }

    fn reads(a: Rgb, b: Rgb, least: f32, what: &str, look: &Look) {
        let found = contrast(a, b);
        assert!(found >= least - 0.01, "{what}: {found} in {look:?}");
    }

    #[test]
    fn whatever_is_chosen_stays_readable() {
        let mut dice = Dice(0x9e37_79b9_7f4a_7c15);
        for round in 0..20_000 {
            let look = Look {
                base: match round % 5 {
                    0 => Base::Dark,
                    1 => Base::Light,
                    2 => Base::Black,
                    3 => Base::Colorblind,
                    _ => Base::ColorblindLight,
                },
                colors: Colors {
                    background: dice.maybe(),
                    cards: dice.maybe(),
                    accent: dice.maybe(),
                    text: None,
                    my_bubble: dice.maybe(),
                    my_text: None,
                    their_bubble: dice.maybe(),
                    their_text: None,
                },
                ..Look::default()
            };
            let p = palette(&look);
            for (back, name) in [
                (p.bg, "bg"),
                (p.surface, "surface"),
                (p.surface2, "surface2"),
            ] {
                reads(p.text, back, TEXT, name, &look);
                reads(p.text_secondary, back, TEXT, "secondary", &look);
            }
            reads(p.text, p.primary_soft, TEXT, "soft", &look);
            reads(p.primary, p.bg, SEEN, "accent on bg", &look);
            reads(p.primary, p.surface, SEEN, "accent on cards", &look);
            reads(p.on_primary, p.primary, TEXT, "on accent", &look);
            reads(p.bubble_out_text, p.bubble_out, TEXT, "my text", &look);
            reads(p.bubble_out_meta, p.bubble_out, TEXT, "my meta", &look);
            reads(p.read_tick, p.bubble_out, SEEN, "tick", &look);
            reads(p.bubble_in_text, p.bubble_in, TEXT, "their text", &look);
            reads(p.bubble_in_meta, p.bubble_in, TEXT, "their meta", &look);
            reads(p.on_error, p.error, TEXT, "on error", &look);
            reads(p.error, p.surface, SEEN, "error", &look);
            reads(p.on_success, p.success, TEXT, "on success", &look);
            reads(p.warning, p.surface, SEEN, "warning", &look);
        }
    }

    #[test]
    fn text_someone_picks_stays_as_picked_as_long_as_it_can_be_seen() {
        let mut dice = Dice(0x2545_f491_4f6c_dd1d);
        for _ in 0..20_000 {
            let look = Look {
                colors: Colors {
                    background: dice.maybe(),
                    text: Some(dice.color()),
                    my_bubble: dice.maybe(),
                    my_text: Some(dice.color()),
                    ..Colors::default()
                },
                ..Look::default()
            };
            let p = palette(&look);
            reads(p.text, p.bg, SEEN, "text", &look);
            reads(p.text, p.surface, SEEN, "text on cards", &look);
            reads(p.bubble_out_text, p.bubble_out, SEEN, "my text", &look);
        }
        // A good choice is left alone.
        let look = Look {
            colors: Colors {
                background: Some(Rgb::new(0x10_20_30)),
                text: Some(Rgb::new(0xff_ee_dd)),
                ..Colors::default()
            },
            ..Look::default()
        };
        assert_eq!(palette(&look).text, Rgb::new(0xff_ee_dd));
    }

    #[test]
    fn what_is_chosen_is_used_as_it_is_when_it_reads() {
        let chosen = Rgb::new(0xa9_c4_a0);
        let look = Look {
            colors: Colors {
                background: Some(Rgb::new(0xf5_f6_f9)),
                my_bubble: Some(chosen),
                ..Colors::default()
            },
            ..Look::default()
        };
        let p = palette(&look);
        assert_eq!(p.bubble_out, chosen);
        assert!(!p.dark);
    }

    #[test]
    fn nothing_chosen_is_exactly_what_it_starts_from() {
        assert_eq!(palette(&Look::default()), DARK);
        let light = Look {
            base: Base::Light,
            ..Look::default()
        };
        assert_eq!(palette(&light), LIGHT);
    }

    #[test]
    fn soft_colours_carry_text_and_are_our_own() {
        for (family, tints) in SOFT {
            assert!(!family.is_empty());
            for tint in tints {
                assert!(contrast(readable_on(tint), tint) >= TEXT);
            }
        }
    }

    #[test]
    fn colours_come_back_through_their_codes_and_the_wheel() {
        assert_eq!(Rgb::parse("#a9C4a0"), Some(Rgb::new(0xa9_c4_a0)));
        assert_eq!(Rgb::parse("a9c4a0"), Some(Rgb::new(0xa9_c4_a0)));
        assert_eq!(Rgb::new(0xa9_c4_a0).hex(), "#a9c4a0");
        for bad in [
            "", "#12345", "#1234567", "red", "#12345g", "url(x)", "#12;4}6",
        ] {
            assert_eq!(Rgb::parse(bad), None, "{bad}");
        }
        let mut dice = Dice(0x1234_5678_9abc_def1);
        for _ in 0..5_000 {
            let color = dice.color();
            let (h, s, v) = color.to_hsv();
            let back = Rgb::from_hsv(h, s, v);
            let near = |a: u8, b: u8| a.abs_diff(b) <= 1;
            assert!(
                near(back.0, color.0) && near(back.1, color.1) && near(back.2, color.2),
                "{color:?} became {back:?}"
            );
        }
    }

    #[test]
    fn what_comes_from_someone_else_can_do_no_more_than_look_different() {
        let sent = Look {
            id: "my-look-you-could-overwrite".into(),
            name: format!("A\nname{}", "x".repeat(200)),
            colors: Colors {
                text: Some(Rgb::new(0)),
                background: Some(Rgb::new(0)),
                my_text: Some(Rgb::new(0)),
                their_text: Some(Rgb::new(0)),
                ..Colors::default()
            },
            app_font: FontChoice::Own("../../secret".into()),
            message_font: FontChoice::Own("x".into()),
            message_size: 255,
            dim: 255,
            blur: 255,
            background: Background::Photo("someone-elses-photo".into()),
            ..Look::default()
        };
        let got = sent.from_someone("new-id".into(), "Noor", "m1");
        assert_eq!(got.id, "new-id");
        assert!(got.name.chars().count() <= MAX_NAME);
        assert!(!got.name.contains('\n'));
        assert_eq!(got.app_font, FontChoice::System);
        assert_eq!(got.message_font, FontChoice::System);
        assert_eq!(got.colors.text, None);
        assert_eq!(got.colors.my_text, None);
        assert_eq!(got.colors.their_text, None);
        assert_eq!((got.message_size, got.dim, got.blur), (150, 90, 20));
        assert_eq!(got.background, Background::None);
        assert_eq!(got.from.as_deref(), Some("Noor"));
        assert_eq!(got.from_message.as_deref(), Some("m1"));
        // Black on black was asked for, and that is not what it gets.
        let p = palette(&got);
        assert!(contrast(p.text, p.bg) >= TEXT);
    }

    #[test]
    fn ready_made_looks_are_known_by_their_key() {
        let all = Look::ready_made();
        assert_eq!(all.len(), 6);
        assert!(all.iter().all(Look::is_ready_made));
        let mine = all[0].copied("id".into(), "Mine".into());
        assert_eq!(
            mine.base,
            Base::Dark,
            "the device's colours are not known here"
        );
        assert!(!mine.is_ready_made());
    }
}
