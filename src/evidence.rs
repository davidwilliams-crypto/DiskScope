//! Bounded, privacy-aware support evidence shared by GUI and CLI.
use crate::model::{ScanState, UiSnapshot, ViewRequest};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_REPORT_ROWS: usize = 100;
pub const MAX_REPORT_ERRORS: usize = 1000;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_ACTION_BYTES: usize = 64 * 1024;
pub const MAX_INCIDENT_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Redaction {
    #[default]
    Redacted,
    Full,
}

#[derive(Clone, Debug)]
pub struct EvidenceContext {
    pub root: PathBuf,
    pub started_utc_epoch_ms: Option<u64>,
    pub finished_utc_epoch_ms: Option<u64>,
    pub generated_utc_epoch_ms: Option<u64>,
    /// Controlled token/access label, never a username or arbitrary OS diagnostic.
    pub access_context: String,
    /// Controlled application mode label, never a customer or ticket identifier.
    pub session_context: String,
    pub redaction: Redaction,
    pub timed_out: bool,
    pub action: Option<Value>,
    pub view_request: Option<ViewRequest>,
    /// Explicitly supplied incident labels; never inferred from the environment.
    pub incident_ticket: Option<String>,
    pub endpoint_label: Option<String>,
    /// Collected on the caller's background worker; never probed while exporting.
    pub attribution: Option<crate::identity::Attribution>,
    /// Cached sampler observation at caller retrieval, not a peak-memory claim.
    pub resource_observation: Option<crate::resources::ResourceObservation>,
    pub admission_limits: Option<crate::engine::ScanLimits>,
    pub memory_budget: Option<crate::resources::MemoryBudget>,
    /// Current-generation engine refusal only: controlled resource diagnostics,
    /// never caller/customer text, filenames or opaque native error messages.
    pub resource_refusal_reason: Option<String>,
}

impl EvidenceContext {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            started_utc_epoch_ms: None,
            finished_utc_epoch_ms: None,
            generated_utc_epoch_ms: utc_now_epoch_ms(),
            access_context: "unknown".into(),
            session_context: "unknown".into(),
            redaction: Redaction::Redacted,
            timed_out: false,
            action: None,
            view_request: None,
            incident_ticket: None,
            endpoint_label: None,
            attribution: None,
            resource_observation: None,
            admission_limits: None,
            memory_budget: None,
            resource_refusal_reason: None,
        }
    }
}

/// UTC milliseconds since Unix epoch; absent if the clock cannot represent it.
pub fn utc_now_epoch_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

