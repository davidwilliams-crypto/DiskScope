use super::*;
use diskscope::{
    admin::{ElevationKind, ElevationOutcome},
    file_actions::{self, ActionError, ActionOutcome, RecycleBlock, RecyclePlan, RecycleVerdict},
};
use std::num::NonZeroIsize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AccessMode {
    Standard,
    /// Elevated through UAC consent; a standard window of the same user exists.
    Admin,
    /// Elevated without a UAC split (UAC off, or an account Windows exempts
    /// from Admin Approval Mode). The policy is the same as for `Admin`; only
    /// the explanation differs, because there is no standard window to open.
    AlwaysAdmin,
    Unknown(String),
}

/// Why an always-elevated session cannot save anything or use the Recycle Bin.
/// Shown in the access tooltip and the settings window.
const ALWAYS_ADMIN_EXPLANATION: &str = concat!(
    "This account always runs with full administrator rights: UAC is turned off, ",
    "or Windows does not apply Admin Approval Mode to it ",
    "(for example the built-in Administrator account). ",
    "There is no standard window to switch to. ",
    "History and preferences are unavailable because elevated sessions never ",
    "write the user profile. ",
    "The Recycle Bin is unavailable in every administrator session because ",
    "Windows resolves the path itself, which could redirect the action. ",
    "Permanent deletion stays available behind its own confirmation.",
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileAction {
    Recycle,
    Permanent,
}

struct Eligibility {
    current_generation: u64,
    visible_generation: u64,
    selected: Option<NodeId>,
    row: NodeId,
    current_view: bool,
    regular_file: bool,
    identity_known: bool,
    terminal_scan: bool,
    changes: bool,
    busy: bool,
}

impl AccessMode {
    /// Classifies the process token. An elevated token also asks Windows how it
    /// came to be elevated; that second question only refines the explanation
    /// and never changes what the session may do.
    pub(crate) fn from_probe(probe: Result<bool, String>) -> Self {
        Self::from_probe_with(probe, diskscope::admin::elevation_kind)
    }

    /// [`Self::from_probe`] with the elevation-kind lookup injected, so the
    /// mapping is testable without a Windows token. `kind` is only called for
    /// an elevated token.
    fn from_probe_with(
        probe: Result<bool, String>,
        kind: impl FnOnce() -> Result<ElevationKind, String>,
    ) -> Self {
        match probe {
            Ok(false) => Self::Standard,
            // An unreadable or contradictory kind keeps the generic
            // administrator description rather than guessing that UAC is off.
            Ok(true) => match kind() {
                Ok(ElevationKind::AlwaysElevated) => Self::AlwaysAdmin,
                Ok(_) | Err(_) => Self::Admin,
            },
            Err(error) => Self::Unknown(error),
        }
    }

    /// True for every elevated session, whatever the reason.
    pub(crate) fn is_administrator(&self) -> bool {
        matches!(self, Self::Admin | Self::AlwaysAdmin)
    }

    pub(crate) fn persistence_allowed(&self) -> bool {
        matches!(self, Self::Standard)
    }

    /// Why the Recycle Bin cannot be used in this session, if it cannot.
    fn recycle_refusal(&self) -> Option<&'static str> {
        match self {
            Self::Admin => Some(
                "Recycle Bin requires a Standard window. Open DiskScope normally to recycle this file.",
            ),
            Self::AlwaysAdmin => Some(concat!(
                "The Recycle Bin is unavailable because this account always runs as ",
                "administrator. Permanent deletion is still available.",
            )),
            Self::Standard | Self::Unknown(_) => None,
        }
    }

    /// Extra settings text for a session whose limits need more than the one
    /// line in [`Self::history_message`].
    pub(super) fn settings_note(&self) -> Option<&'static str> {
        matches!(self, Self::AlwaysAdmin).then_some(ALWAYS_ADMIN_EXPLANATION)
    }

    /// The label and the hover text for the access indicator.
    fn indicator(&self) -> (&'static str, String) {
        match self {
            Self::Standard => (
                "Standard",
                "Running with ordinary Windows access. Administrator mode may reveal more metadata."
                    .to_owned(),
            ),
            Self::Admin => (
                "Administrator",
                concat!(
                    "Running with administrator access. File permissions are respected; ",
                    "some locations can still be inaccessible."
                )
                .to_owned(),
            ),
            Self::AlwaysAdmin => ("Administrator (always)", ALWAYS_ADMIN_EXPLANATION.to_owned()),
            Self::Unknown(error) => (
                "Access unknown",
                format!("Windows could not verify this process token: {error}"),
            ),
        }
    }

    pub(super) fn history_message(&self) -> &'static str {
        match self {
            Self::Standard => "Preferences and complete scan history are stored locally.",
            Self::Admin => {
                "Administrator session: history and preferences are not saved. More files may be visible; these scans cannot be compared with ordinary-user baselines."
            }
            Self::AlwaysAdmin => concat!(
                "Administrator session (this account is always elevated): history and ",
                "preferences are not saved. More files may be visible; these scans cannot ",
                "be compared with ordinary-user baselines."
            ),
            Self::Unknown(_) => {
                "Access level could not be verified. History and preferences are not saved, and file actions are unavailable. Read-only scans remain available."
            }
        }
    }

    pub(crate) fn configure_native_options(&self, options: &mut eframe::NativeOptions) {
        if !self.persistence_allowed() {
            // eframe creates its default storage directory before App::new.
            // A custom path bypasses that branch; the Windows null device is a
            // sink even if eframe adds another persistence key in the future.
            options.persistence_path = Some(PathBuf::from(r"\\.\NUL"));
            options.persist_window = false;
        }
    }
}

fn unavailable(access: &AccessMode, action: FileAction, e: &Eligibility) -> Option<&'static str> {
    if matches!(access, AccessMode::Unknown(_)) {
        Some("Windows could not verify this window's access level. File actions are unavailable.")
    } else if e.busy {
        Some("Wait for the current operation to finish, or cancel the confirmation.")
    } else if e.changes {
        Some("File actions are available from the current scan, outside Scan changes.")
    } else if !e.terminal_scan {
        Some("Wait for the scan to stop before changing files.")
    } else if e.current_generation == 0
        || !bound_to_current(
            e.visible_generation,
            e.row,
            e.current_generation,
            e.selected,
        )
        || !e.current_view
    {
        Some("Wait for the current selection and results to finish updating.")
    } else if !e.regular_file {
        Some("Select one regular file. Folders and reparse points cannot be removed here.")
    } else if !e.identity_known {
        Some("This file has no reliable identity. Rescan before changing it.")
    } else if action == FileAction::Recycle
        && let Some(reason) = access.recycle_refusal()
    {
        Some(reason)
    } else {
        None
    }
}

fn confirms_permanent(acknowledged: bool, text: &str) -> bool {
    acknowledged && text == "DELETE"
}

fn bound_to_current(generation: u64, node: NodeId, current: u64, selected: Option<NodeId>) -> bool {
    generation == current && selected == Some(node)
}

#[derive(Clone)]
struct Binding {
    sequence: u64,
    generation: u64,
    node: NodeId,
    root: PathBuf,
    /// The selected file, so every outcome message can name it.
    path: PathBuf,
    action: FileAction,
}

/// A modal the user must acknowledge: a refusal or a surprising outcome.
/// It offers no destructive action; it only points at the separate flows.
struct Report {
    heading: &'static str,
    body: String,
}

struct Confirmation {
    binding: Binding,
    plan: RecyclePlan,
    acknowledged: bool,
    typed: String,
    focus_cancel: bool,
}

enum Operation {
    Preparing(Binding),
    Confirming(Confirmation),
    Running(Binding),
    Report(Report),
}

const RESCAN_LINE: &str = "\n\nDiskScope is rescanning so the list matches the drive.";
const RECYCLE_CONFIRMATION_CHECKED: &str = "DiskScope will request that Windows move this file to the Recycle Bin. DiskScope checked this drive's Recycle Bin settings and the file size first, but Windows makes the final decision. Recycling is not guaranteed. If Windows asks whether to delete the file permanently instead, choose No.";
const RECYCLE_CONFIRMATION_UNVERIFIED: &str = "DiskScope will request that Windows move this file to the Recycle Bin. DiskScope could not fully check this drive's Recycle Bin settings, so Windows alone decides. Recycling is not guaranteed. If Windows asks whether to delete the file permanently instead, choose No.";

