//! Pure background view queries. Call from a worker, never the UI callback.
use crate::diagnostics::{self, Counter, Metric, Span};
use crate::model::*;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    path::Path,
};

/// Snapshot output is bounded even if a caller requests an excessive page size.
pub const MAX_PAGE_ROWS: usize = 2048;

/// Exact append-only root aggregates owned by a private normal Scan, never by
/// public Model. Callers must append exactly the IDs returned by insert_batch
/// and discard the summary whenever its model is replaced. Public model fields
/// cannot provide a mutation stamp, so public build_view never uses this cache.
pub(crate) struct RootSummary {
    files: u64,
    logical: u64,
    types: HashMap<String, (u64, u64)>,
    // Worst-ranked retained file is the root. IDs keep the index independent
    // of arena reallocations and allow current hardlink/metadata flags in rows.
    largest: Vec<NodeId>,
}
impl RootSummary {
    pub(crate) fn new() -> Self {
        Self {
            files: 0,
            logical: 0,
            types: HashMap::new(),
            largest: Vec::with_capacity(MAX_PAGE_ROWS),
        }
    }
    pub(crate) fn append(&mut self, model: &Model, ids: &[NodeId]) {
        let _update = Span::new(Metric::SummaryUpdate);
        let mut files = Counter::new(Metric::SummaryFiles);
        for &id in ids {
            let node = &model.nodes[id as usize];
            if node.is_dir {
                continue;
            }
            files.bump();
            self.files = self.files.saturating_add(1);
            self.logical = self.logical.saturating_add(node.logical);
            let stat = self.types.entry(extension(node)).or_default();
            stat.0 = stat.0.saturating_add(node.logical);
            stat.1 = stat.1.saturating_add(1);
            self.insert_largest(model, id);
        }
    }
    fn insert_largest(&mut self, model: &Model, id: NodeId) {
        if self.largest.len() < MAX_PAGE_ROWS {
            self.largest.push(id);
            let mut index = self.largest.len() - 1;
            while index > 0 {
                let parent = (index - 1) / 2;
                if compare_order(
                    model,
                    SortKey::Logical,
                    true,
                    self.largest[index],
                    self.largest[parent],
                ) != Ordering::Greater
                {
                    break;
                }
                self.largest.swap(index, parent);
                index = parent;
            }
            return;
        }
        let worst = self.largest[0];
        // Strictly smaller files cannot displace the retained boundary. Equal
        // sizes, including zero, must still compare normalized/native names/IDs.
        if model.nodes[id as usize].logical < model.nodes[worst as usize].logical
            || compare_order(model, SortKey::Logical, true, id, worst) != Ordering::Less
        {
            return;
        }
        self.largest[0] = id;
        let mut index = 0;
        loop {
            let left = index * 2 + 1;
            if left >= self.largest.len() {
                break;
            }
            let right = left + 1;
            let worse = if right < self.largest.len()
                && compare_order(
                    model,
                    SortKey::Logical,
                    true,
                    self.largest[left],
                    self.largest[right],
                ) == Ordering::Less
            {
                right
            } else {
                left
            };
            if compare_order(
                model,
                SortKey::Logical,
                true,
                self.largest[index],
                self.largest[worse],
            ) != Ordering::Less
            {
                break;
            }
            self.largest.swap(index, worse);
            index = worse;
        }
    }
    fn covers_largest_page(&self, request: &ViewRequest, limit: usize) -> bool {
        request.mode == ViewMode::Largest
            && request.sort == SortKey::Logical
            && request.descending
            && (limit == 0
                || request.offset as u64 >= self.files
                || request
                    .offset
                    .checked_add(limit)
                    .is_some_and(|end| end <= MAX_PAGE_ROWS))
    }
}
pub(crate) fn build_view_with_summary(
    model: &Model,
    request: &ViewRequest,
    generation: u64,
    summary: &RootSummary,
) -> ViewSnapshot {
    build_view_impl(model, request, generation, Some(summary))
}

pub fn build_view(model: &Model, request: &ViewRequest, generation: u64) -> ViewSnapshot {
    build_view_impl(model, request, generation, None)
}