/// Export only the published view and retained error sample. This is deliberately
/// not an inventory interchange format, a native-path command source, or proof
/// that an action reclaimed the displayed number of bytes.
pub fn build_report(snapshot: &UiSnapshot, context: &EvidenceContext) -> Value {
    let progress = &snapshot.progress;
    let full = context.redaction == Redaction::Full;
    let view_matches_scan = snapshot.view.generation == progress.generation;
    let mut text_truncated = false;
    let root = path_display(&context.root, full, &mut text_truncated);
    let errors: Vec<_> = progress
        .errors
        .iter()
        .take(MAX_REPORT_ERRORS)
        .map(|error| {
            json!({
                "path_display": path_display(&error.path, full, &mut text_truncated),
                "message": private_text(&error.message, full, &mut text_truncated),
            })
        })
        .collect();
    let available_rows = if view_matches_scan {
        snapshot.view.rows.as_slice()
    } else {
        &[]
    };
    let rows: Vec<_> = available_rows
        .iter()
        .take(MAX_REPORT_ROWS)
        .map(|row| {
            json!({
                "path_display": path_display(&row.path, full, &mut text_truncated),
                "logical_bytes": row.logical,
                "allocated_bytes": row.allocated,
                "hardlink": row.hard_link,
                "is_directory": row.is_dir,
                "reparse": row.reparse,
            })
        })
        .collect();
    let message = private_text(&progress.message, full, &mut text_truncated);
    let backend = bounded(&progress.backend, &mut text_truncated);
    let access = bounded(&context.access_context, &mut text_truncated);
    let session = bounded(&context.session_context, &mut text_truncated);
    let view_root = view_matches_scan
        .then(|| path_display(&snapshot.view.root_path, full, &mut text_truncated));
    let requested_view = context.view_request.as_ref().map(|request| {
        let extensions: Vec<_> = request
            .filters
            .extensions
            .iter()
            .take(MAX_REPORT_ROWS)
            .map(|extension| private_text(extension, full, &mut text_truncated))
            .collect();
        json!({
            "context": "requested view at export; may differ from last published result",
            "revision": request.revision,
            "root_node": request.root,
            "mode": format!("{:?}", request.mode),
            "sort": format!("{:?}", request.sort),
            "descending": request.descending,
            "offset": request.offset,
            "limit": request.limit,
            "search": private_text(&request.search, full, &mut text_truncated),
            "search_all": request.search_all,
            "filters": {
                "extensions": extensions,
                "extensions_truncated": request.filters.extensions.len() > MAX_REPORT_ROWS,
                "min_bytes": request.filters.min_bytes,
                "max_bytes": request.filters.max_bytes,
                "modified_after": request.filters.modified_after,
                "modified_before": request.filters.modified_before,
            },
        })
    });
    let action_too_large = full
        && context.action.as_ref().is_some_and(|action| {
            serde_json::to_writer(SizeLimit(MAX_ACTION_BYTES), action).is_err()
        });
    let action = match &context.action {
        Some(_) if !full => json!({"redacted": true}),
        Some(action) if !action_too_large => action.clone(),
        _ => Value::Null,
    };
    let digest = executable_digest();
    let scan_complete =
        !context.timed_out && progress.state == ScanState::Complete && progress.errors_total == 0;
    let attribution = attribution_report(context.attribution.as_ref(), full, &mut text_truncated);
    let incident = json!({
        "source": "explicit caller input; not verified endpoint identity",
        "ticket": incident_label(context.incident_ticket.as_deref(), full, &mut text_truncated),
        "endpoint_label": incident_label(context.endpoint_label.as_deref(), full, &mut text_truncated),
    });
    json!({
        "schema_version": 1,
        "application": "DiskScope",
        "build": {
            "version": env!("CARGO_PKG_VERSION"),
            "source_commit": option_env!("DISKSCOPE_GIT_COMMIT"),
            "executable_sha256": digest.as_ref().ok(),
            "executable_hash_status": if digest.is_ok() { "available" } else { "unavailable" },
            "hash_scope": "running executable bytes; not publisher authentication",
        },
        "timestamps": {
            "unit": "UTC milliseconds since Unix epoch",
            "started_utc_epoch_ms": context.started_utc_epoch_ms,
            "finished_utc_epoch_ms": context.finished_utc_epoch_ms,
            "generated_utc_epoch_ms": context.generated_utc_epoch_ms,
            "scan_timestamps_source": "caller observation; null when unavailable",
        },
        "context": {
            "access": access,
            "session": session,
            "endpoint_attribution": "See attribution observations; associate with the intended endpoint in the ticket",
            "volume_identity": "See attribution observations; a drive letter alone is not durable volume identity",
            "attribution": attribution,
            "incident": incident,
        },
        "privacy": {
            "path_mode": if full { "full" } else { "redacted" },
            "messages_redacted": !full,
            "action_payload_redacted": !full && context.action.is_some(),
            "native_identifiers_redacted": !full && context.attribution.is_some(),
            "incident_labels_redacted": !full && (context.incident_ticket.is_some() || context.endpoint_label.is_some()),
            "native_path_caveat": "Display paths may be lossy or truncated; never use this report as a native-path action source.",
        },
        "root_display": root,
        "scan": {
            "generation": progress.generation,
            "state": if context.timed_out { "Timed out" } else { progress.state.label() },
            "backend": backend,
            "files": progress.files,
            "folders": progress.folders,
            "logical_bytes": progress.logical,
            "unique_allocated_bytes": progress.unique_allocated,
            "hardlink_aliases": progress.hardlinks,
            "skipped_reparse_entries": progress.skipped,
            "elapsed_ms": progress.elapsed.as_secs_f64() * 1000.0,
            "message": message,
        },
        "coverage": {
            "scan_complete": scan_complete,
            "timed_out": context.timed_out,
            "full_inventory_export": false,
            "row_scope": "current published view; may be filtered, paginated, or directory rows",
            "view_matches_scan_generation": view_matches_scan,
            "published_view_revision": view_matches_scan.then_some(snapshot.view.revision),
            "view_root_display": view_root,
            "filters_active": view_matches_scan.then_some(snapshot.view.filters_active),
            "rows_available": available_rows.len(),
            "rows_matching_view": view_matches_scan.then_some(snapshot.view.total_rows),
            "rows_exported": rows.len(),
            "rows_truncated": !view_matches_scan || available_rows.len() > rows.len() || snapshot.view.total_rows > rows.len(),
            "errors_total": progress.errors_total,
            "errors_retained": progress.errors.len(),
            "errors_exported": errors.len(),
            "errors_truncated": progress.errors.len() > errors.len() || progress.errors_total > errors.len() as u64,
            "text_truncated": text_truncated,
            "action_omitted_due_to_size": action_too_large,
        },
        "limits": { "rows": MAX_REPORT_ROWS, "errors": MAX_REPORT_ERRORS, "text_bytes_per_field": MAX_TEXT_BYTES, "action_json_bytes": MAX_ACTION_BYTES, "incident_label_bytes": MAX_INCIDENT_BYTES, "identity_field_bytes": crate::identity::MAX_IDENTITY_BYTES },
        "accounting": {
            "logical": "Counts file entries; hardlink names can refer to the same data.",
            "unique_allocation": "Deduplicates observed file identities; unknown allocation remains null.",
            "excluded": "Alternate data streams and filesystem metadata; reparse targets are not traversed.",
            "live_scan": "A live scan is not a filesystem snapshot; files may change during enumeration.",
            "action_outcomes": "An action request, recycling outcome, and volume free-space change are distinct. No reclaimed-space attribution is made.",
        },
        "errors": errors,
        "rows": rows,
        "requested_view": requested_view,
        "resources": resource_report(context),
        "action_record_present": context.action.is_some(),
        "action": action,
    })
}

