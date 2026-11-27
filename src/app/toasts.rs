use super::*;
use std::time::Duration;

/// Transient outcome feedback floating above the workspace. The inline notice
/// line stays for navigation facts; toasts carry events the user should see
/// even while looking elsewhere (scan finished, evidence saved, reveal
/// failed). They never block input and never outstay their welcome.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastKind {
    Info,
    Success,
    Warning,
    Failure,
}

impl ToastKind {
    fn glyph(self) -> &'static str {
        match self {
            // One status dot, as on a Meridian badge: the colour and the
            // sentence beside it carry the meaning. The check and warning
            // glyphs are missing from Segoe UI and painted as empty boxes.
            Self::Info | Self::Success | Self::Warning | Self::Failure => "●",
        }
    }
    fn color(self, p: Palette) -> Color32 {
        match self {
            Self::Info => p.accent_text,
            Self::Success => p.positive,
            Self::Warning => p.warning,
            Self::Failure => p.danger,
        }
    }
    fn lifetime(self) -> Duration {
        match self {
            Self::Info | Self::Success => Duration::from_secs(6),
            Self::Warning | Self::Failure => Duration::from_secs(9),
        }
    }
}

struct Toast {
    kind: ToastKind,
    text: String,
    /// Countdown decremented only while frames are actually painted, so a
    /// toast raised while the window is minimized is not silently spent
    /// before the user ever sees it.
    remaining: Duration,
}

impl Toast {
    fn lifetime(&self) -> Duration {
        self.kind.lifetime()
    }
    fn entrance(&self) -> f32 {
        let age = (self.lifetime() - self.remaining).as_secs_f32();
        (age / 0.15).min(1.0)
    }
}

#[derive(Default)]
pub(super) struct Toasts {
    items: Vec<Toast>,
}

/// The floating column is capped so a burst of events cannot wallpaper the
/// workspace; the newest toast replaces the oldest overflow.
const MAX_TOASTS: usize = 4;
/// The Area anchor's offset from the bottom-right corner of the content
/// rect: inside the window margin, clear of the status bar.
const TOAST_MARGIN: egui::Vec2 = egui::Vec2::new(-18.0, -60.0);

impl Toasts {
    /// Whether any toast is live and still needs frames to count down.
    pub(super) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub(super) fn push(&mut self, kind: ToastKind, text: impl Into<String>) {
        if self.items.len() >= MAX_TOASTS {
            self.items.remove(0);
        }
        self.items.push(Toast {
            kind,
            text: text.into(),
            remaining: kind.lifetime(),
        });
    }

