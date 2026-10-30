//! Bounded session incident records. No automatic disk persistence.
use super::*;
use serde_json::{Value, json};
use std::collections::VecDeque;

const CAPACITY: usize = 64;
const EXPORT_BYTES: usize = 48 * 1024;

#[derive(Clone, Debug)]
pub(super) struct Record {
    pub sequence: u64,
    pub generation: u64,
    pub operation: &'static str,
    pub root: PathBuf,
    pub path: PathBuf,
    pub requested_utc: Option<u64>,
    pub started_utc: Option<u64>,
    pub finished_utc: Option<u64>,
    pub reconciled_utc: Option<u64>,
    pub status: &'static str,
    pub message: String,
    pub message_truncated: bool,
    pub reconciliation_interrupted: bool,
    pub verification: Option<&'static str>,
    pub logical_change: Option<i128>,
    pub available_change: Option<i128>,
}

#[derive(Default)]
pub(super) struct Journal {
    records: VecDeque<Record>,
    evicted: u64,
}

impl Journal {
    pub fn request(
        &mut self,
        sequence: u64,
        generation: u64,
        operation: &'static str,
        root: PathBuf,
        path: PathBuf,
        time: Option<u64>,
    ) {
        if self.records.iter().any(|r| r.sequence == sequence) {
            return;
        }
        if self.records.len() == CAPACITY {
            self.records.pop_front();
            self.evicted = self.evicted.saturating_add(1);
        }
        self.records.push_back(Record {
            sequence,
            generation,
            operation,
            root,
            path,
            requested_utc: time,
            started_utc: None,
            finished_utc: None,
            reconciled_utc: None,
            status: "preparing",
            message: String::new(),
            message_truncated: false,
            reconciliation_interrupted: false,
            verification: None,
            logical_change: None,
            available_change: None,
        });
    }
    pub fn stage(&mut self, sequence: u64, stage: &'static str) {
        if let Some(record) = self.records.iter_mut().find(|r| r.sequence == sequence)
            && matches!(
                record.status,
                "preparing" | "awaiting_confirmation" | "running"
            )
        {
            record.status = stage;
        }
    }
    pub fn finish(
        &mut self,
        sequence: u64,
        started: Option<u64>,
        finished: Option<u64>,
        status: &'static str,
        message: String,
    ) -> bool {
        let Some(record) = self.records.iter_mut().find(|r| r.sequence == sequence) else {
            return false;
        };
        if !matches!(
            record.status,
            "preparing" | "awaiting_confirmation" | "running"
        ) {
            return false;
        }
        record.started_utc = started;
        record.finished_utc = finished;
        record.status = status;
        (record.message, record.message_truncated) = short(&message);
        true
    }
    pub fn reconcile(
        &mut self,
        sequence: u64,
        time: Option<u64>,
        verification: &'static str,
        logical: Option<i128>,
        available: Option<i128>,
    ) {
        if let Some(record) = self.records.iter_mut().find(|r| r.sequence == sequence)
            && (record.verification != Some(verification)
                || record.logical_change != logical
                || record.available_change != available)
        {
            record.reconciled_utc = time;
            record.verification = Some(verification);
            record.logical_change = logical;
            record.available_change = available;
        }
    }
    pub fn interrupt(
        &mut self,
        sequence: u64,
        time: Option<u64>,
        verification: &'static str,
        logical: Option<i128>,
        available: Option<i128>,
    ) {
        self.reconcile(sequence, time, verification, logical, available);
        if let Some(record) = self.records.iter_mut().find(|r| r.sequence == sequence) {
            record.reconciliation_interrupted = true;
            record.reconciled_utc = time;
        }
    }
    pub fn export(&self) -> Value {
        let mut records = Vec::new();
        let mut used = 0usize;
        for r in self.records.iter().rev() {
            let (root, a) = short(&r.root.to_string_lossy());
            let (path, b) = short(&r.path.to_string_lossy());
            let value = json!({"sequence":r.sequence,"scan_generation":r.generation,"operation":r.operation,"root_display":root,"path_display":path,"requested_utc_epoch_ms":r.requested_utc,"started_utc_epoch_ms":r.started_utc,"finished_utc_epoch_ms":r.finished_utc,"reconciled_utc_epoch_ms":r.reconciled_utc,"status":r.status,"message":r.message,"message_truncated":r.message_truncated,"reconciliation_interrupted":r.reconciliation_interrupted,"verification":r.verification,"logical_change_bytes":r.logical_change.map(|v|v.to_string()),"observed_available_space_change_bytes":r.available_change.map(|v|v.to_string()),"display_path_truncated":a||b});
            let cost = serde_json::to_vec(&value).map_or(EXPORT_BYTES, |v| v.len() + 1);
            if used.saturating_add(cost) > EXPORT_BYTES - 1024 {
                break;
            }
            used += cost;
            records.push(value);
        }
        records.reverse();
        json!({"records_truncated":records.len()!=self.records.len(),"retained_records":self.records.len(),"evicted_records":self.evicted,"session_capacity":CAPACITY,"records":records,"timestamp_source":"UTC process observations; native call boundaries when started/finished are available; not a trusted external time service","note":"Session only. Explicit export is required before closing. Native display paths are not action targets. Available-space change is not attributed reclaimed space."})
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
}

fn short(text: &str) -> (String, bool) {
    let mut end = text.len().min(2048);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), end != text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requested_cancelled_and_completed_are_distinct_and_survive_new_roots() {
        let mut journal = Journal::default();
        journal.request(
            1,
            7,
            "recycle",
            "old-root".into(),
            "old-file".into(),
            Some(10),
        );
        assert!(journal.finish(
            1,
            None,
            Some(11),
            "cancelled_before_execution",
            "Cancelled".into()
        ));
        journal.request(
            2,
            8,
            "permanent",
            "new-root".into(),
            "new-file".into(),
            Some(12),
        );
        assert!(journal.finish(
            2,
            Some(13),
            Some(14),
            "deletion_requested",
            "Removal requested".into()
        ));
        journal.reconcile(2, Some(15), "Path absence verified", Some(-50), Some(0));
        let report = journal.export();
        assert_eq!(report["records"][0]["started_utc_epoch_ms"], Value::Null);
        assert_eq!(report["records"][0]["status"], "cancelled_before_execution");
        assert_eq!(report["records"][1]["started_utc_epoch_ms"], 13);
        assert_eq!(report["records"][1]["finished_utc_epoch_ms"], 14);
        assert_eq!(report["records"][1]["reconciled_utc_epoch_ms"], 15);
        assert_eq!(report["records"][1]["logical_change_bytes"], "-50");
        assert_eq!(
            report["records"][1]["observed_available_space_change_bytes"],
            "0"
        );
    }
    #[test]
    fn eviction_and_export_budget_are_explicit_and_unknown_completion_is_not_invented() {
        let mut journal = Journal::default();
        assert!(!journal.finish(99, Some(1), Some(2), "failed", "Unknown".into()));
        for n in 1..=70 {
            journal.request(
                n,
                n,
                "recycle",
                "r".into(),
                "x".repeat(8192).into(),
                Some(n),
            );
        }
        let report = journal.export();
        assert_eq!(journal.len(), 64);
        assert_eq!(report["evicted_records"], 6);
        assert_eq!(report["retained_records"], 64);
        assert_eq!(report["records_truncated"], true);
        assert!(serde_json::to_vec(&report).unwrap().len() <= EXPORT_BYTES);
        assert!(
            report["records"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["sequence"].as_u64().unwrap() >= 7)
        );
    }
    #[test]
    fn finished_records_cannot_restart_and_truncated_messages_are_disclosed() {
        let mut journal = Journal::default();
        journal.request(1, 1, "recycle", "r".into(), "p".into(), Some(1));
        journal.finish(
            1,
            None,
            Some(2),
            "cancelled_before_execution",
            "é".repeat(2048),
        );
        journal.stage(1, "running");
        assert!(!journal.finish(1, Some(3), Some(4), "recycled", "late".into()));
        let record = &journal.export()["records"][0];
        assert_eq!(record["status"], "cancelled_before_execution");
        assert_eq!(record["message_truncated"], true);
        journal.reconcile(1, Some(5), "Unavailable", None, None);
        journal.reconcile(1, Some(8), "Unavailable", None, None);
        assert_eq!(journal.export()["records"][0]["reconciled_utc_epoch_ms"], 5);
    }
}
