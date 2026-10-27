//! Checked, execution-free NTFS metadata decoding for the optional volume backend.
use crate::model::{Entry, FileIdentity, Model};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::Path;

/// Low 48 bits of an MFT reference: the record index without its sequence.
#[cfg_attr(not(windows), allow(dead_code))]
const INDEX_MASK: u64 = 0x0000_ffff_ffff_ffff;
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

/// System metadata records 0-15 other than the root directory (index 5)
/// anchor subtrees that are excluded from user totals.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_metadata_parent(parent_index: u64) -> bool {
    parent_index < 16 && parent_index != 5
}

/// Builds a [`Model`] from decoded MFT records. The decoder and this
/// reconstruction are portable and unit-tested everywhere; only the Windows
/// volume reader produces records outside tests.
#[cfg_attr(not(windows), allow(dead_code))]
fn reconstruct(
    root: &Path,
    volume: u64,
    cluster: u64,
    records: Vec<Record>,
    cancelled: impl Fn() -> bool,
) -> Result<Model> {
    {
        let lookup: HashMap<_, _> = records.iter().map(|r| (r.reference, r)).collect();
        for record in &records {
            if cancelled() {
                return Err(err("NTFS scan cancelled"));
            }
            if record.base != 0 && !record.attribute_list.is_empty() {
                return Err(err("Extension contains an ATTRIBUTE_LIST"));
            }
            for required in &record.attribute_list {
                let target = lookup
                    .get(&required.reference)
                    .ok_or_else(|| err("Missing or stale ATTRIBUTE_LIST reference"))?;
                if (target.reference != record.reference && target.base != record.reference)
                    || !target.attributes.contains(&required.key)
                {
                    return Err(err("ATTRIBUTE_LIST target does not match"));
                }
            }
            if record.base != 0 {
                let base = lookup
                    .get(&record.base)
                    .ok_or_else(|| err("Missing or stale extension base reference"))?;
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
    }
    let mut bases = HashMap::new();
    let mut extensions = Vec::new();
    let mut indexes = HashSet::new();
    for record in records {
        if cancelled() {
            return Err(err("NTFS scan cancelled"));
        }
        if !indexes.insert(record.reference & INDEX_MASK) {
            return Err(err("Duplicate MFT record index"));
        }
        if record.base == 0 {
            bases.insert(record.reference, record);
        } else {
            extensions.push(record);
        }
    }
    for extension in extensions {
        if cancelled() {
            return Err(err("NTFS scan cancelled"));
        }
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
    for record in bases.values_mut() {
        if cancelled() {
            return Err(err("NTFS scan cancelled"));
        }
        if record.nonresident {
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
    }
    let root_ref = bases
        .keys()
        .copied()
        .find(|r| r & INDEX_MASK == 5)
        .ok_or_else(|| err("Missing NTFS root record"))?;
    if !bases[&root_ref].is_dir {
        return Err(err("NTFS root is not a directory"));
    }
    let mut children: HashMap<u64, Vec<(u64, OsString)>> = HashMap::new();
    let mut sibling_names = HashSet::new();
    let mut excluded_roots = HashSet::new();
    for record in bases.values() {
        if cancelled() {
            return Err(err("NTFS scan cancelled"));
        }
        let index = record.reference & INDEX_MASK;
        if index < 16 {
            continue;
        }
        if record.names.is_empty() || (record.is_dir && record.names.len() != 1) {
            return Err(err("Missing or ambiguous FILE_NAME links"));
        }
        // Files living only under metadata directories ($Extend\$Quota,
        // $ObjId, $Reparse, $UsnJrnl) legitimately have no unnamed DATA
        // stream. They are excluded from user totals, so their accounting is
        // not required; demanding it would reject every standard volume.
        let metadata_only = record
            .names
            .iter()
            .all(|name| is_metadata_parent(name.parent & INDEX_MASK));
        if !record.is_dir
            && !metadata_only
            && (record.logical.is_none() || record.allocated.is_none())
        {
            return Err(err("Missing unnamed DATA accounting"));
        }
        for name in &record.names {
            if is_metadata_parent(name.parent & INDEX_MASK) {
                excluded_roots.insert(name.parent);
            } else if bases.get(&name.parent).is_none_or(|p| !p.is_dir) {
                return Err(err("Missing, stale or non-directory parent reference"));
            }
            if !sibling_names.insert((name.parent, name.name.clone())) {
                return Err(err("Duplicate directory entry name"));
            }
            children
                .entry(name.parent)
                .or_default()
                .push((record.reference, name.name.clone()));
        }
    }
    let mut model = Model::new(root.to_owned());
    let mut queue = VecDeque::from([(root_ref, Some(0))]);
    queue.extend(excluded_roots.into_iter().map(|r| (r, None)));
    let mut visited_dirs = HashSet::new();
    while let Some((parent, node)) = queue.pop_front() {
        if cancelled() {
            return Err(err("NTFS scan cancelled"));
        }
        if !visited_dirs.insert(parent) {
            return Err(err("Directory cycle or multiple directory links"));
        }
        let links = children.remove(&parent).unwrap_or_default();
        for chunk in links.chunks(512) {
            if cancelled() {
                return Err(err("NTFS scan cancelled"));
            }
            let entries = if node.is_some() {
                chunk
                    .iter()
                    .map(|(reference, name)| {
                        let record = &bases[reference];
                        let mut id = [0; 16];
                        id[..8].copy_from_slice(&reference.to_le_bytes());
                        Entry {
                            name: name.clone(),
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
pub struct DataRun {
    pub vcn: u64,
    pub lcn: Option<u64>,
    pub clusters: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameLink {
    pub parent: u64,
    pub name: OsString,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AttributeKey {
    kind: u32,
    id: u16,
    start_vcn: u64,
    name: Vec<u16>,
}
#[derive(Debug, Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct AttributeReference {
    reference: u64,
    key: AttributeKey,
}
#[derive(Debug, Clone)]
pub struct Record {
    pub reference: u64,
    pub base: u64,
    pub is_dir: bool,
    pub reparse: bool,
    pub modified: Option<u64>,
    pub names: Vec<NameLink>,
    pub logical: Option<u64>,
    pub allocated: Option<u64>,
    pub attributes: HashSet<AttributeKey>,
    pub attribute_list: Vec<AttributeReference>,
    pub nonresident: bool,
    pub data_runs: Vec<DataRun>,
}

fn err(message: &str) -> NtfsError {
    NtfsError(message.into())
}
fn range(b: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    b.get(at..at.checked_add(len).ok_or_else(|| err("Offset overflow"))?)
        .ok_or_else(|| err("Truncated NTFS metadata"))
}
fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(range(b, at, 2)?.try_into().unwrap()))
}
fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(range(b, at, 4)?.try_into().unwrap()))
}
fn u64_at(b: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(range(b, at, 8)?.try_into().unwrap()))
}

pub fn decode_runs(data: &[u8], start_vcn: u64) -> Result<Vec<DataRun>> {
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
        if out.len() > 1_000_000 {
            return Err(err("Data-run budget exceeded"));
        }
    }
    Err(err("Missing data-run terminator"))
}

pub fn apply_fixups(bytes: &[u8], sector_size: usize) -> Result<Vec<u8>> {
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
    let values = range(
        bytes,
        usa,
        count
            .checked_mul(2)
            .ok_or_else(|| err("Fixup array overflow"))?,
    )?;
    for sector in 1..count {
        if range(bytes, sector * sector_size - 2, 2)? != &values[..2] {
            return Err(err("NTFS sector fixup mismatch"));
        }
    }
    let mut fixed = bytes.to_vec();
    for sector in 1..count {
        fixed[sector * sector_size - 2..sector * sector_size]
            .copy_from_slice(&values[sector * 2..sector * 2 + 2]);
    }
    Ok(fixed)
}

pub fn parse_record(bytes: &[u8], index: u64, sector_size: usize) -> Result<Option<Record>> {
    if bytes.len() < 48 || index > 0x0000_ffff_ffff_ffff {
        return Err(err("Invalid FILE record header"));
    }
    if &bytes[..4] != b"FILE" {
        if bytes.iter().all(|&x| x == 0) {
            return Ok(None);
        }
        return Err(err("Invalid FILE record signature"));
    }
    let flags = u16_at(bytes, 22)?;
    if flags & 1 == 0 {
        return Ok(None);
    }
    let b = apply_fixups(bytes, sector_size)?;
    let used = u32_at(&b, 24)? as usize;
    let mut at = u16_at(&b, 20)? as usize;
    if at < 48
        || !at.is_multiple_of(8)
        || used > b.len()
        || at.checked_add(4).is_none_or(|n| n > used)
    {
        return Err(err("Invalid FILE attribute bounds"));
    }
    let usa = u16_at(&b, 4)? as usize;
    let count = u16_at(&b, 6)? as usize;
    if usa < 48 || usa + count * 2 > at || u32_at(&b, 28)? as usize != b.len() {
        return Err(err("Invalid FILE allocation or update-sequence bounds"));
    }
    let mut record = Record {
        reference: index | ((u16_at(&b, 16)? as u64) << 48),
        base: u64_at(&b, 32)?,
        is_dir: flags & 2 != 0,
        reparse: false,
        modified: None,
        names: Vec::new(),
        logical: None,
        allocated: None,
        attributes: HashSet::new(),
        attribute_list: Vec::new(),
        nonresident: false,
        data_runs: Vec::new(),
    };
    let mut terminated = false;
    while at + 4 <= used {
        let kind = u32_at(&b, at)?;
        if kind == u32::MAX {
            terminated = true;
            break;
        }
        let length = u32_at(&b, at + 4)? as usize;
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
        let attr_name = if a[9] != 0 {
            let name_offset = u16_at(a, 10)? as usize;
            if name_offset < if nonresident == 0 { 24 } else { 64 } {
                return Err(err("Invalid attribute name offset"));
            }
            range(a, name_offset, a[9] as usize * 2)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect()
        } else {
            Vec::new()
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
        if !record.attributes.insert(AttributeKey {
            kind,
            id: u16_at(a, 14)?,
            start_vcn: if nonresident == 0 { 0 } else { u64_at(a, 16)? },
            name: attr_name,
        }) {
            return Err(err("Duplicate attribute identity"));
        }
        if nonresident != 0 && matches!(kind, 0x10 | 0x30) {
            return Err(err("Unsupported nonresident name or standard information"));
        }
        match kind {
            0x20 => {
                let v = value.ok_or_else(|| err("Nonresident ATTRIBUTE_LIST is not supported"))?;
                let mut p = 0;
                while p < v.len() {
                    let length = u16_at(v, p + 4)? as usize;
                    if length < 26 || !length.is_multiple_of(8) {
                        return Err(err("Invalid ATTRIBUTE_LIST entry length"));
                    }
                    let entry = range(v, p, length)?;
                    let name_length = entry[6] as usize;
                    let name_offset = entry[7] as usize;
                    if name_length > 0 && name_offset < 26 {
                        return Err(err("Invalid ATTRIBUTE_LIST name offset"));
                    }
                    let name = if name_length == 0 {
                        Vec::new()
                    } else {
                        range(entry, name_offset, name_length * 2)?
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|p| u16::from_le_bytes([p[0], p[1]]))
                            .collect()
                    };
                    record.attribute_list.push(AttributeReference {
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
            }
            0x10 => {
                if let Some(v) = value {
                    if v.len() < 36 {
                        return Err(err("Truncated standard information"));
                    }
                    record.modified = u64_at(v, 8)?
                        .checked_sub(116_444_736_000_000_000)
                        .map(|ticks| ticks / 10_000_000);
                    record.reparse |= u32_at(v, 32)? & 0x400 != 0;
                }
            }
            0x30 => {
                if let Some(v) = value {
                    if v.len() < 66 {
                        return Err(err("Truncated file name"));
                    }
                    if v[65] > 3 {
                        return Err(err("Invalid filename namespace"));
                    }
                    record.reparse |= u32_at(v, 56)? & 0x400 != 0;
                    let encoded = range(v, 66, v[64] as usize * 2)?;
                    if v[65] != 2 {
                        let wide: Vec<u16> = encoded
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|p| u16::from_le_bytes([p[0], p[1]]))
                            .collect();
                        if wide.iter().any(|c| matches!(*c, 0 | 47 | 92 | 58)) {
                            return Err(err("Invalid filename path component"));
                        }
                        #[cfg(windows)]
                        let name = {
                            use std::os::windows::ffi::OsStringExt;
                            OsString::from_wide(&wide)
                        };
                        #[cfg(not(windows))]
                        let name = OsString::from(String::from_utf16_lossy(&wide));
                        if name != "." && name != ".." && !name.is_empty() {
                            let link = NameLink {
                                parent: u64_at(v, 0)?,
                                name,
                            };
                            if !record.names.contains(&link) {
                                record.names.push(link)
                            }
                        }
                    }
                }
            }
            0x80 => {
                if a[9] == 0 {
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
                    } else {
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
                        let minimum = if data_flags & 0x80ff != 0 { 72 } else { 64 };
                        if mapping < minimum || mapping >= a.len() {
                            return Err(err("Invalid DATA mapping-pairs offset"));
                        }
                        let runs = decode_runs(&a[mapping..], start)?;
                        if runs.is_empty() {
                            if start != 0 || u64_at(a, 40)? != 0 || u64_at(a, 48)? != 0 {
                                return Err(err("Missing DATA mapping pairs"));
                            }
                        } else {
                            let last = runs.last().unwrap();
                            if last.vcn.checked_add(last.clusters) != highest.checked_add(1) {
                                return Err(err("DATA highest VCN disagrees with mapping pairs"));
                            }
                            if data_flags & 0x80ff == 0 && runs.iter().any(|r| r.lcn.is_none()) {
                                return Err(err("Sparse mapping in ordinary DATA"));
                            }
                        }
                        record.data_runs.extend(runs);
                        if start == 0 {
                            if record.logical.is_some() {
                                return Err(err("Duplicate unnamed DATA head"));
                            }
                            let logical = u64_at(a, 48)?;
                            let valid = u64_at(a, 56)?;
                            let allocated = if data_flags & 0x80ff != 0 {
                                u64_at(a, 64)?
                            } else {
                                u64_at(a, 40)?
                            };
                            if logical > i64::MAX as u64
                                || allocated > i64::MAX as u64
                                || valid > logical
                            {
                                return Err(err("Invalid DATA sizes"));
                            }
                            record.logical = Some(logical);
                            record.allocated = Some(allocated);
                        }
                    }
                }
            }
            0xc0 => record.reparse = true,
            _ => {}
        }
        at += length;
    }
    if !terminated {
        return Err(err("Missing FILE attribute terminator"));
    }
    Ok(Some(record))
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn fixups_validate_every_sector_before_returning_bytes() {
        let b = raw_record();
        let fixed = apply_fixups(&b, 512).unwrap();
        assert_eq!(&fixed[510..512], &[0x11, 0x22]);
        assert_eq!(&fixed[1022..1024], &[0x33, 0x44]);
        let mut broken = b;
        broken[1022] = 0;
        assert!(apply_fixups(&broken, 512).is_err());
        assert!(apply_fixups(&fixed, 0).is_err());
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
    fn unsupported_nonresident_attribute_lists_are_rejected() {
        let mut b = resident_record(0x20, &[0; 64]);
        b[64] = 1;
        assert!(parse_record(&b, 17, 512).is_err());
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
}
