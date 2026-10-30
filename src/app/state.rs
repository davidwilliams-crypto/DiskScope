//! Pure browse-state helpers: navigation history, pins, lenses and the command
//! palette. Nothing here touches egui, so every rule is unit-testable.
use super::*;
use diskscope::view::NO_EXTENSION;
use std::{collections::VecDeque, path::Path};

/// Back/Forward entries beyond this count drop the oldest context.
const BROWSE_HISTORY_LIMIT: usize = 128;
/// Pinned comparison capacity; the comparison tray shows `n/MAX_PINS`.
pub(super) const MAX_PINS: usize = 4;
/// Extension tokens accepted from one filter string.
const MAX_FILTER_EXTENSIONS: usize = 32;

#[derive(Clone, Debug)]
pub(super) struct BrowseContext {
    pub request: ViewRequest,
    pub changes: bool,
    pub older_scan: Option<u64>,
    pub newer_scan: Option<u64>,
}
impl std::ops::Deref for BrowseContext {
    type Target = ViewRequest;
    fn deref(&self) -> &ViewRequest {
        &self.request
    }
}
impl From<&ViewRequest> for BrowseContext {
    fn from(request: &ViewRequest) -> Self {
        Self {
            request: request.clone(),
            changes: false,
            older_scan: None,
            newer_scan: None,
        }
    }
}
impl From<&BrowseContext> for BrowseContext {
    fn from(context: &BrowseContext) -> Self {
        context.clone()
    }
}

#[derive(Default)]
pub(super) struct BrowseHistory {
    back: VecDeque<BrowseContext>,
    forward: Vec<BrowseContext>,
}
impl BrowseHistory {
    pub fn remember(&mut self, context: impl Into<BrowseContext>) {
        if self.back.len() == BROWSE_HISTORY_LIMIT {
            self.back.pop_front();
        }
        self.back.push_back(context.into());
        self.forward.clear();
    }
    pub fn back(&mut self, context: impl Into<BrowseContext>) -> Option<BrowseContext> {
        let previous = self.back.pop_back()?;
        self.forward.push(context.into());
        Some(previous)
    }
    pub fn forward(&mut self, context: impl Into<BrowseContext>) -> Option<BrowseContext> {
        let next = self.forward.pop()?;
        self.back.push_back(context.into());
        Some(next)
    }
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }
    pub fn clear(&mut self) {
        self.back.clear();
        self.forward.clear();
    }
}

/// A restored context keeps the live revision counter and the current pins;
/// everything else (root, selection, lens, page) comes from the saved entry.
pub(super) fn restore_browse_context(
    current: &ViewRequest,
    mut previous: BrowseContext,
) -> BrowseContext {
    previous.request.revision = current.revision;
    previous.request.pinned = current.pinned.clone();
    previous
}

/// Where to put the user once a rescan has discovered it again.
///
/// Node ids are handed out in discovery order, so every scan numbers the same
/// folder differently. The place is therefore kept as native paths and looked
/// up again. It stays pending while the scan is still running (the model does
/// not contain a folder until the scan reaches it) and ends only when the path
/// resolves or the scan has stopped.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PendingRestore {
    /// The scan root; the walk towards an ancestor never goes above it.
    scan_root: PathBuf,
    /// The folder to reopen. Replaced by its parent when it cannot be found.
    folder: PathBuf,
    /// A direct child of `folder` to select, when one is worth selecting.
    select: Option<PathBuf>,
    /// Set once `folder` was not found and an ancestor is being tried.
    fallen_back: Option<FallBack>,
}

/// The folder the user was in, remembered for the notice about losing it.
#[derive(Clone, Debug, PartialEq)]
struct FallBack {
    from: PathBuf,
    /// The scan stopped early, so the folder may exist without having been read.
    incomplete: bool,
    /// The path now names a file, so the folder was replaced rather than missed.
    now_a_file: bool,
}

/// What a snapshot means for a pending restore.
#[derive(Debug, PartialEq)]
pub(super) enum RestoreStep {
    /// The scan has not reached the target yet; ask again with the next snapshot.
    Wait,
    /// The target changed (the neighbour never appeared, or an ancestor is
    /// tried instead); ask again with [`PendingRestore::focus_path`].
    Retarget,
    /// Done: adopt this place.
    Land(Landing),
}

/// The place a restore ended at.
#[derive(Debug, PartialEq)]
pub(super) struct Landing {
    pub root: NodeId,
    pub selected: Option<NodeId>,
    /// The snapshot that resolved the path was built around a different
    /// folder (its parent), so the view must be requested again.
    pub rebuild: bool,
    /// Set when the folder was lost and a nearby place was opened instead.
    pub notice: Option<String>,
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn lost_folder_notice(lost: &FallBack, landed_in: Option<&Path>) -> String {
    let name = folder_name(&lost.from);
    let why = if lost.now_a_file {
        "is no longer a folder"
    } else if lost.incomplete {
        "was not found in these incomplete results"
    } else {
        "is no longer in the scan"
    };
    match landed_in {
        Some(parent) => format!(
            "{name} {why}, so DiskScope opened its closest remaining parent folder, {}.",
            folder_name(parent)
        ),
        None => format!("{name} {why}, so DiskScope opened the scan root."),
    }
}

impl PendingRestore {
    /// `None` when there is nothing to restore: the user was at the scan root
    /// with no neighbour worth selecting.
    pub fn new(scan_root: &Path, folder: &Path, select: Option<PathBuf>) -> Option<Self> {
        (folder != scan_root || select.is_some()).then(|| Self {
            scan_root: scan_root.to_path_buf(),
            folder: folder.to_path_buf(),
            select,
            fallen_back: None,
        })
    }

