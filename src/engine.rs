//! Bounded directory workers and latest-value UI mailboxes. No UI method waits.
use crate::{
    diagnostics::{self, Counter, Metric, Span},
    model::*,
    platform::{self, EnumerationEvent},
};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, bounded};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const QUEUE: usize = 16;
const PUBLISH_INTERVAL: Duration = Duration::from_millis(200);

struct Latest<T> {
    tx: Sender<T>,
    rx: Receiver<T>,
}
impl<T> Clone for Latest<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            rx: self.rx.clone(),
        }
    }
}
impl<T> Latest<T> {
    fn new() -> Self {
        let (tx, rx) = bounded(1);
        Self { tx, rx }
    }
    fn set(&self, mut value: T) {
        loop {
            match self.tx.try_send(value) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => return,
                Err(TrySendError::Full(v)) => {
                    value = v;
                    let _ = self.rx.try_recv();
                }
            }
        }
    }
    fn take(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

#[derive(Default)]
struct Signals {
    generation: AtomicU64,
    cancel: AtomicBool,
    stop: AtomicBool,
}
impl Signals {
    fn obsolete(&self, generation: u64) -> bool {
        self.stop.load(Ordering::Acquire)
            || self.cancel.load(Ordering::Acquire)
            || self.generation.load(Ordering::Acquire) != generation
    }
}
struct Start {
    generation: u64,
    root: PathBuf,
    backend: BackendChoice,
}
struct Task {
    generation: u64,
    parent: NodeId,
    path: PathBuf,
    kind: TaskKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskKind {
    Directory,
    Ntfs,
}
enum Event {
    Started(u64, PathBuf),
    Entries(u64, NodeId, Vec<Entry>),
    Error(u64, PathBuf, String),
    Skipped(u64),
    Done(u64, NodeId),
    NtfsProgress(u64, crate::ntfs::NtfsProgress),
    NtfsResult(u64, Result<Model, String>),
}

pub struct Controller {
    signals: Arc<Signals>,
    starts: Latest<Start>,
    views: Latest<ViewRequest>,
    snapshots: Latest<Arc<UiSnapshot>>,
}
impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}
impl Controller {
    pub fn new() -> Self {
        Self::create(None)
    }
    pub fn with_history(history: crate::history::HistoryCapture) -> Self {
        Self::create(Some(history))
    }
    fn create(history: Option<crate::history::HistoryCapture>) -> Self {
        let signals = Arc::new(Signals::default());
        let starts = Latest::new();
        let views = Latest::new();
        let snapshots = Latest::new();
        let (tasks_tx, tasks_rx) = bounded::<Task>(QUEUE);
        let (events_tx, events_rx) = bounded::<Event>(QUEUE);
        let workers = thread::available_parallelism()
            .map_or(2, usize::from)
            .clamp(2, 4);
        for index in 0..workers {
            let (tasks, events, signals) = (tasks_rx.clone(), events_tx.clone(), signals.clone());
            thread::Builder::new()
                .name(format!("directory-{index}"))
                .spawn(move || worker(tasks, events, signals))
                .expect("directory worker");
        }
        let (s, v, p, control) = (
            starts.clone(),
            views.clone(),
            snapshots.clone(),
            signals.clone(),
        );
        thread::Builder::new()
            .name("scan-coordinator".into())
            .spawn(move || coordinate(s, v, p, control, tasks_tx, events_rx, history))
            .expect("scan coordinator");
        Self {
            signals,
            starts,
            views,
            snapshots,
        }
    }
    pub fn start(&mut self, root: PathBuf, backend: BackendChoice) -> u64 {
        let generation = self.signals.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.signals.cancel.store(false, Ordering::Release);
        self.starts.set(Start {
            generation,
            root,
            backend,
        });
        generation
    }
    pub fn cancel(&self) {
        self.signals.cancel.store(true, Ordering::Release);
    }
    pub fn request_view(&self, request: ViewRequest) {
        self.views.set(request);
    }
    pub fn poll(&self) -> Option<Arc<UiSnapshot>> {
        let snapshot = self.snapshots.take()?;
        (snapshot.progress.generation == self.signals.generation.load(Ordering::Acquire))
            .then_some(snapshot)
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.signals.stop.store(true, Ordering::Release);
    }
}

fn deliver(events: &Sender<Event>, mut event: Event, signals: &Signals, generation: u64) -> bool {
    let _delivery = Span::new(Metric::WorkerDelivery);
    let mut timeouts = Counter::new(Metric::WorkerDeliveryTimeouts);
    loop {
        if signals.obsolete(generation) {
            diagnostics::add(Metric::WorkerDeliveryAbandoned, 1);
            break false;
        }
        match events.send_timeout(event, Duration::from_millis(10)) {
            Ok(()) => break true,
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => break false,
            Err(crossbeam_channel::SendTimeoutError::Timeout(value)) => {
                timeouts.bump();
                event = value;
            }
        }
    }
}
fn worker(tasks: Receiver<Task>, events: Sender<Event>, signals: Arc<Signals>) {
    while !signals.stop.load(Ordering::Acquire) {
        let task = match tasks.recv_timeout(Duration::from_millis(20)) {
            Ok(t) => t,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        if signals.obsolete(task.generation) {
            continue;
        }
        if !deliver(
            &events,
            Event::Started(task.generation, task.path.clone()),
            &signals,
            task.generation,
        ) {
            continue;
        }
        if task.kind == TaskKind::Ntfs {
            let mut last_progress: Option<Instant> = None;
            let result = crate::ntfs::scan_volume(
                &task.path,
                || signals.obsolete(task.generation),
                |progress| {
                    if !signals.obsolete(task.generation)
                        && last_progress.is_none_or(|last| last.elapsed() >= PUBLISH_INTERVAL)
                    {
                        // Progress is expendable; a full queue must never delay cancellation or raw I/O.
                        let _ = events.try_send(Event::NtfsProgress(task.generation, progress));
                        last_progress = Some(Instant::now());
                    }
                },
            );
            if !signals.obsolete(task.generation) {
                deliver(
                    &events,
                    Event::NtfsResult(task.generation, result.map_err(|error| error.to_string())),
                    &signals,
                    task.generation,
                );
            }
            continue;
        }
        let enumeration = Span::new(Metric::WorkerEnumeration);
        let result = platform::enumerate(
            &task.path,
            || signals.obsolete(task.generation),
            |event| {
                let _callback_delivery = Span::new(Metric::WorkerEnumerationDelivery);
                let event = match event {
                    EnumerationEvent::Entries(entries) => {
                        Event::Entries(task.generation, task.parent, entries)
                    }
                    EnumerationEvent::Error(path, error) => {
                        Event::Error(task.generation, path, error.to_string())
                    }
                    EnumerationEvent::Skipped => Event::Skipped(task.generation),
                };
                deliver(&events, event, &signals, task.generation)
            },
        );
        drop(enumeration);
        if let Err(error) = result {
            deliver(
                &events,
                Event::Error(task.generation, task.path, error.to_string()),
                &signals,
                task.generation,
            );
        }
        deliver(
            &events,
            Event::Done(task.generation, task.parent),
            &signals,
            task.generation,
        );
    }
}

struct Scan {
    generation: u64,
    model: Arc<Model>,
    root_summary: Option<crate::view::RootSummary>,
    history_captured: bool,
    next_node: usize,
    outstanding: usize,
    state: ScanState,
    started: Instant,
    elapsed: Option<Duration>,
    current_path: PathBuf,
    backend: String,
    root_failed: bool,
    kind: TaskKind,
    fallback_reason: Option<String>,
    raw_progress: Option<crate::ntfs::NtfsProgress>,
}
fn whole_drive_root(path: &std::path::Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    bytes.len() == 3 && bytes[0].is_ascii_alphabetic() && bytes[1..] == *b":\\"
}
impl Scan {
    fn new(start: Start) -> Self {
        let mut model = Model::new(start.root.clone());
        model.nodes[0].allocation_complete = false;
        let kind = if start.backend == BackendChoice::Ntfs && whole_drive_root(&start.root) {
            TaskKind::Ntfs
        } else {
            TaskKind::Directory
        };
        let fallback_reason = (start.backend == BackendChoice::Ntfs && kind == TaskKind::Directory)
            .then(||"NTFS reading requires a whole local drive root such as C:\\; using normal enumeration for this path.".into());
        Self {
            generation: start.generation,
            current_path: start.root.clone(),
            model: Arc::new(model),
            root_summary: (kind == TaskKind::Directory).then(crate::view::RootSummary::new),
            history_captured: false,
            next_node: 0,
            outstanding: 0,
            state: ScanState::Preparing,
            started: Instant::now(),
            elapsed: None,
            backend: if kind == TaskKind::Ntfs {
                "NTFS volume preview (unbenchmarked)"
            } else if fallback_reason.is_some() {
                "Normal fallback (whole-drive NTFS request required)"
            } else {
                "Normal (Windows directory metadata)"
            }
            .into(),
            root_failed: false,
            kind,
            fallback_reason,
            raw_progress: None,
        }
    }
    fn normal_fallback(&mut self, reason: String) {
        // Raw attempts publish no partial model. Replace every accounting structure
        // before scheduling a normal attempt within this user-visible generation.
        let mut model = Model::new(self.model.root_path.clone());
        model.nodes[0].allocation_complete = false;
        self.model = Arc::new(model);
        self.root_summary = Some(crate::view::RootSummary::new());
        self.next_node = 0;
        self.outstanding = 0;
        self.state = ScanState::Preparing;
        self.elapsed = None;
        self.root_failed = false;
        self.kind = TaskKind::Directory;
        self.backend = "Normal fallback (NTFS reader unavailable)".into();
        self.fallback_reason = Some(format!(
            "NTFS attempt failed: {reason}. Restarted with normal enumeration."
        ));
        self.raw_progress = None;
    }
    fn insert_entries(&mut self, parent: NodeId, entries: Vec<Entry>) {
        let model = Arc::get_mut(&mut self.model).expect("active scan owns model");
        let ids = model.insert_batch(parent, entries);
        if let Some(summary) = self.root_summary.as_mut() {
            summary.append(model, &ids);
        }
        for id in ids {
            let node = &mut model.nodes[id as usize];
            if node.is_dir {
                node.allocation_complete = false;
                if node.reparse {
                    model.skipped = model.skipped.saturating_add(1);
                }
            }
        }
    }
    fn finish(&mut self, state: ScanState) {
        self.state = state;
        self.elapsed = Some(self.started.elapsed());
        let _sweep = Span::new(Metric::FinishSweep);
        let model = Arc::get_mut(&mut self.model).expect("only active scans mutate metadata");
        for node in &mut model.nodes {
            if node.is_dir {
                node.allocation_complete = state == ScanState::Complete;
            }
        }
        if state != ScanState::Complete {
            model.unique_allocated = None;
        }
    }
    fn snapshot(&self, request: &ViewRequest) -> UiSnapshot {
        let root = &self.model.nodes[0];
        let message: String = if self.state == ScanState::Preparing {
            "Waiting for an available worker; previous I/O may still be finishing.".into()
        } else if self.state == ScanState::Partial {
            "Scan finished with partial results: some directories could not be read or totals overflowed.".into()
        } else if self.state == ScanState::Cancelled {
            "Scan cancelled; results and directory allocation are incomplete.".into()
        } else if self.state == ScanState::Failed {
            "Root could not be read; results are incomplete.".into()
        } else if let Some(raw) = self.raw_progress {
            format!(
                "Read {} of {} NTFS records. File totals appear only after validated reconstruction.",
                raw.records, raw.total_records
            )
        } else if self.model.overflowed {
            "A size total exceeded u64; displayed totals are saturated.".into()
        } else if self.model.unique_allocated.is_none() {
            "Unique allocation unknown: some metadata was unavailable.".into()
        } else {
            "Preview: filesystem changes during scanning may affect totals.".into()
        };
        let progress = Progress {
            generation: self.generation,
            state: self.state,
            root: self.model.root_path.clone(),
            backend: self.backend.clone(),
            files: root.files,
            folders: root.folders,
            logical: root.logical,
            unique_allocated: if root.allocation_complete {
                self.model.unique_allocated
            } else {
                None
            },
            skipped: self.model.skipped,
            errors_total: self.model.errors_total,
            errors: self.model.errors.clone(),
            hardlinks: self.model.hardlinks,
            elapsed: self.elapsed.unwrap_or_else(|| self.started.elapsed()),
            current_path: self.current_path.clone(),
            message: self
                .fallback_reason
                .as_ref()
                .map_or(message.clone(), |reason| format!("{reason} {message}")),
        };
        let view = {
            let _query = Span::new(diagnostics::query_metric(
                self.state.is_active(),
                request.mode,
            ));
            if let Some(summary) = self.root_summary.as_ref() {
                crate::view::build_view_with_summary(&self.model, request, self.generation, summary)
            } else {
                crate::view::build_view(&self.model, request, self.generation)
            }
        };
        UiSnapshot { progress, view }
    }
}
fn coordinate(
    starts: Latest<Start>,
    views: Latest<ViewRequest>,
    snapshots: Latest<Arc<UiSnapshot>>,
    signals: Arc<Signals>,
    tasks: Sender<Task>,
    events: Receiver<Event>,
    history: Option<crate::history::HistoryCapture>,
) {
    let mut scan: Option<Scan> = None;
    let mut request = ViewRequest::default();
    let mut published = Instant::now();
    let mut dirty = false;
    let mut first_publication = false;
    let mut pending_event = None;
    while !signals.stop.load(Ordering::Acquire) {
        let mut made_progress = false;
        if let Some(start) = starts.take() {
            let _replacement = Span::new(Metric::Replacement);
            scan = Some(Scan::new(start));
            dirty = true;
            first_publication = true;
            made_progress = true;
        }
        if let Some(next) = views.take() {
            request = next;
            dirty = true;
            made_progress = true;
        }
        if let Some(active) = scan.as_mut() {
            if signals.cancel.load(Ordering::Acquire) && active.state.is_active() {
                active.finish(ScanState::Cancelled);
                dirty = true;
                made_progress = true;
            }
            // Bound event work per coordinator turn so commands cannot starve.
            for _ in 0..QUEUE {
                let Some(event) = pending_event.take().or_else(|| events.try_recv().ok()) else {
                    break;
                };
                made_progress = true;
                if !active.state.is_active() {
                    continue;
                }
                match event {
                    Event::NtfsProgress(generation, progress)
                        if generation == active.generation && active.kind == TaskKind::Ntfs =>
                    {
                        active.raw_progress = Some(progress);
                        dirty = true;
                    }
                    Event::NtfsResult(generation, result)
                        if generation == active.generation && active.kind == TaskKind::Ntfs =>
                    {
                        match result {
                            Ok(model) => {
                                active.model = Arc::new(model);
                                // Raw models can be constructed independently of
                                // normal insertion, so retain the pure query path.
                                active.root_summary = None;
                                active.raw_progress = None;
                                active.outstanding = 0;
                                active.next_node = active.model.nodes.len();
                                let state =
                                    if active.model.errors_total > 0 || active.model.overflowed {
                                        ScanState::Partial
                                    } else {
                                        ScanState::Complete
                                    };
                                active.finish(state);
                            }
                            Err(reason) => active.normal_fallback(reason),
                        }
                        dirty = true;
                    }
                    Event::Started(generation, path) if generation == active.generation => {
                        if active.state == ScanState::Preparing {
                            active.state = ScanState::Scanning;
                        }
                        active.current_path = path;
                        dirty = true;
                    }
                    Event::Entries(generation, parent, entries)
                        if generation == active.generation =>
                    {
                        let _ingest = Span::new(Metric::Ingest);
                        diagnostics::add(Metric::IngestEntries, entries.len() as u64);
                        active.insert_entries(parent, entries);
                        dirty = true;
                    }
                    Event::Error(generation, path, error) if generation == active.generation => {
                        if path == active.model.root_path {
                            active.root_failed = true;
                        }
                        let model =
                            Arc::get_mut(&mut active.model).expect("active scan owns model");
                        model.record_error(path, error);
                        model.unique_allocated = None;
                        dirty = true;
                    }
                    Event::Skipped(generation) if generation == active.generation => {
                        Arc::get_mut(&mut active.model)
                            .expect("active scan owns model")
                            .skipped += 1;
                        dirty = true;
                    }
                    Event::Done(generation, parent) if generation == active.generation => {
                        active.outstanding = active.outstanding.saturating_sub(1);
                        active.current_path = active.model.path(parent);
                    }
                    _ => {}
                }
            }
            if active.state.is_active() && !signals.obsolete(active.generation) {
                let _schedule = Span::new(Metric::Schedule);
                let previous_cursor = active.next_node;
                let previous_outstanding = active.outstanding;
                let mut cursor_visited = Counter::new(Metric::CursorVisited);
                let mut tasks_accepted = Counter::new(Metric::TasksAccepted);
                let mut tasks_rejected = Counter::new(Metric::TasksFull);
                if active.kind == TaskKind::Ntfs {
                    if active.outstanding == 0 {
                        let task = Task {
                            generation: active.generation,
                            parent: 0,
                            path: active.model.root_path.clone(),
                            kind: TaskKind::Ntfs,
                        };
                        if tasks.try_send(task).is_ok() {
                            active.outstanding = 1;
                            tasks_accepted.bump();
                        } else {
                            tasks_rejected.bump();
                        }
                    }
                } else {
                    // A cursor over the model avoids a second unbounded directory frontier.
                    // Cap skipped file nodes too, keeping command handling responsive.
                    for _ in 0..512 {
                        let Some(node) = active.model.nodes.get(active.next_node) else {
                            break;
                        };
                        cursor_visited.bump();
                        if node.is_dir && !node.reparse {
                            let task = Task {
                                generation: active.generation,
                                parent: node.id,
                                path: active.model.path(node.id),
                                kind: TaskKind::Directory,
                            };
                            match tasks.try_send(task) {
                                Ok(()) => {
                                    active.outstanding += 1;
                                    tasks_accepted.bump();
                                }
                                Err(_) => {
                                    tasks_rejected.bump();
                                    break;
                                }
                            }
                        }
                        active.next_node += 1;
                    }
                    if active.next_node == active.model.nodes.len() && active.outstanding == 0 {
                        let state = if active.root_failed {
                            ScanState::Failed
                        } else if active.model.errors_total > 0 || active.model.overflowed {
                            ScanState::Partial
                        } else {
                            ScanState::Complete
                        };
                        active.finish(state);
                        dirty = true;
                    }
                }
                made_progress |= active.next_node != previous_cursor
                    || active.outstanding > previous_outstanding;
            }
            // Share the immutable complete model exactly once. The worker performs all
            // validation, metadata copying and disk operations; coordinator admission is O(1).
            if active.state == ScanState::Complete
                && !active.history_captured
                && !signals.obsolete(active.generation)
            {
                active.history_captured = true;
                if let Some(history) = &history {
                    let _handoff = Span::new(Metric::HistoryHandoff);
                    history.submit(
                        active.model.clone(),
                        active.generation,
                        active.state,
                        active.backend.clone(),
                    );
                }
            }
            if dirty
                && (first_publication
                    || !active.state.is_active()
                    || published.elapsed() >= PUBLISH_INTERVAL)
                && active.generation == signals.generation.load(Ordering::Acquire)
            {
                let _publish = Span::new(Metric::Publish);
                let snapshot = active.snapshot(&request);
                if active.generation == signals.generation.load(Ordering::Acquire) {
                    snapshots.set(Arc::new(snapshot));
                    diagnostics::add(Metric::Publications, 1);
                }
                dirty = false;
                first_publication = false;
                published = Instant::now();
            }
        }
        // Preserve per-turn event/cursor bounds for command fairness. When no
        // progress is possible, wait for a result rather than delaying workers
        // with polling sleeps. Stash just one event: the next turn checks new
        // starts/views/cancellation before handling it, including its generation.
        if !made_progress {
            let _wait = Span::new(Metric::EventWait);
            match events.recv_timeout(Duration::from_millis(2)) {
                Ok(event) => pending_event = Some(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let _sleep = Span::new(Metric::IdleSleep);
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controlled_io() -> (Controller, Receiver<Task>, Sender<Event>) {
        controlled_io_with_history(None)
    }
    fn controlled_io_with_history(
        history: Option<crate::history::HistoryCapture>,
    ) -> (Controller, Receiver<Task>, Sender<Event>) {
        let signals = Arc::new(Signals::default());
        let starts = Latest::new();
        let views = Latest::new();
        let snapshots = Latest::new();
        let (tasks_tx, tasks_rx) = bounded(QUEUE);
        let (events_tx, events_rx) = bounded(QUEUE);
        let (s, v, p, control) = (
            starts.clone(),
            views.clone(),
            snapshots.clone(),
            signals.clone(),
        );
        thread::spawn(move || coordinate(s, v, p, control, tasks_tx, events_rx, history));
        (
            Controller {
                signals,
                starts,
                views,
                snapshots,
            },
            tasks_rx,
            events_tx,
        )
    }

    #[test]
    fn history_captures_once_only_complete_current_terminal_generation() {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let history = crate::history::HistoryService::new(temp.path().join("history"));
        let (mut controller, tasks, events) = controlled_io_with_history(Some(history.capture()));
        let stale = controller.start(temp.path().join("old"), BackendChoice::Normal);
        let held = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        let generation = controller.start(temp.path().join("current"), BackendChoice::Normal);
        let task = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        events.send(Event::Done(stale, held.parent)).unwrap();
        events
            .send(Event::Entries(
                generation,
                task.parent,
                vec![fixture_entry("saved", false, 12)],
            ))
            .unwrap();
        events.send(Event::Done(generation, task.parent)).unwrap();
        wait_snapshot(&controller, |s| s.progress.state == ScanState::Complete);
        controller.request_view(ViewRequest {
            revision: 99,
            ..Default::default()
        });
        wait_snapshot(&controller, |s| s.view.revision == 99);
        let end = Instant::now() + Duration::from_secs(3);
        let mut captured = None;
        while Instant::now() < end {
            if let Some(snapshot) = history.poll()
                && !snapshot.busy
                && snapshot.scans.len() == 1
            {
                captured = Some(snapshot);
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let snapshot = captured.expect("complete model was not captured");
        assert_eq!(snapshot.scans[0].live_generation, Some(generation));
        assert_eq!(snapshot.scans[0].logical, 12);
        let cancelled = controller.start(temp.path().join("cancelled"), BackendChoice::Normal);
        let _held = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        controller.cancel();
        wait_snapshot(&controller, |s| {
            s.progress.generation == cancelled && s.progress.state == ScanState::Cancelled
        });
        history.compare(snapshot.scans[0].id, snapshot.scans[0].id);
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(s) = history.poll()
                && !s.busy
            {
                assert_eq!(s.scans.len(), 1);
                break;
            }
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn ntfs_success_hands_off_only_the_complete_model() {
        let (mut controller, tasks, events) = controlled_io();
        let generation = controller.start("C:\\".into(), BackendChoice::Ntfs);
        let task = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(task.kind, TaskKind::Ntfs);
        events
            .send(Event::Started(generation, task.path.clone()))
            .unwrap();
        events
            .send(Event::NtfsProgress(
                generation,
                crate::ntfs::NtfsProgress {
                    records: 40,
                    total_records: 100,
                },
            ))
            .unwrap();
        let during = wait_snapshot(&controller, |s| s.progress.state == ScanState::Scanning);
        assert_eq!(during.progress.files, 0);
        assert!(during.progress.message.contains("40") && during.progress.message.contains("100"));
        let mut model = Model::new(task.path);
        model.insert_batch(0, vec![fixture_entry("raw-result", false, 71)]);
        events
            .send(Event::NtfsResult(generation, Ok(model)))
            .unwrap();
        let terminal = wait_snapshot(&controller, |s| !s.progress.state.is_active());
        assert_eq!(terminal.progress.state, ScanState::Complete);
        assert_eq!(terminal.progress.logical, 71);
        assert_eq!(terminal.progress.files, 1);
        assert!(terminal.progress.backend.contains("NTFS"));
        assert!(!terminal.progress.backend.contains("fallback"));
        assert!(tasks.recv_timeout(Duration::from_millis(30)).is_err());
    }
    #[test]
    fn ntfs_initial_and_midscan_errors_start_clean_normal_attempts() {
        for reason in [
            "Access denied while opening volume",
            "Unsupported NTFS layout after record 123",
        ] {
            let (mut controller, tasks, events) = controlled_io();
            let generation = controller.start("C:\\".into(), BackendChoice::Ntfs);
            let raw = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(raw.kind, TaskKind::Ntfs);
            events
                .send(Event::NtfsProgress(
                    generation,
                    crate::ntfs::NtfsProgress {
                        records: 123,
                        total_records: 999,
                    },
                ))
                .unwrap();
            events
                .send(Event::NtfsResult(generation, Err(reason.into())))
                .unwrap();
            let normal = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(normal.generation, generation);
            assert_eq!(normal.kind, TaskKind::Directory);
            let mut stale = Model::new("C:\\".into());
            stale.insert_batch(0, vec![fixture_entry("must-not-leak", false, 9000)]);
            events
                .send(Event::NtfsResult(generation, Ok(stale)))
                .unwrap();
            events
                .send(Event::Entries(
                    generation,
                    0,
                    vec![fixture_entry("normal", false, 9)],
                ))
                .unwrap();
            events.send(Event::Done(generation, 0)).unwrap();
            let terminal = wait_snapshot(&controller, |s| !s.progress.state.is_active());
            assert_eq!(terminal.progress.logical, 9);
            assert_eq!(terminal.progress.files, 1);
            assert_eq!(terminal.progress.errors_total, 0);
            assert!(terminal.progress.backend.contains("fallback"));
            assert!(terminal.progress.message.contains(reason));
            assert!(!terminal.progress.message.contains("999"));
        }
    }
    #[test]
    fn ntfs_folder_request_explains_normal_fallback() {
        let (mut controller, tasks, events) = controlled_io();
        let generation = controller.start("C:\\folder".into(), BackendChoice::Ntfs);
        let task = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(task.kind, TaskKind::Directory);
        events.send(Event::Done(generation, task.parent)).unwrap();
        let terminal = wait_snapshot(&controller, |s| !s.progress.state.is_active());
        assert!(terminal.progress.backend.contains("fallback"));
        assert!(
            terminal.progress.message.contains("whole")
                || terminal.progress.message.contains("drive root")
        );
    }
    #[test]
    fn cancelled_and_replaced_ntfs_results_never_start_fallback_or_publish_stale_models() {
        let (mut controller, tasks, events) = controlled_io();
        let generation = controller.start("C:\\".into(), BackendChoice::Ntfs);
        assert_eq!(
            tasks.recv_timeout(Duration::from_secs(1)).unwrap().kind,
            TaskKind::Ntfs
        );
        controller.cancel();
        events
            .send(Event::NtfsResult(generation, Err("cancelled read".into())))
            .unwrap();
        assert_eq!(
            wait_snapshot(&controller, |s| !s.progress.state.is_active())
                .progress
                .state,
            ScanState::Cancelled
        );
        assert!(tasks.recv_timeout(Duration::from_millis(30)).is_err());
        let replacement = controller.start("C:\\new".into(), BackendChoice::Normal);
        let normal = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        events
            .send(Event::NtfsResult(generation, Ok(Model::new("C:\\".into()))))
            .unwrap();
        events
            .send(Event::Done(replacement, normal.parent))
            .unwrap();
        let terminal = wait_snapshot(&controller, |s| !s.progress.state.is_active());
        assert_eq!(terminal.progress.generation, replacement);
        assert_eq!(terminal.progress.root, PathBuf::from("C:\\new"));
        assert!(terminal.progress.backend.starts_with("Normal"));
    }
    fn wait_snapshot(
        controller: &Controller,
        predicate: impl Fn(&UiSnapshot) -> bool,
    ) -> Arc<UiSnapshot> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(snapshot) = controller.poll()
                && predicate(&snapshot)
            {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "expected coordinator state was never published"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn fixture_entry(name: &str, is_dir: bool, logical: u64) -> Entry {
        Entry {
            name: name.into(),
            is_dir,
            reparse: false,
            logical,
            allocated: Some(logical),
            modified: None,
            identity: Some(FileIdentity {
                volume: 1,
                id: [name.as_bytes()[0]; 16],
            }),
        }
    }
    #[test]
    fn child_enumeration_failure_retains_results_but_is_partial_and_allocation_unknown() {
        for partial_child in [false, true] {
            let (mut controller, tasks, events) = controlled_io();
            let generation = controller.start("fixture".into(), BackendChoice::Normal);
            let root = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
            events
                .send(Event::Entries(
                    generation,
                    root.parent,
                    vec![
                        fixture_entry("good", false, 17),
                        fixture_entry("bad", true, 0),
                    ],
                ))
                .unwrap();
            events.send(Event::Done(generation, root.parent)).unwrap();
            let child = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
            if partial_child {
                events
                    .send(Event::Entries(
                        generation,
                        child.parent,
                        vec![fixture_entry("retained", false, 23)],
                    ))
                    .unwrap();
            }
            events
                .send(Event::Error(
                    generation,
                    child.path.clone(),
                    "injected child access denied".into(),
                ))
                .unwrap();
            events.send(Event::Done(generation, child.parent)).unwrap();
            let snapshot = wait_snapshot(&controller, |s| !s.progress.state.is_active());
            assert_eq!(snapshot.progress.state, ScanState::Partial);
            assert_eq!(
                snapshot.progress.logical,
                if partial_child { 40 } else { 17 }
            );
            assert_eq!(snapshot.progress.errors_total, 1);
            assert_eq!(snapshot.progress.unique_allocated, None);
            assert!(
                snapshot.progress.message.contains("partial")
                    || snapshot.progress.message.contains("incomplete")
            );
            controller.request_view(ViewRequest {
                root: child.parent,
                revision: 1,
                ..Default::default()
            });
            let subtree = wait_snapshot(&controller, |s| s.view.revision == 1);
            assert_eq!(subtree.view.unique_allocated, None);
            assert_eq!(
                subtree.view.total_logical,
                if partial_child { 23 } else { 0 }
            );
        }
    }
    #[test]
    fn cancelled_pending_child_has_unknown_allocation() {
        let (mut controller, tasks, events) = controlled_io();
        let generation = controller.start("fixture".into(), BackendChoice::Normal);
        let root = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        events
            .send(Event::Entries(
                generation,
                root.parent,
                vec![fixture_entry("pending", true, 0)],
            ))
            .unwrap();
        events.send(Event::Done(generation, root.parent)).unwrap();
        let child = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        controller.cancel();
        let snapshot = wait_snapshot(&controller, |s| s.progress.state == ScanState::Cancelled);
        assert_eq!(snapshot.progress.unique_allocated, None);
        controller.request_view(ViewRequest {
            root: child.parent,
            revision: 9,
            ..Default::default()
        });
        assert_eq!(
            wait_snapshot(&controller, |s| s.view.revision == 9)
                .view
                .unique_allocated,
            None
        );
    }
    #[test]
    fn replacement_waits_in_preparing_until_worker_admission() {
        let (mut controller, tasks, _events) = controlled_io();
        controller.start("old-io".into(), BackendChoice::Normal);
        let _held_old_io = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        let generation = controller.start("replacement".into(), BackendChoice::Normal);
        let _queued_replacement = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        let waiting = wait_snapshot(&controller, |s| s.progress.generation == generation);
        assert_eq!(waiting.progress.state, ScanState::Preparing);
        assert!(waiting.progress.message.to_lowercase().contains("waiting"));
        assert_eq!(waiting.progress.unique_allocated, None);
    }
    #[test]
    fn active_view_request_pressure_obeys_five_hz_publication_budget() {
        let (mut controller, tasks, events) = controlled_io();
        let generation = controller.start("slow-io".into(), BackendChoice::Normal);
        let task = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        events
            .send(Event::Entries(generation, task.parent, vec![]))
            .unwrap();
        let _initial = wait_snapshot(&controller, |_| true);
        let started = Instant::now();
        let mut published = 0;
        for revision in 1..=80 {
            controller.request_view(ViewRequest {
                revision,
                ..Default::default()
            });
            thread::sleep(Duration::from_millis(10));
            if controller.poll().is_some() {
                published += 1;
            }
        }
        assert!(started.elapsed() >= Duration::from_millis(800));
        let allowed = (started.elapsed().as_millis() / 200 + 1) as usize;
        assert!(
            published <= allowed,
            "urgent view requests bypassed scan budget: {published} snapshots"
        );
    }
    #[test]
    fn terminal_elapsed_is_frozen_across_later_view_requests() {
        let (mut controller, tasks, events) = controlled_io();
        let generation = controller.start("fixture".into(), BackendChoice::Normal);
        let task = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        events.send(Event::Done(generation, task.parent)).unwrap();
        let terminal = wait_snapshot(&controller, |s| s.progress.state == ScanState::Complete);
        thread::sleep(Duration::from_millis(30));
        controller.request_view(ViewRequest {
            revision: 77,
            ..Default::default()
        });
        let later = wait_snapshot(&controller, |s| s.view.revision == 77);
        assert_eq!(terminal.progress.elapsed, later.progress.elapsed);
    }

    #[test]
    fn root_summary_tracks_ingestion_and_resets_on_fallback_and_replacement() {
        fn assert_queries(scan: &Scan) {
            for mode in [
                ViewMode::Overview,
                ViewMode::Types,
                ViewMode::Explorer,
                ViewMode::Largest,
            ] {
                let request = ViewRequest {
                    mode,
                    revision: 606,
                    ..Default::default()
                };
                let actual = scan.snapshot(&request).view;
                let pure = crate::view::build_view(&scan.model, &request, scan.generation);
                assert_eq!(format!("{actual:?}"), format!("{pure:?}"));
            }
        }
        let mut scan = Scan::new(Start {
            generation: 1,
            root: "summary-old".into(),
            backend: BackendChoice::Normal,
        });
        assert!(scan.root_summary.is_some());
        scan.insert_entries(
            0,
            vec![
                fixture_entry("old.dat", false, 10),
                fixture_entry("nested", true, 0),
            ],
        );
        scan.insert_entries(2, vec![fixture_entry("child.txt", false, 20)]);
        assert_queries(&scan);
        scan.finish(ScanState::Cancelled);
        assert_queries(&scan);
        scan.normal_fallback("injected raw failure".into());
        assert!(scan.root_summary.is_some());
        assert_queries(&scan);
        assert_eq!(
            scan.snapshot(&ViewRequest {
                mode: ViewMode::Types,
                ..Default::default()
            })
            .view
            .matching_files,
            0
        );
        scan.insert_entries(0, vec![fixture_entry("fresh.bin", false, 7)]);
        assert_queries(&scan);
        let fresh = scan.snapshot(&ViewRequest {
            mode: ViewMode::Types,
            ..Default::default()
        });
        assert_eq!(fresh.view.matching_files, 1);
        assert_eq!(fresh.view.matching_logical, 7);
        assert_eq!(fresh.view.types.len(), 1);
        assert_eq!(fresh.view.types[0].extension, ".bin");
        scan.finish(ScanState::Complete);
        assert_queries(&scan);

        let replacement = Scan::new(Start {
            generation: 2,
            root: "summary-replacement".into(),
            backend: BackendChoice::Normal,
        });
        assert_queries(&replacement);
        assert_eq!(
            replacement
                .snapshot(&ViewRequest {
                    mode: ViewMode::Overview,
                    ..Default::default()
                })
                .view
                .matching_files,
            0
        );
        let raw = Scan::new(Start {
            generation: 3,
            root: "C:\\".into(),
            backend: BackendChoice::Ntfs,
        });
        assert!(raw.root_summary.is_none());
    }

    #[test]
    fn independently_constructed_ntfs_result_keeps_pure_root_queries() {
        let (mut controller, tasks, events) = controlled_io();
        let generation = controller.start("C:\\".into(), BackendChoice::Ntfs);
        let task = tasks.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(task.kind, TaskKind::Ntfs);
        let mut model = Model::new(task.path);
        // Like public synthetic models, the arena may be built directly instead
        // of through normal insertion. No normal Scan summary may be reused.
        let mut file = model.nodes[0].clone();
        file.id = 1;
        file.parent = Some(0);
        file.name = "raw-file.data".into();
        file.is_dir = false;
        file.logical = 71;
        file.allocated = None;
        model.nodes.push(file);
        model.nodes[0].children.push(1);
        model.nodes[0].logical = 71;
        controller.request_view(ViewRequest {
            mode: ViewMode::Overview,
            revision: 707,
            ..Default::default()
        });
        events
            .send_timeout(
                Event::NtfsResult(generation, Ok(model)),
                Duration::from_secs(2),
            )
            .unwrap();
        let complete = wait_snapshot(&controller, |s| {
            s.progress.state == ScanState::Complete && s.view.revision == 707
        });
        assert_eq!(complete.progress.generation, generation);
        assert_eq!(complete.view.matching_files, 1);
        assert_eq!(complete.view.matching_logical, 71);
        assert_eq!(complete.view.types.len(), 1);
        assert_eq!(complete.view.types[0].extension, ".data");
        assert_eq!(complete.view.types[0].files, 1);
        assert_eq!(complete.view.types[0].logical, 71);
        controller.request_view(ViewRequest {
            mode: ViewMode::Largest,
            revision: 708,
            ..Default::default()
        });
        let largest = wait_snapshot(&controller, |s| s.view.revision == 708);
        assert_eq!(largest.view.matching_files, 1);
        assert_eq!(largest.view.total_rows, 1);
        assert_eq!(largest.view.rows[0].name, "raw-file.data");
    }

    #[test]
    fn cursor_crosses_file_prefix_and_retains_every_directory_when_task_queue_is_full() {
        use std::collections::BTreeSet;
        const FILES: usize = 700;
        const DIRECTORIES: usize = QUEUE + 5;
        let (mut controller, tasks, events) = controlled_io();
        let generation = controller.start("cursor-fixture".into(), BackendChoice::Normal);
        let root = tasks.recv_timeout(Duration::from_secs(2)).unwrap();
        events
            .send_timeout(
                Event::Started(generation, root.path.clone()),
                Duration::from_secs(2),
            )
            .unwrap();
        let entries: Vec<_> = (0..FILES)
            .map(|index| {
                let mut entry = fixture_entry(&format!("file-{index:04}"), false, 1);
                entry.identity = None;
                entry
            })
            .chain(
                (0..DIRECTORIES)
                    .map(|index| fixture_entry(&format!("directory-{index:02}"), true, 0)),
            )
            .collect();
        // Preserve the enumerator's bounded batches; the directory frontier begins
        // after more than one full 512-node cursor turn of file nodes.
        for batch in entries.chunks(128) {
            events
                .send_timeout(
                    Event::Entries(generation, root.parent, batch.to_vec()),
                    Duration::from_secs(2),
                )
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while tasks.len() != QUEUE {
            assert!(
                Instant::now() < deadline,
                "directory task queue never reached capacity"
            );
            thread::sleep(Duration::from_millis(2));
        }
        // No worker is released yet, including the root. A view must still be
        // admitted while the bounded task queue retains sixteen pending tasks.
        controller.request_view(ViewRequest {
            mode: ViewMode::Largest,
            limit: 7,
            revision: 404,
            ..Default::default()
        });
        let saturated = wait_snapshot(&controller, |snapshot| snapshot.view.revision == 404);
        assert_eq!(saturated.progress.state, ScanState::Scanning);
        assert_eq!(saturated.progress.files, FILES as u64);
        assert_eq!(saturated.progress.folders, DIRECTORIES as u64);
        assert_eq!(saturated.progress.logical, FILES as u64);
        assert_eq!(saturated.view.total_rows, FILES);
        assert_eq!(saturated.view.rows.len(), 7);
        assert_eq!(tasks.len(), QUEUE);

        let mut covered = BTreeSet::new();
        for _ in 0..DIRECTORIES {
            let task = tasks.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(task.generation, generation);
            assert_eq!(task.kind, TaskKind::Directory);
            assert!(
                covered.insert(task.parent),
                "directory task scheduled twice"
            );
            let index = task.parent as usize - FILES - 1;
            assert!(index < DIRECTORIES);
            assert_eq!(task.path, root.path.join(format!("directory-{index:02}")));
            let mut leaf = fixture_entry(&format!("leaf-{index:02}"), false, 10);
            leaf.identity = None;
            events
                .send_timeout(
                    Event::Entries(generation, task.parent, vec![leaf]),
                    Duration::from_secs(2),
                )
                .unwrap();
            events
                .send_timeout(Event::Done(generation, task.parent), Duration::from_secs(2))
                .unwrap();
        }
        assert_eq!(
            covered,
            ((FILES + 1) as NodeId..=(FILES + DIRECTORIES) as NodeId).collect::<BTreeSet<_>>()
        );
        events
            .send_timeout(Event::Done(generation, root.parent), Duration::from_secs(2))
            .unwrap();
        let complete = wait_snapshot(&controller, |snapshot| {
            snapshot.progress.state == ScanState::Complete
        });
        assert_eq!(complete.progress.generation, generation);
        assert_eq!(complete.progress.files, (FILES + DIRECTORIES) as u64);
        assert_eq!(complete.progress.folders, DIRECTORIES as u64);
        assert_eq!(complete.progress.logical, (FILES + DIRECTORIES * 10) as u64);
        assert_eq!(complete.progress.errors_total, 0);
        assert_eq!(complete.view.total_rows, FILES + DIRECTORIES);
        assert!(tasks.try_recv().is_err());
    }

    #[test]
    fn finite_event_bursts_allow_view_cancel_and_replacement_without_stale_contamination() {
        const PREFIX: usize = QUEUE * 2 + 3;
        let (mut controller, tasks, events) = controlled_io();
        let old = controller.start("burst-old".into(), BackendChoice::Normal);
        let held_old = tasks.recv_timeout(Duration::from_secs(2)).unwrap();
        for index in 0..PREFIX {
            let mut entry = fixture_entry(&format!("retained-{index:02}"), false, 3);
            entry.identity = None;
            events
                .send_timeout(
                    Event::Entries(old, held_old.parent, vec![entry]),
                    Duration::from_secs(2),
                )
                .unwrap();
        }
        let checkpoint = PathBuf::from("prefix-ingested");
        events
            .send_timeout(
                Event::Started(old, checkpoint.clone()),
                Duration::from_secs(2),
            )
            .unwrap();
        let prefix = wait_snapshot(&controller, |snapshot| {
            snapshot.progress.current_path == checkpoint
        });
        assert_eq!(prefix.progress.files, PREFIX as u64);
        assert_eq!(prefix.progress.logical, (PREFIX * 3) as u64);

        // Three finite, gated bursts exceed a coordinator event turn. Stale
        // Entries/Done/Error events must neither alter counts nor finish the
        // retained current root. The producer has a fixed bound and deadlines.
        let (release, gates) = bounded::<()>(0);
        let (staged, completed) = bounded(1);
        let producer_events = events.clone();
        let producer = thread::spawn(move || {
            for phase in 0..3 {
                gates.recv_timeout(Duration::from_secs(2)).unwrap();
                let stale = if phase == 2 { old } else { old - 1 };
                for _ in 0..PREFIX {
                    producer_events
                        .send_timeout(
                            Event::Entries(stale, 0, vec![fixture_entry("poison", false, 99_999)]),
                            Duration::from_secs(2),
                        )
                        .unwrap();
                    producer_events
                        .send_timeout(Event::Done(stale, 0), Duration::from_secs(2))
                        .unwrap();
                }
                producer_events
                    .send_timeout(
                        Event::Error(stale, "burst-new".into(), "stale failure".into()),
                        Duration::from_secs(2),
                    )
                    .unwrap();
                staged.send_timeout(phase, Duration::from_secs(2)).unwrap();
            }
        });

        controller.request_view(ViewRequest {
            mode: ViewMode::Largest,
            limit: 3,
            revision: 505,
            ..Default::default()
        });
        release.send_timeout((), Duration::from_secs(2)).unwrap();
        let viewed = wait_snapshot(&controller, |snapshot| snapshot.view.revision == 505);
        assert_eq!(completed.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
        assert_eq!(viewed.progress.state, ScanState::Scanning);
        assert_eq!(viewed.progress.files, PREFIX as u64);
        assert_eq!(viewed.progress.logical, (PREFIX * 3) as u64);
        assert_eq!(viewed.view.total_rows, PREFIX);
        assert_eq!(viewed.view.rows.len(), 3);
        assert_eq!(viewed.progress.errors_total, 0);

        controller.cancel();
        release.send_timeout((), Duration::from_secs(2)).unwrap();
        let cancelled = wait_snapshot(&controller, |snapshot| {
            snapshot.progress.state == ScanState::Cancelled
        });
        assert_eq!(completed.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        assert_eq!(cancelled.progress.generation, old);
        assert_eq!(cancelled.progress.files, PREFIX as u64);
        assert_eq!(cancelled.progress.logical, (PREFIX * 3) as u64);
        assert_eq!(cancelled.progress.errors_total, 0);

        let replacement = controller.start("burst-new".into(), BackendChoice::Normal);
        let fresh = tasks.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(fresh.generation, replacement);
        assert_eq!(fresh.path, PathBuf::from("burst-new"));
        release.send_timeout((), Duration::from_secs(2)).unwrap();
        assert_eq!(completed.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
        producer.join().unwrap();
        let mut entry = fixture_entry("fresh", false, 7);
        entry.identity = None;
        events
            .send_timeout(
                Event::Entries(replacement, fresh.parent, vec![entry]),
                Duration::from_secs(2),
            )
            .unwrap();
        events
            .send_timeout(
                Event::Done(replacement, fresh.parent),
                Duration::from_secs(2),
            )
            .unwrap();
        let complete = wait_snapshot(&controller, |snapshot| {
            snapshot.progress.generation == replacement
                && snapshot.progress.state == ScanState::Complete
        });
        assert_eq!(complete.progress.root, PathBuf::from("burst-new"));
        assert_eq!(complete.progress.files, 1);
        assert_eq!(complete.progress.folders, 0);
        assert_eq!(complete.progress.logical, 7);
        assert_eq!(complete.progress.errors_total, 0);
        assert_eq!(complete.progress.skipped, 0);
        assert_eq!(complete.view.total_rows, 1);
        assert_eq!(complete.view.rows[0].name, "fresh");
        assert!(tasks.try_recv().is_err());
    }

    #[test]
    fn overflow_is_partial_instead_of_complete() {
        let (mut controller, tasks, events) = controlled_io();
        let generation = controller.start("fixture".into(), BackendChoice::Normal);
        let task = tasks.recv_timeout(Duration::from_secs(1)).unwrap();
        events
            .send(Event::Entries(
                generation,
                task.parent,
                vec![
                    fixture_entry("a", false, u64::MAX),
                    fixture_entry("b", false, 1),
                ],
            ))
            .unwrap();
        events.send(Event::Done(generation, task.parent)).unwrap();
        let snapshot = wait_snapshot(&controller, |s| !s.progress.state.is_active());
        assert_eq!(snapshot.progress.state, ScanState::Partial);
        assert_eq!(snapshot.progress.unique_allocated, None);
    }

    #[test]
    fn saturated_result_delivery_abandons_cancelled_and_replaced_generations() {
        for replacement in [false, true] {
            let signals = Arc::new(Signals::default());
            signals.generation.store(1, Ordering::Release);
            let (events, _held_receiver) = bounded(1);
            events.send(Event::Skipped(1)).unwrap();
            let (finished, received) = bounded(1);
            let control = signals.clone();
            thread::spawn(move || {
                finished
                    .send(deliver(&events, Event::Skipped(1), &control, 1))
                    .unwrap();
            });
            thread::sleep(Duration::from_millis(20));
            if replacement {
                signals.generation.store(2, Ordering::Release);
            } else {
                signals.cancel.store(true, Ordering::Release);
            }
            assert!(
                !received
                    .recv_timeout(Duration::from_secs(1))
                    .expect("saturated producer ignored cancellation")
            );
        }
    }
}