fn recycle_confirmation(verdict: RecycleVerdict) -> &'static str {
    if verdict == RecycleVerdict::Unverified {
        RECYCLE_CONFIRMATION_UNVERIFIED
    } else {
        RECYCLE_CONFIRMATION_CHECKED
    }
}

/// What to show and whether to rescan once a file action has finished.
struct Completion {
    notice: String,
    report: Option<Report>,
    rescan: bool,
}

fn file_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn blocked_report(path: &std::path::Path, block: RecycleBlock) -> Report {
    Report {
        heading: "Recycle Bin is not available for this file",
        body: format!(
            "{}\n\n{}\n\nTo remove the file anyway, close this and choose Delete permanently… (Shift+Delete). That asks for its own confirmation: a checkbox and typing DELETE.",
            path.display(),
            ActionError::RecycleUnavailable(block)
        ),
    }
}

fn report_for(path: &std::path::Path, error: &ActionError) -> Option<Report> {
    let heading = match error {
        ActionError::RecycleUnavailable(block) => return Some(blocked_report(path, *block)),
        ActionError::DeletedNotRecycled => "Windows deleted this file instead of recycling it",
        ActionError::RecycleOutcomeUnconfirmed => "The Recycle Bin result could not be confirmed",
        _ => return None,
    };
    Some(Report {
        heading,
        body: format!("{}\n\n{error}", path.display()),
    })
}

/// Decides the notice, the modal and the rescan for a finished action. A rescan
/// follows every result in which the file may no longer exist, so the list
/// never keeps a row for a deleted file.
fn completion_effects(
    path: &std::path::Path,
    result: &Result<ActionOutcome, ActionError>,
) -> Completion {
    match result {
        Ok(outcome) => Completion {
            notice: match outcome {
                ActionOutcome::Recycled { original_path } => {
                    format!("Moved to Recycle Bin: {}", original_path.display())
                }
                ActionOutcome::DeletionRequested { original_path } => format!(
                    "Permanent deletion requested: {}. Windows may finish when remaining handles close.",
                    original_path.display()
                ),
            },
            report: None,
            rescan: true,
        },
        Err(error) => {
            let name = file_label(path);
            Completion {
                notice: match error {
                    ActionError::RecycleUnavailable(_) => {
                        format!("Not moved to the Recycle Bin: {name}. Nothing was changed.")
                    }
                    ActionError::DeletedNotRecycled => format!(
                        "Deleted instead of recycled: {name}. It is not in the Recycle Bin."
                    ),
                    ActionError::RecycleOutcomeUnconfirmed => format!(
                        "Recycle Bin result not confirmed: {name} is no longer where it was."
                    ),
                    other => other.to_string(),
                },
                report: report_for(path, error),
                rescan: error.file_may_be_gone(),
            }
        }
    }
}

/// A Recycle request whose preflight already predicts a permanent delete is
/// shown as a refusal instead of a confirmation. Permanent deletion never
/// consults the verdict.
fn prepared_block(action: FileAction, verdict: RecycleVerdict) -> Option<RecycleBlock> {
    match (action, verdict) {
        (FileAction::Recycle, RecycleVerdict::Blocked(block)) => Some(block),
        _ => None,
    }
}

fn running_lines(action: FileAction, elapsed: Duration) -> Vec<&'static str> {
    let mut lines = vec!["Waiting for Windows…"];
    if action == FileAction::Recycle {
        lines.push("If Windows asks whether to delete the file permanently, choose No.");
        if elapsed >= Duration::from_secs(3) {
            lines.push(
                "Windows may be waiting for your answer in another window. If DiskScope is minimised, restore it; otherwise look in front of DiskScope or press Alt+Tab.",
            );
        }
    }
    lines
}

/// The main window's handle, available only on the event-loop thread. The
/// Shell uses it to keep its own prompts in front of DiskScope.
#[cfg(windows)]
pub(super) fn native_window_id(frame: &eframe::Frame) -> Option<NonZeroIsize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd),
        _ => None,
    }
}
#[cfg(not(windows))]
pub(super) fn native_window_id(_frame: &eframe::Frame) -> Option<NonZeroIsize> {
    None
}

enum ManagementResult {
    Prepared(Binding, Result<RecyclePlan, ActionError>),
    Completed(Binding, Result<ActionOutcome, ActionError>),
    Elevation(Result<ElevationOutcome, String>),
}

pub(super) struct Management {
    tx: mpsc::Sender<ManagementResult>,
    rx: Receiver<ManagementResult>,
    sequence: u64,
    operation: Option<Operation>,
    elevating: bool,
    /// The main window, captured once from the UI thread.
    pub(super) owner: Option<NonZeroIsize>,
    /// When the current action started running, for the waiting hint.
    running_since: Option<Instant>,
}

impl Default for Management {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            sequence: 0,
            operation: None,
            elevating: false,
            owner: None,
            running_since: None,
        }
    }
}

impl Management {
    pub(super) fn busy(&self) -> bool {
        self.operation.is_some() || self.elevating
    }
}

