//! Opt-in process aggregates. Worker wall spans overlap one another and the coordinator.
//! Reset only before starting a controller; snapshots taken during work are approximate.
use serde::Serialize;

macro_rules! metrics {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug)]
        #[repr(usize)]
        pub enum Metric { $($variant,)+ }
        #[cfg(feature = "diagnostics")]
        const METRICS: &[Metric] = &[$(Metric::$variant,)+];
        impl Metric {
            pub const fn name(self) -> &'static str {
                match self { $(Self::$variant => $name,)+ }
            }
        }
    };
}
metrics! {
    QueryActiveLargest => "query_active_largest_ns",
    QueryActiveOverview => "query_active_overview_ns",
    QueryActiveExplorer => "query_active_explorer_ns",
    QueryActiveTypes => "query_active_types_ns",
    QueryTerminalLargest => "query_terminal_largest_ns",
    QueryTerminalOverview => "query_terminal_overview_ns",
    QueryTerminalExplorer => "query_terminal_explorer_ns",
    QueryTerminalTypes => "query_terminal_types_ns",
    Ingest => "coordinator_ingest_ns",
    IngestEntries => "ingested_entries",
    Schedule => "coordinator_schedule_ns",
    CursorVisited => "scheduler_cursor_visited",
    TasksAccepted => "scheduler_tasks_accepted",
    TasksFull => "scheduler_queue_rejected",
    // `EventWait` covers every blocking wait of the coordinator: for a worker event, a UI
    // command, or a timeout. An idle coordinator sleeps up to a second per wait, so a few long
    // samples mean idle, not busy. `IdleSleep` is only the shutdown wait after every event
    // sender has gone.
    IdleSleep => "coordinator_sleep_ns",
    EventWait => "coordinator_event_wait_ns",
    WorkerEnumeration => "worker_enumeration_including_delivery_ns",
    WorkerEnumerationDelivery => "worker_enumeration_callback_delivery_ns",
    WorkerDelivery => "worker_event_delivery_ns",
    WorkerDeliveryTimeouts => "worker_event_delivery_timeouts",
    WorkerDeliveryAbandoned => "worker_event_delivery_abandoned",
    FinishSweep => "coordinator_completion_sweep_ns",
    Replacement => "coordinator_replace_scan_including_drop_ns",
    PathBuild => "path_build_ns",
    PathSteps => "path_parent_steps",
    AncestorSteps => "insert_ancestor_steps",
    QueryAllocation => "query_allocation_ns",
    QueryMap => "query_map_ns",
    QueryMatches => "query_match_flags_ns",
    QueryBreakdown => "query_pins_breakdown_ns",
    QueryTraversal => "query_subtree_traversal_ns",
    SummaryUpdate => "root_summary_update_ns",
    SummaryFiles => "root_summary_files_appended",
    SummaryHits => "root_summary_query_hits",
    QuerySort => "query_sort_ns",
    QueryVisited => "query_subtree_nodes_visited",
    QueryCandidates => "query_sort_candidates",
    Publish => "coordinator_snapshot_build_ns",
    Publications => "coordinator_publications",
    HistoryHandoff => "coordinator_history_handoff_ns",
}

pub const HISTOGRAM_BUCKETS: usize = 24;

#[derive(Debug, Serialize)]
pub struct MetricSnapshot {
    pub name: &'static str,
    pub unit: &'static str,
    pub samples: u64,
    pub spans_started: u64,
    pub in_flight: u64,
    pub total: u64,
    pub max: u64,
    /// Upper bounds: 1000 * 2^index native units; last bucket is overflow.
    pub histogram: [u64; HISTOGRAM_BUCKETS],
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub enabled: bool,
    pub semantics: &'static str,
    pub metrics: Vec<MetricSnapshot>,
}

#[inline(always)]
pub fn query_metric(active: bool, mode: crate::model::ViewMode) -> Metric {
    use crate::model::ViewMode;
    match (active, mode) {
        (true, ViewMode::Largest) => Metric::QueryActiveLargest,
        (true, ViewMode::Overview) => Metric::QueryActiveOverview,
        (true, ViewMode::Explorer) => Metric::QueryActiveExplorer,
        (true, ViewMode::Types) => Metric::QueryActiveTypes,
        (false, ViewMode::Largest) => Metric::QueryTerminalLargest,
        (false, ViewMode::Overview) => Metric::QueryTerminalOverview,
        (false, ViewMode::Explorer) => Metric::QueryTerminalExplorer,
        (false, ViewMode::Types) => Metric::QueryTerminalTypes,
    }
}