fn build_view_impl(
    model: &Model,
    request: &ViewRequest,
    generation: u64,
    summary: Option<&RootSummary>,
) -> ViewSnapshot {
    // Focus is a one-shot request against this exact model, resolved with native
    // components rather than display strings (which may contain replacements).
    let mut focused;
    let mut notice = None;
    let request = if let Some(path) = request.focus_path.as_ref() {
        if let Some(id) = resolve_path(model, path) {
            focused = request.clone();
            focused.root = model.nodes[id as usize].parent.unwrap_or(id);
            focused.selected = Some(id);
            focused.offset = 0;
            &focused
        } else {
            notice = Some("This path is no longer present in the current scan.".into());
            request
        }
    } else {
        request
    };
    let mut view = ViewSnapshot {
        generation,
        revision: request.revision,
        root: request.root,
        notice,
        ..Default::default()
    };
    let Some(root) = model.nodes.get(request.root as usize) else {
        return view;
    };
    let filter = FileFilter::new(&request.search, &request.filters);
    view.filters_active = filter.is_active();
    view.root_path = model.path(request.root);
    view.total_logical = root.logical;
    view.unique_allocated = {
        let _allocation = Span::new(Metric::QueryAllocation);
        if summary.is_some() && request.root == 0 {
            // A private Scan keeps its root incomplete until every directory is
            // finished, and clears it for partial/cancelled results. Its root
            // flag is therefore the aggregate completeness bit. Public models
            // can be mutated directly and still use the checked traversal below.
            #[cfg(test)]
            query_work_tests::record_allocation_check();
            root.allocation_complete
                .then_some(model.unique_allocated)
                .flatten()
        } else {
            unique_allocation(model, request.root)
        }
    };
    view.selected = request
        .selected
        .and_then(|id| model.nodes.get(id as usize).map(|_| row(model, id, 0)));
    let mut parent = Some(request.root);
    while let Some(id) = parent {
        let Some(node) = model.nodes.get(id as usize) else {
            break;
        };
        view.breadcrumbs
            .push((id, node.name.to_string_lossy().into_owned()));
        parent = node.parent;
    }
    view.breadcrumbs.reverse();
    let map_span = Span::new(Metric::QueryMap);
    view.map = match request.map_layout {
        MapLayout::Treemap => {
            crate::treemap::build_treemap(model, request.root, request.map_aspect)
        }
        MapLayout::Icicle => crate::icicle::build_icicle(model, request.root),
    };
    drop(map_span);
    {
        let _breakdown = Span::new(Metric::QueryBreakdown);
        populate_pins(model, request, &mut view);
        populate_breakdown(model, request, &mut view);
    }
    let scope = if request.search_all && view.filters_active {
        0
    } else {
        request.root
    };
    view.row_scope_root = scope;
    view.row_scope_logical = model.nodes[scope as usize].logical;
    let limit = request.limit.min(MAX_PAGE_ROWS);
    let mut candidates = Vec::new();
    let mut types: HashMap<String, (u64, u64)> = HashMap::new();
    // Local lenses retain only scoped matches/ancestors. A whole-scan lens uses
    // a compact bitmap because its necessary traversal may match every file.
    // Unfiltered queries allocate neither representation.
    let mut matched = MatchFlags::new(view.filters_active, scope, model.nodes.len());
    let summary = summary.filter(|summary| {
        request.root == 0
            && !view.filters_active
            && (matches!(
                request.mode,
                ViewMode::Overview | ViewMode::Types | ViewMode::Explorer
            ) || summary.covers_largest_page(request, limit))
    });
    let indexed_total_rows = summary
        .filter(|_| request.mode == ViewMode::Largest)
        .map(|summary| {
            usize::try_from(summary.files).expect("file count is bounded by the arena length")
        });
    if let Some(summary) = summary {
        // These assertions check the private Scan insertion invariant; they are
        // not a validity heuristic for callers that mutate public Model fields.
        debug_assert_eq!(summary.files, root.files);
        debug_assert_eq!(summary.logical, root.logical);
        diagnostics::add(Metric::SummaryHits, 1);
        view.matching_files = summary.files;
        view.matching_logical = summary.logical;
        if matches!(request.mode, ViewMode::Overview | ViewMode::Types) {
            types = summary.types.clone();
        } else if request.mode == ViewMode::Largest
            && limit != 0
            && (request.offset as u64) < summary.files
        {
            candidates.clone_from(&summary.largest);
        }
    } else {
        let traversal = Span::new(Metric::QueryTraversal);
        let _matches = Span::new(Metric::QueryMatches);
        let mut visited = Counter::new(Metric::QueryVisited);
        visit_subtree(model, scope, |id| {
            visited.bump();
            let node = &model.nodes[id as usize];
            let mut extension_key = None;
            if !filter.matches(node, &mut extension_key) {
                return;
            }
            if view.filters_active {
                let mut ancestor = Some(id);
                while let Some(id) = ancestor {
                    // Stop at an already marked ancestor. Each scoped node is
                    // inserted at most once even in very deep matching trees.
                    if !matched.insert(id) {
                        break;
                    }
                    if id == scope {
                        break;
                    }
                    ancestor = model.nodes[id as usize].parent;
                }
            }
            view.matching_files = view.matching_files.saturating_add(1);
            view.matching_logical = view.matching_logical.saturating_add(node.logical);
            if matches!(request.mode, ViewMode::Types | ViewMode::Overview) {
                let count = types
                    .entry(extension_key.unwrap_or_else(|| extension(node)))
                    .or_default();
                count.0 = count.0.saturating_add(node.logical);
                count.1 = count.1.saturating_add(1);
            }
            if request.mode == ViewMode::Largest
                || (request.mode == ViewMode::Explorer && view.filters_active)
            {
                candidates.push(id);
            }
        });
        drop(visited);
        drop(traversal);
    }
    if view.filters_active {
        let _matches = Span::new(Metric::QueryMatches);
        for tile in &mut view.map {
            tile.matched = matched.contains(&tile.id);
        }
        // Other contains only omitted children: a visible match alone must not
        // highlight an unrelated aggregate. Icicle Other uses its actual parent.
        for index in 0..view.map.len() {
            if !view.map[index].others {
                continue;
            }
            let parent = view.map[index].id;
            let visible: HashSet<_> = view
                .map
                .iter()
                .filter(|t| !t.others && model.nodes[t.id as usize].parent == Some(parent))
                .map(|t| t.id)
                .collect();
            view.map[index].matched = model.nodes[parent as usize]
                .children
                .iter()
                .any(|id| !visible.contains(id) && matched.contains(id));
        }
    }
    #[cfg(test)]
    query_work_tests::record_match_bytes(matched.allocated_bytes());
    if request.mode == ViewMode::Explorer && !view.filters_active {
        // Collapsed subtrees are never sorted; off-page paths are not allocated.
        let mut stack = Vec::new();
        push_children(model, request, request.root, 0, &mut stack);
        while let Some((id, depth)) = stack.pop() {
            let index = view.total_rows;
            view.total_rows += 1;
            if index >= request.offset && view.rows.len() < limit {
                view.rows.push(row(model, id, depth));
            }
            if request.expanded.contains(&id) {
                push_children(model, request, id, depth.saturating_add(1), &mut stack);
            }
        }
        return view;
    }
    if matches!(request.mode, ViewMode::Types | ViewMode::Overview) {
        let mut stats: Vec<_> = types
            .into_iter()
            .map(|(extension, (logical, files))| TypeStat {
                extension,
                logical,
                files,
            })
            .collect();
        diagnostics::add(Metric::QueryCandidates, stats.len() as u64);
        let sort_span = Span::new(Metric::QuerySort);
        stats.sort_unstable_by(|a, b| {
            if request.mode == ViewMode::Overview {
                return b
                    .logical
                    .cmp(&a.logical)
                    .then_with(|| a.extension.cmp(&b.extension));
            }
            let order = match request.sort {
                SortKey::Name => a.extension.cmp(&b.extension),
                SortKey::Files => a.files.cmp(&b.files),
                _ => a.logical.cmp(&b.logical),
            };
            directed(order, request.descending).then_with(|| a.extension.cmp(&b.extension))
        });
        drop(sort_span);
        if request.mode == ViewMode::Types {
            view.total_rows = stats.len();
            view.types = stats.into_iter().skip(request.offset).take(limit).collect();
            return view;
        }
        view.types = stats.into_iter().take(5).collect();
        candidates = root
            .children
            .iter()
            .copied()
            .filter(|id| !view.filters_active || matched.contains(id))
            .collect();
        diagnostics::add(Metric::QueryCandidates, candidates.len() as u64);
        let sort_span = Span::new(Metric::QuerySort);
        candidates.sort_unstable_by(|a, b| {
            model.nodes[*b as usize]
                .logical
                .cmp(&model.nodes[*a as usize].logical)
                .then_with(|| {
                    normalized_name_order(&model.nodes[*a as usize], &model.nodes[*b as usize])
                })
                .then_with(|| a.cmp(b))
        });
        drop(sort_span);
        view.total_rows = candidates.len();
        view.rows = candidates
            .into_iter()
            .take(6)
            .map(|id| row(model, id, 0))
            .collect();
        return view;
    }
    diagnostics::add(Metric::QueryCandidates, candidates.len() as u64);
    view.total_rows = indexed_total_rows.unwrap_or(candidates.len());
    let sort_span = Span::new(Metric::QuerySort);
    let page = rank_page_by(&mut candidates, request.offset, limit, |a, b| {
        compare(model, request, *a, *b)
    });
    drop(sort_span);
    view.rows = page.iter().map(|&id| row(model, id, 0)).collect();
    view
}

