use super::*;

#[derive(Clone, Default)]
struct DialogFocus {
    invoker: Option<egui::Id>,
    initialized: bool,
}

#[derive(Clone, Default)]
struct PendingDialogRestore {
    invoker: Option<egui::Id>,
    retry: bool,
}

fn dialog_focus_key(title: &str) -> egui::Id {
    egui::Id::new(("dialog_invoker", title))
}

/// Call when an invoking control is available, before opening its window.
pub(super) fn set_dialog_invoker(ctx: &egui::Context, title: &str, invoker: egui::Id) {
    ctx.data_mut(|d| {
        d.insert_temp(
            dialog_focus_key(title),
            DialogFocus {
                invoker: Some(invoker),
                initialized: false,
            },
        )
    });
}

/// Capture the previously focused control once, before the first window draw.
pub(super) fn remember_dialog_focus(ctx: &egui::Context, title: &str) {
    let focused = ctx.memory(|m| m.focused());
    let first_draw = ctx.data_mut(|d| {
        let saved = d.get_temp_mut_or_insert_with(dialog_focus_key(title), || DialogFocus {
            invoker: focused,
            initialized: false,
        });
        let first_draw = !saved.initialized;
        saved.initialized = true;
        first_draw
    });
    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new(title));
    if first_draw {
        ctx.move_to_top(layer);
    }
    ctx.memory_mut(|m| {
        m.set_modal_layer(layer);
        if first_draw && let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
        // A window's first sizing pass has no interactive controls yet.
        // Retry on its visible pass, and wrap Tab after its last control.
        if m.focused().is_none() && m.allows_interaction(layer) {
            m.move_focus(egui::FocusDirection::Next);
        }
    });
}

/// Use for both Escape and the title-bar close button.
pub(super) fn restore_dialog_focus(ctx: &egui::Context, title: &str) {
    let saved = ctx.data_mut(|d| d.remove_temp::<DialogFocus>(dialog_focus_key(title)));
    if let Some(id) = saved.and_then(|s| s.invoker) {
        ctx.memory_mut(|m| m.request_focus(id));
        ctx.data_mut(|d| {
            d.insert_temp(
                egui::Id::new("pending_dialog_focus_restore"),
                PendingDialogRestore {
                    invoker: Some(id),
                    retry: false,
                },
            )
        });
    }
}

/// egui still applies the previous frame's modal layer while drawing the
/// closing frame. Restore after those widgets can no longer surrender focus.
pub(super) fn finish_dialog_frame(ctx: &egui::Context) {
    let key = egui::Id::new("pending_dialog_focus_restore");
    if let Some(mut pending) = ctx.data_mut(|d| d.remove_temp::<PendingDialogRestore>(key)) {
        if let Some(id) = pending.invoker {
            ctx.memory_mut(|m| {
                if !pending.retry || m.focused().is_none() {
                    m.request_focus(id);
                }
            });
        }
        if !pending.retry {
            // A title-bar close registers this window's modal layer during
            // its final draw. That stale layer can surrender focus once more
            // on the next frame, unlike Escape (which closes before drawing).
            pending.retry = true;
            ctx.data_mut(|d| d.insert_temp(key, pending));
        }
        ctx.request_repaint();
    }
    if ctx.memory(|m| m.top_modal_layer().is_some() && m.focused().is_none()) {
        ctx.request_repaint();
    }
}

pub(super) const FILE_COLUMN_MIN_WIDTHS: [f32; 4] = [150.0, 72.0, 72.0, 50.0];
const COLUMN_KEYBOARD_STEP: f32 = 16.0;

fn table_keyboard_widths_key(ui: &egui::Ui, table_salt: &str) -> egui::Id {
    ui.id()
        .with(egui::Id::new(table_salt))
        .with("keyboard_widths")
}

pub(super) fn take_table_keyboard_widths(ui: &egui::Ui, table_salt: &str) -> Option<Vec<f32>> {
    ui.data_mut(|d| d.remove_temp::<Vec<f32>>(table_keyboard_widths_key(ui, table_salt)))
}

/// egui_extras 0.34.3 generates focusable column handles without widget info
/// or keyboard resizing. Queue widths through its public initial/reset API.
pub(super) fn describe_table_resize_handles(
    ui: &egui::Ui,
    table_salt: &str,
    columns: &[&str],
    widths: &[f32],
    minimums: &[f32],
    width_budget: f32,
    p: Palette,
) {
    let state_id = ui.id().with(egui::Id::new(table_salt));
    for (index, column) in columns.iter().enumerate() {
        let id = state_id.with("resize_column").with(index);
        let Some(response) = ui.ctx().read_response(id) else {
            continue;
        };
        let current = widths[index];
        let minimum = minimums[index];
        let maximum = if index + 1 < widths.len() {
            current + (widths[index + 1] - minimums[index + 1]).max(0.0)
        } else {
            width_budget - widths[..index].iter().sum::<f32>()
        }
        .max(minimum);
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(egui::accesskit::Role::Splitter);
            node.set_label(format!("Resize {column} column"));
            node.set_description(
                "Press Left or Right to resize by 16 points, or drag with a pointing device.",
            );
            node.set_orientation(egui::accesskit::Orientation::Vertical);
            node.set_numeric_value(f64::from(current));
            node.set_min_numeric_value(f64::from(minimum));
            node.set_max_numeric_value(f64::from(maximum));
            node.set_numeric_value_step(f64::from(COLUMN_KEYBOARD_STEP));
        });
        if response.has_focus() {
            ui.painter().vline(
                response.rect.center().x,
                response.rect.y_range(),
                Stroke::new(2.0_f32, p.accent),
            );
            let delta = ui.input_mut(|input| {
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                    Some(-COLUMN_KEYBOARD_STEP)
                } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                    Some(COLUMN_KEYBOARD_STEP)
                } else {
                    None
                }
            });
            if let Some(delta) = delta {
                ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
                let mut adjusted = widths.to_vec();
                adjusted[index] = (current + delta).clamp(minimum, maximum);
                if index + 1 < adjusted.len() {
                    adjusted[index + 1] -= adjusted[index] - current;
                }
                ui.data_mut(|d| d.insert_temp(table_keyboard_widths_key(ui, table_salt), adjusted));
                ui.ctx().request_repaint();
            }
        }
    }
}

