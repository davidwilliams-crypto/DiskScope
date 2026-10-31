//! Bounded normalized treemap layout using additive logical bytes.
use crate::model::*;
use std::{cmp::Reverse, collections::BinaryHeap};

pub const MAX_MAP_ITEMS: usize = 2000;

/// `others=true` uses the current root as its drill-down handle. It represents
/// hidden direct children, so consumers must check `others` before selecting it.
/// Scratch and output storage are O(MAX_MAP_ITEMS), even for a million siblings.
pub fn build_treemap(model: &Model, root: NodeId, aspect: f32) -> Vec<MapItem> {
    let Some(node) = model.nodes.get(root as usize) else {
        return Vec::new();
    };
    let mut largest = BinaryHeap::new();
    let mut total = 0u128;
    let mut positive = 0usize;
    let ids: &[NodeId] = if node.is_dir {
        &node.children
    } else {
        std::slice::from_ref(&root)
    };
    for &id in ids {
        let Some(child) = model.nodes.get(id as usize) else {
            continue;
        };
        if child.logical == 0 {
            continue;
        }
        positive += 1;
        total += u128::from(child.logical);
        retain_map_candidate(&mut largest, (child.logical, Reverse(id)), MAX_MAP_ITEMS);
    }
    if total == 0 {
        return Vec::new();
    }
    // Reserve the last slot for Other only when entries were discarded.
    if positive > MAX_MAP_ITEMS {
        largest.pop();
    }
    let mut visible: Vec<_> = largest
        .into_iter()
        .map(|Reverse((size, Reverse(id)))| (id, size))
        .collect();
    visible.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut remaining = total;
    let mut weights = Vec::with_capacity(MAX_MAP_ITEMS);
    let mut items = Vec::with_capacity(MAX_MAP_ITEMS);
    for (id, size) in visible {
        // Very small slivers are aggregated even below the count cap.
        if size as f64 / (total as f64) < 0.000001 {
            continue;
        }
        let child = &model.nodes[id as usize];
        remaining -= u128::from(size);
        weights.push(size as f64);
        items.push(MapItem {
            id,
            name: child.name.to_string_lossy().into_owned(),
            logical: size,
            rect: [0.; 4],
            is_dir: child.is_dir,
            others: false,
            matched: false,
        });
    }
    if remaining > 0 {
        // If all 2,000 retained entries survived the tiny-entry threshold, free a
        // slot before appending an aggregate (possible after numeric extremes).
        if items.len() == MAX_MAP_ITEMS {
            remaining += u128::from(items.pop().unwrap().logical);
            weights.pop();
        }
        weights.push(remaining as f64);
        items.push(MapItem {
            id: root,
            name: "Other".into(),
            logical: remaining.min(u128::from(u64::MAX)) as u64,
            rect: [0.; 4],
            is_dir: true,
            others: true,
            matched: false,
        });
    }
    let aspect = if aspect.is_finite() && aspect > 0. {
        f64::from(aspect).clamp(0.01, 100.)
    } else {
        1.
    };
    let mut prefix = Vec::with_capacity(weights.len() + 1);
    prefix.push(0.);
    for weight in weights {
        prefix.push(prefix.last().copied().unwrap() + weight);
    }
    let mut work = vec![(0usize, items.len(), [0., 0., 1., 1.])];
    while let Some((start, end, [x, y, w, h])) = work.pop() {
        if end - start == 1 {
            items[start].rect = [x as f32, y as f32, w as f32, h as f32];
            continue;
        }
        let sum = prefix[end] - prefix[start];
        let half = prefix[start] + sum / 2.;
        let mut split = prefix[start + 1..end].partition_point(|weight| *weight < half) + start + 1;
        split = split.clamp(start + 1, end - 1);
        if split > start + 1 && (prefix[split - 1] - half).abs() < (prefix[split] - half).abs() {
            split -= 1;
        }
        let fraction = ((prefix[split] - prefix[start]) / sum).clamp(0., 1.);
        if w * aspect >= h {
            let first = w * fraction;
            work.push((split, end, [x + first, y, w - first, h]));
            work.push((start, split, [x, y, first, h]));
        } else {
            let first = h * fraction;
            work.push((split, end, [x, y + first, w, h - first]));
            work.push((start, split, [x, y, w, first]));
        }
    }
    items
}

