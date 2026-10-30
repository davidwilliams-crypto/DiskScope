//! Stable, polite scan feedback; avoid announcing every discovered path.
use super::*;

#[derive(Default)]
pub(super) struct ScanFeedback {
    key: Option<(u64, ScanState, u64)>,
    updated: Option<Instant>,
    text: String,
}

impl ScanFeedback {
    fn observe(&mut self, progress: &Progress, state: ScanState, now: Instant) -> &str {
        let key = (
            progress.generation,
            state,
            if state.is_active() {
                0
            } else {
                progress.errors_total
            },
        );
        if self.key != Some(key)
            || self.updated.is_none_or(|updated| {
                now.saturating_duration_since(updated) >= Duration::from_secs(5)
            })
        {
            self.text = if state == ScanState::Idle {
                "Ready to explore".into()
            } else {
                format!(
                    "{} · {} files · {} errors",
                    state.label(),
                    format_count(progress.files),
                    format_count(progress.errors_total)
                )
            };
            self.key = Some(key);
            self.updated = Some(now);
        }
        &self.text
    }
}

impl DiskScopeApp {
    pub(super) fn accessible_scan_status(&mut self, ui: &mut egui::Ui, p: Palette) {
        let state = self.scan_state();
        let text = self
            .scan_feedback
            .observe(&self.snapshot.progress, state, Instant::now());
        ui.push_id("live_scan_status", |ui| {
            let response = ui.label(RichText::new(text).small().color(p.muted));
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_role(egui::accesskit::Role::Status);
                node.set_live(egui::accesskit::Live::Polite);
                node.set_live_atomic();
                node.set_value(text);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_status_exposes_polite_atomic_feedback_and_changes_with_terminal_state() {
        let cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        cc.egui_ctx.enable_accesskit();
        let mut app = DiskScopeApp::new_session(
            &cc,
            None,
            AccessMode::Standard,
            SessionPolicy {
                helpdesk: true,
                no_history: true,
            },
        );
        Arc::make_mut(&mut app.snapshot).progress = Progress {
            generation: 1,
            state: ScanState::Partial,
            files: 12,
            errors_total: 3,
            ..Default::default()
        };
        let output = test_support::settle_frames(&cc.egui_ctx, |ui| {
            app.accessible_scan_status(ui, app.preferences.theme.palette())
        });
        let update = output.platform_output.accesskit_update.unwrap();
        let status = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == egui::accesskit::Role::Status)
            .unwrap();
        assert_eq!(status.1.live(), Some(egui::accesskit::Live::Polite));
        assert!(status.1.is_live_atomic());
        assert!(status.1.value().unwrap().contains("12 files"));
        assert!(status.1.value().unwrap().contains("3 errors"));
    }
    #[test]
    fn active_counts_are_throttled_but_terminal_errors_are_immediate_without_paths() {
        let now = Instant::now();
        let mut feedback = ScanFeedback::default();
        let mut progress = Progress {
            generation: 1,
            files: 5,
            errors_total: 0,
            current_path: "private-customer".into(),
            ..Default::default()
        };
        let first = feedback
            .observe(&progress, ScanState::Scanning, now)
            .to_owned();
        progress.files = 99;
        assert_eq!(
            feedback.observe(&progress, ScanState::Scanning, now + Duration::from_secs(1)),
            first
        );
        assert!(
            feedback
                .observe(&progress, ScanState::Scanning, now + Duration::from_secs(6))
                .contains("99")
        );
        progress.errors_total = 2;
        let terminal =
            feedback.observe(&progress, ScanState::Partial, now + Duration::from_secs(7));
        assert!(terminal.contains("2 errors"));
        assert!(!terminal.contains("private-customer"));
        progress.generation = 2;
        progress.files = 0;
        assert!(
            feedback
                .observe(
                    &progress,
                    ScanState::Preparing,
                    now + Duration::from_secs(7)
                )
                .contains("0 files")
        );
    }
}
