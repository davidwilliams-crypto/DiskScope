//! Shared scope, recovery and compact-workspace presentation.
use super::*;

pub(super) fn compact(ctx: &egui::Context) -> bool {
    ctx.content_rect().width() < 1100.0
}

impl DiskScopeApp {
    pub(super) fn requested_path(&self) -> PathBuf {
        if self.path_edited {
            self.draft_path
                .clone()
                .unwrap_or_else(|| PathBuf::from(&self.path_text))
        } else {
            self.root_path.clone()
        }
    }
    pub(super) fn compact_file_list(&mut self, ui: &mut egui::Ui, height: f32, p: Palette) {
        const SIZE_WIDTH: f32 = 105.0;
        // Reserve the same scrollbar gutter above and inside the list so the
        // numeric column never moves when scrolling becomes necessary.
        let name_width = (ui.available_width()
            - ui.spacing().scroll.allocated_width()
            - SIZE_WIDTH
            - ui.spacing().item_spacing.x)
            .max(100.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(name_width, 20.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_size(egui::vec2(name_width, 20.0));
                    ui.add(
                        egui::Label::new(RichText::new("NAME").small().color(p.faint))
                            .selectable(false),
                    );
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(SIZE_WIDTH, 20.0),
                Layout::right_to_left(Align::Center),
                |ui| {
                    ui.set_min_size(egui::vec2(SIZE_WIDTH, 20.0));
                    ui.add(
                        egui::Label::new(RichText::new("LOGICAL SIZE").small().color(p.faint))
                            .selectable(false),
                    );
                },
            );
        });
        let snapshot = self.visible.clone();
        let rows = &snapshot.view.rows;
        let mut action = None;
        let focus_key = ui.id().with("compact_focus_target");
        let target = ui.data_mut(|d| d.remove_temp::<NodeId>(focus_key));
        let mut list = egui::ScrollArea::vertical()
            .id_salt("compact_files")
            .max_height(height)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, false]);
        if let Some(index) = target.and_then(|target| rows.iter().position(|row| row.id == target))
        {
            list = list.vertical_scroll_offset(index as f32 * (38.0 + ui.spacing().item_spacing.y));
        }
        list.show_rows(ui, 38.0, rows.len(), |ui, range| {
            for index in range {
                let row = &rows[index];
                ui.horizontal(|ui| {
                    let response = ui.add_sized(
                        [name_width, 32.0],
                        egui::Button::selectable(
                            self.request.selected == Some(row.id),
                            (row.name.as_str(), egui::Atom::grow()),
                        )
                        .wrap_mode(egui::TextWrapMode::Truncate),
                    );
                    if target == Some(row.id) {
                        response.request_focus();
                        response.scroll_to_me(None);
                    }
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            true,
                            self.request.selected == Some(row.id),
                            format!(
                                "{} · {} · {}",
                                row.name,
                                if row.is_dir { "folder" } else { "file" },
                                format_bytes(row.logical)
                            ),
                        )
                    });
                    if response.clicked() {
                        response.request_focus();
                        action = Some(Action::Select(row.id));
                    }
                    if response.double_clicked() && row.is_dir {
                        action = Some(Action::Navigate(row.id));
                    }
                    if response.has_focus() {
                        let direction = response.ctx.input_mut(|i| {
                            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                                1
                            } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                                -1
                            } else {
                                0
                            }
                        });
                        if direction != 0 {
                            let next = (index as isize + direction)
                                .clamp(0, rows.len().saturating_sub(1) as isize)
                                as usize;
                            action = Some(Action::Select(rows[next].id));
                            ui.data_mut(|d| d.insert_temp(focus_key, rows[next].id));
                        }
                        if row.is_dir
                            && response.ctx.input_mut(|i| {
                                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                            })
                        {
                            action = Some(Action::Navigate(row.id));
                        }
                    }
                    ui.allocate_ui_with_layout(
                        egui::vec2(SIZE_WIDTH, 32.0),
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            ui.set_min_size(egui::vec2(SIZE_WIDTH, 32.0));
                            ui.add(egui::Label::new(format_bytes(row.logical)).selectable(false));
                        },
                    );
                    response.on_hover_text(row.path.to_string_lossy());
                });
            }
        });
        if rows.is_empty() {
            self.empty_results(ui, p);
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.visible_offset > 0, egui::Button::new("Previous"))
                .clicked()
            {
                action = Some(Action::Page(self.visible_offset.saturating_sub(PAGE_SIZE)));
            }
            ui.label(format!("{} items", snapshot.view.total_rows));
            if ui
                .add_enabled(
                    self.visible_offset + rows.len() < snapshot.view.total_rows,
                    egui::Button::new("Next"),
                )
                .clicked()
            {
                action = Some(Action::Page(self.visible_offset + PAGE_SIZE));
            }
        });
        if let Some(action) = action {
            self.action(action);
        }
    }

    pub(super) fn compact_header(&mut self, ui: &mut egui::Ui, p: Palette) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(if self.changes {
                    "Scan changes"
                } else if self.generation > 0 {
                    self.scan_state().label()
                } else {
                    "Storage investigation"
                })
                .strong(),
            );
            if self.generation > 0 {
                ui.label(format!(
                    "{} · {} files",
                    format_bytes(self.snapshot.progress.logical),
                    format_count(self.snapshot.progress.files)
                ));
                if self.snapshot.progress.errors_total > 0
                    && errors_button(ui, self.snapshot.progress.errors_total)
                {
                    self.show_errors = true;
                }
            }
            if ui.button("Commands").clicked() {
                self.open_palette();
            }
            ui.menu_button("Access", |ui| {
                self.access_controls(ui, p);
                if !self.access_mode.persistence_allowed() {
                    ui.label(self.access_mode.history_message());
                }
            });
            let export_was_open = self.export_open;
            let evidence_menu = ui.menu_button(
                if self.session_policy.helpdesk {
                    "Helpdesk & evidence"
                } else {
                    "Session & evidence"
                },
                |ui| {
                    ui.label(self.session_policy.label());
                    ui.label(self.persistence_message());
                    self.evidence_controls(ui);
                },
            );
            if !export_was_open && self.export_open {
                controls::set_dialog_invoker(
                    ui.ctx(),
                    "Export investigation evidence",
                    evidence_menu.response.id,
                );
            }
        });
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 240.0).max(80.0);
            let response = ui.add_sized(
                [width, 30.0],
                egui::TextEdit::singleline(&mut self.path_text)
                    .desired_width(width)
                    .hint_text("Folder or drive path"),
            );
            if response.changed() {
                self.path_edited = true;
                self.draft_path = None;
            }
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(!self.picker_open, egui::Button::new("Browse…"))
                .clicked()
            {
                self.choose_folder(ui.ctx());
            }
            let scan = ui
                .add_enabled(!self.path_text.trim().is_empty(), egui::Button::new("Scan"))
                .clicked();
            if scan || enter {
                let path = self.requested_path();
                if !path.as_os_str().is_empty() {
                    self.begin_scan(path);
                }
            }
            if ui
                .add_enabled(self.scan_state().is_active(), egui::Button::new("Stop"))
                .clicked()
            {
                self.cancellation
                    .request(self.generation, self.snapshot.progress.state);
                self.controller.cancel();
            }
        });
        self.notice_line(ui, p);
    }

    pub(super) fn empty_message(&self, map: bool) -> &'static str {
        if self.generation == 0 {
            return "Choose a location to investigate";
        }
        if !map && (self.request.filters.is_active() || !self.request.search.is_empty()) {
            if diskscope::search::SearchQuery::parse(&self.request.search).is_err() {
                return "Correct the search to see matching files";
            }
            return "No matches in this scope";
        }
        match self.scan_state() {
            ScanState::Preparing
            | ScanState::Scanning
            | ScanState::Finalizing
            | ScanState::Cancelling => "Discovering files · results will appear here",
            ScanState::Cancelled => "Scan stopped · this location may not have been read",
            ScanState::Failed => "This location could not be scanned · inspect errors or retry",
            ScanState::Partial => {
                "Incomplete coverage · inspect errors before interpreting absence"
            }
            _ if map => "No drawable file sizes · empty or zero-byte items",
            _ => "This folder has no items in the current view",
        }
    }

    pub(super) fn empty_results(&mut self, ui: &mut egui::Ui, p: Palette) {
        ui.label(RichText::new(self.empty_message(false)).color(p.muted));
        if self.request.filters.is_active() || !self.request.search.is_empty() {
            if ui.button("Clear search and filters").clicked() {
                self.request.search.clear();
                self.request.filters = QueryFilter::default();
                self.mark_query_changed();
            }
        } else if matches!(
            self.scan_state(),
            ScanState::Failed | ScanState::Partial | ScanState::Cancelled
        ) {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Retry this location").clicked() {
                    self.begin_scan(self.root_path.clone());
                }
                if self.snapshot.progress.errors_total > 0
                    && ui.button("Inspect scan errors").clicked()
                {
                    self.show_errors = true;
                }
            });
        }
    }

    pub(super) fn workspace(&mut self, ui: &mut egui::Ui, p: Palette) {
        self.header(ui, p);
        if self.generation > 0 {
            if compact(ui.ctx()) {
                ui.horizontal(|ui| {
                    ui.label("Scanned location");
                    ui.menu_button("View path and coverage", |ui| {
                        ui.set_max_width((ui.ctx().content_rect().width()-60.0).max(200.0));
                        ui.label(self.root_path.to_string_lossy());
                        ui.label(match self.scan_state() { ScanState::Complete=>"Complete scan of readable entries; skipped locations remain excluded.", _=>"Coverage is incomplete or still being discovered. Totals are provisional." });
                        if ui.button("Copy scanned path").clicked() {ui.ctx().copy_text(self.root_path.to_string_lossy().into_owned());}
                    });
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Scanned location").strong());
                    ui.label(self.root_path.to_string_lossy());
                    let label = match self.scan_state() {
                        ScanState::Complete => "Complete coverage of readable entries",
                        ScanState::Partial | ScanState::Failed => {
                            "Coverage incomplete · totals have limits"
                        }
                        ScanState::Cancelled | ScanState::Cancelling => {
                            "Stopped / stopping · partial coverage"
                        }
                        _ => "Discovery in progress · totals are provisional",
                    };
                    ui.label(RichText::new(label).small().color(p.accent));
                });
            }
            if self.path_edited && std::path::Path::new(&self.path_text) != self.root_path {
                ui.label(RichText::new("The path above is a draft. These results belong to the scanned location until you select Scan.").small().color(p.muted));
            }
            let detail = status_detail(
                &self.snapshot.progress,
                self.scan_state(),
                self.restore.as_ref(),
            );
            if !detail.is_empty() {
                ui.collapsing("Full scan details", |ui| {
                    ui.add(egui::Label::new(&detail).wrap());
                    if ui.button("Copy scan details").clicked() {
                        ui.ctx().copy_text(detail.clone());
                    }
                });
            }
        }
        self.action_review_card(ui, p);
        if self.changes {
            self.changes_page(ui, p);
        } else if self.request.mode == ViewMode::Overview {
            self.overview(ui, p);
        } else {
            self.explore_page(ui, p);
        }
    }

    pub(super) fn compact_navigation(&mut self, ui: &mut egui::Ui, p: Palette) {
        egui::Panel::top("compact_navigation")
            .frame(egui::Frame::new().fill(p.side).inner_margin(8.0))
            .show_inside(ui, |ui| {
                if self.management.busy() {
                    ui.disable();
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("DiskScope").strong().color(p.accent));
                    ui.menu_button(
                        if self.changes {
                            "Changes"
                        } else {
                            match self.request.mode {
                                ViewMode::Overview => "Overview",
                                ViewMode::Explorer => "Explorer",
                                ViewMode::Largest => "Largest files",
                                ViewMode::Types => "File types",
                            }
                        },
                        |ui| {
                            for (mode, label) in [
                                (ViewMode::Overview, "Overview"),
                                (ViewMode::Explorer, "Explorer"),
                                (ViewMode::Largest, "Largest files"),
                                (ViewMode::Types, "File types"),
                            ] {
                                if ui
                                    .selectable_label(
                                        !self.changes && self.request.mode == mode,
                                        label,
                                    )
                                    .clicked()
                                {
                                    self.destination(mode);
                                    ui.close();
                                }
                            }
                            if ui.selectable_label(self.changes, "Changes").clicked() {
                                self.show_changes();
                                ui.close();
                            }
                        },
                    );
                    if ui.button("Settings").clicked() {
                        self.settings_open = true;
                    }
                    if ui.button("Help").clicked() {
                        self.help_open = true;
                    }
                });
            });
    }
}
