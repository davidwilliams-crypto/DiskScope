//! Checked, execution-free NTFS metadata decoding for the optional volume backend.
//!
//! The record decoder and the model reconstruction are portable and unit-tested on every platform;
//! only the Windows volume reader (`ntfs_windows.rs`) feeds them real data. They are private to
//! this module on purpose: what the decoder returns depends on the platform (see below), so no
//! other module may build on it. The public surface is [`scan_volume`], [`NtfsProgress`],
//! [`NtfsError`] and the drive-root predicate shared with the engine.
//!
//! # Platform difference in decoded names
//!
//! NTFS file names are arbitrary UTF-16, including unpaired surrogates. On Windows an `OsString`
//! can hold them, so `os_string_from_units` keeps every unit. Elsewhere an `OsString` cannot,
//! so those units become U+FFFD. Off Windows the decoder only runs in unit tests, so the lossy
//! path never reaches a user; a test that compares names must not assume lossless decoding there.

// Off Windows nothing but the unit tests reaches the decoder, so rustc sees most of it as unused.
// The Windows target is linted too, and there genuinely dead decoder code is still reported.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::model::{Entry, FileIdentity, Model};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::ops::Range;
use std::path::Path;

/// Low 48 bits of an MFT reference: the record index without its sequence.
const INDEX_MASK: u64 = 0x0000_ffff_ffff_ffff;
/// MFT record index of the root directory.
const ROOT_INDEX: u64 = 5;
/// Records below this index are NTFS system files (`$MFT`, `$LogFile`, `$Extend`, ...).
const FIRST_USER_INDEX: u64 = 16;

const RECORD_IN_USE: u16 = 0x0001;
const RECORD_IS_DIRECTORY: u16 = 0x0002;

const ATTR_STANDARD_INFORMATION: u32 = 0x10;
const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_REPARSE_POINT: u32 = 0xc0;
const ATTR_END: u32 = u32::MAX;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// `$FILE_NAME` namespace of a DOS 8.3 alias; the long name is always stored separately.
const NAMESPACE_DOS: u8 = 2;
const NAMESPACE_MAX: u8 = 3;
/// Compression-unit, compressed and sparse bits of a nonresident attribute's flags.
const DATA_FLAGS_COMPRESSED_OR_SPARSE: u16 = 0x80ff;

const FILETIME_TICKS_PER_SECOND: u64 = 10_000_000;
/// 1601-01-01 to 1970-01-01 in FILETIME ticks.
const FILETIME_UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;

/// Upper bound on decoded mapping-pair runs per attribute, so a corrupt run list cannot
/// exhaust memory.
const MAX_DATA_RUNS: usize = 1_000_000;
/// Records between cancellation checks while decoding.
const CANCEL_CHECK_RECORDS: usize = 256;
/// Directory links handled per model-building step; keeps cancellation responsive.
const LINK_CHUNK: usize = 512;

#[derive(Debug, Clone, Copy)]
pub struct NtfsProgress {
    pub records: u64,
    pub total_records: u64,
}
#[cfg(windows)]
#[path = "ntfs_windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::scan_volume;
#[cfg(not(windows))]
pub fn scan_volume(
    _root: &Path,
    _cancelled: impl Fn() -> bool,
    _progress: impl FnMut(NtfsProgress),
) -> Result<Model> {
    Err(err("The NTFS volume backend requires Windows"))
}

/// The drive letter of `root` when it is exactly a whole local drive root such as `C:\`.
///
/// This is the one definition of "whole-drive root" for the NTFS backend. The engine uses it to
/// choose the backend and to explain a fallback; [`scan_volume`] uses it to open the volume. The
/// two therefore cannot disagree about which paths qualify. ASCII is matched on the encoded bytes,
/// which `OsStr::as_encoded_bytes` documents as a superset of 7-bit ASCII on every platform.
pub(crate) fn drive_root_letter(root: &Path) -> Option<u8> {
    match root.as_os_str().as_encoded_bytes() {
        [letter, b':', b'\\'] if letter.is_ascii_alphabetic() => Some(*letter),
        _ => None,
    }
}

/// True for the NTFS system files: records 0-15 other than the root directory. They anchor
/// subtrees (`$Extend`, ...) that are excluded from user totals.
fn is_metadata_index(index: u64) -> bool {
    index < FIRST_USER_INDEX && index != ROOT_INDEX
}

/// True when the record has at least one name and every name sits directly under a system file,
/// as `$Extend\$UsnJrnl` does. Such records are excluded from user totals.
fn anchored_under_metadata(record: &Record) -> bool {
    !record.names.is_empty()
        && record
            .names
            .iter()
            .all(|link| is_metadata_index(link.parent & INDEX_MASK))
}

/// True for records that never contribute to user totals.
fn is_excluded_metadata(record: &Record) -> bool {
    is_metadata_index(record.reference & INDEX_MASK) || anchored_under_metadata(record)
}

fn check_cancelled(cancelled: &impl Fn() -> bool) -> Result<()> {
    if cancelled() {
        Err(err("NTFS scan cancelled"))
    } else {
        Ok(())
    }
}

/// Splits decoded records into base records (keyed by reference) and extension records, and
/// rejects two records that claim the same MFT index.
fn partition_records(
    records: Vec<Record>,
    cancelled: &impl Fn() -> bool,
) -> Result<(HashMap<u64, Record>, Vec<Record>)> {
    let mut bases = HashMap::new();
    let mut extensions = Vec::new();
    let mut indexes = HashSet::new();
    for record in records {
        check_cancelled(cancelled)?;
        if !indexes.insert(record.reference & INDEX_MASK) {
            return Err(err("Duplicate MFT record index"));
        }
        if record.base == 0 {
            bases.insert(record.reference, record);
        } else {
            extensions.push(record);
        }
    }
    Ok((bases, extensions))
}

/// Cross-checks every readable ATTRIBUTE_LIST against the records it names.
///
/// Only a base record's own attributes and its extension records can be listed, so lookups search
/// the base map and a small index of the (rare) extensions; no map of all records is built. A
/// base whose list is nonresident cannot be cross-checked at all; whether that is acceptable is
/// decided by [`reject_unread_attribute_lists_on_user_records`].
fn validate_attribute_lists(
    bases: &HashMap<u64, Record>,
    extensions: &[Record],
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    let extension_slot: HashMap<u64, usize> = extensions
        .iter()
        .enumerate()
        .map(|(slot, extension)| (extension.reference, slot))
        .collect();
    let find = |reference: u64| {
        bases.get(&reference).or_else(|| {
            extension_slot
                .get(&reference)
                .map(|&slot| &extensions[slot])
        })
    };
    for record in bases.values().chain(extensions) {
        check_cancelled(cancelled)?;
        if record.base != 0
            && (!record.attribute_list.is_empty() || record.attribute_list_unresolved)
        {
            return Err(err("Extension contains an ATTRIBUTE_LIST"));
        }
        if record.attribute_list_unresolved {
            continue;
        }
        for required in &record.attribute_list {
            let target = find(required.reference)
                .ok_or_else(|| err("Missing or stale ATTRIBUTE_LIST reference"))?;
            if (target.reference != record.reference && target.base != record.reference)
                || !target.attributes.contains(&required.key)
            {
                return Err(err("ATTRIBUTE_LIST target does not match"));
            }
        }
        if record.base != 0 {
            let base = bases
                .get(&record.base)
                .ok_or_else(|| err("Missing or stale extension base reference"))?;
            // An unread list cannot say which extension attributes it names.
            if base.attribute_list_unresolved {
                continue;
            }
            if record.attributes.iter().any(|key| {
                !base
                    .attribute_list
                    .iter()
                    .any(|r| r.reference == record.reference && r.key == *key)
            }) {
                return Err(err("Unlisted extension attribute"));
            }
        }
    }
    Ok(())
}

/// Folds each extension record's names, timestamps and unnamed DATA extents into its base.
fn merge_extensions(
    bases: &mut HashMap<u64, Record>,
    extensions: Vec<Record>,
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    for extension in extensions {
        check_cancelled(cancelled)?;
        let base = bases
            .get_mut(&extension.base)
            .ok_or_else(|| err("Missing or stale extension base reference"))?;
        if base.is_dir != extension.is_dir {
            return Err(err("Extension directory flag disagrees with base"));
        }
        if (base.logical.is_some() && !base.nonresident && extension.nonresident)
            || (extension.logical.is_some() && !extension.nonresident && base.nonresident)
        {
            return Err(err("Mixed resident and nonresident unnamed DATA"));
        }
        if let Some(logical) = extension.logical {
            if base.logical.is_some() {
                return Err(err("Duplicate unnamed DATA head"));
            }
            base.logical = Some(logical);
            base.allocated = extension.allocated;
        }
        for name in extension.names {
            if !base.names.contains(&name) {
                base.names.push(name)
            }
        }
        base.reparse |= extension.reparse;
        base.modified = base.modified.or(extension.modified);
        base.nonresident |= extension.nonresident;
        base.data_runs.extend(extension.data_runs);
    }
    Ok(())
}

/// Rejects a user file or folder whose ATTRIBUTE_LIST is nonresident.
///
/// A nonresident list lives in clusters this decoder never reads, so nothing proves that every
/// attribute of the record (names, data streams) was seen; totals could be silently wrong. System
/// metadata such as `$Extend\$UsnJrnl` commonly has one and is excluded from user totals anyway, so
/// it is accepted. Run after [`merge_extensions`] so names contributed by extension records count
/// when deciding whether a record is metadata. Reading the list clusters would lift this limit,
/// but is deliberately not attempted without a way to test it.
fn reject_unread_attribute_lists_on_user_records(
    bases: &HashMap<u64, Record>,
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    for record in bases.values() {
        check_cancelled(cancelled)?;
        if record.attribute_list_unresolved && !is_excluded_metadata(record) {
            return Err(err(&format!(
                "MFT record {}: a user file or folder has a nonresident ATTRIBUTE_LIST, \
                 which the NTFS preview does not read",
                record.reference & INDEX_MASK
            )));
        }
    }
    Ok(())
}

/// Checks that each nonresident unnamed DATA stream's extents are contiguous and agree with its
/// recorded allocation.
fn verify_unnamed_data(
    bases: &mut HashMap<u64, Record>,
    cluster: u64,
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    for record in bases.values_mut() {
        check_cancelled(cancelled)?;
        if !record.nonresident {
            continue;
        }
        record.data_runs.sort_unstable_by_key(|r| r.vcn);
        let (mut next, mut physical) = (0u64, 0u64);
        for run in &record.data_runs {
            if run.vcn != next {
                return Err(err("Gap or overlap in unnamed DATA extents"));
            }
            next = next
                .checked_add(run.clusters)
                .ok_or_else(|| err("DATA cluster count overflow"))?;
            if run.lcn.is_some() {
                physical = physical
                    .checked_add(run.clusters)
                    .ok_or_else(|| err("DATA physical count overflow"))?;
            }
        }
        let logical = record
            .logical
            .ok_or_else(|| err("Missing unnamed DATA head"))?;
        if next.checked_mul(cluster).is_none_or(|n| n < logical)
            || physical.checked_mul(cluster) != record.allocated
        {
            return Err(err("Unnamed DATA allocation disagrees with mapping pairs"));
        }
    }
    Ok(())
}

