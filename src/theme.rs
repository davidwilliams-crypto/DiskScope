use eframe::egui::{self, Color32, FontId, Stroke, TextStyle};
use serde::{Deserialize, Serialize};

/// The serialized names are stable: preferences saved by earlier builds keep
/// their theme. Only the labels and values follow the design system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    HighContrast,
}

/// Meridian radius tokens, in logical points. An inner radius is always
/// smaller than the radius of the surface that holds it.
pub const RADIUS_SM: u8 = 8;
pub const RADIUS_MD: u8 = 12;
pub const RADIUS_LG: u8 = 20;
pub const RADIUS_XL: u8 = 28;
/// Radius of thin bars and badges (`radius-full`).
pub const RADIUS_FULL: u8 = 255;
/// Meridian spacing steps used directly by layout code.
pub const SPACE_3: f32 = 12.0;
pub const SPACE_5: i8 = 20;
/// Meridian type scale sizes that are not egui text styles.
pub const TYPE_LABEL: f32 = 11.0;
pub const TYPE_H3: f32 = 16.0;
pub const TYPE_H2: f32 = 20.0;
pub const TYPE_H1: f32 = 28.0;
pub const TYPE_METRIC: f32 = 32.0;

/// The Meridian colour tokens. Field names follow the design system's token
/// names so a rule such as "cards are `surface` on `canvas`" reads the same
/// in the brand book and in the code. High contrast is not a Meridian theme;
/// it fills the same roles with its own legibility-first values.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Page ground behind every card.
    pub canvas: Color32,
    /// Card, dialog and control ground.
    pub surface: Color32,
    /// Inset areas: tracks, list rows, table headers, the status strip.
    pub sunken: Color32,
    /// Quiet accent fill: selected rows and chips, pressed controls.
    pub tint: Color32,
    /// The one dark emphasis panel a view may carry.
    pub surface_ink: Color32,
    /// Decorative hairlines only. Carries no meaning.
    pub border: Color32,
    /// Control outlines that must be seen (3:1 on every ground).
    pub border_strong: Color32,
    /// Primary text.
    pub text: Color32,
    /// Labels, captions and helper text.
    pub muted: Color32,
    pub on_ink: Color32,
    pub on_ink_muted: Color32,
    /// The single brand hue, tuned for fills. Never used for text.
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    /// Accent-coloured text, links and icons.
    pub accent_text: Color32,
    /// Keyboard focus ring.
    pub focus: Color32,
    /// Status text. Each reads on `surface`, `canvas` and its own tint, and
    /// is always paired with a word.
    pub positive: Color32,
    pub positive_tint: Color32,
    pub warning: Color32,
    pub warning_tint: Color32,
    pub danger: Color32,
    pub danger_tint: Color32,
    /// Meridian's dark `shadow-card` is a faint outline rather than a shadow;
    /// transparent where the shadow does the work instead.
    pub card_outline: Color32,
    pub float_outline: Color32,
    /// Resting cards. Nothing else rests with a shadow.
    pub card_shadow: egui::Shadow,
    /// Menus, toasts and dialogs: anything floating above cards.
    pub float_shadow: egui::Shadow,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Dark, Theme::Light, Theme::HighContrast];
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::HighContrast => "High contrast",
        }
    }
    /// The next theme in [`Theme::ALL`], wrapping around.
    pub fn next(self) -> Theme {
        let index = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub fn palette(self) -> Palette {
        let rgb = Color32::from_rgb;
        let hex = |value: u32| rgb((value >> 16) as u8, (value >> 8) as u8, value as u8);
        let flat = egui::Shadow {
            offset: [0, 0],
            blur: 0,
            spread: 0,
            color: Color32::TRANSPARENT,
        };
        match self {
            Self::Dark => Palette {
                canvas: hex(0x070b16),
                surface: hex(0x10172a),
                sunken: hex(0x0a101f),
                tint: hex(0x1a2c5e),
                surface_ink: hex(0x1c2540),
                border: hex(0x222b45),
                border_strong: hex(0x66749a),
                text: hex(0xeef2fb),
                muted: hex(0x9aa6c1),
                on_ink: hex(0xeef2fb),
                on_ink_muted: hex(0xa3afc9),
                accent: hex(0x7da2ff),
                accent_hover: hex(0x9bb8ff),
                on_accent: hex(0x06102b),
                accent_text: hex(0x9bb8ff),
                focus: hex(0x9bb8ff),
                positive: hex(0x4fd1a5),
                positive_tint: hex(0x0f2f29),
                warning: hex(0xf2b84b),
                warning_tint: hex(0x33270d),
                danger: hex(0xff8a73),
                danger_tint: hex(0x3a1712),
                // rgba(255,255,255,0.05) and 0.08 flattened onto `surface`,
                // so the outline is the same on any backend.
                card_outline: hex(0x1c2335),
                float_outline: hex(0x232a3b),
                card_shadow: flat,
                float_shadow: egui::Shadow {
                    offset: [0, 16],
                    blur: 40,
                    spread: 0,
                    color: Color32::from_black_alpha(128),
                },
            },
            Self::Light => Palette {
                canvas: hex(0xe8edf7),
                surface: hex(0xffffff),
                sunken: hex(0xf2f5fb),
                tint: hex(0xdbe6fe),
                surface_ink: hex(0x0b1433),
                border: hex(0xdbe2f0),
                border_strong: hex(0x7b88a4),
                text: hex(0x0b1433),
                muted: hex(0x4f5a73),
                on_ink: hex(0xffffff),
                on_ink_muted: hex(0xa9b6d6),
                accent: hex(0x2152e5),
                accent_hover: hex(0x1a43c4),
                on_accent: hex(0xffffff),
                accent_text: hex(0x1c47cc),
                focus: hex(0x2152e5),
                positive: hex(0x0a6e55),
                positive_tint: hex(0xd7f2e8),
                warning: hex(0x855000),
                warning_tint: hex(0xfdefd0),
                danger: hex(0xb42a18),
                danger_tint: hex(0xfde3de),
                card_outline: Color32::TRANSPARENT,
                float_outline: Color32::TRANSPARENT,
                // Meridian's two-layer shadows folded into the single layer
                // egui paints: the wide soft layer carries the elevation.
                card_shadow: egui::Shadow {
                    offset: [0, 6],
                    blur: 20,
                    spread: 0,
                    color: Color32::from_rgba_unmultiplied(11, 20, 51, 18),
                },
                float_shadow: egui::Shadow {
                    offset: [0, 16],
                    blur: 40,
                    spread: 0,
                    color: Color32::from_rgba_unmultiplied(11, 20, 51, 44),
                },
            },
            Self::HighContrast => Palette {
                canvas: rgb(0, 0, 0),
                surface: rgb(0, 0, 0),
                sunken: rgb(20, 20, 20),
                tint: rgb(0, 55, 75),
                surface_ink: rgb(20, 20, 20),
                border: rgb(210, 210, 210),
                border_strong: rgb(255, 255, 255),
                text: rgb(255, 255, 255),
                muted: rgb(240, 240, 240),
                on_ink: rgb(255, 255, 255),
                on_ink_muted: rgb(240, 240, 240),
                accent: rgb(255, 235, 0),
                accent_hover: rgb(255, 245, 130),
                on_accent: rgb(17, 23, 30),
                accent_text: rgb(255, 235, 0),
                // Distinct from both the white outlines and the yellow fills.
                focus: rgb(110, 220, 255),
                positive: rgb(120, 255, 150),
                positive_tint: rgb(0, 45, 20),
                warning: rgb(255, 220, 60),
                warning_tint: rgb(45, 38, 0),
                danger: rgb(255, 130, 130),
                danger_tint: rgb(60, 0, 0),
                // With no shadow and no tonal step, the outline is the card.
                card_outline: rgb(210, 210, 210),
                float_outline: rgb(255, 255, 255),
                card_shadow: flat,
                float_shadow: flat,
            },
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        let p = self.palette();
        let mut visuals = match self {
            Self::Dark | Self::HighContrast => egui::Visuals::dark(),
            Self::Light => egui::Visuals::light(),
        };
        let radius = egui::CornerRadius::same(RADIUS_SM);
        visuals.override_text_color = Some(p.text);
        visuals.panel_fill = p.canvas;
        visuals.window_fill = p.surface;
        visuals.window_stroke = Stroke::new(1.0_f32, p.float_outline);
        visuals.window_shadow = p.float_shadow;
        visuals.popup_shadow = p.float_shadow;
        visuals.window_corner_radius = egui::CornerRadius::same(RADIUS_XL);
        visuals.menu_corner_radius = egui::CornerRadius::same(RADIUS_MD);
        visuals.extreme_bg_color = p.sunken;
        visuals.faint_bg_color = p.sunken;
        visuals.text_edit_bg_color = Some(p.surface);
        visuals.code_bg_color = p.sunken;
        visuals.warn_fg_color = p.warning;
        visuals.error_fg_color = p.danger;
        visuals.hyperlink_color = p.accent_text;
        visuals.disabled_alpha = 0.45;
        visuals.selection.bg_fill = p.tint;
        // egui reuses this stroke as the text colour of selected items and
        // selected text (both on `tint`) and as a focused field's outline.
        visuals.selection.stroke = Stroke::new(2.0_f32, p.accent_text);
        let w = &mut visuals.widgets;
        w.noninteractive.bg_fill = p.surface;
        w.noninteractive.weak_bg_fill = p.surface;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
        w.noninteractive.corner_radius = radius;
        // Resting controls are Meridian's ghost button: no fill, an outline a
        // person can see. Filled accent is reserved for the primary action.
        w.inactive.bg_fill = p.surface;
        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
        w.inactive.bg_stroke = Stroke::new(1.0_f32, p.border_strong);
        w.inactive.fg_stroke = Stroke::new(1.5_f32, p.text);
        w.inactive.corner_radius = radius;
        w.inactive.expansion = 0.0;
        w.hovered.bg_fill = p.sunken;
        w.hovered.weak_bg_fill = p.sunken;
        w.hovered.bg_stroke = Stroke::new(1.0_f32, p.border_strong);
        w.hovered.fg_stroke = Stroke::new(1.5_f32, p.text);
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;
        // egui paints a keyboard-focused widget with the active visuals, so
        // this stroke is the focus ring as well as the pressed state.
        w.active.bg_fill = p.tint;
        w.active.weak_bg_fill = p.tint;
        w.active.bg_stroke = Stroke::new(2.0_f32, p.focus);
        w.active.fg_stroke = Stroke::new(1.5_f32, p.text);
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;
        w.open.bg_fill = p.sunken;
        w.open.weak_bg_fill = p.sunken;
        w.open.bg_stroke = Stroke::new(1.0_f32, p.border_strong);
        w.open.fg_stroke = Stroke::new(1.5_f32, p.text);
        w.open.corner_radius = radius;
        w.open.expansion = 0.0;
        ctx.set_visuals(visuals);
        let heading = if semibold_loaded(ctx) {
            FontId::new(TYPE_H3, egui::FontFamily::Name(SEMIBOLD.into()))
        } else {
            FontId::proportional(TYPE_H3)
        };
        let mut style = (*ctx.global_style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size.y = 32.0;
        style.spacing.window_margin = egui::Margin::same(SPACE_5);
        style.spacing.menu_margin = egui::Margin::same(8);
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(12.0));
        style.text_styles.insert(TextStyle::Heading, heading);
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(12.0));
        ctx.set_global_style(style);
    }
}

