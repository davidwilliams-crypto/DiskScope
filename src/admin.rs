//! Explicit, user-initiated Windows elevation. No startup elevation or privilege changes.
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElevationOutcome {
    /// Windows supplied a live process handle and a nonzero process ID.
    /// This does not mean the new window has finished initialization.
    Launched {
        process_id: u32,
    },
    Cancelled,
}

/// Read the current process token, rather than trusting command-line state.
pub fn is_elevated() -> Result<bool, String> {
    #[cfg(windows)]
    {
        windows::is_elevated()
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

/// Request Windows UAC consent. Call on a dedicated background thread.
/// An error or cancelled prompt must leave the caller's window open.
pub fn restart_elevated(root: Option<&Path>) -> Result<ElevationOutcome, String> {
    #[cfg(windows)]
    {
        windows::restart_elevated(root)
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        Err("Administrator relaunch is only available on Windows.".into())
    }
}

#[cfg(any(windows, test))]
fn quote_windows_argument(argument: &[u16]) -> Result<Vec<u16>, String> {
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    if argument.contains(&0) {
        return Err("The scan folder contains a NUL character.".into());
    }
    // Microsoft CRT rules: backslashes are literal except before a quote.
    // In particular, C:\ must become "C:\\" rather than escaping the closing quote.
    let mut quoted = Vec::with_capacity(argument.len() + 2);
    quoted.push(QUOTE);
    let mut backslashes = 0;
    for &unit in argument {
        if unit == BACKSLASH {
            backslashes += 1;
            continue;
        }
        let escaped_count = if unit == QUOTE {
            backslashes * 2 + 1
        } else {
            backslashes
        };
        quoted.extend(std::iter::repeat_n(BACKSLASH, escaped_count));
        quoted.push(unit);
        backslashes = 0;
    }
    quoted.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2));
    quoted.push(QUOTE);
    Ok(quoted)
}

#[cfg(windows)]
fn shell_launch_failure(code: u32) -> Result<ElevationOutcome, String> {
    use windows_sys::Win32::Foundation::ERROR_CANCELLED;
    if code == ERROR_CANCELLED {
        Ok(ElevationOutcome::Cancelled)
    } else {
        Err(windows_error("Administrator relaunch failed", code))
    }
}

#[cfg(windows)]
fn windows_error(action: &str, code: u32) -> String {
    if code == 0 {
        format!("{action}. Windows did not report an error code.")
    } else {
        format!(
            "{action}: {}",
            std::io::Error::from_raw_os_error(code as i32)
        )
    }
}

#[cfg(windows)]
mod windows {
    use super::{
        ElevationOutcome, Path, quote_windows_argument, shell_launch_failure, windows_error,
    };
    use std::{ffi::OsStr, mem::size_of, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE},
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::{
            Com::{
                COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
            },
            Threading::{GetCurrentProcess, GetExitCodeProcess, GetProcessId, OpenProcessToken},
        },
        UI::{
            Shell::{
                SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
                ShellExecuteExW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    };

    struct OwnedNativeHandle(HANDLE);

    impl OwnedNativeHandle {
        fn from_returned(handle: HANDLE) -> Option<Self> {
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                None
            } else {
                Some(Self(handle))
            }
        }
    }

    impl Drop for OwnedNativeHandle {
        fn drop(&mut self) {
            // The handle is owned and valid. Closing it never terminates its process.
            unsafe { CloseHandle(self.0) };
        }
    }

    struct ComApartment;

    impl ComApartment {
        fn initialize() -> Result<Self, String> {
            // Shell extensions can require STA. This call stays on the dedicated worker.
            let result = unsafe {
                CoInitializeEx(
                    ptr::null(),
                    (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                )
            };
            if result < 0 {
                Err(format!(
                    "Could not initialize the Windows Shell on this thread (HRESULT 0x{:08X}).",
                    result as u32
                ))
            } else {
                Ok(Self)
            }
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            // Balance both S_OK and S_FALSE on the thread that initialized COM.
            unsafe { CoUninitialize() };
        }
    }

    fn nul_terminated(value: &OsStr, description: &str) -> Result<Vec<u16>, String> {
        let mut wide: Vec<u16> = value.encode_wide().collect();
        if wide.contains(&0) {
            return Err(format!("The {description} contains a NUL character."));
        }
        wide.push(0);
        Ok(wide)
    }

