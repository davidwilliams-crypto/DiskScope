//! Helpers shared by the kittest-based tests of several `app` submodules.
use super::*;

/// Every piece of text a frame paints, in paint order.
pub(super) fn rendered_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    fn collect(shape: &egui::epaint::Shape, texts: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().for_each(|shape| collect(shape, texts));
            }
            _ => {}
        }
    }
    let mut texts = Vec::new();
    shapes
        .iter()
        .for_each(|shape| collect(&shape.shape, &mut texts));
    texts
}

/// Runs `show` for a few frames so windows and tables finish their sizing
/// passes, and returns the last frame.
pub(super) fn settle_frames(
    ctx: &egui::Context,
    mut show: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    let mut output = egui::FullOutput::default();
    for _ in 0..3 {
        output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 900.0),
                )),
                ..Default::default()
            },
            &mut show,
        );
    }
    output
}