    /// The path the next view requests should focus on.
    pub fn focus_path(&self) -> &Path {
        self.select.as_deref().unwrap_or(&self.folder)
    }

    /// Gives up the neighbour selection (the user selected something else).
    /// True when the focus target changed and must be requested again.
    pub fn drop_selection(&mut self) -> bool {
        self.select.take().is_some()
    }

    /// Short text for the header while the restore is pending.
    pub fn status(&self) -> String {
        format!(
            "Restoring your place in {} once the scan reaches it.",
            folder_name(&self.folder)
        )
    }

    /// Decides what the newest admitted snapshot means. `view` must have been
    /// built from a request focused on [`Self::focus_path`]; a resolved focus
    /// shows up as the selected row carrying exactly that path.
    pub fn observe(&mut self, view: &ViewSnapshot, state: ScanState) -> RestoreStep {
        let target = self.focus_path();
        let found = view.selected.as_ref().filter(|row| row.path == target);
        if let Some(row) = found {
            if self.select.is_some() {
                return RestoreStep::Land(Landing {
                    root: row.parent.unwrap_or(0),
                    selected: Some(row.id),
                    rebuild: false,
                    notice: None,
                });
            }
            if row.is_dir {
                return RestoreStep::Land(Landing {
                    root: row.id,
                    selected: None,
                    rebuild: true,
                    notice: self
                        .fallen_back
                        .as_ref()
                        .map(|lost| lost_folder_notice(lost, Some(&self.folder))),
                });
            }
            // The path now names a file, which cannot turn back into a folder
            // during this scan: treat the folder as lost, but say it was
            // replaced rather than blaming incomplete results.
            return self.fall_back(state, true);
        }
        if state.is_active() {
            return RestoreStep::Wait;
        }
        // The scan has stopped and the target is still absent.
        if self.select.take().is_some() {
            // The neighbour is gone too; the folder itself is still wanted.
            return RestoreStep::Retarget;
        }
        self.fall_back(state, false)
    }

    /// Moves to the parent folder, or lands on the scan root when the walk is
    /// used up. `now_a_file` is set when the path resolved to a file.
    fn fall_back(&mut self, state: ScanState, now_a_file: bool) -> RestoreStep {
        let lost = self.fallen_back.get_or_insert_with(|| FallBack {
            from: self.folder.clone(),
            incomplete: state != ScanState::Complete && !now_a_file,
            now_a_file,
        });
        match self.folder.parent() {
            Some(parent) if parent != self.scan_root && parent.starts_with(&self.scan_root) => {
                self.folder = parent.to_path_buf();
                RestoreStep::Retarget
            }
            _ => RestoreStep::Land(Landing {
                root: 0,
                selected: None,
                rebuild: true,
                notice: Some(lost_folder_notice(lost, None)),
            }),
        }
    }
}

/// A browse position reduced to what survives a rescan.
#[derive(Debug)]
pub(super) struct SavedBrowse {
    /// Query settings with every node id removed.
    pub request: ViewRequest,
    /// Present when the user was somewhere other than the top of the scan.
    pub restore: Option<PendingRestore>,
}

/// The sibling to select after `removed` disappears: the next row under the
/// same parent, otherwise the previous one.
fn neighbour_of(rows: &[Row], removed: NodeId) -> Option<&Row> {
    let index = rows.iter().position(|row| row.id == removed)?;
    let parent = rows[index].parent;
    let sibling = |row: &&Row| row.parent == parent;
    rows[index + 1..]
        .iter()
        .find(sibling)
        .or_else(|| rows[..index].iter().rev().find(sibling))
}

/// Reduces the current browse position to what can be carried into a rescan of
/// `scan_root` after the file `removed` was acted on.
///
/// Query settings are kept as they are. Everything that holds a node id (root,
/// selection, expanded folders, pins) starts fresh because ids are reassigned,
/// and the page offset restarts at the first page because the listing changed.
/// The folder and a neighbouring row are kept as paths from `view`, which must
/// be the view currently displayed for `request` (`None` when it is not).
pub(super) fn save_browse(
    request: &ViewRequest,
    view: Option<&ViewSnapshot>,
    scan_root: &Path,
    removed: NodeId,
) -> SavedBrowse {
    let kept = ViewRequest {
        limit: request.limit,
        mode: request.mode,
        search: request.search.clone(),
        search_all: request.search_all,
        sort: request.sort,
        descending: request.descending,
        map_aspect: request.map_aspect,
        filters: request.filters.clone(),
        map_layout: request.map_layout,
        ..ViewRequest::default()
    };
    let restore = view
        .filter(|view| view.root == request.root)
        .and_then(|view| {
            // Only a direct child can be focused without leaving the folder.
            let select = neighbour_of(&view.rows, removed)
                .filter(|row| row.parent == Some(view.root))
                .map(|row| row.path.clone());
            PendingRestore::new(scan_root, &view.root_path, select)
        });
    SavedBrowse {
        request: kept,
        restore,
    }
}

/// `None` switches to the Changes page; `Some(mode)` switches to a workspace.
pub(super) fn transitioned(current: &BrowseContext, mode: Option<ViewMode>) -> BrowseContext {
    let mut next = current.clone();
    next.changes = mode.is_none();
    if let Some(mode) = mode {
        next.request.mode = mode;
    }
    next
}

/// Returns false only when a new pin would exceed [`MAX_PINS`].
pub(super) fn toggle_pin(pins: &mut Vec<NodeId>, id: NodeId) -> bool {
    if let Some(index) = pins.iter().position(|pin| *pin == id) {
        pins.remove(index);
        true
    } else if pins.len() < MAX_PINS {
        pins.push(id);
        true
    } else {
        false
    }
}

/// Parses a comma or whitespace separated extension list. Tokens are lowercased,
/// given a leading dot and deduplicated; the literal `(no extension)` bucket is
/// accepted as its own token.
pub(super) fn apply_extensions(filter: &mut QueryFilter, text: &str) {
    filter.extensions.clear();
    let lower = text.to_lowercase();
    let mut rest = lower.as_str();
    let is_separator = |c: char| c == ',' || c.is_whitespace();
    for _ in 0..MAX_FILTER_EXTENSIONS {
        rest = rest.trim_start_matches(is_separator);
        if rest.is_empty() {
            break;
        }
        let (extension, next) = if let Some(tail) = rest.strip_prefix(NO_EXTENSION)
            && tail.chars().next().is_none_or(is_separator)
        {
            (NO_EXTENSION.to_string(), tail)
        } else {
            let end = rest.find(is_separator).unwrap_or(rest.len());
            let token = rest[..end].trim_start_matches('.');
            let extension = if token.is_empty() {
                String::new()
            } else {
                format!(".{token}")
            };
            (extension, &rest[end..])
        };
        if !extension.is_empty() && !filter.extensions.contains(&extension) {
            filter.extensions.push(extension);
        }
        rest = next;
    }
}

pub(super) fn apply_type_lens(request: &mut ViewRequest, extension: &str) {
    apply_extensions(&mut request.filters, extension);
    request.mode = ViewMode::Explorer;
}

/// Jump to one native path from a comparison: lenses and paging are cleared so
/// the item cannot be hidden by a stale filter.
pub(super) fn focus_change_request(request: &mut ViewRequest, path: PathBuf) {
    request.search.clear();
    request.search_all = false;
    request.filters = QueryFilter::default();
    request.offset = 0;
    request.expanded.clear();
    request.focus_path = Some(path);
    request.mode = ViewMode::Explorer;
}

pub(super) fn comparison_matches(
    older: Option<u64>,
    newer: Option<u64>,
    actual: (u64, u64),
) -> bool {
    older == Some(actual.0) && newer == Some(actual.1)
}

/// The newest scan and its most recent compatible baseline for `root`.
pub(super) fn previous_pair(
    scans: &[scan_history::ScanSummary],
    root: &std::path::Path,
) -> Option<(u64, u64)> {
    let mut matching = scans.iter().filter(|s| s.root == root);
    let newer = matching.next()?;
    let older = matching.find(|older| {
        older.id != newer.id && scan_history::compatible_backends(&older.backend, &newer.backend)
    })?;
    Some((older.id, newer.id))
}

/// Validate catalog choices before a request reaches the worker. Paths retain
/// native identity; same-named folders and different backend families differ.
pub(super) fn comparison_pair(
    scans: &[scan_history::ScanSummary],
    root: &Path,
    older: Option<u64>,
    newer: Option<u64>,
) -> Result<(u64, u64), &'static str> {
    let (older, newer) = older
        .zip(newer)
        .ok_or("Select a Before and an After scan.")?;
    if older == newer {
        return Err("Before and After must be different scans.");
    }
    let find = |id| scans.iter().find(|s| s.id == id);
    let (a, b) = find(older)
        .zip(find(newer))
        .ok_or("A selected scan is no longer saved. Choose again.")?;
    if a.root != root || b.root != root {
        return Err("Choose two scans of the selected full root path.");
    }
    if !scan_history::compatible_backends(&a.backend, &b.backend) {
        return Err(
            "Normal scans and raw NTFS preview scans cannot be compared. Choose the same backend family.",
        );
    }
    Ok((older, newer))
}