impl DiskScopeApp {
    fn action_unavailable(&self, row: &Row, action: FileAction) -> Option<&'static str> {
        if !self.session_policy.file_actions_allowed() {
            return Some(
                "Helpdesk diagnostic policy disables all file removal. Use your approved cleanup procedure.",
            );
        }
        unavailable(
            &self.access_mode,
            action,
            &Eligibility {
                current_generation: self.generation,
                visible_generation: self.visible.view.generation,
                selected: self.request.selected,
                row: row.id,
                current_view: !self.dirty
                    && self.pending.is_none()
                    && self.visible.view.revision == self.request.revision
                    && self.visible.progress.state == self.snapshot.progress.state
                    && self.root_path == self.snapshot.progress.root,
                regular_file: !row.is_dir && !row.reparse,
                identity_known: row.identity.is_some(),
                terminal_scan: !self.scan_state().is_active(),
                changes: self.changes,
                busy: self.management.busy() || self.picker_open,
            },
        )
    }

    pub(super) fn access_controls(&mut self, ui: &mut egui::Ui, p: Palette) {
        let (label, explanation) = self.access_mode.indicator();
        ui.label(RichText::new(label).small().color(p.accent))
            .on_hover_text(explanation);
        // An elevated window cannot be elevated again, and an always-elevated
        // account has no ordinary window to return to either.
        if !self.access_mode.is_administrator()
            && ui
                .add_enabled(
                    !self.management.busy()
                        && !self.picker_open
                        && self.session_policy == SessionPolicy::default(),
                    egui::Button::new("Restart as administrator…").small(),
                )
                .clicked()
        {
            self.restart_as_administrator(ui.ctx());
        }
    }

    fn restart_as_administrator(&mut self, ctx: &egui::Context) {
        if self.management.busy()
            || self.picker_open
            || self.access_mode.is_administrator()
            || self.session_policy != SessionPolicy::default()
        {
            if self.session_policy != SessionPolicy::default() {
                self.notice = Some("To retain this session's restrictions, launch DiskScope as administrator with the same --helpdesk or --no-history switch. Elevated windows never save profile data.".into());
            }
            return;
        }
        self.management.elevating = true;
        let root = (!self.root_path.as_os_str().is_empty()).then(|| self.root_path.clone());
        let tx = self.management.tx.clone();
        let ctx = ctx.clone();
        let started = std::thread::Builder::new()
            .name("administrator-relaunch".into())
            .spawn(move || {
                let result = diskscope::admin::restart_elevated(root.as_deref());
                let _ = tx.send(ManagementResult::Elevation(result));
                ctx.request_repaint();
            });
        if let Err(error) = started {
            self.management.elevating = false;
            self.notice = Some(format!("Could not start administrator relaunch: {error}"));
        }
    }

    pub(super) fn selected_file_actions(&mut self, ui: &mut egui::Ui, row: &Row, p: Palette) {
        if row.is_dir {
            return;
        }
        ui.add_space(12.0);
        overline(ui, "FILE ACTIONS", p);
        self.file_action_buttons(ui, row);
        if let Some(reason) = self.action_unavailable(row, FileAction::Recycle) {
            ui.label(RichText::new(reason).small().color(p.muted));
        }
    }

    pub(super) fn file_action_buttons(&mut self, ui: &mut egui::Ui, row: &Row) -> bool {
        let mut opened = false;
        for (action, label) in [
            (FileAction::Recycle, "Move to Recycle Bin…"),
            (FileAction::Permanent, "Delete permanently…"),
        ] {
            let unavailable = self.action_unavailable(row, action);
            let response = ui.add_enabled(unavailable.is_none(), egui::Button::new(label));
            let clicked = response.clicked();
            if let Some(reason) = unavailable {
                response.on_disabled_hover_text(reason);
            }
            if clicked {
                self.prepare_file_action(row.clone(), action, ui.ctx());
                opened = true;
            }
        }
        opened
    }

    pub(super) fn file_action_keyboard(&mut self, ctx: &egui::Context) {
        if self.management.busy()
            || ctx.egui_wants_keyboard_input()
            || self.palette_open
            || self.settings_open
            || self.filter_open
            || self.help_open
            || self.comparison_open
            || self.show_errors
            || self.picker_open
        {
            return;
        }
        // egui-winit converts Windows Shift+Delete into Cut and omits the
        // Delete key event. Preserve ordinary clipboard Cut and text editing.
        let native_shift_delete = cfg!(windows)
            && ctx.input_mut(|i| {
                // A complete native chord can be queued before this frame; the
                // final modifier snapshot is then NONE, but Delete release keeps
                // the modifiers associated with the shortcut.
                let shifted_delete = i.modifiers.matches_exact(egui::Modifiers::SHIFT)
                    || i.events.iter().any(|event| {
                        matches!(event,
                    egui::Event::Key { key: egui::Key::Delete, modifiers, .. }
                    if modifiers.matches_exact(egui::Modifiers::SHIFT))
                    });
                let requested = shifted_delete
                    && i.events
                        .iter()
                        .any(|event| matches!(event, egui::Event::Cut));
                if requested {
                    i.events.retain(|event| !matches!(event, egui::Event::Cut));
                }
                requested
            });
        let action = if native_shift_delete
            || ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Delete))
        {
            Some(FileAction::Permanent)
        } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)) {
            Some(FileAction::Recycle)
        } else {
            None
        };
        if let Some(action) = action
            && let Some(row) = self.visible.view.selected.clone()
            && self.action_unavailable(&row, action).is_none()
        {
            self.prepare_file_action(row, action, ctx);
        }
    }

    fn prepare_file_action(&mut self, row: Row, action: FileAction, ctx: &egui::Context) {
        if let Some(reason) = self.action_unavailable(&row, action) {
            self.notice = Some(reason.into());
            return;
        }
        let Some(identity) = row.identity else {
            return;
        };
        self.management.sequence = self.management.sequence.wrapping_add(1);
        let binding = Binding {
            sequence: self.management.sequence,
            generation: self.generation,
            node: row.id,
            root: self.snapshot.progress.root.clone(),
            path: row.path.clone(),
            action,
        };
        self.management.operation = Some(Operation::Preparing(binding.clone()));
        self.palette_open = false;
        let tx = self.management.tx.clone();
        let ctx = ctx.clone();
        let started = std::thread::Builder::new()
            .name("file-action-verify".into())
            .spawn(move || {
                let result = file_actions::prepare(&row.path, &identity, row.logical, row.modified);
                let _ = tx.send(ManagementResult::Prepared(binding, result));
                ctx.request_repaint();
            });
        if let Err(error) = started {
            self.management.operation = None;
            self.notice = Some(format!("Could not verify the selected file: {error}"));
        }
    }

    pub(super) fn poll_management(&mut self, ctx: &egui::Context) {
        for _ in 0..4 {
            let Ok(result) = self.management.rx.try_recv() else {
                break;
            };
            match result {
                ManagementResult::Prepared(binding, result) => {
                    if !matches!(&self.management.operation, Some(Operation::Preparing(current)) if current.sequence == binding.sequence)
                    {
                        continue;
                    }
                    self.management.operation = None;
                    if !bound_to_current(
                        binding.generation,
                        binding.node,
                        self.generation,
                        self.request.selected,
                    ) {
                        self.notice = Some(
                            "The selection changed. Select the current file and try again.".into(),
                        );
                        continue;
                    }
                    match result {
                        Ok(plan) => {
                            self.management.operation = Some(
                                match prepared_block(binding.action, plan.recycle_verdict()) {
                                    Some(block) => {
                                        Operation::Report(blocked_report(&binding.path, block))
                                    }
                                    None => Operation::Confirming(Confirmation {
                                        binding,
                                        plan,
                                        acknowledged: false,
                                        typed: String::new(),
                                        focus_cancel: true,
                                    }),
                                },
                            )
                        }
                        Err(error) => self.notice = Some(error.to_string()),
                    }
                }
                ManagementResult::Completed(binding, result) => {
                    if !matches!(&self.management.operation, Some(Operation::Running(current)) if current.sequence == binding.sequence)
                    {
                        continue;
                    }
                    self.management.operation = None;
                    self.management.running_since = None;
                    let effects = completion_effects(&binding.path, &result);
                    let before_logical = (self.snapshot.progress.state == ScanState::Complete)
                        .then_some(self.snapshot.progress.logical);
                    let before_volume = self.volume.zip(self.volume_observed_at);
                    let before_backend = self.snapshot.progress.backend.clone();
                    // begin_scan ignores requests while an operation is active,
                    // so it runs after the operation is cleared and before the
                    // report (an operation) and the notice (cleared by it).
                    let rescanned = effects.rescan
                        && self.generation == binding.generation
                        && self.root_path == binding.root;
                    if rescanned {
                        self.rescan_keeping_place(binding.root.clone(), binding.node);
                        self.action_review = Some(outcome::ActionReview {
                            path: binding.path.clone(),
                            root: binding.root.clone(),
                            generation: self.generation,
                            message: effects.notice.clone(),
                            recycled: matches!(&result, Ok(ActionOutcome::Recycled { .. })),
                            before_logical,
                            before_backend,
                            after_logical: None,
                            before_volume,
                            after_volume: None,
                            state: None,
                            presence: None,
                            probe_started: false,
                            volume_refresh_requested: false,
                        });
                    }
                    self.notice = Some(effects.notice);
                    if let Some(mut report) = effects.report {
                        if rescanned {
                            report.body.push_str(RESCAN_LINE);
                        }
                        self.management.operation = Some(Operation::Report(report));
                    }
                }
                ManagementResult::Elevation(result) => {
                    self.management.elevating = false;
                    match result {
                        Ok(ElevationOutcome::Launched { .. }) => {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close)
                        }
                        Ok(ElevationOutcome::Cancelled) => {
                            self.notice = Some(
                                "Administrator restart cancelled. This window is still available."
                                    .into(),
                            )
                        }
                        Err(error) => {
                            self.notice =
                                Some(format!("Could not restart as administrator: {error}"))
                        }
                    }
                }
            }
        }
    }

    fn execute_confirmed(&mut self, confirmation: Confirmation, ctx: &egui::Context) {
        if !self.session_policy.file_actions_allowed() {
            self.notice = Some("Helpdesk diagnostic policy refused file removal.".into());
            return;
        }
        let Confirmation {
            binding,
            plan,
            acknowledged,
            typed,
            ..
        } = confirmation;
        if !bound_to_current(
            binding.generation,
            binding.node,
            self.generation,
            self.request.selected,
        ) || (binding.action == FileAction::Permanent
            && !confirms_permanent(acknowledged, &typed))
        {
            self.notice = Some(
                "The file confirmation is no longer current. Select the file and try again.".into(),
            );
            return;
        }
        self.management.operation = Some(Operation::Running(binding.clone()));
        self.management.running_since = Some(Instant::now());
        let owner = self.management.owner;
        let tx = self.management.tx.clone();
        let ctx = ctx.clone();
        let started = std::thread::Builder::new()
            .name("confirmed-file-action".into())
            .spawn(move || {
                let result = match binding.action {
                    FileAction::Recycle => file_actions::recycle(plan, owner),
                    FileAction::Permanent => file_actions::delete_permanently(plan),
                };
                let _ = tx.send(ManagementResult::Completed(binding, result));
                ctx.request_repaint();
            });
        if let Err(error) = started {
            self.management.operation = None;
            self.management.running_since = None;
            self.notice = Some(format!(
                "Could not start the confirmed file action: {error}"
            ));
        }
    }

    pub(super) fn management_dialog(&mut self, ctx: &egui::Context, p: Palette) {
        if self.management.elevating {
            egui::Modal::new(egui::Id::new("administrator_restart")).show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.heading("Restarting as administrator");
                ui.label("Respond to the Windows permission prompt. Cancelling it keeps this window open.");
                ui.label(RichText::new("Waiting for Windows…").color(p.muted));
            });
            return;
        }
        let Some(mut operation) = self.management.operation.take() else {
            return;
        };
        let mut cancel = false;
        let mut confirm = false;
        let cancellable = !matches!(operation, Operation::Running(_));
        if cancellable && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            return;
        }
        let elapsed = self
            .management
            .running_since
            .map(|started| started.elapsed())
            .unwrap_or_default();
        let modal = egui::Modal::new(egui::Id::new("confirmed_file_action")).show(ctx, |ui| {
            ui.set_width(470.0);
            match &mut operation {
                Operation::Preparing(binding) => {
                    ui.heading("Checking the selected file");
                    ui.label(if binding.action == FileAction::Recycle {
                        "Verifying the current path, file identity, size and Recycle Bin availability before confirmation."
                    } else {
                        "Verifying the current path, file identity and size before confirmation."
                    });
                    let button = ui.button("Cancel");
                    button.request_focus();
                    cancel = button.clicked();
                }
                Operation::Running(binding) => {
                    ui.heading("Applying the confirmed file action");
                    for line in running_lines(binding.action, elapsed) {
                        ui.label(line);
                    }
                    ui.spinner();
                }
                Operation::Report(report) => {
                    ui.heading(report.heading);
                    ui.add_space(10.0);
                    ui.add(egui::Label::new(report.body.as_str()).wrap());
                    ui.add_space(12.0);
                    let button = ui.button("OK");
                    button.request_focus();
                    cancel = button.clicked();
                }
                Operation::Confirming(confirmation) => {
                    let permanent = confirmation.binding.action == FileAction::Permanent;
                    ui.heading(if permanent { "Permanently delete this file?" } else { "Move this file to Recycle Bin?" });
                    ui.add_space(10.0);
                    ui.add(egui::Label::new(confirmation.plan.path().display().to_string()).wrap());
                    ui.label(format!("{} logical · {} bytes", format_bytes(confirmation.plan.size()), confirmation.plan.size()));
                    ui.label(RichText::new("Hard links may share storage. This logical size is not a promise of reclaimed space.").small().color(p.muted));
                    ui.add_space(12.0);
                    if permanent {
                        ui.label("This bypasses Recycle Bin. The file cannot be restored from Recycle Bin.");
                        ui.checkbox(&mut confirmation.acknowledged, "I understand this is permanent");
                        ui.label("Type DELETE to enable permanent deletion:");
                        ui.add(egui::TextEdit::singleline(&mut confirmation.typed).char_limit(6).desired_width(240.0));
                    } else {
                        ui.label(recycle_confirmation(confirmation.plan.recycle_verdict()));
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let button = ui.button("Cancel");
                        if confirmation.focus_cancel {
                            button.request_focus();
                            confirmation.focus_cancel = false;
                        }
                        cancel = button.clicked();
                        let enabled = !permanent || confirms_permanent(confirmation.acknowledged, &confirmation.typed);
                        let button = ui.add_enabled(enabled, egui::Button::new(if permanent { "Delete permanently" } else { "Move to Recycle Bin" }));
                        // Enter can arrive as a text-edit completion or carried-over
                        // shortcut. Only an intentional click or Space may confirm.
                        confirm = button.clicked() && !ui.input(|i| i.key_pressed(egui::Key::Enter));
                    });
                    ui.label(RichText::new("Enter does not confirm this action. Cancel is selected by default.").small().color(p.muted));
                }
            }
        });
        if cancellable && (cancel || modal.should_close()) {
            return;
        }
        if confirm {
            if let Operation::Confirming(confirmation) = operation {
                self.execute_confirmed(confirmation, ctx);
            }
        } else {
            self.management.operation = Some(operation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{rendered_texts, settle_frames};
    use super::*;
    use eframe::App;
    use std::cell::Cell;

    #[derive(Default)]
    struct RecordingStorage {
        reads: Cell<usize>,
        writes: usize,
    }
    impl eframe::Storage for RecordingStorage {
        fn get_string(&self, _key: &str) -> Option<String> {
            self.reads.set(self.reads.get() + 1);
            Some(r#"{"map_focus":true,"ntfs_preview":true}"#.into())
        }
        fn set_string(&mut self, _key: &str, _value: String) {
            self.writes += 1;
        }
        fn flush(&mut self) {}
    }

    fn test_app(
        access: AccessMode,
        directory: PathBuf,
        storage: &RecordingStorage,
    ) -> DiskScopeApp {
        let mut cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        cc.storage = Some(storage);
        DiskScopeApp::with_history_directory(&cc, None, directory, access)
    }

    #[test]
    fn privileged_app_creation_skips_preference_reads_and_starts_with_normal_scan_defaults() {
        let directory = tempfile::tempdir().unwrap();
        for access in [
            AccessMode::Admin,
            AccessMode::Unknown("probe failed".into()),
        ] {
            let storage = RecordingStorage::default();
            let app = test_app(access, directory.path().join("history"), &storage);
            assert_eq!(storage.reads.get(), 0);
            assert!(!app.preferences.map_focus);
            assert!(!app.preferences.ntfs_preview);
        }
    }

    #[test]
    fn privileged_app_save_does_not_write_preferences_or_egui_memory() {
        let directory = tempfile::tempdir().unwrap();
        for access in [
            AccessMode::Admin,
            AccessMode::Unknown("probe failed".into()),
        ] {
            let mut storage = RecordingStorage::default();
            let mut app = test_app(access, directory.path().join("history"), &storage);
            app.save(&mut storage);
            assert_eq!(storage.writes, 0);
            assert!(!app.persist_egui_memory());
        }
    }

    #[test]
    fn standard_app_keeps_existing_preference_behavior() {
        let directory = tempfile::tempdir().unwrap();
        let mut storage = RecordingStorage::default();
        let mut app = test_app(
            AccessMode::Standard,
            directory.path().join("history"),
            &storage,
        );
        assert!(storage.reads.get() > 0);
        assert!(app.preferences.map_focus);
        assert!(app.preferences.ntfs_preview);
        app.save(&mut storage);
        assert!(storage.writes > 0);
        assert!(app.persist_egui_memory());
    }

    fn binding(sequence: u64) -> Binding {
        Binding {
            sequence,
            generation: 7,
            node: 42,
            root: PathBuf::from(r"C:\fixture"),
            path: PathBuf::from(r"C:\fixture\demo.mp4"),
            action: FileAction::Recycle,
        }
    }

    /// An app whose current scan matches `binding(1)`, so completion handling
    /// can rescan (the root and generation must agree for that).
    fn app_for_binding(storage: &RecordingStorage, directory: &std::path::Path) -> DiskScopeApp {
        let mut app = test_app(AccessMode::Admin, directory.join("history"), storage);
        app.generation = 7;
        app.root_path = binding(1).root;
        app.request.selected = Some(42);
        app
    }

    fn complete(app: &mut DiskScopeApp, result: Result<ActionOutcome, ActionError>) {
        app.management.operation = Some(Operation::Running(binding(1)));
        app.management
            .tx
            .send(ManagementResult::Completed(binding(1), result))
            .unwrap();
        app.poll_management(&egui::Context::default());
    }

    fn keyboard_app() -> DiskScopeApp {
        let storage = RecordingStorage::default();
        let mut app = test_app(AccessMode::Admin, PathBuf::new(), &storage);
        let mut model = Model::new(PathBuf::from(r"C:\fixture"));
        model.insert_batch(
            0,
            vec![Entry {
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
            }],
        );
        app.generation = 7;
        app.root_path = model.root_path.clone();
        app.request.selected = Some(1);
        let mut snapshot = UiSnapshot::default();
        snapshot.progress.generation = 7;
        snapshot.progress.root = model.root_path.clone();
        snapshot.progress.state = ScanState::Complete;
        snapshot.view = diskscope::view::build_view(&model, &app.request, 7);
        app.snapshot = Arc::new(snapshot);
        app.visible = app.snapshot.clone();
        app
    }

    // The Cut-event translation is compiled only for Windows (`cfg!(windows)`
    // in `file_action_keyboard`), so these two regressions are Windows-only.
    #[cfg(windows)]
    #[test]
    fn windows_shift_delete_cut_event_opens_only_permanent_preparation() {
        let mut app = keyboard_app();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            modifiers: egui::Modifiers::SHIFT,
            events: vec![egui::Event::Cut],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.file_action_keyboard(ui.ctx()));
        assert!(matches!(
            app.management.operation,
            Some(Operation::Preparing(Binding {
                action: FileAction::Permanent,
                ..
            }))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_shift_delete_cut_and_key_release_preserve_the_shortcut_after_modifier_release() {
        let mut app = keyboard_app();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            modifiers: egui::Modifiers::NONE,
            events: vec![
                egui::Event::Cut,
                egui::Event::Key {
                    key: egui::Key::Delete,
                    physical_key: Some(egui::Key::Delete),
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::SHIFT,
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.file_action_keyboard(ui.ctx()));
        assert!(matches!(
            app.management.operation,
            Some(Operation::Preparing(Binding {
                action: FileAction::Permanent,
                ..
            }))
        ));
    }

    #[test]
    fn clipboard_cut_without_shift_never_prepares_a_file_action() {
        let mut app = keyboard_app();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            modifiers: egui::Modifiers::CTRL,
            events: vec![egui::Event::Cut],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.file_action_keyboard(ui.ctx()));
        assert!(!app.management.busy());
    }

    #[cfg(windows)]
    #[test]
    fn helpdesk_refuses_a_previously_prepared_permanent_confirmation_at_commit() {
        let (_fixture, mut app, row) = browsing_in_a_nested_folder();
        let before = std::fs::read(&row.path).unwrap();
        let plan = file_actions::prepare(
            &row.path,
            row.identity.as_ref().unwrap(),
            row.logical,
            row.modified,
        )
        .unwrap();
        let binding = Binding {
            sequence: 1,
            generation: app.generation,
            node: row.id,
            root: app.root_path.clone(),
            path: row.path.clone(),
            action: FileAction::Permanent,
        };
        app.request.selected = Some(row.id);
        app.session_policy = SessionPolicy {
            helpdesk: true,
            no_history: true,
        };
        app.execute_confirmed(
            Confirmation {
                binding,
                plan,
                acknowledged: true,
                typed: "DELETE".into(),
                focus_cancel: false,
            },
            &egui::Context::default(),
        );
        assert!(!app.management.busy());
        assert_eq!(std::fs::read(&row.path).unwrap(), before);
        assert!(app.notice.as_deref().unwrap().contains("refused"));
    }

    #[test]
    fn helpdesk_policy_refuses_buttons_keyboard_preparation_and_elevation() {
        let mut app = keyboard_app();
        app.session_policy = SessionPolicy {
            helpdesk: true,
            no_history: true,
        };
        let row = app.visible.view.selected.as_ref().unwrap().clone();
        let ctx = egui::Context::default();
        for action in [FileAction::Recycle, FileAction::Permanent] {
            assert!(
                app.action_unavailable(&row, action)
                    .unwrap()
                    .contains("Helpdesk")
            );
            app.prepare_file_action(row.clone(), action, &ctx);
            assert!(!app.management.busy());
        }
        let _ = ctx.run_ui(
            egui::RawInput {
                modifiers: egui::Modifiers::SHIFT,
                events: vec![egui::Event::Cut],
                ..Default::default()
            },
            |ui| app.file_action_keyboard(ui.ctx()),
        );
        assert!(!app.management.busy());
        app.restart_as_administrator(&ctx);
        assert!(!app.management.elevating);
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|n| n.contains("restrictions"))
        );
    }

    #[test]
    fn native_shift_cut_stays_text_editing_when_an_input_has_focus() {
        let mut app = keyboard_app();
        let ctx = egui::Context::default();
        let mut text = "selected text".to_owned();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut text))
                    .request_focus();
            });
        });
        let input = egui::RawInput {
            modifiers: egui::Modifiers::SHIFT,
            events: vec![egui::Event::Cut],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.file_action_keyboard(ui.ctx()));
        assert!(!app.management.busy());
        assert!(ctx.input(|i| {
            i.events
                .iter()
                .any(|event| matches!(event, egui::Event::Cut))
        }));
    }

    #[test]
    fn escape_cancels_preparation_and_preserves_the_scan_and_selection() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordingStorage::default();
        let mut app = test_app(
            AccessMode::Admin,
            directory.path().join("history"),
            &storage,
        );
        app.generation = 7;
        app.request.selected = Some(42);
        app.management.operation = Some(Operation::Preparing(binding(1)));
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run_ui(input, |ui| {
            app.management_dialog(ui.ctx(), app.preferences.theme.palette())
        });
        assert!(!app.management.busy());
        assert_eq!(app.generation, 7);
        assert_eq!(app.request.selected, Some(42));
    }

    #[test]
    fn failed_file_operation_releases_busy_guard_and_surfaces_the_error() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordingStorage::default();
        let mut app = test_app(
            AccessMode::Admin,
            directory.path().join("history"),
            &storage,
        );
        app.generation = 7;
        app.root_path = binding(1).root;
        app.request.selected = Some(42);
        app.management.operation = Some(Operation::Running(binding(1)));
        app.management
            .tx
            .send(ManagementResult::Completed(
                binding(1),
                Err(ActionError::PermissionDenied),
            ))
            .unwrap();
        app.poll_management(&egui::Context::default());
        assert!(!app.management.busy());
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|notice| notice.contains("Windows denied"))
        );
        assert_eq!(app.generation, 7);
    }

    #[test]
    fn cancelled_preparation_result_does_not_replace_a_later_operation() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordingStorage::default();
        let mut app = test_app(
            AccessMode::Admin,
            directory.path().join("history"),
            &storage,
        );
        app.management.operation = Some(Operation::Preparing(binding(2)));
        app.management
            .tx
            .send(ManagementResult::Prepared(
                binding(1),
                Err(ActionError::Changed),
            ))
            .unwrap();
        app.poll_management(&egui::Context::default());
        assert!(matches!(
            app.management.operation,
            Some(Operation::Preparing(Binding { sequence: 2, .. }))
        ));
        assert!(app.notice.is_none());
    }

    #[test]
    fn cancelled_or_failed_elevation_keeps_the_window_open_and_usable() {
        for result in [Ok(ElevationOutcome::Cancelled), Err("launch failed".into())] {
            let directory = tempfile::tempdir().unwrap();
            let storage = RecordingStorage::default();
            let mut app = test_app(
                AccessMode::Standard,
                directory.path().join("history"),
                &storage,
            );
            app.management.elevating = true;
            app.management
                .tx
                .send(ManagementResult::Elevation(result))
                .unwrap();
            let ctx = egui::Context::default();
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                app.poll_management(ui.ctx())
            });
            assert!(!app.management.busy());
            assert!(app.notice.is_some());
            assert!(output.viewport_output.values().all(|v| {
                !v.commands
                    .iter()
                    .any(|c| matches!(c, egui::ViewportCommand::Close))
            }));
        }
    }

    #[test]
    fn completion_effects_matrix() {
        let path = PathBuf::from(r"C:\fixture\demo.mp4");
        let original_path = path.clone();
        for outcome in [
            ActionOutcome::Recycled {
                original_path: original_path.clone(),
            },
            ActionOutcome::DeletionRequested {
                original_path: original_path.clone(),
            },
        ] {
            let fx = completion_effects(&path, &Ok(outcome));
            assert!(fx.rescan && fx.report.is_none());
        }
        for error in [
            ActionError::DeletedNotRecycled,
            ActionError::RecycleOutcomeUnconfirmed,
        ] {
            let fx = completion_effects(&path, &Err(error.clone()));
            assert!(fx.rescan, "{error:?}");
            assert!(fx.report.is_some(), "{error:?}");
            assert!(fx.notice.contains("demo.mp4"), "{}", fx.notice);
        }
        for block in [
            RecycleBlock::NoBinOnDrive,
            RecycleBlock::DisabledByPolicy,
            RecycleBlock::DeletesImmediately,
            RecycleBlock::TooLarge { size: 5, limit: 1 },
            RecycleBlock::ShellDeclined,
        ] {
            let fx = completion_effects(&path, &Err(ActionError::RecycleUnavailable(block)));
            assert!(!fx.rescan, "a refused recycle leaves the file in place");
            let report = fx.report.expect("a refusal is shown, not just noted");
            assert!(report.body.contains("Delete permanently…"));
            assert!(report.body.contains("Nothing was changed"));
            assert!(fx.notice.contains("Nothing was changed"));
        }
        for error in [
            ActionError::Cancelled,
            ActionError::PermissionDenied,
            ActionError::Busy,
            ActionError::Changed,
            ActionError::Failed("x".into()),
        ] {
            let fx = completion_effects(&path, &Err(error.clone()));
            assert!(!fx.rescan && fx.report.is_none(), "{error:?}");
            assert_eq!(fx.notice, error.to_string());
        }
    }

    #[test]
    fn prepared_block_only_blocks_recycle() {
        let blocked = RecycleVerdict::Blocked(RecycleBlock::NoBinOnDrive);
        assert_eq!(
            prepared_block(FileAction::Recycle, blocked),
            Some(RecycleBlock::NoBinOnDrive)
        );
        assert_eq!(prepared_block(FileAction::Permanent, blocked), None);
        for verdict in [RecycleVerdict::Likely, RecycleVerdict::Unverified] {
            assert_eq!(prepared_block(FileAction::Recycle, verdict), None);
        }
    }

    #[test]
    fn running_lines_hint_only_for_recycle_after_three_seconds() {
        let early = running_lines(FileAction::Recycle, Duration::from_millis(2900));
        assert!(early.iter().any(|line| line.contains("choose No")));
        assert!(!early.iter().any(|line| line.contains("another window")));
        let late = running_lines(FileAction::Recycle, Duration::from_secs(3));
        assert!(late.iter().any(|line| line.contains("choose No")));
        assert!(late.iter().any(|line| line.contains("another window")));
        for elapsed in [Duration::ZERO, Duration::from_secs(60)] {
            let lines = running_lines(FileAction::Permanent, elapsed);
            assert!(!lines.iter().any(|line| line.contains("choose No")));
            assert!(!lines.iter().any(|line| line.contains("another window")));
        }
    }

    #[test]
    fn recycle_confirmation_copy_does_not_overpromise() {
        let checked = recycle_confirmation(RecycleVerdict::Likely);
        let unverified = recycle_confirmation(RecycleVerdict::Unverified);
        assert_ne!(checked, unverified);
        assert!(checked.contains("Windows makes the final decision"));
        // When the check could not run, the copy must not claim it did.
        assert!(!unverified.contains("checked"));
        assert!(unverified.contains("could not fully check"));
        for text in [checked, unverified] {
            assert!(text.contains("choose No"), "{text}");
            assert!(!text.contains("fails without falling back"), "{text}");
        }
        assert!(
            super::controls::FILE_ACTION_HELP.contains("final decision"),
            "Help must not promise that Recycle always stops"
        );
    }

    #[test]
    fn destroyed_file_rescans_and_opens_a_report() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordingStorage::default();
        let mut app = app_for_binding(&storage, directory.path());
        complete(&mut app, Err(ActionError::DeletedNotRecycled));
        // begin_scan ran: the generation moved on and the list will refresh.
        assert_ne!(app.generation, 7);
        let Some(Operation::Report(report)) = &app.management.operation else {
            panic!("a destroyed file must be acknowledged");
        };
        assert!(report.body.contains("rescanning"), "{}", report.body);
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.starts_with("Deleted instead of recycled"))
        );
        assert!(app.management.busy(), "the report holds the UI until OK");
    }

    #[test]
    fn refused_or_failed_recycle_does_not_rescan() {
        // Negative control: the file is intact, so the scan must be kept.
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordingStorage::default();
        let mut app = app_for_binding(&storage, directory.path());
        complete(
            &mut app,
            Err(ActionError::RecycleUnavailable(RecycleBlock::ShellDeclined)),
        );
        assert_eq!(app.generation, 7);
        assert!(matches!(
            app.management.operation,
            Some(Operation::Report(_))
        ));
        assert!(app.notice.is_some());
        app.management.operation = None;
        complete(&mut app, Err(ActionError::PermissionDenied));
        assert_eq!(app.generation, 7);
        assert!(app.management.operation.is_none());
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Windows denied"))
        );
    }

    #[test]
    fn successful_recycle_rescans() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordingStorage::default();
        let mut app = app_for_binding(&storage, directory.path());
        complete(
            &mut app,
            Ok(ActionOutcome::Recycled {
                original_path: binding(1).path,
            }),
        );
        assert_ne!(app.generation, 7);
        assert!(app.management.operation.is_none());
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.starts_with("Moved to Recycle Bin"))
        );
    }

    /// True once the newest request has been answered for the current scan,
    /// the scan has stopped and no restore is pending.
    fn settled(app: &DiskScopeApp) -> bool {
        !app.dirty
            && app.restore.is_none()
            && app.snapshot.progress.generation == app.generation
            && !app.snapshot.progress.state.is_active()
            && app.visible.view.generation == app.generation
            && app.visible.view.revision == app.request.revision
    }

    /// Polls and submits view requests like the frame loop until `done` holds.
    fn pump_until(app: &mut DiskScopeApp, what: &str, done: impl Fn(&DiskScopeApp) -> bool) {
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done(app) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            app.logic(&ctx, &mut frame);
            app.submit_view();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn row_named(app: &DiskScopeApp, name: &str) -> Row {
        app.visible
            .view
            .rows
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("no row named {name}"))
            .clone()
    }

    fn visible_names(app: &DiskScopeApp) -> Vec<String> {
        app.visible
            .view
            .rows
            .iter()
            .map(|row| row.name.clone())
            .collect()
    }

    /// A real scan of `root/a/b/{one,two,three}.bin` (300, 200 and 100 bytes)
    /// that the user has opened down to `b`, sorted by name with a name
    /// search, with `two.bin` selected.
    fn browsing_in_a_nested_folder() -> (tempfile::TempDir, DiskScopeApp, Row) {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("a").join("b");
        std::fs::create_dir_all(&folder).unwrap();
        for (name, size) in [("one.bin", 300), ("two.bin", 200), ("three.bin", 100)] {
            std::fs::write(folder.join(name), vec![0u8; size]).unwrap();
        }
        std::fs::write(dir.path().join("top.txt"), b"x").unwrap();
        let storage = RecordingStorage::default();
        let mut app = test_app(AccessMode::Admin, PathBuf::new(), &storage);
        app.begin_scan(dir.path().to_path_buf());
        pump_until(&mut app, "the first scan", settled);
        app.destination(ViewMode::Explorer);
        pump_until(&mut app, "the Explorer view", settled);
        for name in ["a", "b"] {
            let id = row_named(&app, name).id;
            app.action(Action::Navigate(id));
            pump_until(&mut app, "opening a folder", settled);
        }
        app.request.sort = SortKey::Name;
        app.request.descending = false;
        app.request.search = "bin".into();
        app.mark_query_changed();
        pump_until(&mut app, "the sorted, searched listing", settled);
        assert_eq!(visible_names(&app), ["one.bin", "three.bin", "two.bin"]);
        let two = row_named(&app, "two.bin");
        app.action(Action::Select(two.id));
        pump_until(&mut app, "the selection", settled);
        assert_eq!(
            app.visible.view.selected.as_ref().map(|row| row.id),
            Some(two.id)
        );
        (dir, app, two)
    }

    /// Finishes a file action on `removed` the way the worker thread would.
    fn finish_action(
        app: &mut DiskScopeApp,
        removed: &Row,
        result: Result<ActionOutcome, ActionError>,
    ) {
        let binding = Binding {
            sequence: 1,
            generation: app.generation,
            node: removed.id,
            root: app.root_path.clone(),
            path: removed.path.clone(),
            action: FileAction::Recycle,
        };
        app.management.operation = Some(Operation::Running(binding.clone()));
        app.management
            .tx
            .send(ManagementResult::Completed(binding, result))
            .unwrap();
        app.poll_management(&egui::Context::default());
    }

    fn recycled(row: &Row) -> Result<ActionOutcome, ActionError> {
        Ok(ActionOutcome::Recycled {
            original_path: row.path.clone(),
        })
    }

    #[test]
    fn rescan_after_a_file_action_returns_to_the_same_folder_with_the_same_query() {
        let (dir, mut app, two) = browsing_in_a_nested_folder();
        let folder = dir.path().join("a").join("b");
        let old_generation = app.generation;
        std::fs::remove_file(&two.path).unwrap();
        finish_action(&mut app, &two, recycled(&two));
        // Immediately after the action: a new scan with the place pending.
        assert_ne!(app.generation, old_generation);
        assert!(app.restore.is_some());
        assert_eq!(app.request.root, 0, "node ids from the old scan are gone");
        assert_eq!(app.request.selected, None);
        assert_eq!(app.request.mode, ViewMode::Explorer);
        assert_eq!(app.request.sort, SortKey::Name);
        assert!(!app.request.descending);
        assert_eq!(app.request.search, "bin");
        assert_eq!(
            app.request.focus_path.as_deref(),
            Some(folder.join("three.bin").as_path()),
            "two.bin was last, so its previous neighbour is the target"
        );
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.starts_with("Moved to Recycle Bin"))
        );
        pump_until(&mut app, "the rescan", settled);
        assert_eq!(app.visible.view.root_path, folder);
        assert_eq!(visible_names(&app), ["one.bin", "three.bin"]);
        let selected = app.visible.view.selected.as_ref().expect("a neighbour");
        assert_eq!(selected.name, "three.bin");
        assert_eq!(app.request.selected, Some(selected.id));
        assert_eq!(app.request.mode, ViewMode::Explorer);
        assert_eq!(app.request.sort, SortKey::Name);
        assert_eq!(app.request.search, "bin");
        assert!(app.request.focus_path.is_none());
        // Back/Forward still start empty: their entries hold old node ids.
        assert!(!app.history.can_back() && !app.history.can_forward());
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.starts_with("Moved to Recycle Bin")),
            "{:?}",
            app.notice
        );
    }

    #[test]
    fn a_draft_location_does_not_skip_action_reconciliation_or_replace_the_draft() {
        let (_dir, mut app, two) = browsing_in_a_nested_folder();
        let root = app.root_path.clone();
        let generation = app.generation;
        let draft = root.join("different-location");
        app.native_tx
            .send(NativeResult::Folder(Some(draft.clone())))
            .unwrap();
        app.poll(&egui::Context::default());
        std::fs::remove_file(&two.path).unwrap();
        finish_action(&mut app, &two, recycled(&two));
        assert_ne!(app.generation, generation);
        assert_eq!(app.root_path, root);
        assert_eq!(app.path_text, draft.to_string_lossy());
        assert!(app.path_edited);
        pump_until(&mut app, "reconciling the scanned location", settled);
        assert!(!visible_names(&app).iter().any(|name| name == "two.bin"));
        let review = app
            .action_review
            .as_ref()
            .expect("persistent action evidence");
        assert_eq!(review.root, root);
    }

    #[test]
    fn an_emptied_folder_is_reopened_empty_without_a_lost_folder_notice() {
        let (dir, mut app, two) = browsing_in_a_nested_folder();
        let folder = dir.path().join("a").join("b");
        for name in ["one.bin", "two.bin", "three.bin"] {
            std::fs::remove_file(folder.join(name)).unwrap();
        }
        finish_action(&mut app, &two, recycled(&two));
        pump_until(&mut app, "the rescan", settled);
        // The folder still exists, so the user lands in it, not above it.
        assert_eq!(app.visible.view.root_path, folder);
        assert!(app.visible.view.rows.is_empty());
        assert_eq!(app.request.selected, None);
        let notice = app.notice.as_deref().unwrap();
        assert!(notice.starts_with("Moved to Recycle Bin"), "{notice}");
        assert!(!notice.contains("no longer in the scan"), "{notice}");
    }

    #[test]
    fn a_folder_deleted_meanwhile_reopens_its_parent_with_a_notice() {
        let (dir, mut app, two) = browsing_in_a_nested_folder();
        std::fs::remove_dir_all(dir.path().join("a").join("b")).unwrap();
        finish_action(&mut app, &two, recycled(&two));
        pump_until(&mut app, "the rescan", settled);
        assert_eq!(app.visible.view.root_path, dir.path().join("a"));
        assert_eq!(app.request.selected, None);
        assert_eq!(app.request.mode, ViewMode::Explorer);
        assert_eq!(app.request.sort, SortKey::Name);
        let notice = app.notice.as_deref().unwrap();
        assert!(notice.starts_with("Moved to Recycle Bin"), "{notice}");
        assert!(
            notice.contains("b is no longer in the scan, so DiskScope opened its closest remaining parent folder, a."),
            "{notice}"
        );
    }

    #[test]
    fn the_report_and_notice_ordering_survives_a_restoring_rescan() {
        let (_dir, mut app, two) = browsing_in_a_nested_folder();
        std::fs::remove_file(&two.path).unwrap();
        finish_action(&mut app, &two, Err(ActionError::DeletedNotRecycled));
        let Some(Operation::Report(report)) = &app.management.operation else {
            panic!("a destroyed file must still be acknowledged");
        };
        assert!(report.body.contains("rescanning"), "{}", report.body);
        assert!(app.restore.is_some());
        pump_until(&mut app, "the rescan", settled);
        // The modal is still up, and the restore ran underneath it without
        // replacing the file action's notice.
        assert!(matches!(
            app.management.operation,
            Some(Operation::Report(_))
        ));
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.starts_with("Deleted instead of recycled"))
        );
        assert_eq!(
            app.visible.view.selected.as_ref().unwrap().name,
            "three.bin"
        );
    }

    #[test]
    fn without_a_displayed_view_the_rescan_keeps_the_query_but_not_the_folder() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordingStorage::default();
        let mut app = app_for_binding(&storage, directory.path());
        app.request.sort = SortKey::Files;
        app.request.search = "needle".into();
        // The fixture has no admitted view for generation 7.
        complete(
            &mut app,
            Ok(ActionOutcome::Recycled {
                original_path: binding(1).path,
            }),
        );
        assert!(app.restore.is_none());
        assert_eq!(app.request.sort, SortKey::Files);
        assert_eq!(app.request.search, "needle");
        assert_eq!(app.request.root, 0);
    }

    #[test]
    fn an_always_elevated_token_is_told_apart_and_nothing_else_changes() {
        let never = || -> Result<ElevationKind, String> {
            panic!("a token that is not elevated needs no second question")
        };
        assert_eq!(
            AccessMode::from_probe_with(Ok(false), never),
            AccessMode::Standard
        );
        assert_eq!(
            AccessMode::from_probe_with(Err("denied".into()), never),
            AccessMode::Unknown("denied".into())
        );
        for (kind, expected) in [
            (Ok(ElevationKind::AlwaysElevated), AccessMode::AlwaysAdmin),
            (Ok(ElevationKind::Elevated), AccessMode::Admin),
            // A contradictory or unreadable answer never claims UAC is off.
            (Ok(ElevationKind::Standard), AccessMode::Admin),
            (Err("no token".to_owned()), AccessMode::Admin),
        ] {
            assert_eq!(
                AccessMode::from_probe_with(Ok(true), || kind.clone()),
                expected,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn always_elevated_sessions_follow_the_administrator_policy_with_their_own_words() {
        let always = AccessMode::AlwaysAdmin;
        // Same policy as a consented administrator window.
        assert!(always.is_administrator());
        assert!(!always.persistence_allowed());
        assert!(unavailable(&always, FileAction::Permanent, &eligible()).is_none());
        let reason = unavailable(&always, FileAction::Recycle, &eligible()).unwrap();
        // Different advice: there is no normal window to open.
        let consented = unavailable(&AccessMode::Admin, FileAction::Recycle, &eligible()).unwrap();
        assert_ne!(reason, consented);
        assert!(consented.contains("Open DiskScope normally"));
        assert!(!reason.contains("normally"), "{reason}");
        assert!(reason.contains("always runs as administrator"), "{reason}");
        // The explanation names the cause and the consequence honestly.
        let (label, hover) = always.indicator();
        assert_eq!(label, "Administrator (always)");
        for fragment in [
            "UAC",
            "Admin Approval Mode",
            "no standard window",
            "never write the user profile",
            "Recycle Bin is unavailable in every administrator session",
        ] {
            assert!(hover.contains(fragment), "{fragment}: {hover}");
        }
        assert!(always.history_message().contains("always elevated"));
        assert_ne!(
            always.history_message(),
            AccessMode::Admin.history_message()
        );
        assert_eq!(always.settings_note(), Some(ALWAYS_ADMIN_EXPLANATION));
        assert_eq!(AccessMode::Admin.settings_note(), None);
    }

    #[test]
    fn settings_explain_an_always_elevated_session_and_offer_no_restart() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordingStorage::default();
        let mut shown = Vec::new();
        for access in [
            AccessMode::Standard,
            AccessMode::Admin,
            AccessMode::AlwaysAdmin,
        ] {
            let mut app = test_app(access.clone(), directory.path().join("history"), &storage);
            app.settings_open = true;
            let ctx = egui::Context::default();
            let output = settle_frames(&ctx, |ui| {
                app.settings_window(ui.ctx(), app.preferences.theme.palette())
            });
            shown.push((access, rendered_texts(&output.shapes)));
        }
        let has = |index: usize, text: &str| shown[index].1.iter().any(|t| t == text);
        // Only a Standard window can offer a restart.
        assert!(has(0, "Restart as administrator…"));
        assert!(!has(1, "Restart as administrator…"));
        assert!(!has(2, "Restart as administrator…"));
        // The explanation is visible text, not only a tooltip, and only for
        // the always-elevated session.
        assert!(has(2, ALWAYS_ADMIN_EXPLANATION));
        assert!(!has(1, ALWAYS_ADMIN_EXPLANATION));
        assert!(has(2, "Administrator (always)"));
        assert!(has(1, "Administrator"));
    }

    #[test]
    fn report_dismisses_with_escape_and_never_starts_an_action() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RecordingStorage::default();
        let mut app = app_for_binding(&storage, directory.path());
        app.management.operation = Some(Operation::Report(blocked_report(
            &binding(1).path,
            RecycleBlock::NoBinOnDrive,
        )));
        assert!(app.management.busy());
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run_ui(input, |ui| {
            app.management_dialog(ui.ctx(), app.preferences.theme.palette())
        });
        assert!(!app.management.busy());
        assert_eq!(app.generation, 7);
        assert_eq!(app.request.selected, Some(42));
        assert!(app.management.rx.try_recv().is_err());
    }

    fn eligible() -> Eligibility {
        Eligibility {
            current_generation: 7,
            visible_generation: 7,
            selected: Some(42),
            row: 42,
            current_view: true,
            regular_file: true,
            identity_known: true,
            terminal_scan: true,
            changes: false,
            busy: false,
        }
    }

    #[test]
    fn actual_token_probe_fails_closed_and_privileged_sessions_do_not_persist() {
        assert_eq!(AccessMode::from_probe(Ok(false)), AccessMode::Standard);
        let elevated = AccessMode::from_probe(Ok(true));
        assert!(elevated.is_administrator());
        assert!(!elevated.persistence_allowed());
        assert_eq!(
            AccessMode::from_probe(Err("probe failed".into())),
            AccessMode::Unknown("probe failed".into())
        );
        assert!(AccessMode::Standard.persistence_allowed());
        assert!(!AccessMode::Admin.persistence_allowed());
        assert!(!AccessMode::Unknown("probe failed".into()).persistence_allowed());
    }

    #[test]
    fn actions_require_current_live_terminal_regular_selection_and_known_identity() {
        let access = AccessMode::Standard;
        assert!(unavailable(&access, FileAction::Recycle, &eligible()).is_none());
        for e in [
            Eligibility {
                busy: true,
                ..eligible()
            },
            Eligibility {
                changes: true,
                ..eligible()
            },
            Eligibility {
                terminal_scan: false,
                ..eligible()
            },
            Eligibility {
                regular_file: false,
                ..eligible()
            },
            Eligibility {
                identity_known: false,
                ..eligible()
            },
            Eligibility {
                selected: Some(41),
                ..eligible()
            },
            Eligibility {
                current_generation: 8,
                ..eligible()
            },
            Eligibility {
                current_view: false,
                ..eligible()
            },
            Eligibility {
                current_generation: 0,
                visible_generation: 0,
                ..eligible()
            },
        ] {
            assert!(unavailable(&access, FileAction::Recycle, &e).is_some());
            assert!(unavailable(&access, FileAction::Permanent, &e).is_some());
        }
    }

    #[test]
    fn elevated_recycling_and_unknown_token_file_actions_are_disabled() {
        assert!(unavailable(&AccessMode::Admin, FileAction::Recycle, &eligible()).is_some());
        assert!(unavailable(&AccessMode::Admin, FileAction::Permanent, &eligible()).is_none());
        let unknown = AccessMode::Unknown("probe failed".into());
        assert!(unavailable(&unknown, FileAction::Recycle, &eligible()).is_some());
        assert!(unavailable(&unknown, FileAction::Permanent, &eligible()).is_some());
    }

    #[test]
    fn permanent_confirmation_requires_both_explicit_steps() {
        assert!(!confirms_permanent(false, "DELETE"));
        assert!(!confirms_permanent(true, ""));
        assert!(!confirms_permanent(true, "delete"));
        assert!(!confirms_permanent(true, " DELETE "));
        assert!(confirms_permanent(true, "DELETE"));
    }

    #[test]
    fn prepared_results_never_follow_a_new_generation_or_selection() {
        assert!(bound_to_current(7, 42, 7, Some(42)));
        assert!(!bound_to_current(7, 42, 8, Some(42)));
        assert!(!bound_to_current(7, 42, 7, Some(41)));
        assert!(!bound_to_current(7, 42, 7, None));
    }

    #[test]
    fn privileged_native_options_bypass_the_ordinary_storage_directory() {
        for access in [
            AccessMode::Admin,
            AccessMode::Unknown("probe failed".into()),
        ] {
            let mut options = eframe::NativeOptions::default();
            access.configure_native_options(&mut options);
            assert_eq!(options.persistence_path, Some(PathBuf::from(r"\\.\NUL")));
            assert!(!options.persist_window);
        }
        let mut standard = eframe::NativeOptions::default();
        AccessMode::Standard.configure_native_options(&mut standard);
        assert!(standard.persistence_path.is_none());
        assert!(standard.persist_window);
    }
}
