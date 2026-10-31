//! Read-only Win32 NTFS volume access. No privilege adjustment or elevation.
use super::*;
use std::{
    io,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{IO::*, Ioctl::*, Threading::*},
};

const READ_BUDGET: usize = 4 * 1024 * 1024;
struct ReadBuffer {
    pointer: std::ptr::NonNull<u8>,
    layout: std::alloc::Layout,
}
impl ReadBuffer {
    fn new(size: usize) -> Result<Self> {
        if size == 0 || size > READ_BUDGET {
            return Err(err("Invalid raw read-buffer size"));
        }
        let layout = std::alloc::Layout::from_size_align(size, 65536)
            .map_err(|_| err("Invalid raw read-buffer layout"))?;
        // The maximum supported sector size is 64 KiB. DASD reads can be
        // noncached even without FILE_FLAG_NO_BUFFERING, so Vec<u8> is insufficient.
        let pointer = std::ptr::NonNull::new(unsafe { std::alloc::alloc_zeroed(layout) })
            .ok_or_else(|| err("Cannot allocate aligned MFT buffer"))?;
        Ok(Self { pointer, layout })
    }
    fn bytes(&mut self) -> &mut [u8] {
        // Exclusive self borrow keeps the allocation live and forbids aliasing.
        unsafe { std::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.layout.size()) }
    }
}
impl Drop for ReadBuffer {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.pointer.as_ptr(), self.layout) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Geometry {
    serial: u64,
    sector: u64,
    cluster: u64,
    record: u64,
    mft_length: u64,
    total_clusters: u64,
    mft_lcn: u64,
}
/// The drive letter of an exact drive root. The engine chooses this backend with the same
/// predicate (`drive_root_letter`), so it can never route a path here that this rejects.
fn root_drive(root: &Path) -> Result<u8> {
    drive_root_letter(root)
        .ok_or_else(|| err("NTFS acceleration requires an exact local drive root such as C:\\"))
}
fn is_volume_device(target: &[u16]) -> bool {
    let prefix: Vec<_> = "\\Device\\HarddiskVolume".encode_utf16().collect();
    target
        .strip_prefix(prefix.as_slice())
        .is_some_and(|suffix| !suffix.is_empty() && suffix.iter().all(|c| (48..=57).contains(c)))
}
fn parse_geometry(bytes: &[u8]) -> Result<Geometry> {
    if u32_at(bytes, 96)? < 8 || u16_at(bytes, 100)? != 3 || u16_at(bytes, 102)? > 1 {
        return Err(err("Unsupported NTFS version"));
    }
    let g = Geometry {
        serial: u64_at(bytes, 0)?,
        sector: u32_at(bytes, 40)? as u64,
        cluster: u32_at(bytes, 44)? as u64,
        record: u32_at(bytes, 48)? as u64,
        mft_length: u64_at(bytes, 56)?,
        total_clusters: u64_at(bytes, 16)?,
        mft_lcn: u64_at(bytes, 64)?,
    };
    if !g.sector.is_power_of_two()
        || !(512..=65536).contains(&g.sector)
        || !g.cluster.is_power_of_two()
        || !(g.sector..=2 * 1024 * 1024).contains(&g.cluster)
        || !g.record.is_power_of_two()
        || !(g.sector..=65536).contains(&g.record)
        || g.mft_length == 0
        || !g.mft_length.is_multiple_of(g.record)
        || g.mft_length > i64::MAX as u64
        || g.total_clusters == 0
        || g.total_clusters > i64::MAX as u64
        || g.mft_lcn >= g.total_clusters
        || g.total_clusters
            .checked_mul(g.cluster)
            .is_none_or(|n| n > i64::MAX as u64)
        || g.mft_length > g.total_clusters * g.cluster
    {
        return Err(err("Unsupported or inconsistent NTFS geometry"));
    }
    Ok(g)
}
fn parse_extents(bytes: &[u8], start: u64, total_clusters: u64) -> Result<Vec<DataRun>> {
    let count = u32_at(bytes, 0)? as usize;
    if count == 0 || count > READ_BUDGET / 16 || u64_at(bytes, 8)? != start {
        return Err(err("Invalid retrieval-pointer header"));
    }
    range(
        bytes,
        16,
        count
            .checked_mul(16)
            .ok_or_else(|| err("Extent count overflow"))?,
    )?;
    let mut result = Vec::with_capacity(count);
    let mut vcn = start;
    for n in 0..count {
        let next = u64_at(bytes, 16 + n * 16)?;
        let lcn = u64_at(bytes, 24 + n * 16)?;
        if next <= vcn
            || next > i64::MAX as u64
            || lcn >= total_clusters
            || lcn
                .checked_add(next - vcn)
                .is_none_or(|end| end > total_clusters)
        {
            return Err(err("Sparse, out-of-range or reversed MFT extent"));
        }
        result.push(DataRun {
            vcn,
            lcn: Some(lcn),
            clusters: next - vcn,
        });
        vcn = next;
    }
    Ok(result)
}
fn read_stream(
    runs: &[DataRun],
    cluster: u64,
    mut offset: u64,
    mut out: &mut [u8],
    mut read: impl FnMut(u64, &mut [u8]) -> Result<()>,
) -> Result<()> {
    if cluster == 0 || out.len() > READ_BUDGET {
        return Err(err("Invalid MFT read size"));
    }
    let mut index = runs
        .partition_point(|r| r.vcn <= offset / cluster)
        .checked_sub(1)
        .ok_or_else(|| err("Unmapped MFT offset"))?;
    while !out.is_empty() {
        let run = runs
            .get(index)
            .ok_or_else(|| err("MFT extent coverage ended early"))?;
        let start = run
            .vcn
            .checked_mul(cluster)
            .ok_or_else(|| err("Extent offset overflow"))?;
        let length = run
            .clusters
            .checked_mul(cluster)
            .ok_or_else(|| err("Extent length overflow"))?;
        let inside = offset
            .checked_sub(start)
            .filter(|n| *n < length)
            .ok_or_else(|| err("Gap in MFT extents"))?;
        let physical = run
            .lcn
            .ok_or_else(|| err("Sparse MFT is unsupported"))?
            .checked_mul(cluster)
            .and_then(|n| n.checked_add(inside))
            .ok_or_else(|| err("Physical offset overflow"))?;
        let amount = (length - inside).min(out.len() as u64) as usize;
        let (now, rest) = out.split_at_mut(amount);
        read(physical, now)?;
        out = rest;
        offset = offset
            .checked_add(amount as u64)
            .ok_or_else(|| err("MFT read offset overflow"))?;
        index += 1;
    }
    Ok(())
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn os_error(context: &str, code: u32) -> NtfsError {
    err(&format!(
        "{context}: {} (Windows error {code})",
        io::Error::from_raw_os_error(code as i32)
    ))
}
fn open(path: &str, access: u32) -> Result<Handle> {
    let wide: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
    // Read-only/metadata access, shared with the filesystem and live applications.
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        Err(os_error("Cannot open NTFS metadata", unsafe {
            GetLastError()
        }))
    } else {
        Ok(Handle(raw))
    }
}

