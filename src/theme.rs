use eframe::egui::{self, Color32, FontId, Stroke, TextStyle};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    HighContrast,
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub page: Color32,
    pub side: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub wash: Color32,
    pub selected: Color32,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Dark, Theme::Light, Theme::HighContrast];
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Graphite",
            Self::Light => "Warm light",
            Self::HighContrast => "High contrast",
        }
    }
    /// The next theme in [`Theme::ALL`], wrapping around.
    pub fn next(self) -> Theme {
        let index = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
    pub fn on_accent(self) -> Color32 {
        match self {
            Self::Light => Color32::WHITE,
            Self::Dark | Self::HighContrast => Color32::from_rgb(17, 23, 30),
        }
    }

    pub fn palette(self) -> Palette {
        let rgb = Color32::from_rgb;
        match self {
            Self::Dark => Palette {
                page: rgb(17, 23, 30),
                side: rgb(13, 18, 24),
                surface: rgb(23, 30, 38),
                raised: rgb(34, 43, 53),
                line: rgb(63, 75, 87),
                text: rgb(236, 239, 242),
                muted: rgb(171, 184, 196),
                faint: rgb(155, 170, 183),
                accent: rgb(238, 168, 111),
                wash: rgb(61, 43, 32),
                selected: rgb(59, 48, 39),
            },
            Self::Light => Palette {
                page: rgb(241, 238, 232),
                side: rgb(246, 243, 237),
                surface: rgb(255, 253, 248),
                raised: rgb(239, 236, 228),
                line: rgb(218, 214, 204),
                text: rgb(31, 48, 60),
                muted: rgb(84, 106, 121),
                faint: rgb(87, 103, 110),
                accent: rgb(151, 67, 29),
                wash: rgb(249, 232, 217),
                selected: rgb(244, 223, 202),
            },
            Self::HighContrast => Palette {
                page: rgb(0, 0, 0),
                side: rgb(0, 0, 0),
                surface: rgb(0, 0, 0),
                raised: rgb(20, 20, 20),
                line: rgb(210, 210, 210),
                text: rgb(255, 255, 255),
                muted: rgb(240, 240, 240),
                faint: rgb(220, 220, 220),
                accent: rgb(255, 235, 0),
                wash: rgb(45, 45, 0),
                selected: rgb(0, 55, 75),
            },
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        let p = self.palette();
        let mut visuals = match self {
            Self::Dark | Self::HighContrast => egui::Visuals::dark(),
            Self::Light => egui::Visuals::light(),
        };
        visuals.override_text_color = Some(p.text);
        visuals.panel_fill = p.surface;
        visuals.window_fill = p.page;
        visuals.extreme_bg_color = p.side;
        visuals.faint_bg_color = p.side;
        visuals.selection.bg_fill = p.selected;
        visuals.selection.stroke = Stroke::new(1.0_f32, p.accent);
        visuals.hyperlink_color = p.accent;
        visuals.widgets.noninteractive.bg_fill = p.surface;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.line);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
        visuals.widgets.inactive.bg_fill = p.raised;
        visuals.widgets.inactive.weak_bg_fill = p.surface;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.line);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
        visuals.widgets.hovered.bg_fill = p.raised;
        visuals.widgets.hovered.weak_bg_fill = p.raised;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);
        visuals.widgets.active.bg_fill = p.wash;
        visuals.widgets.active.weak_bg_fill = p.wash;
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.accent);
        ctx.set_visuals(visuals);
        let mut style = (*ctx.global_style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 9.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.interact_size.y = 32.0;
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(12.0));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::proportional(22.0));
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(11.0));
        ctx.set_global_style(style);
    }
}

/// System font I/O is confined to initialization, before the update callback.
/// The font is read from the real Windows directory, so installations on a
/// drive other than `C:` still get Segoe UI; failures keep egui's bundled font.
pub fn load_system_font(ctx: &egui::Context) {
    #[cfg(windows)]
    {
        use eframe::egui::{FontData, FontDefinitions, FontFamily};
        let Ok(directory) = crate::app::windows_directory() else {
            return;
        };
        if let Ok(bytes) = std::fs::read(directory.join("Fonts").join("segoeui.ttf")) {
            let mut definitions = FontDefinitions::default();
            definitions
                .font_data
                .insert("Segoe UI".into(), FontData::from_owned(bytes).into());
            definitions
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "Segoe UI".into());
            ctx.set_fonts(definitions);
        }
    }
    #[cfg(not(windows))]
    let _ = ctx;
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

    #[test]
    fn readable_text_and_primary_actions_in_every_theme() {
        for theme in [Theme::Dark, Theme::Light, Theme::HighContrast] {
            let p = theme.palette();
            for (foreground, background) in [
                (p.text, p.surface),
                (p.muted, p.surface),
                (p.faint, p.side),
                (p.accent, p.wash),
                (theme.on_accent(), p.accent),
            ] {
                let a = luminance(foreground);
                let b = luminance(background);
                assert!(
                    (a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5,
                    "Insufficient text contrast in {theme:?}"
                );
            }
        }
    }
}