/// Keep the largest logical weights, with smaller IDs winning equal-size ties.
pub(crate) fn retain_map_candidate(
    heap: &mut BinaryHeap<Reverse<(u64, Reverse<NodeId>)>>,
    candidate: (u64, Reverse<NodeId>),
    limit: usize,
) -> bool {
    if heap.len() < limit {
        heap.push(Reverse(candidate));
        true
    } else if heap.peek().is_some_and(|worst| candidate > worst.0) {
        *heap.peek_mut().expect("the full heap has a root") = Reverse(candidate);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model(sizes: &[u64]) -> Model {
        let mut m = Model::new("fixture".into());
        m.insert_batch(
            0,
            sizes
                .iter()
                .enumerate()
                .map(|(i, s)| Entry {
                    name: i.to_string().into(),
                    is_dir: false,
                    reparse: false,
                    logical: *s,
                    allocated: Some(0),
                    modified: None,
                    identity: None,
                })
                .collect(),
        );
        m
    }
    fn check(items: &[MapItem]) {
        for a in items {
            let [x, y, w, h] = a.rect;
            assert!(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite());
            assert!(
                x >= 0. && y >= 0. && w >= 0. && h >= 0. && x + w <= 1.000001 && y + h <= 1.000001
            );
        }
        for (i, a) in items.iter().enumerate() {
            for b in &items[i + 1..] {
                let overlap_x =
                    (a.rect[0] + a.rect[2]).min(b.rect[0] + b.rect[2]) - a.rect[0].max(b.rect[0]);
                let overlap_y =
                    (a.rect[1] + a.rect[3]).min(b.rect[1] + b.rect[3]) - a.rect[1].max(b.rect[1]);
                assert!(overlap_x <= 0.000001 || overlap_y <= 0.000001);
            }
        }
    }
    #[test]
    fn normalized_nonoverlapping_areas_track_additive_logical_weights() {
        let m = model(&[6, 3, 1, 0]);
        let a = build_treemap(&m, 0, 2.5);
        assert_eq!(a.len(), 3);
        check(&a);
        for item in &a {
            assert!(
                ((item.rect[2] * item.rect[3]) as f64 - item.logical as f64 / 10.).abs() < 0.00001
            );
        }
        assert_eq!(a.iter().map(|x| x.logical).sum::<u64>(), 10);
    }
    #[test]
    fn cap_keeps_largest_and_aggregates_remaining_weight_in_drilldown_other() {
        let m = model(&vec![1; 5000]);
        let a = build_treemap(&m, 0, f32::NAN);
        assert!(a.len() <= 2000);
        assert_eq!(a.iter().map(|x| x.logical).sum::<u64>(), 5000);
        let other = a.iter().find(|x| x.others).unwrap();
        assert_eq!(other.id, 0);
        assert_eq!(other.logical, 3001);
        check(&a);
        assert!((a.iter().map(|x| x.rect[2] * x.rect[3]).sum::<f32>() - 1.).abs() < 0.0001);
    }
    #[test]
    fn empty_zero_and_invalid_roots_have_no_rectangles() {
        assert!(build_treemap(&model(&[0, 0]), 0, 0.).is_empty());
        assert!(build_treemap(&model(&[1]), u32::MAX, 1.).is_empty());
    }

    #[test]
    fn map_candidates_skip_heap_updates_for_noncompetitive_weights() {
        let mut heap = BinaryHeap::new();
        let updates = (0..100_000)
            .filter(|&id| {
                retain_map_candidate(&mut heap, (100_000 - u64::from(id), Reverse(id)), 100)
            })
            .count();
        assert_eq!(updates, 100);
        assert_eq!(heap.len(), 100);
        assert_eq!(heap.peek().unwrap().0, (99_901, Reverse(99)));
    }

    #[test]
    fn map_candidate_cutoff_preserves_exact_weights_and_id_ties() {
        let candidates: Vec<_> = (0..5000)
            .map(|id| {
                (
                    if id % 19 == 0 {
                        u64::MAX
                    } else {
                        u64::from((id * 7919) % 17)
                    },
                    Reverse(id),
                )
            })
            .collect();
        for limit in [0, 1, 17, 2000, 6000] {
            let mut heap = BinaryHeap::new();
            for &candidate in &candidates {
                retain_map_candidate(&mut heap, candidate, limit);
            }
            let mut result: Vec<_> = heap.into_iter().map(|Reverse(value)| value).collect();
            result.sort_unstable_by(|a, b| b.cmp(a));
            let mut oracle = candidates.clone();
            oracle.sort_unstable_by(|a, b| b.cmp(a));
            oracle.truncate(limit);
            assert_eq!(result, oracle);
        }
    }
}