// This guard outlives every buffer passed into a pending call. Unwinding (including a
// caller callback panic) cancels and drains before the event/OVERLAPPED is freed.
struct Pending<'a> {
    handle: &'a Handle,
    event: Handle,
    overlapped: Box<OVERLAPPED>,
    active: bool,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if self.active {
            unsafe {
                CancelIoEx(self.handle.0, &*self.overlapped);
                let mut bytes = 0;
                GetOverlappedResult(self.handle.0, &*self.overlapped, &mut bytes, 1);
            }
        }
    }
}
fn operation(
    handle: &Handle,
    offset: u64,
    cancelled: &impl Fn() -> bool,
    start: impl FnOnce(*mut OVERLAPPED, *mut u32) -> i32,
) -> Result<(u32, u32)> {
    if cancelled() {
        return Err(err("NTFS scan cancelled"));
    }
    let event = unsafe { CreateEventW(null(), 1, 0, null()) };
    if event.is_null() {
        return Err(os_error("Cannot create NTFS I/O event", unsafe {
            GetLastError()
        }));
    }
    let mut pending = Pending {
        handle,
        event: Handle(event),
        overlapped: Box::new(OVERLAPPED::default()),
        active: false,
    };
    pending.overlapped.hEvent = pending.event.0;
    pending.overlapped.Anonymous.Anonymous.Offset = offset as u32;
    pending.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
    let mut bytes = 0;
    if start(&mut *pending.overlapped, &mut bytes) != 0 {
        return Ok((bytes, 0));
    }
    let code = unsafe { GetLastError() };
    if code != ERROR_IO_PENDING {
        return Ok((bytes, code));
    }
    pending.active = true;
    loop {
        if cancelled() {
            return Err(err("NTFS scan cancelled"));
        }
        match unsafe { WaitForSingleObject(pending.event.0, 50) } {
            WAIT_TIMEOUT => continue,
            WAIT_OBJECT_0 => break,
            _ => return Err(os_error("NTFS I/O wait failed", unsafe { GetLastError() })),
        }
    }
    let ok = unsafe { GetOverlappedResult(handle.0, &*pending.overlapped, &mut bytes, 0) };
    let code = if ok != 0 {
        0
    } else {
        unsafe { GetLastError() }
    };
    // A signalled manual-reset event completes the operation; ERROR_IO_INCOMPLETE
    // is still drained defensively rather than allowing a buffer lifetime violation.
    if code != ERROR_IO_INCOMPLETE {
        pending.active = false;
    }
    if code == ERROR_IO_INCOMPLETE {
        return Err(err("NTFS I/O event signalled before completion"));
    }
    Ok((bytes, code))
}
fn ioctl(
    handle: &Handle,
    code: u32,
    input: &[u8],
    output: &mut [u8],
    cancelled: &impl Fn() -> bool,
) -> Result<(usize, u32)> {
    let (bytes, status) = operation(handle, 0, cancelled, |overlapped, returned| unsafe {
        DeviceIoControl(
            handle.0,
            code,
            if input.is_empty() {
                null()
            } else {
                input.as_ptr().cast()
            },
            input.len() as u32,
            output.as_mut_ptr().cast(),
            output.len() as u32,
            returned,
            overlapped,
        )
    })?;
    if bytes as usize > output.len() {
        return Err(err("NTFS control output exceeds supplied buffer"));
    }
    if status != 0 && status != ERROR_MORE_DATA {
        return Err(os_error("NTFS metadata control failed", status));
    }
    Ok((bytes as usize, status))
}
fn geometry(handle: &Handle, cancelled: &impl Fn() -> bool) -> Result<Geometry> {
    let mut out = [0u8; 128];
    let (bytes, status) = ioctl(handle, FSCTL_GET_NTFS_VOLUME_DATA, &[], &mut out, cancelled)?;
    if status != 0 {
        return Err(err("Truncated NTFS volume geometry"));
    }
    parse_geometry(&out[..bytes])
}
fn extents(handle: &Handle, g: &Geometry, cancelled: &impl Fn() -> bool) -> Result<Vec<DataRun>> {
    let mut out = vec![0; 64 * 1024];
    let mut start = 0u64;
    let mut runs = Vec::new();
    loop {
        let (bytes, status) = ioctl(
            handle,
            FSCTL_GET_RETRIEVAL_POINTERS,
            &start.to_le_bytes(),
            &mut out,
            cancelled,
        )?;
        let page = parse_extents(&out[..bytes], start, g.total_clusters)?;
        // parse_extents rejects a zero-extent header, so every page has a final
        // extent; prove that here instead of indexing the page with unwrap.
        let Some(last) = page.last() else {
            return Err(err("Empty MFT extent page"));
        };
        start = last
            .vcn
            .checked_add(last.clusters)
            .ok_or_else(|| err("MFT VCN overflow"))?;
        runs.extend(page);
        if runs.len() > 1_000_000 {
            return Err(err("MFT extent budget exceeded"));
        }
        if status == 0 {
            break;
        }
    }
    if runs.first().and_then(|r| r.lcn) != Some(g.mft_lcn)
        || start
            .checked_mul(g.cluster)
            .is_none_or(|n| n < g.mft_length)
    {
        return Err(err("MFT extent map disagrees with volume geometry"));
    }
    Ok(runs)
}
pub fn scan_volume(
    root: &Path,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(NtfsProgress),
) -> Result<Model> {
    if cancelled() {
        return Err(err("NTFS scan cancelled"));
    }
    let letter = root_drive(root)?;
    let drive = u16::from(letter);
    let root_wide = [drive, 58, 92, 0];
    // DRIVE_REMOVABLE=2, DRIVE_FIXED=3. Reject remote, substituted and unknown roots.
    if !matches!(unsafe { GetDriveTypeW(root_wide.as_ptr()) }, 2 | 3) {
        return Err(err(
            "NTFS acceleration requires a local fixed or removable volume",
        ));
    }
    let device = [drive, 58, 0];
    let mut target = [0u16; 1024];
    let count =
        unsafe { QueryDosDeviceW(device.as_ptr(), target.as_mut_ptr(), target.len() as u32) };
    if count == 0 {
        return Err(os_error("Cannot resolve volume device", unsafe {
            GetLastError()
        }));
    }
    let end = target
        .iter()
        .position(|c| *c == 0)
        .ok_or_else(|| err("Unterminated volume device name"))?;
    if !is_volume_device(&target[..end]) {
        return Err(err(
            "Substituted or unsupported volume device; use normal enumeration",
        ));
    }
    let letter = char::from(letter);
    let volume = open(&format!("\\\\.\\{letter}:"), GENERIC_READ)?;
    let g = geometry(&volume, &cancelled)?;
    let mft = open(&format!("\\\\?\\{letter}:\\$MFT"), 0)?;
    let runs = extents(&mft, &g, &cancelled)?;
    let total = g.mft_length / g.record;
    progress(NtfsProgress {
        records: 0,
        total_records: total,
    });
    let mut storage = ReadBuffer::new(READ_BUDGET)?;
    let buffer = storage.bytes();
    let mut records = Vec::new();
    let mut parse_scratch = ParseScratch::default();
    let layout = MftLayout {
        record_size: g.record as usize,
        sector_size: g.sector as usize,
    };
    let mut offset = 0;
    while offset < g.mft_length {
        if cancelled() {
            return Err(err("NTFS scan cancelled"));
        }
        let size = (g.mft_length - offset).min(READ_BUDGET as u64) as usize;
        read_stream(
            &runs,
            g.cluster,
            offset,
            &mut buffer[..size],
            |physical, out| {
                let (read, status) =
                    operation(&volume, physical, &cancelled, |overlapped, bytes| unsafe {
                        ReadFile(
                            volume.0,
                            out.as_mut_ptr(),
                            out.len() as u32,
                            bytes,
                            overlapped,
                        )
                    })?;
                if status != 0 {
                    return Err(os_error("Reading MFT extent failed", status));
                }
                if read as usize != out.len() {
                    return Err(err("Short MFT extent read"));
                }
                Ok(())
            },
        )?;
        // Fixups are applied in place; the buffer is refilled by the next read.
        parse_mft_records(
            &mut buffer[..size],
            offset / g.record,
            layout,
            &mut parse_scratch,
            &cancelled,
            &mut records,
        )?;
        offset += size as u64;
        progress(NtfsProgress {
            records: offset / g.record,
            total_records: total,
        });
    }
    // Live scanning is not a snapshot. Detect MFT growth/relocation explicitly;
    // reference and attribute validation catches inconsistent record reuse.
    if geometry(&volume, &cancelled)? != g || extents(&mft, &g, &cancelled)? != runs {
        return Err(err(
            "MFT changed during the scan; restart with normal enumeration",
        ));
    }
    reconstruct(root, g.serial, g.cluster, records, cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    #[test]
    fn raw_volume_read_buffer_is_sector_aligned_and_bounded() {
        let mut buffer = ReadBuffer::new(READ_BUDGET).unwrap();
        assert_eq!(buffer.bytes().len(), 4 * 1024 * 1024);
        assert_eq!(buffer.bytes().as_ptr() as usize % 65536, 0);
        assert!(ReadBuffer::new(READ_BUDGET + 1).is_err());
    }
    #[test]
    fn substituted_drive_targets_are_rejected() {
        assert!(is_volume_device(
            &"\\Device\\HarddiskVolume3"
                .encode_utf16()
                .collect::<Vec<_>>()
        ));
        for target in [
            "\\??\\C:\\folder",
            "\\Device\\Mup\\server\\share",
            "\\Device\\HarddiskVolume3\\folder",
        ] {
            assert!(!is_volume_device(
                &target.encode_utf16().collect::<Vec<_>>()
            ));
        }
    }
    #[test]
    fn overlapped_reads_use_the_requested_file_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bytes");
        std::fs::write(&path, b"0123456789").unwrap();
        let file = open(path.to_str().unwrap(), GENERIC_READ).unwrap();
        let mut out = [0; 4];
        let result = operation(&file, 3, &|| false, |overlapped, bytes| unsafe {
            ReadFile(file.0, out.as_mut_ptr(), 4, bytes, overlapped)
        })
        .unwrap();
        assert_eq!(result, (4, 0));
        assert_eq!(&out, b"3456");
    }
    #[test]
    fn pending_io_is_cancelled_and_drained_before_buffers_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let wide: Vec<_> = dir
            .path()
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        assert_ne!(raw, INVALID_HANDLE_VALUE);
        let handle = Handle(raw);
        let mut buffer = [0u32; 256];
        let checks = std::cell::Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() >= 3
        };
        let result = operation(&handle, 0, &cancelled, |overlapped, bytes| unsafe {
            let accepted = ReadDirectoryChangesW(
                handle.0,
                buffer.as_mut_ptr().cast(),
                1024,
                0,
                FILE_NOTIFY_CHANGE_FILE_NAME,
                bytes,
                overlapped,
                None,
            );
            // This API reports successful queuing as TRUE, unlike ReadFile.
            if accepted != 0 {
                SetLastError(ERROR_IO_PENDING);
                0
            } else {
                accepted
            }
        });
        assert!(result.unwrap_err().0.contains("cancelled"));
    }
    #[test]
    #[ignore = "Explicit read-only metadata eligibility probe; no full-volume scan"]
    fn local_c_volume_metadata_probe() {
        let result = (|| {
            let volume = open("\\\\.\\C:", GENERIC_READ)?;
            let g = super::geometry(&volume, &|| false)?;
            let mft = open("\\\\?\\C:\\$MFT", 0)?;
            let map = extents(&mft, &g, &|| false)?;
            Ok::<_, NtfsError>((g, map.len()))
        })();
        eprintln!("Read-only C: metadata eligibility: {result:?}");
    }
    #[test]
    fn only_exact_drive_roots_are_eligible() {
        assert_eq!(root_drive(Path::new("C:\\")).unwrap(), b'C');
        for path in [
            "C:",
            "C:\\folder",
            "\\\\host\\share\\",
            "\\\\?\\C:\\",
            "C:/",
            "C:\\\\",
        ] {
            assert!(root_drive(Path::new(path)).is_err(), "{path}");
        }
    }
    fn geometry() -> Vec<u8> {
        let mut b = vec![0; 128];
        b[0..8].copy_from_slice(&42u64.to_le_bytes());
        b[16..24].copy_from_slice(&1000u64.to_le_bytes());
        b[40..44].copy_from_slice(&512u32.to_le_bytes());
        b[44..48].copy_from_slice(&4096u32.to_le_bytes());
        b[48..52].copy_from_slice(&1024u32.to_le_bytes());
        b[56..64].copy_from_slice(&102400u64.to_le_bytes());
        b[64..72].copy_from_slice(&4u64.to_le_bytes());
        b[96..100].copy_from_slice(&8u32.to_le_bytes());
        b[100..102].copy_from_slice(&3u16.to_le_bytes());
        b[102..104].copy_from_slice(&1u16.to_le_bytes());
        b
    }
    #[test]
    fn geometry_rejects_unsupported_versions_and_record_alignment() {
        assert_eq!(parse_geometry(&geometry()).unwrap().mft_length, 102400);
        let mut b = geometry();
        b[100] = 4;
        assert!(parse_geometry(&b).is_err());
        let mut b = geometry();
        b[48..52].copy_from_slice(&1000u32.to_le_bytes());
        assert!(parse_geometry(&b).is_err());
    }
    #[test]
    fn extent_decoder_checks_signed_locations_and_contiguous_vcns() {
        let mut b = vec![0; 48];
        b[..4].copy_from_slice(&2u32.to_le_bytes());
        b[16..24].copy_from_slice(&2u64.to_le_bytes());
        b[24..32].copy_from_slice(&10u64.to_le_bytes());
        b[32..40].copy_from_slice(&5u64.to_le_bytes());
        b[40..48].copy_from_slice(&20u64.to_le_bytes());
        assert_eq!(
            parse_extents(&b, 0, 100).unwrap(),
            vec![
                DataRun {
                    vcn: 0,
                    lcn: Some(10),
                    clusters: 2
                },
                DataRun {
                    vcn: 2,
                    lcn: Some(20),
                    clusters: 3
                }
            ]
        );
        b[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(parse_extents(&b, 0, 100).is_err());
        assert!(parse_extents(&b[..30], 0, 100).is_err());
    }
    #[test]
    fn stream_read_spans_fragmented_cluster_runs_without_losing_record_bytes() {
        let runs = [
            DataRun {
                vcn: 0,
                lcn: Some(3),
                clusters: 1,
            },
            DataRun {
                vcn: 1,
                lcn: Some(8),
                clusters: 2,
            },
        ];
        let mut out = vec![0; 1024];
        let mut offsets = Vec::new();
        read_stream(&runs, 512, 0, &mut out, |offset, bytes| {
            offsets.push(offset);
            bytes.fill(if offset == 1536 { 1 } else { 2 });
            Ok(())
        })
        .unwrap();
        assert_eq!(offsets, vec![1536, 4096]);
        assert_eq!(&out[..512], &[1; 512]);
        assert_eq!(&out[512..], &[2; 512]);
        assert!(read_stream(&runs, 512, 1400, &mut out, |_, _| Ok(())).is_err());
    }
}
