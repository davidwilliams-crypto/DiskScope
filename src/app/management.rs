use super::*;
use diskscope::{
    admin::ElevationOutcome,
    file_actions::{self, ActionError, ActionOutcome, RecycleBlock, RecyclePlan, RecycleVerdict},
};
use std::num::NonZeroIsize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AccessMode {
    Standard,
    Admin,
    Unknown(String),
}

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
    pub(crate) fn from_probe(probe: Result<bool, String>) -> Self {
        match probe {
            Ok(false) => Self::Standard,
            Ok(true) => Self::Admin,
            Err(error) => Self::Unknown(error),
        }
    }

    pub(crate) fn persistence_allowed(&self) -> bool {
        matches!(self, Self::Standard)
    }

    pub(super) fn history_message(&self) -> &'static str {
        match self {
            Self::Standard => "Preferences and complete scan history are stored locally.",
            Self::Admin => {
                "Administrator session: history and preferences are not saved. More files may be visible; these scans cannot be compared with ordinary-user baselines."
            }
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
        Some("File actions are available from the current scan, outside What changed?.")
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
    } else if matches!(access, AccessMode::Admin) && action == FileAction::Recycle {
        Some(
            "Recycle Bin requires a Standard window. Open DiskScope normally to recycle this file.",
        )
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
const RECYCLE_CONFIRMATION: &str = "Windows will move this file to the Recycle Bin. DiskScope checked this drive's Recycle Bin settings and the file size first, but Windows makes the final decision. If Windows asks whether to delete the file permanently instead, choose No.";

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
                "Windows may be waiting for your answer in another window. Look for it in front of DiskScope or in the taskbar, or press Alt+Tab.",
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
        let (label, explanation) = match &self.access_mode {
            AccessMode::Standard => ("Standard", "Running with ordinary Windows access. Administrator mode may reveal more metadata.".to_owned()),
            AccessMode::Admin => ("Administrator", "Running with administrator access. File permissions are respected; some locations can still be inaccessible.".to_owned()),
            AccessMode::Unknown(error) => ("Access unknown", format!("Windows could not verify this process token: {error}")),
        };
        ui.label(RichText::new(label).small().color(p.accent))
            .on_hover_text(explanation);
        if !matches!(self.access_mode, AccessMode::Admin)
            && ui
                .add_enabled(
                    !self.management.busy() && !self.picker_open,
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
            || matches!(self.access_mode, AccessMode::Admin)
        {
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
                    // begin_scan ignores requests while an operation is active,
                    // so it runs after the operation is cleared and before the
                    // report (an operation) and the notice (cleared by it).
                    let rescanned = effects.rescan
                        && self.generation == binding.generation
                        && self.root_path == binding.root;
                    if rescanned {
                        self.begin_scan(binding.root.clone());
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
                        ui.label(RECYCLE_CONFIRMATION);
                        if confirmation.plan.recycle_verdict() == RecycleVerdict::Unverified {
                            ui.label(
                                RichText::new(
                                    "DiskScope could not fully check this drive's Recycle Bin settings.",
                                )
                                .small()
                                .color(p.muted),
                            );
                        }
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
        assert!(RECYCLE_CONFIRMATION.contains("Windows makes the final decision"));
        assert!(RECYCLE_CONFIRMATION.contains("choose No"));
        assert!(!RECYCLE_CONFIRMATION.contains("fails without falling back"));
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
        assert_eq!(AccessMode::from_probe(Ok(true)), AccessMode::Admin);
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