#[cfg(feature = "diagnostics")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "diagnostics")]
struct Aggregate {
    samples: AtomicU64,
    spans_started: AtomicU64,
    in_flight: AtomicU64,
    total: AtomicU64,
    max: AtomicU64,
    histogram: [AtomicU64; HISTOGRAM_BUCKETS],
}

#[cfg(feature = "diagnostics")]
impl Aggregate {
    const fn new() -> Self {
        Self {
            samples: AtomicU64::new(0),
            spans_started: AtomicU64::new(0),
            in_flight: AtomicU64::new(0),
            total: AtomicU64::new(0),
            max: AtomicU64::new(0),
            histogram: [const { AtomicU64::new(0) }; HISTOGRAM_BUCKETS],
        }
    }
    fn record(&self, value: u64) {
        self.samples.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(value, Ordering::Relaxed);
        self.max.fetch_max(value, Ordering::Relaxed);
        let thousands = value.saturating_add(999) / 1000;
        let bucket = if thousands <= 1 {
            0
        } else {
            (u64::BITS - (thousands - 1).leading_zeros()) as usize
        };
        self.histogram[bucket.min(HISTOGRAM_BUCKETS - 1)].fetch_add(1, Ordering::Relaxed);
    }
    fn begin_span(&self) {
        self.spans_started.fetch_add(1, Ordering::Relaxed);
        self.in_flight.fetch_add(1, Ordering::Relaxed);
    }
    fn end_span(&self, value: u64) {
        self.record(value);
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
    fn clear(&self) {
        self.samples.store(0, Ordering::Relaxed);
        self.spans_started.store(0, Ordering::Relaxed);
        self.in_flight.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
        self.max.store(0, Ordering::Relaxed);
        for bucket in &self.histogram {
            bucket.store(0, Ordering::Relaxed);
        }
    }
    fn snapshot(&self, metric: Metric) -> MetricSnapshot {
        MetricSnapshot {
            name: metric.name(),
            unit: if metric.name().ends_with("_ns") {
                "nanoseconds"
            } else {
                "count"
            },
            samples: self.samples.load(Ordering::Relaxed),
            spans_started: self.spans_started.load(Ordering::Relaxed),
            in_flight: self.in_flight.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            max: self.max.load(Ordering::Relaxed),
            histogram: std::array::from_fn(|i| self.histogram[i].load(Ordering::Relaxed)),
        }
    }
}

#[cfg(feature = "diagnostics")]
static AGGREGATES: [Aggregate; METRICS.len()] = [const { Aggregate::new() }; METRICS.len()];

/// One bulk observation, not one atomic increment per traversed node.
#[inline(always)]
pub fn add(metric: Metric, value: u64) {
    #[cfg(feature = "diagnostics")]
    AGGREGATES[metric as usize].record(value);
    #[cfg(not(feature = "diagnostics"))]
    let _ = (metric, value);
}

/// Call before starting any controller. Reset during active work loses observations.
#[inline]
pub fn reset() {
    #[cfg(feature = "diagnostics")]
    for aggregate in &AGGREGATES {
        aggregate.clear();
    }
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        enabled: cfg!(feature = "diagnostics"),
        semantics: "Process aggregates; worker wall spans overlap each other and coordinator work. Enumeration includes event delivery; query subspans nest inside query and snapshot spans. Do not add overlapping totals. Live snapshots are approximate; reset only before workers start.",
        #[cfg(feature = "diagnostics")]
        metrics: METRICS
            .iter()
            .map(|&m| AGGREGATES[m as usize].snapshot(m))
            .collect(),
        #[cfg(not(feature = "diagnostics"))]
        metrics: Vec::new(),
    }
}

/// Zero-sized, timer-free and inlinable when diagnostics is disabled.
pub struct Span {
    #[cfg(feature = "diagnostics")]
    metric: Metric,
    #[cfg(feature = "diagnostics")]
    started: std::time::Instant,
}
impl Span {
    #[inline(always)]
    pub fn new(metric: Metric) -> Self {
        #[cfg(feature = "diagnostics")]
        AGGREGATES[metric as usize].begin_span();
        #[cfg(not(feature = "diagnostics"))]
        let _ = metric;
        Self {
            #[cfg(feature = "diagnostics")]
            metric,
            #[cfg(feature = "diagnostics")]
            started: std::time::Instant::now(),
        }
    }
}
impl Drop for Span {
    #[inline(always)]
    fn drop(&mut self) {
        #[cfg(feature = "diagnostics")]
        AGGREGATES[self.metric as usize]
            .end_span(self.started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64);
    }
}