fn resource_report(context: &EvidenceContext) -> Value {
    let mut refusal_truncated = false;
    let admission_refusal = context
        .resource_refusal_reason
        .as_ref()
        .map(|reason| bounded(reason, &mut refusal_truncated));
    let observation = context.resource_observation.as_ref();
    let measured = observation.and_then(|o| o.measured.as_ref().ok());
    let reason = observation
        .and_then(|o| o.measured.as_ref().err())
        .map(|error| match error {
            crate::resources::ProbeError::Unavailable(reason) => *reason,
        });
    json!({
        "status": match observation {
            None => "not_observed",
            Some(_) if reason.is_some() => "unavailable",
            Some(o) if o.age_ms > crate::resources::MAX_OBSERVATION_AGE.as_millis() as u64 => "stale",
            _ => "available",
        },
        "private_committed_bytes": measured.map(|m| m.private_bytes),
        "system_available_physical_bytes": measured.map(|m| m.system_available_bytes),
        "observed_utc_epoch_ms": measured.and_then(|m| m.observed_utc_epoch_ms),
        "source": measured.map(|m| m.source),
        "observation_age_ms": observation.map(|o| o.age_ms),
        "unavailable_reason": reason,
        "admission_limits": context.admission_limits.map(|l| json!({ "max_nodes": l.max_nodes, "max_estimated_inventory_bytes": l.max_estimated_bytes })),
        "memory_budget": context.memory_budget,
        "observed_headroom_refusal": context.memory_budget.zip(observation).and_then(|(budget, sample)| budget.refusal(sample)),
        "admission_refusal_reason": admission_refusal,
        "admission_refusal_truncated": refusal_truncated,
        "admission_refusal_scope": "Recorded current-scan admission stop; retained separately from later sampler observations.",
        "headroom_assessment_scope": "Assessment of supplied sample/budget at caller retrieval; not proof that the scan was refused for this reason.",
        "probe_interval_ms": crate::resources::PROBE_INTERVAL.as_millis() as u64,
        "max_observation_age_ms": crate::resources::MAX_OBSERVATION_AGE.as_millis() as u64,
        "scope": "Cached caller observation, not peak memory. Admission checks are sampled tripwires, not a hard RAM limit or whole-drive scalability certification. Estimated inventory bytes exclude other application allocations.",
    })
}