/// Directory links of the volume, borrowed from the merged records.
struct Links<'a> {
    /// Parent reference to its `(child reference, name)` links.
    children: HashMap<u64, Vec<(u64, &'a OsStr)>>,
    /// Parent references of system files; their subtrees are walked for validation only.
    excluded_roots: HashSet<u64>,
}

/// Validates every user record's names and parents and groups the links by parent.
///
/// Names are borrowed rather than cloned: this runs for every link on the volume, and the model
/// builder is the only place that needs an owned copy.
fn collect_links<'a>(
    bases: &'a HashMap<u64, Record>,
    cancelled: &impl Fn() -> bool,
) -> Result<Links<'a>> {
    let mut children: HashMap<u64, Vec<(u64, &OsStr)>> = HashMap::new();
    let mut sibling_names: HashSet<(u64, &OsStr)> = HashSet::new();
    let mut excluded_roots = HashSet::new();
    for record in bases.values() {
        check_cancelled(cancelled)?;
        if (record.reference & INDEX_MASK) < FIRST_USER_INDEX {
            continue;
        }
        if record.names.is_empty() || (record.is_dir && record.names.len() != 1) {
            return Err(err("Missing or ambiguous FILE_NAME links"));
        }
        // Files living only under metadata directories ($Extend\$Quota,
        // $ObjId, $Reparse, $UsnJrnl) legitimately have no unnamed DATA
        // stream. They are excluded from user totals, so their accounting is
        // not required; demanding it would reject every standard volume.
        if !record.is_dir
            && !anchored_under_metadata(record)
            && (record.logical.is_none() || record.allocated.is_none())
        {
            return Err(err("Missing unnamed DATA accounting"));
        }
        for link in &record.names {
            if is_metadata_index(link.parent & INDEX_MASK) {
                excluded_roots.insert(link.parent);
            } else if bases.get(&link.parent).is_none_or(|p| !p.is_dir) {
                return Err(err("Missing, stale or non-directory parent reference"));
            }
            let name = link.name.as_os_str();
            if !sibling_names.insert((link.parent, name)) {
                return Err(err("Duplicate directory entry name"));
            }
            children
                .entry(link.parent)
                .or_default()
                .push((record.reference, name));
        }
    }
    Ok(Links {
        children,
        excluded_roots,
    })
}

/// Builds a [`Model`] from decoded MFT records. The decoder and this
/// reconstruction are portable and unit-tested everywhere; only the Windows
/// volume reader produces records outside tests.
fn reconstruct(
    root: &Path,
    volume: u64,
    cluster: u64,
    records: Vec<Record>,
    cancelled: impl Fn() -> bool,
) -> Result<Model> {
    let (mut bases, extensions) = partition_records(records, &cancelled)?;
    validate_attribute_lists(&bases, &extensions, &cancelled)?;
    merge_extensions(&mut bases, extensions, &cancelled)?;
    reject_unread_attribute_lists_on_user_records(&bases, &cancelled)?;
    verify_unnamed_data(&mut bases, cluster, &cancelled)?;
    let root_ref = bases
        .keys()
        .copied()
        .find(|r| r & INDEX_MASK == ROOT_INDEX)
        .ok_or_else(|| err("Missing NTFS root record"))?;
    if !bases[&root_ref].is_dir {
        return Err(err("NTFS root is not a directory"));
    }
    let Links {
        mut children,
        excluded_roots,
    } = collect_links(&bases, &cancelled)?;
    let mut model = Model::new(root.to_owned());
    let mut queue = VecDeque::from([(root_ref, Some(0))]);
    queue.extend(excluded_roots.into_iter().map(|r| (r, None)));
    let mut visited_dirs = HashSet::new();
    while let Some((parent, node)) = queue.pop_front() {
        check_cancelled(&cancelled)?;
        if !visited_dirs.insert(parent) {
            return Err(err("Directory cycle or multiple directory links"));
        }
        let links = children.remove(&parent).unwrap_or_default();
        for chunk in links.chunks(LINK_CHUNK) {
            check_cancelled(&cancelled)?;
            let entries = if node.is_some() {
                chunk
                    .iter()
                    .map(|(reference, name)| {
                        let record = &bases[reference];
                        let mut id = [0; 16];
                        id[..8].copy_from_slice(&reference.to_le_bytes());
                        Entry {
                            name: (*name).to_os_string(),
                            is_dir: record.is_dir,
                            reparse: record.reparse,
                            logical: record.logical.unwrap_or(0),
                            allocated: record.allocated,
                            modified: record.modified,
                            identity: Some(FileIdentity { volume, id }),
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let ids = node
                .map(|n| model.insert_batch(n, entries))
                .unwrap_or_default();
            if node.is_some() && ids.len() != chunk.len() {
                return Err(err("Model node limit reached"));
            }
            for (offset, (reference, _)) in chunk.iter().enumerate() {
                let record = &bases[reference];
                if record.is_dir {
                    let child_node = if record.reparse {
                        if node.is_some() {
                            model.skipped = model
                                .skipped
                                .checked_add(1)
                                .ok_or_else(|| err("Skip counter overflow"))?;
                        }
                        None
                    } else {
                        ids.get(offset).copied()
                    };
                    queue.push_back((*reference, child_node));
                }
            }
        }
    }
    if !children.is_empty() {
        return Err(err("Unreachable or cyclic NTFS directory hierarchy"));
    }
    if model.overflowed {
        return Err(err("NTFS accounting overflow"));
    }
    Ok(model)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtfsError(pub String);
impl std::fmt::Display for NtfsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for NtfsError {}
type Result<T> = std::result::Result<T, NtfsError>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DataRun {
    vcn: u64,
    lcn: Option<u64>,
    clusters: u64,
}
/// One hard link of a file: its parent directory and its name there.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NameLink {
    parent: u64,
    /// Lossless on Windows, lossy elsewhere; see the module documentation.
    name: OsString,
}
/// Identity of one attribute within its record.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AttributeKey {
    kind: u32,
    id: u16,
    start_vcn: u64,
    name: Vec<u16>,
}
/// One entry of an ATTRIBUTE_LIST: where a listed attribute lives.
#[derive(Debug, Clone)]
struct AttributeReference {
    reference: u64,
    key: AttributeKey,
}
#[derive(Debug, Clone)]
struct Record {
    reference: u64,
    base: u64,
    is_dir: bool,
    reparse: bool,
    modified: Option<u64>,
    names: Vec<NameLink>,
    logical: Option<u64>,
    allocated: Option<u64>,
    /// Identities of this record's own attributes. Only collected for records that take part in
    /// an ATTRIBUTE_LIST (extension records, and base records carrying a readable list), because
    /// those are the only records whose attributes are ever looked up; empty for all others.
    attributes: HashSet<AttributeKey>,
    /// Entries of a resident ATTRIBUTE_LIST.
    attribute_list: Vec<AttributeReference>,
    /// The record has an ATTRIBUTE_LIST stored in clusters elsewhere on the volume. Its entries
    /// were not read, so the record's attribute set cannot be proven complete.
    attribute_list_unresolved: bool,
    nonresident: bool,
    data_runs: Vec<DataRun>,
}

fn err(message: &str) -> NtfsError {
    NtfsError(message.into())
}
fn range(b: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    b.get(at..at.checked_add(len).ok_or_else(|| err("Offset overflow"))?)
        .ok_or_else(|| err("Truncated NTFS metadata"))
}
fn array_at<const N: usize>(b: &[u8], at: usize) -> Result<[u8; N]> {
    range(b, at, N)?
        .try_into()
        .map_err(|_| err("Truncated NTFS metadata"))
}
fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(array_at(b, at)?))
}
fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(array_at(b, at)?))
}
fn u64_at(b: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(array_at(b, at)?))
}

/// Appends the UTF-16LE code units in `bytes` to `out`; a trailing odd byte is ignored.
fn extend_units(out: &mut Vec<u16>, bytes: &[u8]) {
    out.extend(
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair)),
    );
}

/// Converts file-name code units to the platform string type; see the module documentation for
/// the platform difference.
#[cfg(windows)]
fn os_string_from_units(units: &[u16]) -> OsString {
    use std::os::windows::ffi::OsStringExt;
    OsString::from_wide(units)
}
#[cfg(not(windows))]
fn os_string_from_units(units: &[u16]) -> OsString {
    OsString::from(String::from_utf16_lossy(units))
}

fn decode_runs(data: &[u8], start_vcn: u64) -> Result<Vec<DataRun>> {
    let (mut at, mut vcn, mut lcn) = (0usize, start_vcn, 0i64);
    let mut out = Vec::new();
    while let Some(&header) = data.get(at) {
        at += 1;
        if header == 0 {
            return Ok(out);
        }
        let length_bytes = (header & 15) as usize;
        let offset_bytes = (header >> 4) as usize;
        if length_bytes == 0 || length_bytes > 8 || offset_bytes > 8 {
            return Err(err("Invalid data-run widths"));
        }
        let mut length = [0u8; 8];
        length[..length_bytes].copy_from_slice(range(data, at, length_bytes)?);
        at += length_bytes;
        let clusters = u64::from_le_bytes(length);
        if clusters == 0 {
            return Err(err("Zero-length data run"));
        }
        let run_lcn = if offset_bytes == 0 {
            None
        } else {
            let encoded = range(data, at, offset_bytes)?;
            at += offset_bytes;
            let mut signed = [if encoded[offset_bytes - 1] & 0x80 != 0 {
                0xff
            } else {
                0
            }; 8];
            signed[..offset_bytes].copy_from_slice(encoded);
            lcn = lcn
                .checked_add(i64::from_le_bytes(signed))
                .ok_or_else(|| err("Data-run location overflow"))?;
            Some(u64::try_from(lcn).map_err(|_| err("Negative data-run location"))?)
        };
        out.push(DataRun {
            vcn,
            lcn: run_lcn,
            clusters,
        });
        vcn = vcn
            .checked_add(clusters)
            .ok_or_else(|| err("Data-run VCN overflow"))?;
        if out.len() > MAX_DATA_RUNS {
            return Err(err("Data-run budget exceeded"));
        }
    }
    Err(err("Missing data-run terminator"))
}

/// Validates the update-sequence array of one raw FILE record and, only when every sector
/// matches, restores in place the two bytes NTFS replaced at the end of each sector.
///
/// Nothing is written unless the whole record validates. Restoring is not idempotent (a second
/// call would compare the restored bytes with the sequence number and fail), so call it once per
/// raw record; the volume reader refills its buffer before the next use.
fn apply_fixups(bytes: &mut [u8], sector_size: usize) -> Result<()> {
    if !sector_size.is_power_of_two()
        || !(512..=65536).contains(&sector_size)
        || bytes.is_empty()
        || !bytes.len().is_multiple_of(sector_size)
    {
        return Err(err("Invalid NTFS sector/record size"));
    }
    let usa = u16_at(bytes, 4)? as usize;
    let count = u16_at(bytes, 6)? as usize;
    if usa < 8 || !usa.is_multiple_of(2) || count != bytes.len() / sector_size + 1 {
        return Err(err("Invalid update-sequence array"));
    }
    let array_len = count
        .checked_mul(2)
        .ok_or_else(|| err("Fixup array overflow"))?;
    let array = range(bytes, usa, array_len)?;
    let sequence = [array[0], array[1]];
    for sector in 1..count {
        let tail = sector * sector_size - 2;
        // The array must not overlap the sector ends it protects: restoring one end could then
        // rewrite an array entry that is read later.
        if tail < usa + array_len && usa < tail + 2 {
            return Err(err("Invalid update-sequence array"));
        }
        if range(bytes, tail, 2)? != sequence {
            return Err(err("NTFS sector fixup mismatch"));
        }
    }
    for sector in 1..count {
        let source = usa + sector * 2;
        bytes.copy_within(source..source + 2, sector * sector_size - 2);
    }
    Ok(())
}

