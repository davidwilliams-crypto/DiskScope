use super::*;

impl DiskScopeApp {
    pub(super) fn focus_changed_item(&mut self, path: PathBuf) {
        // Both features steer the request's single focus path.
        self.cancel_restore();
        self.remember_context();
        focus_change_request(&mut self.request, path);
        self.changes = false;
        self.mark_query_changed();
    }
    pub(super) fn changes_page(&mut self, ui: &mut egui::Ui, p: Palette) {
        if !self.access_mode.persistence_allowed() {
            ui.add_space(18.0);
            ui.label(
                RichText::new("Scan history is unavailable in this window")
                    .size(22.0)
                    .strong(),
            );
            ui.label(RichText::new(self.access_mode.history_message()).color(p.muted));
            return;
        }
        let history = self.history_snapshot.clone();
        egui::ScrollArea::vertical()
            .id_salt("changes_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(
                        "Compare two complete scans of the same folder. See what grew, disappeared or moved.",
                    )
                    .color(p.muted),
                );
                ui.add_space(12.0);
                if self.older_scan.is_none()
                    && let Some((older, newer)) =
                        previous_pair(&history.scans, &self.snapshot.progress.root)
                {
                    self.older_scan = Some(older);
                    self.newer_scan = Some(newer);
                }
                self.comparison_picker(ui, &history, p);
                if !history.message.is_empty() {
                    ui.label(RichText::new(&history.message).color(p.muted));
                }
                let comparison = history.comparison.as_ref().filter(|c| {
                    comparison_matches(self.older_scan, self.newer_scan, (c.older_id, c.newer_id))
                });
                if let Some(comparison) = comparison {
                    self.comparison_results(ui, comparison, p);
                } else {
                    ui.add_space(30.0);
                    ui.label(
                        RichText::new(if history.scans.len() < 2 {
                            "No earlier complete scan of this folder yet."
                        } else {
                            "Choose your scans, then compare."
                        })
                        .size(24.0),
                    );
                    ui.label(
                        RichText::new(
                            "Finish a scan, make changes to your files, then scan the same folder again. Only complete scans without errors become baselines.",
                        )
                        .color(p.muted),
                    );
                }
                ui.add_space(24.0);
                self.history_storage_section(ui, p);
            });
    }
    fn comparison_picker(&mut self, ui: &mut egui::Ui, history: &HistorySnapshot, p: Palette) {
        ux::card(p).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                scan_picker(ui, "Before", &mut self.older_scan, &history.scans);
                scan_picker(ui, "After", &mut self.newer_scan, &history.scans);
                let pair = self
                    .older_scan
                    .zip(self.newer_scan)
                    .filter(|(older, newer)| older != newer);
                let compare = ui.add_enabled(
                    !history.busy && pair.is_some(),
                    egui::Button::new("Compare scans →"),
                );
                if compare.clicked()
                    && let Some((older, newer)) = pair
                    && !self.history_service.compare(older, newer)
                {
                    self.notice =
                        Some("History worker is busy. Try Compare again when it finishes.".into());
                }
                if history.busy {
                    ui.label(RichText::new("Working in background…").color(p.accent));
                }
            });
        });
    }
    fn comparison_results(
        &mut self,
        ui: &mut egui::Ui,
        comparison: &scan_history::Comparison,
        p: Palette,
    ) {
        ui.add_space(16.0);
        ui.label(RichText::new(comparison.root.to_string_lossy()).color(p.muted));
        ui.horizontal_wrapped(|ui| {
            metric(ui, &format_bytes(comparison.before), "BEFORE", p, 170.0);
            metric(ui, &format_bytes(comparison.after), "AFTER", p, 170.0);
            metric(
                ui,
                &delta_label(comparison.delta),
                "NET LOGICAL CHANGE",
                p,
                210.0,
            );
        });
        ui.add_space(14.0);
        ui.horizontal_wrapped(|ui| {
            for (count, label) in [
                (comparison.added, "added"),
                (comparison.removed, "removed"),
                (comparison.modified, "modified"),
                (comparison.moved, "moved"),
            ] {
                ui.label(RichText::new(format!("{} {label}", format_count(count))).color(p.accent));
                ui.add_space(12.0);
            }
        });
        ui.add_space(16.0);
        ux::card(p).show(ui, |ui| {
            ui.label(RichText::new("Folder changes").size(17.0).strong());
            for folder in &comparison.folder_changes {
                ui.horizontal_wrapped(|ui| {
                    let relative = folder
                        .path
                        .strip_prefix(&comparison.root)
                        .unwrap_or(&folder.path);
                    ui.label(if relative.as_os_str().is_empty() {
                        std::borrow::Cow::Borrowed("Entire scanned folder")
                    } else {
                        relative.to_string_lossy()
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(delta_label(folder.delta)).color(p.accent));
                    });
                });
            }
        });
        ui.add_space(16.0);
        ui.label(RichText::new("Changed items").size(17.0).strong());
        ui.label(
            RichText::new(format!(
                "Showing {} of {} changes · path and size metadata; no content hashes",
                format_count(comparison.rows.len() as u64),
                format_count(comparison.total_changes)
            ))
            .small()
            .color(p.muted),
        );
        let live = comparison.newer_generation == Some(self.generation)
            && comparison.root == self.snapshot.progress.root;
        for (index, row) in comparison.rows.iter().enumerate() {
            ui.push_id(index, |ui| {
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(if index % 2 == 0 { p.side } else { p.surface })
                    .inner_margin(9.0)
                    .corner_radius(4)
                    .show(ui, |ui| self.change_row(ui, comparison, row, live, p));
            });
        }
        ui.add_space(10.0);
        ui.label(
            RichText::new(
                "Moves require an unambiguous file identity. Unknown identities and hardlink aliases can limit detection. Removed paths no longer exist in Explorer.",
            )
            .small()
            .color(p.muted),
        );
    }
    fn change_row(
        &mut self,
        ui: &mut egui::Ui,
        comparison: &scan_history::Comparison,
        row: &scan_history::ChangeRow,
        live: bool,
        p: Palette,
    ) {
        let removed = row.kind == scan_history::ChangeKind::Removed;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(change_kind_label(row.kind)).color(p.accent));
            ui.add(
                egui::Label::new(
                    row.path
                        .strip_prefix(&comparison.root)
                        .unwrap_or(&row.path)
                        .to_string_lossy(),
                )
                .truncate(),
            )
            .on_hover_text(row.path.to_string_lossy());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let reveal = ui
                    .add_enabled(!removed, egui::Button::new("Reveal").small())
                    .on_disabled_hover_text("This path no longer exists.");
                if reveal.clicked() {
                    self.reveal(row.path.clone(), false, ui.ctx());
                }
                if live && !removed && ui.small_button("Show in map").clicked() {
                    self.focus_changed_item(row.path.clone());
                }
                ui.label(delta_label(row.delta));
            });
        });
        ui.label(
            RichText::new(format!(
                "{} → {} logical",
                format_bytes(row.before),
                format_bytes(row.after)
            ))
            .small()
            .color(p.muted),
        );
        if let Some(previous) = row.previous_path.as_ref() {
            ui.label(
                RichText::new(format!("From {}", previous.to_string_lossy()))
                    .small()
                    .color(p.muted),
            );
        }
    }
    fn history_storage_section(&mut self, ui: &mut egui::Ui, p: Palette) {
        ui.collapsing("Local history & storage", |ui| {
            ui.label(format!(
                "{} most recent complete scans · metadata only · stored at the location below",
                scan_history::MAX_SNAPSHOTS
            ));
            ui.label(self.history_directory.to_string_lossy());
            ui.label(
                RichText::new(format!(
                    "Up to {} and {} items per snapshot; {} disk budget including staging. Oversized scans are skipped with an explanation.",
                    format_bytes(scan_history::MAX_SNAPSHOT_BYTES as u64),
                    format_count(scan_history::MAX_NODES as u64),
                    format_bytes(scan_history::MAX_DISK_BYTES)
                ))
                .small()
                .color(p.muted),
            );
            if ui.button("Clear saved scan history").clicked() {
                if self.history_service.forget_all() {
                    self.older_scan = None;
                    self.newer_scan = None;
                } else {
                    self.notice = Some("History is busy. Try clearing again when it finishes.".into());
                }
            }
        });
    }
}
fn change_kind_label(kind: scan_history::ChangeKind) -> &'static str {
    match kind {
        scan_history::ChangeKind::Added => "Added",
        scan_history::ChangeKind::Removed => "Removed",
        scan_history::ChangeKind::Modified => "Modified",
        scan_history::ChangeKind::Moved => "Moved",
    }
}
fn delta_label(delta: i128) -> String {
    format!(
        "{}{}",
        if delta > 0 {
            "+"
        } else if delta < 0 {
            "−"
        } else {
            ""
        },
        format_bytes(delta.unsigned_abs().min(u64::MAX as u128) as u64)
    )
}
fn scan_picker(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut Option<u64>,
    scans: &[scan_history::ScanSummary],
) {
    ui.label(label);
    let text = scans
        .iter()
        .find(|s| Some(s.id) == *selected)
        .map(|s| {
            format!(
                "{} · {} · {}",
                date_time_label(s.finished_unix),
                s.root
                    .file_name()
                    .unwrap_or(s.root.as_os_str())
                    .to_string_lossy(),
                format_bytes(s.logical)
            )
        })
        .unwrap_or_else(|| "Select a complete scan".into());
    egui::ComboBox::from_id_salt(label)
        .selected_text(text)
        .width(270.0)
        .show_ui(ui, |ui| {
            for scan in scans {
                ui.selectable_value(
                    selected,
                    Some(scan.id),
                    format!(
                        "{} · {} · {}",
                        date_time_label(scan.finished_unix),
                        scan.root.to_string_lossy(),
                        format_bytes(scan.logical)
                    ),
                );
            }
        });
}