fn incident_label(value: Option<&str>, full: bool, truncated: &mut bool) -> Value {
    let mut field_truncated = false;
    let exported = value
        .filter(|_| full)
        .map(|text| bounded_to(text, MAX_INCIDENT_BYTES, &mut field_truncated));
    *truncated |= field_truncated;
    json!({
        "value": exported,
        "status": match (value, full) { (None, _) => "not_provided", (Some(_), false) => "redacted", _ => "available" },
        "truncated": field_truncated,
    })
}

fn attribution_report(
    attribution: Option<&crate::identity::Attribution>,
    full: bool,
    truncated: &mut bool,
) -> Value {
    let observation = |item: Option<&crate::identity::Observation>, truncated: &mut bool| {
        let mut field_truncated = false;
        let value = item
            .filter(|_| full)
            .and_then(|item| item.value.as_deref())
            .map(|text| {
                bounded_to(
                    text,
                    crate::identity::MAX_IDENTITY_BYTES,
                    &mut field_truncated,
                )
            });
        field_truncated |= item.is_some_and(|item| full && item.truncated);
        *truncated |= field_truncated;
        json!({
            "value": value,
            "status": match item { None => "not_collected", Some(_) if !full => "redacted", Some(item) if item.value.is_some() => "available", _ => "unavailable" },
            "source": item.map(|item| item.source),
            "observed_utc_epoch_ms": item.and_then(|item| item.observed_utc_epoch_ms),
            "unavailable_reason": if full { item.and_then(|item| item.unavailable_reason) } else { None },
            "win32_error": if full { item.and_then(|item| item.win32_error) } else { None },
            "truncated": field_truncated,
        })
    };
    json!({
        "hostname": observation(attribution.map(|a| &a.hostname), truncated),
        "volume_guid": observation(attribution.map(|a| &a.volume_guid), truncated),
        "volume_serial": observation(attribution.map(|a| &a.volume_serial), truncated),
        "filesystem": observation(attribution.map(|a| &a.filesystem), truncated),
        "started_utc_epoch_ms": attribution.and_then(|a| a.started_utc_epoch_ms),
        "finished_utc_epoch_ms": attribution.and_then(|a| a.finished_utc_epoch_ms),
        "elapsed_ms": attribution.map(|a| a.elapsed_ms),
        "scope": "Point-in-time display context from the supplied root's volume mount. Hostnames, volume GUIDs and serials may change or be cloned; provider behavior varies. Not authenticated endpoint identity; never an action target.",
        "permanent_machine_identifier": "not collected",
    })
}

fn bounded(text: &str, truncated: &mut bool) -> String {
    bounded_to(text, MAX_TEXT_BYTES, truncated)
}

fn bounded_to(text: &str, maximum: usize, truncated: &mut bool) -> String {
    let mut end = text.len().min(maximum);
    if end < text.len() {
        *truncated = true;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
    }
    text[..end].to_owned()
}

fn private_text(text: &str, full: bool, truncated: &mut bool) -> String {
    if full {
        bounded(text, truncated)
    } else {
        "[redacted]".into()
    }
}

fn path_display(path: &Path, full: bool, truncated: &mut bool) -> String {
    private_text(&path.to_string_lossy(), full, truncated)
}

struct SizeLimit(usize);
impl Write for SizeLimit {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| io::Error::other("action record exceeds export limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn executable_digest() -> &'static Result<String, String> {
    static DIGEST: OnceLock<Result<String, String>> = OnceLock::new();
    DIGEST.get_or_init(|| {
        let result = std::env::current_exe()
            .and_then(std::fs::File::open)
            .and_then(hash_reader);
        result.map_err(|_| "Unable to read running executable".into())
    })
}

fn hash_reader(mut reader: impl Read) -> io::Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = match reader.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Create a new evidence file. Existing files (including links) are never opened
/// for writing. An I/O failure can leave a partial new file, so callers must
/// surface errors and must not announce an export until this returns success.
pub fn write_report_new(report: &Value, destination: &Path) -> io::Result<()> {
    let encoded = serde_json::to_vec_pretty(report).map_err(io::Error::other)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    file.write_all(&encoded)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Progress, Row, ScanError, ScanState, ViewSnapshot};
    use serde_json::json;

