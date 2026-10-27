mod changes;
mod controls;
mod management;
mod native;
mod state;
mod ux;

use crate::theme::{Palette, TextScale, Theme, load_system_font};
use controls::{date_label, date_time_label};
use diskscope::{
    engine::Controller,
    history::{self as scan_history, HistoryService, HistorySnapshot},
    model::*,
};
use eframe::egui::{self, Align, Color32, FontId, Layout, Rect, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};
pub(crate) use management::AccessMode;
#[cfg(windows)]
pub(crate) use native::windows_directory;
use serde::{Deserialize, Serialize};
use state::{
    BrowseContext, BrowseHistory, Command, MAX_PINS, apply_extensions, apply_type_lens,
    command_matches, comparison_matches, focus_change_request, hold_view, overview_type_share,
    previous_pair, restore_browse_context, toggle_pin, transitioned,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

const PAGE_SIZE: usize = 256;
const PREFERENCES_KEY: &str = "diskscope.preferences.v1";
/// Breadcrumb trail length; deeper trails keep the current folder and hide the
/// oldest ancestors behind an ellipsis.
const MAX_BREADCRUMBS: usize = 12;
/// Search text is re-queried this long after the last keystroke.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(180);
#[derive(Default)]
struct CancellationAcknowledgement {
    generation: Option<u64>,
}
impl CancellationAcknowledgement {
    fn request(&mut self, generation: u64, published: ScanState) {
        if published.is_active() {
            self.generation = Some(generation);
        }
    }
    fn accept(&mut self, generation: u64, published: ScanState) {
        if self.generation == Some(generation) && !published.is_active() {
            self.reset();
        }
    }
    fn reset(&mut self) {
        self.generation = None;
    }
    fn state(&self, generation: u64, published: ScanState) -> ScanState {
        if self.generation == Some(generation) && published.is_active() {
            ScanState::Cancelling
        } else {
            published
        }
    }
}
fn folder_allocation_text(snapshot: &UiSnapshot) -> String {
    snapshot
        .view
        .unique_allocated
        .map(format_bytes)
        .unwrap_or_else(|| "unknown".into())
}
fn progress_explanation(progress: &Progress) -> &str {
    if !progress.message.is_empty() {
        &progress.message
    } else if progress.state.is_active() {
        "Reading folders in the background"
    } else if progress.state == ScanState::Idle {
        "Choose a folder to see what uses its space"
    } else {
        ""
    }
}
fn row_share(bytes: u64, view: &ViewSnapshot) -> f32 {
    share(bytes, view.row_scope_logical)
}
/// Stop wording shared by the compact and the metric header layouts.
fn stop_status(state: ScanState) -> Option<&'static str> {
    match state {
        ScanState::Cancelling => Some("Stop requested · keeping partial results"),
        ScanState::Cancelled => Some("Stopped · partial results"),
        _ => None,
    }
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    theme: Theme,
    map_focus: bool,
    ntfs_preview: bool,
    stable_updates: bool,
    text_scale: TextScale,
    reduced_motion: bool,
}
enum NativeResult {
    Folder(Option<PathBuf>),
    Reveal(Result<(), String>),
    Volume {
        generation: u64,
        path: PathBuf,
        volume: Option<native::VolumeSpace>,
    },
}
enum Action {
    Select(NodeId),
    Toggle(NodeId),
    Navigate(NodeId),
    Back,
    Forward,
    Up,
    Page(usize),
}

