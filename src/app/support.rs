//! Intentional, bounded ticket evidence. Explicit exports are allowed even in
//! sessions that never write automatic history or preferences.
use super::*;
use diskscope::evidence::{self, EvidenceContext, Redaction};
use serde_json::json;

impl DiskScopeApp {
    fn evidence_snapshot(&self) -> UiSnapshot {
        UiSnapshot {
            progress: self.snapshot.progress.clone(),
            view: self.visible.view.clone(),
        }
    }
    fn evidence_context(&self) -> EvidenceContext {
        let mut context = EvidenceContext::new(self.root_path.clone());
        context.started_utc_epoch_ms = self.scan_started_utc;
        context.finished_utc_epoch_ms = self.scan_finished_utc;
        context.access_context = match self.access_mode {
            AccessMode::Standard => "standard",
            AccessMode::Admin => "administrator",
            AccessMode::AlwaysAdmin => "always-administrator",
            AccessMode::Unknown(_) => "unknown",
        }
        .into();
        context.session_context = self.session_policy.label().into();
        context.view_request = Some(self.request.clone());
        context.redaction = if self.export_redacted {
            Redaction::Redacted
        } else {
            Redaction::Full
        };
        context.action = self.action_review.as_ref().map(|r| json!({
            "path_display":r.path.to_string_lossy(), "root_display":r.root.to_string_lossy(),
            "message":r.message, "recycled":r.recycled, "verification":r.verification(),
            "before_logical_bytes":r.before_logical, "after_logical_bytes":r.after_logical,
            "logical_change_bytes":r.logical_change().map(|n| n.to_string()),
            "observed_available_space_change_bytes":r.volume_change().map(|n|n.to_string()),
            "reconciliation_state":r.state.map(|s|s.label()),
            "note":"Measurements are independent. Available space changes are not attributed reclaimed space. Action time was not independently recorded."
        }));
        context
    }

    pub(super) fn evidence_controls(&mut self, ui: &mut egui::Ui) {
        let response = ui.add_enabled(
            self.generation != 0 && !self.management.busy(),
            egui::Button::new("Export ticket evidence…"),
        );
        if response.clicked() {
            controls::set_dialog_invoker(ui.ctx(), "Export investigation evidence", response.id);
            self.export_open = true;
        }
    }

