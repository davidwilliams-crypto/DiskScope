use super::*;

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
    back: Vec<BrowseContext>,
    forward: Vec<BrowseContext>,
}
impl BrowseHistory {
    pub fn remember(&mut self, context: impl Into<BrowseContext>) {
        if self.back.len() == 128 {
            self.back.remove(0);
        }
        self.back.push(context.into());
        self.forward.clear();
    }
    pub fn back(&mut self, context: impl Into<BrowseContext>) -> Option<BrowseContext> {
        let previous = self.back.pop()?;
        self.forward.push(context.into());
        Some(previous)
    }
    pub fn forward(&mut self, context: impl Into<BrowseContext>) -> Option<BrowseContext> {
        let next = self.forward.pop()?;
        self.back.push(context.into());
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
pub(super) fn restore_browse_context(
    current: &ViewRequest,
    mut previous: BrowseContext,
) -> BrowseContext {
    previous.request.revision = current.revision;
    previous.request.pinned = current.pinned.clone();
    previous
}
pub(super) fn transitioned(current: &BrowseContext, mode: Option<ViewMode>) -> BrowseContext {
    let mut next = current.clone();
    next.changes = mode.is_none();
    if let Some(mode) = mode {
        next.request.mode = mode;
    }
    next
}
pub(super) fn toggle_pin(pins: &mut Vec<NodeId>, id: NodeId) -> bool {
    if let Some(index) = pins.iter().position(|pin| *pin == id) {
        pins.remove(index);
        true
    } else if pins.len() < 4 {
        pins.push(id);
        true
    } else {
        false
    }
}
pub(super) fn apply_extensions(filter: &mut QueryFilter, text: &str) {
    filter.extensions.clear();
    let lower = text.to_lowercase();
    let mut rest = lower.as_str();
    for _ in 0..32 {
        rest = rest.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
        if rest.is_empty() {
            break;
        }
        let sentinel = "(no extension)";
        let (extension, next) = if let Some(tail) = rest.strip_prefix(sentinel)
            && tail
                .chars()
                .next()
                .is_none_or(|c| c == ',' || c.is_whitespace())
        {
            (sentinel.to_string(), tail)
        } else {
            let end = rest
                .find(|c: char| c == ',' || c.is_whitespace())
                .unwrap_or(rest.len());
            let token = rest[..end].trim_start_matches('.');
            (
                if token.is_empty() {
                    String::new()
                } else {
                    format!(".{token}")
                },
                &rest[end..],
            )
        };
        if !extension.is_empty() && !filter.extensions.contains(&extension) {
            filter.extensions.push(extension);
        }
        rest = next;
    }
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
pub(super) fn command_matches(query: &str) -> Vec<Command> {
    [
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
    ]
    .into_iter()
    .filter(|c| {
        c.label()
            .to_lowercase()
            .contains(&query.trim().to_lowercase())
    })
    .collect()
}
pub(super) fn hold_view(
    stable: bool,
    visible_revision: u64,
    incoming_revision: u64,
    selected_changed: bool,
) -> bool {
    stable && visible_revision == incoming_revision && !selected_changed
}
#[cfg(test)]
mod tests {
    use super::*;
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
}

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
#[cfg(test)]
mod focus_tests {
    use super::*;
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
}

pub(super) fn previous_pair(
    scans: &[scan_history::ScanSummary],
    root: &std::path::Path,
) -> Option<(u64, u64)> {
    let mut matching = scans.iter().filter(|s| s.root == root);
    let newer = matching.next()?;
    let older = matching.next()?;
    Some((older.id, newer.id))
}

pub(super) fn apply_type_lens(request: &mut ViewRequest, extension: &str) {
    apply_extensions(&mut request.filters, extension);
    request.mode = ViewMode::Explorer;
}
pub(super) fn overview_type_share(bytes: u64, view: &ViewSnapshot) -> f32 {
    share(bytes, view.row_scope_logical)
}
#[cfg(test)]
mod review_regressions {
    use super::*;
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
            .find(|t| t.extension == "(no extension)")
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
        assert_eq!(request.filters.extensions, [".txt", "(no extension)"]);
    }
    #[test]
    fn overview_type_bar_uses_whole_scan_scope_but_folder_share_keeps_map_scope() {
        let mut model = Model::new("C:/fixture".into());
        let folder = model.insert_batch(
            0,
            vec![Entry {
                name: "small".into(),
                is_dir: true,
                reparse: false,
                logical: 0,
                allocated: Some(0),
                modified: None,
                identity: None,
            }],
        )[0];
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
}

#[cfg(test)]
mod navigation_review_tests {
    use super::*;
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