/// Identity of an attribute inside the record being parsed. The name is a byte range of the
/// record itself, so duplicate detection allocates nothing.
struct SeenAttribute {
    kind: u32,
    id: u16,
    start_vcn: u64,
    name: Range<usize>,
}
impl SeenAttribute {
    /// The attribute name as stored in `record`. Capture proves the range lies
    /// inside the record, but reads still go through `get` so a future edit can
    /// never turn a stale range into an out-of-bounds panic.
    fn name<'a>(&self, record: &'a [u8]) -> Option<&'a [u8]> {
        record.get(self.name.clone())
    }
    fn same_identity(&self, other: &Self, record: &[u8]) -> Result<bool> {
        // Instance ids differ between nearly all attributes, so test the cheap fields first.
        let names = match (self.name(record), other.name(record)) {
            (Some(mine), Some(theirs)) => (mine, theirs),
            // A captured range outside the record is malformed state, not inequality.
            _ => return Err(err("Stale attribute name range")),
        };
        Ok(self.id == other.id
            && self.kind == other.kind
            && self.start_vcn == other.start_vcn
            && names.0 == names.1)
    }
    fn to_key(&self, record: &[u8]) -> Result<AttributeKey> {
        let mut name = Vec::new();
        extend_units(
            &mut name,
            self.name(record)
                .ok_or_else(|| err("Stale attribute name range"))?,
        );
        Ok(AttributeKey {
            kind: self.kind,
            id: self.id,
            start_vcn: self.start_vcn,
            name,
        })
    }
}

/// Working memory reused across records so the hot MFT loop allocates only for data it keeps.
#[derive(Default)]
struct ParseScratch {
    /// Attributes of the record being parsed, for duplicate detection.
    seen: Vec<SeenAttribute>,
    /// UTF-16 units of the `$FILE_NAME` being decoded.
    units: Vec<u16>,
}

/// Size of the MFT records and of the sectors whose ends they protect.
#[derive(Clone, Copy)]
struct MftLayout {
    record_size: usize,
    sector_size: usize,
}

/// Decodes every in-use FILE record of `chunk`, which holds whole consecutive MFT records
/// starting at record `first_index`, and appends them to `out`.
///
/// Fixups are applied in place, so `chunk` is consumed: the caller must refill it before reuse.
/// Records of the NTFS system files (indexes 0-15 other than the root), and extension records of
/// those files, are skipped unparsed: they are excluded from user totals and their private
/// layouts must not be able to fail the scan.
fn parse_mft_records(
    chunk: &mut [u8],
    first_index: u64,
    layout: MftLayout,
    scratch: &mut ParseScratch,
    cancelled: &impl Fn() -> bool,
    out: &mut Vec<Record>,
) -> Result<()> {
    if layout.record_size == 0 {
        return Err(err("Invalid NTFS sector/record size"));
    }
    for (n, raw) in chunk.chunks_exact_mut(layout.record_size).enumerate() {
        if n % CANCEL_CHECK_RECORDS == 0 {
            check_cancelled(cancelled)?;
        }
        let index = first_index + n as u64;
        if is_metadata_index(index) {
            continue;
        }
        let base = u64_at(raw, 32)?;
        if base != 0 && is_metadata_index(base & INDEX_MASK) {
            continue;
        }
        if let Some(record) = parse_record_in_place(raw, index, layout.sector_size, scratch)
            .map_err(|e| err(&format!("MFT record {index}: {e}")))?
        {
            out.push(record);
        }
    }
    Ok(())
}

/// Decodes one raw MFT record, applying its fixups in place (see [`apply_fixups`]). Returns
/// `None` for a zeroed or not-in-use record.
fn parse_record_in_place(
    bytes: &mut [u8],
    index: u64,
    sector_size: usize,
    scratch: &mut ParseScratch,
) -> Result<Option<Record>> {
    if bytes.len() < 48 || index > INDEX_MASK {
        return Err(err("Invalid FILE record header"));
    }
    if &bytes[..4] != b"FILE" {
        if bytes.iter().all(|&x| x == 0) {
            return Ok(None);
        }
        return Err(err("Invalid FILE record signature"));
    }
    let flags = u16_at(bytes, 22)?;
    if flags & RECORD_IN_USE == 0 {
        return Ok(None);
    }
    apply_fixups(bytes, sector_size)?;
    let b: &[u8] = bytes;
    let used = u32_at(b, 24)? as usize;
    let mut at = u16_at(b, 20)? as usize;
    if at < 48
        || !at.is_multiple_of(8)
        || used > b.len()
        || at.checked_add(4).is_none_or(|n| n > used)
    {
        return Err(err("Invalid FILE attribute bounds"));
    }
    let usa = u16_at(b, 4)? as usize;
    let count = u16_at(b, 6)? as usize;
    if usa < 48 || usa + count * 2 > at || u32_at(b, 28)? as usize != b.len() {
        return Err(err("Invalid FILE allocation or update-sequence bounds"));
    }
    let mut record = Record {
        reference: index | ((u16_at(b, 16)? as u64) << 48),
        base: u64_at(b, 32)?,
        is_dir: flags & RECORD_IS_DIRECTORY != 0,
        reparse: false,
        modified: None,
        names: Vec::new(),
        logical: None,
        allocated: None,
        attributes: HashSet::new(),
        attribute_list: Vec::new(),
        attribute_list_unresolved: false,
        nonresident: false,
        data_runs: Vec::new(),
    };
    scratch.seen.clear();
    let mut terminated = false;
    while at + 4 <= used {
        let kind = u32_at(b, at)?;
        if kind == ATTR_END {
            terminated = true;
            break;
        }
        let length = u32_at(b, at + 4)? as usize;
        if length < 24
            || !length.is_multiple_of(8)
            || at.checked_add(length).is_none_or(|n| n > used)
        {
            return Err(err("Invalid attribute length"));
        }
        let a = &b[at..at + length];
        let nonresident = a[8];
        if nonresident > 1 {
            return Err(err("Invalid attribute residency"));
        }
        let name = if a[9] != 0 {
            let name_offset = u16_at(a, 10)? as usize;
            if name_offset < if nonresident == 0 { 24 } else { 64 } {
                return Err(err("Invalid attribute name offset"));
            }
            // `range` proves the name lies inside the attribute, so the record range is in bounds.
            range(a, name_offset, a[9] as usize * 2)?;
            at + name_offset..at + name_offset + a[9] as usize * 2
        } else {
            0..0
        };
        let value = if nonresident == 0 {
            let start = u16_at(a, 20)? as usize;
            if start < 24 {
                return Err(err("Invalid resident value offset"));
            }
            Some(range(a, start, u32_at(a, 16)? as usize)?)
        } else {
            if length < 64 {
                return Err(err("Truncated nonresident attribute"));
            }
            None
        };
        let identity = SeenAttribute {
            kind,
            id: u16_at(a, 14)?,
            start_vcn: if nonresident == 0 { 0 } else { u64_at(a, 16)? },
            name,
        };
        let mut duplicate = false;
        for earlier in &scratch.seen {
            if earlier.same_identity(&identity, b)? {
                duplicate = true;
                break;
            }
        }
        if duplicate {
            return Err(err("Duplicate attribute identity"));
        }
        scratch.seen.push(identity);
        if nonresident != 0 && matches!(kind, ATTR_STANDARD_INFORMATION | ATTR_FILE_NAME) {
            return Err(err("Unsupported nonresident name or standard information"));
        }
        match kind {
            ATTR_ATTRIBUTE_LIST => match value {
                Some(v) => parse_attribute_list(v, &mut record.attribute_list)?,
                // The list is stored in clusters elsewhere on the volume, which this decoder
                // never reads. Remember that instead of failing the whole volume: reconstruction
                // decides whether the record is one the preview can do without.
                None => record.attribute_list_unresolved = true,
            },
            ATTR_STANDARD_INFORMATION => {
                if let Some(v) = value {
                    if v.len() < 36 {
                        return Err(err("Truncated standard information"));
                    }
                    record.modified = u64_at(v, 8)?
                        .checked_sub(FILETIME_UNIX_EPOCH_TICKS)
                        .map(|ticks| ticks / FILETIME_TICKS_PER_SECOND);
                    record.reparse |= u32_at(v, 32)? & FILE_ATTRIBUTE_REPARSE_POINT != 0;
                }
            }
            ATTR_FILE_NAME => {
                if let Some(v) = value {
                    parse_file_name(v, &mut record, &mut scratch.units)?;
                }
            }
            ATTR_DATA => {
                if a[9] == 0 {
                    parse_unnamed_data(a, value, &mut record)?;
                }
            }
            ATTR_REPARSE_POINT => record.reparse = true,
            _ => {}
        }
        at += length;
    }
    if !terminated {
        return Err(err("Missing FILE attribute terminator"));
    }
    // Only records that take part in an ATTRIBUTE_LIST are ever asked for their attribute keys,
    // so only they pay for owned copies. For a typical volume that is a tiny fraction of records.
    if record.base != 0 || !record.attribute_list.is_empty() {
        record.attributes = scratch
            .seen
            .iter()
            .map(|seen| seen.to_key(b))
            .collect::<Result<HashSet<_>>>()?;
    }
    Ok(Some(record))
}

/// Decodes the entries of a resident ATTRIBUTE_LIST value.
fn parse_attribute_list(value: &[u8], out: &mut Vec<AttributeReference>) -> Result<()> {
    let mut p = 0;
    while p < value.len() {
        let length = u16_at(value, p + 4)? as usize;
        if length < 26 || !length.is_multiple_of(8) {
            return Err(err("Invalid ATTRIBUTE_LIST entry length"));
        }
        let entry = range(value, p, length)?;
        let name_length = entry[6] as usize;
        let name_offset = entry[7] as usize;
        if name_length > 0 && name_offset < 26 {
            return Err(err("Invalid ATTRIBUTE_LIST name offset"));
        }
        let mut name = Vec::new();
        if name_length > 0 {
            extend_units(&mut name, range(entry, name_offset, name_length * 2)?);
        }
        out.push(AttributeReference {
            reference: u64_at(entry, 16)?,
            key: AttributeKey {
                kind: u32_at(entry, 0)?,
                id: u16_at(entry, 24)?,
                start_vcn: u64_at(entry, 8)?,
                name,
            },
        });
        p += length;
    }
    Ok(())
}