    fn fixture() -> UiSnapshot {
        UiSnapshot {
            progress: Progress {
                generation: 1,
                state: ScanState::Complete,
                files: 400,
                errors_total: 2,
                message: "secret-customer failed".into(),
                errors: vec![ScanError {
                    path: "secret-customer/denied".into(),
                    message: "Cannot open secret-customer".into(),
                }],
                ..Default::default()
            },
            view: ViewSnapshot {
                generation: 1,
                total_rows: 400,
                rows: vec![Row {
                    id: 1,
                    parent: Some(0),
                    depth: 0,
                    name: "secret-customer".into(),
                    path: "secret-customer/file".into(),
                    is_dir: false,
                    reparse: false,
                    identity: None,
                    hard_link: false,
                    logical: 12,
                    allocated: Some(16),
                    files: 0,
                    folders: 0,
                    modified: None,
                    has_children: false,
                }],
                ..Default::default()
            },
        }
    }

    #[test]
    fn absent_attribution_is_explicit_without_changing_schema() {
        let report = build_report(&fixture(), &EvidenceContext::new("root".into()));
        assert_eq!(report["schema_version"], 1);
        assert_eq!(
            report["context"]["attribution"]["hostname"]["status"],
            "not_collected"
        );
        assert_eq!(
            report["context"]["attribution"]["hostname"]["value"],
            Value::Null
        );
        assert_eq!(
            report["context"]["incident"]["ticket"]["status"],
            "not_provided"
        );
    }

    #[test]
    fn absent_resource_context_is_unknown_instead_of_fabricated_measurement() {
        let report = build_report(&fixture(), &EvidenceContext::new("root".into()));
        assert_eq!(report["resources"]["status"], "not_observed");
        assert_eq!(report["resources"]["private_committed_bytes"], Value::Null);
        assert_eq!(report["resources"]["admission_limits"], Value::Null);
    }

    #[test]
    fn recorded_resource_stop_survives_a_later_healthy_sample_in_redacted_evidence() {
        let mut context = EvidenceContext::new("private-root".into());
        context.memory_budget = Some(crate::resources::MemoryBudget::helpdesk());
        context.resource_observation = Some(crate::resources::ResourceObservation {
            measured: Ok(crate::resources::MemoryObservation {
                private_bytes: 1_048_576,
                system_available_bytes: 8_589_934_592,
                observed_utc_epoch_ms: Some(7000),
                source: "controlled boundary",
            }),
            age_ms: 0,
        });
        context.resource_refusal_reason =
            Some("Memory pressure: system available bytes 40 reached minimum 50".into());
        let report = build_report(&fixture(), &context);
        assert_eq!(
            report["resources"]["admission_refusal_reason"],
            "Memory pressure: system available bytes 40 reached minimum 50"
        );
        assert_eq!(
            report["resources"]["observed_headroom_refusal"],
            Value::Null
        );
        assert!(!report.to_string().contains("private-root"));
    }

    #[test]
    fn resource_evidence_preserves_measured_bytes_age_limits_and_failure() {
        use crate::resources::{MemoryObservation, ProbeError, ResourceObservation};
        let mut context = EvidenceContext::new("root".into());
        context.resource_observation = Some(ResourceObservation {
            measured: Ok(MemoryObservation {
                private_bytes: 12345,
                system_available_bytes: 98765,
                observed_utc_epoch_ms: Some(5000),
                source: "native fixture boundary",
            }),
            age_ms: 6000,
        });
        context.admission_limits = Some(crate::engine::ScanLimits {
            max_nodes: 17,
            max_estimated_bytes: 1234,
        });
        context.memory_budget = Some(crate::resources::MemoryBudget::helpdesk());
        let report = build_report(&fixture(), &context);
        assert_eq!(report["resources"]["status"], "stale");
        assert_eq!(report["resources"]["private_committed_bytes"], 12345);
        assert_eq!(
            report["resources"]["system_available_physical_bytes"],
            98765
        );
        assert_eq!(report["resources"]["observation_age_ms"], 6000);
        assert!(
            report["resources"]["observed_headroom_refusal"]
                .as_str()
                .is_some()
        );
        assert_eq!(report["resources"]["admission_limits"]["max_nodes"], 17);
        context.resource_observation.as_mut().unwrap().measured =
            Err(ProbeError::Unavailable("probe failed"));
        let report = build_report(&fixture(), &context);
        assert_eq!(report["resources"]["status"], "unavailable");
        assert_eq!(report["resources"]["private_committed_bytes"], Value::Null);
        assert_eq!(report["resources"]["unavailable_reason"], "probe failed");
    }