/// Stack-local bulk counter; recording happens once at scope exit, never per node.
pub struct Counter {
    #[cfg(feature = "diagnostics")]
    metric: Metric,
    #[cfg(feature = "diagnostics")]
    value: u64,
}
impl Counter {
    #[inline(always)]
    pub fn new(metric: Metric) -> Self {
        #[cfg(not(feature = "diagnostics"))]
        let _ = metric;
        Self {
            #[cfg(feature = "diagnostics")]
            metric,
            #[cfg(feature = "diagnostics")]
            value: 0,
        }
    }
    #[inline(always)]
    pub fn bump(&mut self) {
        #[cfg(feature = "diagnostics")]
        {
            self.value += 1;
        }
    }
}
impl Drop for Counter {
    #[inline(always)]
    fn drop(&mut self) {
        #[cfg(feature = "diagnostics")]
        add(self.metric, self.value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "diagnostics")]
    #[test]
    fn aggregates_are_fixed_and_preserve_count_sum_max_and_buckets() {
        let aggregate = Aggregate::new();
        aggregate.record(0);
        aggregate.record(1);
        aggregate.record(1_000);
        aggregate.record(8_000_000);
        let sample = aggregate.snapshot(Metric::PathSteps);
        assert_eq!(sample.samples, 4);
        assert_eq!(sample.total, 8_001_001);
        assert_eq!(sample.max, 8_000_000);
        assert_eq!(sample.histogram.iter().sum::<u64>(), 4);
        assert_eq!(sample.histogram[0], 3);
        assert_eq!(sample.histogram[13], 1);
        aggregate.clear();
        assert_eq!(aggregate.snapshot(Metric::PathSteps).total, 0);
        assert_eq!(aggregate.snapshot(Metric::PathSteps).samples, 0);
    }

    #[cfg(feature = "diagnostics")]
    #[test]
    fn query_metrics_distinguish_active_terminal_and_every_view_mode() {
        use crate::model::ViewMode;
        let modes = [
            ViewMode::Largest,
            ViewMode::Overview,
            ViewMode::Explorer,
            ViewMode::Types,
        ];
        let mut selected = std::collections::HashSet::new();
        for mode in modes {
            assert!(selected.insert(query_metric(true, mode) as usize));
            assert!(selected.insert(query_metric(false, mode) as usize));
        }
        assert_eq!(selected.len(), 8);
    }

    #[cfg(feature = "diagnostics")]
    #[test]
    fn unfinished_spans_remain_visible_without_claiming_elapsed_time() {
        let aggregate = Aggregate::new();
        aggregate.begin_span();
        let active = aggregate.snapshot(Metric::QueryActiveLargest);
        assert_eq!(active.spans_started, 1);
        assert_eq!(active.in_flight, 1);
        assert_eq!(active.samples, 0);
        assert_eq!(active.total, 0);
        aggregate.end_span(42);
        let finished = aggregate.snapshot(Metric::QueryActiveLargest);
        assert_eq!(finished.in_flight, 0);
        assert_eq!(finished.samples, 1);
        assert_eq!(finished.total, 42);
    }

    #[cfg(feature = "diagnostics")]
    #[test]
    fn concurrent_observations_preserve_bulk_totals() {
        let aggregate = std::sync::Arc::new(Aggregate::new());
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let aggregate = aggregate.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        aggregate.record(7);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let sample = aggregate.snapshot(Metric::AncestorSteps);
        assert_eq!(sample.samples, 400);
        assert_eq!(sample.total, 2800);
        assert_eq!(sample.max, 7);
        assert_eq!(sample.histogram.iter().sum::<u64>(), 400);
    }

    #[cfg(not(feature = "diagnostics"))]
    #[test]
    fn disabled_instrumentation_is_zero_sized_and_empty() {
        assert_eq!(std::mem::size_of::<Span>(), 0);
        assert_eq!(std::mem::size_of::<Counter>(), 0);
        let mut counter = Counter::new(Metric::AncestorSteps);
        counter.bump();
        drop(counter);
        add(Metric::PathSteps, 99);
        drop(Span::new(Metric::WorkerEnumeration));
        assert!(!snapshot().enabled);
        assert!(snapshot().metrics.is_empty());
    }
}