/// Rank only the requested window. The caller's total comparator preserves the
/// same page membership and order as a full sort, including native-name/ID ties.
fn rank_page_by<T>(
    values: &mut [T],
    offset: usize,
    limit: usize,
    mut compare: impl FnMut(&T, &T) -> Ordering,
) -> &[T] {
    if limit == 0 || offset >= values.len() {
        return &values[..0];
    }
    if offset > 0 {
        let _ = values.select_nth_unstable_by(offset, &mut compare);
    }
    let suffix = &mut values[offset..];
    let count = limit.min(suffix.len());
    if count < suffix.len() {
        let _ = suffix.select_nth_unstable_by(count, &mut compare);
    }
    let page = &mut suffix[..count];
    page.sort_unstable_by(compare);
    page
}

fn extension(node: &Node) -> String {
    Path::new(&node.name)
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy().to_lowercase()))
        .unwrap_or_else(|| "(no extension)".into())
}

enum MatchFlags {
    None,
    WholeScan(Vec<u64>),
    Scoped(HashSet<NodeId>),
}
impl MatchFlags {
    fn new(active: bool, scope: NodeId, nodes: usize) -> Self {
        if !active {
            Self::None
        } else if scope == 0 {
            #[cfg(test)]
            query_work_tests::record_match_storage(nodes);
            Self::WholeScan(vec![0; nodes.div_ceil(u64::BITS as usize)])
        } else {
            Self::Scoped(HashSet::new())
        }
    }
    fn insert(&mut self, id: NodeId) -> bool {
        match self {
            Self::None => false,
            Self::WholeScan(words) => {
                let bit = 1u64 << (id % u64::BITS);
                let word = &mut words[id as usize / u64::BITS as usize];
                let inserted = *word & bit == 0;
                *word |= bit;
                inserted
            }
            Self::Scoped(ids) => {
                let inserted = ids.insert(id);
                #[cfg(test)]
                if inserted {
                    query_work_tests::record_match_storage(1);
                }
                inserted
            }
        }
    }
    fn contains(&self, id: &NodeId) -> bool {
        match self {
            Self::None => false,
            Self::WholeScan(words) => {
                words[*id as usize / u64::BITS as usize] & (1u64 << (*id % u64::BITS)) != 0
            }
            Self::Scoped(ids) => ids.contains(id),
        }
    }
    #[cfg(test)]
    fn allocated_bytes(&self) -> usize {
        match self {
            Self::None => 0,
            Self::WholeScan(words) => words.capacity() * std::mem::size_of::<u64>(),
            Self::Scoped(ids) => ids.capacity() * std::mem::size_of::<NodeId>(),
        }
    }
}

