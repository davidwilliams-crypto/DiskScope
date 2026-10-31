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
        context.incident_ticket = (!self.incident_ticket.trim().is_empty())
            .then(|| self.incident_ticket.trim().to_owned());
        context.endpoint_label =
            (!self.endpoint_label.trim().is_empty()).then(|| self.endpoint_label.trim().to_owned());
        context.attribution = self.attribution.clone();
        let limits = if self.session_policy.helpdesk {
            diskscope::engine::ScanLimits::helpdesk()
        } else {
            diskscope::engine::ScanLimits::default()
        };
        context.resource_observation = self.controller.resource_observation();
        context.resource_refusal_reason = self.controller.resource_refusal_reason();
        context.admission_limits = Some(limits);
        context.memory_budget = Some(limits.memory_budget());
        context.view_request = Some(self.request.clone());
        context.redaction = if self.export_redacted {
            Redaction::Redacted
        } else {
            Redaction::Full
        };
        if self.action_journal.len() > 0 || self.action_review.is_some() {
            context.action = Some(
                json!({"journal":self.action_journal.export(),"current_review":self.action_review.as_ref().map(|r|json!({"sequence":r.sequence,"verification":r.verification(),"before_logical_bytes":r.before_logical,"after_logical_bytes":r.after_logical,"logical_change_bytes":r.logical_change().map(|n|n.to_string()),"observed_available_space_change_bytes":r.volume_change().map(|n|n.to_string()),"reconciliation_state":r.state.map(|s|s.label())})),"note":"Measurements are independent. Available-space change is not attributed reclaimed space. Explicit export is required to retain this session journal after closing."}),
            );
        }
        context
    }

    pub(super) fn evidence_controls(&mut self, ui: &mut egui::Ui) {
        let response = ui.add_enabled(
            !self.management.busy(),
            egui::Button::new("Incident & evidence…"),
        );
        if response.clicked() {
            controls::set_dialog_invoker(ui.ctx(), "Export investigation evidence", response.id);
            self.export_open = true;
            self.export_feedback = None;
        }
    }

    pub(super) fn export_window(&mut self, ctx: &egui::Context) {
        if !self.export_open {
            return;
        }
        controls::remember_dialog_focus(ctx, "Export investigation evidence");
        let mut open = true;
        controls::show_dialog(
            egui::Window::new("Export investigation evidence")
                .open(&mut open)
                .collapsible(false)
                .default_width(560.0),
            ctx,
            |ui| {
                ui.strong("Incident context · session only");
                let label = ui.label("Ticket / incident reference (optional)");
                ui.add(
                    egui::TextEdit::singleline(&mut self.incident_ticket)
                        .char_limit(512)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
                let label = ui.label("Endpoint label for this ticket (optional)");
                ui.add(
                    egui::TextEdit::singleline(&mut self.endpoint_label)
                        .char_limit(512)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
                ui.checkbox(
                    &mut self.export_redacted,
                    "Redact paths, incident labels and action details",
                );
                ui.label(if self.export_redacted {
                    "Paths, native identifiers, labels and error/action text are omitted."
                } else {
                    "Full report includes customer paths, identifiers and action details. Review before sharing."
                });
                let label = ui.label("Destination JSON file (new file only)");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.export_path)
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label.id)
                    .changed()
                {
                    self.export_native_path = None;
                    self.export_feedback = None;
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(!self.export_busy, egui::Button::new("Choose destination…"))
                        .clicked()
                    {
                        self.export_busy = true;
                        self.export_feedback = Some("Choosing destination…".into());
                        let tx = self.native_tx.clone();
                        let context = ctx.clone();
                        if let Err(error) = std::thread::Builder::new()
                            .name("evidence-destination".into())
                            .spawn(move || {
                                let path = rfd::FileDialog::new()
                                    .set_file_name("DiskScope-evidence.json")
                                    .add_filter("JSON report", &["json"])
                                    .save_file();
                                let _ = tx.send(NativeResult::ExportDestination(path));
                                context.request_repaint();
                            })
                        {
                            self.export_busy = false;
                            let message = format!("Could not open destination picker: {error}");
                            self.export_feedback = Some(message.clone());
                            self.notice = Some(message);
                        }
                    }
                    if ui
                        .add_enabled(
                            self.generation > 0 && !self.export_path.trim().is_empty(),
                            egui::Button::new("Save evidence"),
                        )
                        .clicked()
                        && !self.export_busy
                    {
                        self.export_busy = true;
                        self.export_feedback = Some("Saving evidence…".into());
                        let path = self
                            .export_native_path
                            .clone()
                            .unwrap_or_else(|| PathBuf::from(&self.export_path));
                        let snapshot = self.evidence_snapshot();
                        let evidence_context = self.evidence_context();
                        let tx = self.native_tx.clone();
                        let context = ctx.clone();
                        if let Err(error) = std::thread::Builder::new()
                            .name("evidence-export".into())
                            .spawn(move || {
                                let report = evidence::build_report(&snapshot, &evidence_context);
                                let result = evidence::write_report_new(&report, &path)
                                    .map(|()| path)
                                    .map_err(|e| e.to_string());
                                let _ = tx.send(NativeResult::Export(result));
                                context.request_repaint();
                            })
                        {
                            self.export_busy = false;
                            let message = format!("Could not start evidence export: {error}");
                            self.export_feedback = Some(message.clone());
                            self.notice = Some(message);
                        }
                    }
                });
                if let Some(message) = &self.export_feedback {
                    ui.push_id("export_feedback", |ui| {
                        let response = ui.add(egui::Label::new(message).wrap());
                        ui.ctx().accesskit_node_builder(response.id, |node| {
                            node.set_role(egui::accesskit::Role::Status);
                            node.set_live(egui::accesskit::Live::Polite);
                            node.set_live_atomic();
                            node.set_value(message.as_str());
                        });
                        let changed = ui.data_mut(|data| {
                            let previous = data.get_temp::<String>(response.id);
                            data.insert_temp(response.id, message.clone());
                            previous.as_ref() != Some(message)
                        });
                        if changed {
                            response
                                .scroll_to_me_animation(None, egui::style::ScrollAnimation::none());
                        }
                    });
                }
                if self.generation == 0 {
                    ui.label("Scan a folder before saving evidence.");
                }
                ui.small(format!(
                    "{} action requests retained · bounded visible rows · new files only",
                    self.action_journal.len()
                ));
                ui.collapsing("Report contents & limits", |ui| {
                    ui.label("Save a bounded JSON summary for the correct incident ticket, using an approved ticket location. Existing files are never overwritten.");
                    ui.label("It includes scan coverage, errors, visible rows and any current action review. It is not a full inventory or restorable comparison baseline.");
                    ui.label("Displayed rows reflect your current view, filters and page. Export from Largest files for a size-focused list.");
                    ui.label("Native hostname and volume metadata are observed in the background. Full reports include available identifiers; redacted reports omit them and both labels. Verify the endpoint against your ticket; identifiers can change or be shared by clones.");
                    ui.label("Requests, cancellation and completed calls are distinct. Export before closing to retain the session journal. Oldest records and long displays are bounded with explicit truncation.");
                    ui.label("Redacted reports retain counts and measurements. Native identities are observations, not proof of an endpoint's ownership. Associate the report with the correct ticket.");
                    ui.label("Full reports contain customer filenames, paths, identifiers and action details. Review before sharing. Incident labels remain in this session only; explicit exports retain the selected report content.");
                });
            },
        );
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

    fn export_test_app() -> (DiskScopeApp, egui::Context) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = DiskScopeApp::new_session(
            &cc,
            None,
            AccessMode::Standard,
            SessionPolicy {
                helpdesk: true,
                no_history: true,
            },
        );
        app.export_open = true;
        app.generation = 1;
        app.export_path = "C:\\new-evidence.json".into();
        (app, ctx)
    }

    fn export_app_frame(
        app: &mut DiskScopeApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        test_support::physical_app_frame(app, ctx, egui::vec2(1440.0, 820.0), 1.0, events)
    }

    #[test]
    fn export_native_results_are_polite_enabled_status_inside_the_dialog() {
        for result in [
            Ok(PathBuf::from("C:\\ticket-evidence.json")),
            Err("Destination already exists; it was not overwritten.".into()),
        ] {
            let (mut app, ctx) = export_test_app();
            for _ in 0..3 {
                export_app_frame(&mut app, &ctx, vec![]);
            }
            let expected = if result.is_ok() {
                "Evidence exported to"
            } else {
                "Evidence export failed:"
            };
            app.native_tx.send(NativeResult::Export(result)).unwrap();
            app.logic(&ctx, &mut eframe::Frame::_new_kittest());
            let output = export_app_frame(&mut app, &ctx, vec![]);
            let update = output.platform_output.accesskit_update.unwrap();
            let (_, status) = update.nodes.iter().find(|(_, node)| node.role() == egui::accesskit::Role::Status
                && node.value().is_some_and(|value| value.contains(expected)))
                .expect("export result must appear in its dialog's status instead of only the disabled background");
            assert!(!status.is_disabled());
            assert_eq!(status.live(), Some(egui::accesskit::Live::Polite));
            assert!(status.is_live_atomic());
            assert!(app.export_open && !app.export_busy);
            assert!(app.notice.as_ref().unwrap().contains(expected));
        }
    }

    #[test]
    fn busy_export_keeps_save_focus_without_starting_a_duplicate_worker() {
        let (mut app, ctx) = export_test_app();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("must-not-be-created.json");
        app.export_path = path.to_string_lossy().into_owned();
        for _ in 0..3 {
            export_app_frame(&mut app, &ctx, vec![]);
        }
        let mut save_focus = None;
        for _ in 0..30 {
            let output = export_app_frame(
                &mut app,
                &ctx,
                vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            if let Some(focused) = ctx.memory(|m| m.focused())
                && output
                    .platform_output
                    .accesskit_update
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|(id, node)| {
                        *id == focused.accesskit_id() && node.label() == Some("Save evidence")
                    })
            {
                save_focus = Some(focused);
                break;
            }
        }
        let save_focus = save_focus.expect("Save is keyboard reachable");
        app.export_busy = true;
        let output = export_app_frame(&mut app, &ctx, vec![]);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(save_focus),
            "starting an async export must not send focus back to Ticket"
        );
        let update = output.platform_output.accesskit_update.unwrap();
        assert!(
            !update
                .nodes
                .iter()
                .find(|(id, _)| *id == save_focus.accesskit_id())
                .unwrap()
                .1
                .is_disabled()
        );
        for _ in 0..3 {
            export_app_frame(
                &mut app,
                &ctx,
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        let deadline = Instant::now() + Duration::from_millis(100);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match app.native_rx.recv_timeout(remaining) {
                Ok(NativeResult::Export(_)) => {
                    panic!("Enter while busy must not enqueue a duplicate export result")
                }
                Ok(_) => {} // Independent location/attribution workers may finish.
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(error) => panic!("native channel unexpectedly closed: {error}"),
            }
        }
        assert!(!path.exists());
        app.native_tx
            .send(NativeResult::Export(Err("Existing destination.".into())))
            .unwrap();
        app.logic(&ctx, &mut eframe::Frame::_new_kittest());
        export_app_frame(&mut app, &ctx, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(save_focus));
    }

    #[test]
    fn export_result_is_visible_at_real_150_percent_clients_without_moving_save_focus() {
        for (physical_size, native_scale) in [
            (egui::vec2(900.0, 640.0), 1.0),
            (egui::vec2(900.0, 640.0), 1.25),
            (egui::vec2(1440.0, 820.0), 1.0),
        ] {
            let (mut app, ctx) = export_test_app();
            test_support::physical_app_frame(&mut app, &ctx, physical_size, native_scale, vec![]);
            app.preferences.text_scale = TextScale::Largest;
            app.preferences.reduced_motion = true;
            app.apply_appearance(&ctx);
            for _ in 0..4 {
                test_support::physical_app_frame(
                    &mut app,
                    &ctx,
                    physical_size,
                    native_scale,
                    vec![],
                );
            }
            let mut save_focus = None;
            for _ in 0..30 {
                let output = test_support::physical_app_frame(
                    &mut app,
                    &ctx,
                    physical_size,
                    native_scale,
                    vec![egui::Event::Key {
                        key: egui::Key::Tab,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
                if let Some(focused) = ctx.memory(|m| m.focused())
                    && output
                        .platform_output
                        .accesskit_update
                        .unwrap()
                        .nodes
                        .iter()
                        .any(|(id, node)| {
                            *id == focused.accesskit_id() && node.label() == Some("Save evidence")
                        })
                {
                    save_focus = Some(focused);
                    break;
                }
            }
            let save_focus = save_focus.expect("Save is reachable in the scaled client");
            app.export_busy = true;
            app.native_tx
                .send(NativeResult::Export(Err(
                    "Destination already exists; it was not overwritten.".into(),
                )))
                .unwrap();
            app.logic(&ctx, &mut eframe::Frame::_new_kittest());
            let mut output = egui::FullOutput::default();
            for _ in 0..4 {
                output = test_support::physical_app_frame(
                    &mut app,
                    &ctx,
                    physical_size,
                    native_scale,
                    vec![],
                );
            }
            assert!((output.pixels_per_point - native_scale * 1.5).abs() < 0.01);
            painted_text_rect(
                &output,
                app.export_feedback.as_deref().unwrap(),
                ctx.content_rect(),
            );
            assert_eq!(ctx.memory(|m| m.focused()), Some(save_focus));
        }
    }

    #[test]
    fn editing_the_export_destination_clears_the_previous_result() {
        let (mut app, ctx) = export_test_app();
        app.native_tx
            .send(NativeResult::Export(Err("Old destination error".into())))
            .unwrap();
        app.logic(&ctx, &mut eframe::Frame::_new_kittest());
        for _ in 0..3 {
            export_app_frame(&mut app, &ctx, vec![]);
        }
        let mut reached = false;
        for _ in 0..30 {
            let output = export_app_frame(
                &mut app,
                &ctx,
                vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            if let Some(focused) = ctx.memory(|m| m.focused())
                && output
                    .platform_output
                    .accesskit_update
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|(id, node)| {
                        *id == focused.accesskit_id()
                            && node.value() == Some(app.export_path.as_str())
                    })
            {
                reached = true;
                break;
            }
        }
        assert!(reached);
        export_app_frame(&mut app, &ctx, vec![egui::Event::Text("-edited".into())]);
        assert_eq!(app.export_path, "C:\\new-evidence.json-edited");
        assert!(
            app.export_feedback.is_none(),
            "a previous result must not describe the newly edited destination"
        );
    }

    fn painted_text_rect(output: &egui::FullOutput, expected: &str, viewport: Rect) -> Rect {
        fn find(shape: &egui::epaint::Shape, expected: &str) -> Option<Rect> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == expected => {
                    Some(text.visual_bounding_rect())
                }
                egui::epaint::Shape::Vec(shapes) => {
                    shapes.iter().find_map(|shape| find(shape, expected))
                }
                _ => None,
            }
        }
        let (rect, clip) = output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, expected).map(|rect| (rect, shape.clip_rect)))
            .unwrap_or_else(|| panic!("Missing painted control text: {expected}"));
        assert!(
            viewport.contains_rect(rect),
            "{expected} outside viewport: {rect:?}"
        );
        assert!(
            clip.contains_rect(rect),
            "{expected} clipped: {rect:?} vs {clip:?}"
        );
        rect
    }

    #[test]
    fn export_primary_fields_fit_a_900_by_640_point_viewport() {
        // This is a logical-point layout test. Smaller physical clients with
        // native scaling and 150% zoom are covered by the scroll/focus tests.
        for scale in [TextScale::Normal, TextScale::Largest] {
            for redacted in [true, false] {
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
                app.preferences.text_scale = scale;
                app.apply_appearance(&cc.egui_ctx);
                app.export_open = true;
                app.generation = 1;
                app.export_redacted = redacted;
                app.incident_ticket = "ticket-field-value".into();
                app.endpoint_label = "endpoint-field-value".into();
                app.export_path = "C:\\new-evidence.json".into();
                let viewport = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 640.0));
                let mut output = egui::FullOutput::default();
                for _ in 0..3 {
                    output = cc.egui_ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(viewport),
                            ..Default::default()
                        },
                        |ui| app.export_window(ui.ctx()),
                    );
                }
                for text in [
                    "Ticket / incident reference (optional)",
                    "ticket-field-value",
                    "Endpoint label for this ticket (optional)",
                    "endpoint-field-value",
                    "Redact paths, incident labels and action details",
                    "Destination JSON file (new file only)",
                    "C:\\new-evidence.json",
                    "Choose destination…",
                    "Save evidence",
                ] {
                    painted_text_rect(&output, text, viewport);
                }
                painted_text_rect(
                    &output,
                    if redacted {
                        "Paths, native identifiers, labels and error/action text are omitted."
                    } else {
                        "Full report includes customer paths, identifiers and action details. Review before sharing."
                    },
                    viewport,
                );
                let texts = test_support::rendered_texts(&output.shapes).join(" ");
                assert!(texts.contains("Report contents & limits"));
                assert!(!texts.contains("not a full inventory or restorable comparison baseline"));
            }
        }
    }

    #[test]
    fn escape_closes_export_and_restores_the_incident_control_focus() {
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
        let invoker = egui::Id::new("incident-control");
        controls::set_dialog_invoker(&cc.egui_ctx, "Export investigation evidence", invoker);
        app.export_open = true;
        test_support::settle_frames(&cc.egui_ctx, |ui| app.export_window(ui.ctx()));
        let _ = cc.egui_ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                app.keyboard(ui.ctx());
                app.export_window(ui.ctx());
            },
        );
        assert!(!app.export_open);
        assert_eq!(cc.egui_ctx.memory(|m| m.focused()), Some(invoker));
    }
    #[test]
    fn newest_attribution_request_is_retried_when_the_worker_queue_is_full() {
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
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.try_send((2, "old-pending".into())).unwrap();
        app.attribution_tx = tx;
        app.pending_attribution = Some((3, "new-current".into()));
        app.submit_attribution();
        assert_eq!(app.pending_attribution.as_ref().unwrap().0, 3);
        assert_eq!(rx.recv().unwrap().0, 2);
        app.submit_attribution();
        assert!(app.pending_attribution.is_none());
        assert_eq!(rx.recv().unwrap(), (3, PathBuf::from("new-current")));
    }
    #[test]
    fn incident_export_retains_journal_across_new_scan_and_redacts_identifiers() {
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
        app.incident_ticket = "private-ticket".into();
        app.endpoint_label = "private-endpoint".into();
        app.action_journal.request(
            1,
            1,
            "recycle",
            "private-root".into(),
            "private-file".into(),
            Some(10),
        );
        app.action_journal
            .finish(1, Some(11), Some(12), "recycled", "private-result".into());
        let directory = tempfile::tempdir().unwrap();
        app.begin_scan(directory.path().to_path_buf());
        assert_eq!(app.action_journal.len(), 1);
        let redacted = evidence::build_report(&app.evidence_snapshot(), &app.evidence_context());
        let encoded = redacted.to_string();
        for secret in [
            "private-ticket",
            "private-endpoint",
            "private-root",
            "private-file",
            "private-result",
        ] {
            assert!(!encoded.contains(secret), "{secret}");
        }
        assert_eq!(redacted["action_record_present"], true);
        app.export_redacted = false;
        let full = evidence::build_report(&app.evidence_snapshot(), &app.evidence_context());
        assert_eq!(
            full["action"]["journal"]["records"][0]["finished_utc_epoch_ms"],
            12
        );
        assert!(full.to_string().contains("private-ticket"));
    }
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
