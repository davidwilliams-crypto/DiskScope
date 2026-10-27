use super::*;

impl DiskScopeApp {
    pub(super) fn browse_context(&self) -> BrowseContext {
        BrowseContext {
            request: self.request.clone(),
            changes: self.changes,
            older_scan: self.older_scan,
            newer_scan: self.newer_scan,
        }
    }
    pub(super) fn remember_context(&mut self) {
        let current = self.browse_context();
        self.history.remember(&current);
    }
    pub(super) fn restore_context(&mut self, context: BrowseContext) {
        let context = restore_browse_context(&self.request, context);
        self.request = context.request;
        self.changes = context.changes;
        self.older_scan = context.older_scan;
        self.newer_scan = context.newer_scan;
        self.search_changed = None;
        self.dirty = true;
    }
    fn transition(&mut self, mode: Option<ViewMode>) {
        if self.management.busy() {
            return;
        }
        let current = self.browse_context();
        let next = transitioned(&current, mode);
        if current.changes != next.changes || current.request.mode != next.request.mode {
            self.history.remember(&current);
        }
        self.changes = next.changes;
        self.request.mode = next.request.mode;
        if self.request.mode == ViewMode::Types
            && matches!(self.request.sort, SortKey::Allocated | SortKey::Modified)
        {
            self.request.sort = SortKey::Logical;
        }
        self.mark_query_changed();
    }
    pub(super) fn destination(&mut self, mode: ViewMode) {
        self.transition(Some(mode));
    }
    pub(super) fn show_changes(&mut self) {
        self.transition(None);
    }
    pub(super) fn apply_appearance(&self, ctx: &egui::Context) {
        self.preferences.theme.apply(ctx);
        ctx.set_zoom_factor(self.preferences.text_scale.factor());
        ctx.global_style_mut(|style| {
            style.animation_time = if self.preferences.reduced_motion {
                0.0
            } else {
                0.12
            };
        });
    }
    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui, p: Palette) {
        egui::Panel::left("navigation")
            .exact_size(180.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(p.side).inner_margin(18.0))
            .show_inside(ui, |ui| {
                if self.management.busy() {
                    ui.disable();
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 24.0), Sense::hover());
                    for (offset, size, color) in [
                        (egui::vec2(0.0, 0.0), egui::vec2(8.0, 24.0), p.accent),
                        (egui::vec2(11.0, 0.0), egui::vec2(11.0, 10.0), p.accent),
                        (egui::vec2(11.0, 13.0), egui::vec2(11.0, 11.0), p.faint),
                    ] {
                        ui.painter().rect_filled(
                            Rect::from_min_size(rect.min + offset, size),
                            2.0,
                            color,
                        );
                    }
                    ui.label(RichText::new("DiskScope").size(21.0).strong());
                });
                ui.label(
                    RichText::new("SPACE TO UNDERSTAND")
                        .size(9.0)
                        .color(p.faint),
                );
                ui.add_space(31.0);
                overline(ui, "WORKSPACE", p);
                for (mode, label) in [
                    (ViewMode::Overview, "Overview"),
                    (ViewMode::Explorer, "Explorer"),
                    (ViewMode::Largest, "Largest files"),
                    (ViewMode::Types, "File types"),
                ] {
                    let selected = !self.changes && self.request.mode == mode;
                    if ui
                        .add(
                            egui::Button::new(RichText::new(label).color(if selected {
                                p.accent
                            } else {
                                p.muted
                            }))
                            .fill(if selected { p.wash } else { p.side })
                            .stroke(Stroke::NONE)
                            .min_size(egui::vec2(144.0, 37.0)),
                        )
                        .clicked()
                    {
                        self.destination(mode);
                    }
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("Changes").color(if self.changes {
                            p.accent
                        } else {
                            p.muted
                        }))
                        .fill(if self.changes { p.wash } else { p.side })
                        .stroke(Stroke::NONE)
                        .min_size(egui::vec2(144.0, 37.0)),
                    )
                    .clicked()
                {
                    self.show_changes();
                }
                if ui.available_height() > 310.0 {
                    ui.add_space(29.0);
                    overline(ui, "CURRENT SCAN", p);
                    let root = &self.snapshot.progress.root;
                    if root.as_os_str().is_empty() {
                        ui.label(RichText::new("No folder selected").color(p.muted));
                    } else {
                        ui.add(
                            egui::Label::new(
                                RichText::new(
                                    root.file_name()
                                        .unwrap_or(root.as_os_str())
                                        .to_string_lossy(),
                                )
                                .strong(),
                            )
                            .truncate(),
                        )
                        .on_hover_text(root.to_string_lossy());
                        ui.label(
                            RichText::new(format_bytes(self.snapshot.progress.logical))
                                .size(21.0)
                                .color(p.accent),
                        );
                        ui.label(
                            RichText::new("logical bytes discovered")
                                .small()
                                .color(p.faint),
                        );
                        if ui.small_button("Go to scan root").clicked() {
                            self.action(Action::Navigate(0));
                        }
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.label(
                        RichText::new("Metadata-only history")
                            .small()
                            .color(p.faint),
                    );
                    ui.label(
                        RichText::new(format!("{}  PREVIEW", env!("CARGO_PKG_VERSION")))
                            .small()
                            .color(p.accent),
                    );
                    if ui.button("Keyboard help  F1").clicked() {
                        self.help_open = true;
                    }
                    if ui.button("Appearance & settings").clicked() {
                        self.settings_open = true;
                    }
                });
            });
    }
    pub(super) fn overview(&mut self, ui: &mut egui::Ui, p: Palette) {
        if self.generation == 0 {
            ui.add_space(38.0);
            card(p).show(ui,|ui| {
                ui.label(RichText::new("Make room for what matters.").size(30.0).strong());
                ui.add_space(12.0);
                ui.label("Choose a folder above to discover its largest folders and file types. History stores metadata only; Changes shows its storage location.");
                ui.add_space(12.0);
                if ui.button("Choose a folder…").clicked(){self.choose_folder(ui.ctx());}
                ui.add_space(10.0);ui.label(RichText::new("Scans read metadata. Single-file actions require a separate confirmation.").color(p.muted));
            });
            return;
        }
        if self.request.root != 0 {
            self.navigation(ui, p);
        }
        if self.preferences.stable_updates || self.pending.is_some() {
            self.update_controls(ui, p);
        }
        if self.request.filters.is_active() || !self.request.search.is_empty() {
            self.filter_chips(ui, p);
        }
        let snapshot = self.visible.clone();
        let view = &snapshot.view;
        egui::ScrollArea::vertical()
            .id_salt("overview_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(12.0);
                if let Some(volume) = self.volume {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "Volume: {} available to you / {} capacity",
                                format_bytes(volume.available),
                                format_bytes(volume.total)
                            ))
                            .color(p.muted),
                        );
                        ui.label(
                            RichText::new(
                                "Volume capacity is separate from this folder's logical size.",
                            )
                            .small()
                            .color(p.faint),
                        );
                    });
                    ui.add_space(10.0);
                }
                if let Some(largest) = view.rows.first() {
                    card(p).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        overline(ui, "START HERE", p);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} uses {:.1}% of this folder",
                                    largest.name,
                                    share(largest.logical, view.total_logical) * 100.0
                                ))
                                .size(22.0)
                                .strong(),
                            );
                            if ui
                                .button(if largest.is_dir {
                                    "Explore folder →"
                                } else {
                                    "Inspect file →"
                                })
                                .clicked()
                            {
                                self.action(if largest.is_dir {
                                    Action::Navigate(largest.id)
                                } else {
                                    Action::Select(largest.id)
                                });
                                if !largest.is_dir {
                                    self.destination(ViewMode::Explorer);
                                }
                            }
                        });
                        ui.label(
                            RichText::new(format!(
                                "{} logical · factual sizes from this scan",
                                format_bytes(largest.logical)
                            ))
                            .color(p.muted),
                        );
                    });
                }
                ui.add_space(8.0);
                if let Some((older, newer)) =
                    previous_pair(&self.history_snapshot.scans, &self.snapshot.progress.root)
                {
                    if ui.button("Compare with previous scan →").clicked() {
                        self.show_changes();
                        self.older_scan = Some(older);
                        self.newer_scan = Some(newer);
                        if !self.history_service.compare(older, newer) {
                            self.notice =
                                Some("History worker is busy. Try Compare again shortly.".into());
                        }
                    }
                } else {
                    ui.label(
                        RichText::new(if self.access_mode.persistence_allowed() {
                            "No earlier complete scan of this folder yet."
                        } else {
                            "Scan history is unavailable in this window."
                        })
                        .small()
                        .color(p.faint),
                    );
                }
                ui.add_space(8.0);
                if ui.available_width() > 760.0 {
                    ui.columns(2, |columns| {
                        self.overview_folders(&mut columns[0], view, p);
                        self.overview_types(&mut columns[1], view, p);
                    });
                } else {
                    self.overview_folders(ui, view, p);
                    ui.add_space(12.0);
                    self.overview_types(ui, view, p);
                }
                ui.add_space(18.0);
                self.treemap(ui, 270.0, p);
                ui.add_space(12.0);
                self.comparison_tray(ui, p);
            });
    }
    fn overview_folders(&mut self, ui: &mut egui::Ui, view: &ViewSnapshot, p: Palette) {
        card(p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Largest items").size(17.0).strong());
                ui.label(RichText::new("in this folder").small().color(p.muted));
            });
            for row in view.rows.iter().take(6) {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    entry_icon(ui, row.is_dir, 14.0, p.accent);
                    if ui.add(egui::Button::new(&row.name).frame(false)).clicked() {
                        self.action(if row.is_dir {
                            Action::Navigate(row.id)
                        } else {
                            Action::Select(row.id)
                        });
                        if !row.is_dir {
                            self.destination(ViewMode::Explorer);
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(format_bytes(row.logical));
                    });
                });
                size_bar(ui, share(row.logical, view.total_logical), p.accent, p);
            }
            ui.add_space(8.0);
            if ui.button("View all in Explorer →").clicked() {
                self.destination(ViewMode::Explorer);
            }
        });
    }
    fn overview_types(&mut self, ui: &mut egui::Ui, view: &ViewSnapshot, p: Palette) {
        card(p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Leading file types").size(17.0).strong());
                ui.label(
                    RichText::new(if view.row_scope_root == 0 {
                        "whole scan · click to explore"
                    } else {
                        "current folder · click to explore"
                    })
                    .small()
                    .color(p.muted),
                );
            });
            for item in view.types.iter().take(5) {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.colored_label(tile_color(&item.extension), "■");
                    if ui
                        .add(
                            egui::Button::new(if item.extension.is_empty() {
                                "No extension"
                            } else {
                                &item.extension
                            })
                            .frame(false),
                        )
                        .clicked()
                    {
                        self.set_extension_lens(&item.extension);
                    }
                    ui.label(
                        RichText::new(format!(
                            "{} {}",
                            format_count(item.files),
                            if item.files == 1 { "file" } else { "files" }
                        ))
                        .small()
                        .color(p.muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(format_bytes(item.logical));
                    });
                });
                size_bar(
                    ui,
                    overview_type_share(item.logical, view),
                    tile_color(&item.extension),
                    p,
                );
            }
            ui.add_space(8.0);
            if ui.button("Compare all file types →").clicked() {
                self.destination(ViewMode::Types);
            }
        });
    }
    pub(super) fn explore_page(&mut self, ui: &mut egui::Ui, p: Palette) {
        self.navigation(ui, p);
        self.update_controls(ui, p);
        self.filter_chips(ui, p);
        ui.add_space(6.0);
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(252.0)
            .min_size(180.0)
            .max_size(340.0)
            .frame(
                egui::Frame::new()
                    .fill(p.side)
                    .inner_margin(16.0)
                    .corner_radius(8),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.inspector(ui, p));
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(egui::Margin {
                        left: 0,
                        right: 16,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("explore_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let available = ui.available_height();
                        let table_height = (available
                            * if self.preferences.map_focus {
                                0.32
                            } else {
                                0.48
                            })
                        .clamp(180.0, 380.0);
                        self.explorer(ui, table_height, p);
                        ui.add_space(12.0);
                        self.treemap(ui, (available - table_height - 12.0).max(240.0), p);
                        self.comparison_tray(ui, p);
                    });
            });
    }
    pub(super) fn set_extension_lens(&mut self, extension: &str) {
        self.remember_context();
        apply_type_lens(&mut self.request, extension);
        self.filter_extensions = self.request.filters.extensions.join(", ");
        self.changes = false;
        self.mark_query_changed();
    }
    fn update_controls(&mut self, ui: &mut egui::Ui, p: Palette) {
        ui.horizontal_wrapped(|ui|{
            if ui.checkbox(&mut self.preferences.stable_updates,"Stable display").on_hover_text("Keep the visible results still while background scanning and progress continue. Navigation and selection still update.").changed() && !self.preferences.stable_updates {self.apply_pending();}
            if self.pending.is_some(){
                ui.colored_label(p.accent,"New results waiting");
                if ui.button("Apply update").clicked(){self.apply_pending();}
            }
            if ui.button(if self.request.filters.is_active(){"Edit filters •"}else{"Filters"}).clicked(){self.sync_filter_fields();self.filter_open=true;}
            if !self.request.pinned.is_empty() && ui.button(format!("Compare pinned ({})",self.request.pinned.len())).clicked(){self.comparison_open=true;}
            if !self.changes && self.visible.view.filters_active {
                ui.label(RichText::new(format!("{} matching files · {} logical",format_count(self.visible.view.matching_files),format_bytes(self.visible.view.matching_logical))).small().color(p.accent));
            }
        });
        if let Some(notice) = self.visible.view.notice.as_ref() {
            ui.colored_label(p.muted, notice);
        }
    }
    fn apply_pending(&mut self) {
        if let Some(pending) = self.pending.take()
            && admits_snapshot(
                self.generation,
                self.request.revision,
                pending.view.generation,
                pending.view.revision,
            )
        {
            self.visible = pending;
            self.visible_offset = self.request.offset;
        }
    }
    fn filter_chips(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut clear = Vec::new();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(if self.request.search_all {
                    "Scope: whole scan"
                } else {
                    "Scope: current folder"
                })
                .small()
                .color(p.muted),
            );
            let f = &self.request.filters;
            for (key, label) in [
                (
                    0,
                    if f.extensions.is_empty() {
                        None
                    } else {
                        Some(f.extensions.join(", "))
                    },
                ),
                (
                    1,
                    f.min_bytes.map(|n| format!("At least {}", format_bytes(n))),
                ),
                (
                    2,
                    f.max_bytes.map(|n| format!("At most {}", format_bytes(n))),
                ),
                (
                    3,
                    f.modified_after
                        .map(|n| format!("Modified since {}", date_label(n))),
                ),
                (
                    4,
                    f.modified_before
                        .map(|n| format!("Modified through {}", date_label(n))),
                ),
            ] {
                if let Some(label) = label
                    && ui
                        .add(
                            egui::Button::new(format!("{label}  ×"))
                                .small()
                                .fill(p.wash),
                        )
                        .clicked()
                {
                    clear.push(key);
                }
            }
            if !self.request.search.is_empty()
                && ui
                    .small_button(format!("Name: {}  ×", self.request.search))
                    .clicked()
            {
                self.request.search.clear();
                self.mark_query_changed();
            }
            if self.request.filters.is_active() && ui.small_button("Clear filters").clicked() {
                clear.extend(0..5);
            }
        });
        if !clear.is_empty() {
            for key in clear {
                match key {
                    0 => self.request.filters.extensions.clear(),
                    1 => self.request.filters.min_bytes = None,
                    2 => self.request.filters.max_bytes = None,
                    3 => self.request.filters.modified_after = None,
                    _ => self.request.filters.modified_before = None,
                }
            }
            self.mark_query_changed();
        }
    }
    pub(super) fn sync_filter_fields(&mut self) {
        let f = &self.request.filters;
        self.filter_extensions = f.extensions.join(", ");
        self.filter_min = f
            .min_bytes
            .map(|n| format!("{}", n as f64 / 1048576.0))
            .unwrap_or_default();
        self.filter_max = f
            .max_bytes
            .map(|n| format!("{}", n as f64 / 1048576.0))
            .unwrap_or_default();
        self.filter_after = f.modified_after.map(date_label).unwrap_or_default();
        self.filter_before = f.modified_before.map(date_label).unwrap_or_default();
    }
}

pub(super) fn card(p: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.side)
        .corner_radius(9)
        .inner_margin(18.0)
}
pub(super) fn size_bar(ui: &mut egui::Ui, fraction: f32, color: Color32, p: Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.0), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, p.raised);
    ui.painter().rect_filled(
        Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction, rect.height())),
        2.0,
        color,
    );
}