impl DiskScopeApp {
    fn transient_dialogs(&self) -> [(&'static str, bool); 7] {
        [
            ("Scan errors & skipped locations", self.show_errors),
            ("Filter files", self.filter_open),
            ("Commands", self.palette_open),
            ("Appearance & settings", self.settings_open),
            ("Keyboard help", self.help_open),
            ("Pinned comparison", self.comparison_open),
            ("Export investigation evidence", self.export_open),
        ]
    }
    pub(super) fn transient_dialog_open(&self) -> bool {
        self.transient_dialogs().iter().any(|(_, open)| *open)
    }
    pub(super) fn capture_dialog_invokers(&self, ctx: &egui::Context) {
        let focused = ctx.memory(|m| m.focused());
        for (title, open) in self.transient_dialogs() {
            if open {
                ctx.data_mut(|d| {
                    d.get_temp_mut_or_insert_with(dialog_focus_key(title), || DialogFocus {
                        invoker: focused,
                        initialized: false,
                    });
                });
            }
        }
    }
    pub(super) fn filter_window(&mut self, ctx: &egui::Context, p: Palette) {
        if !self.filter_open {
            return;
        }
        remember_dialog_focus(ctx, "Filter files");
        let mut open = true;
        egui::Window::new("Filter files")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Conditions apply together. Folder map area stays logical; matches are highlighted.",
                    )
                    .color(p.muted),
                );
                ui.add_space(10.0);
                let label = ui.label("File extensions (comma separated)");
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter_extensions)
                        .hint_text(".mp4, .mkv")
                        .char_limit(1024)
                        .desired_width(f32::INFINITY),
                ).labelled_by(label.id);
                ui.horizontal_wrapped(|ui| {
                    for category in &FILE_CATEGORIES {
                        if ui.small_button(category.label).clicked() {
                            self.filter_extensions = category.filter_text();
                        }
                    }
                });
                egui::Grid::new("filter_fields")
                    .num_columns(2)
                    .spacing([18.0, 14.0])
                    .show(ui, |ui| {
                        for (label, field, hint) in [
                            ("Minimum size · MiB", &mut self.filter_min, "Any size"),
                            ("Maximum size · MiB", &mut self.filter_max, "No maximum"),
                            ("Modified since · UTC", &mut self.filter_after, "YYYY-MM-DD"),
                            ("Modified through · UTC", &mut self.filter_before, "YYYY-MM-DD"),
                        ] {
                            let label = ui.label(label);
                            ui.add(
                                egui::TextEdit::singleline(field)
                                    .desired_width(190.0)
                                    .hint_text(hint),
                            ).labelled_by(label.id);
                            ui.end_row();
                        }
                    });
                ui.label(
                    RichText::new(
                        "1 MiB = 1,048,576 bytes. Unknown dates cannot match a date condition.",
                    )
                    .small()
                    .color(p.muted),
                );
                ui.add_space(8.0);
                if ui.button("Apply filters").clicked() {
                    match self.parsed_filter_fields() {
                        Some((min, max, after, before)) => {
                            self.remember_context();
                            apply_extensions(&mut self.request.filters, &self.filter_extensions);
                            let f = &mut self.request.filters;
                            f.min_bytes = min;
                            f.max_bytes = max;
                            f.modified_after = after;
                            f.modified_before = before;
                            self.filter_error = None;
                            self.mark_query_changed();
                            self.filter_open = false;
                        }
                        None => {
                            self.filter_error = Some(
                                "Check filter values: use positive MiB, valid YYYY-MM-DD dates, and a minimum no greater than the maximum.",
                            );
                        }
                    }
                }
                if let Some(error) = self.filter_error {
                    ui.colored_label(p.accent, error);
                }
            });
        self.filter_open &= open;
        if !self.filter_open {
            self.filter_error = None;
            restore_dialog_focus(ctx, "Filter files");
        }
    }
    /// The size and date fields as filter bounds, or `None` when any field is
    /// invalid or a minimum exceeds its maximum.
    fn parsed_filter_fields(&self) -> Option<FilterBounds> {
        let min = parse_mib(&self.filter_min).ok()?;
        let max = parse_mib(&self.filter_max).ok()?;
        let after = parse_date(&self.filter_after, false).ok()?;
        let before = parse_date(&self.filter_before, true).ok()?;
        let ordered =
            |low: Option<u64>, high: Option<u64>| low.zip(high).is_none_or(|(a, b)| a <= b);
        (ordered(min, max) && ordered(after, before)).then_some((min, max, after, before))
    }
    pub(super) fn pin(&mut self, id: NodeId) {
        if toggle_pin(&mut self.request.pinned, id) {
            self.dirty = true;
        } else {
            self.notice = Some(format!(
                "Compare up to {MAX_PINS} objects. Unpin one to add another."
            ));
        }
    }
    pub(super) fn comparison_window(&mut self, ctx: &egui::Context, p: Palette) {
        if !self.comparison_open {
            return;
        }
        remember_dialog_focus(ctx, "Pinned comparison");
        let mut open = true;
        egui::Window::new("Pinned comparison")
            .open(&mut open)
            .default_width(650.0)
            .show(ctx, |ui| self.comparison_tray(ui, p));
        self.comparison_open = open;
        if !open {
            restore_dialog_focus(ctx, "Pinned comparison");
        }
    }
    pub(super) fn comparison_tray(&mut self, ui: &mut egui::Ui, p: Palette) {
        let snapshot = self.visible.clone();
        let rows = &snapshot.view.pinned_rows;
        if self.request.pinned.is_empty() {
            return;
        }
        ux::card(p).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Pinned comparison · {}/{MAX_PINS}",
                        self.request.pinned.len()
                    ))
                    .strong(),
                );
                if ui.small_button("Clear pins").clicked() {
                    self.request.pinned.clear();
                    self.dirty = true;
                }
            });
            let max = rows.iter().map(|r| r.logical).max().unwrap_or(1);
            for row in rows {
                ui.horizontal_wrapped(|ui| {
                    let unpin = ui.small_button("×").on_hover_text("Unpin");
                    unpin.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), format!("Unpin {}", row.name)));
                    if unpin.clicked() {
                        self.pin(row.id);
                    }
                    if ui.add(egui::Button::new(&row.name).frame(false)).clicked() {
                        self.action(Action::Select(row.id));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let figures = comparison_figures(row.logical, row.allocated);
                        ui.label(figures.summary).on_hover_text(figures.exact);
                    });
                });
                ux::size_bar(ui, share(row.logical, max), p.accent, p);
            }
            ui.label(
                RichText::new(if snapshot.view.pinned_overlap {
                    "Overlapping folders selected. Individual values overlap; do not add them."
                } else {
                    "Individual logical sizes · allocation may include shared hardlinks. These are not reclaimable-space totals."
                })
                .small()
                .color(p.muted),
            );
        });
    }
    pub(super) fn breakdown(&mut self, ui: &mut egui::Ui, p: Palette) {
        let snapshot = self.visible.clone();
        let view = &snapshot.view;
        overline(ui, "WHY THIS FOLDER IS LARGE", p);
        let max = view.breakdown.iter().map(|r| r.logical).max().unwrap_or(1);
        for item in &view.breakdown {
            ui.horizontal_wrapped(|ui| {
                if let Some(id) = item.id {
                    if ui.add(egui::Button::new(&item.name).frame(false)).clicked() {
                        self.action(if item.is_dir {
                            Action::Navigate(id)
                        } else {
                            Action::Select(id)
                        });
                    }
                } else {
                    ui.label(&item.name);
                }
                ui.label(RichText::new(format_bytes(item.logical)).color(p.muted));
            });
            ux::size_bar(ui, share(item.logical, max), p.accent, p);
        }
        ui.add_space(10.0);
    }
    pub(super) fn open_palette(&mut self) {
        self.palette_open = true;
        self.palette_focus = true;
        self.palette_query.clear();
        self.palette_index = 0;
    }
    fn palette_facts(&self) -> PaletteFacts {
        let selected = self.request.selected;
        PaletteFacts {
            scan_loaded: self.generation != 0,
            scan_state: self.scan_state(),
            can_back: self.history.can_back(),
            can_forward: self.history.can_forward(),
            has_selection: selected.is_some(),
            selection_pinned: selected.is_some_and(|id| self.request.pinned.contains(&id)),
            pins_full: self.request.pinned.len() >= MAX_PINS,
            history_saved: self.persistence_allowed(),
            on_changes_page: self.changes,
        }
    }
    fn run_command(&mut self, command: Command, ctx: &egui::Context) {
        // The palette shows an unavailable command as disabled. Refusing here
        // as well keeps Enter, and any other caller, from running one.
        if command.unavailable(&self.palette_facts()).is_some() {
            return;
        }
        self.palette_open = false;
        restore_dialog_focus(ctx, "Commands");
        match command {
            Command::Overview => self.destination(ViewMode::Overview),
            Command::Explorer => self.destination(ViewMode::Explorer),
            Command::Largest => self.destination(ViewMode::Largest),
            Command::Types => self.destination(ViewMode::Types),
            Command::Changes => self.show_changes(),
            Command::Back => self.action(Action::Back),
            Command::Forward => self.action(Action::Forward),
            Command::Root => self.action(Action::Navigate(0)),
            Command::Stop => {
                self.cancellation
                    .request(self.generation, self.snapshot.progress.state);
                self.controller.cancel();
                ctx.request_repaint();
            }
            Command::Pin => {
                if let Some(id) = self.request.selected {
                    self.pin(id);
                }
            }
            Command::Filters => {
                self.sync_filter_fields();
                self.filter_open = true;
            }
            Command::Theme => {
                self.preferences.theme = self.preferences.theme.next();
                self.apply_appearance(ctx);
            }
        }
    }
    pub(super) fn keyboard(&mut self, ctx: &egui::Context) {
        if !self.transient_dialog_open() {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::K)) {
                self.open_palette();
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft)) {
                self.action(Action::Back);
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowRight)) {
                self.action(Action::Forward);
            }
            if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F1)) {
                self.help_open = true;
            }
        }
        let dialogs = self.transient_dialogs();
        // Let egui dismiss an open menu/ComboBox before its owning window.
        if !egui::Popup::is_any_open(ctx)
            && dialogs.iter().any(|(_, open)| *open)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            let foremost = ctx
                .memory(|m| {
                    m.layer_ids()
                        .filter_map(|layer| {
                            dialogs.iter().position(|(title, open)| {
                                *open
                                    && layer
                                        == egui::LayerId::new(
                                            egui::Order::Middle,
                                            egui::Id::new(title),
                                        )
                            })
                        })
                        .last()
                })
                .or_else(|| dialogs.iter().rposition(|(_, open)| *open));
            if let Some(index) = foremost {
                match index {
                    0 => self.show_errors = false,
                    1 => {
                        self.filter_open = false;
                        self.filter_error = None;
                    }
                    2 => self.palette_open = false,
                    3 => self.settings_open = false,
                    4 => self.help_open = false,
                    5 => self.comparison_open = false,
                    6 => self.export_open = false,
                    _ => unreachable!(),
                }
                restore_dialog_focus(ctx, dialogs[index].0);
            }
        }
    }
    pub(super) fn command_palette(&mut self, ctx: &egui::Context, p: Palette) {
        if !self.palette_open {
            return;
        }
        remember_dialog_focus(ctx, "Commands");
        let mut open = true;
        let mut chosen = None;
        egui::Window::new("Commands")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 100.0])
            .default_width(480.0)
            .show(ctx, |ui| {
                let label = ui.label("Find a command");
                let response = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.palette_query)
                            .hint_text("Where would you like to go?")
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(label.id);
                if self.palette_focus {
                    response.request_focus();
                    self.palette_focus = false;
                }
                if response.changed() {
                    self.palette_index = 0;
                }
                let commands = command_matches(&self.palette_query);
                let facts = self.palette_facts();
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    self.palette_index =
                        (self.palette_index + 1).min(commands.len().saturating_sub(1));
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    self.palette_index = self.palette_index.saturating_sub(1);
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    // Enter on a command that cannot run does nothing, and the
                    // palette stays open so its reason stays readable.
                    chosen = commands
                        .get(self.palette_index)
                        .copied()
                        .filter(|command| command.unavailable(&facts).is_none());
                }
                for (index, command) in commands.iter().enumerate() {
                    let reason = command.unavailable(&facts);
                    let mut button = egui::Button::new(command.label())
                        .selected(index == self.palette_index)
                        .fill(if index == self.palette_index {
                            p.wash
                        } else {
                            p.surface
                        });
                    if let Some(reason) = reason {
                        button = button.shortcut_text(RichText::new(reason).small().color(p.muted));
                    }
                    let response = ui
                        .add_enabled_ui(reason.is_none(), |ui| {
                            ui.add_sized([ui.available_width(), 32.0], button)
                        })
                        .inner;
                    if response.clicked() {
                        chosen = Some(*command);
                    }
                }
                if commands.is_empty() {
                    ui.label("No matching command");
                }
                ui.label(
                    RichText::new("↑ ↓ to choose · Enter to run · Escape to close")
                        .small()
                        .color(p.muted),
                );
            });
        self.palette_open &= open;
        if !self.palette_open {
            restore_dialog_focus(ctx, "Commands");
        }
        if let Some(command) = chosen {
            self.run_command(command, ctx);
        }
    }
    pub(super) fn settings_window(&mut self, ctx: &egui::Context, p: Palette) {
        if !self.settings_open {
            return;
        }
        remember_dialog_focus(ctx, "Appearance & settings");
        let mut open = true;
        let previous = (
            self.preferences.theme,
            self.preferences.text_scale,
            self.preferences.reduced_motion,
        );
        egui::Window::new("Appearance & settings")
            .open(&mut open)
            .collapsible(false)
            .default_width(370.0)
            .show(ctx, |ui| {
                overline(ui, "COLOR THEME", p);
                ui.horizontal_wrapped(|ui| {
                    for theme in Theme::ALL {
                        ui.selectable_value(&mut self.preferences.theme, theme, theme.label());
                    }
                });
                ui.add_space(12.0);
                overline(ui, "TEXT & COMFORT", p);
                ui.horizontal_wrapped(|ui| {
                    for scale in TextScale::ALL {
                        ui.selectable_value(&mut self.preferences.text_scale, scale, scale.label());
                    }
                });
                ui.checkbox(&mut self.preferences.reduced_motion, "Reduced motion");
                ui.checkbox(&mut self.preferences.map_focus, "Give the map more room");
                ui.add_space(12.0);
                overline(ui, "SCANNING", p);
                ui.add_enabled(!self.session_policy.helpdesk, egui::Checkbox::new(&mut self.preferences.ntfs_preview, "NTFS preview acceleration"))
                    .on_hover_text(
                        "Experimental. Unsupported cases fall back to the normal folder scan.",
                    );
                ui.add_space(12.0);
                overline(ui, "WINDOWS ACCESS", p);
                self.access_controls(ui, p);
                ui.label(
                    RichText::new(self.persistence_message())
                        .small()
                        .color(p.muted),
                );
                if let Some(note) = self.access_mode.settings_note() {
                    ui.label(RichText::new(note).small().color(p.muted));
                }
                ui.label(self.session_policy.label());
                if self.persistence_allowed() {
                    ui.label(format!("History location: {}", self.history_directory.display()));
                    ui.label("For a session without saved metadata, close this window and launch with --no-history. For helpdesk diagnostics, launch with --helpdesk.");
                }
                if self.session_policy != SessionPolicy::default() {
                    ui.label("For elevated diagnostics, launch as administrator with the same session switch. In-app relaunch is disabled to preserve restrictions.");
                }
                ui.label(
                    RichText::new(
                        "Scans read metadata. Recycle Bin and permanent deletion are separate, confirmed single-file actions.",
                    )
                    .small()
                    .color(p.muted),
                );
            });
        self.settings_open = open;
        if !open {
            restore_dialog_focus(ctx, "Appearance & settings");
        }
        if previous
            != (
                self.preferences.theme,
                self.preferences.text_scale,
                self.preferences.reduced_motion,
            )
        {
            self.apply_appearance(ctx);
        }
    }
    pub(super) fn help_window(&mut self, ctx: &egui::Context, p: Palette) {
        if !self.help_open {
            return;
        }
        remember_dialog_focus(ctx, "Keyboard help");
        egui::Window::new("Keyboard help")
            .open(&mut self.help_open)
            .collapsible(false)
            .show(ctx, |ui| {
                for (key, action) in [
                    ("Ctrl K", "Open commands"),
                    ("↑ / ↓, Enter", "Choose and run a command"),
                    ("Alt ← / Alt →", "Back / Forward with your context"),
                    ("Tab / Shift Tab", "Move keyboard focus"),
                    ("Space / Enter", "Activate the focused control or map item"),
                    ("Delete", "Check Recycle Bin availability, then open the confirmation for the selected file"),
                    ("Shift Delete", "Open permanent-delete confirmation for the selected file"),
                    ("Escape", "Cancel a confirmation or close the foremost dialog"),
                    ("F1", "Keyboard help"),
                ] {
                    detail(ui, key, action, p);
                }
                ui.label(FILE_ACTION_HELP);
                ui.add_space(10.0);
                ui.label(
                    "Map items have names and logical sizes. The table offers the same objects and paths.",
                );
            });
        if !self.help_open {
            restore_dialog_focus(ctx, "Keyboard help");
        }
    }
}
/// The two texts for one pinned row: sizes in the binary units every other
/// view uses, and the exact byte counts for hover.
struct ComparisonFigures {
    summary: String,
    exact: String,
}

