use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
};
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
    pub fn palette(self) -> Palette {
        let rgb = Color32::from_rgb;
        match self {
            Self::Dark => Palette {
                page: rgb(19, 21, 23),
                side: rgb(23, 26, 28),
                surface: rgb(29, 32, 35),
                raised: rgb(37, 42, 45),
                line: rgb(55, 61, 64),
                text: rgb(226, 234, 240),
                muted: rgb(147, 165, 179),
                faint: rgb(157, 170, 177),
                accent: rgb(123, 215, 197),
                wash: rgb(30, 57, 56),
                selected: rgb(33, 60, 67),
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
                accent: rgb(20, 113, 102),
                wash: rgb(224, 242, 237),
                selected: rgb(218, 237, 237),
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
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.muted);
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
        style.spacing.interact_size.y = 26.0;
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(13.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(13.0));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(11.0));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::proportional(20.0));
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(11.0));
        ctx.set_global_style(style);
    }
}

/// System font I/O is confined to initialization, before the update callback.
pub fn load_system_font(ctx: &egui::Context) {
    #[cfg(windows)]
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
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
    pub fn factor(self) -> f32 {
        match self {
            Self::Normal => 1.0,
            Self::Large => 1.25,
            Self::Largest => 1.5,
        }
    }
}
