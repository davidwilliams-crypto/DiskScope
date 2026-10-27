//! Shared scanner, query and UI contracts. Paths remain OS strings until display.
use crate::diagnostics::{Counter, Metric, Span};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::PathBuf,
    time::Duration,
};

pub type NodeId = u32;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    pub volume: u64,
    pub id: [u8; 16],
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: OsString,
    pub is_dir: bool,
    pub reparse: bool,
    pub logical: u64,
    pub allocated: Option<u64>,
    pub modified: Option<u64>,
    pub identity: Option<FileIdentity>,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub name: OsString,
    pub is_dir: bool,
    pub reparse: bool,
    pub logical: u64,
    /// Per-entry allocation sum, NOT unique/reclaimable bytes.
    pub allocated: Option<u64>,
    pub files: u64,
    pub folders: u64,
    pub modified: Option<u64>,
    pub identity: Option<FileIdentity>,
    pub hard_link: bool,
    /// False while a directory is pending or after its enumeration failed.
    pub allocation_complete: bool,
    pub children: Vec<NodeId>,
}

#[derive(Clone, Debug)]
pub struct ScanError {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug)]
pub struct Model {
    pub root_path: PathBuf,
    pub nodes: Vec<Node>,
    pub revision: u64,
    pub skipped: u64,
    pub errors_total: u64,
    pub errors: Vec<ScanError>,
    pub overflowed: bool,
    pub unique_allocated: Option<u64>,
    pub hardlinks: u64,
    pub(crate) seen_files: HashMap<FileIdentity, NodeId>,
}

impl Model {
    pub fn new(root: PathBuf) -> Self {
        let name = root.file_name().unwrap_or(root.as_os_str()).to_os_string();
        Self {
            root_path: root,
            nodes: vec![Node {
                id: 0,
                parent: None,
                name,
                is_dir: true,
                reparse: false,
                logical: 0,
                allocated: Some(0),
                files: 0,
                folders: 0,
                modified: None,
                identity: None,
                hard_link: false,
                allocation_complete: true,
                children: Vec::new(),
            }],
            revision: 0,
            skipped: 0,
            errors_total: 0,
            errors: Vec::new(),
            overflowed: false,
            unique_allocated: Some(0),
            hardlinks: 0,
            seen_files: HashMap::new(),
        }
    }
    pub fn insert_batch(&mut self, parent: NodeId, entries: Vec<Entry>) -> Vec<NodeId> {
        if self.nodes.get(parent as usize).is_none_or(|n| !n.is_dir) {
            self.record_error(self.root_path.clone(), "Invalid scanner parent".into());
            return Vec::new();
        }
        let mut ids = Vec::with_capacity(entries.len());
        let (mut logical, mut allocated, mut files, mut folders) = (0u64, Some(0u64), 0u64, 0u64);
        for e in entries {
            let Ok(id) = NodeId::try_from(self.nodes.len()) else {
                self.overflowed = true;
                self.record_error(self.root_path.clone(), "Node index limit reached".into());
                break;
            };
            let bytes = if e.is_dir { 0 } else { e.logical };
            let allocation = if e.is_dir { Some(0) } else { e.allocated };
            let mut hard_link = false;
            if !e.is_dir {
                if let Some(identity) = e.identity.as_ref() {
                    if let Some(&first) = self.seen_files.get(identity) {
                        self.hardlinks = self.hardlinks.saturating_add(1);
                        hard_link = true;
                        let original = &mut self.nodes[first as usize];
                        original.hard_link = true;
                        if original.logical != bytes || original.allocated != allocation {
                            self.unique_allocated = None;
                        }
                    } else {
                        self.seen_files.insert(identity.clone(), id);
                        self.unique_allocated =
                            optional_add(self.unique_allocated, allocation, &mut self.overflowed);
                    }
                } else {
                    self.unique_allocated = None;
                }
            }
            logical = checked_sum(logical, bytes, &mut self.overflowed);
            allocated = optional_add(allocated, allocation, &mut self.overflowed);
            if e.is_dir {
                folders = folders.saturating_add(1)
            } else {
                files = files.saturating_add(1)
            }
            self.nodes.push(Node {
                id,
                parent: Some(parent),
                name: e.name,
                is_dir: e.is_dir,
                reparse: e.reparse,
                logical: bytes,
                allocated: allocation,
                files: 0,
                folders: 0,
                modified: e.modified,
                identity: e.identity,
                hard_link,
                allocation_complete: true,
                children: Vec::new(),
            });
            ids.push(id);
        }
        self.nodes[parent as usize].children.extend_from_slice(&ids);
        let mut ancestor = Some(parent);
        let mut ancestor_steps = Counter::new(Metric::AncestorSteps);
        while let Some(id) = ancestor {
            ancestor_steps.bump();
            let n = &mut self.nodes[id as usize];
            n.logical = checked_sum(n.logical, logical, &mut self.overflowed);
            n.allocated = optional_add(n.allocated, allocated, &mut self.overflowed);
            n.files = checked_sum(n.files, files, &mut self.overflowed);
            n.folders = checked_sum(n.folders, folders, &mut self.overflowed);
            ancestor = n.parent;
        }
        if !ids.is_empty() {
            self.revision = self.revision.saturating_add(1);
        }
        ids
    }
    pub fn path(&self, id: NodeId) -> PathBuf {
        let _path = Span::new(Metric::PathBuild);
        let mut names = Vec::new();
        let mut current = Some(id);
        let mut path_steps = Counter::new(Metric::PathSteps);
        while let Some(i) = current {
            path_steps.bump();
            let Some(n) = self.nodes.get(i as usize) else {
                return self.root_path.clone();
            };
            if n.parent.is_some() {
                names.push(n.name.as_os_str());
            }
            current = n.parent;
        }
        let mut path = self.root_path.clone();
        for name in names.into_iter().rev() {
            path.push(name);
        }
        path
    }
    pub fn record_error(&mut self, path: PathBuf, message: String) {
        self.errors_total = self.errors_total.saturating_add(1);
        if self.errors.len() < 1000 {
            self.errors.push(ScanError { path, message });
        }
        self.revision = self.revision.saturating_add(1);
    }
}