    pub(super) fn export_window(&mut self, ctx: &egui::Context) {
        if !self.export_open {
            return;
        }
        controls::remember_dialog_focus(ctx, "Export investigation evidence");
        let mut open = true;
        egui::Window::new("Export investigation evidence").open(&mut open)
            .default_width(520.0).vscroll(true).show(ctx, |ui| {
                ui.label("Save a bounded JSON summary for the correct incident ticket. It includes scan coverage, errors, visible rows and any current action review. It is not a full inventory or restorable comparison baseline.");
                ui.label("Displayed rows reflect your current view, filters and page. Export from Largest files for a size-focused list. Existing files are never overwritten.");
                ui.checkbox(&mut self.export_redacted, "Redact customer paths, error messages and action details");
                if !self.export_redacted { ui.label("Full report contains customer filenames, paths and action details. Review it before sharing; use an approved ticket location."); }
                ui.label("Redacted reports retain counts and measurements. Associate the report with its endpoint in your ticket; endpoint identity is not independently verified.");
                let label = ui.label("Destination JSON file (new file only)");
                if ui.add(egui::TextEdit::singleline(&mut self.export_path).desired_width(f32::INFINITY)).labelled_by(label.id).changed() { self.export_native_path = None; }
                if ui.add_enabled(!self.export_busy, egui::Button::new("Choose destination…")).clicked() {
                    self.export_busy = true;
                    let tx = self.native_tx.clone(); let context = ctx.clone();
                    if let Err(error) = std::thread::Builder::new().name("evidence-destination".into()).spawn(move || {
                        let path = rfd::FileDialog::new().set_file_name("DiskScope-evidence.json").add_filter("JSON report", &["json"]).save_file();
                        let _ = tx.send(NativeResult::ExportDestination(path)); context.request_repaint();
                    }) { self.export_busy = false; self.notice = Some(format!("Could not open destination picker: {error}")); }
                }
                if ui.add_enabled(!self.export_busy && !self.export_path.trim().is_empty(), egui::Button::new("Save evidence")).clicked() {
                    self.export_busy = true;
                    let path = self.export_native_path.clone().unwrap_or_else(|| PathBuf::from(&self.export_path));
                    let snapshot = self.evidence_snapshot(); let evidence_context = self.evidence_context();
                    let tx = self.native_tx.clone(); let context = ctx.clone();
                    if let Err(error) = std::thread::Builder::new().name("evidence-export".into()).spawn(move || {
                        let report = evidence::build_report(&snapshot, &evidence_context);
                        let result = evidence::write_report_new(&report, &path).map(|()| path).map_err(|e| e.to_string());
                        let _ = tx.send(NativeResult::Export(result)); context.request_repaint();
                    }) { self.export_busy = false; self.notice = Some(format!("Could not start evidence export: {error}")); }
                }
                if self.export_busy { ui.label("Working in background…"); }
            });
        self.export_open = open;
        if !open {
            controls::restore_dialog_focus(ctx, "Export investigation evidence");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::App;
    #[test]
    fn helpdesk_welcome_and_compact_controls_explain_the_available_workflow() {
        let cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        let mut app = DiskScopeApp::new_session(
            &cc,
            None,
            AccessMode::Standard,
            SessionPolicy {
                helpdesk: true,
                no_history: true,
            },
        );
        let output = test_support::settle_frames(&cc.egui_ctx, |ui| {
            app.overview(ui, app.preferences.theme.palette())
        });
        let text = test_support::rendered_texts(&output.shapes).join(" ");
        assert!(text.contains("03  Evidence"), "{text}");
        assert!(text.contains("does not save comparison history"));
        assert!(!text.contains("03  Compare"));
        assert!(text.contains("disables file removal"));
        let output = test_support::settle_frames(&cc.egui_ctx, |ui| {
            app.compact_header(ui, app.preferences.theme.palette())
        });
        let text = test_support::rendered_texts(&output.shapes).join(" ");
        assert!(text.contains("Helpdesk & evidence"), "{text}");
    }
    #[test]
    fn damaged_baseline_protection_exposes_repair_without_clearing_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        let mut app = DiskScopeApp::with_history_directory(
            &cc,
            None,
            temp.path().join("history"),
            AccessMode::Standard,
        );
        app.history_snapshot = Arc::new(HistorySnapshot {
            message: "Protection metadata damaged; Unprotect or Clear".into(),
            ..Default::default()
        });
        let output = test_support::settle_frames(&cc.egui_ctx, |ui| {
            app.changes_page(ui, app.preferences.theme.palette())
        });
        let text = test_support::rendered_texts(&output.shapes).join(" ");
        assert!(
            text.contains("Release or repair baseline protection"),
            "{text}"
        );
        assert!(text.contains("without clearing saved scans"));
    }
    #[test]
    fn evidence_uses_frozen_visible_rows_with_current_scan_coverage() {
        let cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        let mut app = DiskScopeApp::new_session(
            &cc,
            None,
            AccessMode::Standard,
            SessionPolicy {
                helpdesk: true,
                no_history: true,
            },
        );
        let mut visible = UiSnapshot::default();
        visible.view.generation = 7;
        visible.view.revision = 3;
        let mut newest = visible.clone();
        newest.view.revision = 4;
        newest.progress.generation = 7;
        newest.progress.files = 25;
        app.visible = Arc::new(visible);
        app.snapshot = Arc::new(newest);
        let export = app.evidence_snapshot();
        assert_eq!(export.view.revision, 3);
        assert_eq!(export.progress.files, 25);
    }
    #[test]
    fn helpdesk_session_never_reads_or_writes_preferences_or_starts_history() {
        struct NoStorage;
        impl eframe::Storage for NoStorage {
            fn get_string(&self, _: &str) -> Option<String> {
                panic!("session read profile");
            }
            fn set_string(&mut self, _: &str, _: String) {
                panic!("session wrote profile");
            }
            fn flush(&mut self) {}
        }
        let temp = tempfile::tempdir().unwrap();
        for policy in [
            SessionPolicy {
                helpdesk: true,
                no_history: false,
            },
            SessionPolicy {
                helpdesk: false,
                no_history: true,
            },
        ] {
            let storage = NoStorage;
            let mut cc = eframe::CreationContext::_new_kittest(egui::Context::default());
            cc.storage = Some(&storage);
            let directory = temp.path().join("must-not-create-history");
            let mut app = DiskScopeApp::with_session_directory(
                &cc,
                None,
                directory.clone(),
                AccessMode::Standard,
                policy,
            );
            assert!(!app.persistence_allowed());
            assert!(!app.preferences.ntfs_preview);
            app.save(&mut NoStorage);
            assert!(!app.persist_egui_memory());
            assert!(!directory.exists());
            let mut options = eframe::NativeOptions::default();
            configure_session_options(policy, &mut options);
            assert!(!options.persist_window);
            assert_eq!(options.persistence_path, Some(PathBuf::from(r"\\.\NUL")));
        }
    }
}