pub(super) fn overview_type_share(bytes: u64, view: &ViewSnapshot) -> f32 {
    share(bytes, view.row_scope_logical)
}

/// A passive worker update is held back only while the stable-display
/// preference is on and neither the revision nor the selection changed.
pub(super) fn hold_view(
    stable: bool,
    visible_revision: u64,
    incoming_revision: u64,
    selected_changed: bool,
) -> bool {
    stable && visible_revision == incoming_revision && !selected_changed
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Command {
    Overview,
    Explorer,
    Largest,
    Types,
    Changes,
    Back,
    Forward,
    Root,
    Stop,
    Pin,
    Filters,
    Theme,
}
impl Command {
    const ALL: [Command; 12] = [
        Command::Overview,
        Command::Explorer,
        Command::Largest,
        Command::Types,
        Command::Changes,
        Command::Back,
        Command::Forward,
        Command::Root,
        Command::Stop,
        Command::Pin,
        Command::Filters,
        Command::Theme,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Explorer => "Explorer",
            Self::Largest => "Largest files",
            Self::Types => "File types",
            Self::Changes => "Changes between scans",
            Self::Back => "Back",
            Self::Forward => "Forward",
            Self::Root => "Go to scan root",
            Self::Stop => "Stop scan · keep results",
            Self::Pin => "Pin / unpin selection",
            Self::Filters => "Edit filters",
            Self::Theme => "Switch theme",
        }
    }
}
/// What the command palette needs to know to tell which commands can run.
#[derive(Clone, Copy, Debug)]
pub(super) struct PaletteFacts {
    pub scan_loaded: bool,
    /// The displayed scan state, with a pending stop request folded in.
    pub scan_state: ScanState,
    pub can_back: bool,
    pub can_forward: bool,
    pub has_selection: bool,
    pub selection_pinned: bool,
    pub pins_full: bool,
    pub history_saved: bool,
    pub on_changes_page: bool,
}