const SEMIBOLD: &str = "semibold";
fn semibold_flag() -> egui::Id {
    egui::Id::new("diskscope_semibold_font_loaded")
}
/// Whether this context registered the semibold family. egui panics on an
/// unregistered family, so every use asks first.
fn semibold_loaded(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(semibold_flag()).unwrap_or(false))
}
/// Meridian headings and numerals: weight 600 at a size from the type scale.
/// Falls back to the regular face in a context that never installed fonts.
pub fn title(ui: &egui::Ui, text: impl Into<String>, size: f32) -> egui::RichText {
    let text = egui::RichText::new(text).size(size);
    if semibold_loaded(ui.ctx()) {
        text.family(egui::FontFamily::Name(SEMIBOLD.into()))
    } else {
        text.strong()
    }
}
/// The dialog surface: `surface` at `radius-xl` under `shadow-pop`.
pub fn dialog_frame(p: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0_f32, p.float_outline))
        .corner_radius(RADIUS_XL)
        .inner_margin(egui::Margin::same(SPACE_5 + 4))
        .shadow(p.float_shadow)
}

/// Installs Meridian's typefaces. Geist and Geist Mono are embedded in the
/// executable (SIL Open Font License; see `assets/fonts/`), so the interface
/// looks the same on every machine and needs no font I/O for them. Segoe UI
/// follows as the fallback for glyphs Geist does not carry; it is read once,
/// here, from the real Windows directory, before the update callback runs.
pub fn install_fonts(ctx: &egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};
    let mut definitions = FontDefinitions::default();
    for (name, bytes) in [
        (
            "Geist",
            &include_bytes!("../assets/fonts/Geist-Regular.ttf")[..],
        ),
        (
            "Geist SemiBold",
            &include_bytes!("../assets/fonts/Geist-SemiBold.ttf")[..],
        ),
        (
            "Geist Mono",
            &include_bytes!("../assets/fonts/GeistMono-Regular.ttf")[..],
        ),
    ] {
        definitions
            .font_data
            .insert(name.into(), FontData::from_static(bytes).into());
    }
    let mut fallbacks = definitions
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    #[cfg(windows)]
    if let Ok(directory) = crate::app::windows_directory()
        && let Ok(bytes) = std::fs::read(directory.join("Fonts").join("segoeui.ttf"))
    {
        definitions
            .font_data
            .insert("Segoe UI".into(), FontData::from_owned(bytes).into());
        fallbacks.insert(0, "Segoe UI".into());
    }
    let with = |first: &str| {
        let mut family = vec![first.to_owned()];
        family.extend(fallbacks.iter().cloned());
        family
    };
    definitions
        .families
        .insert(FontFamily::Proportional, with("Geist"));
    definitions
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), with("Geist SemiBold"));
    definitions
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "Geist Mono".into());
    ctx.set_fonts(definitions);
    ctx.data_mut(|data| data.insert_temp(semibold_flag(), true));
}