fn checked_sum(a: u64, b: u64, overflow: &mut bool) -> u64 {
    a.checked_add(b).unwrap_or_else(|| {
        *overflow = true;
        u64::MAX
    })
}
fn optional_add(a: Option<u64>, b: Option<u64>, overflow: &mut bool) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => match x.checked_add(y) {
            Some(n) => Some(n),
            None => {
                *overflow = true;
                None
            }
        },
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanState {
    #[default]
    Idle,
    Preparing,
    Scanning,
    Finalizing,
    Cancelling,
    Cancelled,
    Complete,
    Partial,
    Failed,
}
impl ScanState {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Scanning | Self::Finalizing | Self::Cancelling
        )
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Ready to explore",
            Self::Preparing => "Preparing scan",
            Self::Scanning => "Discovering files",
            Self::Finalizing => "Finishing scan",
            Self::Cancelling => "Cancelling",
            Self::Cancelled => "Scan cancelled",
            Self::Complete => "Scan complete",
            Self::Partial => "Scan finished with errors",
            Self::Failed => "Scan failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackendChoice {
    #[default]
    Normal,
    Ntfs,
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub generation: u64,
    pub state: ScanState,
    pub root: PathBuf,
    pub backend: String,
    pub files: u64,
    pub folders: u64,
    pub logical: u64,
    pub unique_allocated: Option<u64>,
    pub skipped: u64,
    pub errors_total: u64,
    pub errors: Vec<ScanError>,
    pub hardlinks: u64,
    pub elapsed: Duration,
    pub current_path: PathBuf,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    Explorer,
    Overview,
    Largest,
    Types,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MapLayout {
    #[default]
    Treemap,
    Icicle,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryFilter {
    /// Case-insensitive extensions including the leading dot; empty means all types.
    pub extensions: Vec<String>,
    pub min_bytes: Option<u64>,
    pub max_bytes: Option<u64>,
    /// Unix seconds, inclusive. A file with unknown modification time cannot match.
    pub modified_after: Option<u64>,
    pub modified_before: Option<u64>,
}
impl QueryFilter {
    pub fn is_active(&self) -> bool {
        !self.extensions.is_empty()
            || self.min_bytes.is_some()
            || self.max_bytes.is_some()
            || self.modified_after.is_some()
            || self.modified_before.is_some()
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    Name,
    #[default]
    Logical,
    Allocated,
    Files,
    Modified,
}

#[derive(Clone, Debug)]
pub struct ViewRequest {
    pub revision: u64,
    pub root: NodeId,
    pub selected: Option<NodeId>,
    pub expanded: HashSet<NodeId>,
    pub mode: ViewMode,
    pub search: String,
    pub search_all: bool,
    pub sort: SortKey,
    pub descending: bool,
    pub offset: usize,
    pub limit: usize,
    pub map_aspect: f32,
    pub filters: QueryFilter,
    pub map_layout: MapLayout,
    pub pinned: Vec<NodeId>,
    /// One-shot native path focus for a change belonging to the current scan.
    pub focus_path: Option<PathBuf>,
}
impl Default for ViewRequest {
    fn default() -> Self {
        Self {
            revision: 0,
            root: 0,
            selected: None,
            expanded: HashSet::new(),
            mode: ViewMode::Explorer,
            search: String::new(),
            search_all: false,
            sort: SortKey::Logical,
            descending: true,
            offset: 0,
            limit: 128,
            map_aspect: 2.5,
            filters: QueryFilter::default(),
            map_layout: MapLayout::Treemap,
            pinned: Vec::new(),
            focus_path: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub depth: u16,
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub reparse: bool,
    pub identity: Option<FileIdentity>,
    pub hard_link: bool,
    pub logical: u64,
    pub allocated: Option<u64>,
    pub files: u64,
    pub folders: u64,
    pub modified: Option<u64>,
    pub has_children: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TypeStat {
    pub extension: String,
    pub logical: u64,
    pub files: u64,
}
#[derive(Clone, Debug)]
pub struct MapItem {
    pub id: NodeId,
    pub name: String,
    pub logical: u64,
    pub rect: [f32; 4],
    pub is_dir: bool,
    pub others: bool,
    /// Matching descendant/file under the active lens; geometry remains unfiltered.
    pub matched: bool,
}
#[derive(Clone, Debug)]
pub struct BreakdownItem {
    pub id: Option<NodeId>,
    pub name: String,
    pub logical: u64,
    pub is_dir: bool,
}
#[derive(Clone, Debug, Default)]
pub struct ViewSnapshot {
    pub generation: u64,
    pub revision: u64,
    pub root: NodeId,
    pub root_path: PathBuf,
    pub breadcrumbs: Vec<(NodeId, String)>,
    pub rows: Vec<Row>,
    pub total_rows: usize,
    pub selected: Option<Row>,
    pub types: Vec<TypeStat>,
    pub map: Vec<MapItem>,
    pub total_logical: u64,
    /// Logical denominator for returned rows; all-scan search may differ from the map root.
    pub row_scope_logical: u64,
    pub row_scope_root: NodeId,
    pub unique_allocated: Option<u64>,
    pub matching_files: u64,
    pub matching_logical: u64,
    pub filters_active: bool,
    pub pinned_rows: Vec<Row>,
    pub pinned_overlap: bool,
    pub breakdown_root: Option<NodeId>,
    pub breakdown: Vec<BreakdownItem>,
    pub notice: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct UiSnapshot {
    pub progress: Progress,
    pub view: ViewSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(name: &str, logical: u64, allocated: Option<u64>, id: u8) -> Entry {
        Entry {
            name: name.into(),
            is_dir: false,
            reparse: false,
            logical,
            allocated,
            modified: None,
            identity: Some(FileIdentity {
                volume: 1,
                id: [id; 16],
            }),
        }
    }
    #[test]
    fn nested_logical_totals_are_additive() {
        let mut m = Model::new(PathBuf::from("C:\\fixture"));
        let folder = m.insert_batch(
            0,
            vec![Entry {
                name: "sub".into(),
                is_dir: true,
                reparse: false,
                logical: 0,
                allocated: Some(0),
                modified: None,
                identity: None,
            }],
        )[0];
        m.insert_batch(
            folder,
            vec![file("a", 17, Some(4096), 1), file("b", 29, Some(4096), 2)],
        );
        assert_eq!(
            (m.nodes[0].logical, m.nodes[0].files, m.nodes[0].folders),
            (46, 2, 1)
        );
        assert_eq!(
            m.path(2),
            PathBuf::from("C:\\fixture").join("sub").join("a")
        );
    }
    #[test]
    fn hardlinks_count_logically_but_allocate_once() {
        let mut m = Model::new("fixture".into());
        m.insert_batch(
            0,
            vec![file("a", 100, Some(4096), 1), file("b", 100, Some(4096), 1)],
        );
        assert_eq!(m.nodes[0].logical, 200);
        assert_eq!(m.unique_allocated, Some(4096));
        assert_eq!(m.hardlinks, 1);
        assert!(m.nodes[2].hard_link);
    }
    #[test]
    fn missing_metadata_does_not_invent_allocated_bytes() {
        let mut m = Model::new("fixture".into());
        m.insert_batch(0, vec![file("a", 100, None, 1)]);
        assert_eq!(m.unique_allocated, None);
        assert_eq!(m.nodes[0].allocated, None);
    }
    #[test]
    fn overflow_is_reported_instead_of_wrapping() {
        let mut m = Model::new("fixture".into());
        m.insert_batch(
            0,
            vec![file("a", u64::MAX, Some(0), 1), file("b", 1, Some(0), 2)],
        );
        assert_eq!(m.nodes[0].logical, u64::MAX);
        assert!(m.overflowed);
    }
}