impl Command {
    /// Why this command cannot run right now, or `None` when it can. The
    /// palette shows the reason and runs nothing for an unavailable command.
    pub fn unavailable(self, facts: &PaletteFacts) -> Option<&'static str> {
        match self {
            Self::Stop => match facts.scan_state {
                ScanState::Cancelling => Some("Stopping has already been requested."),
                state if !state.is_active() => Some("No scan is running."),
                _ => None,
            },
            Self::Back => (!facts.can_back).then_some("Nothing earlier to go back to."),
            Self::Forward => (!facts.can_forward).then_some("Nothing later to go forward to."),
            Self::Root => (!facts.scan_loaded).then_some("Scan a folder first."),
            Self::Pin => {
                if !facts.has_selection {
                    Some("Select a folder or file first.")
                } else if facts.pins_full && !facts.selection_pinned {
                    Some("The comparison is full. Unpin an item first.")
                } else {
                    None
                }
            }
            Self::Changes => {
                if !facts.history_saved {
                    Some("Scan history is not saved in this window.")
                } else if facts.on_changes_page {
                    Some("Already showing changes between scans.")
                } else {
                    None
                }
            }
            Self::Overview
            | Self::Explorer
            | Self::Largest
            | Self::Types
            | Self::Filters
            | Self::Theme => None,
        }
    }
}
pub(super) fn command_matches(query: &str) -> Vec<Command> {
    let query = query.trim().to_lowercase();
    Command::ALL
        .into_iter()
        .filter(|c| c.label().to_lowercase().contains(&query))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline(id: u64, root: &str, backend: &str) -> scan_history::ScanSummary {
        scan_history::ScanSummary {
            id,
            root: root.into(),
            finished_unix: id,
            logical: 1,
            files: 1,
            folders: 0,
            backend: backend.into(),
            live_generation: None,
        }
    }

    #[test]
    fn previous_pair_skips_incompatible_backends_and_same_named_other_roots() {
        let scans = vec![
            baseline(5, "/a/data", "Normal"),
            baseline(4, "/b/data", "Normal"),
            baseline(3, "/a/data", "NTFS volume preview (raw)"),
            baseline(2, "/a/data", "Normal fallback (unsupported)"),
        ];
        assert_eq!(previous_pair(&scans, Path::new("/a/data")), Some((2, 5)));
        assert_eq!(previous_pair(&scans[..3], Path::new("/a/data")), None);
    }

    #[test]
    fn comparison_admission_requires_saved_distinct_same_root_same_family_scans() {
        let scans = vec![
            baseline(5, "/a/data", "Normal"),
            baseline(4, "/a/data", "Normal fallback (unsupported)"),
            baseline(3, "/a/data", "NTFS volume preview (raw)"),
            baseline(2, "/b/data", "Normal"),
            baseline(1, "/a/data", "NTFS volume preview (fast)"),
        ];
        let root = Path::new("/a/data");
        for (before, after) in [
            (None, Some(5)),
            (Some(5), Some(5)),
            (Some(99), Some(5)),
            (Some(2), Some(5)),
            (Some(3), Some(5)),
        ] {
            assert!(comparison_pair(&scans, root, before, after).is_err());
        }
        assert_eq!(comparison_pair(&scans, root, Some(4), Some(5)), Ok((4, 5)));
        assert_eq!(comparison_pair(&scans, root, Some(5), Some(4)), Ok((5, 4)));
        assert_eq!(comparison_pair(&scans, root, Some(1), Some(3)), Ok((1, 3)));
    }

    fn file(name: &str, bytes: u64) -> Entry {
        Entry {
            name: name.into(),
            is_dir: false,
            reparse: false,
            logical: bytes,
            allocated: Some(bytes),
            modified: None,
            identity: None,
        }
    }
    fn directory(name: &str) -> Entry {
        Entry {
            name: name.into(),
            is_dir: true,
            reparse: false,
            logical: 0,
            allocated: Some(0),
            modified: None,
            identity: None,
        }
    }

    #[test]
    fn navigation_restores_lens_selection_and_invalidates_forward_on_new_branch() {
        let mut history = BrowseHistory::default();
        let a = ViewRequest {
            root: 4,
            selected: Some(8),
            mode: ViewMode::Types,
            filters: QueryFilter {
                extensions: vec![".rs".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        history.remember(&a);
        let b = ViewRequest {
            root: 12,
            selected: Some(14),
            ..Default::default()
        };
        let restored = history.back(&b).expect("Back restores the prior context");
        assert_eq!(
            (restored.root, restored.selected, restored.mode),
            (4, Some(8), ViewMode::Types)
        );
        assert_eq!(restored.filters.extensions, [".rs"]);
        assert_eq!(history.forward(&restored).unwrap().selected, Some(14));
        history.back(&b);
        history.remember(&a);
        assert!(!history.can_forward());
    }

    #[test]
    fn browse_history_drops_the_oldest_entry_beyond_its_limit() {
        let mut history = BrowseHistory::default();
        for root in 0..=BROWSE_HISTORY_LIMIT as NodeId {
            history.remember(&ViewRequest {
                root,
                ..Default::default()
            });
        }
        let mut roots = Vec::new();
        while let Some(context) = history.back(&ViewRequest::default()) {
            roots.push(context.root);
        }
        assert_eq!(roots.len(), BROWSE_HISTORY_LIMIT);
        assert_eq!(roots.first(), Some(&(BROWSE_HISTORY_LIMIT as NodeId)));
        assert_eq!(roots.last(), Some(&1));
    }

    #[test]
    fn compare_pin_cap_is_four_and_existing_pin_can_always_be_removed() {
        let mut pins = Vec::new();
        for id in 1..=4 {
            assert!(toggle_pin(&mut pins, id));
        }
        assert!(!toggle_pin(&mut pins, 5));
        assert_eq!(pins, [1, 2, 3, 4]);
        assert!(toggle_pin(&mut pins, 2));
        assert!(toggle_pin(&mut pins, 5));
        assert_eq!(pins, [1, 3, 4, 5]);
    }

    #[test]
    fn extension_lens_is_explicit_normalized_and_clearable() {
        let mut filter = QueryFilter {
            min_bytes: Some(1024),
            ..Default::default()
        };
        apply_extensions(&mut filter, "RS, .PNG rs");
        assert_eq!(filter.extensions, [".rs", ".png"]);
        assert_eq!(filter.min_bytes, Some(1024));
        apply_extensions(&mut filter, "");
        assert!(filter.extensions.is_empty());
    }

    #[test]
    fn palette_routes_named_keyboard_actions() {
        assert_eq!(command_matches("largest"), [Command::Largest]);
        assert_eq!(command_matches("stop"), [Command::Stop]);
        assert!(command_matches("nonsense").is_empty());
        assert_eq!(command_matches("  ").len(), Command::ALL.len());
    }

    #[test]
    fn stable_admission_holds_only_passive_updates_not_navigation_or_selection() {
        assert!(hold_view(true, 7, 7, false));
        assert!(!hold_view(true, 7, 8, false));
        assert!(!hold_view(true, 7, 7, true));
        assert!(!hold_view(false, 7, 7, false));
        assert!(!admits_snapshot(4, 7, 3, 7));
        assert!(!admits_snapshot(4, 7, 4, 6));
    }

    #[test]
    fn changed_item_focus_clears_lens_but_back_keeps_it() {
        let mut request = ViewRequest {
            search: "video".into(),
            filters: QueryFilter {
                extensions: vec![".mp4".into()],
                ..Default::default()
            },
            offset: 256,
            pinned: vec![3],
            ..Default::default()
        };
        let mut history = BrowseHistory::default();
        history.remember(&request);
        let path = PathBuf::from("C:/fixture/資料.txt");
        focus_change_request(&mut request, path.clone());
        assert!(!request.filters.is_active());
        assert!(request.search.is_empty());
        assert_eq!(request.offset, 0);
        assert_eq!(request.focus_path, Some(path));
        assert_eq!(request.pinned, [3]);
        assert_eq!(history.back(&request).unwrap().filters.extensions, [".mp4"]);
    }

    #[test]
    fn comparison_result_must_match_both_selected_baselines() {
        assert!(comparison_matches(Some(1), Some(2), (1, 2)));
        assert!(!comparison_matches(Some(1), Some(3), (1, 2)));
        assert!(!comparison_matches(None, Some(2), (1, 2)));
    }

    #[test]
    fn no_extension_bucket_click_and_edit_roundtrip_use_real_query() {
        let mut model = Model::new("C:/fixture".into());
        model.insert_batch(
            0,
            vec![
                file("README", 200),
                file("a.txt", 100),
                file("photo.png", 999),
            ],
        );
        let types = diskscope::view::build_view(
            &model,
            &ViewRequest {
                mode: ViewMode::Types,
                ..Default::default()
            },
            1,
        );
        let bucket = &types
            .types
            .iter()
            .find(|t| t.extension == NO_EXTENSION)
            .unwrap()
            .extension;
        let mut request = ViewRequest::default();
        apply_type_lens(&mut request, bucket);
        let clicked = diskscope::view::build_view(&model, &request, 1);
        assert_eq!((clicked.matching_files, clicked.matching_logical), (1, 200));
        assert_eq!(clicked.rows[0].name, "README");
        let edit = request.filters.extensions.join(", ");
        apply_extensions(&mut request.filters, &edit);
        let edited = diskscope::view::build_view(&model, &request, 1);
        assert_eq!((edited.matching_files, edited.matching_logical), (1, 200));
        apply_extensions(&mut request.filters, ".txt, (no extension)");
        let mixed = diskscope::view::build_view(&model, &request, 1);
        assert_eq!((mixed.matching_files, mixed.matching_logical), (2, 300));
        assert_eq!(request.filters.extensions, [".txt", NO_EXTENSION]);
    }

    #[test]
    fn overview_type_bar_uses_whole_scan_scope_but_folder_share_keeps_map_scope() {
        let mut model = Model::new("C:/fixture".into());
        let folder = model.insert_batch(0, vec![directory("small")])[0];
        model.insert_batch(folder, vec![file("inside.txt", 100)]);
        model.insert_batch(
            0,
            vec![file("outside.txt", 900), file("unmatched.bin", 9000)],
        );
        let view = diskscope::view::build_view(
            &model,
            &ViewRequest {
                root: folder,
                mode: ViewMode::Overview,
                search_all: true,
                filters: QueryFilter {
                    extensions: vec![".txt".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            1,
        );
        assert_eq!((view.total_logical, view.row_scope_logical), (100, 10000));
        assert_eq!(view.types[0].logical, 1000);
        assert_eq!(overview_type_share(view.types[0].logical, &view), 0.1);
        assert_eq!(share(100, view.total_logical), 1.0);
    }

    #[test]
    fn restoration_keeps_saved_page_through_production_helper() {
        let current = ViewRequest {
            revision: 17,
            pinned: vec![3],
            ..Default::default()
        };
        let saved = ViewRequest {
            offset: 256,
            selected: Some(7),
            ..Default::default()
        };
        let restored = restore_browse_context(&current, BrowseContext::from(&saved));
        assert_eq!(restored.offset, 256);
        assert_eq!(restored.revision, 17);
        assert_eq!(restored.pinned, [3]);
    }

    /// `/scan/a/{x.bin, y.bin, z.bin}` plus `/scan/top.bin`. Returns the model
    /// and the id of `a`.
    fn restore_model() -> (Model, NodeId) {
        let mut model = Model::new("/scan".into());
        let ids = model.insert_batch(0, vec![directory("a"), file("top.bin", 5)]);
        let a = ids[0];
        model.insert_batch(
            a,
            vec![file("x.bin", 30), file("y.bin", 20), file("z.bin", 10)],
        );
        (model, a)
    }

    /// The view the worker would build for a request focused on `path`.
    fn focused_view(model: &Model, path: &str) -> ViewSnapshot {
        let request = ViewRequest {
            focus_path: Some(PathBuf::from(path)),
            ..Default::default()
        };
        diskscope::view::build_view(model, &request, 1)
    }

    fn pending(folder: &str, select: Option<&str>) -> PendingRestore {
        PendingRestore::new(
            Path::new("/scan"),
            Path::new(folder),
            select.map(PathBuf::from),
        )
        .expect("a folder below the root needs restoring")
    }

    #[test]
    fn restore_stays_pending_until_the_scan_reaches_the_folder() {
        let mut model = Model::new("/scan".into());
        let a = model.insert_batch(0, vec![directory("a")])[0];
        let mut restore = pending("/scan/a/b", None);
        // The scan is still running and has not listed `a` yet: not found is
        // not an answer, however many snapshots arrive.
        for _ in 0..3 {
            let view = focused_view(&model, "/scan/a/b");
            assert!(view.selected.is_none(), "the folder is not in the model");
            assert_eq!(
                restore.observe(&view, ScanState::Scanning),
                RestoreStep::Wait
            );
        }
        let b = model.insert_batch(a, vec![directory("b")])[0];
        let view = focused_view(&model, "/scan/a/b");
        assert_eq!(
            restore.observe(&view, ScanState::Scanning),
            RestoreStep::Land(Landing {
                root: b,
                selected: None,
                // The resolving view is built around the parent `a`.
                rebuild: true,
                notice: None,
            })
        );
    }

    #[test]
    fn restore_selects_a_neighbour_inside_the_folder_without_rebuilding() {
        let (model, a) = restore_model();
        let mut restore = pending("/scan/a", Some("/scan/a/y.bin"));
        assert_eq!(restore.focus_path(), Path::new("/scan/a/y.bin"));
        let view = focused_view(&model, "/scan/a/y.bin");
        let y = view.selected.as_ref().map(|row| row.id);
        assert_eq!(
            restore.observe(&view, ScanState::Scanning),
            RestoreStep::Land(Landing {
                root: a,
                selected: y,
                rebuild: false,
                notice: None,
            })
        );
    }

    #[test]
    fn a_neighbour_that_never_appears_falls_back_to_just_the_folder() {
        let (model, a) = restore_model();
        let mut restore = pending("/scan/a", Some("/scan/a/gone.bin"));
        let missing = focused_view(&model, "/scan/a/gone.bin");
        // While scanning, the neighbour may simply not be listed yet.
        assert_eq!(
            restore.observe(&missing, ScanState::Scanning),
            RestoreStep::Wait
        );
        assert_eq!(restore.focus_path(), Path::new("/scan/a/gone.bin"));
        // Once the scan has stopped it cannot appear any more.
        assert_eq!(
            restore.observe(&missing, ScanState::Complete),
            RestoreStep::Retarget
        );
        assert_eq!(restore.focus_path(), Path::new("/scan/a"));
        let folder = focused_view(&model, "/scan/a");
        assert_eq!(
            restore.observe(&folder, ScanState::Complete),
            RestoreStep::Land(Landing {
                root: a,
                selected: None,
                rebuild: true,
                notice: None,
            })
        );
    }

    #[test]
    fn a_lost_folder_opens_the_closest_surviving_parent_and_says_so() {
        let (model, a) = restore_model();
        let mut restore = pending("/scan/a/b/c", None);
        let view = focused_view(&model, "/scan/a/b/c");
        assert_eq!(
            restore.observe(&view, ScanState::Complete),
            RestoreStep::Retarget
        );
        assert_eq!(restore.focus_path(), Path::new("/scan/a/b"));
        let view = focused_view(&model, "/scan/a/b");
        assert_eq!(
            restore.observe(&view, ScanState::Complete),
            RestoreStep::Retarget
        );
        assert_eq!(restore.focus_path(), Path::new("/scan/a"));
        let view = focused_view(&model, "/scan/a");
        let RestoreStep::Land(landing) = restore.observe(&view, ScanState::Complete) else {
            panic!("the parent exists, so the walk must end there");
        };
        assert_eq!(
            (landing.root, landing.selected, landing.rebuild),
            (a, None, true)
        );
        assert_eq!(
            landing.notice.as_deref(),
            Some(
                "c is no longer in the scan, so DiskScope opened its closest remaining parent folder, a."
            )
        );
    }

    #[test]
    fn a_lost_top_level_folder_opens_the_scan_root_and_says_so() {
        let (model, _) = restore_model();
        let mut restore = pending("/scan/vanished", None);
        let view = focused_view(&model, "/scan/vanished");
        let RestoreStep::Land(landing) = restore.observe(&view, ScanState::Complete) else {
            panic!("nothing is left to try below the scan root");
        };
        assert_eq!(
            (landing.root, landing.selected, landing.rebuild),
            (0, None, true)
        );
        assert_eq!(
            landing.notice.as_deref(),
            Some("vanished is no longer in the scan, so DiskScope opened the scan root.")
        );
    }

    #[test]
    fn an_incomplete_scan_never_claims_the_folder_was_removed() {
        let (model, _) = restore_model();
        for state in [ScanState::Cancelled, ScanState::Partial, ScanState::Failed] {
            let mut restore = pending("/scan/vanished", None);
            let view = focused_view(&model, "/scan/vanished");
            let RestoreStep::Land(landing) = restore.observe(&view, state) else {
                panic!("{state:?} is terminal");
            };
            let notice = landing.notice.expect("a lost folder is announced");
            assert!(notice.contains("incomplete results"), "{state:?}: {notice}");
            assert!(!notice.contains("no longer"), "{state:?}: {notice}");
        }
    }

    #[test]
    fn a_folder_replaced_by_a_file_is_lost_even_while_scanning() {
        let (model, _) = restore_model();
        // `/scan/top.bin` exists as a file; it cannot become a folder later.
        let mut restore = pending("/scan/top.bin", None);
        let view = focused_view(&model, "/scan/top.bin");
        assert!(view.selected.is_some());
        let RestoreStep::Land(landing) = restore.observe(&view, ScanState::Scanning) else {
            panic!("a file is a definite answer");
        };
        assert_eq!(landing.root, 0);
        // The folder was replaced, so the notice must not blame incomplete
        // results even though the scan is still running.
        let notice = landing.notice.expect("a lost folder is announced");
        assert!(notice.contains("is no longer a folder"), "{notice}");
        assert!(!notice.contains("incomplete"), "{notice}");
    }

    #[test]
    fn nothing_is_restored_at_the_scan_root_unless_a_neighbour_is_wanted() {
        let root = Path::new("/scan");
        assert!(PendingRestore::new(root, root, None).is_none());
        let with_neighbour = PendingRestore::new(root, root, Some("/scan/top.bin".into()));
        assert!(with_neighbour.is_some());
    }

    #[test]
    fn user_selection_drops_only_the_neighbour_not_the_folder() {
        let mut restore = pending("/scan/a", Some("/scan/a/y.bin"));
        assert!(restore.drop_selection());
        assert_eq!(restore.focus_path(), Path::new("/scan/a"));
        assert!(!restore.drop_selection(), "already dropped");
    }

    fn row_at(id: NodeId, parent: NodeId, name: &str) -> Row {
        Row {
            id,
            parent: Some(parent),
            depth: 0,
            name: name.into(),
            path: PathBuf::from("/scan/a").join(name),
            is_dir: false,
            reparse: false,
            identity: None,
            hard_link: false,
            logical: 0,
            allocated: None,
            files: 0,
            folders: 0,
            modified: None,
            has_children: false,
        }
    }

    fn view_of(root: NodeId, rows: Vec<Row>) -> ViewSnapshot {
        ViewSnapshot {
            root,
            root_path: PathBuf::from("/scan/a"),
            rows,
            ..Default::default()
        }
    }

    #[test]
    fn saved_browse_keeps_the_query_and_forgets_every_node_id() {
        let mut request = ViewRequest {
            root: 7,
            selected: Some(9),
            expanded: [3, 4].into_iter().collect(),
            mode: ViewMode::Largest,
            search: "log".into(),
            search_all: true,
            sort: SortKey::Name,
            descending: false,
            offset: 256,
            limit: 256,
            map_aspect: 1.5,
            filters: QueryFilter {
                extensions: vec![".log".into()],
                min_bytes: Some(10),
                ..Default::default()
            },
            map_layout: MapLayout::Icicle,
            pinned: vec![2],
            focus_path: Some("/scan/elsewhere".into()),
            ..Default::default()
        };
        request.revision = 41;
        let view = view_of(7, vec![row_at(9, 7, "gone.log")]);
        let saved = save_browse(&request, Some(&view), Path::new("/scan"), 9);
        let kept = saved.request;
        assert_eq!(kept.mode, ViewMode::Largest);
        assert_eq!(kept.search, "log");
        assert!(kept.search_all);
        assert_eq!(kept.sort, SortKey::Name);
        assert!(!kept.descending);
        assert_eq!(kept.limit, 256);
        assert_eq!(kept.map_aspect, 1.5);
        assert_eq!(kept.map_layout, MapLayout::Icicle);
        assert_eq!(kept.filters.extensions, [".log"]);
        assert_eq!(kept.filters.min_bytes, Some(10));
        // Ids and one-shot state start fresh.
        assert_eq!((kept.root, kept.selected, kept.offset), (0, None, 0));
        assert!(kept.expanded.is_empty() && kept.pinned.is_empty());
        assert_eq!((kept.revision, kept.focus_path), (0, None));
        // The folder survives as a path.
        let restore = saved.restore.expect("the user was inside a folder");
        assert_eq!(restore.focus_path(), Path::new("/scan/a"));
    }

    #[test]
    fn the_next_sibling_is_selected_else_the_previous_one() {
        let rows = vec![
            row_at(1, 7, "first"),
            row_at(2, 7, "removed"),
            row_at(3, 7, "next"),
        ];
        let request = ViewRequest {
            root: 7,
            ..Default::default()
        };
        let view = view_of(7, rows.clone());
        let saved = save_browse(&request, Some(&view), Path::new("/scan"), 2);
        let restore = saved.restore.unwrap();
        assert_eq!(restore.focus_path(), Path::new("/scan/a/next"));

        let last = view_of(7, rows[..2].to_vec());
        let saved = save_browse(&request, Some(&last), Path::new("/scan"), 2);
        assert_eq!(
            saved.restore.unwrap().focus_path(),
            Path::new("/scan/a/first")
        );
    }

    #[test]
    fn a_neighbour_must_be_a_sibling_and_a_direct_child_of_the_folder() {
        // The removed file sits inside an expanded subfolder (parent 8), so
        // its neighbour is not a direct child of the folder (7): focusing it
        // would leave the folder, so only the folder is restored.
        let rows = vec![
            row_at(8, 7, "sub"),
            row_at(9, 8, "removed"),
            row_at(10, 8, "other"),
        ];
        let request = ViewRequest {
            root: 7,
            ..Default::default()
        };
        let view = view_of(7, rows);
        let saved = save_browse(&request, Some(&view), Path::new("/scan"), 9);
        assert_eq!(saved.restore.unwrap().focus_path(), Path::new("/scan/a"));

        // The only row removed: nothing to select, but the folder is kept.
        let view = view_of(7, vec![row_at(9, 7, "removed")]);
        let saved = save_browse(&request, Some(&view), Path::new("/scan"), 9);
        assert_eq!(saved.restore.unwrap().focus_path(), Path::new("/scan/a"));
    }

    #[test]
    fn a_stale_or_missing_view_restores_the_query_but_not_the_folder() {
        let request = ViewRequest {
            root: 7,
            sort: SortKey::Files,
            ..Default::default()
        };
        // The displayed view belongs to another folder than the request.
        let view = view_of(3, vec![row_at(9, 3, "removed")]);
        let saved = save_browse(&request, Some(&view), Path::new("/scan"), 9);
        assert!(saved.restore.is_none());
        assert_eq!(saved.request.sort, SortKey::Files);
        let saved = save_browse(&request, None, Path::new("/scan"), 9);
        assert!(saved.restore.is_none());
        assert_eq!(saved.request.sort, SortKey::Files);
    }

    #[test]
    fn a_user_at_the_scan_root_needs_no_folder_but_keeps_a_neighbour() {
        let request = ViewRequest::default();
        let mut view = view_of(0, vec![row_at(1, 0, "removed"), row_at(2, 0, "next")]);
        view.root_path = PathBuf::from("/scan");
        let saved = save_browse(&request, Some(&view), Path::new("/scan"), 1);
        assert_eq!(
            saved.restore.unwrap().focus_path(),
            Path::new("/scan/a/next")
        );
        view.rows.truncate(1);
        assert!(
            save_browse(&request, Some(&view), Path::new("/scan"), 1)
                .restore
                .is_none()
        );
    }

    fn facts() -> PaletteFacts {
        PaletteFacts {
            scan_loaded: true,
            scan_state: ScanState::Scanning,
            can_back: true,
            can_forward: true,
            has_selection: true,
            selection_pinned: false,
            pins_full: false,
            history_saved: true,
            on_changes_page: false,
        }
    }

    #[test]
    fn every_command_runs_when_its_preconditions_hold() {
        for command in Command::ALL {
            assert_eq!(command.unavailable(&facts()), None, "{command:?}");
        }
    }

    #[test]
    fn commands_that_cannot_run_say_why() {
        let blocked = [
            (
                Command::Stop,
                PaletteFacts {
                    scan_state: ScanState::Complete,
                    ..facts()
                },
            ),
            (
                Command::Stop,
                PaletteFacts {
                    scan_state: ScanState::Idle,
                    ..facts()
                },
            ),
            (
                Command::Stop,
                PaletteFacts {
                    scan_state: ScanState::Cancelling,
                    ..facts()
                },
            ),
            (
                Command::Back,
                PaletteFacts {
                    can_back: false,
                    ..facts()
                },
            ),
            (
                Command::Forward,
                PaletteFacts {
                    can_forward: false,
                    ..facts()
                },
            ),
            (
                Command::Root,
                PaletteFacts {
                    scan_loaded: false,
                    ..facts()
                },
            ),
            (
                Command::Pin,
                PaletteFacts {
                    has_selection: false,
                    ..facts()
                },
            ),
            (
                Command::Pin,
                PaletteFacts {
                    pins_full: true,
                    ..facts()
                },
            ),
            (
                Command::Changes,
                PaletteFacts {
                    history_saved: false,
                    ..facts()
                },
            ),
            (
                Command::Changes,
                PaletteFacts {
                    on_changes_page: true,
                    ..facts()
                },
            ),
        ];
        for (command, facts) in blocked {
            let reason = command.unavailable(&facts);
            assert!(
                reason.is_some_and(|text| text.ends_with('.')),
                "{command:?} with {facts:?}"
            );
        }
    }

    #[test]
    fn a_full_comparison_still_lets_the_user_unpin_the_selection() {
        let full_but_pinned = PaletteFacts {
            pins_full: true,
            selection_pinned: true,
            ..facts()
        };
        assert_eq!(Command::Pin.unavailable(&full_but_pinned), None);
    }

    #[test]
    fn changes_is_a_destination_in_back_forward_history() {
        let overview = BrowseContext::from(&ViewRequest {
            mode: ViewMode::Overview,
            selected: Some(8),
            filters: QueryFilter {
                extensions: vec![".rs".into()],
                ..Default::default()
            },
            ..Default::default()
        });
        let mut history = BrowseHistory::default();
        history.remember(&overview);
        let explorer = transitioned(&overview, Some(ViewMode::Explorer));
        history.remember(&explorer);
        let mut changes = transitioned(&explorer, None);
        changes.older_scan = Some(10);
        changes.newer_scan = Some(20);
        let back = history.back(&changes).unwrap();
        assert_eq!(back.mode, ViewMode::Explorer);
        assert!(!back.changes);
        assert_eq!(back.selected, Some(8));
        assert_eq!(back.filters.extensions, [".rs"]);
        let forward = history.forward(&back).unwrap();
        assert!(forward.changes);
        assert_eq!(
            (forward.older_scan, forward.newer_scan),
            (Some(10), Some(20))
        );
        assert_eq!(forward.selected, Some(8));
        assert_eq!(forward.filters.extensions, [".rs"]);
    }
}