fn comparison_figures(logical: u64, allocated: Option<u64>) -> ComparisonFigures {
    let exact_bytes = |bytes: u64| format!("{} B", format_count(bytes));
    ComparisonFigures {
        summary: format!(
            "{} logical · {} allocated",
            format_bytes(logical),
            allocated.map_or_else(|| "unknown".to_owned(), format_bytes)
        ),
        exact: format!(
            "Logical: {}\nAllocated: {}",
            exact_bytes(logical),
            allocated.map_or_else(|| "unknown".to_owned(), exact_bytes)
        ),
    }
}

/// Explains what the file-action keys do and what they cannot promise.
pub(super) const FILE_ACTION_HELP: &str = "Enter never confirms a file action. Permanent deletion also requires the checkbox and typing DELETE. Move to Recycle Bin is checked first and stops when Windows would clearly delete the file instead, but Windows makes the final decision; if it asks about permanent deletion, choose No.";

/// `(min_bytes, max_bytes, modified_after, modified_before)` as parsed from
/// the filter dialog.
type FilterBounds = (Option<u64>, Option<u64>, Option<u64>, Option<u64>);

fn parse_mib(text: &str) -> Result<Option<u64>, ()> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let n = text.trim().parse::<f64>().map_err(|_| ())?;
    if !n.is_finite() || n < 0.0 || n * 1048576.0 > u64::MAX as f64 {
        Err(())
    } else {
        Ok(Some((n * 1048576.0).round() as u64))
    }
}
fn parse_date(text: &str, end: bool) -> Result<Option<u64>, ()> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let parts: Vec<_> = text.trim().split('-').collect();
    if parts.len() != 3 {
        return Err(());
    }
    let year = parts[0].parse::<i64>().map_err(|_| ())?;
    let month = parts[1].parse::<i64>().map_err(|_| ())?;
    let day = parts[2].parse::<i64>().map_err(|_| ())?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return Err(()),
    };
    if !(1970..=9999).contains(&year) || day < 1 || day > max {
        return Err(());
    }
    let y = year - i64::from(month <= 2);
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    Ok(Some((days * 86400) as u64 + if end { 86399 } else { 0 }))
}
pub(super) fn date_label(seconds: u64) -> String {
    let z = (seconds / 86400) as i64 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}
