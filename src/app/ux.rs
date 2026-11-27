use super::*;

impl DiskScopeApp {
    pub(super) fn browse_context(&self) -> BrowseContext {
        let mut request = self.request.clone();
        // A focus is a pending lookup, not a place: restoring it later would
        // jump to a path the user has long since left.
        request.focus_path = None;
        BrowseContext {
            request,
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
        self.cancel_restore();
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
        let panel = egui::Panel::left("navigation")
            .exact_size(194.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(18.0))
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
                        (
                            egui::vec2(11.0, 13.0),
                            egui::vec2(11.0, 11.0),
                            p.border_strong,
                        ),
                    ] {
                        ui.painter().rect_filled(
                            Rect::from_min_size(rect.min + offset, size),
                            2.0,
                            color,
                        );
                    }
                    ui.label(title(ui, "DiskScope", TYPE_H2));
                });
                ui.label(
                    RichText::new("Disk usage analyzer")
                        .size(TYPE_LABEL)
                        .color(p.muted),
                );
                ui.add_space(22.0);
                overline(ui, "Views", p);
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
                                p.accent_text
                            } else {
                                p.muted
                            }))
                            .fill(if selected {
                                p.tint
                            } else {
                                Color32::TRANSPARENT
                            })
                            .selected(selected)
                            .stroke(Stroke::NONE)
                            .corner_radius(RADIUS_MD)
                            .min_size(egui::vec2(158.0, 40.0)),
                        )
                        .clicked()
                    {
                        self.destination(mode);
                    }
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("Changes").color(if self.changes {
                            p.accent_text
                        } else {
                            p.muted
                        }))
                        .fill(if self.changes {
                            p.tint
                        } else {
                            Color32::TRANSPARENT
                        })
                        .selected(self.changes)
                        .stroke(Stroke::NONE)
                        .corner_radius(RADIUS_MD)
                        .min_size(egui::vec2(158.0, 40.0)),
                    )
                    .clicked()
                {
                    self.show_changes();
                }
                if ui.available_height() > 310.0 {
                    ui.add_space(29.0);
                    overline(ui, "Current scan", p);
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
                        ui.label(title(
                            ui,
                            format_bytes(self.snapshot.progress.logical),
                            TYPE_H2,
                        ));
                        ui.label(
                            RichText::new("logical bytes discovered")
                                .small()
                                .color(p.muted),
                        );
                        if ui.small_button("Go to scan root").clicked() {
                            self.action(Action::Navigate(0));
                        }
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(if self.persistence_allowed() {
                                "Metadata-only history"
                            } else {
                                "History not saved"
                            })
                            .small()
                            .color(p.muted),
                        )
                        .truncate(),
                    )
                    .on_hover_text(if self.persistence_allowed() {
                        "Saved scans contain sizes and metadata only — never file contents."
                    } else {
                        "This session does not save comparison history."
                    });
                    ui.label(
                        RichText::new(format!("{} preview", env!("CARGO_PKG_VERSION")))
                            .small()
                            .color(p.accent_text),
                    );
                    if ui.button("Keyboard help  F1").clicked() {
                        self.help_open = true;
                    }
                    if ui.button("Settings").clicked() {
                        self.settings_open = true;
                    }
                });
            });
        // A hairline where the navigation plane meets the workspace, so the
        // sidebar reads as a step below the content rather than the same sheet.
        let edge = panel.response.rect;
        ui.painter().vline(
            edge.right(),
            edge.top()..=edge.bottom(),
            Stroke::new(1.0_f32, p.border),
        );
    }
    fn welcome(&mut self, ui: &mut egui::Ui, p: Palette) {
        egui::ScrollArea::vertical()
            .id_salt("welcome_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(20.0);
                // The hero states the app's purpose before it asks for
                // anything: measure honestly, let nothing leave without proof.
                // Meridian allows one dark emphasis panel per view; on the
                // welcome page it is the hero. Everything inside is coloured
                // explicitly, because the page's text colour is not on-ink.
                let ink = Palette {
                    surface: p.surface_ink,
                    text: p.on_ink,
                    muted: p.on_ink_muted,
                    ..p
                };
                card(ink).shadow(egui::Shadow::NONE).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    overline(ui, "DiskScope · honest disk usage for Windows", ink);
                    ui.add_space(6.0);
                    ui.label(
                        title(ui, "Know where every byte lives.", TYPE_METRIC).color(ink.text),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Every file counted once. Every size measured, never estimated. \
                             Nothing removed without your explicit confirmation. \
                             DiskScope shows what is actually on your disk — so you can \
                             decide, with proof, what stays.",
                        )
                        .color(ink.muted),
                    );
                    ui.add_space(16.0);
                    ui.horizontal_wrapped(|ui| {
                        if primary_button(
                            ui,
                            p,
                            !self.picker_open,
                            "Choose a folder…",
                            egui::vec2(180.0, 40.0),
                            RADIUS_MD,
                        )
                        .clicked()
                        {
                            self.choose_folder(ui.ctx());
                        }
                        ui.label(
                            RichText::new("or press Ctrl K for the command palette")
                                .small()
                                .color(ink.muted),
                        );
                    });
                    ui.add_space(18.0);
                    hero_relief(ui, ink);
                });
                ui.add_space(SPACE_3);
                self.purpose_pillars(ui, p);
                ui.add_space(SPACE_3);
                self.drive_cards(ui, p);
                if self.access_mode.persistence_allowed() && !self.preferences.recent_locations.is_empty() {
                    ui.add_space(SPACE_3);
                    card(p).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        overline(ui, "Recent locations", p);
                        ui.horizontal_wrapped(|ui| {
                            for path in self.preferences.recent_locations.clone().into_iter().take(5) {
                                if ui.button(format!("Scan {}", path.display())).clicked() {
                                    self.begin_scan(path);
                                }
                            }
                        });
                    });
                }
                ui.add_space(SPACE_3);
                card(p).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    overline(ui, "How it works", p);
                    for (title, description) in [
                        ("01  Scan", "Read file sizes and dates without opening file contents."),
                        ("02  Explore", "Open folders in the map or jump to Largest files while the scan runs."),
                        if self.persistence_allowed() {("03  Compare", "Compare saved scans of the same folder to see exactly what changed.")} else {("03  Evidence", "Export a redacted summary for your ticket. This session does not save comparison history.")},
                    ] {
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(title).strong());
                            ui.label(RichText::new(description).color(p.muted));
                        });
                    }
                    ui.add_space(SPACE_3);
                    ui.label(
                        RichText::new(if self.session_policy.helpdesk {
                            "Helpdesk diagnostic policy disables file removal. Use your approved cleanup procedure."
                        } else {
                            "Scanning never deletes files. File actions always require confirmation, then verify the result."
                        })
                        .small()
                        .color(p.muted),
                    );
                });
                ui.add_space(20.0);
            });
    }
    /// The three promises the app makes, stated once, up front.
    fn purpose_pillars(&mut self, ui: &mut egui::Ui, p: Palette) {
        let pillars: [(&str, String); 3] = [
            (
                "Measured, not estimated",
                "Sizes come from the file system itself. Hard-linked files are counted once, so totals never double-count.".into(),
            ),
            (
                "Answers while it scans",
                "Search names, types, sizes, and dates across the whole scan while it is still running.".into(),
            ),
            if self.persistence_allowed() {
                (
                    "Cleanup with proof",
                    "Remove a file only after explicit confirmation, then watch the freed space come back.".into(),
                )
            } else {
                (
                    "Evidence, not deletion",
                    "Export a redacted, ticket-ready summary of what you find. Removal stays with your approved procedure.".into(),
                )
            },
        ];
        let pillar = |ui: &mut egui::Ui, title: &str, body: &str| {
            card(p).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                // Columns are justified layouts, which would stretch the
                // word spacing of every wrapped line.
                ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                    ui.label(RichText::new(title).strong());
                    ui.add_space(4.0);
                    ui.label(RichText::new(body).small().color(p.muted));
                });
            });
        };
        if ui.available_width() > 700.0 {
            ui.columns(3, |columns| {
                for (column, (title, body)) in columns.iter_mut().zip(pillars) {
                    pillar(column, title, &body);
                }
            });
        } else {
            for (title, body) in pillars {
                pillar(ui, title, &body);
                ui.add_space(8.0);
            }
        }
    }
    /// One card per discovered drive with its live capacity picture. The used
    /// share is measured by the file system, matching what a scan will find.
    fn drive_cards(&mut self, ui: &mut egui::Ui, p: Palette) {
        if self.locations.is_empty() {
            return;
        }
        overline(ui, "Your drives", p);
        ui.add_space(4.0);
        for location in self.locations.clone() {
            card(p).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(
                            location
                                .path
                                .iter()
                                .next()
                                .map(|c| c.to_string_lossy().to_uppercase())
                                .unwrap_or_else(|| "·".into()),
                        )
                        .size(TYPE_H2)
                        .strong(),
                    );
                    ui.separator();
                    // The trailing action is laid out first so the capacity
                    // bar takes what is left instead of the whole row.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let scan = primary_button(
                            ui,
                            p,
                            true,
                            "Scan drive",
                            egui::vec2(64.0, 32.0),
                            RADIUS_SM,
                        );
                        scan.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                format!("Scan drive {}", location.path.display()),
                            )
                        });
                        if scan.clicked() {
                            self.begin_scan(location.path.clone());
                        }
                        ui.add_space(8.0);
                        ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(location.path.to_string_lossy())
                                        .small()
                                        .color(p.muted),
                                )
                                .truncate(),
                            )
                            .on_hover_text(location.path.to_string_lossy());
                            if let Some(space) = location.space {
                                let used = space.total.saturating_sub(space.available);
                                let used_fraction = eased_fraction(
                                    ui,
                                    egui::Id::new("drive_used").with(location.path.as_os_str()),
                                    share(used, space.total),
                                    0.35,
                                );
                                let (bar, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 6.0),
                                    Sense::hover(),
                                );
                                ui.painter().rect_filled(bar, 3.0, p.tint);
                                let fill = Rect::from_min_size(
                                    bar.min,
                                    egui::vec2(bar.width() * used_fraction, bar.height()),
                                );
                                ui.painter().rect_filled(fill, 3.0, p.accent);
                                ui.label(
                                    RichText::new(format!(
                                        "{} used · {} free of {}",
                                        format_bytes(used),
                                        format_bytes(space.available),
                                        format_bytes(space.total)
                                    ))
                                    .small()
                                    .color(p.muted),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Free space could not be read")
                                        .small()
                                        .color(p.muted),
                                );
                            }
                        });
                    });
                });
            });
            ui.add_space(6.0);
        }
    }

    pub(super) fn overview(&mut self, ui: &mut egui::Ui, p: Palette) {
        if self.generation == 0 {
            self.welcome(ui, p);
            return;
        }
        if self.request.root != 0 {
            self.navigation(ui, p);
        }
        // Always shown on Overview: this row holds its only Filters and
        // Compare pinned buttons, and the Stable display toggle must not
        // vanish the moment it is switched off.
        self.update_controls(ui, p);
        self.navigation_notice(ui, p);
        if self.request.filters.is_active() || !self.request.search.is_empty() {
            self.filter_chips(ui, p);
        }
        self.search_feedback(ui, p);
        let snapshot = self.visible.clone();
        let view = &snapshot.view;
        egui::ScrollArea::vertical()
            .id_salt("overview_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(12.0);
                if let Some(volume) = self.volume {
                    let control_height =
                        std::mem::replace(&mut ui.spacing_mut().interact_size.y, 16.0);
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
                            .color(p.muted),
                        );
                    });
                    ui.spacing_mut().interact_size.y = control_height;
                }
                ui.add_space(4.0);
                self.composition_card(ui, view, p);
                ui.add_space(8.0);
                if let Some(largest) = view.rows.first() {
                    card(p).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        overline(ui, "Largest item", p);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(title(
                                ui,
                                format!(
                                    "{} uses {:.1}% of this folder",
                                    largest.name,
                                    share(largest.logical, view.total_logical) * 100.0
                                ),
                                TYPE_H2,
                            ));
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
                    // "Previous" is only true when the newest saved scan is
                    // the one on screen (a cancelled or partial scan is not
                    // saved); otherwise say what will really be compared.
                    let current_is_saved = self.history_snapshot.scans.iter().any(|scan| {
                        scan.id == newer && scan.live_generation == Some(self.generation)
                    });
                    if ui
                        .button(if current_is_saved {
                            "Compare with previous scan →"
                        } else {
                            "Compare the last two saved scans →"
                        })
                        .clicked()
                    {
                        self.show_changes();
                        self.older_scan = Some(older);
                        self.newer_scan = Some(newer);
                        if !self.history_service.compare(older, newer) {
                            self.toasts.push(
                                toasts::ToastKind::Warning,
                                "History worker is busy. Try Compare again shortly.",
                            );
                        }
                    }
                } else {
                    ui.label(
                        RichText::new(if self.persistence_allowed() {
                            "No earlier complete scan of this folder yet."
                        } else {
                            "Scan history is unavailable in this window."
                        })
                        .small()
                        .color(p.muted),
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
                ui.label(title(ui, "Largest items", TYPE_H3));
                ui.label(RichText::new("in this folder").small().color(p.muted));
            });
            for row in view.rows.iter().take(6) {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    entry_icon(ui, row.is_dir, 14.0, p.accent_text);
                    // Size first, so a long name truncates instead of
                    // running underneath it.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(format_bytes(row.logical));
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new(&row.name).frame(false).truncate())
                                .on_hover_text(&row.name)
                                .clicked()
                            {
                                self.action(if row.is_dir {
                                    Action::Navigate(row.id)
                                } else {
                                    Action::Select(row.id)
                                });
                                if !row.is_dir {
                                    self.destination(ViewMode::Explorer);
                                }
                            }
                        });
                    });
                });
                animated_size_bar(
                    ui,
                    egui::Id::new("overview_row").with(row.id),
                    share(row.logical, view.total_logical),
                    p.accent,
                    p,
                );
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
                ui.label(title(ui, "Leading file types", TYPE_H3));
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
                        .add(egui::Button::new(&item.extension).frame(false))
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
                animated_size_bar(
                    ui,
                    egui::Id::new("overview_type").with(&item.extension),
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
    /// A single stacked bar answering the opening question — what is this
    /// space? — before any reading. Slices use the map's type colors and
    /// lens into their type on click.
    fn composition_card(&mut self, ui: &mut egui::Ui, view: &ViewSnapshot, p: Palette) {
        const SLICES: usize = 6;
        if view.types.is_empty() {
            return;
        }
        card(p).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(title(ui, "Composition", TYPE_H3));
                ui.label(
                    RichText::new("by file type · click a type to focus it")
                        .small()
                        .color(p.muted),
                );
            });
            ui.add_space(8.0);
            let width = ui.available_width();
            let (bar, _) = ui.allocate_exact_size(egui::vec2(width, 14.0), Sense::hover());
            ui.painter().rect_filled(bar, 7.0, p.tint);
            let mut cursor = bar.min.x;
            let mut covered = 0.0_f32;
            let mut legend: Vec<(String, Color32, u64)> = Vec::new();
            for item in view.types.iter().take(SLICES) {
                let fraction = eased_fraction(
                    ui,
                    egui::Id::new("composition").with(&item.extension),
                    overview_type_share(item.logical, view),
                    0.30,
                );
                let color = tile_color(&item.extension);
                let slice = Rect::from_min_max(
                    egui::pos2(cursor, bar.min.y),
                    egui::pos2((cursor + bar.width() * fraction).min(bar.max.x), bar.max.y),
                );
                if slice.is_positive() {
                    // Square joins between slices; the track rounds the ends.
                    ui.painter().rect_filled(slice, 0.0, color);
                }
                cursor += bar.width() * fraction;
                covered += fraction;
                legend.push((item.extension.clone(), color, item.logical));
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let mut focus = None;
                for (extension, color, logical) in &legend {
                    let label = extension.as_str();
                    ui.colored_label(*color, RichText::new("■").small());
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("{label} · {}", format_bytes(*logical)))
                                    .small()
                                    .color(p.muted),
                            )
                            .frame(false),
                        )
                        // Extensions arrive with their dot; the bucket for
                        // files without one arrives as a phrase.
                        .on_hover_text(if extension.starts_with('.') {
                            format!("Focus the list on {label} files")
                        } else {
                            "Focus the list on files without an extension".to_owned()
                        })
                        .clicked()
                    {
                        focus = Some(extension.clone());
                    }
                }
                if covered < 0.999 {
                    ui.label(RichText::new("· remaining types").small().color(p.muted));
                }
                if let Some(extension) = focus {
                    self.set_extension_lens(&extension);
                }
            });
        });
    }
    pub(super) fn explore_page(&mut self, ui: &mut egui::Ui, p: Palette) {
        self.navigation(ui, p);
        if self.preferences.stable_updates || self.pending.is_some() {
            self.update_controls(ui, p);
        }
        self.navigation_notice(ui, p);
        if self.request.filters.is_active() || !self.request.search.is_empty() {
            self.filter_chips(ui, p);
        }
        ui.add_space(4.0);
        if investigation::compact(ui.ctx()) {
            self.query_controls(ui, p);
            let available = ui.available_height();
            let details_reserve = (available * 0.45).clamp(110.0, 180.0);
            let list_chrome =
                20.0 + ui.spacing().interact_size.y + 3.0 * ui.spacing().item_spacing.y;
            let body_height = (available - details_reserve - list_chrome).clamp(38.0, 240.0);
            if self.request.mode == ViewMode::Types {
                self.types_table(ui, (available - details_reserve).max(100.0), p);
            } else {
                self.compact_file_list(ui, body_height, p);
            }
            let _details = egui::ScrollArea::vertical()
                .id_salt("compact_details")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::CollapsingHeader::new("Selected item · details and actions")
                        .id_salt(("compact_selection", self.request.selected))
                        .default_open(self.request.selected.is_some())
                        .show(ui, |ui| self.inspector(ui, p));
                    self.treemap(ui, 220.0, p);
                    self.comparison_tray(ui, p);
                });
            #[cfg(test)]
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    egui::Id::new("compact_details_viewport"),
                    _details.inner_rect,
                )
            });
            return;
        }
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(252.0)
            .min_size(180.0)
            .max_size(340.0)
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0_f32, p.card_outline))
                    .inner_margin(egui::Margin::same(SPACE_5))
                    .corner_radius(RADIUS_LG),
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
                    .fill(p.canvas)
                    .inner_margin(egui::Margin {
                        left: 0,
                        right: 12,
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
                                0.36
                            } else {
                                0.60
                            })
                        .clamp(200.0, 480.0);
                        self.explorer(ui, table_height, p);
                        ui.add_space(12.0);
                        self.treemap(ui, (available - table_height - 12.0).max(180.0), p);
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
        ui.horizontal_wrapped(|ui| {
            let stable = ux::checkbox(ui, &mut self.preferences.stable_updates, "Stable display")
                .on_hover_text(
                    "Keep the visible results still while background scanning and progress continue. Navigation and selection still update.",
                );
            if stable.changed() && !self.preferences.stable_updates {
                self.apply_pending();
            }
            if self.pending.is_some() {
                ui.colored_label(p.accent_text, "New results waiting");
                if ui.button("Apply update").clicked() {
                    self.apply_pending();
                }
            }
            let filters_label = if self.request.filters.is_active() {
                "Edit filters •"
            } else {
                "Filters"
            };
            if self.request.mode == ViewMode::Overview && ui.button(filters_label).clicked() {
                self.sync_filter_fields();
                self.filter_open = true;
            }
            if !self.request.pinned.is_empty()
                && ui
                    .button(format!("Compare pinned ({})", self.request.pinned.len()))
                    .clicked()
            {
                self.comparison_open = true;
            }
        });
    }
    fn navigation_notice(&self, ui: &mut egui::Ui, p: Palette) {
        // While a restore is pending its focus is expected to miss for a
        // while, so the worker's "path is not present" note would mislead.
        if self.restore.is_none()
            && let Some(notice) = self.visible.view.notice.as_ref()
        {
            ui.colored_label(p.muted, notice);
        }
    }
    pub(super) fn apply_pending(&mut self) {
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
            if self.visible.view.filters_active {
                ui.label(
                    RichText::new(format!(
                        "{} matching files · {} logical",
                        format_count(self.visible.view.matching_files),
                        format_bytes(self.visible.view.matching_logical)
                    ))
                    .small()
                    .color(p.accent_text),
                );
            }
            for field in FilterField::ALL {
                if let Some(label) = field.chip_label(&self.request.filters)
                    && ui
                        .add(
                            egui::Button::new(format!("{label}  ×"))
                                .small()
                                .fill(p.tint),
                        )
                        .clicked()
                {
                    clear.push(field);
                }
            }
            if !self.request.search.is_empty()
                && ui
                    .small_button(format!("Search: {}  ×", self.request.search))
                    .clicked()
            {
                self.request.search.clear();
                self.mark_query_changed();
            }
            if self.request.filters.is_active() && ui.small_button("Clear filters").clicked() {
                clear.extend(FilterField::ALL);
            }
        });
        if !clear.is_empty() {
            for field in clear {
                field.clear(&mut self.request.filters);
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

/// One removable chip per filter condition.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterField {
    Extensions,
    MinBytes,
    MaxBytes,
    ModifiedAfter,
    ModifiedBefore,
}
impl FilterField {
    const ALL: [FilterField; 5] = [
        Self::Extensions,
        Self::MinBytes,
        Self::MaxBytes,
        Self::ModifiedAfter,
        Self::ModifiedBefore,
    ];
    fn chip_label(self, f: &QueryFilter) -> Option<String> {
        match self {
            Self::Extensions => (!f.extensions.is_empty()).then(|| f.extensions.join(", ")),
            Self::MinBytes => f.min_bytes.map(|n| format!("At least {}", format_bytes(n))),
            Self::MaxBytes => f.max_bytes.map(|n| format!("At most {}", format_bytes(n))),
            Self::ModifiedAfter => f
                .modified_after
                .map(|n| format!("Modified since {}", date_label(n))),
            Self::ModifiedBefore => f
                .modified_before
                .map(|n| format!("Modified through {}", date_label(n))),
        }
    }
    fn clear(self, f: &mut QueryFilter) {
        match self {
            Self::Extensions => f.extensions.clear(),
            Self::MinBytes => f.min_bytes = None,
            Self::MaxBytes => f.max_bytes = None,
            Self::ModifiedAfter => f.modified_after = None,
            Self::ModifiedBefore => f.modified_before = None,
        }
    }
}

pub(super) fn card(p: Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0_f32, p.card_outline))
        .corner_radius(RADIUS_LG)
        .inner_margin(egui::Margin::same(SPACE_5))
        .shadow(p.card_shadow)
}
/// Meridian's primary button: the one accent-filled action of its group,
/// stepping to `accent-hover` under the pointer. The focus ring is drawn in
/// `on-accent`, since the ordinary ring colour is the fill itself.
pub(super) fn primary_button(
    ui: &mut egui::Ui,
    p: Palette,
    enabled: bool,
    label: &str,
    min_size: egui::Vec2,
    radius: u8,
) -> egui::Response {
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = p.accent;
        widgets.inactive.bg_stroke = Stroke::NONE;
        widgets.hovered.weak_bg_fill = p.accent_hover;
        widgets.hovered.bg_stroke = Stroke::NONE;
        widgets.active.weak_bg_fill = p.accent_hover;
        widgets.active.bg_stroke = Stroke::new(2.0_f32, p.on_accent);
        ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(label).strong().color(p.on_accent))
                .corner_radius(radius)
                .min_size(min_size),
        )
    })
    .inner
}
/// A checkbox with Meridian's small box radius. Controls share `radius-sm`,
/// which on a box this size would round it into a radio button.
pub(super) fn checkbox(ui: &mut egui::Ui, value: &mut bool, text: &str) -> egui::Response {
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        for state in [
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.noninteractive,
        ] {
            state.corner_radius = egui::CornerRadius::same(5);
        }
        ui.spacing_mut().icon_width = 18.0;
        ui.spacing_mut().icon_width_inner = 10.0;
        ui.checkbox(value, text)
    })
    .inner
}
/// Meridian's badge: one or two status words on the status's own tint. The
/// word carries the meaning; the colour only reinforces it.
pub(super) fn badge(
    ui: &mut egui::Ui,
    text: &str,
    ink: Color32,
    ground: Color32,
) -> egui::Response {
    egui::Frame::new()
        .fill(ground)
        .corner_radius(RADIUS_FULL)
        .inner_margin(egui::Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().strong().color(ink))
        })
        .inner
}
pub(super) fn size_bar(ui: &mut egui::Ui, fraction: f32, color: Color32, p: Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.0), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, p.tint);
    ui.painter().rect_filled(
        Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction, rect.height())),
        2.0,
        color,
    );
}
/// A size bar whose width eases toward its target instead of snapping. Living
/// data should feel alive; the eased value is presentation, never the
/// reported figure.
/// Reduced-motion sessions (style animation time 0) keep the exact value.
pub(super) fn animated_size_bar(
    ui: &mut egui::Ui,
    id: egui::Id,
    fraction: f32,
    color: Color32,
    p: Palette,
) {
    let eased = eased_fraction(ui, id, fraction, 0.30);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.0), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, p.tint);
    let fill = Rect::from_min_size(rect.min, egui::vec2(rect.width() * eased, rect.height()));
    ui.painter().rect_filled(fill, 2.0, color);
}
/// Shared easing for motion-friendly figures. First appearance does not
/// animate (egui seeds at the target); later changes glide.
pub(super) fn eased_fraction(ui: &mut egui::Ui, id: egui::Id, target: f32, seconds: f32) -> f32 {
    if ui.ctx().global_style().animation_time <= 0.0 {
        target
    } else {
        ui.ctx()
            .animate_value_with_time(id, target.clamp(0.0, 1.0), seconds)
    }
}
/// The hero's abstract space map: a quiet, beveled relief of tiles in the
/// app's own category colors. Decoration, not data — it tells you what the
/// app looks like before you scan anything.
fn hero_relief(ui: &mut egui::Ui, p: Palette) {
    const HEIGHT: f32 = 96.0;
    let width = ui.available_width();
    if width < 480.0 {
        return;
    }
    let (band, _) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), Sense::hover());
    let tiles: [([f32; 4], Color32); 9] = [
        ([0.00, 0.00, 0.34, 1.00], OTHER_TILE_COLOR),
        ([0.36, 0.00, 0.20, 0.55], FILE_CATEGORIES[0].color),
        ([0.36, 0.57, 0.20, 0.43], FILE_CATEGORIES[1].color),
        ([0.58, 0.00, 0.14, 0.32], FILE_CATEGORIES[2].color),
        ([0.58, 0.34, 0.14, 0.28], FILE_CATEGORIES[3].color),
        ([0.58, 0.64, 0.14, 0.36], FILE_CATEGORIES[4].color),
        ([0.74, 0.00, 0.10, 1.00], OTHER_TILE_COLOR),
        ([0.86, 0.00, 0.14, 0.46], FILE_CATEGORIES[0].color),
        ([0.86, 0.48, 0.14, 0.52], FILE_CATEGORIES[2].color),
    ];
    let painter = ui.painter();
    for ([fx, fy, fw, fh], color) in tiles {
        let tile = Rect::from_min_max(
            egui::pos2(band.min.x + width * fx, band.min.y + HEIGHT * fy),
            egui::pos2(
                band.min.x + width * (fx + fw),
                band.min.y + HEIGHT * (fy + fh),
            ),
        )
        .shrink(1.5);
        if !tile.is_positive() {
            continue;
        }
        let fill = crate::theme::mix(color, p.surface, 0.55);
        // Raised block: a dark bed, the face set slightly into it from the
        // bottom right, and a light catch on the top and left edges.
        painter.rect_filled(tile, 3.0, crate::theme::darken(fill, 0.35));
        let face = Rect::from_min_max(
            tile.min,
            egui::pos2(tile.right() - 1.0, tile.bottom() - 1.0),
        );
        painter.rect_filled(face, 3.0, fill);
        let hairline = 1.0;
        painter.rect_filled(
            Rect::from_min_size(
                egui::pos2(face.min.x, face.min.y),
                egui::vec2(face.width(), hairline),
            ),
            3.0,
            crate::theme::lighten(fill, 0.22),
        );
        painter.rect_filled(
            Rect::from_min_size(
                egui::pos2(face.min.x, face.min.y),
                egui::vec2(hairline, face.height()),
            ),
            3.0,
            crate::theme::lighten(fill, 0.22),
        );
    }
}
