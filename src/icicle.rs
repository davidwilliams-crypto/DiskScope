//! Bounded normalized hierarchy-band layout using additive logical bytes.
use crate::model::*;

pub const MAX_LEVELS: usize = 8;

/// Each completed band partitions its parent's width. Child bands intentionally
/// repeat logical bytes across levels; consumers must never sum all bands.
/// When the global rectangle budget is exhausted, directories remain drillable.
pub fn build_icicle(model: &Model, root: NodeId) -> Vec<MapItem> {
    use std::{
        cmp::Reverse,
        collections::{BinaryHeap, VecDeque},
    };
    let Some(root_node) = model.nodes.get(root as usize) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    let mut queue = VecDeque::from([(root, 0f64, 1f64, 0usize)]);
    let mut levels = 1;
    while let Some((parent, x, width, depth)) = queue.pop_front() {
        let budget = crate::treemap::MAX_MAP_ITEMS - items.len();
        if budget == 0 || depth >= MAX_LEVELS {
            continue;
        }
        let node = &model.nodes[parent as usize];
        let ids: &[NodeId] = if !root_node.is_dir && parent == root {
            std::slice::from_ref(&root)
        } else {
            &node.children
        };
        let mut heap = BinaryHeap::new();
        let mut total = 0u128;
        let mut count = 0;
        for &id in ids {
            let Some(child) = model.nodes.get(id as usize) else {
                continue;
            };
            if child.logical == 0 {
                continue;
            }
            total += u128::from(child.logical);
            count += 1;
            heap.push(Reverse((child.logical, Reverse(id))));
            if heap.len() > budget {
                heap.pop();
            }
        }
        if total == 0 {
            continue;
        }
        if count > budget {
            heap.pop();
        }
        let mut visible: Vec<_> = heap
            .into_iter()
            .map(|Reverse((size, Reverse(id)))| (id, size))
            .collect();
        visible.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut remaining = total;
        let mut cursor = x;
        for (id, size) in visible {
            let child = &model.nodes[id as usize];
            let child_width = width * size as f64 / total as f64;
            items.push(MapItem {
                id,
                name: child.name.to_string_lossy().into_owned(),
                logical: size,
                rect: [cursor as f32, depth as f32, child_width as f32, 1.],
                is_dir: child.is_dir,
                others: false,
                matched: false,
            });
            if child.is_dir && depth + 1 < MAX_LEVELS {
                queue.push_back((id, cursor, child_width, depth + 1));
            }
            cursor += child_width;
            remaining -= u128::from(size);
        }
        if remaining > 0 {
            items.push(MapItem {
                id: parent,
                name: "Other".into(),
                logical: remaining.min(u128::from(u64::MAX)) as u64,
                rect: [
                    cursor as f32,
                    depth as f32,
                    (width * remaining as f64 / total as f64) as f32,
                    1.,
                ],
                is_dir: true,
                others: true,
                matched: false,
            });
        }
        levels = levels.max(depth + 1);
    }
    for item in &mut items {
        item.rect[1] /= levels as f32;
        item.rect[3] /= levels as f32;
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, logical: u64) -> Entry {
        Entry {
            name: name.into(),
            is_dir,
            logical,
            allocated: Some(0),
            modified: None,
            identity: None,
            reparse: false,
        }
    }

    #[test]
    fn nested_bands_keep_additive_widths_and_navigation_handles() {
        let mut model = Model::new("fixture".into());
        model.insert_batch(0, vec![entry("folder", true, 0), entry("file", false, 2)]);
        model.insert_batch(1, vec![entry("nested", false, 6), entry("small", false, 2)]);
        let items = build_icicle(&model, 0);
        assert_eq!(items.len(), 4);
        let folder = items.iter().find(|i| i.id == 1).unwrap();
        let nested = items.iter().find(|i| i.id == 3).unwrap();
        assert!((folder.rect[2] - 0.8).abs() < 0.00001);
        assert!((nested.rect[2] - 0.6).abs() < 0.00001);
        assert!(nested.rect[1] >= folder.rect[1] + folder.rect[3] - 0.00001);
        assert_eq!(
            items
                .iter()
                .filter(|i| i.rect[1] == 0.)
                .map(|i| i.logical)
                .sum::<u64>(),
            10
        );
        for i in &items {
            assert!(i.rect.iter().all(|x| x.is_finite() && *x >= 0.));
            assert!(i.rect[0] + i.rect[2] <= 1.00001 && i.rect[1] + i.rect[3] <= 1.00001);
        }
    }

    #[test]
    fn broad_hierarchy_is_bounded_and_other_retains_full_weight() {
        let mut model = Model::new("fixture".into());
        model.insert_batch(
            0,
            (0..5000).map(|i| entry(&i.to_string(), false, 1)).collect(),
        );
        let items = build_icicle(&model, 0);
        assert_eq!(items.len(), 2000);
        assert_eq!(items.iter().map(|i| i.logical).sum::<u64>(), 5000);
        let other = items.iter().find(|i| i.others).unwrap();
        assert_eq!((other.id, other.logical), (0, 3001));
        assert!((items.iter().map(|i| i.rect[2]).sum::<f32>() - 1.).abs() < 0.0001);
    }
    #[test]
    fn deep_hierarchy_stops_at_eight_bands_without_recursion() {
        let mut model = Model::new("fixture".into());
        for id in 1..=30_000u32 {
            model.nodes.push(Node {
                id,
                parent: Some(id - 1),
                name: "folder".into(),
                is_dir: true,
                reparse: false,
                logical: 1,
                allocated: Some(0),
                files: 1,
                folders: 0,
                modified: None,
                identity: None,
                hard_link: false,
                allocation_complete: true,
                children: if id < 30_000 {
                    vec![id + 1]
                } else {
                    Vec::new()
                },
            });
        }
        model.nodes[0].children = vec![1];
        model.nodes[0].logical = 1;
        let items = build_icicle(&model, 0);
        assert_eq!(items.len(), 8);
        for (i, item) in items.iter().enumerate() {
            assert_eq!(item.id, i as u32 + 1);
            assert_eq!(item.rect, [0., i as f32 / 8., 1., 1. / 8.]);
        }
    }
    #[test]
    fn empty_invalid_and_file_roots_are_defined() {
        let mut model = Model::new("fixture".into());
        assert!(build_icicle(&model, 0).is_empty());
        assert!(build_icicle(&model, u32::MAX).is_empty());
        model.insert_batch(0, vec![entry("file", false, 6)]);
        let item = build_icicle(&model, 1).pop().unwrap();
        assert_eq!(item.id, 1);
        assert_eq!(item.rect, [0., 0., 1., 1.]);
    }
}