/// Linear gamma-space blend of two opaque colors. Used for bevels and hover
/// tints, where perceptual correctness matters less than cheap smoothness.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let channel = |x: u8, y: u8| x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0);
    Color32::from_rgb(
        channel(a.r(), b.r()) as u8,
        channel(a.g(), b.g()) as u8,
        channel(a.b(), b.b()) as u8,
    )
}
/// A color shifted toward the light source; used for the top/left edges of
/// raised surfaces such as map tiles.
pub fn lighten(color: Color32, t: f32) -> Color32 {
    mix(color, Color32::WHITE, t)
}
/// A color shifted away from the light source; bottom/right edges.
pub fn darken(color: Color32, t: f32) -> Color32 {
    mix(color, Color32::BLACK, t)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextScale {
    #[default]
    Normal,
    Large,
    Largest,
}

impl TextScale {
    pub const ALL: [TextScale; 3] = [TextScale::Normal, TextScale::Large, TextScale::Largest];
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "100%",
            Self::Large => "125%",
            Self::Largest => "150%",
        }
    }
    pub fn factor(self) -> f32 {
        match self {
            Self::Normal => 1.0,
            Self::Large => 1.25,
            Self::Largest => 1.5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color32) -> f32 {
        let linear = |channel: u8| {
            let value = f32::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    }

    fn contrast(foreground: Color32, background: Color32) -> f32 {
        let a = luminance(foreground);
        let b = luminance(background);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn readable_text_and_primary_actions_in_every_theme() {
        for theme in Theme::ALL {
            let p = theme.palette();
            for (name, foreground, background) in [
                ("text on canvas", p.text, p.canvas),
                ("text on surface", p.text, p.surface),
                ("text on sunken", p.text, p.sunken),
                ("text on tint", p.text, p.tint),
                ("muted on canvas", p.muted, p.canvas),
                ("muted on surface", p.muted, p.surface),
                ("muted on sunken", p.muted, p.sunken),
                ("muted on tint", p.muted, p.tint),
                ("accent text on canvas", p.accent_text, p.canvas),
                ("accent text on surface", p.accent_text, p.surface),
                ("accent text on sunken", p.accent_text, p.sunken),
                ("accent text on tint", p.accent_text, p.tint),
                ("on-accent on accent", p.on_accent, p.accent),
                ("on-accent on accent hover", p.on_accent, p.accent_hover),
                ("on-ink on ink", p.on_ink, p.surface_ink),
                ("on-ink muted on ink", p.on_ink_muted, p.surface_ink),
                ("positive on canvas", p.positive, p.canvas),
                ("positive on surface", p.positive, p.surface),
                ("positive on its tint", p.positive, p.positive_tint),
                ("warning on canvas", p.warning, p.canvas),
                ("warning on surface", p.warning, p.surface),
                ("warning on its tint", p.warning, p.warning_tint),
                ("danger on canvas", p.danger, p.canvas),
                ("danger on surface", p.danger, p.surface),
                ("danger on its tint", p.danger, p.danger_tint),
            ] {
                let ratio = contrast(foreground, background);
                assert!(ratio >= 4.5, "{name} is {ratio:.2}:1 in {theme:?}");
            }
        }
    }

    #[test]
    fn outlines_and_focus_rings_are_visible_in_every_theme() {
        for theme in Theme::ALL {
            let p = theme.palette();
            for (name, mark, ground) in [
                ("control outline on canvas", p.border_strong, p.canvas),
                ("control outline on surface", p.border_strong, p.surface),
                ("control outline on sunken", p.border_strong, p.sunken),
                ("focus on canvas", p.focus, p.canvas),
                ("focus on surface", p.focus, p.surface),
                ("focus on sunken", p.focus, p.sunken),
                ("accent fill on canvas", p.accent, p.canvas),
                ("accent fill on surface", p.accent, p.surface),
            ] {
                let ratio = contrast(mark, ground);
                assert!(ratio >= 3.0, "{name} is {ratio:.2}:1 in {theme:?}");
            }
        }
    }

    #[test]
    fn saved_theme_names_still_load() {
        for (saved, theme) in [
            ("\"Dark\"", Theme::Dark),
            ("\"Light\"", Theme::Light),
            ("\"HighContrast\"", Theme::HighContrast),
        ] {
            assert_eq!(serde_json::from_str::<Theme>(saved).unwrap(), theme);
        }
    }
}
