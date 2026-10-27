use super::*;

impl DiskScopeApp {
    pub(super) fn filter_window(&mut self, ctx: &egui::Context, p: Palette) {
        if !self.filter_open {
            return;
        }
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
                ui.label("File extensions (comma separated)");
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter_extensions)
                        .hint_text(".mp4, .mkv")
                        .char_limit(1024)
                        .desired_width(f32::INFINITY),
                );
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
                            ui.label(label);
                            ui.add(
                                egui::TextEdit::singleline(field)
                                    .desired_width(190.0)
                                    .hint_text(hint),
                            );
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
        let mut open = true;
        egui::Window::new("Pinned comparison")
            .open(&mut open)
            .default_width(650.0)
            .show(ctx, |ui| self.comparison_tray(ui, p));
        self.comparison_open = open;
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
                    if ui.small_button("×").on_hover_text("Unpin").clicked() {
                        self.pin(row.id);
                    }
                    if ui.add(egui::Button::new(&row.name).frame(false)).clicked() {
                        self.action(Action::Select(row.id));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let allocated = row
                            .allocated
                            .map(|n| format!("{} B", format_count(n)))
                            .unwrap_or_else(|| "unknown".into());
                        ui.label(format!(
                            "{} B logical · {allocated} allocated",
                            format_count(row.logical)
                        ));
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
    fn run_command(&mut self, command: Command, ctx: &egui::Context) {
        self.palette_open = false;
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
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.palette_open = false;
            self.filter_open = false;
            self.help_open = false;
            self.settings_open = false;
        }
    }
    pub(super) fn command_palette(&mut self, ctx: &egui::Context, p: Palette) {
        if !self.palette_open {
            return;
        }
        let mut open = true;
        let mut chosen = None;
        egui::Window::new("Commands")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 100.0])
            .default_width(480.0)
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.palette_query)
                        .hint_text("Where would you like to go?")
                        .desired_width(f32::INFINITY),
                );
                if self.palette_focus {
                    response.request_focus();
                    self.palette_focus = false;
                }
                if response.changed() {
                    self.palette_index = 0;
                }
                let commands = command_matches(&self.palette_query);
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    self.palette_index =
                        (self.palette_index + 1).min(commands.len().saturating_sub(1));
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    self.palette_index = self.palette_index.saturating_sub(1);
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    chosen = commands.get(self.palette_index).copied();
                }
                for (index, command) in commands.iter().enumerate() {
                    if ui
                        .add_sized(
                            [ui.available_width(), 32.0],
                            egui::Button::new(command.label()).fill(
                                if index == self.palette_index {
                                    p.wash
                                } else {
                                    p.surface
                                },
                            ),
                        )
                        .clicked()
                    {
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
        if let Some(command) = chosen {
            self.run_command(command, ctx);
        }
    }
    pub(super) fn settings_window(&mut self, ctx: &egui::Context, p: Palette) {
        if !self.settings_open {
            return;
        }
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
                ui.checkbox(&mut self.preferences.ntfs_preview, "NTFS preview acceleration")
                    .on_hover_text(
                        "Experimental. Unsupported cases fall back to the normal folder scan.",
                    );
                ui.add_space(12.0);
                overline(ui, "WINDOWS ACCESS", p);
                self.access_controls(ui, p);
                ui.label(
                    RichText::new(self.access_mode.history_message())
                        .small()
                        .color(p.muted),
                );
                ui.label(
                    RichText::new(
                        "Scans read metadata. Recycle Bin and permanent deletion are separate, confirmed single-file actions.",
                    )
                    .small()
                    .color(p.muted),
                );
            });
        self.settings_open = open;
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
                    ("Delete", "Open Recycle Bin confirmation for the selected file"),
                    ("Shift Delete", "Open permanent-delete confirmation for the selected file"),
                    ("Escape", "Cancel a confirmation or close a dialog"),
                    ("F1", "Keyboard help"),
                ] {
                    detail(ui, key, action, p);
                }
                ui.label(
                    "Enter never confirms a file action. Permanent deletion also requires the checkbox and typing DELETE.",
                );
                ui.add_space(10.0);
                ui.label(
                    "Map items have names and logical sizes. The table offers the same objects and paths.",
                );
            });
    }
}
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
    use super::*;
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
}