struct FileFilter<'a> {
    query: String,
    extensions: HashSet<String>,
    ranges: &'a QueryFilter,
}
impl<'a> FileFilter<'a> {
    fn new(query: &str, filters: &'a QueryFilter) -> Self {
        Self {
            query: query.trim().to_lowercase(),
            extensions: filters
                .extensions
                .iter()
                .map(|extension| {
                    #[cfg(test)]
                    query_work_tests::record_filter_normalization();
                    extension.to_lowercase()
                })
                .collect(),
            ranges: filters,
        }
    }
    fn is_active(&self) -> bool {
        !self.query.is_empty() || self.ranges.is_active()
    }
    fn matches(&self, node: &Node, extension_key: &mut Option<String>) -> bool {
        #[cfg(test)]
        query_work_tests::record_match_evaluation();
        let filters = self.ranges;
        !node.is_dir
            && filters.min_bytes.is_none_or(|min| node.logical >= min)
            && filters.max_bytes.is_none_or(|max| node.logical <= max)
            && filters
                .modified_after
                .is_none_or(|after| node.modified.is_some_and(|m| m >= after))
            && filters
                .modified_before
                .is_none_or(|before| node.modified.is_some_and(|m| m <= before))
            && (self.query.is_empty()
                || node
                    .name
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&self.query))
            && (self.extensions.is_empty()
                || self
                    .extensions
                    .contains(extension_key.get_or_insert_with(|| extension(node))))
    }
}

fn resolve_path(model: &Model, path: &Path) -> Option<NodeId> {
    let relative = path.strip_prefix(&model.root_path).ok()?;
    let mut id = 0;
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return None;
        };
        id = *model
            .nodes
            .get(id as usize)?
            .children
            .iter()
            .find(|child| model.nodes[**child as usize].name == name)?;
    }
    Some(id)
}

fn populate_pins(model: &Model, request: &ViewRequest, view: &mut ViewSnapshot) {
    for &id in &request.pinned {
        if view.pinned_rows.len() == 4 {
            break;
        }
        if model.nodes.get(id as usize).is_some() && !view.pinned_rows.iter().any(|r| r.id == id) {
            view.pinned_rows.push(row(model, id, 0));
        }
    }
    for pin in &view.pinned_rows {
        let mut parent = pin.parent;
        while let Some(id) = parent {
            if view.pinned_rows.iter().any(|r| r.id == id) {
                view.pinned_overlap = true;
                break;
            }
            parent = model.nodes[id as usize].parent;
        }
    }
}

fn populate_breakdown(model: &Model, request: &ViewRequest, view: &mut ViewSnapshot) {
    let root = request
        .selected
        .filter(|id| model.nodes.get(*id as usize).is_some_and(|n| n.is_dir))
        .unwrap_or(request.root);
    if !model.nodes[root as usize].is_dir {
        return;
    }
    view.breakdown_root = Some(root);
    let mut direct = 0u64;
    let mut has_direct = false;
    let mut dirs = Vec::new();
    for &id in &model.nodes[root as usize].children {
        let node = &model.nodes[id as usize];
        if node.is_dir {
            dirs.push(id);
        } else {
            has_direct = true;
            direct = direct.saturating_add(node.logical);
        }
    }
    dirs.sort_unstable_by(|a, b| {
        model.nodes[*b as usize]
            .logical
            .cmp(&model.nodes[*a as usize].logical)
            .then_with(|| a.cmp(b))
    });
    view.breakdown = dirs
        .into_iter()
        .take(if has_direct { 5 } else { 6 })
        .map(|id| BreakdownItem {
            id: Some(id),
            name: model.nodes[id as usize].name.to_string_lossy().into_owned(),
            logical: model.nodes[id as usize].logical,
            is_dir: true,
        })
        .collect();
    if has_direct {
        view.breakdown.push(BreakdownItem {
            id: None,
            name: "Files directly in this folder".into(),
            logical: direct,
            is_dir: false,
        });
    }
    view.breakdown
        .sort_by(|a, b| b.logical.cmp(&a.logical).then_with(|| a.id.cmp(&b.id)));
}

fn directed(order: Ordering, descending: bool) -> Ordering {
    if descending { order.reverse() } else { order }
}

fn compare(model: &Model, request: &ViewRequest, a: NodeId, b: NodeId) -> Ordering {
    compare_order(model, request.sort, request.descending, a, b)
}

fn compare_order(model: &Model, sort: SortKey, descending: bool, a: NodeId, b: NodeId) -> Ordering {
    #[cfg(test)]
    ranking_tests::record_comparison();
    let lhs = &model.nodes[a as usize];
    let rhs = &model.nodes[b as usize];
    let order = match sort {
        SortKey::Name => normalized_name_order(lhs, rhs),
        SortKey::Logical => lhs.logical.cmp(&rhs.logical),
        SortKey::Allocated => known_allocation(lhs).cmp(&known_allocation(rhs)),
        SortKey::Files => lhs.files.cmp(&rhs.files),
        SortKey::Modified => lhs.modified.cmp(&rhs.modified),
    };
    // Secondary name direction stays ascending when the primary direction changes.
    directed(order, descending)
        .then_with(|| normalized_name_order(lhs, rhs))
        .then_with(|| lhs.name.cmp(&rhs.name))
        .then_with(|| a.cmp(&b))
}

fn normalized_name_order(lhs: &Node, rhs: &Node) -> Ordering {
    let lhs = lhs.name.to_string_lossy();
    let rhs = rhs.name.to_string_lossy();
    lhs.chars()
        .flat_map(char::to_lowercase)
        .cmp(rhs.chars().flat_map(char::to_lowercase))
}

fn known_allocation(node: &Node) -> Option<u64> {
    if node.is_dir && !node.allocation_complete {
        None
    } else {
        node.allocated
    }
}

