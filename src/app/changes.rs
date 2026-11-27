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
    /// One message, one place, for every history request the worker turned
    /// away: a click must never do nothing visible.
    pub(super) fn history_busy_toast(&mut self) {
        self.toasts.push(
            toasts::ToastKind::Warning,
            "Scan history is busy. Try again when it finishes.",
        );
    }
    pub(super) fn changes_page(&mut self, ui: &mut egui::Ui, p: Palette) {
        if !self.persistence_allowed() {
            ui.add_space(18.0);
            ui.label(title(
                ui,
                "Scan history is unavailable in this window",
                TYPE_H2,
            ));
            ui.label(RichText::new(self.persistence_message()).color(p.muted));
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
                self.comparison_picker(ui, &history, p);
                self.saved_scans_card(ui, &history, p);
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
                    ui.label(title(
                        ui,
                        if history.busy {
                            "Working on scan history…"
                        } else if history.scans.is_empty() {
                            "No saved complete scans yet."
                        } else {
                            "Choose compatible Before and After scans, then compare."
                        },
                        TYPE_H2,
                    ));
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
    /// The picker root is the last manually chosen one, but Navigation and
    /// Overview can explicitly choose a pair for another root. Those IDs are
    /// authoritative over the cached picker root. A manual root change in the
    /// picker still clears the pair in the same frame.
    fn history_root(&self, history: &HistorySnapshot, ctx: &egui::Context) -> PathBuf {
        let root_key = egui::Id::new("history_selected_root");
        let mut root = ctx
            .data_mut(|d| d.get_temp::<PathBuf>(root_key))
            .unwrap_or_else(|| self.snapshot.progress.root.clone());
        if let Some(selected) = history.scans.iter().find(|s| Some(s.id) == self.older_scan)
            && state::comparison_pair(
                &history.scans,
                &selected.root,
                self.older_scan,
                self.newer_scan,
            )
            .is_ok()
        {
            root = selected.root.clone();
        }
        if !history.scans.iter().any(|s| s.root == root)
            && let Some(scan) = history.scans.first()
        {
            root = scan.root.clone();
        }
        root
    }
    fn comparison_picker(&mut self, ui: &mut egui::Ui, history: &HistorySnapshot, p: Palette) {
        let root_key = egui::Id::new("history_selected_root");
        let mut root = self.history_root(history, ui.ctx());
        ux::card(p).show(ui, |ui| {
            ui.label(RichText::new("Compare two scans").strong());
            let previous_root = root.clone();
            egui::ComboBox::from_label("Scanned root")
                .selected_text(root.to_string_lossy())
                .width(ui.available_width().min(620.0))
                .show_ui(ui, |ui| {
                    let mut roots: Vec<_> = history.scans.iter().map(|s| &s.root).collect();
                    roots.sort();
                    roots.dedup();
                    for candidate in roots {
                        ui.selectable_value(
                            &mut root,
                            candidate.clone(),
                            candidate.to_string_lossy(),
                        );
                    }
                });
            ui.label(root.to_string_lossy());
            ui.ctx().data_mut(|d| d.insert_temp(root_key, root.clone()));
            if previous_root != root
                || [self.older_scan, self.newer_scan]
                    .into_iter()
                    .flatten()
                    .any(|id| !history.scans.iter().any(|s| s.id == id && s.root == root))
            {
                self.older_scan = None;
                self.newer_scan = None;
            }
            if self.older_scan.is_none() && self.newer_scan.is_none() {
                if let Some((older, newer)) = previous_pair(&history.scans, &root) {
                    self.older_scan = Some(older);
                    self.newer_scan = Some(newer);
                } else {
                    self.newer_scan = history.scans.iter().find(|s| s.root == root).map(|s| s.id);
                }
            }
            let before_after = self.newer_scan;
            ui.horizontal_wrapped(|ui| {
                scan_picker(
                    ui,
                    "After",
                    &mut self.newer_scan,
                    &history.scans,
                    &root,
                    None,
                );
                if before_after != self.newer_scan
                    && state::comparison_pair(
                        &history.scans,
                        &root,
                        self.older_scan,
                        self.newer_scan,
                    )
                    .is_err()
                {
                    self.older_scan = None;
                }
                scan_picker(
                    ui,
                    "Before",
                    &mut self.older_scan,
                    &history.scans,
                    &root,
                    self.newer_scan,
                );
                let pair =
                    state::comparison_pair(&history.scans, &root, self.older_scan, self.newer_scan);
                let compare = ui.add_enabled(
                    !history.busy && pair.is_ok(),
                    egui::Button::new("Compare scans →"),
                );
                if compare.clicked()
                    && let Ok((older, newer)) = pair
                    && !self.history_service.compare(older, newer)
                {
                    self.notice =
                        Some("History worker is busy. Try Compare again when it finishes.".into());
                }
                if history.busy {
                    ui.label(RichText::new("Working in background…").color(p.muted));
                }
                if let Err(reason) = pair {
                    ui.label(RichText::new(reason).color(p.muted));
                }
            });
            if let Ok((older, newer)) =
                state::comparison_pair(&history.scans, &root, self.older_scan, self.newer_scan)
            {
                let a = history.scans.iter().find(|s| s.id == older).unwrap();
                let b = history.scans.iter().find(|s| s.id == newer).unwrap();
                let line = if a.finished_unix > b.finished_unix {
                    "After is earlier than Before · change is After minus Before in logical bytes"
                } else {
                    "Compatible backend family · change is After minus Before in logical bytes"
                };
                ui.label(RichText::new(line).small().color(p.muted))
                    .on_hover_text(
                        "Only complete, error-free scans within storage limits become baselines. \
                     Cancelled, partial, failed and oversized scans are excluded. Normal and raw \
                     NTFS preview scans are separate families. View filters do not change saved \
                     baselines.",
                    );
            }
        });
    }
    fn saved_scans_card(&mut self, ui: &mut egui::Ui, history: &HistorySnapshot, p: Palette) {
        let root = self.history_root(history, ui.ctx());
        if !history.scans.iter().any(|s| s.root == root) && history.protected_baseline.is_none() {
            return;
        }
        ui.add_space(12.0);
        ux::card(p).show(ui, |ui| {
            ui.label(RichText::new("Saved scans of this root").strong());
            for scan in history.scans.iter().filter(|s| s.root == root) {
                let role = match (
                    Some(scan.id) == self.older_scan,
                    Some(scan.id) == self.newer_scan,
                ) {
                    (true, _) => "Before",
                    (_, true) => "After",
                    _ => "Saved",
                };
                let protected = if history.protected_baseline == Some(scan.id) {
                    " · protected"
                } else {
                    ""
                };
                ui.label(
                    RichText::new(format!(
                        "{role} · {} · {} · {}{protected}",
                        date_time_label(scan.finished_unix),
                        format_bytes(scan.logical),
                        scan.backend,
                    ))
                    .small(),
                );
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(id) = history.protected_baseline {
                    ui.label(
                        RichText::new(format!("Protected incident baseline: {id}"))
                            .small()
                            .color(p.muted),
                    );
                    if ui
                        .add_enabled(!history.busy, egui::Button::new("Release protection"))
                        .clicked()
                        && !self.history_service.unprotect_baseline()
                    {
                        self.history_busy_toast();
                    }
                } else if ui
                    .add_enabled(
                        !history.busy && self.older_scan.is_some(),
                        egui::Button::new("Protect Before baseline"),
                    )
                    .clicked()
                    && let Some(id) = self.older_scan
                    && !self.history_service.protect_baseline(id)
                {
                    self.history_busy_toast();
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
            metric(ui, &format_bytes(comparison.before), "Before", p, 170.0);
            metric(ui, &format_bytes(comparison.after), "After", p, 170.0);
            metric(
                ui,
                &delta_label(comparison.delta),
                "Net logical change",
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
                ui.label(RichText::new(format!("{} {label}", format_count(count))).strong());
                ui.add_space(12.0);
            }
        });
        ui.add_space(16.0);
        ux::card(p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(title(ui, "Folder changes", TYPE_H3));
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
                        ui.label(RichText::new(delta_label(folder.delta)).strong());
                    });
                });
            }
        });
        ui.add_space(16.0);
        ui.label(title(ui, "Changed items", TYPE_H3));
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
                // Meridian list rows: one quiet ground for every row, no
                // zebra striping.
                egui::Frame::new()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0_f32, p.card_outline))
                    .inner_margin(SPACE_3)
                    .corner_radius(RADIUS_MD)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        self.change_row(ui, comparison, row, live, p)
                    });
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
        ui.horizontal(|ui| {
            ui.label(RichText::new(change_kind_label(row.kind)).color(p.accent_text));
            // The trailing actions are laid out first; the path truncates
            // into whatever is left instead of running underneath them.
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
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
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
                });
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
        // Catalog load can fail before a protected id is known. Recovery must
        // stay reachable then, without making Clear the only available route,
        // so this repair row stays outside the collapsed maintenance header.
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.history_snapshot.busy,
                    egui::Button::new("Release or repair baseline protection"),
                )
                .clicked()
                && !self.history_service.unprotect_baseline() {
                    self.history_busy_toast();
                }
            ui.label(
                RichText::new(
                    "Repair releases protection and reloads history without clearing saved scans. Use it when protection metadata cannot be read.",
                )
                .small()
                .color(p.muted),
            );
        });
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!(
                "{} saved scans · at most {} kept · oldest removed automatically",
                self.history_snapshot.scans.len(),
                scan_history::MAX_SNAPSHOTS
            ))
            .small()
            .color(p.muted),
        );
        egui::CollapsingHeader::new("History storage & maintenance")
            .id_salt("history_storage_maintenance")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(format!(
                    "{} most recent complete scans across all roots · metadata only · stored at the location below",
                    scan_history::MAX_SNAPSHOTS
                ));
                ui.label(self.history_directory.to_string_lossy());
                ui.label("History is for focused folder investigations: at most 1,000,000 nodes per snapshot. Whole-drive scans may be too large to save. Export evidence before closing; export is a bounded summary, not a restorable full inventory. One protected baseline survives automatic retention; explicit Clear removes it.");
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
                ui.separator();
                // Clearing removes every saved scan, the protected baseline
                // included, and cannot be undone: it takes two deliberate
                // clicks, and the second one is never under the first.
                let armed_id = egui::Id::new("clear_saved_history_armed");
                let armed = ui.data(|d| d.get_temp::<bool>(armed_id).unwrap_or(false));
                let busy = self.history_snapshot.busy;
                if !armed {
                    if ui
                        .add_enabled(!busy, egui::Button::new("Clear saved scan history…"))
                        .clicked()
                    {
                        ui.data_mut(|d| d.insert_temp(armed_id, true));
                    }
                } else {
                    ui.label(format!(
                        "Clear all {} saved scans? This cannot be undone.",
                        self.history_snapshot.scans.len()
                    ));
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Keep saved scans").clicked() {
                            ui.data_mut(|d| d.insert_temp(armed_id, false));
                        }
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(
                                    RichText::new("Clear all saved scans").color(p.danger),
                                ),
                            )
                            .clicked()
                        {
                            ui.data_mut(|d| d.insert_temp(armed_id, false));
                            if self.history_service.forget_all() {
                                self.older_scan = None;
                                self.newer_scan = None;
                            } else {
                                self.notice = Some(
                                    "History is busy. Try clearing again when it finishes.".into(),
                                );
                            }
                        }
                    });
                }
                ui.label(
                    RichText::new(
                        "Clear removes every saved scan across all roots, including the protected baseline.",
                    )
                    .small()
                    .color(p.muted),
                );
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
    root: &std::path::Path,
    compatible_with: Option<u64>,
) {
    let text = scans
        .iter()
        .find(|s| Some(s.id) == *selected)
        .map(|s| {
            format!(
                "{} · {} · {}",
                date_time_label(s.finished_unix),
                s.root.to_string_lossy(),
                format_bytes(s.logical)
            )
        })
        .unwrap_or_else(|| "Select a complete scan".into());
    egui::ComboBox::from_label(label)
        .selected_text(text)
        .width(270.0)
        .show_ui(ui, |ui| {
            for scan in scans.iter().filter(|s| s.root == root) {
                let reason = compatible_with.and_then(|other| {
                    state::comparison_pair(scans, root, Some(scan.id), Some(other)).err()
                });
                ui.add_enabled_ui(reason.is_none(), |ui| {
                    ui.selectable_value(
                        selected,
                        Some(scan.id),
                        format!(
                            "{} · {} · {}",
                            date_time_label(scan.finished_unix),
                            scan.root.to_string_lossy(),
                            format_bytes(scan.logical)
                        ),
                    )
                    .on_disabled_hover_text(reason.unwrap_or(""));
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{rendered_texts, settle_frames};
    use super::*;

    fn click_changes_page(app: &mut DiskScopeApp, ctx: &egui::Context, position: egui::Pos2) {
        for pressed in [true, false] {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1440.0, 900.0),
                    )),
                    events: vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ui| app.changes_page(ui, app.preferences.theme.palette()),
            );
        }
    }

    #[test]
    fn explicit_comparison_pair_replaces_another_roots_cached_picker_state() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let directory = tempfile::tempdir().unwrap();
        let mut app = DiskScopeApp::with_history_directory(
            &cc,
            None,
            directory.path().to_path_buf(),
            AccessMode::Standard,
        );
        let summary = |id, root: &str| scan_history::ScanSummary {
            id,
            root: root.into(),
            finished_unix: id,
            logical: id,
            files: 1,
            folders: 0,
            backend: "Normal".into(),
            live_generation: None,
        };
        app.history_snapshot = Arc::new(HistorySnapshot {
            scans: vec![
                summary(4, "/b/data"),
                summary(3, "/b/data"),
                summary(2, "/a/data"),
                summary(1, "/a/data"),
            ],
            ..Default::default()
        });
        ctx.data_mut(|d| {
            d.insert_temp(
                egui::Id::new("history_selected_root"),
                PathBuf::from("/a/data"),
            )
        });
        app.older_scan = Some(1);
        app.newer_scan = Some(2);
        settle_frames(&ctx, |ui| {
            app.changes_page(ui, app.preferences.theme.palette())
        });
        // The Overview CTA assigns this pair after scanning a different root.
        app.older_scan = Some(3);
        app.newer_scan = Some(4);
        Arc::make_mut(&mut app.history_snapshot).comparison =
            Some(Arc::new(scan_history::Comparison {
                root: "/b/data".into(),
                older_id: 3,
                newer_id: 4,
                newer_generation: None,
                before: 3,
                after: 4,
                delta: 1,
                added: 1,
                removed: 0,
                modified: 0,
                moved: 0,
                total_changes: 1,
                rows: vec![],
                folder_changes: vec![],
            }));
        let output = settle_frames(&ctx, |ui| {
            app.changes_page(ui, app.preferences.theme.palette())
        });
        assert_eq!(
            (app.older_scan, app.newer_scan),
            (Some(3), Some(4)),
            "a cached root must not discard the explicitly requested pair"
        );
        assert_eq!(
            ctx.data_mut(|d| d.get_temp::<PathBuf>(egui::Id::new("history_selected_root"))),
            Some(PathBuf::from("/b/data"))
        );
        assert!(
            rendered_texts(&output.shapes)
                .iter()
                .any(|text| text == "Folder changes"),
            "the matching comparison result must remain visible"
        );
        // A real manual root selection must remain authoritative afterwards.
        let combo = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .into_iter()
            .find(|(_, node)| node.role() == egui::accesskit::Role::ComboBox)
            .unwrap()
            .1
            .bounds()
            .unwrap();
        click_changes_page(
            &mut app,
            &ctx,
            egui::pos2(
                ((combo.x0 + combo.x1) / 2.0) as f32,
                ((combo.y0 + combo.y1) / 2.0) as f32,
            ),
        );
        let output = settle_frames(&ctx, |ui| {
            app.changes_page(ui, app.preferences.theme.palette())
        });
        let root_choice = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .into_iter()
            .find(|(_, node)| node.label() == Some("/a/data"))
            .expect("root A choice must be available")
            .1
            .bounds()
            .unwrap();
        let position = egui::pos2(
            ((root_choice.x0 + root_choice.x1) / 2.0) as f32,
            ((root_choice.y0 + root_choice.y1) / 2.0) as f32,
        );
        click_changes_page(&mut app, &ctx, position);
        settle_frames(&ctx, |ui| {
            app.changes_page(ui, app.preferences.theme.palette())
        });
        assert_eq!(
            (app.older_scan, app.newer_scan),
            (Some(1), Some(2)),
            "manual root choice must persist across frames after explicit navigation"
        );
    }

    #[test]
    fn history_picker_disables_incompatible_pair_and_shows_full_root_identity() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app =
            DiskScopeApp::with_history_directory(&cc, None, PathBuf::new(), AccessMode::Admin);
        let summary = |id, root: &str, backend: &str| scan_history::ScanSummary {
            id,
            root: root.into(),
            finished_unix: id,
            logical: 1,
            files: 1,
            folders: 0,
            backend: backend.into(),
            live_generation: None,
        };
        let history = HistorySnapshot {
            scans: vec![
                summary(3, "/one/data", "Normal"),
                summary(2, "/one/data", "NTFS volume preview (raw)"),
                summary(1, "/two/data", "Normal"),
            ],
            ..Default::default()
        };
        app.older_scan = Some(2);
        app.newer_scan = Some(3);
        let output = settle_frames(&ctx, |ui| {
            app.comparison_picker(ui, &history, app.preferences.theme.palette())
        });
        assert!(
            rendered_texts(&output.shapes)
                .iter()
                .any(|t| t == "/one/data")
        );
        let update = output.platform_output.accesskit_update.unwrap();
        let compare = update
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some("Compare scans →"))
            .expect("Compare must be named in the accessibility tree");
        assert!(
            compare.1.is_disabled(),
            "incompatible families must never submit"
        );
        app.older_scan = Some(1);
        let _ = settle_frames(&ctx, |ui| {
            app.comparison_picker(ui, &history, app.preferences.theme.palette())
        });
        assert_ne!(
            app.older_scan,
            Some(1),
            "a same-named other root cannot survive selection validation"
        );
    }
}