    #[test]
    fn incident_and_native_identity_are_private_even_when_scan_paths_are_empty() {
        let mut context = EvidenceContext::new("root".into());
        context.incident_ticket = Some("private-ticket".into());
        context.endpoint_label = Some("private-endpoint".into());
        let mut found = crate::identity::collect(Path::new("invalid\0root"));
        found.hostname.value = Some("private-hostname".into());
        found.volume_guid.value = Some("private-volume".into());
        context.attribution = Some(found);
        let report = build_report(&fixture(), &context);
        assert!(!report.to_string().contains("private-"));
        assert_eq!(
            report["context"]["attribution"]["hostname"]["status"],
            "redacted"
        );
        assert_eq!(
            report["context"]["attribution"]["hostname"]["value"],
            Value::Null
        );
        assert_eq!(
            report["context"]["incident"]["ticket"]["status"],
            "redacted"
        );
        assert_eq!(
            report["context"]["incident"]["endpoint_label"]["value"],
            Value::Null
        );
    }

    #[test]
    fn full_identity_export_bounds_utf8_labels_and_preserves_failure_reason() {
        let mut context = EvidenceContext::new("root".into());
        context.redaction = Redaction::Full;
        context.incident_ticket = Some("é".repeat(1000));
        context.endpoint_label = Some("customer-label".into());
        context.attribution = Some(crate::identity::collect(Path::new("bad\0path")));
        let report = build_report(&fixture(), &context);
        assert!(
            report["context"]["incident"]["ticket"]["value"]
                .as_str()
                .unwrap()
                .len()
                <= 512
        );
        assert_eq!(report["context"]["incident"]["ticket"]["truncated"], true);
        assert_eq!(report["coverage"]["text_truncated"], true);
        assert_eq!(
            report["context"]["incident"]["endpoint_label"]["value"],
            "customer-label"
        );
        assert_eq!(
            report["context"]["attribution"]["volume_guid"]["status"],
            "unavailable"
        );
        assert!(report["context"]["attribution"]["volume_guid"]["unavailable_reason"].is_string());
        assert_eq!(
            report["context"]["attribution"]["volume_guid"]["value"],
            Value::Null
        );
    }

    #[test]
    fn redaction_covers_paths_messages_and_opaque_action_data() {
        let mut context = EvidenceContext::new("secret-customer/root".into());
        context.action = Some(json!({"secret-customer-key": {"path":"secret-customer/file"}}));
        let report = build_report(&fixture(), &context);
        assert!(!report.to_string().contains("secret-customer"));
        assert_eq!(report["privacy"]["path_mode"], "redacted");
        assert_eq!(report["action_record_present"], true);
        assert_eq!(report["action"], json!({"redacted":true}));
        assert_eq!(report["rows"][0]["logical_bytes"], 12);
    }

