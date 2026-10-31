mod accessibility;
mod changes;
mod controls;
mod investigation;
mod journal;
mod management;
mod native;
mod outcome;
mod state;
mod support;
#[cfg(test)]
mod test_support;
mod ux;

use crate::theme::{Palette, TextScale, Theme, load_system_font};
use controls::{date_label, date_time_label};
use diskscope::session::SessionPolicy;
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
    BrowseContext, BrowseHistory, Command, MAX_PINS, PaletteFacts, PendingRestore, RestoreStep,
    SavedBrowse, apply_extensions, apply_type_lens, command_matches, comparison_matches,
    focus_change_request, hold_view, overview_type_share, previous_pair, restore_browse_context,
    save_browse, toggle_pin, transitioned,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

pub(crate) fn configure_session_options(
    policy: SessionPolicy,
    options: &mut eframe::NativeOptions,
) {
    if !policy.persistence_allowed() {
        options.persistence_path = Some(PathBuf::from(r"\\.\NUL"));
        options.persist_window = false;
    }
}

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
/// What the header's stop status says while a stop is pending or done.
fn stop_status(state: ScanState) -> Option<&'static str> {
    match state {
        ScanState::Cancelling => Some("Stop requested · keeping partial results"),
        ScanState::Cancelled => Some("Stopped · partial results"),
        _ => None,
    }
}
/// The header's detail line: what the scan is doing, where it is reading, and
/// whether the user's place is still being looked up. Empty when there is
/// nothing to add.
fn status_detail(
    progress: &Progress,
    state: ScanState,
    restore: Option<&PendingRestore>,
) -> String {
    let mut parts = Vec::new();
    if let Some(restore) = restore {
        parts.push(restore.status());
    }
    if state.is_active() {
        parts.push(progress_explanation(progress).to_owned());
        let reading = progress.current_path.to_string_lossy();
        if !reading.is_empty() {
            parts.push(reading.into_owned());
        }
    } else if matches!(state, ScanState::Partial | ScanState::Failed) {
        parts.push(progress_explanation(progress).to_owned());
    }
    parts.retain(|part| !part.is_empty());
    parts.join(" · ")
}
const NO_ERRORS_HINT: &str =
    "No errors were reported. Skipped links and reparse points are not errors.";