fn push_children(
    model: &Model,
    request: &ViewRequest,
    parent: NodeId,
    depth: u16,
    stack: &mut Vec<(NodeId, u16)>,
) {
    let Some(node) = model.nodes.get(parent as usize) else {
        return;
    };
    let mut children: Vec<_> = node
        .children
        .iter()
        .copied()
        .filter(|id| model.nodes.get(*id as usize).is_some())
        .collect();
    diagnostics::add(Metric::QueryCandidates, children.len() as u64);
    let sort_span = Span::new(Metric::QuerySort);
    children.sort_unstable_by(|a, b| compare(model, request, *a, *b));
    drop(sort_span);
    stack.extend(children.into_iter().rev().map(|id| (id, depth)));
}

fn visit_subtree(model: &Model, root: NodeId, mut visitor: impl FnMut(NodeId)) {
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if let Some(node) = model.nodes.get(id as usize) {
            #[cfg(test)]
            summary_tests::record_visit();
            visitor(id);
            pending.extend(node.children.iter().rev().copied());
        }
    }
}

fn unique_allocation(model: &Model, root: NodeId) -> Option<u64> {
    if root == 0 {
        return if model.nodes.iter().any(|node| {
            #[cfg(test)]
            query_work_tests::record_allocation_check();
            node.is_dir && !node.allocation_complete
        }) {
            None
        } else {
            model.unique_allocated
        };
    }
    let mut seen: HashMap<&FileIdentity, (u64, u64)> = HashMap::new();
    let mut result = Some(0u64);
    visit_subtree(model, root, |id| {
        #[cfg(test)]
        query_work_tests::record_allocation_check();
        let node = &model.nodes[id as usize];
        if result.is_none() {
            return;
        }
        if node.is_dir {
            if !node.allocation_complete {
                result = None;
            }
            return;
        }
        let (Some(identity), Some(allocated)) = (node.identity.as_ref(), node.allocated) else {
            result = None;
            return;
        };
        if let Some(previous) = seen.get(identity) {
            if *previous != (node.logical, allocated) {
                result = None;
            }
        } else {
            seen.insert(identity, (node.logical, allocated));
            result = result.and_then(|sum| sum.checked_add(allocated));
        }
    });
    result
}

fn row(model: &Model, id: NodeId, depth: u16) -> Row {
    let node = &model.nodes[id as usize];
    Row {
        id,
        parent: node.parent,
        depth,
        name: node.name.to_string_lossy().into_owned(),
        path: model.path(id),
        is_dir: node.is_dir,
        reparse: node.reparse,
        identity: node.identity.clone(),
        hard_link: node.hard_link,
        logical: node.logical,
        allocated: known_allocation(node),
        files: node.files,
        folders: node.folders,
        modified: node.modified,
        has_children: !node.children.is_empty(),
    }
}

#[cfg(test)]
#[path = "view/ranking_tests.rs"]
mod ranking_tests;