/// Decodes a resident `$FILE_NAME` value, adding its link unless it is a DOS alias or a duplicate.
/// `units` is scratch space for the decoded code units.
fn parse_file_name(value: &[u8], record: &mut Record, units: &mut Vec<u16>) -> Result<()> {
    if value.len() < 66 {
        return Err(err("Truncated file name"));
    }
    let namespace = value[65];
    if namespace > NAMESPACE_MAX {
        return Err(err("Invalid filename namespace"));
    }
    record.reparse |= u32_at(value, 56)? & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    // Validated even for DOS aliases so a truncated alias cannot hide corruption.
    let encoded = range(value, 66, value[64] as usize * 2)?;
    if namespace == NAMESPACE_DOS {
        return Ok(());
    }
    units.clear();
    extend_units(units, encoded);
    // NUL, '/', '\\' and ':' would let a name escape its parent directory.
    if units.iter().any(|&c| matches!(c, 0 | 0x2f | 0x5c | 0x3a)) {
        return Err(err("Invalid filename path component"));
    }
    let dot = u16::from(b'.');
    if units.is_empty() || units[..] == [dot] || units[..] == [dot, dot] {
        return Ok(());
    }
    let link = NameLink {
        parent: u64_at(value, 0)?,
        name: os_string_from_units(units),
    };
    if !record.names.contains(&link) {
        record.names.push(link)
    }
    Ok(())
}

