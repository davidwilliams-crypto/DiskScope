//! Read-only, point-in-time display attribution. Never an action target.
use std::path::Path;

pub const MAX_IDENTITY_BYTES: usize = 512;

#[derive(Clone, Debug)]
pub struct Observation {
    pub value: Option<String>,
    pub source: &'static str,
    pub observed_utc_epoch_ms: Option<u64>,
    pub unavailable_reason: Option<&'static str>,
    pub win32_error: Option<u32>,
    pub truncated: bool,
}

impl Observation {
    fn unavailable(source: &'static str, reason: &'static str, code: Option<u32>) -> Self {
        Self {
            value: None,
            source,
            observed_utc_epoch_ms: crate::evidence::utc_now_epoch_ms(),
            unavailable_reason: Some(reason),
            win32_error: code,
            truncated: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Attribution {
    pub hostname: Observation,
    pub volume_guid: Observation,
    pub volume_serial: Observation,
    pub filesystem: Observation,
    pub started_utc_epoch_ms: Option<u64>,
    pub finished_utc_epoch_ms: Option<u64>,
    pub elapsed_ms: f64,
}

/// Call on a worker thread: native providers can block. Report construction does
/// not call this function. No registry, machine GUID or volume label is read.
/// A remote root can involve filesystem-provider/network I/O.
#[cfg(not(windows))]
pub fn collect(_root: &Path) -> Attribution {
    let absent = || Observation::unavailable("native attribution", "unsupported_platform", None);
    Attribution {
        hostname: absent(),
        volume_guid: absent(),
        volume_serial: absent(),
        filesystem: absent(),
        started_utc_epoch_ms: None,
        finished_utc_epoch_ms: None,
        elapsed_ms: 0.0,
    }
}

#[cfg(windows)]
pub fn collect(root: &Path) -> Attribution {
    native::collect(root)
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{os::windows::ffi::OsStrExt, ptr, time::Instant};
    use windows_sys::Win32::{
        Foundation::GetLastError,
        Storage::FileSystem::{
            GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
        },
        System::{
            Diagnostics::Debug::{GetThreadErrorMode, SEM_FAILCRITICALERRORS, SetThreadErrorMode},
            SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW},
        },
    };

    const HOST_SOURCE: &str = "GetComputerNameExW(ComputerNameDnsHostname)";
    const GUID_SOURCE: &str = "GetVolumePathNameW -> GetVolumeNameForVolumeMountPointW";
    const INFO_SOURCE: &str = "GetVolumePathNameW -> GetVolumeInformationW";

    struct VolumeErrorMode(u32);
    fn suppress_volume_errors() -> Option<VolumeErrorMode> {
        // SAFETY: Only this worker's runtime error handling is changed. Preserve
        // inherited flags; unavailable suppression refuses provider calls.
        let current = unsafe { GetThreadErrorMode() };
        let mut previous = 0;
        if unsafe { SetThreadErrorMode(current | SEM_FAILCRITICALERRORS, &mut previous) } == 0 {
            None
        } else {
            Some(VolumeErrorMode(previous))
        }
    }
    impl Drop for VolumeErrorMode {
        fn drop(&mut self) {
            // SAFETY: Restore only this worker's preceding mode, including on
            // provider failure/early return. No process/global setting changes.
            unsafe {
                SetThreadErrorMode(self.0, ptr::null_mut());
            }
        }
    }

    fn failed(source: &'static str) -> Observation {
        // SAFETY: Thread-local diagnostic only; called immediately after failure.
        let code = unsafe { GetLastError() };
        Observation::unavailable(source, "native_call_failed", Some(code))
    }

    fn available(source: &'static str, mut value: String) -> Observation {
        let truncated = value.len() > MAX_IDENTITY_BYTES;
        if truncated {
            let mut end = MAX_IDENTITY_BYTES;
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            value.truncate(end);
        }
        Observation {
            value: Some(value),
            source,
            observed_utc_epoch_ms: crate::evidence::utc_now_epoch_ms(),
            unavailable_reason: None,
            win32_error: None,
            truncated,
        }
    }

    fn wide_value(source: &'static str, buffer: &[u16]) -> Observation {
        match buffer.iter().position(|unit| *unit == 0) {
            Some(0) => Observation::unavailable(source, "empty_native_value", None),
            Some(end) => available(source, String::from_utf16_lossy(&buffer[..end])),
            None => Observation::unavailable(source, "unterminated_native_value", None),
        }
    }

    fn hostname() -> Observation {
        // DNS hostname maximum is 63 characters; bounded buffer permits provider
        // variants without an unbounded required-size allocation or retry loop.
        let mut buffer = [0u16; 256];
        let mut count = buffer.len() as u32;
        // SAFETY: Buffer/count are valid for their declared writable capacities.
        if unsafe { GetComputerNameExW(ComputerNameDnsHostname, buffer.as_mut_ptr(), &mut count) }
            == 0
        {
            return failed(HOST_SOURCE);
        }
        wide_value(HOST_SOURCE, &buffer)
    }

    fn unavailable_volume(reason: &'static str) -> (Observation, Observation, Observation) {
        (
            Observation::unavailable(GUID_SOURCE, reason, None),
            Observation::unavailable(INFO_SOURCE, reason, None),
            Observation::unavailable(INFO_SOURCE, reason, None),
        )
    }

    fn volume(root: &Path) -> (Observation, Observation, Observation) {
        let Some(_error_mode) = suppress_volume_errors() else {
            return unavailable_volume("critical_error_dialog_suppression_failed");
        };
        let Ok(absolute) = std::path::absolute(root) else {
            return unavailable_volume("invalid_path");
        };
        let mut native = Vec::with_capacity(260);
        for unit in absolute.as_os_str().encode_wide() {
            if unit == 0 {
                return unavailable_volume("invalid_path");
            }
            if native.len() >= 32_767 {
                return unavailable_volume("path_exceeds_native_probe_limit");
            }
            native.push(unit);
        }
        native.push(0);
        // A fixed maximum Win32 path buffer; never export the mount path, which
        // can contain usernames or other identifiers on mounted-folder volumes.
        let mut mount = [0u16; 32_768];
        // SAFETY: Input is NUL-terminated and output is writable for capacity.
        if unsafe { GetVolumePathNameW(native.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) }
            == 0
        {
            let error = failed("GetVolumePathNameW");
            return (error.clone(), error.clone(), error);
        }
        if !mount.contains(&0) || mount[0] == 0 {
            return unavailable_volume("invalid_native_mount_path");
        }
        let mut guid = [0u16; 64];
        // SAFETY: Mount is NUL-terminated; GUID output is writable for capacity.
        let guid = if unsafe {
            GetVolumeNameForVolumeMountPointW(mount.as_ptr(), guid.as_mut_ptr(), guid.len() as u32)
        } == 0
        {
            failed(GUID_SOURCE)
        } else {
            wide_value(GUID_SOURCE, &guid)
        };
        let mut filesystem = [0u16; 64];
        let mut serial = 0;
        // SAFETY: Mount is NUL-terminated; outputs are valid, unused outputs are
        // documented nullable pointers. No volume label or write handle requested.
        if unsafe {
            GetVolumeInformationW(
                mount.as_ptr(),
                ptr::null_mut(),
                0,
                &mut serial,
                ptr::null_mut(),
                ptr::null_mut(),
                filesystem.as_mut_ptr(),
                filesystem.len() as u32,
            )
        } == 0
        {
            let error = failed(INFO_SOURCE);
            (guid, error.clone(), error)
        } else {
            (
                guid,
                available(INFO_SOURCE, format!("{serial:08X}")),
                wide_value(INFO_SOURCE, &filesystem),
            )
        }
    }

    pub(super) fn collect(root: &Path) -> Attribution {
        let started_utc_epoch_ms = crate::evidence::utc_now_epoch_ms();
        let started = Instant::now();
        let hostname = hostname();
        let (volume_guid, volume_serial, filesystem) = volume(root);
        Attribution {
            hostname,
            volume_guid,
            volume_serial,
            filesystem,
            started_utc_epoch_ms,
            finished_utc_epoch_ms: crate::evidence::utc_now_epoch_ms(),
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows_sys::Win32::System::Diagnostics::Debug::{
            GetThreadErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetThreadErrorMode,
        };

        #[test]
        fn volume_probe_suppresses_critical_error_dialogs_and_restores_thread_mode() {
            struct Restore(u32);
            impl Drop for Restore {
                fn drop(&mut self) {
                    // SAFETY: Restore only this test thread's original mode.
                    unsafe {
                        SetThreadErrorMode(self.0, ptr::null_mut());
                    }
                }
            }
            // SAFETY: These APIs alter only the current worker/test thread mode.
            let original = unsafe { GetThreadErrorMode() };
            let _restore = Restore(original);
            assert_ne!(
                unsafe { SetThreadErrorMode(SEM_NOGPFAULTERRORBOX, ptr::null_mut()) },
                0
            );
            {
                let _guard = suppress_volume_errors().expect("thread-local suppression");
                let during = unsafe { GetThreadErrorMode() };
                assert_ne!(during & SEM_FAILCRITICALERRORS, 0);
                assert_ne!(during & SEM_NOGPFAULTERRORBOX, 0);
            }
            assert_eq!(unsafe { GetThreadErrorMode() }, SEM_NOGPFAULTERRORBOX);
            let found = collect(Path::new(
                "\\\\?\\Volume{00000000-0000-0000-0000-000000000000}\\",
            ));
            assert!(found.volume_serial.value.is_none());
            assert!(found.volume_serial.unavailable_reason.is_some());
            assert_eq!(unsafe { GetThreadErrorMode() }, SEM_NOGPFAULTERRORBOX);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn native_local_fixture_has_timestamped_display_context() {
        let fixture = tempfile::tempdir().unwrap();
        let found = collect(fixture.path());
        assert!(found.hostname.value.as_ref().is_some_and(|v| !v.is_empty()));
        assert_eq!(
            found.hostname.source,
            "GetComputerNameExW(ComputerNameDnsHostname)"
        );
        assert!(
            found
                .volume_guid
                .value
                .as_ref()
                .is_some_and(|v| v.starts_with("\\\\?\\Volume{"))
        );
        assert_eq!(found.volume_serial.value.as_ref().map(String::len), Some(8));
        assert!(
            found
                .filesystem
                .value
                .as_ref()
                .is_some_and(|v| !v.is_empty())
        );
        assert!(found.started_utc_epoch_ms <= found.finished_utc_epoch_ms);
        assert!(found.finished_utc_epoch_ms.is_some());
    }

    #[cfg(windows)]
    #[test]
    fn embedded_nul_path_cannot_silently_select_another_volume() {
        let found = collect(Path::new("C:\\safe\0different"));
        assert!(found.volume_guid.value.is_none());
        assert_eq!(found.volume_guid.unavailable_reason, Some("invalid_path"));
        assert!(found.hostname.value.is_some());
    }
}