#[cfg(test)]
#[path = "view/query_work_tests.rs"]
mod query_work_tests;
#[cfg(test)]
#[path = "view/summary_tests.rs"]
mod summary_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, time::Instant};

    #[test]
    fn selected_row_carries_the_native_identity_for_verified_file_actions() {
        let model = fixture();
        let view = build_view(
            &model,
            &ViewRequest {
                selected: Some(4),
                ..Default::default()
            },
            7,
        );
        assert_eq!(
            view.selected.unwrap().identity,
            Some(FileIdentity {
                volume: 1,
                id: [3; 16]
            })
        );
    }

    fn entry(name: &str, dir: bool, size: u64, identity: u8) -> Entry {
        Entry {
            name: name.into(),
            is_dir: dir,
            reparse: false,
            logical: size,
            allocated: Some(if dir { 0 } else { 4096 }),
            modified: Some(size),
            identity: (!dir).then_some(FileIdentity {
                volume: 1,
                id: [identity; 16],
            }),
        }
    }
    fn fixture() -> Model {
        let mut m = Model::new(PathBuf::from("C:\\fixture"));
        m.insert_batch(
            0,
            vec![
                entry("sub", true, 0, 0),
                entry("Z.TXT", false, 10, 1),
                entry("a.txt", false, 10, 2),
            ],
        );
        m.insert_batch(
            1,
            vec![
                entry("Needle.bin", false, 90, 3),
                entry("copy.bin", false, 90, 3),
            ],
        );
        m
    }
    #[test]
    fn combined_filters_use_file_only_totals_all_scan_scope_and_fixed_map_geometry() {
        let m = fixture();
        let base = ViewRequest {
            root: 1,
            search_all: true,
            ..Default::default()
        };
        let before = build_view(&m, &base, 1);
        let filtered = build_view(
            &m,
            &ViewRequest {
                filters: QueryFilter {
                    extensions: vec![".TxT".into()],
                    min_bytes: Some(10),
                    max_bytes: Some(10),
                    modified_after: Some(10),
                    modified_before: Some(10),
                },
                ..base
            },
            1,
        );
        assert!(filtered.filters_active);
        assert_eq!(
            (filtered.row_scope_root, filtered.row_scope_logical),
            (0, 200)
        );
        assert_eq!(
            (filtered.matching_files, filtered.matching_logical),
            (2, 20)
        );
        assert_eq!(
            filtered.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![3, 2]
        );
        assert!(filtered.map.iter().all(|t| !t.matched));
        assert_eq!(
            before.map.iter().map(|t| t.rect).collect::<Vec<_>>(),
            filtered.map.iter().map(|t| t.rect).collect::<Vec<_>>()
        );
        let nested = build_view(
            &m,
            &ViewRequest {
                search: "needle".into(),
                ..Default::default()
            },
            1,
        );
        assert_eq!((nested.matching_files, nested.matching_logical), (1, 90));
        assert!(nested.map.iter().find(|t| t.id == 1).unwrap().matched);
    }
    #[test]
    fn overview_breakdown_and_pins_have_bounded_factual_scope() {
        let m = fixture();
        let v = build_view(
            &m,
            &ViewRequest {
                mode: ViewMode::Overview,
                selected: Some(1),
                pinned: vec![0, 1, 4, 4, 3, 2],
                ..Default::default()
            },
            1,
        );
        assert_eq!(
            v.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![1, 3, 2]
        );
        assert_eq!(v.types.len(), 2);
        assert_eq!(
            v.pinned_rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![0, 1, 4, 3]
        );
        assert!(v.pinned_overlap);
        assert_eq!(v.breakdown_root, Some(1));
        assert_eq!(v.breakdown.len(), 1);
        assert_eq!(v.breakdown[0].id, None);
        assert_eq!(v.breakdown[0].logical, 180);
        let root = build_view(&m, &ViewRequest::default(), 1);
        assert_eq!(root.breakdown.len(), 2);
        assert_eq!(root.breakdown.iter().map(|b| b.logical).sum::<u64>(), 200);
    }
    #[test]
    fn focus_resolves_native_hierarchy_and_missing_focus_keeps_context() {
        let m = fixture();
        let v = build_view(
            &m,
            &ViewRequest {
                focus_path: Some(m.path(4)),
                ..Default::default()
            },
            1,
        );
        assert_eq!(v.root, 1);
        assert_eq!(v.selected.as_ref().map(|r| r.id), Some(4));
        assert_eq!(v.root_path, m.path(1));
        let missing = build_view(
            &m,
            &ViewRequest {
                root: 1,
                focus_path: Some(m.root_path.join("gone")),
                ..Default::default()
            },
            1,
        );
        assert_eq!(missing.root, 1);
        assert!(missing.notice.is_some());
    }
    #[test]
    fn unknown_times_and_conflicting_ranges_never_invent_matches() {
        let mut m = fixture();
        m.nodes[4].modified = None;
        for filters in [
            QueryFilter {
                modified_after: Some(90),
                modified_before: Some(90),
                ..Default::default()
            },
            QueryFilter {
                min_bytes: Some(100),
                max_bytes: Some(10),
                ..Default::default()
            },
            QueryFilter {
                modified_after: Some(100),
                modified_before: Some(10),
                ..Default::default()
            },
        ] {
            let v = build_view(
                &m,
                &ViewRequest {
                    filters: filters.clone(),
                    ..Default::default()
                },
                1,
            );
            assert_eq!(
                v.matching_files,
                if filters.modified_after == Some(90) {
                    1
                } else {
                    0
                }
            );
        }
        let v = build_view(
            &m,
            &ViewRequest {
                search: "sub".into(),
                ..Default::default()
            },
            1,
        );
        assert_eq!(
            (v.matching_files, v.matching_logical, v.total_rows),
            (0, 0, 0)
        );
    }
    #[test]
    fn overview_caps_children_types_and_breakdown_without_losing_direct_file_fact() {
        let mut m = Model::new("fixture".into());
        m.insert_batch(
            0,
            (0..10)
                .map(|i| entry(&format!("folder{i}"), true, 0, 0))
                .collect(),
        );
        for i in 1..=10 {
            m.insert_batch(
                i,
                vec![entry(&format!("file.ext{i}"), false, u64::from(i), i as u8)],
            );
        }
        m.insert_batch(0, vec![entry("tiny.dat", false, 1, 50)]);
        let v = build_view(
            &m,
            &ViewRequest {
                mode: ViewMode::Overview,
                ..Default::default()
            },
            1,
        );
        assert_eq!(v.rows.len(), 6);
        assert!(v.rows.iter().all(|r| r.parent == Some(0)));
        assert_eq!(v.types.len(), 5);
        assert_eq!(v.breakdown.len(), 6);
        assert_eq!(
            v.breakdown.iter().find(|b| b.id.is_none()).unwrap().logical,
            1
        );
        assert_eq!((v.matching_files, v.matching_logical), (11, 56));
    }
    #[test]
    fn other_highlighting_only_counts_omitted_matching_children() {
        let mut m = Model::new("fixture".into());
        m.insert_batch(
            0,
            vec![
                entry("visible.txt", false, 10_000_000, 1),
                entry("tiny.bin", false, 1, 2),
            ],
        );
        for layout in [MapLayout::Treemap, MapLayout::Icicle] {
            let visible = build_view(
                &m,
                &ViewRequest {
                    search: "visible".into(),
                    map_layout: layout,
                    ..Default::default()
                },
                1,
            );
            assert!(
                visible
                    .map
                    .iter()
                    .find(|t| !t.others && t.id == 1)
                    .unwrap()
                    .matched
            );
            assert!(visible.map.iter().filter(|t| t.others).all(|t| !t.matched));
            let tiny = build_view(
                &m,
                &ViewRequest {
                    search: "tiny".into(),
                    map_layout: layout,
                    ..Default::default()
                },
                1,
            );
            assert!(
                tiny.map
                    .iter()
                    .any(|t| t.matched && (t.others || t.id == 2))
            );
        }
    }
    #[cfg(windows)]
    #[test]
    fn focus_distinguishes_native_names_with_identical_lossy_display() {
        use std::os::windows::ffi::OsStringExt;
        let mut m = Model::new("fixture".into());
        let mut first = entry("unused", false, 1, 1);
        let mut second = entry("unused", false, 1, 2);
        first.name = std::ffi::OsString::from_wide(&[0xd800]);
        second.name = std::ffi::OsString::from_wide(&[0xd801]);
        m.insert_batch(0, vec![first, second]);
        assert_eq!(
            m.nodes[1].name.to_string_lossy(),
            m.nodes[2].name.to_string_lossy()
        );
        let v = build_view(
            &m,
            &ViewRequest {
                focus_path: Some(m.path(2)),
                ..Default::default()
            },
            1,
        );
        assert_eq!(v.selected.unwrap().id, 2);
    }
    #[test]
    fn all_scan_search_keeps_browse_map_totals_separate_from_row_scope() {
        let mut m = Model::new("fixture".into());
        m.insert_batch(
            0,
            vec![entry("small", true, 0, 0), entry("outside", true, 0, 0)],
        );
        m.insert_batch(1, vec![entry("small.dat", false, 100, 1)]);
        m.insert_batch(
            2,
            vec![
                entry("needle-large.dat", false, 1000, 2),
                entry("filler.dat", false, 8900, 3),
            ],
        );
        let q = ViewRequest {
            root: 1,
            search: "needle".into(),
            search_all: true,
            selected: Some(4),
            ..Default::default()
        };
        let view = build_view(&m, &q, 8);
        assert_eq!(view.total_logical, 100, "map stays in browsed folder");
        assert_eq!(view.map.iter().map(|tile| tile.logical).sum::<u64>(), 100);
        assert_eq!(
            view.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![4]
        );
        assert_eq!(
            view.row_scope_logical, 10_000,
            "rows use full scan denominator"
        );
        assert_eq!(view.row_scope_root, 0);
        let local = build_view(
            &m,
            &ViewRequest {
                search_all: false,
                ..q
            },
            8,
        );
        assert_eq!((local.row_scope_root, local.row_scope_logical), (1, 100));
        assert!(local.rows.is_empty());
    }
    #[test]
    fn explorer_pages_only_visible_expansion_in_stable_sibling_order() {
        let m = fixture();
        let mut q = ViewRequest {
            sort: SortKey::Name,
            descending: false,
            offset: 1,
            limit: 2,
            ..Default::default()
        };
        let v = build_view(&m, &q, 7);
        assert_eq!(v.total_rows, 3);
        assert_eq!(v.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(v.generation, 7);
        q.expanded.insert(1);
        let v = build_view(&m, &q, 8);
        assert_eq!(v.total_rows, 5);
        assert_eq!(
            v.rows.iter().map(|r| (r.id, r.depth)).collect::<Vec<_>>(),
            vec![(1, 0), (5, 1)]
        );
        assert_eq!(v.rows[1].path, m.path(5));
    }
    #[test]
    fn search_is_case_insensitive_and_respects_subtree_or_all_scan() {
        let m = fixture();
        let mut q = ViewRequest {
            root: 1,
            search: "nEeDlE".into(),
            ..Default::default()
        };
        assert_eq!(
            build_view(&m, &q, 1)
                .rows
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![4]
        );
        q.search = "TXT".into();
        assert_eq!(build_view(&m, &q, 1).total_rows, 0);
        q.search_all = true;
        assert_eq!(build_view(&m, &q, 1).total_rows, 2);
    }
    #[test]
    fn largest_and_types_keep_logical_links_but_unique_allocation_deduplicates() {
        let m = fixture();
        let q = ViewRequest {
            root: 1,
            mode: ViewMode::Largest,
            limit: 1,
            ..Default::default()
        };
        let v = build_view(&m, &q, 1);
        assert_eq!(v.total_rows, 2);
        assert_eq!(v.rows[0].id, 5);
        assert_eq!(v.total_logical, 180);
        assert_eq!(v.unique_allocated, Some(4096));
        let q = ViewRequest {
            mode: ViewMode::Types,
            ..Default::default()
        };
        let v = build_view(&m, &q, 1);
        assert_eq!(
            v.types
                .iter()
                .map(|t| (t.extension.as_str(), t.logical, t.files))
                .collect::<Vec<_>>(),
            vec![(".bin", 180, 2), (".txt", 20, 2)]
        );
    }
    #[test]
    fn missing_identity_or_allocation_leaves_subtree_unique_total_unknown() {
        let mut m = fixture();
        let q = ViewRequest {
            root: 1,
            ..Default::default()
        };
        assert_eq!(build_view(&m, &q, 1).unique_allocated, Some(4096));
        m.nodes[4].identity = None;
        assert_eq!(build_view(&m, &q, 1).unique_allocated, None);
    }
    #[test]
    fn each_sort_key_and_direction_controls_order_with_deterministic_ties() {
        let m = fixture();
        for (key, expected) in [
            (SortKey::Logical, vec![3, 2, 1]),
            (SortKey::Files, vec![3, 2, 1]),
            (SortKey::Modified, vec![1, 3, 2]),
            (SortKey::Allocated, vec![3, 2, 1]),
        ] {
            let q = ViewRequest {
                sort: key,
                descending: false,
                ..Default::default()
            };
            assert_eq!(
                build_view(&m, &q, 1)
                    .rows
                    .iter()
                    .map(|r| r.id)
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
    #[test]
    fn scalar_sort_ties_use_name_ascending_in_both_directions_and_native_duplicate_order() {
        let mut m = Model::new("fixture".into());
        m.insert_batch(
            0,
            vec![
                entry("zeta", false, 10, 1),
                entry("alpha", false, 10, 2),
                entry("Alpha", false, 10, 3),
                entry("alpha", false, 10, 4),
                entry("beta", false, 10, 5),
            ],
        );
        for key in [
            SortKey::Logical,
            SortKey::Allocated,
            SortKey::Files,
            SortKey::Modified,
        ] {
            for descending in [false, true] {
                let q = ViewRequest {
                    sort: key,
                    descending,
                    ..Default::default()
                };
                let v = build_view(&m, &q, 1);
                assert_eq!(
                    v.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
                    vec![3, 2, 4, 5, 1]
                );
                let page = build_view(
                    &m,
                    &ViewRequest {
                        offset: 1,
                        limit: 2,
                        ..q
                    },
                    1,
                );
                assert_eq!(
                    page.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
                    vec![2, 4]
                );
            }
        }
        let q = ViewRequest {
            sort: SortKey::Name,
            descending: true,
            ..Default::default()
        };
        assert_eq!(
            build_view(&m, &q, 1)
                .rows
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![1, 5, 3, 2, 4]
        );
    }
    #[test]
    fn failed_partial_and_cancelled_pending_directories_have_unknown_allocation() {
        let mut m = Model::new("fixture".into());
        m.insert_batch(
            0,
            vec![
                entry("failed", true, 0, 0),
                entry("partial", true, 0, 0),
                entry("cancelled-pending", true, 0, 0),
                entry("complete-empty", true, 0, 0),
            ],
        );
        m.insert_batch(2, vec![entry("retained.dat", false, 17, 1)]);
        for id in [1usize, 2, 3] {
            m.nodes[id].allocation_complete = false;
        }
        for id in [1u32, 2, 3] {
            let v = build_view(
                &m,
                &ViewRequest {
                    root: id,
                    selected: Some(id),
                    ..Default::default()
                },
                1,
            );
            assert_eq!(v.unique_allocated, None, "incomplete directory {id}");
            assert_eq!(v.selected.unwrap().allocated, None);
        }
        let root = build_view(&m, &ViewRequest::default(), 1);
        assert_eq!(
            root.unique_allocated, None,
            "incomplete descendants invalidate root accounting"
        );
        for row in &root.rows {
            assert_eq!(row.allocated, if row.id == 4 { Some(0) } else { None });
        }
        let retained = build_view(
            &m,
            &ViewRequest {
                root: 5,
                selected: Some(5),
                ..Default::default()
            },
            1,
        );
        assert_eq!(retained.unique_allocated, Some(4096));
        assert_eq!(retained.selected.unwrap().allocated, Some(4096));
    }
    #[test]
    fn incomplete_root_and_nested_pending_directory_invalidate_subtree_totals() {
        let mut m = fixture();
        m.nodes[0].allocation_complete = false;
        assert_eq!(
            build_view(&m, &ViewRequest::default(), 1).unique_allocated,
            None
        );
        m.nodes[0].allocation_complete = true;
        let pending = m.insert_batch(1, vec![entry("pending", true, 0, 0)])[0];
        m.nodes[pending as usize].allocation_complete = false;
        assert_eq!(
            build_view(
                &m,
                &ViewRequest {
                    root: 1,
                    ..Default::default()
                },
                1
            )
            .unique_allocated,
            None
        );
    }
    #[test]
    fn invalid_root_and_out_of_range_pages_are_empty() {
        let m = fixture();
        let q = ViewRequest {
            root: u32::MAX,
            ..Default::default()
        };
        assert!(build_view(&m, &q, 1).rows.is_empty());
        let q = ViewRequest {
            offset: usize::MAX,
            limit: usize::MAX,
            ..Default::default()
        };
        let v = build_view(&m, &q, 1);
        assert_eq!(v.total_rows, 3);
        assert!(v.rows.is_empty());
    }
    // Build directly to keep stress setup O(n), independent of aggregation insertion costs.
    fn synthetic(count: usize, deep: bool) -> Model {
        let mut m = Model::new("synthetic".into());
        m.nodes.reserve(count);
        for i in 1..=count {
            m.nodes.push(Node {
                id: i as u32,
                parent: Some(if deep { (i - 1) as u32 } else { 0 }),
                name: format!("{i}.dat").into(),
                is_dir: deep && i < count,
                reparse: false,
                logical: 1,
                allocated: Some(0),
                files: 1,
                folders: 0,
                modified: None,
                identity: None,
                hard_link: false,
                allocation_complete: true,
                children: if deep && i < count {
                    vec![(i + 1) as u32]
                } else {
                    vec![]
                },
            });
        }
        m.nodes[0].children = if deep {
            vec![1]
        } else {
            (1..=count as u32).collect()
        };
        m.nodes[0].logical = count as u64;
        m.unique_allocated = None;
        m
    }
    #[test]
    fn deep_queries_do_not_recurse_or_materialize_every_path() {
        let m = synthetic(30_000, true);
        let q = ViewRequest {
            mode: ViewMode::Largest,
            limit: 1,
            ..Default::default()
        };
        let v = build_view(&m, &q, 1);
        assert_eq!(v.total_rows, 1);
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.rows[0].id, 30_000);
    }
    #[test]
    #[ignore = "explicit million-node background query benchmark"]
    fn million_node_query_stress() {
        let m = synthetic(1_000_000, false);
        let started = Instant::now();
        let v = build_view(
            &m,
            &ViewRequest {
                mode: ViewMode::Largest,
                limit: 128,
                ..Default::default()
            },
            1,
        );
        eprintln!(
            "million nodes: {:?}, arena node bytes >= {}, returned rows {}, rectangles {}",
            started.elapsed(),
            m.nodes.len() * std::mem::size_of::<Node>(),
            v.rows.len(),
            v.map.len()
        );
        assert_eq!(v.total_rows, 1_000_000);
        assert_eq!(v.rows.len(), 128);
        assert!(v.map.len() <= 2000);
    }
}