    pub(super) fn is_elevated() -> Result<bool, String> {
        let mut token = ptr::null_mut();
        // Query only. No token privilege is enabled and the pseudo process handle is not closed.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(windows_error("Could not query the process token", unsafe {
                GetLastError()
            }));
        }
        let token = OwnedNativeHandle::from_returned(token)
            .ok_or("Windows did not return a valid process token handle.")?;
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0;
        let expected_size = size_of::<TOKEN_ELEVATION>() as u32;
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                expected_size,
                &mut returned,
            )
        } == 0
        {
            return Err(windows_error(
                "Could not read the process elevation",
                unsafe { GetLastError() },
            ));
        }
        if returned != expected_size {
            return Err("Windows returned an unexpected process elevation token size.".into());
        }
        Ok(elevation.TokenIsElevated != 0)
    }

    pub(super) fn restart_elevated(root: Option<&Path>) -> Result<ElevationOutcome, String> {
        let executable = std::env::current_exe().map_err(|error| {
            format!("Could not locate DiskScope for administrator relaunch: {error}")
        })?;
        let directory = executable
            .parent()
            .ok_or("The DiskScope executable has no parent folder.")?;
        let executable = nul_terminated(executable.as_os_str(), "executable path")?;
        let directory = nul_terminated(directory.as_os_str(), "executable folder")?;
        let parameters = root
            .map(|root| {
                // Resolve relative paths before changing the child working directory.
                // absolute/encode_wide preserve Windows paths without a lossy UTF-8 conversion.
                let absolute = std::path::absolute(root)
                    .map_err(|error| format!("Could not resolve the scan folder: {error}"))?;
                let units: Vec<u16> = absolute.as_os_str().encode_wide().collect();
                let mut quoted = quote_windows_argument(&units)?;
                quoted.push(0);
                Ok::<_, String>(quoted)
            })
            .transpose()?;
        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let _apartment = ComApartment::initialize()?;
        let mut request = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            // NOASYNC completes Shell work before this worker exits. NO_UI suppresses
            // Shell error dialogs, while the Windows UAC consent prompt still appears.
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: verb.as_ptr(),
            lpFile: executable.as_ptr(),
            lpParameters: parameters
                .as_ref()
                .map_or(ptr::null(), |wide| wide.as_ptr()),
            lpDirectory: directory.as_ptr(),
            nShow: SW_SHOWNORMAL,
            ..Default::default()
        };
        // Native argument buffers and COM apartment remain alive for this synchronous call.
        let succeeded = unsafe { ShellExecuteExW(&mut request) } != 0;
        let error = if succeeded {
            0
        } else {
            unsafe { GetLastError() }
        };
        let process = OwnedNativeHandle::from_returned(request.hProcess);
        if !succeeded {
            return shell_launch_failure(error);
        }
        let process = process.ok_or(
            "Windows accepted the relaunch but did not return a process handle. Keep this window open and check for another DiskScope window.",
        )?;
        let process_id = unsafe { GetProcessId(process.0) };
        if process_id == 0 {
            return Err(windows_error(
                "Could not verify the new DiskScope process",
                unsafe { GetLastError() },
            ));
        }
        let mut exit_code = 0;
        if unsafe { GetExitCodeProcess(process.0, &mut exit_code) } == 0 {
            return Err(windows_error(
                "Could not check the new DiskScope process",
                unsafe { GetLastError() },
            ));
        }
        if exit_code != STILL_ACTIVE as u32 {
            return Err(format!(
                "The new DiskScope process exited before the administrator handoff (exit code {exit_code})."
            ));
        }
        Ok(ElevationOutcome::Launched { process_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn command_argument_is_always_quoted_including_empty_and_spaces() {
        for (source, expected) in [
            ("", "\"\""),
            ("C:\\folder", "\"C:\\folder\""),
            ("C:\\a b", "\"C:\\a b\""),
            ("a\tb", "\"a\tb\""),
        ] {
            assert_eq!(
                quote_windows_argument(&wide(source)).unwrap(),
                wide(expected)
            );
        }
    }

    #[test]
    fn trailing_backslashes_are_doubled_before_the_closing_quote() {
        assert_eq!(
            quote_windows_argument(&wide("C:\\")).unwrap(),
            wide("\"C:\\\\\"")
        );
        assert_eq!(
            quote_windows_argument(&wide("C:\\with spaces\\")).unwrap(),
            wide("\"C:\\with spaces\\\\\"")
        );
        assert_eq!(
            quote_windows_argument(&wide("\\\\server\\share\\")).unwrap(),
            wide("\"\\\\server\\share\\\\\"")
        );
    }

    #[test]
    fn quotes_and_preceding_backslashes_follow_the_crt_argument_rules() {
        assert_eq!(
            quote_windows_argument(&wide("a\"b")).unwrap(),
            wide("\"a\\\"b\"")
        );
        assert_eq!(
            quote_windows_argument(&wide("a\\\"b")).unwrap(),
            wide("\"a\\\\\\\"b\"")
        );
    }

    #[test]
    fn native_utf16_units_are_preserved_without_a_lossy_string_conversion() {
        let units = [
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xd800,
            b' ' as u16,
            0xdc00,
        ];
        let mut expected = vec![b'"' as u16];
        expected.extend(units);
        expected.push(b'"' as u16);
        assert_eq!(quote_windows_argument(&units).unwrap(), expected);
    }

    #[test]
    fn embedded_nul_is_rejected_instead_of_truncating_a_path() {
        assert!(quote_windows_argument(&[b'C' as u16, 0, b'x' as u16]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn current_process_token_is_queryable_without_requesting_elevation() {
        eprintln!("Current process elevated: {}", is_elevated().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn uac_cancellation_is_distinct_from_a_launch_error() {
        use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_CANCELLED};
        assert_eq!(
            shell_launch_failure(ERROR_CANCELLED),
            Ok(ElevationOutcome::Cancelled)
        );
        assert!(shell_launch_failure(ERROR_ACCESS_DENIED).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn missing_windows_error_code_is_still_reported_as_a_launch_failure() {
        let error = shell_launch_failure(0).unwrap_err();
        assert!(!error.contains("operation completed successfully"));
        assert!(error.contains("Windows did not report an error code"));
    }
}
