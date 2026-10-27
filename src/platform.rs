//! Read-only bounded enumeration. Native paths never round-trip through UTF-8.
use crate::model::Entry;
use std::{
    io,
    path::{Path, PathBuf},
};
const BATCH: usize = 128;
pub enum EnumerationEvent {
    Entries(Vec<Entry>),
    Error(PathBuf, io::Error),
    Skipped,
}

/// Emits bounded batches; returning false or setting cancellation abandons the directory.
pub fn enumerate(
    path: &Path,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(EnumerationEvent) -> bool,
) -> io::Result<()> {
    if cancelled() {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if is_reparse(&metadata) {
        // Only a scan root reaches this point: nested reparse directories are
        // listed but never scheduled. A silent empty "complete" scan would hide
        // that nothing was read, so report it as an explicit root error.
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "The scan root is a directory junction or link. DiskScope does not follow links; choose the target folder instead.",
        ));
    }
    #[cfg(windows)]
    if windows::enumerate(path, &cancelled, &mut emit)? {
        return Ok(());
    }
    fallback(path, &cancelled, &mut emit)
}
#[cfg(windows)]
fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}
#[cfg(not(windows))]
fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn fallback(
    path: &Path,
    cancelled: &impl Fn() -> bool,
    emit: &mut impl FnMut(EnumerationEvent) -> bool,
) -> io::Result<()> {
    let mut batch = Vec::with_capacity(BATCH);
    for item in std::fs::read_dir(path)? {
        if cancelled() {
            return Ok(());
        }
        let item = match item {
            Ok(item) => item,
            Err(error) => {
                if !emit(EnumerationEvent::Error(path.into(), error)) {
                    return Ok(());
                }
                continue;
            }
        };
        let metadata = match std::fs::symlink_metadata(item.path()) {
            Ok(metadata) => metadata,
            Err(error) => {
                if !emit(EnumerationEvent::Error(item.path(), error)) {
                    return Ok(());
                }
                continue;
            }
        };
        let reparse = is_reparse(&metadata);
        let is_dir = metadata.is_dir();
        batch.push(Entry {
            name: item.file_name(),
            is_dir,
            reparse,
            logical: if is_dir { 0 } else { metadata.len() },
            allocated: if is_dir { Some(0) } else { None },
            identity: None,
            modified: metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
        });
        if batch.len() == BATCH
            && !emit(EnumerationEvent::Entries(std::mem::replace(
                &mut batch,
                Vec::with_capacity(BATCH),
            )))
        {
            return Ok(());
        }
    }
    if !batch.is_empty() {
        emit(EnumerationEvent::Entries(batch));
    }
    Ok(())
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::model::FileIdentity;
    use std::{
        ffi::OsString,
        fs::OpenOptions,
        mem::{offset_of, size_of},
        os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES,
            ERROR_NOT_SUPPORTED,
        },
        Storage::FileSystem::*,
    };

    pub(super) fn enumerate(
        path: &Path,
        cancelled: &impl Fn() -> bool,
        emit: &mut impl FnMut(EnumerationEvent) -> bool,
    ) -> io::Result<bool> {
        let directory = OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let handle = directory.as_raw_handle();
        let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: valid live directory handle and correctly sized writable output.
        if unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } != 0
            && tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            emit(EnumerationEvent::Skipped);
            return Ok(true);
        }
        let mut id = FILE_ID_INFO::default();
        // SAFETY: same handle, initialized output matching FileIdInfo.
        let volume = (unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        } != 0)
            .then_some(id.VolumeSerialNumber);
        // u64 storage supplies the eight-byte alignment required by directory records. The
        // allocation is zeroed, so the first call already sees a clean buffer.
        let mut storage = vec![0u64; 8192];
        let capacity = storage.len() * size_of::<u64>();
        let mut first = true;
        loop {
            if cancelled() {
                return Ok(true);
            }
            // Clear a buffer that an earlier call wrote to. The API reports no byte count, so the
            // record walk below cannot tell where the new page ends; zeroing guarantees it can
            // never reach records left over from the previous page. The first call skips this:
            // the buffer is still the zeroed allocation (a small directory needs only one call).
            if !first {
                storage.fill(0);
            }
            // SAFETY: writable aligned 64 KiB buffer; passed size is its actual capacity.
            let success = unsafe {
                GetFileInformationByHandleEx(
                    handle,
                    FileIdExtdDirectoryInfo,
                    storage.as_mut_ptr().cast(),
                    capacity as u32,
                )
            };
            if success == 0 {
                let error = io::Error::last_os_error();
                let code = error.raw_os_error().unwrap_or(0) as u32;
                if code == ERROR_NO_MORE_FILES {
                    return Ok(true);
                }
                if first
                    && [
                        ERROR_INVALID_FUNCTION,
                        ERROR_INVALID_PARAMETER,
                        ERROR_NOT_SUPPORTED,
                    ]
                    .contains(&code)
                {
                    return Ok(false);
                }
                return Err(error);
            }
            first = false;
            let mut offset = 0usize;
            let mut batch = Vec::with_capacity(BATCH);
            loop {
                if cancelled() {
                    return Ok(true);
                }
                if offset + size_of::<FILE_ID_EXTD_DIR_INFO>() > capacity {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "directory record exceeds buffer",
                    ));
                }
                // SAFETY: checked record bounds; unaligned read also tolerates provider offsets.
                let entry = unsafe {
                    std::ptr::read_unaligned(
                        storage
                            .as_ptr()
                            .cast::<u8>()
                            .add(offset)
                            .cast::<FILE_ID_EXTD_DIR_INFO>(),
                    )
                };
                let name_offset = offset + offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
                let bytes = entry.FileNameLength as usize;
                if !bytes.is_multiple_of(2) || name_offset + bytes > capacity {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid directory name length",
                    ));
                }
                // SAFETY: bounds checked and record/name offsets have UTF-16 alignment.
                let wide = unsafe {
                    std::slice::from_raw_parts(
                        storage.as_ptr().cast::<u8>().add(name_offset).cast::<u16>(),
                        bytes / 2,
                    )
                };
                if wide != [b'.' as u16] && wide != [b'.' as u16, b'.' as u16] {
                    let is_dir = entry.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                    batch.push(Entry {
                        name: OsString::from_wide(wide),
                        is_dir,
                        reparse: entry.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
                        logical: if is_dir {
                            0
                        } else {
                            entry.EndOfFile.max(0) as u64
                        },
                        allocated: if is_dir {
                            Some(0)
                        } else {
                            u64::try_from(entry.AllocationSize).ok()
                        },
                        modified: u64::try_from(entry.LastWriteTime)
                            .ok()
                            .and_then(|v| (v / 10_000_000).checked_sub(11_644_473_600)),
                        identity: volume.filter(|_| entry.FileId.Identifier != [0; 16]).map(
                            |volume| FileIdentity {
                                volume,
                                id: entry.FileId.Identifier,
                            },
                        ),
                    });
                    if batch.len() == BATCH
                        && !emit(EnumerationEvent::Entries(std::mem::replace(
                            &mut batch,
                            Vec::with_capacity(BATCH),
                        )))
                    {
                        return Ok(true);
                    }
                }
                if entry.NextEntryOffset == 0 {
                    break;
                }
                let next = entry.NextEntryOffset as usize;
                if next < offset_of!(FILE_ID_EXTD_DIR_INFO, FileName) + bytes
                    || !next.is_multiple_of(8)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid directory record offset",
                    ));
                }
                offset = offset.checked_add(next).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "directory offset overflow")
                })?;
            }
            if !batch.is_empty() && !emit(EnumerationEvent::Entries(batch)) {
                return Ok(true);
            }
        }
    }
}