/// The error-count button. With nothing to list it is disabled and says why
/// instead of opening an empty window. True when it was clicked.
fn errors_button(ui: &mut egui::Ui, errors_total: u64) -> bool {
    let label = format!(
        "{} {}",
        format_count(errors_total),
        if errors_total == 1 { "error" } else { "errors" }
    );
    let response = ui.add_enabled(errors_total > 0, egui::Button::new(label).small());
    let clicked = response.clicked();
    if clicked {
        controls::set_dialog_invoker(ui.ctx(), "Scan errors & skipped locations", response.id);
    }
    response.on_disabled_hover_text(NO_ERRORS_HINT);
    clicked
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    recent_locations: Vec<PathBuf>,
    theme: Theme,
    map_focus: bool,
    ntfs_preview: bool,
    stable_updates: bool,
    text_scale: TextScale,
    reduced_motion: bool,
}
enum NativeResult {
    Attribution {
        generation: u64,
        root: PathBuf,
        attribution: Box<diskscope::identity::Attribution>,
    },
    ExportDestination(Option<PathBuf>),
    Export(Result<PathBuf, String>),
    Locations(Vec<native::Location>),
    Folder(Option<PathBuf>),
    Reveal(Result<(), String>),
    Volume {
        generation: u64,
        path: PathBuf,
        volume: Option<native::VolumeSpace>,
        measured_at: std::time::SystemTime,
    },
    ActionPresence {
        generation: u64,
        path: PathBuf,
        presence: outcome::Presence,
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
    session_policy: SessionPolicy,
    scan_started_utc: Option<u64>,
    scan_finished_utc: Option<u64>,
    export_open: bool,
    export_redacted: bool,
    export_path: String,
    export_busy: bool,
    export_native_path: Option<PathBuf>,
    incident_ticket: String,
    endpoint_label: String,
    attribution: Option<diskscope::identity::Attribution>,
    action_journal: journal::Journal,
    scan_feedback: accessibility::ScanFeedback,
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
    volume_observed_at: Option<std::time::SystemTime>,
    action_review: Option<outcome::ActionReview>,
    action_review_hidden: bool,
    volume_tx: SyncSender<(u64, PathBuf)>,
    attribution_tx: SyncSender<(u64, PathBuf)>,
    pending_attribution: Option<(u64, PathBuf)>,
    history: BrowseHistory,
    pending: Option<Arc<UiSnapshot>>,
    /// A browse position being looked up again in a rescan; see
    /// [`PendingRestore`]. While it is set, `request.focus_path` carries its
    /// current target so every view request keeps asking until it resolves.
    restore: Option<PendingRestore>,
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
    draft_path: Option<PathBuf>,
    preferences: Preferences,
    native_tx: SyncSender<NativeResult>,
    native_rx: Receiver<NativeResult>,
    picker_open: bool,
    locations: Vec<native::Location>,
    show_errors: bool,
    notice: Option<String>,
}
impl DiskScopeApp {
    pub fn new_session(
        cc: &eframe::CreationContext<'_>,
        initial_path: Option<PathBuf>,
        access_mode: AccessMode,
        policy: SessionPolicy,
    ) -> Self {
        Self::with_session_directory(
            cc,
            initial_path,
            scan_history::default_directory(),
            access_mode,
            policy,
        )
    }
    #[cfg(test)]
    fn with_history_directory(
        cc: &eframe::CreationContext<'_>,
        initial_path: Option<PathBuf>,
        history_directory: PathBuf,
        access_mode: AccessMode,
    ) -> Self {
        Self::with_session_directory(
            cc,
            initial_path,
            history_directory,
            access_mode,
            SessionPolicy::default(),
        )
    }
    fn with_session_directory(
        cc: &eframe::CreationContext<'_>,
        initial_path: Option<PathBuf>,
        history_directory: PathBuf,
        access_mode: AccessMode,
        session_policy: SessionPolicy,
    ) -> Self {
        let persistence_allowed =
            access_mode.persistence_allowed() && session_policy.persistence_allowed();
        let preferences = if persistence_allowed {
            cc.storage
                .and_then(|s| s.get_string(PREFERENCES_KEY))
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Preferences::default()
        };
        let (native_tx, native_rx) = mpsc::sync_channel(4);
        native::discover_locations(native_tx.clone());
        let snapshot = Arc::new(UiSnapshot::default());
        let volume_tx = native::volume_worker(native_tx.clone());
        let attribution_tx = native::attribution_worker(native_tx.clone());
        let history_service = if persistence_allowed {
            HistoryService::new(history_directory.clone())
        } else {
            HistoryService::unavailable(if !session_policy.persistence_allowed() {
                session_policy.label()
            } else {
                access_mode.history_message()
            })
        };
        let controller = if persistence_allowed {
            Controller::with_history(history_service.capture())
        } else {
            Controller::new()
        };
        let mut app = Self {
            session_policy,
            scan_started_utc: None,
            scan_finished_utc: None,
            export_open: false,
            export_redacted: true,
            export_path: String::new(),
            export_busy: false,
            export_native_path: None,
            incident_ticket: String::new(),
            endpoint_label: String::new(),
            attribution: None,
            action_journal: journal::Journal::default(),
            scan_feedback: accessibility::ScanFeedback::default(),
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
            volume_observed_at: None,
            action_review: None,
            action_review_hidden: false,
            volume_tx,
            attribution_tx,
            pending_attribution: None,
            history: BrowseHistory::default(),
            pending: None,
            restore: None,
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
            draft_path: None,
            preferences,
            native_tx,
            native_rx,
            picker_open: false,
            locations: Vec::new(),
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
    fn persistence_allowed(&self) -> bool {
        self.access_mode.persistence_allowed() && self.session_policy.persistence_allowed()
    }
    fn persistence_message(&self) -> &'static str {
        if !self.session_policy.persistence_allowed() {
            self.session_policy.label()
        } else {
            self.access_mode.history_message()
        }
    }
    /// Starts a new scan from the top: Overview at the scan root, no history.
    fn begin_scan(&mut self, path: PathBuf) {
        self.start_scan(path, None);
    }
    /// Rescans `root` after the file `removed` was acted on and keeps the
    /// user's place: the folder, workspace, sort, filters and search. Back and
    /// Forward still reset, because their entries hold node ids that the new
    /// scan reassigns.
    fn rescan_keeping_place(&mut self, root: PathBuf, removed: NodeId) {
        // The displayed view names its folder by path, but is only trusted
        // when it belongs to the scan the request is about.
        let view = (self.visible.view.generation == self.generation).then_some(&self.visible.view);
        let saved = save_browse(&self.request, view, &root, removed);
        self.start_scan(root, Some(saved));
    }
    fn start_scan(&mut self, path: PathBuf, keep: Option<SavedBrowse>) {
        if self.management.busy() {
            return;
        }
        let path = match std::path::absolute(path) {
            Ok(path) => path,
            Err(error) => {
                self.notice = Some(format!("Could not resolve this location: {error}"));
                return;
            }
        };
        // Absolute is lexical: resolving a junction here would bypass the
        // scanner's deliberate refusal to follow a link used as the root.
        let preserve_draft = keep.is_some() && self.path_edited;
        if let Some(review) = self.action_review.as_ref()
            && (review.presence.is_none()
                || review.state.is_none()
                || review.after_volume.is_none())
        {
            self.action_journal.interrupt(
                review.sequence,
                diskscope::evidence::utc_now_epoch_ms(),
                if review.presence.is_none() {
                    "Path verification interrupted before a result"
                } else {
                    review.verification()
                },
                review.logical_change(),
                review.volume_change(),
            );
        }
        if keep.is_none() {
            self.action_review = None;
            self.action_review_hidden = false;
        }
        self.root_path = path;
        if self.persistence_allowed() {
            self.preferences
                .recent_locations
                .retain(|p| p != &self.root_path);
            self.preferences
                .recent_locations
                .insert(0, self.root_path.clone());
            self.preferences.recent_locations.truncate(5);
        }
        if !preserve_draft {
            self.path_text = self.root_path.to_string_lossy().into_owned();
            self.path_edited = false;
            self.draft_path = None;
        }
        let (mut request, restore) = match keep {
            Some(saved) => (saved.request, saved.restore),
            None => (
                ViewRequest {
                    mode: ViewMode::Overview,
                    ..ViewRequest::default()
                },
                None,
            ),
        };
        request.limit = PAGE_SIZE;
        // The coordinator can finish a small new scan using its previous
        // request before submit_view sends this scan's focus path. Reserve a
        // new revision now so that terminal, unfocused answer cannot settle a
        // restore using node ids from the previous scan.
        request.revision = self.request.revision.wrapping_add(1);
        request.focus_path = restore
            .as_ref()
            .map(|restore| restore.focus_path().to_path_buf());
        self.request = request;
        self.restore = restore;
        self.history.clear();
        self.pending = None;
        self.changes = false;
        self.notice = None;
        self.generation = self.controller.start_limited(
            self.root_path.clone(),
            if self.preferences.ntfs_preview && !self.session_policy.helpdesk {
                BackendChoice::Ntfs
            } else {
                BackendChoice::Normal
            },
            if self.session_policy.helpdesk {
                diskscope::engine::ScanLimits::helpdesk()
            } else {
                diskscope::engine::ScanLimits::default()
            },
        );
        self.scan_started_utc = diskscope::evidence::utc_now_epoch_ms();
        self.scan_finished_utc = None;
        self.attribution = None;
        self.pending_attribution = Some((self.generation, self.root_path.clone()));
        self.submit_attribution();
        self.volume = None;
        self.volume_observed_at = None;
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
        self.submit_attribution();
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
            ) && !self.settle_focus(&next)
            {
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
                NativeResult::Attribution {
                    generation,
                    root,
                    attribution,
                } => {
                    if generation == self.generation && root == self.root_path {
                        self.attribution = Some(*attribution);
                    }
                }
                NativeResult::ExportDestination(path) => {
                    self.export_busy = false;
                    if let Some(path) = path {
                        self.export_path = path.to_string_lossy().into_owned();
                        self.export_native_path = Some(path);
                    }
                }
                NativeResult::Export(result) => {
                    self.export_busy = false;
                    self.notice = Some(match result {
                        Ok(path) => format!(
                            "Evidence exported to {}. Attach it to the correct ticket using your approved process.",
                            path.display()
                        ),
                        Err(error) => format!("Evidence export failed: {error}"),
                    });
                }
                NativeResult::Locations(locations) => self.locations = locations,
                NativeResult::Folder(path) => {
                    self.picker_open = false;
                    if let Some(path) = path {
                        self.path_text = path.to_string_lossy().into_owned();
                        self.draft_path = Some(path);
                        self.path_edited = true;
                    }
                }
                NativeResult::Volume {
                    generation,
                    path,
                    volume,
                    measured_at,
                } => {
                    if generation == self.generation && path == self.root_path {
                        self.volume = volume;
                        self.volume_observed_at = Some(measured_at);
                        if let Some(review) = self.action_review.as_mut()
                            && review.generation == generation
                            && review.root == path
                        {
                            review.after_volume = volume.map(|space| (space, measured_at));
                        }
                    }
                }
                NativeResult::ActionPresence {
                    generation,
                    path,
                    presence,
                } => {
                    if let Some(review) = self.action_review.as_mut()
                        && review.generation == generation
                        && review.path == path
                    {
                        review.presence = Some(presence);
                    }
                }
                NativeResult::Reveal(Err(message)) => self.notice = Some(message),
                NativeResult::Reveal(Ok(())) => {}
            }
        }
        if let Some(history) = self.history_service.poll() {
            self.history_snapshot = history;
        }
        self.poll_action_review(ctx);
        if let Some(review) = self.action_review.as_ref() {
            self.action_journal.reconcile(
                review.sequence,
                if review.presence.is_some() {
                    diskscope::evidence::utc_now_epoch_ms()
                } else {
                    None
                },
                review.verification(),
                review.logical_change(),
                review.volume_change(),
            );
        }
        if self.generation != 0
            && !self.snapshot.progress.state.is_active()
            && self.scan_finished_utc.is_none()
        {
            self.scan_finished_utc = diskscope::evidence::utc_now_epoch_ms();
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
    fn submit_attribution(&mut self) {
        if let Some(request) = self.pending_attribution.take() {
            match self.attribution_tx.try_send(request) {
                Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
                Err(mpsc::TrySendError::Full(request)) => self.pending_attribution = Some(request),
            }
        }
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
    /// Applies a one-shot path focus carried by the newest admitted snapshot.
    /// Returns true when the snapshot must not be displayed because the view
    /// has to be requested again around the place that was just found.
    fn settle_focus(&mut self, next: &UiSnapshot) -> bool {
        let Some(restore) = self.restore.as_mut() else {
            // A jump to one changed item resolves against a finished scan, so
            // the first admitted snapshot answers it either way.
            if self.request.focus_path.take().is_some() {
                self.request.root = next.view.root;
                self.request.selected = next.view.selected.as_ref().map(|row| row.id);
            }
            return false;
        };
        match restore.observe(&next.view, next.progress.state) {
            RestoreStep::Wait => false,
            RestoreStep::Retarget => {
                self.request.focus_path = Some(restore.focus_path().to_path_buf());
                self.dirty = true;
                false
            }
            RestoreStep::Land(landing) => {
                self.restore = None;
                self.request.focus_path = None;
                self.request.root = landing.root;
                self.request.selected = landing.selected;
                self.request.expanded.clear();
                self.request.offset = 0;
                if let Some(lost) = landing.notice {
                    // Keep the file action's own notice; the lost folder is
                    // extra information, not a replacement for it.
                    self.notice = Some(match self.notice.take() {
                        Some(existing) => format!("{existing} {lost}"),
                        None => lost,
                    });
                }
                // Always ask again: the worker keeps answering with the last
                // request it was given, focus path included, until a new one
                // replaces it. When `rebuild` is false this snapshot is already
                // consistent with the new request and is still displayed.
                self.dirty = true;
                landing.rebuild
            }
        }
    }
    /// Ends a pending restore because the user chose somewhere else.
    fn cancel_restore(&mut self) {
        if self.restore.take().is_some() {
            self.request.focus_path = None;
        }
    }
    fn action(&mut self, action: Action) {
        if self.management.busy() {
            return;
        }
        match action {
            Action::Select(id) => {
                // An explicit choice beats the remembered neighbour; the
                // folder is still restored.
                if let Some(restore) = self.restore.as_mut()
                    && restore.drop_selection()
                {
                    self.request.focus_path = Some(restore.focus_path().to_path_buf());
                }
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
                self.cancel_restore();
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
        if investigation::compact(ui.ctx()) {
            self.compact_header(ui, p);
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(if self.changes {
                    "Scan changes"
                } else {
                    match self.request.mode {
                        ViewMode::Overview => "Storage overview",
                        ViewMode::Explorer => "Folder explorer",
                        ViewMode::Largest => "Largest files",
                        ViewMode::Types => "File types",
                    }
                })
                .size(27.0)
                .strong(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Commands  Ctrl K").clicked() {
                    self.open_palette();
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("Access and permissions", |ui| self.access_controls(ui, p));
            self.evidence_controls(ui);
        });
        ui.label(
            RichText::new(self.session_policy.label())
                .small()
                .color(p.muted),
        );
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
                    .hint_text("Folder or drive path — or use Browse"),
            );
            if response.changed() {
                self.path_edited = true;
                self.draft_path = None;
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
                            .color(self.preferences.theme.on_accent()),
                    )
                    .fill(p.accent)
                    .min_size(egui::vec2(63.0, 33.0)),
                )
                .clicked();
            if scan || enter {
                let path = self.requested_path();
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
        if self.generation > 0 {
            self.scan_summary(ui, p);
        }
        self.notice_line(ui, p);
        ui.add_space(6.0);
        ui.separator();
    }
    /// The totals and status of the current scan. Every page and every scan
    /// state uses this one layout, so the header keeps its shape when a scan
    /// finishes or the workspace changes. The detail line is always laid out
    /// (blank when there is nothing to say) to keep the height constant.
    fn scan_summary(&mut self, ui: &mut egui::Ui, p: Palette) {
        let state = self.scan_state();
        let progress = &self.snapshot.progress;
        let errors_total = progress.errors_total;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(stop_status(state).unwrap_or(state.label())).color(p.accent));
            ui.separator();
            ui.label(RichText::new(format_bytes(progress.logical)).strong());
            let skipped = if progress.skipped > 0 {
                format!(" · {} skipped", format_count(progress.skipped))
            } else {
                String::new()
            };
            ui.label(
                RichText::new(format!(
                    "logical · {} files · {} folders · {} elapsed{skipped}",
                    format_count(progress.files),
                    format_count(progress.folders),
                    format_duration(progress.elapsed)
                ))
                .color(p.muted),
            );
            if errors_button(ui, errors_total) {
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
        let detail = status_detail(progress, state, self.restore.as_ref());
        ui.horizontal(|ui| {
            // A blank line still takes the height of a text line.
            let shown = if detail.is_empty() { " " } else { &detail };
            let line =
                ui.add(egui::Label::new(RichText::new(shown).small().color(p.muted)).truncate());
            if !detail.is_empty() {
                line.on_hover_text(&detail);
            }
        });
    }
    fn notice_line(&mut self, ui: &mut egui::Ui, p: Palette) {
        if let Some(notice) = self.notice.clone() {
            ui.horizontal_wrapped(|ui| {
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
        if investigation::compact(ui.ctx()) {
            ui.horizontal_wrapped(|ui| {
                let width = (ui.available_width() - 350.0).max(80.0);
                if ui
                    .add_sized(
                        [width, 30.0],
                        egui::TextEdit::singleline(&mut self.request.search)
                            .desired_width(width)
                            .hint_text("Search names…"),
                    )
                    .changed()
                {
                    self.search_changed = Some(Instant::now());
                }
                if ui.button("Filters").clicked() {
                    self.sync_filter_fields();
                    self.filter_open = true;
                }
                let old = self.request.sort;
                egui::ComboBox::from_id_salt("compact_sort")
                    .selected_text(sort_label(old))
                    .width(80.0)
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
                        if ui.button("Reverse order").clicked() {
                            self.request.descending = !self.request.descending;
                            self.mark_query_changed();
                        }
                    });
                let scope_changed = ui
                    .checkbox(&mut self.request.search_all, "Whole scan")
                    .changed();
                if old != self.request.sort || scope_changed {
                    self.mark_query_changed();
                }
            });
            return;
        }
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
        let mut row_focus = Vec::new();
        let mut column_widths = Vec::new();
        let mut column_minimums = controls::FILE_COLUMN_MIN_WIDTHS;
        let width = ui.available_width();
        let width_budget =
            width - ui.spacing().scroll.allocated_width() - 3.0 * ui.spacing().item_spacing.x;
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
        let keyboard_widths =
            controls::take_table_keyboard_widths(ui, "file_table").filter(|_| !width_changed);
        let mut table = TableBuilder::new(ui)
            .id_salt("file_table")
            .striped(false)
            .resizable(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .sense(Sense::CLICK)
            .min_scrolled_height(50.0)
            .max_scroll_height((height - 87.0).max(65.0));
        for (index, minimum) in controls::FILE_COLUMN_MIN_WIDTHS.iter().enumerate() {
            let column = if let Some(widths) = &keyboard_widths {
                Column::initial(widths[index])
            } else if index == 0 {
                Column::remainder()
            } else {
                Column::initial([0.0, 92.0, 90.0, 72.0][index])
            };
            table = table.column(column.at_least(*minimum));
        }
        // Saved resizable widths otherwise push the size columns outside the
        // clipped table after a window, inspector, or text-scale resize.
        if width_changed || keyboard_widths.is_some() {
            table.reset();
        }
        table
            .header(27.0, |mut header| {
                for (index, label) in [
                    "NAME",
                    "LOGICAL SIZE",
                    if view.row_scope_root != view.root {
                        "SCAN SHARE"
                    } else {
                        "FOLDER SHARE"
                    },
                    "FILES",
                ]
                .into_iter()
                .enumerate()
                {
                    let (used, _) = header.col(|ui| {
                        ui.label(RichText::new(label).small().color(p.faint));
                    });
                    column_minimums[index] = column_minimums[index].max(used.width());
                }
            })
            .body(|body| {
                column_widths = body.widths().to_vec();
                body.rows(29.0, view.rows.len(), |mut table_row| {
                    let row = &view.rows[table_row.index()];
                    table_row.set_selected(selected == Some(row.id));
                    let mut keyboard_control = None;
                    table_row.col(|ui| {
                        keyboard_control = Some(ui.interact(
                            ui.max_rect(),
                            ui.id().with("row_keyboard_control"),
                            Sense::click(),
                        ));
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
                    let (used, _) = table_row.col(|ui| {
                        ui.add(egui::Label::new(format_bytes(row.logical)).selectable(false));
                    });
                    column_minimums[1] = column_minimums[1].max(used.width());
                    let (used, _) = table_row.col(|ui| {
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
                    column_minimums[2] = column_minimums[2].max(used.width());
                    let (used, _) = table_row.col(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(format_count(row.files)).color(p.muted))
                                .selectable(false),
                        );
                    });
                    column_minimums[3] = column_minimums[3].max(used.width());
                    let response = keyboard_control.unwrap().union(table_row.response());
                    if response.has_focus() {
                        response
                            .ctx
                            .layer_painter(response.layer_id)
                            .with_clip_rect(response.interact_rect)
                            .rect_stroke(
                                response.rect,
                                2.0,
                                Stroke::new(1.0_f32, p.accent),
                                egui::StrokeKind::Inside,
                            );
                    }
                    row_focus.push((response.id, row.id, row.is_dir));
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            true,
                            selected == Some(row.id),
                            format!(
                                "{} · {} · {}{}",
                                row.name,
                                if row.is_dir { "folder" } else { "file" },
                                format_bytes(row.logical),
                                if row.has_children {
                                    if self.request.expanded.contains(&row.id) {
                                        " · expanded"
                                    } else {
                                        " · collapsed"
                                    }
                                } else {
                                    ""
                                }
                            ),
                        )
                    });
                    if response.clicked() {
                        response.request_focus();
                    }
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
        controls::describe_table_resize_handles(
            ui,
            "file_table",
            &[
                "Name",
                "Logical size",
                if view.row_scope_root != view.root {
                    "Scan share"
                } else {
                    "Folder share"
                },
                "Files",
            ],
            &column_widths,
            &column_minimums,
            width_budget,
            p,
        );
        if let Some(index) = row_focus
            .iter()
            .position(|(id, _, _)| ui.memory(|m| m.has_focus(*id)))
        {
            let (focus_id, node, is_dir) = row_focus[index];
            let next = ui.input_mut(|i| {
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    Some((index + 1).min(row_focus.len() - 1))
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    Some(index.saturating_sub(1))
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
                    Some(0)
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::End) {
                    Some(row_focus.len() - 1)
                } else {
                    None
                }
            });
            if let Some(next) = next {
                let (id, node, _) = row_focus[next];
                ui.memory_mut(|m| m.request_focus(id));
                action = Some(Action::Select(node));
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
                && is_dir
            {
                action = Some(Action::Navigate(node));
            } else if is_dir
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight))
            {
                if !self.request.expanded.contains(&node) {
                    action = Some(Action::Toggle(node));
                }
            } else if is_dir
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft))
            {
                if self.request.expanded.contains(&node) {
                    action = Some(Action::Toggle(node));
                }
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
                action = Some(Action::Select(node));
                ui.memory_mut(|m| m.request_focus(focus_id));
            }
        }
        if view.rows.is_empty() {
            self.empty_results(ui, p);
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
            .sense(Sense::CLICK)
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
                    let mut keyboard_control = None;
                    row.col(|ui| {
                        keyboard_control = Some(ui.interact(
                            ui.max_rect(),
                            ui.id().with("row_keyboard_control"),
                            Sense::click(),
                        ));
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
                    let response = keyboard_control.unwrap().union(row.response());
                    if response.has_focus() {
                        response
                            .ctx
                            .layer_painter(response.layer_id)
                            .with_clip_rect(response.interact_rect)
                            .rect_stroke(
                                response.rect,
                                2.0,
                                Stroke::new(1.0_f32, p.accent),
                                egui::StrokeKind::Inside,
                            );
                    }
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!(
                                "Explore {} · {} · {} files",
                                item.extension,
                                format_bytes(item.logical),
                                format_count(item.files)
                            ),
                        )
                    });
                    if response.clicked()
                        || (response.has_focus()
                            && response.ctx.input(|i| {
                                i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)
                            }))
                    {
                        extension_lens = Some(item.extension.clone());
                    }
                });
            });
        if let Some(extension) = extension_lens {
            self.set_extension_lens(&extension);
        }
        if view.types.is_empty() {
            self.empty_results(ui, p);
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
                self.empty_message(true),
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
        if !self.show_errors {
            return;
        }
        controls::remember_dialog_focus(ctx, "Scan errors & skipped locations");
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
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for error in &progress.errors {
                        ui.add(
                            egui::Label::new(RichText::new(error.path.to_string_lossy()).strong())
                            .wrap(),
                        )
                        .on_hover_text(error.path.to_string_lossy());
                        ui.label(RichText::new(&error.message).small().color(p.muted));
                        if ui.button("Copy error and path").clicked() { ui.ctx().copy_text(format!("{}\n{}", error.path.display(), error.message)); }
                    }
                });
                if progress.errors.is_empty() {
                    ui.label("No error samples.");
                }
            });
        if !self.show_errors {
            controls::restore_dialog_focus(ctx, "Scan errors & skipped locations");
        }
    }
}
impl eframe::App for DiskScopeApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // eframe runs logic even when a minimized window does not paint.
        self.poll(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // The handle is only available on the event-loop thread, which is where
        // ui() runs. Windows parents its own prompts to it during Recycle.
        if self.management.owner.is_none() {
            self.management.owner = management::native_window_id(frame);
        }
        let ctx = ui.ctx().clone();
        if !self.management.busy() {
            if !self.transient_dialog_open() {
                self.file_action_keyboard(&ctx);
            }
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
        if self.transient_dialog_open() {
            // Capture before disabling the invoker, since egui surrenders
            // keyboard focus when a disabled widget is rendered.
            self.capture_dialog_invokers(&ctx);
            ui.disable();
        }
        if investigation::compact(&ctx) {
            self.compact_navigation(ui, p);
        } else {
            self.sidebar(ui, p);
        }
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(p.side)
                    .inner_margin(egui::Margin::symmetric(18, 6)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    self.accessible_scan_status(ui, p);
                    ui.label(
                        RichText::new(if self.snapshot.progress.backend.is_empty() {
                            "Normal folder scan"
                        } else {
                            &self.snapshot.progress.backend
                        })
                        .small()
                        .color(p.faint),
                    );
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
                self.workspace(ui, p);
            });
        if !self.management.busy() {
            self.error_window(&ctx, p);
            self.filter_window(&ctx, p);
            self.command_palette(&ctx, p);
            self.settings_window(&ctx, p);
            self.help_window(&ctx, p);
            self.comparison_window(&ctx, p);
            self.export_window(&ctx);
        }
        self.management_dialog(&ctx, p);
        controls::finish_dialog_frame(&ctx);
        self.submit_view();
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if !self.persistence_allowed() {
            return;
        }
        if let Ok(json) = serde_json::to_string(&self.preferences) {
            storage.set_string(PREFERENCES_KEY, json);
        }
    }
    fn persist_egui_memory(&self) -> bool {
        self.persistence_allowed()
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
/// A figure with a caption, used by the Changes page.
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
    use super::test_support::{rendered_texts, settle_frames};

    #[test]
    fn choosing_a_draft_folder_keeps_the_scanned_root() {
        let (mut app, ctx) = table_fixture();
        let scanned = PathBuf::from(r"C:\scan-a");
        let draft = PathBuf::from(r"C:\scan-b");
        app.root_path = scanned.clone();
        app.native_tx
            .send(NativeResult::Folder(Some(draft.clone())))
            .unwrap();
        app.poll(&ctx);
        assert_eq!(app.root_path, scanned);
        assert_eq!(app.path_text, draft.to_string_lossy());
        assert!(app.path_edited);
    }

    #[test]
    fn filtered_empty_results_offer_recovery_instead_of_claiming_an_empty_folder() {
        let (mut app, ctx) = table_fixture();
        app.request.filters.min_bytes = Some(1000);
        Arc::make_mut(&mut app.visible).view.rows.clear();
        let output = table_frame(&mut app, &ctx, vec![]);
        let texts = rendered_texts(&output.shapes);
        assert!(texts.iter().any(|s| s == "No matches in this scope"));
        assert!(texts.iter().any(|s| s == "Clear search and filters"));
    }

    #[test]
    fn completed_empty_map_does_not_promise_more_discovery() {
        let (mut app, ctx) = table_fixture();
        Arc::make_mut(&mut app.visible).view.map.clear();
        let p = app.preferences.theme.palette();
        let output = settle_frames(&ctx, |ui| app.treemap(ui, 250.0, p));
        let texts = rendered_texts(&output.shapes);
        assert!(
            !texts
                .iter()
                .any(|s| s.contains("as file sizes are discovered"))
        );
        assert!(texts.iter().any(|s| s.contains("No drawable file sizes")));
    }

    #[test]
    fn compact_workspace_keeps_identification_and_size_inside_minimum_viewport() {
        use eframe::App as _;
        for factor in [1.0, 1.25, 1.5] {
            let (mut app, ctx) = table_fixture();
            let screen =
                Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0 / factor, 640.0 / factor));
            let mut output = egui::FullOutput::default();
            for _ in 0..3 {
                output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()),
                );
            }
            for label in ["NAME", "LOGICAL SIZE", "demo.mp4"] {
                assert!(
                    rendered_texts(&output.shapes).iter().any(|s| s == label),
                    "Missing {label} at {factor}: {:?}",
                    rendered_texts(&output.shapes)
                );
                let point = text_center(&output.shapes, label);
                assert!(
                    screen.contains(point),
                    "{label} outside compact viewport at {factor}: {point:?}"
                );
            }
            assert!(
                !rendered_texts(&output.shapes)
                    .iter()
                    .any(|s| s == "CURRENT SCAN")
            );
        }
    }

    #[test]
    fn compact_pending_results_keep_an_apply_control() {
        use eframe::App as _;
        let (mut app, ctx) = table_fixture();
        app.preferences.stable_updates = true;
        app.pending = Some(app.visible.clone());
        let output = settle_frames(&ctx, |ui| {
            ui.set_max_width(600.0);
            app.ui(ui, &mut eframe::Frame::_new_kittest());
        });
        assert!(
            rendered_texts(&output.shapes)
                .iter()
                .any(|s| s == "Apply update")
        );
    }

    #[test]
    fn compact_outcome_is_a_dismissible_overlay_and_does_not_displace_the_file_list() {
        use eframe::App as _;
        let (mut app, ctx) = table_fixture();
        app.action_review = Some(outcome::ActionReview {
            sequence: 1,
            path: "removed.bin".into(),
            root: app.root_path.clone(),
            generation: app.generation,
            message: "Moved to Recycle Bin".into(),
            recycled: true,
            before_logical: Some(100),
            before_backend: "Normal".into(),
            after_logical: Some(20),
            before_volume: None,
            after_volume: None,
            state: Some(ScanState::Complete),
            presence: Some(outcome::Presence::Absent),
            probe_started: true,
            volume_refresh_requested: true,
        });
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 640.0 / 1.5));
        let frame = |app: &mut DiskScopeApp, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()),
            )
        };
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        let output = frame(&mut app, vec![]);
        let point = text_center(&output.shapes, "Action outcome · verification");
        assert!(screen.contains(text_center(&output.shapes, "demo.mp4")));
        frame(&mut app, vec![egui::Event::PointerMoved(point)]);
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        let output = frame(&mut app, vec![]);
        assert!(
            rendered_texts(&output.shapes)
                .iter()
                .any(|s| s == "Path absence verified")
        );
        frame(
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let output = frame(&mut app, vec![]);
        assert!(screen.contains(text_center(&output.shapes, "demo.mp4")));
    }

    #[test]
    fn arrow_navigation_selects_the_next_row_without_changing_scope() {
        let (mut app, ctx) = table_fixture();
        let output = table_frame(&mut app, &ctx, vec![]);
        let point = text_center(&output.shapes, "demo.mp4");
        table_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        table_frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let output = table_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::ArrowDown,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let _ = output;
        assert_eq!(app.request.selected, Some(2));
        assert_eq!(app.request.root, 0);
    }

    #[test]
    fn relative_scan_root_is_made_absolute_without_resolving_links() {
        let (mut app, _) = table_fixture();
        app.begin_scan(PathBuf::from("research/diskscope-ui-fixture"));
        assert!(app.root_path.is_absolute());
        assert_eq!(
            app.root_path,
            std::env::current_dir()
                .unwrap()
                .join("research/diskscope-ui-fixture")
        );
    }

    #[cfg(windows)]
    #[test]
    fn picked_native_path_survives_its_lossy_display() {
        use std::os::windows::ffi::OsStringExt;
        let (mut app, ctx) = table_fixture();
        let path = PathBuf::from(std::ffi::OsString::from_wide(&[67, 58, 92, 0xd800]));
        app.native_tx
            .send(NativeResult::Folder(Some(path.clone())))
            .unwrap();
        app.poll(&ctx);
        assert_eq!(app.requested_path(), path);
        app.begin_scan(app.requested_path());
        assert_eq!(app.root_path, path);
    }

    #[test]
    fn compact_keyboard_can_advance_more_than_one_row() {
        use eframe::App as _;
        let (mut app, ctx) = table_fixture();
        let mut third = app.visible.view.rows[0].clone();
        third.id = 999;
        third.name = "third.bin".into();
        Arc::make_mut(&mut app.visible).view.rows.push(third);
        let frame = |app: &mut DiskScopeApp, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()),
            )
        };
        frame(&mut app, vec![]);
        let output = frame(&mut app, vec![]);
        let point = text_center(&output.shapes, "demo.mp4");
        frame(&mut app, vec![egui::Event::PointerMoved(point)]);
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        for expected in [2, 999] {
            frame(
                &mut app,
                vec![egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            frame(
                &mut app,
                vec![egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert_eq!(app.request.selected, Some(expected));
        }
    }
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
    fn welcome_action_stays_visible_at_large_text_sizes() {
        use eframe::App as _;
        for scale in [TextScale::Normal, TextScale::Large, TextScale::Largest] {
            let (mut app, ctx) = table_fixture();
            app.generation = 0;
            app.request.mode = ViewMode::Overview;
            app.preferences.text_scale = scale;
            app.apply_appearance(&ctx);
            let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
            let mut output = egui::FullOutput::default();
            for _ in 0..3 {
                output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        app.ui(ui, &mut eframe::Frame::_new_kittest());
                    },
                );
            }
            let action = text_center(&output.shapes, "Choose a folder…");
            assert!(
                screen.contains(action),
                "Primary action outside window at {scale:?}"
            );
            assert!(action.y > text_center(&output.shapes, "See where your space goes.").y);
        }
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
    fn export_opening_frame_blocks_file_action_shortcuts() {
        use eframe::App as _;
        let eligible_file = || {
            let (mut app, ctx) = table_fixture();
            app.request.selected = Some(1);
            let file = app
                .visible
                .view
                .rows
                .iter()
                .find(|row| row.id == 1)
                .unwrap()
                .clone();
            assert!(!file.is_dir);
            Arc::make_mut(&mut app.visible).view.selected = Some(file);
            app.dirty = false;
            (app, ctx)
        };
        let (mut app, ctx) = eligible_file();
        app.export_open = true;
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Delete,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::SHIFT,
                }],
                ..Default::default()
            },
            |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()),
        );
        assert!(
            !app.management.busy(),
            "Delete must not prepare a background file action while exporting"
        );
        assert!(app.export_open);
        let (mut app, ctx) = eligible_file();
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Delete,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::SHIFT,
                }],
                ..Default::default()
            },
            |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()),
        );
        assert!(
            app.management.busy(),
            "the same selected file and shortcut must be eligible outside the dialog"
        );
    }

    #[test]
    fn keyboard_splitter_resize_changes_rendered_width_and_preserves_focus() {
        let (mut app, ctx) = table_fixture();
        ctx.enable_accesskit();
        for _ in 0..3 {
            table_frame(&mut app, &ctx, vec![]);
        }
        let mut splitter = None;
        for _ in 0..24 {
            let output = table_frame(
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
            if let Some(focused) = ctx.memory(|m| m.focused()) {
                let update = output.platform_output.accesskit_update.unwrap();
                if update.nodes.iter().any(|(id, node)| {
                    *id == focused.accesskit_id()
                        && node.label() == Some("Resize Logical size column")
                }) {
                    splitter = Some(focused);
                    break;
                }
            }
        }
        let splitter = splitter.expect("logical-size splitter must be reachable by Tab");
        let before = table_frame(&mut app, &ctx, vec![]);
        let share_before = text_center(&before.shapes, "FOLDER SHARE").x;
        let files_before = text_center(&before.shapes, "FILES").x;
        table_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let after = table_frame(&mut app, &ctx, vec![]);
        assert!(
            text_center(&after.shapes, "FOLDER SHARE").x > share_before + 4.0,
            "Right must enlarge the real rendered logical-size column"
        );
        assert!(
            (text_center(&after.shapes, "FILES").x - files_before).abs() < 1.0,
            "an interior resize transfers adjacent width without overflowing the table"
        );
        assert_eq!(ctx.memory(|m| m.focused()), Some(splitter));
        let update = after.platform_output.accesskit_update.unwrap();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == splitter.accesskit_id())
            .unwrap();
        let value = node
            .numeric_value()
            .expect("splitter exposes its real width");
        assert!(
            value >= node.min_numeric_value().unwrap()
                && value <= node.max_numeric_value().unwrap()
        );
        assert!(node.description().unwrap().contains("Left"));
        // The reverse key shrinks the rendered column; either direction can
        // take a shorter final step at the column's content minimum.
        table_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::ArrowLeft,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let restored = table_frame(&mut app, &ctx, vec![]);
        assert!(
            text_center(&restored.shapes, "FOLDER SHARE").x
                < text_center(&after.shapes, "FOLDER SHARE").x - 4.0
        );
        for direction in [egui::Key::ArrowLeft, egui::Key::ArrowRight] {
            for _ in 0..80 {
                table_frame(
                    &mut app,
                    &ctx,
                    vec![egui::Event::Key {
                        key: direction,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
                let output = table_frame(&mut app, &ctx, vec![]);
                let update = output.platform_output.accesskit_update.unwrap();
                let (_, node) = update
                    .nodes
                    .iter()
                    .find(|(id, _)| *id == splitter.accesskit_id())
                    .unwrap();
                assert!(
                    node.numeric_value().unwrap() >= 72.0,
                    "logical width must respect its original minimum"
                );
                assert!(node.numeric_value().unwrap() <= node.max_numeric_value().unwrap() + 0.1);
                assert!(
                    (text_center(&output.shapes, "FILES").x - files_before).abs() < 1.0,
                    "repeated resizing cannot push the last column out of the table"
                );
                assert_eq!(ctx.memory(|m| m.focused()), Some(splitter));
            }
        }
        // A queued resize based on the old viewport cannot override the
        // existing reset that keeps all columns visible after a resize.
        table_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::ArrowLeft,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let resized = table_frame_at_width(&mut app, &ctx, vec![], 900.0);
        let (mut fresh_app, fresh_ctx) = table_fixture();
        for _ in 0..3 {
            table_frame_at_width(&mut fresh_app, &fresh_ctx, vec![], 900.0);
        }
        let fresh = table_frame_at_width(&mut fresh_app, &fresh_ctx, vec![], 900.0);
        for label in ["LOGICAL SIZE", "FOLDER SHARE", "FILES"] {
            assert!(
                (text_center(&resized.shapes, label).x - text_center(&fresh.shapes, label).x).abs()
                    < 1.0,
                "viewport reset must win over queued keyboard widths for {label}"
            );
        }
        // Public reset/initial updates must leave native pointer resizing intact.
        for _ in 0..2 {
            table_frame_at_width(&mut app, &ctx, vec![], 900.0);
        }
        let point = ctx.read_response(splitter).unwrap().rect.center();
        table_frame_at_width(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(point)],
            900.0,
        );
        table_frame_at_width(
            &mut app,
            &ctx,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            900.0,
        );
        let dragged = point + egui::vec2(12.0, 0.0);
        table_frame_at_width(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(dragged)],
            900.0,
        );
        table_frame_at_width(
            &mut app,
            &ctx,
            vec![egui::Event::PointerButton {
                pos: dragged,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            900.0,
        );
        let pointer_resized = table_frame_at_width(&mut app, &ctx, vec![], 900.0);
        assert!(
            text_center(&pointer_resized.shapes, "FOLDER SHARE").x
                > text_center(&resized.shapes, "FOLDER SHARE").x + 4.0,
            "pointer drag must still resize the real column after keyboard resizing"
        );
    }

    #[test]
    fn every_table_splitter_honors_keyboard_minimum_and_table_fit() {
        for (label, minimum) in [
            ("Resize Name column", 150.0),
            ("Resize Logical size column", 72.0),
            ("Resize Folder share column", 72.0),
            ("Resize Files column", 50.0),
        ] {
            let (mut app, ctx) = table_fixture();
            ctx.enable_accesskit();
            for _ in 0..3 {
                table_frame(&mut app, &ctx, vec![]);
            }
            let mut splitter = None;
            for _ in 0..24 {
                let output = table_frame(
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
                            *id == focused.accesskit_id() && node.label() == Some(label)
                        })
                {
                    splitter = Some(focused);
                    break;
                }
            }
            let splitter = splitter.expect("named splitter must be keyboard reachable");
            for direction in [egui::Key::ArrowLeft, egui::Key::ArrowRight] {
                for _ in 0..80 {
                    table_frame(
                        &mut app,
                        &ctx,
                        vec![egui::Event::Key {
                            key: direction,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                    let output = table_frame(&mut app, &ctx, vec![]);
                    let update = output.platform_output.accesskit_update.unwrap();
                    let (_, node) = update
                        .nodes
                        .iter()
                        .find(|(id, _)| *id == splitter.accesskit_id())
                        .unwrap();
                    let value = node.numeric_value().unwrap();
                    assert!(value >= minimum, "{label} must honor its minimum");
                    assert!(value <= node.max_numeric_value().unwrap() + 0.1);
                    for (_, handle) in update
                        .nodes
                        .iter()
                        .filter(|(_, n)| n.role() == egui::accesskit::Role::Splitter)
                    {
                        assert!(
                            handle.bounds().unwrap().x1 <= 1000.0,
                            "{label} must keep every column inside the viewport"
                        );
                    }
                    assert_eq!(ctx.memory(|m| m.focused()), Some(splitter));
                }
            }
        }
    }

    #[test]
    fn tab_traversal_skips_static_table_headers_and_unnamed_cells() {
        for types in [false, true] {
            let (mut app, ctx) = table_fixture();
            ctx.enable_accesskit();
            if types {
                app.request.mode = ViewMode::Types;
                Arc::make_mut(&mut app.visible).view.types = vec![TypeStat {
                    extension: ".mp4".into(),
                    logical: 100,
                    files: 1,
                }];
            }
            for _ in 0..3 {
                table_frame(&mut app, &ctx, vec![]);
            }
            let expected_rows = if types {
                1
            } else {
                app.visible.view.rows.len()
            };
            let mut row_controls = std::collections::HashSet::new();
            for _ in 0..24 {
                let output = table_frame(
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
                let Some(focused) = ctx.memory(|m| m.focused()) else {
                    continue;
                };
                let update = output.platform_output.accesskit_update.unwrap();
                let (_, node) = update
                    .nodes
                    .iter()
                    .find(|(id, _)| *id == focused.accesskit_id())
                    .expect("focused table control must exist in accessibility tree");
                assert!(
                    !matches!(node.role(), egui::accesskit::Role::GenericContainer),
                    "Tab must skip static header cells and anonymous body cells (types={types})"
                );
                let label = node.label().or(node.value()).unwrap_or("");
                assert!(
                    !label.is_empty()
                        || !node.labelled_by().is_empty()
                        || node.placeholder().is_some(),
                    "every focus stop must have an accessible name: {node:?}"
                );
                if label.contains("demo.mp4")
                    || label.contains("folder ·")
                    || label.contains("Explore .mp4")
                {
                    row_controls.insert(focused);
                }
            }
            assert_eq!(
                row_controls.len(),
                expected_rows,
                "one named keyboard control per row"
            );
        }
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

    /// Lays the header out until egui's layout has settled and returns the
    /// vertical position just below it.
    fn settled_header_bottom(app: &mut DiskScopeApp, ctx: &egui::Context) -> f32 {
        let mut bottom = 0.0;
        for _ in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1440.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show_inside(ui, |ui| {
                        app.header(ui, app.preferences.theme.palette());
                        bottom = ui.cursor().top();
                    });
                },
            );
        }
        bottom
    }

    fn set_progress(app: &mut DiskScopeApp, state: ScanState, message: &str) {
        let snapshot = Arc::make_mut(&mut app.snapshot);
        snapshot.progress.state = state;
        snapshot.progress.message = message.into();
        snapshot.progress.current_path = PathBuf::from(r"C:\fixture\deeply\nested\folder");
        snapshot.progress.files = 12_345;
        snapshot.progress.folders = 678;
        snapshot.progress.logical = 9_876_543_210;
    }

    #[test]
    fn header_keeps_one_height_for_every_page_and_scan_state() {
        let (mut app, ctx) = table_fixture();
        set_progress(&mut app, ScanState::Complete, "");
        app.request.mode = ViewMode::Overview;
        let reference = settled_header_bottom(&mut app, &ctx);
        assert!(reference > 0.0);
        for changes in [false, true] {
            for mode in [
                ViewMode::Overview,
                ViewMode::Explorer,
                ViewMode::Largest,
                ViewMode::Types,
            ] {
                for (state, message) in [
                    (
                        ScanState::Idle,
                        "Choose a folder to see what uses its space",
                    ),
                    (ScanState::Preparing, "Waiting for an available worker."),
                    (ScanState::Scanning, "Reading folders in the background"),
                    (ScanState::Finalizing, ""),
                    (ScanState::Cancelling, ""),
                    (
                        ScanState::Cancelled,
                        "Scan cancelled; results are incomplete.",
                    ),
                    (ScanState::Complete, ""),
                    (ScanState::Partial, "Scan finished with partial results."),
                    (
                        ScanState::Failed,
                        "Root could not be read; results are incomplete.",
                    ),
                ] {
                    app.changes = changes;
                    app.request.mode = mode;
                    set_progress(&mut app, state, message);
                    let bottom = settled_header_bottom(&mut app, &ctx);
                    assert_eq!(
                        bottom, reference,
                        "header height moved for {mode:?} {state:?} changes={changes}"
                    );
                }
            }
        }
    }

    fn header_frame(
        app: &mut DiskScopeApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    app.header(ui, app.preferences.theme.palette());
                });
            },
        )
    }

    fn header_click(app: &mut DiskScopeApp, ctx: &egui::Context, label: &str) {
        header_frame(app, ctx, vec![]);
        let position = text_center(&header_frame(app, ctx, vec![]).shapes, label);
        header_frame(app, ctx, vec![egui::Event::PointerMoved(position)]);
        for pressed in [true, false] {
            header_frame(
                app,
                ctx,
                vec![egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
    }

    #[test]
    fn errors_button_does_nothing_without_errors_and_opens_the_list_with_them() {
        let (mut app, ctx) = table_fixture();
        assert_eq!(app.snapshot.progress.errors_total, 0);
        header_click(&mut app, &ctx, "0 errors");
        assert!(!app.show_errors, "an empty list must not open");
        // Negative control: the same click opens the list once there is
        // something in it, so the check above is about the count.
        Arc::make_mut(&mut app.snapshot).progress.errors_total = 1;
        header_click(&mut app, &ctx, "1 error");
        assert!(app.show_errors);
    }

    #[test]
    fn status_bar_carries_facts_not_a_permanent_preview_banner() {
        use eframe::App as _;

        let (mut app, ctx) = table_fixture();
        let mut frame = eframe::Frame::_new_kittest();
        let output = settle_frames(&ctx, |ui| app.ui(ui, &mut frame));
        let texts = rendered_texts(&output.shapes);
        assert!(texts.iter().any(|text| text == "Normal folder scan"));
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("performance validation")),
            "{texts:?}"
        );
    }

    #[test]
    fn detail_line_names_the_restore_the_work_and_the_failure() {
        let progress = Progress {
            message: "Reading folders in the background".into(),
            current_path: PathBuf::from("/scan/a"),
            ..Default::default()
        };
        assert_eq!(
            status_detail(&progress, ScanState::Scanning, None),
            "Reading folders in the background · /scan/a"
        );
        // Terminal and healthy: nothing to add.
        assert_eq!(status_detail(&progress, ScanState::Complete, None), "");
        assert_eq!(status_detail(&progress, ScanState::Idle, None), "");
        for state in [ScanState::Partial, ScanState::Failed] {
            assert_eq!(
                status_detail(&progress, state, None),
                "Reading folders in the background"
            );
        }
        let restore = PendingRestore::new(
            std::path::Path::new("/scan"),
            std::path::Path::new("/scan/a"),
            None,
        )
        .unwrap();
        let detail = status_detail(&progress, ScanState::Scanning, Some(&restore));
        assert!(
            detail.starts_with("Restoring your place in a once the scan reaches it. · Reading"),
            "{detail}"
        );
    }

    /// A model mid-scan: `/scan/a` has been listed but `/scan/a/b` has not.
    fn scan_model(with_b: bool) -> Model {
        let directory = |name: &str| Entry {
            name: name.into(),
            is_dir: true,
            reparse: false,
            logical: 0,
            allocated: Some(0),
            modified: None,
            identity: None,
        };
        let mut model = Model::new(PathBuf::from("/scan"));
        let a = model.insert_batch(0, vec![directory("a")])[0];
        if with_b {
            model.insert_batch(a, vec![directory("b")]);
        }
        model
    }

    /// What the worker publishes for the app's current request.
    fn snapshot_for(app: &DiskScopeApp, model: &Model, state: ScanState) -> UiSnapshot {
        let mut snapshot = UiSnapshot::default();
        snapshot.progress.generation = app.generation;
        snapshot.progress.state = state;
        snapshot.view = diskscope::view::build_view(model, &app.request, app.generation);
        snapshot
    }

    fn restoring_app(folder: &str, select: Option<&str>) -> DiskScopeApp {
        let (mut app, _ctx) = table_fixture();
        let restore = PendingRestore::new(
            std::path::Path::new("/scan"),
            std::path::Path::new(folder),
            select.map(PathBuf::from),
        )
        .unwrap();
        app.request.mode = ViewMode::Largest;
        app.request.focus_path = Some(restore.focus_path().to_path_buf());
        app.restore = Some(restore);
        app.dirty = false;
        app
    }

    #[test]
    fn restoring_rescan_rejects_terminal_view_built_before_its_focus_request() {
        let directory = tempfile::tempdir().unwrap();
        let (mut app, _ctx) = table_fixture();
        let previous_request = app.request.clone();
        let neighbour = directory.path().join("neighbour.bin");
        let restore =
            PendingRestore::new(directory.path(), directory.path(), Some(neighbour.clone()))
                .unwrap();
        app.start_scan(
            directory.path().to_path_buf(),
            Some(SavedBrowse {
                request: previous_request.clone(),
                restore: Some(restore),
            }),
        );
        // A small scan can finish before the frame submits its new view. The
        // coordinator still has the old request, whose node ids are invalid in
        // this generation, and can publish a terminal answer for that request.
        let mut model = Model::new(directory.path().to_path_buf());
        model.insert_batch(
            0,
            vec![Entry {
                name: "neighbour.bin".into(),
                is_dir: false,
                reparse: false,
                logical: 1,
                allocated: Some(1),
                modified: None,
                identity: None,
            }],
        );
        let unfocused = diskscope::view::build_view(&model, &previous_request, app.generation);
        assert!(unfocused.selected.is_none());
        assert!(
            !admits_snapshot(
                app.generation,
                app.request.revision,
                unfocused.generation,
                unfocused.revision
            ),
            "an unfocused previous request must not tell a terminal restore its neighbour is absent"
        );
        assert_eq!(app.restore.as_ref().unwrap().focus_path(), neighbour);
        app.submit_view();
        let focused = snapshot_for(&app, &model, ScanState::Complete);
        assert!(admits_snapshot(
            app.generation,
            app.request.revision,
            focused.view.generation,
            focused.view.revision
        ));
        assert!(!app.settle_focus(&focused));
        assert!(app.restore.is_none());
        assert_eq!(app.request.selected, Some(1));
    }

    #[test]
    fn restore_keeps_asking_across_snapshots_until_the_folder_exists() {
        let mut app = restoring_app("/scan/a/b", None);
        // The scan has not reached `b`: snapshot after snapshot, nothing moves.
        for _ in 0..3 {
            let snapshot = snapshot_for(&app, &scan_model(false), ScanState::Scanning);
            assert!(!app.settle_focus(&snapshot), "the snapshot is displayed");
            assert!(app.restore.is_some());
            assert_eq!(app.request.focus_path, Some(PathBuf::from("/scan/a/b")));
            assert_eq!(app.request.root, 0);
            assert!(!app.dirty);
        }
        // The scan reaches it: land on the folder and ask for it to be built.
        let snapshot = snapshot_for(&app, &scan_model(true), ScanState::Scanning);
        assert!(
            app.settle_focus(&snapshot),
            "the parent's view is not shown"
        );
        assert!(app.restore.is_none());
        assert_eq!(app.request.focus_path, None);
        assert_eq!(app.request.root, 2, "b is the third node: root, a, b");
        assert_eq!(app.request.selected, None);
        assert_eq!(app.request.mode, ViewMode::Largest);
        assert!(app.dirty);
    }

    #[test]
    fn a_folder_lost_when_the_scan_stops_is_announced_beside_the_action_notice() {
        let mut app = restoring_app("/scan/gone", None);
        app.notice = Some("Moved to Recycle Bin: /scan/gone/x.bin.".into());
        let snapshot = snapshot_for(&app, &scan_model(false), ScanState::Complete);
        assert!(app.settle_focus(&snapshot));
        assert!(app.restore.is_none());
        assert_eq!(app.request.root, 0);
        let notice = app.notice.as_deref().unwrap();
        assert!(notice.starts_with("Moved to Recycle Bin: /scan/gone/x.bin. gone is no longer"));
        assert!(notice.ends_with("opened the scan root."), "{notice}");
    }

    #[test]
    fn a_missing_neighbour_retargets_the_request_to_the_folder() {
        let mut app = restoring_app("/scan/a", Some("/scan/a/x.bin"));
        let snapshot = snapshot_for(&app, &scan_model(false), ScanState::Complete);
        assert!(!app.settle_focus(&snapshot));
        assert!(app.restore.is_some(), "the folder is still wanted");
        assert_eq!(app.request.focus_path, Some(PathBuf::from("/scan/a")));
        assert!(app.dirty, "the new target has to be requested");
    }

    #[test]
    fn choosing_somewhere_else_cancels_the_restore_instead_of_yanking_the_user_back() {
        // Navigating away ends it, and no focus is left in the request.
        let mut app = restoring_app("/scan/a/b", None);
        app.action(Action::Navigate(1));
        assert!(app.restore.is_none());
        assert_eq!(app.request.focus_path, None);
        assert_eq!(app.request.root, 1);
        // Selecting something keeps the folder but drops the neighbour.
        let mut app = restoring_app("/scan/a", Some("/scan/a/x.bin"));
        app.action(Action::Select(1));
        assert!(app.restore.is_some());
        assert_eq!(app.request.focus_path, Some(PathBuf::from("/scan/a")));
        assert_eq!(app.request.selected, Some(1));
        // Jumping to a changed item uses the same focus and so replaces it.
        let mut app = restoring_app("/scan/a/b", None);
        app.focus_changed_item(PathBuf::from("/scan/elsewhere"));
        assert!(app.restore.is_none());
        assert_eq!(
            app.request.focus_path,
            Some(PathBuf::from("/scan/elsewhere"))
        );
    }

    #[test]
    fn a_new_scan_continues_the_request_revision_instead_of_restarting_it() {
        let (mut app, _ctx) = table_fixture();
        app.request.revision = 41;
        let previous_revision = app.request.revision;
        app.begin_scan(PathBuf::from("/scan-that-does-not-exist"));
        assert!(app.request.revision > previous_revision);
        // A new-generation answer may still carry the coordinator's old view
        // revision before the first submit. It must already be inadmissible.
        assert!(!admits_snapshot(
            app.generation,
            app.request.revision,
            app.generation,
            previous_revision,
        ));
        let reserved_revision = app.request.revision;
        app.submit_view();
        assert!(app.request.revision > reserved_revision);
    }

    #[test]
    fn a_pending_focus_is_never_saved_into_back_and_forward() {
        let app = restoring_app("/scan/a/b", None);
        assert!(app.request.focus_path.is_some());
        assert_eq!(app.browse_context().request.focus_path, None);
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