/// Folds an unnamed `$DATA` attribute (resident, or one nonresident extent) into `record`.
fn parse_unnamed_data(a: &[u8], value: Option<&[u8]>, record: &mut Record) -> Result<()> {
    if let Some(v) = value {
        if record.nonresident {
            return Err(err("Mixed resident and nonresident unnamed DATA"));
        }
        if record.logical.is_some() {
            return Err(err("Duplicate unnamed DATA head"));
        }
        record.logical = Some(v.len() as u64);
        // Resident data occupies MFT metadata, not separately allocated data clusters.
        record.allocated = Some(0);
        return Ok(());
    }
    if record.logical.is_some() && !record.nonresident {
        return Err(err("Mixed resident and nonresident unnamed DATA"));
    }
    record.nonresident = true;
    let data_flags = u16_at(a, 12)?;
    if data_flags & 0xff > 1 {
        return Err(err("Unsupported NTFS compression method"));
    }
    let start = u64_at(a, 16)?;
    let highest = u64_at(a, 24)?;
    let mapping = u16_at(a, 32)? as usize;
    let compressed_or_sparse = data_flags & DATA_FLAGS_COMPRESSED_OR_SPARSE != 0;
    let minimum = if compressed_or_sparse { 72 } else { 64 };
    if mapping < minimum || mapping >= a.len() {
        return Err(err("Invalid DATA mapping-pairs offset"));
    }
    let runs = decode_runs(&a[mapping..], start)?;
    match runs.last() {
        None => {
            if start != 0 || u64_at(a, 40)? != 0 || u64_at(a, 48)? != 0 {
                return Err(err("Missing DATA mapping pairs"));
            }
        }
        Some(last) => {
            if last.vcn.checked_add(last.clusters) != highest.checked_add(1) {
                return Err(err("DATA highest VCN disagrees with mapping pairs"));
            }
            if !compressed_or_sparse && runs.iter().any(|r| r.lcn.is_none()) {
                return Err(err("Sparse mapping in ordinary DATA"));
            }
        }
    }
    record.data_runs.extend(runs);
    if start == 0 {
        if record.logical.is_some() {
            return Err(err("Duplicate unnamed DATA head"));
        }
        let logical = u64_at(a, 48)?;
        let valid = u64_at(a, 56)?;
        let allocated = if compressed_or_sparse {
            u64_at(a, 64)?
        } else {
            u64_at(a, 40)?
        };
        if logical > i64::MAX as u64 || allocated > i64::MAX as u64 || valid > logical {
            return Err(err("Invalid DATA sizes"));
        }
        record.logical = Some(logical);
        record.allocated = Some(allocated);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Decodes a copy of `bytes`. Production decodes in place and consumes its buffer; tests
    /// keep their fixtures reusable.
    fn parse_record(bytes: &[u8], index: u64, sector_size: usize) -> Result<Option<Record>> {
        let mut copy = bytes.to_vec();
        parse_record_in_place(&mut copy, index, sector_size, &mut ParseScratch::default())
    }
    fn record(index: u64, parent: u64, name: &str, is_dir: bool) -> Record {
        Record {
            reference: (7 << 48) | index,
            base: 0,
            is_dir,
            reparse: false,
            modified: None,
            names: vec![NameLink {
                parent: (7 << 48) | parent,
                name: name.into(),
            }],
            logical: Some(10),
            allocated: Some(4096),
            attributes: HashSet::new(),
            attribute_list: Vec::new(),
            attribute_list_unresolved: false,
            nonresident: false,
            data_runs: Vec::new(),
        }
    }
    fn tree() -> Vec<Record> {
        vec![
            record(5, 5, ".", true),
            record(16, 5, "folder", true),
            record(17, 16, "file", false),
        ]
    }
    #[test]
    fn reconstruction_preserves_hardlinks_and_directory_totals() {
        let mut records = tree();
        records[2].names.push(NameLink {
            parent: (7 << 48) | 5,
            name: "second".into(),
        });
        let model = reconstruct(Path::new("C:\\"), 42, 4096, records, || false).unwrap();
        assert_eq!(
            (
                model.nodes[0].logical,
                model.nodes[0].files,
                model.nodes[0].folders
            ),
            (20, 2, 1)
        );
        assert_eq!(model.unique_allocated, Some(4096));
        assert_eq!(model.hardlinks, 1);
        assert!(
            model
                .nodes
                .iter()
                .any(|n| model.path(n.id) == Path::new("C:\\").join("folder").join("file"))
        );
    }
    #[test]
    fn reconstruction_rejects_stale_parent_sequence() {
        let mut records = tree();
        records[2].names[0].parent = (6 << 48) | 16;
        assert!(reconstruct(Path::new("C:\\"), 42, 4096, records, || false).is_err());
    }
    #[test]
    fn reconstruction_merges_extension_name_and_data() {
        let mut records = tree();
        let mut extension = record(18, 5, "second", false);
        extension.base = records[2].reference;
        for (kind, id) in [(0x30, 2), (0x80, 3)] {
            let key = AttributeKey {
                kind,
                id,
                start_vcn: 0,
                name: Vec::new(),
            };
            extension.attributes.insert(key.clone());
            records[2].attribute_list.push(AttributeReference {
                reference: extension.reference,
                key,
            });
        }
        records[2].logical = None;
        records[2].allocated = None;
        records.push(extension);
        let model = reconstruct(Path::new("C:\\"), 42, 4096, records, || false).unwrap();
        assert_eq!(model.nodes[0].logical, 20);
        assert_eq!(model.unique_allocated, Some(4096));
    }
    #[test]
    fn reconstruction_rejects_stale_extension_sequence() {
        let mut records = tree();
        let mut extension = record(18, 5, "second", false);
        extension.base = (6 << 48) | 17;
        records.push(extension);
        assert!(reconstruct(Path::new("C:\\"), 42, 4096, records, || false).is_err());
    }
    #[test]
    fn reconstruction_requires_every_listed_attribute_to_exist() {
        let mut records = tree();
        let key = AttributeKey {
            kind: 0x30,
            id: 2,
            start_vcn: 0,
            name: Vec::new(),
        };
        records[2].attribute_list.push(AttributeReference {
            reference: (7 << 48) | 18,
            key,
        });
        assert!(reconstruct(Path::new("C:\\"), 42, 4096, records, || false).is_err());
    }
    #[test]
    fn reconstruction_rejects_unlisted_extension_attributes() {
        let mut records = tree();
        let mut extension = record(18, 5, "second", false);
        extension.base = records[2].reference;
        extension.attributes.insert(AttributeKey {
            kind: 0x30,
            id: 2,
            start_vcn: 0,
            name: Vec::new(),
        });
        extension.logical = None;
        extension.allocated = None;
        records.push(extension);
        assert!(reconstruct(Path::new("C:\\"), 42, 4096, records, || false).is_err());
    }
    #[test]
    fn reconstruction_does_not_follow_reparse_directories() {
        let mut records = tree();
        records[1].reparse = true;
        let model = reconstruct(Path::new("C:\\"), 42, 4096, records, || false).unwrap();
        assert_eq!(model.nodes.len(), 2);
        assert_eq!(model.nodes[0].logical, 0);
        assert_eq!(model.skipped, 1);
    }
    #[test]
    fn reconstruction_rejects_cycles_or_missing_data() {
        let mut records = tree();
        records[1].names[0].parent = records[1].reference;
        assert!(reconstruct(Path::new("C:\\"), 42, 4096, records, || false).is_err());
        let mut records = tree();
        records[2].logical = None;
        assert!(reconstruct(Path::new("C:\\"), 42, 4096, records, || false).is_err());
    }
    fn raw_record() -> Vec<u8> {
        let mut b = vec![0u8; 1024];
        b[..4].copy_from_slice(b"FILE");
        b[4..6].copy_from_slice(&48u16.to_le_bytes());
        b[6..8].copy_from_slice(&3u16.to_le_bytes());
        b[16..18].copy_from_slice(&7u16.to_le_bytes());
        b[20..22].copy_from_slice(&56u16.to_le_bytes());
        b[22..24].copy_from_slice(&1u16.to_le_bytes());
        b[24..28].copy_from_slice(&64u32.to_le_bytes());
        b[28..32].copy_from_slice(&1024u32.to_le_bytes());
        b[48..54].copy_from_slice(&[0xaa, 0xbb, 0x11, 0x22, 0x33, 0x44]);
        b[510..512].copy_from_slice(&[0xaa, 0xbb]);
        b[1022..1024].copy_from_slice(&[0xaa, 0xbb]);
        b[56..60].copy_from_slice(&u32::MAX.to_le_bytes());
        b
    }
    #[test]
    fn signed_runs_and_sparse_runs_are_checked() {
        let r = decode_runs(&[0x11, 3, 100, 0x11, 2, 0xfe, 0x01, 4, 0], 0).unwrap();
        assert_eq!(
            r,
            vec![
                DataRun {
                    vcn: 0,
                    lcn: Some(100),
                    clusters: 3
                },
                DataRun {
                    vcn: 3,
                    lcn: Some(98),
                    clusters: 2
                },
                DataRun {
                    vcn: 5,
                    lcn: None,
                    clusters: 4
                }
            ]
        );
        assert!(decode_runs(&[0x11, 1, 0xff, 0], 0).is_err());
        assert!(decode_runs(&[0x91, 1, 0], 0).is_err());
        assert!(decode_runs(&[0x11, 0, 1, 0], 0).is_err());
        assert!(decode_runs(&[0x11, 1], 0).is_err());
    }
    #[test]
    fn fixups_restore_sector_ends_in_place() {
        let mut fixed = raw_record();
        apply_fixups(&mut fixed, 512).unwrap();
        assert_eq!(&fixed[510..512], &[0x11, 0x22]);
        assert_eq!(&fixed[1022..1024], &[0x33, 0x44]);
        assert!(apply_fixups(&mut fixed.clone(), 0).is_err());
    }
    #[test]
    fn fixups_validate_every_sector_before_writing_any_byte() {
        // Sector 1 matches the update sequence number, sector 2 does not. An in-place
        // implementation that restored sector 1 before checking sector 2 would leave the
        // caller a half-restored record.
        let mut broken = raw_record();
        broken[1022] = 0;
        let before = broken.clone();
        assert!(apply_fixups(&mut broken, 512).is_err());
        assert_eq!(broken, before);
    }
    #[test]
    fn fixup_array_overlapping_a_sector_end_is_rejected_untouched() {
        // The array would contain the very bytes that restoring sector 1 rewrites.
        let mut b = raw_record();
        b[4..6].copy_from_slice(&508u16.to_le_bytes());
        b[508..514].copy_from_slice(&[0xaa, 0xbb, 0xaa, 0xbb, 0x11, 0x22]);
        let before = b.clone();
        assert!(apply_fixups(&mut b, 512).is_err());
        assert_eq!(b, before);
    }
    #[test]
    fn file_reference_keeps_sequence_and_free_records_are_ignored() {
        let mut b = raw_record();
        let r = parse_record(&b, 42, 512).unwrap().unwrap();
        assert_eq!(r.reference, (7u64 << 48) | 42);
        b[22..24].copy_from_slice(&0u16.to_le_bytes());
        assert!(parse_record(&b, 42, 512).unwrap().is_none());
    }
    #[test]
    fn invalid_attribute_lengths_are_rejected() {
        let mut b = raw_record();
        b[56..60].copy_from_slice(&0x30u32.to_le_bytes());
        b[60..64].copy_from_slice(&0u32.to_le_bytes());
        assert!(parse_record(&b, 1, 512).is_err());
    }
    fn resident_record(kind: u32, value: &[u8]) -> Vec<u8> {
        let mut b = raw_record();
        let size = (24 + value.len() + 7) & !7;
        b[24..28].copy_from_slice(&((56 + size + 8) as u32).to_le_bytes());
        b[56..60].copy_from_slice(&kind.to_le_bytes());
        b[60..64].copy_from_slice(&(size as u32).to_le_bytes());
        b[72..76].copy_from_slice(&(value.len() as u32).to_le_bytes());
        b[76..78].copy_from_slice(&24u16.to_le_bytes());
        b[80..80 + value.len()].copy_from_slice(value);
        b[56 + size..60 + size].copy_from_slice(&u32::MAX.to_le_bytes());
        b
    }
    #[test]
    fn standard_information_preserves_unix_epoch_boundaries() {
        const UNIX_EPOCH: u64 = 116_444_736_000_000_000;
        for (ticks, expected) in [
            (0, None),
            (UNIX_EPOCH - 1, None),
            (UNIX_EPOCH, Some(0)),
            (UNIX_EPOCH + 9_999_999, Some(0)),
            (UNIX_EPOCH + 10_000_000, Some(1)),
        ] {
            let mut value = vec![0; 36];
            value[8..16].copy_from_slice(&ticks.to_le_bytes());
            let parsed = parse_record(&resident_record(0x10, &value), 17, 512)
                .unwrap()
                .unwrap();
            assert_eq!(parsed.modified, expected, "FILETIME {ticks}");
        }
    }
    #[test]
    fn file_names_cannot_escape_their_parent() {
        let mut value = vec![0; 72];
        value[64] = 3;
        value[65] = 1;
        value[66..72].copy_from_slice(&[b'a', 0, b'\\', 0, b'b', 0]);
        assert!(parse_record(&resident_record(0x30, &value), 17, 512).is_err());
    }
    #[test]
    fn nonresident_file_names_are_rejected() {
        let mut b = resident_record(0x30, &[0; 64]);
        b[64] = 1;
        assert!(parse_record(&b, 17, 512).is_err());
    }
    #[test]
    fn nonresident_attribute_lists_are_recorded_not_rejected() {
        let mut b = resident_record(0x20, &[0; 64]);
        b[64] = 1;
        let record = parse_record(&b, 17, 512).unwrap().unwrap();
        assert!(record.attribute_list_unresolved);
        assert!(record.attribute_list.is_empty());
        // A resident list is read, and is not flagged.
        let resident = parse_record(&resident_record(0x20, &[]), 17, 512)
            .unwrap()
            .unwrap();
        assert!(!resident.attribute_list_unresolved);
    }
    #[test]
    fn malformed_resident_attribute_lists_are_rejected() {
        assert!(parse_record(&resident_record(0x20, &[0; 26]), 17, 512).is_err());
    }
    #[test]
    fn unnamed_nonresident_sparse_data_uses_physical_allocation() {
        let b = sparse_record();
        let r = parse_record(&b, 42, 512).unwrap().unwrap();
        assert_eq!(r.logical, Some(10000));
        assert_eq!(r.allocated, Some(4096));
    }
    fn sparse_record() -> Vec<u8> {
        let mut b = raw_record();
        b[24..28].copy_from_slice(&144u32.to_le_bytes());
        b[56..60].copy_from_slice(&0x80u32.to_le_bytes());
        b[60..64].copy_from_slice(&80u32.to_le_bytes());
        b[64] = 1;
        b[68..70].copy_from_slice(&0x8000u16.to_le_bytes());
        b[80..88].copy_from_slice(&2u64.to_le_bytes());
        b[88..90].copy_from_slice(&72u16.to_le_bytes());
        b[96..104].copy_from_slice(&12288u64.to_le_bytes());
        b[104..112].copy_from_slice(&10000u64.to_le_bytes());
        b[112..120].copy_from_slice(&10000u64.to_le_bytes());
        b[120..128].copy_from_slice(&4096u64.to_le_bytes());
        b[128..134].copy_from_slice(&[0x11, 1, 100, 0x01, 2, 0]);
        b[136..140].copy_from_slice(&u32::MAX.to_le_bytes());
        b
    }
    #[test]
    fn nonresident_data_requires_valid_mapping_pairs() {
        let mut b = sparse_record();
        b[88..90].copy_from_slice(&0u16.to_le_bytes());
        assert!(parse_record(&b, 42, 512).is_err());
        let mut b = sparse_record();
        b[132] = 3;
        assert!(parse_record(&b, 42, 512).is_err());
    }
    #[test]
    fn fixup_array_cannot_overlap_attributes() {
        let mut b = raw_record();
        b[4..6].copy_from_slice(&56u16.to_le_bytes());
        b[56..62].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0, 0]);
        b[510..512].fill(0xff);
        b[1022..1024].fill(0xff);
        assert!(parse_record(&b, 42, 512).is_err());
    }
    #[test]
    fn truncated_dos_aliases_do_not_hide_metadata_corruption() {
        let mut value = vec![0; 66];
        value[64] = 12;
        value[65] = 2;
        assert!(parse_record(&resident_record(0x30, &value), 42, 512).is_err());
    }
    #[test]
    fn reconstruction_rejects_mixed_resident_and_nonresident_data() {
        let mut records = tree();
        records[2].logical = Some(0);
        records[2].allocated = Some(0);
        let mut extension = record(18, 5, "second", false);
        extension.base = records[2].reference;
        extension.logical = None;
        extension.allocated = None;
        extension.nonresident = true;
        extension.data_runs = vec![DataRun {
            vcn: 0,
            lcn: None,
            clusters: 1,
        }];
        records.push(extension);
        assert!(reconstruct(Path::new("C:\\"), 42, 4096, records, || false).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn filename_units_are_preserved_and_dos_aliases_are_excluded() {
        use std::os::windows::ffi::OsStrExt;
        let mut value = vec![0; 70];
        value[..8].copy_from_slice(&((7u64 << 48) | 5).to_le_bytes());
        value[64] = 2;
        value[65] = 1;
        value[66..70].copy_from_slice(&[b'x', 0, 0, 0xd8]);
        let parsed = parse_record(&resident_record(0x30, &value), 42, 512)
            .unwrap()
            .unwrap();
        assert_eq!(
            parsed.names[0].name.encode_wide().collect::<Vec<_>>(),
            vec![0x78, 0xd800]
        );
        value[65] = 2;
        assert!(
            parse_record(&resident_record(0x30, &value), 42, 512)
                .unwrap()
                .unwrap()
                .names
                .is_empty()
        );
    }
    #[test]
    fn sparse_or_compressed_allocation_is_verified_against_all_extents() {
        for flags in [0x8000u16, 1] {
            let mut raw = sparse_record();
            raw[68..70].copy_from_slice(&flags.to_le_bytes());
            let mut file = parse_record(&raw, 17, 512).unwrap().unwrap();
            file.names.push(NameLink {
                parent: (7 << 48) | 16,
                name: "file".into(),
            });
            let mut records = tree();
            records[2] = file.clone();
            let model = reconstruct(Path::new("C:\\"), 42, 4096, records, || false).unwrap();
            assert_eq!(model.nodes[0].logical, 10000);
            assert_eq!(model.unique_allocated, Some(4096));
            file.allocated = Some(8192);
            let mut records = tree();
            records[2] = file;
            assert!(reconstruct(Path::new("C:\\"), 42, 4096, records, || false).is_err());
        }
    }
    #[test]
    fn every_truncated_record_prefix_is_rejected_without_panicking() {
        let raw = sparse_record();
        for end in 0..raw.len() {
            assert!(parse_record(&raw[..end], 17, 512).is_err(), "prefix {end}");
        }
    }
    #[test]
    fn metadata_descendants_are_excluded_from_user_file_totals() {
        let mut records = tree();
        records.push(record(20, 11, "metadata", true));
        records.push(record(21, 20, "private", false));
        let model = reconstruct(Path::new("C:\\"), 42, 4096, records, || false).unwrap();
        assert_eq!(model.nodes.len(), 3);
        assert_eq!(model.nodes[0].logical, 10);
    }
    #[test]
    fn metadata_files_without_unnamed_data_do_not_reject_the_volume() {
        // $Extend ($MFT index 11) holds view-index files such as $Quota,
        // $ObjId and $Reparse that carry no unnamed DATA stream.
        let mut records = tree();
        let mut quota = record(24, 11, "$Quota", false);
        quota.logical = None;
        quota.allocated = None;
        records.push(quota);
        let model = reconstruct(Path::new("C:\\"), 42, 4096, records, || false).unwrap();
        assert_eq!(model.nodes.len(), 3);
        assert_eq!(model.nodes[0].logical, 10);
        // The same omission on a user file is still an error.
        let mut records = tree();
        records[2].logical = None;
        assert!(reconstruct(Path::new("C:\\"), 42, 4096, records, || false).is_err());
    }

    // ---- Raw FILE-record fixtures -------------------------------------------------------------
    //
    // These build byte-exact MFT records (update-sequence array included) so the tests below
    // exercise the real decoder, not hand-assembled `Record` values.

    const SECTOR: usize = 512;
    const RECORD_SIZE: usize = 1024;
    const CLUSTER: u64 = 4096;
    const SEQUENCE: u16 = 3;
    const UPDATE_SEQUENCE_NUMBER: [u8; 2] = [0x34, 0x12];
    /// Index of `$Extend`, the system directory that anchors `$UsnJrnl`, `$Quota` and friends.
    const EXTEND_INDEX: u64 = 11;

    fn reference(index: u64) -> u64 {
        (u64::from(SEQUENCE) << 48) | index
    }

    /// An attribute with a resident value. `name` is empty for an unnamed attribute.
    fn resident_attr(kind: u32, id: u16, name: &str, value: &[u8]) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let value_at = (24 + units.len() * 2).next_multiple_of(8);
        let length = (value_at + value.len()).next_multiple_of(8);
        let mut a = vec![0u8; length];
        a[0..4].copy_from_slice(&kind.to_le_bytes());
        a[4..8].copy_from_slice(&(length as u32).to_le_bytes());
        a[9] = units.len() as u8;
        a[10..12].copy_from_slice(&24u16.to_le_bytes());
        a[14..16].copy_from_slice(&id.to_le_bytes());
        a[16..20].copy_from_slice(&(value.len() as u32).to_le_bytes());
        a[20..22].copy_from_slice(&(value_at as u16).to_le_bytes());
        for (n, unit) in units.iter().enumerate() {
            a[24 + n * 2..26 + n * 2].copy_from_slice(&unit.to_le_bytes());
        }
        a[value_at..value_at + value.len()].copy_from_slice(value);
        a
    }

    fn standard_information(flags: u32) -> Vec<u8> {
        const MODIFIED_FILETIME: u64 = 116_444_736_000_000_000 + 1_700_000_000 * 10_000_000;
        let mut value = vec![0u8; 48];
        value[8..16].copy_from_slice(&MODIFIED_FILETIME.to_le_bytes());
        value[32..36].copy_from_slice(&flags.to_le_bytes());
        resident_attr(ATTR_STANDARD_INFORMATION, 0, "", &value)
    }

    fn file_name(id: u16, parent: u64, name: &str, namespace: u8) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let mut value = vec![0u8; 66 + units.len() * 2];
        value[0..8].copy_from_slice(&reference(parent).to_le_bytes());
        value[64] = units.len() as u8;
        value[65] = namespace;
        for (n, unit) in units.iter().enumerate() {
            value[66 + n * 2..68 + n * 2].copy_from_slice(&unit.to_le_bytes());
        }
        resident_attr(ATTR_FILE_NAME, id, "", &value)
    }

    fn resident_data(id: u16, size: usize) -> Vec<u8> {
        resident_attr(ATTR_DATA, id, "", &vec![0xd5; size])
    }

    /// Mapping pairs for `(clusters, absolute lcn)` runs, with signed relative offsets.
    fn mapping_pairs(runs: &[(u64, u64)]) -> Vec<u8> {
        /// Fewest bytes whose two's-complement sign extension reproduces `value`.
        fn signed_width(value: i64) -> usize {
            (1u32..=8)
                .find(|width| {
                    let shift = 64 - 8 * width;
                    (value << shift) >> shift == value
                })
                .unwrap() as usize
        }
        let mut out = Vec::new();
        let mut previous = 0i64;
        for &(clusters, lcn) in runs {
            let delta = lcn as i64 - previous;
            previous = lcn as i64;
            let length_width = (8 - clusters.leading_zeros() as usize / 8).max(1);
            let offset_width = signed_width(delta);
            out.push((offset_width as u8) << 4 | length_width as u8);
            out.extend_from_slice(&clusters.to_le_bytes()[..length_width]);
            out.extend_from_slice(&delta.to_le_bytes()[..offset_width]);
        }
        out.push(0);
        out
    }

    /// The head extent of a nonresident, unnamed DATA stream holding `size` bytes in `runs`.
    fn nonresident_data(id: u16, size: u64, runs: &[(u64, u64)]) -> Vec<u8> {
        let mapping = mapping_pairs(runs);
        let clusters: u64 = runs.iter().map(|run| run.0).sum();
        let length = (64 + mapping.len()).next_multiple_of(8);
        let mut a = vec![0u8; length];
        a[0..4].copy_from_slice(&ATTR_DATA.to_le_bytes());
        a[4..8].copy_from_slice(&(length as u32).to_le_bytes());
        a[8] = 1;
        a[14..16].copy_from_slice(&id.to_le_bytes());
        a[24..32].copy_from_slice(&(clusters - 1).to_le_bytes());
        a[32..34].copy_from_slice(&64u16.to_le_bytes());
        a[40..48].copy_from_slice(&(clusters * CLUSTER).to_le_bytes());
        a[48..56].copy_from_slice(&size.to_le_bytes());
        a[56..64].copy_from_slice(&size.to_le_bytes());
        a[64..64 + mapping.len()].copy_from_slice(&mapping);
        a
    }

    /// An ATTRIBUTE_LIST whose entries live in clusters elsewhere on the volume.
    fn nonresident_attribute_list(id: u16) -> Vec<u8> {
        let mapping = mapping_pairs(&[(1, 1000)]);
        let length = (64 + mapping.len()).next_multiple_of(8);
        let mut a = vec![0u8; length];
        a[0..4].copy_from_slice(&ATTR_ATTRIBUTE_LIST.to_le_bytes());
        a[4..8].copy_from_slice(&(length as u32).to_le_bytes());
        a[8] = 1;
        a[14..16].copy_from_slice(&id.to_le_bytes());
        a[32..34].copy_from_slice(&64u16.to_le_bytes());
        a[64..64 + mapping.len()].copy_from_slice(&mapping);
        a
    }

    /// A resident ATTRIBUTE_LIST naming `(kind, id, holder index)` triples.
    fn attribute_list(id: u16, entries: &[(u32, u16, u64)]) -> Vec<u8> {
        const ENTRY_LENGTH: usize = 32;
        let mut value = Vec::new();
        for &(kind, entry_id, holder) in entries {
            let mut entry = vec![0u8; ENTRY_LENGTH];
            entry[0..4].copy_from_slice(&kind.to_le_bytes());
            entry[4..6].copy_from_slice(&(ENTRY_LENGTH as u16).to_le_bytes());
            entry[16..24].copy_from_slice(&reference(holder).to_le_bytes());
            entry[24..26].copy_from_slice(&entry_id.to_le_bytes());
            value.extend_from_slice(&entry);
        }
        resident_attr(ATTR_ATTRIBUTE_LIST, id, "", &value)
    }

    /// A complete in-use FILE record with a valid update-sequence array.
    fn build_record(base: u64, flags: u16, attributes: &[Vec<u8>]) -> Vec<u8> {
        let mut b = vec![0u8; RECORD_SIZE];
        b[..4].copy_from_slice(b"FILE");
        b[4..6].copy_from_slice(&48u16.to_le_bytes());
        b[6..8].copy_from_slice(&3u16.to_le_bytes());
        b[16..18].copy_from_slice(&SEQUENCE.to_le_bytes());
        b[20..22].copy_from_slice(&56u16.to_le_bytes());
        b[22..24].copy_from_slice(&flags.to_le_bytes());
        b[28..32].copy_from_slice(&(RECORD_SIZE as u32).to_le_bytes());
        b[32..40].copy_from_slice(&base.to_le_bytes());
        let mut at = 56;
        for attribute in attributes {
            b[at..at + attribute.len()].copy_from_slice(attribute);
            at += attribute.len();
        }
        b[at..at + 4].copy_from_slice(&ATTR_END.to_le_bytes());
        let used = at + 8;
        assert!(
            used <= RECORD_SIZE,
            "fixture attributes do not fit a record"
        );
        b[24..28].copy_from_slice(&(used as u32).to_le_bytes());
        // Update sequence: the number, then the original last two bytes of each sector, which
        // are overwritten with the number on disk.
        b[48..50].copy_from_slice(&UPDATE_SEQUENCE_NUMBER);
        for sector in 1..=2 {
            let end = sector * SECTOR;
            let original = [b[end - 2], b[end - 1]];
            b[48 + sector * 2..50 + sector * 2].copy_from_slice(&original);
            b[end - 2..end].copy_from_slice(&UPDATE_SEQUENCE_NUMBER);
        }
        b
    }

    fn root_record() -> Vec<u8> {
        build_record(
            0,
            RECORD_IN_USE | RECORD_IS_DIRECTORY,
            &[standard_information(0), file_name(1, 5, ".", 1)],
        )
    }
    fn directory_record(parent: u64, name: &str) -> Vec<u8> {
        build_record(
            0,
            RECORD_IN_USE | RECORD_IS_DIRECTORY,
            &[standard_information(0), file_name(1, parent, name, 1)],
        )
    }
    fn file_record(parent: u64, name: &str, size: usize) -> Vec<u8> {
        build_record(
            0,
            RECORD_IN_USE,
            &[
                standard_information(0),
                file_name(1, parent, name, 1),
                resident_data(2, size),
            ],
        )
    }

    /// An MFT image with each record at its index; unlisted indexes are zeroed (free) records.
    fn mft_of(records: &[(u64, Vec<u8>)]) -> Vec<u8> {
        let count = records.iter().map(|(index, _)| *index).max().unwrap_or(0) as usize + 1;
        let mut mft = vec![0u8; count * RECORD_SIZE];
        for (index, bytes) in records {
            let at = *index as usize * RECORD_SIZE;
            mft[at..at + RECORD_SIZE].copy_from_slice(bytes);
        }
        mft
    }

    fn decode_mft(mut mft: Vec<u8>) -> Result<Vec<Record>> {
        let layout = MftLayout {
            record_size: RECORD_SIZE,
            sector_size: SECTOR,
        };
        let mut records = Vec::new();
        parse_mft_records(
            &mut mft,
            0,
            layout,
            &mut ParseScratch::default(),
            &|| false,
            &mut records,
        )?;
        Ok(records)
    }

    fn rebuild(records: Vec<Record>) -> Result<Model> {
        reconstruct(Path::new("C:\\"), 42, CLUSTER, records, || false)
    }

    fn rebuild_mft(records: &[(u64, Vec<u8>)]) -> Result<Model> {
        rebuild(decode_mft(mft_of(records))?)
    }

    fn error_text(result: Result<Model>) -> String {
        result.unwrap_err().to_string()
    }

    // ---- ATTRIBUTE_LIST handling ---------------------------------------------------------------

    /// `$Extend\$UsnJrnl` as Windows writes it: its `$J` stream has so many extents that the
    /// attribute list is nonresident and the stream continues in an extension record. The base
    /// record sits at `index`, its extension at `index + 1`.
    fn usn_journal_records(index: u64) -> Vec<(u64, Vec<u8>)> {
        let base = build_record(
            0,
            RECORD_IN_USE,
            &[
                standard_information(0),
                nonresident_attribute_list(1),
                file_name(2, EXTEND_INDEX, "$UsnJrnl", 1),
                resident_attr(ATTR_DATA, 3, "$Max", &[0; 16]),
            ],
        );
        let extension = build_record(
            reference(index),
            RECORD_IN_USE,
            &[resident_attr(ATTR_DATA, 4, "$J", &[])],
        );
        vec![(index, base), (index + 1, extension)]
    }

    #[test]
    fn usn_journal_with_a_nonresident_attribute_list_does_not_reject_the_volume() {
        let mut volume = vec![(5, root_record()), (26, file_record(5, "hello.txt", 10))];
        volume.extend(usn_journal_records(24));
        let model = rebuild_mft(&volume).unwrap();
        // Only the root and the user file: the journal is excluded from user totals.
        assert_eq!(model.nodes.len(), 2);
        assert_eq!(model.nodes[0].logical, 10);
        assert_eq!((model.nodes[0].files, model.nodes[0].folders), (1, 0));
    }

    #[test]
    fn user_file_with_a_nonresident_attribute_list_names_the_record_in_its_fallback_reason() {
        let user_file = build_record(
            0,
            RECORD_IN_USE,
            &[
                standard_information(0),
                nonresident_attribute_list(1),
                file_name(2, 5, "fragmented.bin", 1),
                resident_data(3, 10),
            ],
        );
        let reason = error_text(rebuild_mft(&[(5, root_record()), (26, user_file)]));
        assert!(reason.contains("nonresident ATTRIBUTE_LIST"), "{reason}");
        assert!(reason.contains("MFT record 26"), "{reason}");
        // Distinct from every generic structural failure, so the reason is actionable.
        assert!(!reason.contains("Missing"), "{reason}");
    }

    fn unresolved(mut record: Record) -> Record {
        record.attribute_list_unresolved = true;
        record
    }

    #[test]
    fn nonresident_attribute_lists_are_rejected_on_user_files_folders_and_the_root() {
        for victim in 0..3 {
            let mut records = tree();
            records[victim] = unresolved(records[victim].clone());
            let reason = error_text(rebuild(records));
            assert!(
                reason.contains("nonresident ATTRIBUTE_LIST"),
                "{victim}: {reason}"
            );
        }
    }

    #[test]
    fn nonresident_attribute_lists_are_accepted_only_for_records_anchored_under_metadata() {
        let mut journal = unresolved(record(24, 11, "$UsnJrnl", false));
        journal.logical = None;
        journal.allocated = None;
        let mut records = tree();
        records.push(journal.clone());
        assert!(rebuild(records).is_ok());
        // An extension record may contribute a link into a user directory. Then the file is a
        // user file and its unread list can no longer be ignored.
        let mut alias_extension = record(25, 5, "alias", false);
        alias_extension.base = journal.reference;
        alias_extension.logical = None;
        alias_extension.allocated = None;
        let mut records = tree();
        records.push(journal.clone());
        records.push(alias_extension);
        let reason = error_text(rebuild(records));
        assert!(reason.contains("nonresident ATTRIBUTE_LIST"), "{reason}");
        // A second name directly under the root does the same without any extension.
        journal.names.push(NameLink {
            parent: (7 << 48) | 5,
            name: "alias".into(),
        });
        let mut records = tree();
        records.push(journal);
        assert!(error_text(rebuild(records)).contains("nonresident ATTRIBUTE_LIST"));
    }

    #[test]
    fn extension_records_of_an_unread_list_are_not_checked_against_it() {
        let mut journal = unresolved(record(24, 11, "$UsnJrnl", false));
        journal.logical = None;
        journal.allocated = None;
        let mut extension = record(25, 11, "unused", false);
        extension.names.clear();
        extension.base = journal.reference;
        extension.logical = None;
        extension.allocated = None;
        extension.attributes.insert(AttributeKey {
            kind: ATTR_DATA,
            id: 4,
            start_vcn: 0,
            name: vec![u16::from(b'$'), u16::from(b'J')],
        });
        // With a readable (here empty) list the same extension attribute is unlisted.
        let mut readable = journal.clone();
        readable.attribute_list_unresolved = false;
        for (base, accepted) in [(journal, true), (readable, false)] {
            let mut records = tree();
            records.push(base);
            records.push(extension.clone());
            assert_eq!(rebuild(records).is_ok(), accepted);
        }
    }

    #[test]
    fn an_attribute_list_inside_an_extension_record_is_always_rejected() {
        let mut base = record(18, 5, "base", false);
        base.logical = None;
        base.allocated = None;
        for unread in [false, true] {
            let mut extension = record(19, 5, "extension", false);
            extension.names.clear();
            extension.base = base.reference;
            extension.logical = Some(10);
            extension.allocated = Some(4096);
            if unread {
                extension.attribute_list_unresolved = true;
            } else {
                extension.attribute_list.push(AttributeReference {
                    reference: base.reference,
                    key: AttributeKey {
                        kind: ATTR_FILE_NAME,
                        id: 1,
                        start_vcn: 0,
                        name: Vec::new(),
                    },
                });
            }
            let mut records = tree();
            records.push(base.clone());
            records.push(extension);
            assert_eq!(
                error_text(rebuild(records)),
                "Extension contains an ATTRIBUTE_LIST"
            );
        }
    }

    /// A directory-less volume with one file whose DATA lives in an extension record, built from
    /// raw bytes. `listed` is what the base's ATTRIBUTE_LIST claims about the extension.
    fn volume_with_extension(listed: &[(u32, u16, u64)]) -> Vec<(u64, Vec<u8>)> {
        let mut entries = vec![(ATTR_STANDARD_INFORMATION, 0, 30), (ATTR_FILE_NAME, 2, 30)];
        entries.extend_from_slice(listed);
        let base = build_record(
            0,
            RECORD_IN_USE,
            &[
                standard_information(0),
                attribute_list(1, &entries),
                file_name(2, 5, "split.bin", 1),
            ],
        );
        let extension = build_record(
            reference(30),
            RECORD_IN_USE,
            &[nonresident_data(4, 5000, &[(2, 700)])],
        );
        vec![(5, root_record()), (30, base), (31, extension)]
    }

    #[test]
    fn attribute_lists_validate_extension_records_decoded_from_raw_bytes() {
        let model = rebuild_mft(&volume_with_extension(&[(ATTR_DATA, 4, 31)])).unwrap();
        assert_eq!(model.nodes[0].logical, 5000);
        assert_eq!(model.unique_allocated, Some(2 * CLUSTER));
        // Dropping the DATA entry leaves the extension's attribute unlisted.
        assert_eq!(
            error_text(rebuild_mft(&volume_with_extension(&[]))),
            "Unlisted extension attribute"
        );
        // Naming the wrong instance id: the target exists but holds no such attribute.
        assert_eq!(
            error_text(rebuild_mft(&volume_with_extension(&[(ATTR_DATA, 9, 31)]))),
            "ATTRIBUTE_LIST target does not match"
        );
        // Naming a record that is not in the MFT.
        assert_eq!(
            error_text(rebuild_mft(&volume_with_extension(&[(ATTR_DATA, 4, 99)]))),
            "Missing or stale ATTRIBUTE_LIST reference"
        );
    }

    #[test]
    fn only_records_in_an_attribute_list_keep_attribute_keys() {
        let volume = volume_with_extension(&[(ATTR_DATA, 4, 31)]);
        let mut records = decode_mft(mft_of(&volume)).unwrap();
        records.sort_by_key(|record| record.reference & INDEX_MASK);
        let kept: Vec<(u64, usize)> = records
            .iter()
            .map(|record| (record.reference & INDEX_MASK, record.attributes.len()))
            .collect();
        // Root: no list, not an extension. Base: carries a list (SI, LIST, FN). Extension: DATA.
        assert_eq!(kept, [(5, 0), (30, 3), (31, 1)]);
    }

    #[test]
    fn duplicate_attribute_identities_are_still_rejected_without_a_per_record_set() {
        let same_everything = [
            resident_attr(ATTR_STANDARD_INFORMATION, 0, "", &[0; 48]),
            resident_attr(ATTR_STANDARD_INFORMATION, 0, "", &[0; 48]),
        ];
        assert!(decode_mft(mft_of(&[(20, build_record(0, 1, &same_everything))])).is_err());
        // Same instance id but a different type, or the same type and id but another stream
        // name, are different attributes.
        let distinct = [
            resident_attr(ATTR_STANDARD_INFORMATION, 0, "", &[0; 48]),
            resident_attr(ATTR_DATA, 0, "", &[]),
            resident_attr(ATTR_DATA, 0, "alt", &[]),
        ];
        assert!(decode_mft(mft_of(&[(20, build_record(0, 1, &distinct))])).is_ok());
    }

    #[test]
    fn scratch_space_does_not_leak_attributes_between_records() {
        let attributes = [resident_attr(ATTR_STANDARD_INFORMATION, 0, "", &[0; 48])];
        let first = build_record(0, 1, &attributes);
        let second = build_record(0, 1, &attributes);
        let mut scratch = ParseScratch::default();
        for mut bytes in [first, second] {
            assert!(parse_record_in_place(&mut bytes, 20, SECTOR, &mut scratch).is_ok());
        }
    }

    // ---- Chunk decoding ------------------------------------------------------------------------

    #[test]
    fn system_files_and_their_extensions_are_skipped_before_parsing() {
        // Marked in use but with no update-sequence array: decoding it must fail.
        let garbage = {
            let mut b = vec![0u8; RECORD_SIZE];
            b[..4].copy_from_slice(b"FILE");
            b[22..24].copy_from_slice(&RECORD_IN_USE.to_le_bytes());
            b
        };
        // Index 3 is a system file; index 40 is an extension of system file 11. Neither may be
        // parsed, or their private layouts could fail the whole scan.
        let mut extension_of_system_file = garbage.clone();
        extension_of_system_file[32..40].copy_from_slice(&reference(EXTEND_INDEX).to_le_bytes());
        // `garbage` is not even a valid record, so a user index reports it, with its index.
        let skipped = [(3, garbage.clone()), (40, extension_of_system_file)];
        assert!(decode_mft(mft_of(&skipped)).unwrap().is_empty());
        let reason = decode_mft(mft_of(&[(30, garbage)]))
            .unwrap_err()
            .to_string();
        assert!(reason.starts_with("MFT record 30: "), "{reason}");
    }

    #[test]
    fn decoding_a_chunk_honours_cancellation() {
        let mut mft = mft_of(&[(5, root_record())]);
        let layout = MftLayout {
            record_size: RECORD_SIZE,
            sector_size: SECTOR,
        };
        let result = parse_mft_records(
            &mut mft,
            0,
            layout,
            &mut ParseScratch::default(),
            &|| true,
            &mut Vec::new(),
        );
        assert_eq!(result.unwrap_err().to_string(), "NTFS scan cancelled");
    }

    // ---- Drive roots and name decoding ---------------------------------------------------------

    #[test]
    fn only_whole_drive_roots_name_a_drive_letter() {
        assert_eq!(drive_root_letter(Path::new("C:\\")), Some(b'C'));
        assert_eq!(drive_root_letter(Path::new("d:\\")), Some(b'd'));
        for path in [
            "",
            "C",
            "C:",
            "C:\\folder",
            "\\\\host\\share\\",
            "\\\\?\\C:\\",
            "C:/",
            "C:\\\\",
            "1:\\",
            "CC:\\",
            "\u{e9}:\\",
        ] {
            assert_eq!(drive_root_letter(Path::new(path)), None, "{path:?}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn unpaired_surrogates_become_replacement_characters_off_windows() {
        let mut value = vec![0; 70];
        value[..8].copy_from_slice(&((7u64 << 48) | 5).to_le_bytes());
        value[64] = 2;
        value[65] = 1;
        value[66..70].copy_from_slice(&[b'x', 0, 0, 0xd8]);
        let parsed = parse_record(&resident_record(0x30, &value), 42, 512)
            .unwrap()
            .unwrap();
        assert_eq!(parsed.names[0].name, OsString::from("x\u{fffd}"));
    }

    // ---- Whole-volume equivalence --------------------------------------------------------------

    /// Deterministic pseudo-random numbers, so fixtures need no dev-dependency.
    struct Lcg(u64);
    impl Lcg {
        fn below(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % bound
        }
    }

    /// What one directory node must hold, accumulated by the generator itself.
    #[derive(Default, Clone)]
    struct Totals {
        logical: u64,
        allocated: u64,
        files: u64,
        folders: u64,
    }

    /// A generated MFT plus the model it must reconstruct to, derived from the generator's own
    /// bookkeeping and never from the decoder.
    struct SyntheticVolume {
        mft: Vec<u8>,
        /// `(path, is_dir, logical, allocated, files, folders)` for every user node.
        expected: Vec<(PathBuf, bool, u64, u64, u64, u64)>,
        root: Totals,
        unique_allocated: u64,
        hardlinks: u64,
        /// Records expected to keep attribute keys: each generated extension and its base, plus
        /// the USN journal's extension (its base list is unread, so it keeps none).
        list_participants: usize,
        record_count: usize,
    }

    const SYNTHETIC_DIRECTORIES: usize = 120;
    const SYNTHETIC_FILES: usize = 3000;
    /// Files at or below this size stay resident in their MFT record.
    const RESIDENT_LIMIT: u64 = 200;
    const FIRST_SYNTHETIC_INDEX: u64 = 16;

    fn synthetic_volume() -> SyntheticVolume {
        let mut rng = Lcg(0x5eed_1234);
        let mut records: Vec<(u64, Vec<u8>)> = vec![(5, root_record())];
        let root_path = PathBuf::from("C:\\");
        // Directory k lives at MFT index FIRST + k; its parent has a smaller index.
        let mut dir_paths: Vec<PathBuf> = Vec::new();
        let mut dir_parent: Vec<Option<usize>> = Vec::new();
        let mut dir_totals = vec![Totals::default(); SYNTHETIC_DIRECTORIES];
        let mut root = Totals::default();
        let mut expected = Vec::new();
        for k in 0..SYNTHETIC_DIRECTORIES {
            let parent = if k < 8 { None } else { Some((k - 8) / 3) };
            let parent_index = parent.map_or(5, |p| FIRST_SYNTHETIC_INDEX + p as u64);
            let name = format!("dir{k}");
            let path = parent
                .map_or(root_path.clone(), |p| dir_paths[p].clone())
                .join(&name);
            records.push((
                FIRST_SYNTHETIC_INDEX + k as u64,
                directory_record(parent_index, &name),
            ));
            dir_paths.push(path);
            dir_parent.push(parent);
            // A folder counts toward every ancestor (not itself).
            let mut ancestor = parent;
            while let Some(a) = ancestor {
                dir_totals[a].folders += 1;
                ancestor = dir_parent[a];
            }
            root.folders += 1;
        }
        let first_file = FIRST_SYNTHETIC_INDEX + SYNTHETIC_DIRECTORIES as u64;
        let first_extension = first_file + SYNTHETIC_FILES as u64;
        let mut extensions = 0u64;
        let (mut unique_allocated, mut hardlinks) = (0u64, 0u64);
        let mut next_lcn = 1000u64;
        for j in 0..SYNTHETIC_FILES {
            let index = first_file + j as u64;
            let slot = rng.below(SYNTHETIC_DIRECTORIES as u64 + 1) as usize;
            let parent = (slot < SYNTHETIC_DIRECTORIES).then_some(slot);
            let parent_index = parent.map_or(5, |p| FIRST_SYNTHETIC_INDEX + p as u64);
            let size = rng.below(100_000);
            let clusters = size.div_ceil(CLUSTER);
            let resident = size <= RESIDENT_LIMIT;
            let allocated = if resident { 0 } else { clusters * CLUSTER };
            let name = format!("f{j}.dat");
            let mut links = vec![(parent, parent_index, name)];
            if j % 11 == 0 {
                // A second hard link in another directory.
                let other = parent.map_or(Some(0), |_| None);
                let other_index = other.map_or(5, |p| FIRST_SYNTHETIC_INDEX + p as u64);
                links.push((other, other_index, format!("alias{j}.dat")));
            }
            let in_extension = j % 50 == 7;
            let data = if resident {
                resident_data(4, size as usize)
            } else {
                let lcn = next_lcn;
                next_lcn += clusters + 1;
                if j % 7 == 0 && clusters >= 2 {
                    // Two extents, the second placed before the first (negative offset).
                    let head = clusters / 2;
                    nonresident_data(4, size, &[(head, lcn + 500_000), (clusters - head, lcn)])
                } else {
                    nonresident_data(4, size, &[(clusters, lcn)])
                }
            };
            let mut attributes = vec![standard_information(if j % 97 == 0 {
                FILE_ATTRIBUTE_REPARSE_POINT
            } else {
                0
            })];
            for (n, (_, link_parent, link_name)) in links.iter().enumerate() {
                attributes.push(file_name(1 + n as u16, *link_parent, link_name, 1));
            }
            if j % 5 == 0 {
                // A DOS 8.3 alias of the first name; must not become a link.
                attributes.push(file_name(3, parent_index, &format!("F{j}~1.DAT"), 2));
            }
            if in_extension {
                let extension_index = first_extension + extensions;
                extensions += 1;
                let mut entries = vec![(ATTR_STANDARD_INFORMATION, 0, index)];
                for n in 0..links.len() {
                    entries.push((ATTR_FILE_NAME, 1 + n as u16, index));
                }
                if j % 5 == 0 {
                    entries.push((ATTR_FILE_NAME, 3, index));
                }
                entries.push((ATTR_DATA, 4, extension_index));
                attributes.push(attribute_list(6, &entries));
                records.push((
                    extension_index,
                    build_record(reference(index), RECORD_IN_USE, &[data]),
                ));
            } else {
                attributes.push(data);
                if j % 13 == 0 {
                    // An alternate stream is not part of the file's size.
                    attributes.push(resident_attr(ATTR_DATA, 5, "Zone.Identifier", &[1; 5]));
                }
            }
            records.push((index, build_record(0, RECORD_IN_USE, &attributes)));
            unique_allocated += allocated;
            hardlinks += links.len() as u64 - 1;
            for (link_parent, _, link_name) in links {
                let directory = link_parent.map_or(root_path.clone(), |p| dir_paths[p].clone());
                expected.push((directory.join(link_name), false, size, allocated, 0, 0));
                root.logical += size;
                root.allocated += allocated;
                root.files += 1;
                let mut ancestor = link_parent;
                while let Some(a) = ancestor {
                    dir_totals[a].logical += size;
                    dir_totals[a].allocated += allocated;
                    dir_totals[a].files += 1;
                    ancestor = dir_parent[a];
                }
            }
        }
        // System metadata under $Extend: a file without unnamed DATA, the USN journal with its
        // nonresident list and extension, and a not-in-use record. Gaps stay zeroed.
        let metadata = first_extension + extensions;
        records.push((
            metadata,
            build_record(
                0,
                RECORD_IN_USE,
                &[
                    standard_information(0),
                    file_name(1, EXTEND_INDEX, "$Quota", 1),
                ],
            ),
        ));
        records.extend(usn_journal_records(metadata + 2));
        records.push((metadata + 5, build_record(0, 0, &[standard_information(0)])));
        for (k, path) in dir_paths.iter().enumerate() {
            let totals = &dir_totals[k];
            expected.push((
                path.clone(),
                true,
                totals.logical,
                totals.allocated,
                totals.files,
                totals.folders,
            ));
        }
        let mft = mft_of(&records);
        SyntheticVolume {
            record_count: mft.len() / RECORD_SIZE,
            mft,
            expected,
            root,
            unique_allocated,
            hardlinks,
            list_participants: extensions as usize * 2 + 1,
        }
    }

    #[test]
    fn a_synthetic_volume_of_thousands_of_records_reconstructs_to_the_generators_ground_truth() {
        let volume = synthetic_volume();
        assert!(volume.record_count > 3000, "{}", volume.record_count);
        let records = decode_mft(volume.mft.clone()).unwrap();
        // Records with keys are exactly the list participants; the rest allocate nothing.
        let with_keys = records.iter().filter(|r| !r.attributes.is_empty()).count();
        assert_eq!(with_keys, volume.list_participants);
        assert!(
            records
                .iter()
                .filter(|r| r.attributes.is_empty())
                .all(|r| r.attributes.capacity() == 0 && r.attribute_list.is_empty())
        );
        let model = rebuild(records).unwrap();
        let root = &model.nodes[0];
        assert_eq!(
            (root.logical, root.files, root.folders),
            (volume.root.logical, volume.root.files, volume.root.folders)
        );
        assert_eq!(root.allocated, Some(volume.root.allocated));
        assert_eq!(model.unique_allocated, Some(volume.unique_allocated));
        assert_eq!(model.hardlinks, volume.hardlinks);
        assert_eq!((model.skipped, model.errors_total), (0, 0));
        let mut actual: Vec<_> = model
            .nodes
            .iter()
            .skip(1)
            .map(|n| {
                (
                    model.path(n.id),
                    n.is_dir,
                    n.logical,
                    n.allocated.unwrap(),
                    n.files,
                    n.folders,
                )
            })
            .collect();
        let mut expected = volume.expected.clone();
        actual.sort();
        expected.sort();
        assert_eq!(actual.len(), expected.len());
        // Compare as a whole so a mismatch prints the first differing node, not 3000 lines.
        if let Some(position) = actual.iter().zip(&expected).position(|(a, e)| a != e) {
            panic!(
                "node {position} differs: {:?} != {:?}",
                actual[position], expected[position]
            );
        }
    }

    #[test]
    fn the_same_volume_decodes_identically_in_small_and_large_chunks() {
        // The volume reader hands the decoder up to 4 MiB at a time; chunk boundaries must not
        // change the result.
        let volume = synthetic_volume();
        let whole = decode_mft(volume.mft.clone()).unwrap();
        let layout = MftLayout {
            record_size: RECORD_SIZE,
            sector_size: SECTOR,
        };
        let mut scratch = ParseScratch::default();
        let mut pieces = Vec::new();
        let mut mft = volume.mft.clone();
        const RECORDS_PER_CHUNK: usize = 97;
        for (n, chunk) in mft.chunks_mut(RECORDS_PER_CHUNK * RECORD_SIZE).enumerate() {
            parse_mft_records(
                chunk,
                (n * RECORDS_PER_CHUNK) as u64,
                layout,
                &mut scratch,
                &|| false,
                &mut pieces,
            )
            .unwrap();
        }
        let mut whole: Vec<_> = whole.iter().map(canonical).collect();
        let mut pieces: Vec<_> = pieces.iter().map(canonical).collect();
        whole.sort();
        pieces.sort();
        assert!(!whole.is_empty());
        assert!(whole == pieces, "chunked decoding changed a record");
    }

    /// Everything decoded for a record in a stable order (`attributes` is an unordered set).
    fn canonical(record: &Record) -> String {
        let mut keys: Vec<_> = record
            .attributes
            .iter()
            .map(|key| (key.kind, key.id, key.start_vcn, key.name.clone()))
            .collect();
        keys.sort();
        format!(
            "{:?}",
            (
                (
                    record.reference,
                    record.base,
                    record.is_dir,
                    record.reparse,
                    record.modified,
                    &record.names,
                ),
                (record.logical, record.allocated, record.nonresident),
                (&record.data_runs, keys),
                (
                    record.attribute_list.len(),
                    record.attribute_list_unresolved,
                ),
            )
        )
    }
}