    #[test]
    fn full_mode_preserves_display_paths_and_marks_incomplete_error_and_row_lists() {
        let mut context = EvidenceContext::new("secret-customer/root".into());
        context.redaction = Redaction::Full;
        context.generated_utc_epoch_ms = Some(1234);
        context.started_utc_epoch_ms = Some(1000);
        context.finished_utc_epoch_ms = Some(1200);
        context.access_context = "standard".into();
        context.session_context = "helpdesk".into();
        let report = build_report(&fixture(), &context);
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["root_display"], "secret-customer/root");
        assert_eq!(report["timestamps"]["generated_utc_epoch_ms"], 1234);
        assert_eq!(report["context"]["access"], "standard");
        assert_eq!(report["context"]["session"], "helpdesk");
        assert_eq!(report["coverage"]["scan_complete"], false);
        assert_eq!(report["coverage"]["full_inventory_export"], false);
        assert_eq!(report["coverage"]["errors_truncated"], true);
        assert_eq!(report["coverage"]["rows_truncated"], true);
        assert_eq!(
            report["errors"][0]["path_display"],
            "secret-customer/denied"
        );
    }

    #[test]
    fn report_bounds_rows_errors_text_and_actions_independently() {
        let mut snapshot = fixture();
        snapshot.progress.errors = vec![snapshot.progress.errors[0].clone(); 1001];
        snapshot.progress.errors_total = 1001;
        snapshot.view.rows = vec![snapshot.view.rows[0].clone(); 101];
        snapshot.progress.message = "x".repeat(5000);
        let mut context = EvidenceContext::new("r".into());
        context.redaction = Redaction::Full;
        context.action = Some(json!({"details":"x".repeat(70_000)}));
        let report = build_report(&snapshot, &context);
        assert_eq!(report["errors"].as_array().unwrap().len(), 1000);
        assert_eq!(report["rows"].as_array().unwrap().len(), 100);
        assert!(report["scan"]["message"].as_str().unwrap().len() <= 4096);
        assert_eq!(report["coverage"]["text_truncated"], true);
        assert_eq!(report["coverage"]["action_omitted_due_to_size"], true);
        assert_eq!(report["action"], Value::Null);
    }

    #[test]
    fn timeout_overrides_an_already_complete_snapshot() {
        let mut snapshot = fixture();
        snapshot.progress.errors_total = 0;
        let mut context = EvidenceContext::new("r".into());
        context.timed_out = true;
        let report = build_report(&snapshot, &context);
        assert_eq!(report["coverage"]["scan_complete"], false);
        assert_eq!(report["scan"]["state"], "Timed out");
    }

    #[test]
    fn a_previous_generations_view_cannot_leak_into_current_scan_evidence() {
        let mut snapshot = fixture();
        snapshot.view.generation = 0;
        snapshot.view.root_path = "previous-customer/root".into();
        snapshot.view.rows[0].path = "previous-customer/file".into();
        let mut context = EvidenceContext::new("current-root".into());
        context.redaction = Redaction::Full;
        let report = build_report(&snapshot, &context);
        assert!(!report.to_string().contains("previous-customer"));
        assert_eq!(report["rows"], json!([]));
        assert_eq!(report["coverage"]["view_matches_scan_generation"], false);
    }

    #[test]
    fn requested_view_context_is_explicit_and_redacts_search_text() {
        let mut context = EvidenceContext::new("root".into());
        context.view_request = Some(ViewRequest {
            offset: 17,
            limit: 23,
            search: "secret-customer".into(),
            ..Default::default()
        });
        let report = build_report(&fixture(), &context);
        assert_eq!(report["requested_view"]["offset"], 17);
        assert_eq!(report["requested_view"]["limit"], 23);
        assert_eq!(report["requested_view"]["search"], "[redacted]");
        assert!(!report.to_string().contains("secret-customer"));
    }

    #[test]
    fn streaming_executable_hash_includes_every_chunk() {
        assert_eq!(
            hash_reader(io::repeat(b'a').take(1_000_000)).unwrap(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn text_limit_preserves_unicode_boundaries_and_marks_loss() {
        let mut context = EvidenceContext::new(PathBuf::from("é".repeat(3000)));
        context.redaction = Redaction::Full;
        let report = build_report(&fixture(), &context);
        assert_eq!(report["root_display"].as_str().unwrap().len(), 4096);
        assert_eq!(report["coverage"]["text_truncated"], true);
    }

    #[cfg(windows)]
    #[test]
    fn non_unicode_native_paths_are_display_only_with_an_explicit_caveat() {
        use std::{ffi::OsString, os::windows::ffi::OsStringExt};
        let mut context = EvidenceContext::new(PathBuf::from(OsString::from_wide(&[
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xd800,
        ])));
        context.redaction = Redaction::Full;
        let report = build_report(&fixture(), &context);
        assert_eq!(report["root_display"], "C:\\�");
        assert!(
            report["privacy"]["native_path_caveat"]
                .as_str()
                .unwrap()
                .contains("lossy")
        );
    }
}