    /// Overlay anchored to the bottom-right corner above the status bar —
    /// the Windows-native corner for transient notices, and clear of the
    /// header actions the top-right carries. Toasts fade in over ~150 ms
    /// (instantly when reduced motion is on); rendering is skipped entirely
    /// when nothing is live, so tests and quiet frames are unaffected.
    /// The anchor offset is applied to the already-aligned corner (egui adds
    /// it verbatim), so both components must stay small and negative.
    ///
    /// While a dialog is open (`behind_dialog`) the stack is display-only:
    /// its dismiss buttons must not join the dialog's Tab order or take
    /// clicks. A toast under the pointer is being read, so its countdown
    /// waits.
    pub(super) fn render(&mut self, ctx: &egui::Context, p: Palette, behind_dialog: bool) {
        if self.items.is_empty() {
            return;
        }
        let hovered_id = egui::Id::new("toast_stack_hovered");
        let reading = ctx.data(|d| d.get_temp::<bool>(hovered_id).unwrap_or(false));
        let dt = if reading {
            0.0
        } else {
            ctx.input(|i| i.unstable_dt)
        };
        for toast in &mut self.items {
            toast.remaining = toast.remaining.saturating_sub(Duration::from_secs_f32(dt));
        }
        self.items.retain(|toast| toast.remaining > Duration::ZERO);
        if self.items.is_empty() {
            ctx.data_mut(|d| d.insert_temp(hovered_id, false));
            return;
        }
        let stack = egui::Area::new(egui::Id::new("toast_stack"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, TOAST_MARGIN)
            .interactable(!behind_dialog)
            .show(ctx, |ui| {
                ui.set_max_width(430.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                let motion = ui.ctx().global_style().animation_time > 0.0;
                let mut expired = Vec::new();
                for (index, toast) in self.items.iter().enumerate() {
                    // Ease-out cubic over the entrance; data stays exact,
                    // only presence animates.
                    let raw = if motion { toast.entrance() } else { 1.0 };
                    let entrance = 1.0 - (1.0 - raw).powi(3);
                    if entrance < 1.0 {
                        ui.ctx().request_repaint();
                    }
                    ui.set_opacity(entrance);
                    let color = toast.kind.color(p);
                    // A floating surface under `shadow-pop`. The glyph and
                    // the sentence carry the status; the frame stays neutral.
                    let frame = egui::Frame::new()
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0_f32, p.float_outline))
                        .corner_radius(RADIUS_MD)
                        .inner_margin(egui::Margin::symmetric(16, 12))
                        .shadow(p.float_shadow);
                    frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(color, RichText::new(toast.kind.glyph()).strong());
                            let text = ui.add(
                                egui::Label::new(
                                    RichText::new(toast.text.as_str()).small().color(p.text),
                                )
                                .wrap(),
                            );
                            // Toasts replaced persistent inline text for
                            // reveal and export outcomes, so they must carry
                            // the same polite live-region semantics.
                            text.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Label,
                                    true,
                                    toast.text.as_str(),
                                )
                            });
                            ui.ctx().accesskit_node_builder(text.id, |node| {
                                node.set_role(egui::accesskit::Role::Status);
                                node.set_live(egui::accesskit::Live::Polite);
                                node.set_live_atomic();
                                node.set_value(toast.text.as_str());
                            });
                            let dismiss = ui.add(
                                egui::Button::new(RichText::new("×").small().color(p.muted))
                                    .frame(false),
                            );
                            dismiss.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    true,
                                    "Dismiss notification",
                                )
                            });
                            if dismiss.clicked() {
                                expired.push(index);
                            }
                        });
                    });
                }
                ui.set_opacity(1.0);
                for index in expired.into_iter().rev() {
                    self.items.remove(index);
                }
            });
        let pointer_over = ctx
            .pointer_latest_pos()
            .is_some_and(|pointer| stack.response.rect.contains(pointer));
        ctx.data_mut(|d| d.insert_temp(hovered_id, pointer_over && !self.items.is_empty()));
    }
    #[cfg(test)]
    pub(super) fn last_text(&self) -> Option<&str> {
        self.items.last().map(|toast| toast.text.as_str())
    }
    #[cfg(test)]
    pub(super) fn any_text_contains(&self, needle: &str) -> bool {
        self.items.iter().any(|toast| toast.text.contains(needle))
    }
    #[cfg(test)]
    fn expire_all(&mut self) {
        for toast in &mut self.items {
            toast.remaining = Duration::ZERO;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn toasts() -> Toasts {
        let mut toasts = Toasts::default();
        toasts.push(ToastKind::Info, "one");
        toasts
    }

    #[test]
    fn overflow_replaces_the_oldest_toast() {
        let mut toasts = toasts();
        for n in 0..MAX_TOASTS {
            toasts.push(ToastKind::Success, format!("event {n}"));
        }
        assert_eq!(toasts.items.len(), MAX_TOASTS);
        assert_eq!(toasts.items[0].text, "event 0");
        toasts.push(ToastKind::Failure, "urgent");
        assert_eq!(toasts.items.len(), MAX_TOASTS);
        assert_eq!(toasts.items[0].text, "event 1");
        assert_eq!(toasts.items.last().unwrap().text, "urgent");
    }

    #[test]
    fn countdown_expiry_clears_the_stack() {
        let mut toasts = toasts();
        assert_eq!(toasts.items.len(), 1);
        toasts.expire_all();
        toasts
            .items
            .retain(|toast| toast.remaining > Duration::ZERO);
        assert!(toasts.items.is_empty());
    }

    /// The stack must live INSIDE the window, in the bottom-right corner
    /// above where the status bar sits. egui adds the anchor offset to the
    /// aligned corner verbatim, so a screen-sized offset would push the
    /// whole stack below the window edge — this pins the geometry.
    #[test]
    fn toast_stack_anchors_inside_the_bottom_right_corner() {
        let ctx = egui::Context::default();
        let mut toasts = toasts();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        for _ in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| toasts.render(ui.ctx(), Theme::Dark.palette(), false),
            );
        }
        let rect = ctx
            .memory(|m| m.area_rect(egui::Id::new("toast_stack")))
            .expect("toast stack must lay out");
        assert!(
            screen.contains(rect.min) && screen.contains(rect.max),
            "toast stack escapes the window: {rect:?} in {screen:?}"
        );
        // Bottom-right corner, above the ~60px status-bar margin.
        assert!(
            rect.max.y <= screen.max.y - 40.0,
            "toast stack too low: {rect:?}"
        );
        assert!(
            rect.max.x >= screen.max.x - 30.0,
            "toast stack not right-aligned: {rect:?}"
        );
    }
}