pub struct DiskScopeApp {
    access_mode: AccessMode,
    management: management::Management,
    controller: Controller,
    snapshot: Arc<UiSnapshot>,
    visible: Arc<UiSnapshot>,
    visible_offset: usize,
    generation: u64,
    cancellation: CancellationAcknowledgement,
    request: ViewRequest,
    dirty: bool,
    search_changed: Option<Instant>,
    volume: Option<native::VolumeSpace>,
    volume_tx: SyncSender<(u64, PathBuf)>,
    history: BrowseHistory,
    pending: Option<Arc<UiSnapshot>>,
    history_service: HistoryService,
    history_snapshot: Arc<HistorySnapshot>,
    history_directory: PathBuf,
    older_scan: Option<u64>,
    newer_scan: Option<u64>,
    changes: bool,
    filter_open: bool,
    /// Validation message shown inside the filter dialog until it closes.
    filter_error: Option<&'static str>,
    filter_extensions: String,
    filter_min: String,
    filter_max: String,
    filter_after: String,
    filter_before: String,
    palette_open: bool,
    palette_query: String,
    palette_index: usize,
    palette_focus: bool,
    help_open: bool,
    settings_open: bool,
    comparison_open: bool,
    root_path: PathBuf,
    path_text: String,
    path_edited: bool,
    preferences: Preferences,
    native_tx: SyncSender<NativeResult>,
    native_rx: Receiver<NativeResult>,
    picker_open: bool,
    show_errors: bool,
    notice: Option<String>,
}
impl DiskScopeApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial_path: Option<PathBuf>,
        access_mode: AccessMode,
    ) -> Self {
        Self::with_history_directory(
            cc,
            initial_path,
            scan_history::default_directory(),
            access_mode,
        )
    }
    fn with_history_directory(
        cc: &eframe::CreationContext<'_>,
        initial_path: Option<PathBuf>,
        history_directory: PathBuf,
        access_mode: AccessMode,
    ) -> Self {
        let preferences = if access_mode.persistence_allowed() {
            cc.storage
                .and_then(|s| s.get_string(PREFERENCES_KEY))
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Preferences::default()
        };
        let (native_tx, native_rx) = mpsc::sync_channel(4);
        let snapshot = Arc::new(UiSnapshot::default());
        let volume_tx = native::volume_worker(native_tx.clone());
        let history_service = if access_mode.persistence_allowed() {
            HistoryService::new(history_directory.clone())
        } else {
            HistoryService::unavailable(access_mode.history_message())
        };
        let controller = if access_mode.persistence_allowed() {
            Controller::with_history(history_service.capture())
        } else {
            Controller::new()
        };
        let mut app = Self {
            access_mode,
            management: management::Management::default(),
            controller,
            snapshot: snapshot.clone(),
            visible: snapshot,
            visible_offset: 0,
            generation: 0,
            cancellation: CancellationAcknowledgement::default(),
            request: ViewRequest {
                limit: PAGE_SIZE,
                mode: ViewMode::Overview,
                ..ViewRequest::default()
            },
            dirty: false,
            search_changed: None,
            volume: None,
            volume_tx,
            history: BrowseHistory::default(),
            pending: None,
            history_service,
            history_snapshot: Arc::new(HistorySnapshot::default()),
            history_directory,
            older_scan: None,
            newer_scan: None,
            changes: false,
            filter_open: false,
            filter_error: None,
            filter_extensions: String::new(),
            filter_min: String::new(),
            filter_max: String::new(),
            filter_after: String::new(),
            filter_before: String::new(),
            palette_open: false,
            palette_query: String::new(),
            palette_index: 0,
            palette_focus: false,
            help_open: false,
            settings_open: false,
            comparison_open: false,
            root_path: PathBuf::new(),
            path_text: String::new(),
            path_edited: false,
            preferences,
            native_tx,
            native_rx,
            picker_open: false,
            show_errors: false,
            notice: None,
        };
        load_system_font(&cc.egui_ctx);
        app.apply_appearance(&cc.egui_ctx);
        if let Some(path) = initial_path {
            app.begin_scan(path);
        }
        app
    }
    fn begin_scan(&mut self, path: PathBuf) {
        if self.management.busy() {
            return;
        }
        self.root_path = path;
        self.path_text = self.root_path.to_string_lossy().into_owned();
        self.path_edited = false;
        self.request = ViewRequest {
            limit: PAGE_SIZE,
            mode: ViewMode::Overview,
            ..ViewRequest::default()
        };
        self.history.clear();
        self.pending = None;
        self.changes = false;
        self.notice = None;
        self.generation = self.controller.start(
            self.root_path.clone(),
            if self.preferences.ntfs_preview {
                BackendChoice::Ntfs
            } else {
                BackendChoice::Normal
            },
        );
        self.volume = None;
        let _ = self
            .volume_tx
            .try_send((self.generation, self.root_path.clone()));
        self.cancellation.reset();
        let mut initial = UiSnapshot::default();
        initial.progress.generation = self.generation;
        initial.progress.root = self.root_path.clone();
        initial.progress.state = ScanState::Preparing;
        initial.view.generation = self.generation;
        self.snapshot = Arc::new(initial);
        self.visible = self.snapshot.clone();
        self.visible_offset = 0;
        self.dirty = true;
    }
    fn poll(&mut self, ctx: &egui::Context) {
        self.poll_management(ctx);
        // Both sources are bounded; never drain an unbounded queue in a frame.
        for _ in 0..4 {
            let Some(next) = self.controller.poll() else {
                break;
            };
            if next.progress.generation != self.generation {
                continue;
            }
            self.cancellation
                .accept(next.progress.generation, next.progress.state);
            if admits_snapshot(
                self.generation,
                self.request.revision,
                next.view.generation,
                next.view.revision,
            ) {
                if self.request.focus_path.take().is_some() {
                    self.request.root = next.view.root;
                    self.request.selected = next.view.selected.as_ref().map(|row| row.id);
                }
                if hold_view(
                    self.preferences.stable_updates,
                    self.visible.view.revision,
                    next.view.revision,
                    self.visible.view.selected.as_ref().map(|row| row.id) != self.request.selected,
                ) {
                    self.pending = Some(next.clone());
                } else {
                    self.visible_offset = self.request.offset;
                    self.visible = next.clone();
                    self.pending = None;
                }
            }
            self.snapshot = next;
        }
        for _ in 0..4 {
            let Ok(result) = self.native_rx.try_recv() else {
                break;
            };
            match result {
                NativeResult::Folder(path) => {
                    self.picker_open = false;
                    if let Some(path) = path {
                        self.root_path = path;
                        self.path_text = self.root_path.to_string_lossy().into_owned();
                        self.path_edited = false;
                    }
                }
                NativeResult::Volume {
                    generation,
                    path,
                    volume,
                } => {
                    if generation == self.generation && path == self.root_path {
                        self.volume = volume;
                    }
                }
                NativeResult::Reveal(Err(message)) => self.notice = Some(message),
                NativeResult::Reveal(Ok(())) => {}
            }
        }
        if let Some(history) = self.history_service.poll() {
            self.history_snapshot = history;
        }
        ctx.request_repaint_after(Duration::from_millis(
            if self.snapshot.progress.state.is_active()
                || self.picker_open
                || self.dirty
                || self.search_changed.is_some()
                || self.history_snapshot.busy
                || self.management.busy()
            {
                50
            } else {
                250
            },
        ));
    }
    fn submit_view(&mut self) {
        if !self.dirty || self.generation == 0 {
            return;
        }
        self.request.revision = self.request.revision.wrapping_add(1);
        self.controller.request_view(self.request.clone());
        self.dirty = false;
    }
    fn scan_state(&self) -> ScanState {
        self.cancellation
            .state(self.generation, self.snapshot.progress.state)
    }
    fn mark_query_changed(&mut self) {
        self.request.offset = 0;
        self.search_changed = None;
        self.dirty = true;
    }
    fn action(&mut self, action: Action) {
        if self.management.busy() {
            return;
        }
        match action {
            Action::Select(id) => {
                self.request.selected = Some(id);
                self.dirty = true;
            }
            Action::Toggle(id) => {
                if !self.request.expanded.remove(&id) {
                    if self.request.expanded.len() >= 2048 {
                        self.notice = Some(
                            "Expansion limit reached. Open a folder to explore deeper.".into(),
                        );
                        return;
                    }
                    self.request.expanded.insert(id);
                }
                self.dirty = true;
            }
            Action::Navigate(id) => {
                if id != self.request.root {
                    self.remember_context();
                }
                self.request.root = id;
                self.request.selected = Some(id);
                self.request.mode = ViewMode::Explorer;
                self.changes = false;
                self.request.expanded.clear();
                self.mark_query_changed();
            }
            Action::Back => {
                let current = self.browse_context();
                if let Some(previous) = self.history.back(&current) {
                    self.restore_context(previous);
                }
            }
            Action::Forward => {
                let current = self.browse_context();
                if let Some(next) = self.history.forward(&current) {
                    self.restore_context(next);
                }
            }
            Action::Up => {
                let crumbs = &self.visible.view.breadcrumbs;
                if crumbs.len() >= 2 {
                    self.action(Action::Navigate(crumbs[crumbs.len() - 2].0));
                }
            }
            Action::Page(offset) => {
                self.request.offset = offset;
                self.dirty = true;
            }
        }
    }
    fn choose_folder(&mut self, ctx: &egui::Context) {
        if self.picker_open || self.management.busy() {
            return;
        }
        self.picker_open = true;
        let tx = self.native_tx.clone();
        let ctx = ctx.clone();
        let root = self.root_path.clone();
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title("Choose a folder to analyze");
            if !root.as_os_str().is_empty() {
                dialog = dialog.set_directory(root);
            }
            // A dropped result would leave the Browse button disabled forever;
            // blocking this helper thread until the UI drains is harmless.
            let _ = tx.send(NativeResult::Folder(dialog.pick_folder()));
            ctx.request_repaint();
        });
    }
    fn reveal(&self, path: PathBuf, is_dir: bool, ctx: &egui::Context) {
        let tx = self.native_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = native::reveal_command(&path, is_dir).and_then(|mut command| {
                command
                    .spawn()
                    .map(|_| ())
                    .map_err(|e| format!("Could not open Explorer: {e}"))
            });
            let _ = tx.send(NativeResult::Reveal(result));
            ctx.request_repaint();
        });
    }
    fn header(&mut self, ui: &mut egui::Ui, p: Palette) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(if self.changes {
                    "What changed?"
                } else {
                    match self.request.mode {
                        ViewMode::Overview => "Your storage, understood.",
                        ViewMode::Explorer => "Explore your folders",
                        ViewMode::Largest => "The biggest files",
                        ViewMode::Types => "Space by file type",
                    }
                })
                .size(27.0)
                .strong(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    self.settings_open = true;
                }
                if ui.button("Commands  Ctrl K").clicked() {
                    self.open_palette();
                }
            });
        });
        ui.horizontal_wrapped(|ui| self.access_controls(ui, p));
        if !self.access_mode.persistence_allowed() {
            ui.label(
                RichText::new(self.access_mode.history_message())
                    .small()
                    .color(p.muted),
            );
        }
        ui.add_space(9.0);
        ui.horizontal(|ui| {
            let active = self.scan_state().is_active();
            let edit_width = (ui.available_width() - 235.0).max(120.0);
            let response = ui.add_sized(
                [edit_width, 33.0],
                egui::TextEdit::singleline(&mut self.path_text)
                    .hint_text("Choose a folder to explore…"),
            );
            if response.changed() {
                self.path_edited = true;
            }
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(
                    !self.picker_open,
                    egui::Button::new("Browse…").min_size(egui::vec2(74.0, 33.0)),
                )
                .clicked()
            {
                self.choose_folder(ui.ctx());
            }
            let scan = ui
                .add_enabled(
                    !self.path_text.trim().is_empty(),
                    egui::Button::new(
                        RichText::new(if active { "Rescan" } else { "Scan" })
                            .strong()
                            .color(if self.preferences.theme != Theme::Light {
                                Color32::from_rgb(21, 49, 45)
                            } else {
                                Color32::WHITE
                            }),
                    )
                    .fill(p.accent)
                    .min_size(egui::vec2(63.0, 33.0)),
                )
                .clicked();
            if scan || enter {
                let path = if self.path_edited {
                    PathBuf::from(self.path_text.clone())
                } else {
                    self.root_path.clone()
                };
                if !path.as_os_str().is_empty() {
                    self.begin_scan(path);
                }
            }
            if ui
                .add_enabled(
                    active && self.scan_state() != ScanState::Cancelling,
                    egui::Button::new("Stop").min_size(egui::vec2(65.0, 33.0)),
                )
                .clicked()
            {
                self.cancellation
                    .request(self.generation, self.snapshot.progress.state);
                self.controller.cancel();
                ui.ctx().request_repaint();
            }
        });
        ui.add_space(12.0);
        let progress = &self.snapshot.progress;
        let state = self.scan_state();
        if self.changes || self.request.mode == ViewMode::Overview || !state.is_active() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(stop_status(state).unwrap_or(state.label())).color(p.accent),
                );
                ui.separator();
                ui.label(RichText::new(format_bytes(progress.logical)).strong());
                ui.label(
                    RichText::new(format!(
                        "logical · {} files · {} folders · {} elapsed",
                        format_count(progress.files),
                        format_count(progress.folders),
                        format_duration(progress.elapsed)
                    ))
                    .color(p.muted),
                );
                if ui
                    .small_button(format!("{} errors", format_count(progress.errors_total)))
                    .clicked()
                {
                    self.show_errors = true;
                }
                if let Some(scan) = self
                    .history_snapshot
                    .scans
                    .iter()
                    .find(|scan| scan.live_generation == Some(self.generation))
                {
                    ui.label(
                        RichText::new(format!("Scanned {}", date_time_label(scan.finished_unix)))
                            .small()
                            .color(p.faint),
                    );
                }
            });
            if state.is_active() || matches!(state, ScanState::Partial | ScanState::Failed) {
                ui.label(
                    RichText::new(progress_explanation(progress))
                        .small()
                        .color(p.muted),
                );
            }
            if let Some(notice) = self.notice.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(p.accent, notice);
                    if ui.small_button("Dismiss").clicked() {
                        self.notice = None;
                    }
                });
            }
            ui.add_space(6.0);
            ui.separator();
            return;
        }
        let available = ui.available_width();
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(available * 0.34, 54.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_min_width(available * 0.34);
                    ui.horizontal(|ui| {
                        if state.is_active() && !self.preferences.reduced_motion {
                            ui.add(egui::Spinner::new().size(12.0).color(p.accent));
                        } else {
                            ui.colored_label(p.accent, "●");
                        }
                        ui.label(RichText::new(state.label()).strong());
                    });
                    ui.add(
                        egui::Label::new(
                            RichText::new(progress_explanation(progress))
                                .small()
                                .color(p.muted),
                        )
                        .truncate(),
                    )
                    .on_hover_text(progress_explanation(progress));
                },
            );
            metric(
                ui,
                &format_bytes(progress.logical),
                "LOGICAL SIZE",
                p,
                available * 0.21,
            );
            metric(
                ui,
                &format_count(progress.files),
                "FILES DISCOVERED",
                p,
                available * 0.21,
            );
            metric(
                ui,
                &format_duration(progress.elapsed),
                "ELAPSED",
                p,
                available * 0.18,
            );
        });
        ui.horizontal(|ui| {
            let status = stop_status(state).unwrap_or(if state.is_active() {
                "Discovering · total unknown"
            } else if progress.state == ScanState::Partial {
                "Finished · partial results"
            } else if progress.state == ScanState::Complete {
                "Complete · logical sizes"
            } else {
                "Folder scan · read-only"
            });
            ui.label(RichText::new(status).small().color(p.muted));
            ui.label(
                RichText::new(format!(
                    "{} folders · {} skipped",
                    format_count(progress.folders),
                    format_count(progress.skipped)
                ))
                .small()
                .color(p.faint),
            );
            if ui
                .small_button(format!("{} errors", format_count(progress.errors_total)))
                .clicked()
            {
                self.show_errors = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(progress.current_path.to_string_lossy())
                            .small()
                            .color(p.faint),
                    )
                    .truncate(),
                )
                .on_hover_text(progress.current_path.to_string_lossy());
            });
        });
        ui.separator();
        if let Some(notice) = self.notice.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(p.accent, notice);
                if ui.small_button("Dismiss").clicked() {
                    self.notice = None;
                }
            });
        }
    }
    fn navigation(&mut self, ui: &mut egui::Ui, p: Palette) {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.history.can_back(), egui::Button::new("Back"))
                .clicked()
            {
                action = Some(Action::Back);
            }
            if ui
                .add_enabled(self.history.can_forward(), egui::Button::new("Forward"))
                .clicked()
            {
                action = Some(Action::Forward);
            }
            if ui
                .add_enabled(self.request.root != 0, egui::Button::new("Up"))
                .clicked()
            {
                action = Some(Action::Up);
            }
            let crumbs = &self.visible.view.breadcrumbs;
            if crumbs.is_empty() {
                ui.label(RichText::new("Your folders").color(p.muted));
            } else {
                let hidden = crumbs.len().saturating_sub(MAX_BREADCRUMBS);
                if hidden > 0 {
                    ui.colored_label(p.faint, "…")
                        .on_hover_text(format!("{hidden} parent folders not shown"));
                }
                for (id, name) in crumbs.iter().skip(hidden) {
                    ui.colored_label(p.faint, "/");
                    if ui
                        .add(
                            egui::Button::new(RichText::new(name).color(
                                if *id == self.request.root {
                                    p.text
                                } else {
                                    p.muted
                                },
                            ))
                            .frame(false),
                        )
                        .clicked()
                    {
                        action = Some(Action::Navigate(*id));
                    }
                }
            }
        });
        if let Some(action) = action {
            self.action(action);
        }
    }
    fn query_controls(&mut self, ui: &mut egui::Ui, p: Palette) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(match self.request.mode {
                    ViewMode::Overview => "Overview",
                    ViewMode::Explorer => "Folders & files",
                    ViewMode::Largest => "Largest files",
                    ViewMode::Types => "File types",
                })
                .strong(),
            );
            if self.visible.view.revision != self.request.revision {
                ui.label(RichText::new("Updating…").small().color(p.faint));
            }
            let previous = self.request.sort;
            egui::ComboBox::from_id_salt("sort")
                .selected_text(sort_label(self.request.sort))
                .width(105.0)
                .show_ui(ui, |ui| {
                    for (key, label) in [
                        (SortKey::Logical, "Logical size"),
                        (SortKey::Name, "Name"),
                        (SortKey::Files, "File count"),
                        (SortKey::Allocated, "Allocated"),
                        (SortKey::Modified, "Modified"),
                    ] {
                        if self.request.mode != ViewMode::Types
                            || !matches!(key, SortKey::Allocated | SortKey::Modified)
                        {
                            ui.selectable_value(&mut self.request.sort, key, label);
                        }
                    }
                });
            if previous != self.request.sort {
                self.mark_query_changed();
            }
            if ui
                .small_button(if self.request.descending {
                    "↓"
                } else {
                    "↑"
                })
                .on_hover_text("Reverse sort order")
                .clicked()
            {
                self.request.descending = !self.request.descending;
                self.mark_query_changed();
            }
        });
        ui.horizontal_wrapped(|ui| {
            let width = (ui.available_width() - 130.0).clamp(110.0, 260.0);
            let response = ui.add_sized(
                [width, 26.0],
                egui::TextEdit::singleline(&mut self.request.search)
                    .char_limit(1024)
                    .hint_text("Search names…"),
            );
            if response.changed() {
                self.search_changed = Some(Instant::now());
            }
            let scope = ui
                .checkbox(&mut self.request.search_all, "Whole scan")
                .on_hover_text(
                    "Search and filters use the whole scan; the map keeps your current folder context.",
                );
            if scope.changed() {
                self.mark_query_changed();
            }
        });
    }
    fn explorer(&mut self, ui: &mut egui::Ui, height: f32, p: Palette) {
        self.query_controls(ui, p);
        if self.request.mode == ViewMode::Types {
            self.types_table(ui, height, p);
            return;
        }
        let snapshot = self.visible.clone();
        let view = &snapshot.view;
        let mut action = None;
        let selected = self.request.selected;
        let width = ui.available_width();
        let width_id = ui.id().with("file_table_available_width");
        let width_changed = ui.data_mut(|data| {
            let previous = data.get_temp::<f32>(width_id);
            let changed = previous.is_some_and(|previous| (previous - width).abs() > 1.0);
            // Keep the last sizing width so a sequence of sub-point window
            // changes cannot accumulate into clipped columns without a reset.
            if previous.is_none() || changed {
                data.insert_temp(width_id, width);
            }
            changed
        });
        let table = TableBuilder::new(ui)
            .id_salt("file_table")
            .striped(false)
            .resizable(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .sense(Sense::click())
            .column(Column::remainder().at_least(150.0))
            .column(Column::initial(92.0).at_least(72.0))
            .column(Column::initial(90.0).at_least(72.0))
            .column(Column::initial(72.0).at_least(50.0))
            .min_scrolled_height(50.0)
            .max_scroll_height((height - 87.0).max(65.0));
        // Saved resizable widths otherwise push the size columns outside the
        // clipped table after a window, inspector, or text-scale resize.
        if width_changed {
            table.reset();
        }
        table
            .header(27.0, |mut header| {
                for label in [
                    "NAME",
                    "LOGICAL SIZE",
                    if view.row_scope_root != view.root {
                        "SCAN SHARE"
                    } else {
                        "FOLDER SHARE"
                    },
                    "FILES",
                ] {
                    header.col(|ui| {
                        ui.label(RichText::new(label).small().color(p.faint));
                    });
                }
            })
            .body(|body| {
                body.rows(29.0, view.rows.len(), |mut table_row| {
                    let row = &view.rows[table_row.index()];
                    table_row.set_selected(selected == Some(row.id));
                    table_row.col(|ui| {
                        ui.add_space((row.depth.min(12) as f32) * 12.0);
                        if row.has_children {
                            if ui
                                .add(
                                    egui::Button::new(if self.request.expanded.contains(&row.id) {
                                        "⌄"
                                    } else {
                                        "›"
                                    })
                                    .frame(false)
                                    .min_size(egui::vec2(12.0, 18.0)),
                                )
                                .clicked()
                            {
                                action = Some(Action::Toggle(row.id));
                            }
                        } else {
                            ui.add_space(18.0);
                        }
                        entry_icon(
                            ui,
                            row.is_dir,
                            12.0,
                            if row.is_dir { p.accent } else { p.faint },
                        );
                        ui.add(
                            egui::Label::new(RichText::new(&row.name).color(
                                if selected == Some(row.id) {
                                    p.accent
                                } else {
                                    p.text
                                },
                            ))
                            .truncate()
                            .selectable(false),
                        )
                        .on_hover_text(row.path.to_string_lossy());
                        if row.reparse {
                            ui.add(
                                egui::Label::new(RichText::new("link").small().color(p.faint))
                                    .selectable(false),
                            );
                        }
                        if row.hard_link {
                            ui.add(
                                egui::Label::new(RichText::new("hard link").small().color(p.faint))
                                    .selectable(false),
                            );
                        }
                    });
                    table_row.col(|ui| {
                        ui.add(egui::Label::new(format_bytes(row.logical)).selectable(false));
                    });
                    table_row.col(|ui| {
                        let fraction = row_share(row.logical, view);
                        let (bar, _) =
                            ui.allocate_exact_size(egui::vec2(28.0, 3.0), Sense::hover());
                        ui.painter().rect_filled(bar, 1.0, p.line);
                        ui.painter().rect_filled(
                            Rect::from_min_size(bar.min, egui::vec2(28.0 * fraction, 3.0)),
                            1.0,
                            if selected == Some(row.id) {
                                p.accent
                            } else {
                                p.faint
                            },
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{:.1}%", fraction * 100.0)).small(),
                            )
                            .selectable(false),
                        );
                    });
                    table_row.col(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(format_count(row.files)).color(p.muted))
                                .selectable(false),
                        );
                    });
                    let response = table_row.response();
                    if response.double_clicked() && row.is_dir {
                        action = Some(Action::Navigate(row.id));
                    } else if response.clicked() && action.is_none() {
                        action = Some(Action::Select(row.id));
                    }
                    response.context_menu(|ui| {
                        if row.is_dir && ui.button("Open folder in DiskScope").clicked() {
                            action = Some(Action::Navigate(row.id));
                            ui.close();
                        }
                        if ui.button("Reveal in Explorer").clicked() {
                            self.reveal(row.path.clone(), row.is_dir, ui.ctx());
                            ui.close();
                        }
                        if ui.button("Copy path").clicked() {
                            ui.ctx().copy_text(row.path.to_string_lossy().into_owned());
                            ui.close();
                        }
                        if !row.is_dir {
                            ui.separator();
                            if self.file_action_buttons(ui, row) {
                                ui.close();
                            }
                        }
                    });
                });
            });
        if view.rows.is_empty() {
            ui.label(
                RichText::new(if self.generation == 0 {
                    "Choose a folder, then Scan to begin."
                } else if !self.request.search.is_empty() {
                    "No matching items in this scope."
                } else if self.snapshot.progress.state.is_active() {
                    "Waiting for the first folders…"
                } else {
                    "This folder contains no visible items."
                })
                .color(p.muted),
            );
        }
        ui.horizontal(|ui| {
            let end = self
                .visible_offset
                .saturating_add(view.rows.len())
                .min(view.total_rows);
            let ready = view.revision == self.request.revision && !self.dirty;
            ui.label(
                RichText::new(if view.total_rows == 0 {
                    "0 items".into()
                } else {
                    format!(
                        "{}–{} of {} items",
                        self.visible_offset + 1,
                        end,
                        format_count(view.total_rows as u64)
                    )
                })
                .small()
                .color(p.faint),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        ready && end < view.total_rows,
                        egui::Button::new("Next ›").small(),
                    )
                    .clicked()
                {
                    action = Some(Action::Page(self.visible_offset.saturating_add(PAGE_SIZE)));
                }
                if ui
                    .add_enabled(
                        ready && self.visible_offset > 0,
                        egui::Button::new("‹ Previous").small(),
                    )
                    .clicked()
                {
                    action = Some(Action::Page(self.visible_offset.saturating_sub(PAGE_SIZE)));
                }
            });
        });
        if let Some(action) = action {
            self.action(action);
        }
    }
    fn types_table(&mut self, ui: &mut egui::Ui, height: f32, p: Palette) {
        let snapshot = self.visible.clone();
        let view = &snapshot.view;
        let mut extension_lens = None;
        TableBuilder::new(ui)
            .id_salt("types_table")
            .sense(Sense::click())
            .striped(false)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::remainder())
            .column(Column::initial(110.0))
            .column(Column::initial(90.0))
            .max_scroll_height((height - 70.0).max(65.0))
            .header(27.0, |mut header| {
                for label in ["EXTENSION", "LOGICAL SIZE", "FILES"] {
                    header.col(|ui| {
                        ui.label(RichText::new(label).small().color(p.faint));
                    });
                }
            })
            .body(|body| {
                body.rows(29.0, view.types.len(), |mut row| {
                    let item = &view.types[row.index()];
                    row.col(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new("■").color(tile_color(&item.extension)))
                                .selectable(false),
                        );
                        ui.add(egui::Label::new(&item.extension).selectable(false));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(format_bytes(item.logical)).selectable(false));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(format_count(item.files)).selectable(false));
                    });
                    if row.response().clicked() {
                        extension_lens = Some(item.extension.clone());
                    }
                });
            });
        if let Some(extension) = extension_lens {
            self.set_extension_lens(&extension);
        }
        ui.horizontal(|ui| {
            let end = self
                .visible_offset
                .saturating_add(view.types.len())
                .min(view.total_rows);
            let ready = view.revision == self.request.revision && !self.dirty;
            ui.label(
                RichText::new(format!(
                    "{} file types · logical bytes",
                    format_count(view.total_rows as u64)
                ))
                .small()
                .color(p.faint),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        ready && end < view.total_rows,
                        egui::Button::new("Next ›").small(),
                    )
                    .clicked()
                {
                    self.action(Action::Page(self.visible_offset.saturating_add(PAGE_SIZE)));
                }
                if ui
                    .add_enabled(
                        ready && self.visible_offset > 0,
                        egui::Button::new("‹ Previous").small(),
                    )
                    .clicked()
                {
                    self.action(Action::Page(self.visible_offset.saturating_sub(PAGE_SIZE)));
                }
            });
        });
    }
    fn treemap(&mut self, ui: &mut egui::Ui, height: f32, p: Palette) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Space map").strong());
            let previous = self.request.map_layout;
            ui.selectable_value(&mut self.request.map_layout, MapLayout::Treemap, "Treemap");
            ui.selectable_value(&mut self.request.map_layout, MapLayout::Icicle, "Icicle");
            if previous != self.request.map_layout {
                self.mark_query_changed();
            }
            ui.label(RichText::new("Area = logical size").small().color(p.faint))
                .on_hover_text(
                    "Folder colors distinguish items. Click to inspect; double-click a folder to open it.",
                );
            if ui.available_width() > 260.0 {
                ui.label(
                    RichText::new("Click to inspect · double-click to open")
                        .small()
                        .color(p.faint),
                );
            }
        });
        let width = ui.available_width();
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(width, (height - 70.0).max(70.0)), Sense::hover());
        let aspect = (rect.width() / rect.height()).clamp(0.25, 10.0);
        if self.generation > 0 && ((aspect / self.request.map_aspect).ln()).abs() > 0.12 {
            self.request.map_aspect = aspect;
            self.dirty = true;
        }
        ui.painter().rect_filled(rect, 5.0, p.side);
        let snapshot = self.visible.clone();
        let mut action = None;
        for item in &snapshot.view.map {
            let [x, y, w, h] = item.rect;
            if ![x, y, w, h].iter().all(|n| n.is_finite()) || w <= 0.0 || h <= 0.0 {
                continue;
            }
            let tile = Rect::from_min_size(
                rect.min + egui::vec2(x * rect.width(), y * rect.height()),
                egui::vec2(w * rect.width(), h * rect.height()),
            )
            .intersect(rect)
            .shrink(2.0);
            if !tile.is_positive() {
                continue;
            }
            let response = ui.interact(
                tile,
                ui.id().with(("tile", item.id, item.others)),
                Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    true,
                    format!(
                        "{} · {} · {}",
                        item.name,
                        format_bytes(item.logical),
                        if item.is_dir { "folder" } else { "file" }
                    ),
                )
            });
            let selected = !item.others && self.request.selected == Some(item.id);
            let color = map_tile_color(
                item,
                snapshot.view.filters_active,
                response.hovered(),
                self.preferences.theme,
            );
            if response.has_focus() {
                ui.painter().rect_stroke(
                    tile,
                    3.0,
                    Stroke::new(3.0_f32, p.accent),
                    egui::StrokeKind::Outside,
                );
            }
            ui.painter().rect_filled(tile, 3.0, color);
            if selected {
                ui.painter().rect_stroke(
                    tile.shrink(1.0),
                    3.0,
                    Stroke::new(2.0_f32, Color32::from_rgb(222, 255, 247)),
                    egui::StrokeKind::Inside,
                );
            }
            let painter = ui.painter().with_clip_rect(tile.shrink(5.0));
            if tile.width() > 55.0 && tile.height() > 32.0 {
                painter.text(
                    tile.left_bottom() + egui::vec2(8.0, -24.0),
                    egui::Align2::LEFT_BOTTOM,
                    &item.name,
                    FontId::proportional(12.0),
                    Color32::WHITE,
                );
                painter.text(
                    tile.left_bottom() + egui::vec2(8.0, -8.0),
                    egui::Align2::LEFT_BOTTOM,
                    format_bytes(item.logical),
                    FontId::proportional(10.0),
                    Color32::from_rgb(221, 234, 241),
                );
            }
            if selected && tile.height() > 70.0 && tile.width() > 70.0 {
                painter.text(
                    tile.left_top() + egui::vec2(8.0, 7.0),
                    egui::Align2::LEFT_TOP,
                    "Selected",
                    FontId::proportional(10.0),
                    Color32::WHITE,
                );
            }
            let double = response.double_clicked();
            let click = response.clicked();
            response.clone().on_hover_text(format!(
                "{}\n{}{}",
                item.name,
                format_bytes(item.logical),
                if item.others {
                    "\nOpen the list to inspect these items"
                } else {
                    ""
                }
            ));
            let activate = click
                || response.has_focus()
                    && ui.input(|i| {
                        i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)
                    });
            if (item.others && activate) || (double && item.is_dir) {
                action = Some(Action::Navigate(item.id));
            } else if activate {
                action = Some(Action::Select(item.id));
            }
        }
        if snapshot.view.map.is_empty() {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if self.generation == 0 {
                    "Your space, at a glance"
                } else {
                    "Map appears as file sizes are discovered"
                },
                FontId::proportional(13.0),
                p.faint,
            );
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Files:").small().color(p.faint));
            for (label, color) in FILE_CATEGORIES
                .iter()
                .map(|category| (category.label, category.color))
                .chain([("Other", OTHER_TILE_COLOR)])
            {
                ui.colored_label(color, "■");
                ui.label(RichText::new(label).small().color(p.muted));
            }
        });
        if let Some(action) = action {
            self.action(action);
        }
    }
    fn inspector(&mut self, ui: &mut egui::Ui, p: Palette) {
        ui.spacing_mut().interact_size.y = 16.0;
        overline(ui, "SELECTION", p);
        let snapshot = self.visible.clone();
        let Some(row) = snapshot
            .view
            .selected
            .as_ref()
            .filter(|row| self.request.selected == Some(row.id))
        else {
            ui.add_space(12.0);
            ui.label(
                RichText::new(if self.request.selected.is_some() {
                    "Updating selection…"
                } else {
                    "Select a folder or file"
                })
                .size(18.0),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new("Inspect sizes and open the location in Explorer.")
                    .small()
                    .color(p.muted),
            );
            ui.add_space(22.0);
            self.accounting_note(ui, p);
            return;
        };
        entry_icon(ui, row.is_dir, 30.0, p.accent);
        ui.add(egui::Label::new(RichText::new(&row.name).size(20.0)).wrap());
        ui.add(
            egui::Label::new(
                RichText::new(row.path.to_string_lossy())
                    .small()
                    .color(p.faint),
            )
            .wrap(),
        );
        ui.add_space(15.0);
        ui.label(RichText::new(format_bytes(row.logical)).size(29.0));
        ui.label(
            RichText::new(format!(
                "{:.1}% of {}",
                row_share(row.logical, &snapshot.view) * 100.0,
                if snapshot.view.row_scope_root != snapshot.view.root {
                    "whole scan"
                } else {
                    "current folder"
                }
            ))
            .small()
            .color(p.muted),
        );
        ui.add_space(10.0);
        if ui
            .button(if self.request.pinned.contains(&row.id) {
                "Unpin comparison"
            } else {
                "Pin to compare"
            })
            .clicked()
        {
            self.pin(row.id);
        }
        ui.separator();
        detail(ui, "Files", &format_count(row.files), p);
        detail(ui, "Folders", &format_count(row.folders), p);
        detail(
            ui,
            "Allocated sum",
            &row.allocated
                .map(format_bytes)
                .unwrap_or_else(|| "Unknown".into()),
            p,
        );
        detail(
            ui,
            "Kind",
            if row.reparse {
                "Reparse point"
            } else if row.is_dir {
                "Folder"
            } else {
                "File"
            },
            p,
        );
        if row.hard_link {
            detail(ui, "Hard link", "Yes", p);
        }
        if let Some(modified) = row.modified {
            detail(ui, "Modified", &date_time_label(modified), p);
        }
        ui.add_space(15.0);
        if row.is_dir
            && ui
                .add_sized(
                    [ui.available_width(), 29.0],
                    egui::Button::new("Explore this folder"),
                )
                .clicked()
        {
            self.action(Action::Navigate(row.id));
        }
        if ui
            .add_sized(
                [ui.available_width(), 29.0],
                egui::Button::new("Reveal in Explorer"),
            )
            .clicked()
        {
            self.reveal(row.path.clone(), row.is_dir, ui.ctx());
        }
        if ui
            .add_sized([ui.available_width(), 29.0], egui::Button::new("Copy path"))
            .clicked()
        {
            ui.ctx().copy_text(row.path.to_string_lossy().into_owned());
        }
        self.selected_file_actions(ui, row, p);
        ui.add_space(16.0);
        if row.is_dir {
            self.breakdown(ui, p);
        }
        ui.separator();
        self.accounting_note(ui, p);
        if self.snapshot.progress.state.is_active()
            || matches!(
                self.snapshot.progress.state,
                ScanState::Cancelled | ScanState::Partial
            )
        {
            ui.add_space(10.0);
            ui.label(
                RichText::new("Partial results · sizes can change as files are discovered.")
                    .small()
                    .color(p.faint),
            );
        }
    }
    fn accounting_note(&self, ui: &mut egui::Ui, p: Palette) {
        ui.label(RichText::new("SIZE ACCOUNTING").small().color(p.faint));
        ui.label(
            RichText::new(
                "Logical sizes are additive. Hard links can appear more than once in this total.",
            )
            .small()
            .color(p.muted),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "Current folder unique allocated: {}",
                folder_allocation_text(&self.visible)
            ))
            .small()
            .color(p.muted),
        )
        .on_hover_text(format!(
            "Folder: {}",
            self.visible.view.root_path.to_string_lossy()
        ));
        ui.label(
            RichText::new(
                "Sibling folder unique totals can overlap when hard links cross folders.",
            )
            .small()
            .color(p.faint),
        );
        ui.label(
            RichText::new("Alternate data streams and filesystem metadata are excluded.")
                .small()
                .color(p.faint),
        );
        ui.label(
            RichText::new(
                "Allocated sums are not reclaimable space. Unknown metadata stays unknown.",
            )
            .small()
            .color(p.faint),
        );
    }
    fn error_window(&mut self, ctx: &egui::Context, p: Palette) {
        egui::Window::new("Scan errors & skipped locations")
            .open(&mut self.show_errors)
            .default_size([650.0, 350.0])
            .show(ctx, |ui| {
                let progress = &self.snapshot.progress;
                ui.label(format!(
                    "{} errors · {} skipped entries",
                    format_count(progress.errors_total),
                    format_count(progress.skipped)
                ));
                ui.label(
                    RichText::new(
                        "Skipped directory reparse points are not followed. Error samples are bounded.",
                    )
                    .small()
                    .color(p.muted),
                );
                ui.separator();
                egui::ScrollArea::vertical().show_rows(ui, 50.0, progress.errors.len(), |ui, range| {
                    for error in &progress.errors[range] {
                        ui.add(
                            egui::Label::new(RichText::new(error.path.to_string_lossy()).strong())
                                .truncate(),
                        )
                        .on_hover_text(error.path.to_string_lossy());
                        ui.label(RichText::new(&error.message).small().color(p.muted));
                    }
                });
                if progress.errors.is_empty() {
                    ui.label("No error samples.");
                }
            });
    }
}
impl eframe::App for DiskScopeApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // eframe runs logic even when a minimized window does not paint.
        self.poll(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.management.busy() {
            self.file_action_keyboard(&ctx);
            if !self.management.busy() {
                self.keyboard(&ctx);
            }
        }
        if self
            .search_changed
            .is_some_and(|time| time.elapsed() >= SEARCH_DEBOUNCE)
        {
            self.mark_query_changed();
        }
        let p = self.preferences.theme.palette();
        self.sidebar(ui, p);
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(p.side)
                    .inner_margin(egui::Margin::symmetric(18, 6)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.scan_state().label())
                            .small()
                            .color(p.muted),
                    );
                    ui.label(
                        RichText::new(if self.snapshot.progress.backend.is_empty() {
                            "Normal folder scan"
                        } else {
                            &self.snapshot.progress.backend
                        })
                        .small()
                        .color(p.faint),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("Preview · performance validation in progress")
                                .small()
                                .color(p.faint),
                        );
                    });
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(egui::Margin::symmetric(22, 17)),
            )
            .show_inside(ui, |ui| {
                if self.management.busy() {
                    ui.disable();
                }
                self.header(ui, p);
                if self.changes {
                    self.changes_page(ui, p);
                } else if self.request.mode == ViewMode::Overview {
                    self.overview(ui, p);
                } else {
                    self.explore_page(ui, p);
                }
            });
        if !self.management.busy() {
            self.error_window(&ctx, p);
            self.filter_window(&ctx, p);
            self.command_palette(&ctx, p);
            self.settings_window(&ctx, p);
            self.help_window(&ctx, p);
            self.comparison_window(&ctx, p);
        }
        self.management_dialog(&ctx, p);
        self.submit_view();
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if !self.access_mode.persistence_allowed() {
            return;
        }
        if let Ok(json) = serde_json::to_string(&self.preferences) {
            storage.set_string(PREFERENCES_KEY, json);
        }
    }
    fn persist_egui_memory(&self) -> bool {
        self.access_mode.persistence_allowed()
    }
}
fn overline(ui: &mut egui::Ui, text: &str, p: Palette) {
    ui.label(RichText::new(text).small().color(p.faint));
    ui.add_space(4.0);
}
fn entry_icon(ui: &mut egui::Ui, is_dir: bool, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let point = |x: f32, y: f32| rect.min + egui::vec2(size * x, size * y);
    let points = if is_dir {
        vec![
            point(0.08, 0.25),
            point(0.08, 0.14),
            point(0.43, 0.14),
            point(0.57, 0.3),
            point(0.92, 0.3),
            point(0.92, 0.86),
            point(0.08, 0.86),
            point(0.08, 0.25),
        ]
    } else {
        vec![
            point(0.2, 0.08),
            point(0.62, 0.08),
            point(0.84, 0.3),
            point(0.84, 0.92),
            point(0.2, 0.92),
            point(0.2, 0.08),
        ]
    };
    ui.painter().add(egui::Shape::line(
        points,
        Stroke::new((size / 14.0).clamp(1.0, 2.0), color),
    ));
}
fn metric(ui: &mut egui::Ui, value: &str, label: &str, p: Palette, width: f32) {
    ui.allocate_ui(egui::vec2(width, 54.0), |ui| {
        ui.set_min_width(width);
        ui.horizontal(|ui| {
            ui.separator();
            ui.vertical(|ui| {
                ui.label(RichText::new(value).size(22.0));
                ui.label(RichText::new(label).small().color(p.muted));
            });
        });
    });
}
fn detail(ui: &mut egui::Ui, name: &str, value: &str, p: Palette) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).small().color(p.muted));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value).small());
        });
    });
}
fn sort_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "Name",
        SortKey::Logical => "Logical size",
        SortKey::Allocated => "Allocated",
        SortKey::Files => "File count",
        SortKey::Modified => "Modified",
    }
}
fn share(bytes: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (bytes as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}
fn format_duration(duration: Duration) -> String {
    let s = duration.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}
fn format_count(count: u64) -> String {
    let digits = count.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            text.push(',');
        }
        text.push(c);
    }
    text
}
/// A named family of extensions with its map colour. One table feeds the map
/// tiles, the legend and the filter presets so they can never disagree.
pub(crate) struct FileCategory {
    pub label: &'static str,
    /// Lowercase extensions without the leading dot.
    pub extensions: &'static [&'static str],
    pub color: Color32,
}
impl FileCategory {
    /// The category as a filter string, e.g. `.mp4, .mkv`.
    pub fn filter_text(&self) -> String {
        self.extensions
            .iter()
            .map(|extension| format!(".{extension}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}
pub(crate) const FILE_CATEGORIES: [FileCategory; 5] = [
    FileCategory {
        label: "Video",
        extensions: &["mp4", "mkv", "mov", "avi", "webm", "wmv"],
        color: Color32::from_rgb(80, 127, 152),
    },
    FileCategory {
        label: "Archives",
        extensions: &["zip", "7z", "rar", "tar", "gz", "iso"],
        color: Color32::from_rgb(154, 134, 94),
    },
    FileCategory {
        label: "Code",
        extensions: &[
            "rs", "js", "ts", "py", "cs", "cpp", "c", "h", "json", "toml",
        ],
        color: Color32::from_rgb(105, 140, 120),
    },
    FileCategory {
        label: "Images",
        extensions: &["png", "jpg", "jpeg", "gif", "webp", "svg", "heic", "psd"],
        color: Color32::from_rgb(131, 117, 150),
    },
    FileCategory {
        label: "Documents",
        extensions: &["pdf", "doc", "docx", "txt", "md", "xlsx", "pptx"],
        color: Color32::from_rgb(123, 142, 158),
    },
];
const OTHER_TILE_COLOR: Color32 = Color32::from_rgb(80, 104, 115);
fn folder_color(name: &str) -> Color32 {
    // This distinguishes sibling items only; it says nothing about their contents.
    let hash = name.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    });
    egui::ecolor::Hsva::new((hash % 360) as f32 / 360.0, 0.40, 0.38, 1.0).into()
}
/// Colour for a file name or a type bucket such as `.mp4`.
fn tile_color(name: &str) -> Color32 {
    let extension = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    FILE_CATEGORIES
        .iter()
        .find(|category| category.extensions.contains(&extension.as_str()))
        .map_or(OTHER_TILE_COLOR, |category| category.color)
}
fn opaque_tint(color: Color32, factor: f32) -> Color32 {
    Color32::from_rgb(
        (color.r() as f32 * factor).round() as u8,
        (color.g() as f32 * factor).round() as u8,
        (color.b() as f32 * factor).round() as u8,
    )
}
fn map_tile_color(item: &MapItem, filters: bool, hovered: bool, theme: Theme) -> Color32 {
    let mut color = if item.is_dir && !item.others {
        folder_color(&item.name)
    } else {
        tile_color(&item.name)
    };
    color = opaque_tint(color, 0.68);
    if filters && !item.matched {
        color = opaque_tint(color, 0.32);
    }
    if theme == Theme::HighContrast {
        color = if !filters || item.matched {
            Color32::from_rgb(0, 64, 85)
        } else {
            Color32::from_rgb(30, 30, 30)
        };
    }
    if hovered {
        color = opaque_tint(color, 1.16);
    }
    color
}
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
fn admits_snapshot(
    generation: u64,
    revision: u64,
    incoming_generation: u64,
    incoming_revision: u64,
) -> bool {
    generation == incoming_generation && revision == incoming_revision
}
#[cfg(test)]
mod tests {
    use super::*;

    fn table_fixture() -> (DiskScopeApp, egui::Context) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app =
            DiskScopeApp::with_history_directory(&cc, None, PathBuf::new(), AccessMode::Admin);
        let mut model = Model::new(PathBuf::from(r"C:\fixture"));
        model.insert_batch(
            0,
            vec![
                Entry {
                    name: "demo.mp4".into(),
                    is_dir: false,
                    reparse: false,
                    logical: 100,
                    allocated: Some(4096),
                    modified: None,
                    identity: Some(FileIdentity {
                        volume: 1,
                        id: [1; 16],
                    }),
                },
                Entry {
                    name: "folder".into(),
                    is_dir: true,
                    reparse: false,
                    logical: 0,
                    allocated: Some(0),
                    modified: None,
                    identity: None,
                },
            ],
        );
        model.insert_batch(
            2,
            vec![Entry {
                name: "nested.bin".into(),
                is_dir: false,
                reparse: false,
                logical: 50,
                allocated: Some(4096),
                modified: None,
                identity: Some(FileIdentity {
                    volume: 1,
                    id: [2; 16],
                }),
            }],
        );
        app.generation = 7;
        app.root_path = model.root_path.clone();
        app.request.mode = ViewMode::Explorer;
        let mut snapshot = UiSnapshot::default();
        snapshot.progress.generation = 7;
        snapshot.progress.root = model.root_path.clone();
        snapshot.progress.state = ScanState::Complete;
        snapshot.view = diskscope::view::build_view(&model, &app.request, 7);
        app.snapshot = Arc::new(snapshot);
        app.visible = app.snapshot.clone();
        (app, ctx)
    }

    fn table_frame(
        app: &mut DiskScopeApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        table_frame_at_width(app, ctx, events, 1000.0)
    }

    fn table_frame_at_width(
        app: &mut DiskScopeApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        width: f32,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 700.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                app.explorer(ui, 450.0, app.preferences.theme.palette())
            });
        })
    }

    fn text_center(shapes: &[egui::epaint::ClippedShape], expected: &str) -> egui::Pos2 {
        fn find(shape: &egui::epaint::Shape, expected: &str) -> Option<egui::Pos2> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == expected => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                egui::epaint::Shape::Vec(shapes) => {
                    shapes.iter().find_map(|shape| find(shape, expected))
                }
                _ => None,
            }
        }
        shapes
            .iter()
            .find_map(|shape| find(&shape.shape, expected))
            .unwrap_or_else(|| panic!("Missing rendered text: {expected}"))
    }

    #[test]
    fn main_window_renders_with_each_theme_and_text_scale() {
        use eframe::App as _;

        for theme in [Theme::Dark, Theme::Light, Theme::HighContrast] {
            for scale in [TextScale::Normal, TextScale::Large, TextScale::Largest] {
                let (mut app, ctx) = table_fixture();
                app.preferences.theme = theme;
                app.preferences.text_scale = scale;
                app.apply_appearance(&ctx);
                app.dirty = false;
                let mut frame = eframe::Frame::_new_kittest();
                let mut output = egui::FullOutput::default();
                for _ in 0..3 {
                    output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(1440.0, 900.0),
                            )),
                            ..Default::default()
                        },
                        |ui| app.ui(ui, &mut frame),
                    );
                }
                let sidebar = text_center(&output.shapes, "DiskScope");
                let file = text_center(&output.shapes, "demo.mp4");
                let status = text_center(&output.shapes, "Normal folder scan");
                assert!(sidebar.x < file.x, "{theme:?} {scale:?}");
                assert!(file.y < status.y, "{theme:?} {scale:?}");
                let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
                assert!(primitives.iter().any(|primitive| {
                    matches!(&primitive.primitive, egui::epaint::Primitive::Mesh(mesh)
                        if !mesh.vertices.is_empty() && mesh.is_valid())
                }));
            }
        }
    }

    #[test]
    fn shrinking_an_existing_table_keeps_numeric_columns_visible() {
        let (mut app, ctx) = table_fixture();
        for width in [1440.0, 900.0, 650.0, 1440.0]
            .into_iter()
            .chain((900..1440).rev().map(|width| width as f32))
        {
            let mut output = egui::FullOutput::default();
            for _ in 0..3 {
                output = table_frame_at_width(&mut app, &ctx, vec![], width);
            }
            for expected in ["LOGICAL SIZE", "FOLDER SHARE", "FILES"] {
                assert!(
                    output.shapes.iter().any(|shape| {
                        matches!(&shape.shape, egui::epaint::Shape::Text(text)
                        if text.galley.text() == expected
                            && shape.clip_rect.contains(text.pos + text.galley.size() * 0.5))
                    }),
                    "{expected} clipped at width {width}"
                );
            }
        }
    }

    #[test]
    fn restoring_a_wide_table_into_a_narrow_window_keeps_numeric_columns_visible() {
        let (mut app, old_context) = table_fixture();
        for _ in 0..3 {
            table_frame_at_width(&mut app, &old_context, vec![], 1440.0);
        }
        #[derive(Default)]
        struct MemoryStorage(Option<String>);
        impl eframe::Storage for MemoryStorage {
            fn get_string(&self, _key: &str) -> Option<String> {
                self.0.clone()
            }
            fn set_string(&mut self, _key: &str, value: String) {
                self.0 = Some(value);
            }
            fn flush(&mut self) {}
        }
        let mut saved = MemoryStorage::default();
        old_context.memory(|memory| eframe::set_value(&mut saved, "egui", memory));
        let context = egui::Context::default();
        context.memory_mut(|memory| *memory = eframe::get_value(&saved, "egui").unwrap());
        let mut output = egui::FullOutput::default();
        for _ in 0..3 {
            output = table_frame_at_width(&mut app, &context, vec![], 900.0);
        }
        for expected in ["LOGICAL SIZE", "FOLDER SHARE", "FILES"] {
            assert!(
                output.shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::epaint::Shape::Text(text)
                    if text.galley.text() == expected
                        && shape.clip_rect.contains(text.pos + text.galley.size() * 0.5))
                }),
                "{expected} clipped after restoring table state"
            );
        }
    }

    #[test]
    fn logic_polls_native_results_without_rendering_a_window() {
        use eframe::App as _;

        let (mut app, ctx) = table_fixture();
        app.native_tx
            .send(NativeResult::Reveal(Err("fixture reveal failed".into())))
            .unwrap();
        app.logic(&ctx, &mut eframe::Frame::_new_kittest());
        assert_eq!(app.notice.as_deref(), Some("fixture reveal failed"));
    }

    fn primary_click(app: &mut DiskScopeApp, ctx: &egui::Context, position: egui::Pos2) {
        table_frame(app, ctx, vec![egui::Event::PointerMoved(position)]);
        table_frame(
            app,
            ctx,
            vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        table_frame(
            app,
            ctx,
            vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }

    #[test]
    fn clicking_a_file_name_selects_its_table_row() {
        let (mut app, ctx) = table_fixture();
        table_frame(&mut app, &ctx, vec![]);
        let output = table_frame(&mut app, &ctx, vec![]);
        let position = text_center(&output.shapes, "demo.mp4");
        primary_click(&mut app, &ctx, position);
        assert_eq!(app.request.selected, Some(1));
    }

    #[test]
    fn clicking_a_file_size_selects_its_table_row() {
        let (mut app, ctx) = table_fixture();
        table_frame(&mut app, &ctx, vec![]);
        let output = table_frame(&mut app, &ctx, vec![]);
        let position = text_center(&output.shapes, "100 B");
        primary_click(&mut app, &ctx, position);
        assert_eq!(app.request.selected, Some(1));
    }

    fn panel_frame(
        app: &mut DiskScopeApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        map: bool,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let palette = app.preferences.theme.palette();
                    if map {
                        app.treemap(ui, 450.0, palette);
                    } else {
                        app.types_table(ui, 450.0, palette);
                    }
                });
            },
        )
    }

    fn panel_click(app: &mut DiskScopeApp, ctx: &egui::Context, pos: egui::Pos2, map: bool) {
        panel_frame(app, ctx, vec![egui::Event::PointerMoved(pos)], map);
        for pressed in [true, false] {
            panel_frame(
                app,
                ctx,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                map,
            );
        }
    }

    #[test]
    fn types_text_and_whitespace_activate_the_extension_lens() {
        for target in [".mp4", "100 B", "1", "whitespace"] {
            let (mut app, ctx) = table_fixture();
            app.request.mode = ViewMode::Types;
            let snapshot = Arc::make_mut(&mut app.snapshot);
            snapshot.view.types = vec![TypeStat {
                extension: ".mp4".into(),
                logical: 100,
                files: 1,
            }];
            snapshot.view.total_rows = 1;
            app.visible = app.snapshot.clone();
            panel_frame(&mut app, &ctx, vec![], false);
            let output = panel_frame(&mut app, &ctx, vec![], false);
            let pos = if target == "whitespace" {
                let text = text_center(&output.shapes, ".mp4");
                egui::pos2(300.0, text.y)
            } else {
                text_center(&output.shapes, target)
            };
            panel_click(&mut app, &ctx, pos, false);
            assert_eq!(app.request.filters.extensions, vec![".mp4"], "{target}");
            assert_eq!(app.request.mode, ViewMode::Explorer, "{target}");
        }
    }

    #[test]
    fn nested_icicle_other_opens_its_containing_folder() {
        let (mut app, ctx) = table_fixture();
        app.request.map_layout = MapLayout::Icicle;
        let snapshot = Arc::make_mut(&mut app.snapshot);
        // A sufficiently wide aggregate makes the same nested Other generated
        // by the bounded icicle layout directly clickable in this UI fixture.
        snapshot.view.map = vec![MapItem {
            id: 2,
            name: "Other".into(),
            logical: 50,
            rect: [0.0, 0.5, 1.0, 0.5],
            is_dir: true,
            others: true,
            matched: false,
        }];
        app.visible = app.snapshot.clone();
        panel_frame(&mut app, &ctx, vec![], true);
        let output = panel_frame(&mut app, &ctx, vec![], true);
        let pos = text_center(&output.shapes, "Other");
        panel_click(&mut app, &ctx, pos, true);
        assert_eq!(app.request.root, 2);
        assert_eq!(app.request.mode, ViewMode::Explorer);
    }

    #[test]
    fn largest_sort_and_search_controls_stay_above_the_table_in_the_explore_page() {
        let (mut app, ctx) = table_fixture();
        app.request.mode = ViewMode::Largest;
        let mut output = egui::FullOutput::default();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 900.0),
                )),
                ..Default::default()
            };
            output = ctx.run_ui(input, |ui| {
                let p = app.preferences.theme.palette();
                app.sidebar(ui, p);
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    app.header(ui, p);
                    app.explore_page(ui, p);
                });
            });
        }
        let sort = text_center(&output.shapes, "Logical size");
        let search = text_center(&output.shapes, "Search names…");
        let scope = text_center(&output.shapes, "Whole scan");
        let header = text_center(&output.shapes, "NAME");
        eprintln!(
            "control positions sort={sort:?} search={search:?} scope={scope:?} table={header:?}"
        );
        assert!(search.y > sort.y + 14.0);
        assert!((search.y - scope.y).abs() < 6.0);
        assert!(header.y > search.y + 14.0);
    }

    #[test]
    fn double_clicking_a_folder_name_navigates_to_that_folder() {
        let (mut app, ctx) = table_fixture();
        table_frame(&mut app, &ctx, vec![]);
        let output = table_frame(&mut app, &ctx, vec![]);
        let position = text_center(&output.shapes, "folder");
        primary_click(&mut app, &ctx, position);
        assert_eq!(app.request.selected, Some(2));
        primary_click(&mut app, &ctx, position);
        assert_eq!(app.request.root, 2);
    }

    #[test]
    fn folder_expander_toggles_without_selecting_or_navigating() {
        let (mut app, ctx) = table_fixture();
        table_frame(&mut app, &ctx, vec![]);
        let output = table_frame(&mut app, &ctx, vec![]);
        let position = text_center(&output.shapes, "›");
        primary_click(&mut app, &ctx, position);
        assert!(app.request.expanded.contains(&2));
        assert_eq!(app.request.root, 0);
        assert_eq!(app.request.selected, None);
    }

    #[test]
    fn right_clicking_a_file_name_opens_the_row_context_menu() {
        let (mut app, ctx) = table_fixture();
        table_frame(&mut app, &ctx, vec![]);
        let output = table_frame(&mut app, &ctx, vec![]);
        let position = text_center(&output.shapes, "demo.mp4");
        table_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(position)]);
        for pressed in [true, false] {
            table_frame(
                &mut app,
                &ctx,
                vec![egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        let output = table_frame(&mut app, &ctx, vec![]);
        let _ = text_center(&output.shapes, "Reveal in Explorer");
    }
    #[test]
    fn map_tile_fill_stays_opaque_for_white_labels_in_light_theme() {
        let item = MapItem {
            id: 1,
            name: "report.pdf".into(),
            logical: 100,
            rect: [0.0, 0.0, 1.0, 1.0],
            is_dir: false,
            others: false,
            matched: true,
        };
        let color = map_tile_color(&item, false, false, Theme::Light);
        assert_eq!(
            color.a(),
            255,
            "Opacity must not blend white-label backgrounds into the light surface"
        );
    }
    #[test]
    fn cancellation_is_immediate_survives_inflight_progress_and_clears_on_terminal_or_replacement()
    {
        let mut pending = CancellationAcknowledgement::default();
        pending.request(7, ScanState::Scanning);
        assert_eq!(pending.state(7, ScanState::Scanning), ScanState::Cancelling);
        pending.accept(7, ScanState::Finalizing);
        pending.accept(6, ScanState::Cancelled);
        assert_eq!(pending.state(7, ScanState::Scanning), ScanState::Cancelling);
        pending.accept(7, ScanState::Cancelled);
        assert_eq!(pending.state(7, ScanState::Cancelled), ScanState::Cancelled);
        assert_eq!(pending.state(7, ScanState::Scanning), ScanState::Scanning);
        pending.request(7, ScanState::Scanning);
        pending.reset();
        assert_eq!(pending.state(8, ScanState::Preparing), ScanState::Preparing);
        assert_eq!(pending.generation, None);
    }
    #[test]
    fn current_folder_allocation_uses_admitted_view_with_cross_subtree_hardlinks() {
        let mut model = Model::new("fixture".into());
        let directory = |name: &str| Entry {
            name: name.into(),
            is_dir: true,
            reparse: false,
            logical: 0,
            allocated: Some(0),
            modified: None,
            identity: None,
        };
        let file = |name: &str, id: u8, allocated| Entry {
            name: name.into(),
            is_dir: false,
            reparse: false,
            logical: 100,
            allocated: Some(allocated),
            modified: None,
            identity: Some(FileIdentity {
                volume: 1,
                id: [id; 16],
            }),
        };
        model.insert_batch(0, vec![directory("A"), directory("B")]);
        model.insert_batch(1, vec![file("shared", 1, 4096)]);
        model.insert_batch(
            2,
            vec![file("shared-link", 1, 4096), file("private", 2, 8192)],
        );
        let a = diskscope::view::build_view(
            &model,
            &ViewRequest {
                root: 1,
                revision: 3,
                ..Default::default()
            },
            9,
        );
        let b = diskscope::view::build_view(
            &model,
            &ViewRequest {
                root: 2,
                revision: 4,
                ..Default::default()
            },
            9,
        );
        let mut snapshot = UiSnapshot {
            progress: Progress {
                unique_allocated: model.unique_allocated,
                ..Default::default()
            },
            view: a,
        };
        assert_eq!(snapshot.progress.unique_allocated, Some(12288));
        assert_eq!(folder_allocation_text(&snapshot), "4.0 KiB");
        // A pending revision does not replace the admitted A view with B's allocation.
        assert!(!admits_snapshot(9, 5, b.generation, b.revision));
        assert_eq!(folder_allocation_text(&snapshot), "4.0 KiB");
        snapshot.view = b;
        assert_eq!(folder_allocation_text(&snapshot), "12.0 KiB");
        assert_eq!(snapshot.view.unique_allocated, Some(12288));
    }
    #[test]
    fn active_engine_explanations_survive_waiting_progress_and_fallback() {
        for (state, message) in [
            (ScanState::Preparing, "Waiting for previous scan I/O"),
            (ScanState::Scanning, "MFT records: 400"),
            (
                ScanState::Scanning,
                "NTFS unavailable; normal scan fallback",
            ),
        ] {
            let progress = Progress {
                state,
                message: message.into(),
                ..Default::default()
            };
            assert_eq!(progress_explanation(&progress), message);
        }
    }
    #[test]
    fn result_share_uses_search_scope_without_replacing_browse_map_total() {
        let view = ViewSnapshot {
            root: 1,
            total_logical: 100,
            row_scope_root: 0,
            row_scope_logical: 10_000,
            ..Default::default()
        };
        assert_eq!(row_share(1000, &view), 0.1);
        assert_eq!(view.total_logical, 100);
    }
    #[test]
    fn byte_labels_preserve_binary_units_and_empty_size() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1_572_864), "1.5 MiB");
        assert_eq!(format_bytes(u64::MAX), "16.0 EiB");
    }
    #[test]
    fn stale_worker_views_cannot_replace_current_navigation() {
        assert!(!admits_snapshot(4, 7, 3, 7));
        assert!(!admits_snapshot(4, 7, 4, 6));
        assert!(!admits_snapshot(4, 7, 4, 8));
        assert!(admits_snapshot(4, 7, 4, 7));
    }
}