pub(super) fn date_time_label(seconds: u64) -> String {
    format!(
        "{} {:02}:{:02}:{:02} UTC",
        date_label(seconds),
        seconds % 86400 / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}
#[cfg(test)]
mod tests {
    use super::super::test_support::{rendered_texts, settle_frames};
    use super::*;
    use eframe::App as _;

    fn app_frame(app: &mut DiskScopeApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()),
        );
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn assert_focus_in_dialog(ctx: &egui::Context, title: &str) {
        let focused = ctx
            .memory(|m| m.focused())
            .unwrap_or_else(|| panic!("{title} needs a focused control"));
        let response = ctx
            .read_response(focused)
            .expect("focused control must be rendered");
        assert_eq!(
            response.layer_id,
            egui::LayerId::new(egui::Order::Middle, egui::Id::new(title)),
            "focus escaped {title}"
        );
    }

    #[test]
    fn f1_help_initial_focus_and_tab_stay_inside_help_in_actual_app_frames() {
        let (mut app, ctx) = idle_app();
        for _ in 0..3 {
            app_frame(&mut app, &ctx, vec![]);
        }
        app_frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::F1, egui::Modifiers::NONE)],
        );
        assert!(app.help_open);
        app_frame(&mut app, &ctx, vec![]);
        assert_focus_in_dialog(&ctx, "Keyboard help");
        for modifiers in [egui::Modifiers::NONE, egui::Modifiers::SHIFT] {
            for _ in 0..5 {
                app_frame(&mut app, &ctx, vec![key(egui::Key::Tab, modifiers)]);
                app_frame(&mut app, &ctx, vec![]);
                assert_focus_in_dialog(&ctx, "Keyboard help");
            }
        }
    }

    #[test]
    fn help_close_control_works_with_enter_and_space_and_restores_focus() {
        for key_to_close in [egui::Key::Enter, egui::Key::Space] {
            let (mut app, ctx) = idle_app();
            for _ in 0..3 {
                app_frame(&mut app, &ctx, vec![]);
            }
            app_frame(
                &mut app,
                &ctx,
                vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
            );
            let invoker = ctx.memory(|m| m.focused());
            app.help_open = true;
            for _ in 0..3 {
                app_frame(&mut app, &ctx, vec![]);
            }
            assert_focus_in_dialog(&ctx, "Keyboard help");
            app_frame(
                &mut app,
                &ctx,
                vec![key(key_to_close, egui::Modifiers::NONE)],
            );
            app_frame(&mut app, &ctx, vec![]);
            assert!(
                !app.help_open,
                "{key_to_close:?} should activate the focused help close control"
            );
            assert_eq!(ctx.memory(|m| m.focused()), invoker);
        }
    }

    #[test]
    fn transient_dialogs_contain_tab_focus_and_escape_restores_real_invoker() {
        for title in [
            "Filter files",
            "Commands",
            "Appearance & settings",
            "Pinned comparison",
            "Export investigation evidence",
            "Scan errors & skipped locations",
        ] {
            let (mut app, ctx) = idle_app();
            for _ in 0..3 {
                app_frame(&mut app, &ctx, vec![]);
            }
            app_frame(
                &mut app,
                &ctx,
                vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
            );
            let invoker = ctx
                .memory(|m| m.focused())
                .expect("background control has focus");
            match title {
                "Filter files" => app.filter_open = true,
                "Commands" => app.open_palette(),
                "Appearance & settings" => app.settings_open = true,
                "Pinned comparison" => app.comparison_open = true,
                "Export investigation evidence" => app.export_open = true,
                _ => app.show_errors = true,
            }
            for _ in 0..3 {
                app_frame(&mut app, &ctx, vec![]);
            }
            assert_focus_in_dialog(&ctx, title);
            for modifiers in [egui::Modifiers::NONE, egui::Modifiers::SHIFT] {
                for _ in 0..35 {
                    app_frame(&mut app, &ctx, vec![key(egui::Key::Tab, modifiers)]);
                    app_frame(&mut app, &ctx, vec![]);
                    assert_focus_in_dialog(&ctx, title);
                }
            }
            app_frame(&mut app, &ctx, vec![escape_key()]);
            app_frame(&mut app, &ctx, vec![]);
            assert_eq!(
                ctx.memory(|m| m.focused()),
                Some(invoker),
                "{title} must restore its invoker"
            );
        }
    }

    #[test]
    fn dialog_opening_frame_does_not_activate_the_background_with_enter() {
        let (mut app, ctx) = idle_app();
        for _ in 0..3 {
            app_frame(&mut app, &ctx, vec![]);
        }
        app_frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
        );
        let invoker = ctx.memory(|m| m.focused()).unwrap();
        app.destination(ViewMode::Largest);
        app.help_open = true;
        app_frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(
            app.request.mode,
            ViewMode::Largest,
            "Enter must not activate the underlying Overview control"
        );
        app_frame(&mut app, &ctx, vec![]);
        app_frame(&mut app, &ctx, vec![escape_key()]);
        app_frame(&mut app, &ctx, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(invoker));
        app_frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(
            app.request.mode,
            ViewMode::Overview,
            "restored background control must work again"
        );
    }

    #[test]
    fn dialog_blocks_background_shortcuts_and_restores_nested_dialog_focus() {
        let (mut app, ctx) = idle_app();
        app.destination(ViewMode::Largest);
        app.filter_open = true;
        for _ in 0..3 {
            app_frame(&mut app, &ctx, vec![]);
        }
        let filter_focus = ctx.memory(|m| m.focused());
        app_frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::K, egui::Modifiers::CTRL)],
        );
        assert!(
            !app.palette_open,
            "Ctrl K must not replace a dialog's focus"
        );
        app_frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::ArrowLeft, egui::Modifiers::ALT)],
        );
        assert_eq!(
            app.request.mode,
            ViewMode::Largest,
            "Alt Left must not navigate the background"
        );
        app_frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::F1, egui::Modifiers::NONE)],
        );
        assert!(
            !app.help_open,
            "F1 must not stack an unrequested dialog while editing"
        );
        // A deliberately opened second dialog restores the control in the
        // first, then a second Escape closes that first dialog.
        app.help_open = true;
        for _ in 0..3 {
            app_frame(&mut app, &ctx, vec![]);
        }
        assert_focus_in_dialog(&ctx, "Keyboard help");
        app_frame(&mut app, &ctx, vec![escape_key()]);
        app_frame(&mut app, &ctx, vec![]);
        assert!(!app.help_open && app.filter_open);
        assert_eq!(ctx.memory(|m| m.focused()), filter_focus);
        app_frame(&mut app, &ctx, vec![escape_key()]);
        assert!(!app.filter_open);
    }

    fn escape_key() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn running_a_palette_command_restores_its_invoking_control() {
        let (mut app, ctx) = idle_app();
        let invoker = egui::Id::new("command_button");
        set_dialog_invoker(&ctx, "Commands", invoker);
        app.open_palette();
        app.palette_query = "theme".into();
        palette_frame(&mut app, &ctx, vec![]);
        palette_frame(&mut app, &ctx, vec![enter_key()]);
        assert!(!app.palette_open);
        assert_eq!(ctx.memory(|m| m.focused()), Some(invoker));
    }

    #[test]
    fn escape_defers_to_an_open_popup_before_closing_its_dialog() {
        let (mut app, ctx) = idle_app();
        app.filter_open = true;
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![escape_key()],
                ..Default::default()
            },
            |_| {
                egui::Popup::open_id(&ctx, egui::Id::new("test_combo_popup"));
                app.keyboard(&ctx);
                assert!(
                    app.filter_open,
                    "the popup must dismiss before its owning window"
                );
                assert!(
                    ctx.input(|i| i.key_pressed(egui::Key::Escape)),
                    "the popup still needs its Escape event"
                );
            },
        );
    }

    #[test]
    fn filter_inputs_expose_their_visible_names_in_the_accessibility_tree() {
        let (mut app, ctx) = idle_app();
        ctx.enable_accesskit();
        app.filter_open = true;
        let output = settle_frames(&ctx, |ui| {
            app.filter_window(ui.ctx(), app.preferences.theme.palette())
        });
        let update = output.platform_output.accesskit_update.unwrap();
        let mut names = Vec::new();
        for (_, node) in update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == egui::accesskit::Role::TextInput)
        {
            let labels: Vec<_> = node
                .labelled_by()
                .iter()
                .filter_map(|label| {
                    update
                        .nodes
                        .iter()
                        .find(|(id, _)| id == label)
                        .and_then(|(_, n)| n.value())
                })
                .collect();
            names.extend(labels);
        }
        for name in [
            "File extensions (comma separated)",
            "Minimum size · MiB",
            "Maximum size · MiB",
            "Modified since · UTC",
            "Modified through · UTC",
        ] {
            assert!(
                names.contains(&name),
                "missing filter input name {name}: {names:?}"
            );
        }
    }

    #[test]
    fn escape_closes_comparison_and_errors_one_actual_frontmost_window_at_a_time() {
        let (mut app, ctx) = idle_app();
        app.comparison_open = true;
        app.show_errors = true;
        settle_frames(&ctx, |ui| {
            app.error_window(ui.ctx(), app.preferences.theme.palette());
            app.comparison_window(ui.ctx(), app.preferences.theme.palette());
        });
        // Bring the errors window above comparison, regardless of render order.
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {
            ctx.move_to_top(egui::LayerId::new(
                egui::Order::Middle,
                egui::Id::new("Scan errors & skipped locations"),
            ));
        });
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![escape_key()],
                ..Default::default()
            },
            |_| {
                app.keyboard(&ctx);
            },
        );
        assert!(!app.show_errors);
        assert!(app.comparison_open);
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![escape_key()],
                ..Default::default()
            },
            |_| {
                app.keyboard(&ctx);
            },
        );
        assert!(!app.comparison_open);
    }

    #[test]
    fn escape_closes_only_the_frontmost_of_multiple_open_dialogs() {
        let (mut app, ctx) = idle_app();
        app.filter_open = true;
        app.help_open = true;
        settle_frames(&ctx, |ui| {
            app.filter_window(ui.ctx(), app.preferences.theme.palette());
            app.help_window(ui.ctx(), app.preferences.theme.palette());
        });
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![escape_key()],
                ..Default::default()
            },
            |_| {
                app.keyboard(&ctx);
            },
        );
        assert!(!app.help_open);
        assert!(app.filter_open, "the window behind help must stay open");
    }

    #[test]
    fn dates_and_size_reject_invalid_filters() {
        assert_eq!(parse_date("1970-01-01", false), Ok(Some(0)));
        assert_eq!(
            parse_date("2024-02-29", true).map(|n| date_label(n.unwrap())),
            Ok("2024-02-29".into())
        );
        assert!(parse_date("2023-02-29", false).is_err());
        assert!(parse_mib("NaN").is_err());
        assert_eq!(parse_mib("1.5"), Ok(Some(1572864)));
    }

    /// An app with no scan and no history, as at startup.
    fn idle_app() -> (DiskScopeApp, egui::Context) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app =
            DiskScopeApp::with_history_directory(&cc, None, PathBuf::new(), AccessMode::Admin);
        (app, ctx)
    }

    fn palette_texts(app: &mut DiskScopeApp, ctx: &egui::Context) -> Vec<String> {
        let output = settle_frames(ctx, |ui| {
            app.command_palette(ui.ctx(), app.preferences.theme.palette())
        });
        rendered_texts(&output.shapes)
    }

    fn enter_key() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// One palette frame with `events`, for keyboard tests.
    fn palette_frame(app: &mut DiskScopeApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ui| app.command_palette(ui.ctx(), app.preferences.theme.palette()),
        );
    }

    #[test]
    fn palette_shows_why_a_command_cannot_run_and_nothing_for_the_others() {
        let (mut app, ctx) = idle_app();
        app.open_palette();
        let texts = palette_texts(&mut app, &ctx);
        for reason in [
            "No scan is running.",
            "Nothing earlier to go back to.",
            "Nothing later to go forward to.",
            "Scan a folder first.",
            "Select a folder or file first.",
            "Scan history is not saved in this window.",
        ] {
            assert!(
                texts.iter().any(|text| text == reason),
                "{reason}: {texts:?}"
            );
        }
        // Commands that always work carry no reason.
        for label in ["Overview", "Edit filters", "Switch theme"] {
            assert!(texts.iter().any(|text| text == label), "{label}");
        }
        assert!(!texts.iter().any(|text| text.contains("Already")));
    }

    #[test]
    fn enter_on_an_unavailable_command_runs_nothing_and_keeps_the_palette_open() {
        let (mut app, ctx) = idle_app();
        app.open_palette();
        app.palette_query = "back".into();
        palette_frame(&mut app, &ctx, vec![]);
        palette_frame(&mut app, &ctx, vec![enter_key()]);
        assert!(app.palette_open, "the reason must stay readable");
        assert!(!app.history.can_back() && !app.history.can_forward());
        assert_eq!(app.request.mode, ViewMode::Overview);
        // Negative control: the same key runs an available command.
        let theme = app.preferences.theme;
        app.palette_query = "theme".into();
        app.palette_index = 0;
        palette_frame(&mut app, &ctx, vec![]);
        palette_frame(&mut app, &ctx, vec![enter_key()]);
        assert!(!app.palette_open);
        assert_ne!(app.preferences.theme, theme);
    }

    #[test]
    fn stop_needs_a_running_scan_and_flags_an_acknowledgement_only_then() {
        let (mut app, ctx) = idle_app();
        app.palette_open = true;
        app.run_command(Command::Stop, &ctx);
        assert!(app.palette_open, "an unavailable command is not run");
        assert_eq!(app.cancellation.generation, None);
        // With a running scan the same command goes through.
        app.generation = 3;
        let snapshot = Arc::make_mut(&mut app.snapshot);
        snapshot.progress.generation = 3;
        snapshot.progress.state = ScanState::Scanning;
        app.run_command(Command::Stop, &ctx);
        assert!(!app.palette_open);
        assert_eq!(app.cancellation.generation, Some(3));
        // A second Stop while the first is pending is refused.
        app.palette_open = true;
        app.run_command(Command::Stop, &ctx);
        assert!(app.palette_open);
    }

    #[test]
    fn pin_needs_a_selection_and_honours_the_comparison_limit() {
        let (mut app, ctx) = idle_app();
        app.run_command(Command::Pin, &ctx);
        assert!(app.request.pinned.is_empty());
        app.request.selected = Some(5);
        app.run_command(Command::Pin, &ctx);
        assert_eq!(app.request.pinned, [5]);
        // Full comparison: another selection cannot be pinned, but the
        // pinned one can still be unpinned.
        app.request.pinned = (1..=MAX_PINS as NodeId).collect();
        app.request.selected = Some(9);
        assert!(Command::Pin.unavailable(&app.palette_facts()).is_some());
        app.request.selected = Some(2);
        assert!(Command::Pin.unavailable(&app.palette_facts()).is_none());
    }

    #[test]
    fn comparison_tray_uses_binary_units_and_keeps_exact_bytes_for_hover() {
        let figures = comparison_figures(1_234_567, Some(4096));
        assert_eq!(figures.summary, "1.2 MiB logical · 4.0 KiB allocated");
        assert_eq!(figures.exact, "Logical: 1,234,567 B\nAllocated: 4,096 B");
        let unknown = comparison_figures(512, None);
        assert_eq!(unknown.summary, "512 B logical · unknown allocated");
        assert_eq!(unknown.exact, "Logical: 512 B\nAllocated: unknown");
    }

    #[test]
    fn rendered_tray_shows_binary_units_instead_of_raw_byte_counts() {
        let (mut app, ctx) = idle_app();
        app.request.pinned = vec![1];
        let mut snapshot = UiSnapshot::default();
        snapshot.view.pinned_rows = vec![Row {
            id: 1,
            parent: Some(0),
            depth: 0,
            name: "archive.iso".into(),
            path: PathBuf::from("/scan/archive.iso"),
            is_dir: false,
            reparse: false,
            identity: None,
            hard_link: false,
            logical: 1_234_567,
            allocated: Some(4096),
            files: 1,
            folders: 0,
            modified: None,
            has_children: false,
        }];
        app.visible = Arc::new(snapshot);
        let output = settle_frames(&ctx, |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                app.comparison_tray(ui, app.preferences.theme.palette());
            });
        });
        let texts = rendered_texts(&output.shapes);
        assert!(
            texts
                .iter()
                .any(|text| text == "1.2 MiB logical · 4.0 KiB allocated"),
            "{texts:?}"
        );
        assert!(
            !texts.iter().any(|text| text.contains("1,234,567")),
            "raw byte counts belong in the hover text only: {texts:?}"
        );
    }
}
