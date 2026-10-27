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

/// How the current process token relates to UAC.
///
/// `TokenIsElevated` alone cannot tell the two elevated cases apart, yet they
/// need different explanations: only [`ElevationKind::Elevated`] has a limited
/// twin of the same session, so only there does "open DiskScope normally"
/// mean anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElevationKind {
    /// The token is not elevated.
    Standard,
    /// Elevated through UAC consent: a split token whose limited twin exists.
    Elevated,
    /// Elevated without a UAC split. UAC is turned off (`EnableLUA=0`), or
    /// Windows does not apply Admin Approval Mode to the account (the built-in
    /// Administrator, service accounts). There is no standard window to
    /// switch to.
    AlwaysElevated,
}

/// `TOKEN_ELEVATION_TYPE_Default` from winnt.h: the token has no linked
/// limited token. Held as a plain number so the mapping below is testable on
/// every platform; a Windows-only test checks it against windows-sys.
#[cfg(any(windows, test))]
const TOKEN_ELEVATION_TYPE_DEFAULT: i32 = 1;

/// Combines `TokenIsElevated` with the raw `TOKEN_ELEVATION_TYPE`.
///
/// Only the positive evidence of an elevated token without a linked limited
/// token is reported as [`ElevationKind::AlwaysElevated`], because that is the
/// one case where the UI tells the user UAC is not in play. Every other
/// elevated token (the consented `Full` type and anything unexpected) keeps
/// the generic administrator description.
#[cfg(any(windows, test))]
fn classify_elevation(is_elevated: bool, token_elevation_type: i32) -> ElevationKind {
    if !is_elevated {
        ElevationKind::Standard
    } else if token_elevation_type == TOKEN_ELEVATION_TYPE_DEFAULT {
        ElevationKind::AlwaysElevated
    } else {
        ElevationKind::Elevated
    }
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

/// Read how the current process token came to be elevated, if it is.
/// This only explains the session to the user; it never changes what the
/// session is allowed to do.
pub fn elevation_kind() -> Result<ElevationKind, String> {
    #[cfg(windows)]
    {
        windows::elevation_kind()
    }
    #[cfg(not(windows))]
    {
        Ok(ElevationKind::Standard)
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
        ElevationKind, ElevationOutcome, Path, classify_elevation, quote_windows_argument,
        shell_launch_failure, windows_error,
    };
    use std::{ffi::OsStr, mem::size_of, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE},
        Security::{
            GetTokenInformation, TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE, TOKEN_INFORMATION_CLASS,
            TOKEN_QUERY, TokenElevation, TokenElevationType,
        },
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

    /// Opens the current process token for querying only. No token privilege
    /// is enabled.
    fn open_process_token() -> Result<OwnedNativeHandle, String> {
        let mut token = ptr::null_mut();
        // Query only. The pseudo process handle needs no closing; the token
        // handle is wrapped at once so it is closed on every path.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(windows_error("Could not query the process token", unsafe {
                GetLastError()
            }));
        }
        OwnedNativeHandle::from_returned(token)
            .ok_or_else(|| "Windows did not return a valid process token handle.".into())
    }

    /// Reads one fixed-size token information class into a `T`.
    ///
    /// `T` must be a plain `repr(C)` value for which every bit pattern is
    /// valid, and `class` must be a class whose output is exactly one `T`.
    /// The two callers pass `TOKEN_ELEVATION` for `TokenElevation` and
    /// `TOKEN_ELEVATION_TYPE` (an `i32`) for `TokenElevationType`.
    /// `what` names the value in error messages.
    fn query_token<T: Copy>(
        token: &OwnedNativeHandle,
        class: TOKEN_INFORMATION_CLASS,
        mut value: T,
        what: &str,
    ) -> Result<T, String> {
        let mut returned = 0;
        let expected_size = size_of::<T>() as u32;
        // SAFETY: `token` is a live token handle with TOKEN_QUERY access and
        // `value` is a writable, initialized `T` of exactly `expected_size`
        // bytes that outlives the call.
        let succeeded = unsafe {
            GetTokenInformation(
                token.0,
                class,
                (&mut value as *mut T).cast(),
                expected_size,
                &mut returned,
            )
        } != 0;
        if !succeeded {
            return Err(windows_error(
                &format!("Could not read the {what}"),
                unsafe { GetLastError() },
            ));
        }
        if returned != expected_size {
            return Err(format!("Windows returned an unexpected {what} token size."));
        }
        Ok(value)
    }

    fn token_is_elevated(token: &OwnedNativeHandle) -> Result<bool, String> {
        let elevation = query_token(
            token,
            TokenElevation,
            TOKEN_ELEVATION { TokenIsElevated: 0 },
            "process elevation",
        )?;
        Ok(elevation.TokenIsElevated != 0)
    }

    pub(super) fn is_elevated() -> Result<bool, String> {
        token_is_elevated(&open_process_token()?)
    }

    pub(super) fn elevation_kind() -> Result<ElevationKind, String> {
        let token = open_process_token()?;
        let elevated = token_is_elevated(&token)?;
        if !elevated {
            // The type only matters for an elevated token, so a standard
            // session needs no second query.
            return Ok(ElevationKind::Standard);
        }
        let token_type: TOKEN_ELEVATION_TYPE =
            query_token(&token, TokenElevationType, 0, "process elevation type")?;
        Ok(classify_elevation(elevated, token_type))
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

    #[test]
    fn elevation_kind_needs_both_the_elevated_flag_and_the_missing_limited_twin() {
        // TOKEN_ELEVATION_TYPE: 1 Default, 2 Full, 3 Limited (winnt.h).
        const DEFAULT: i32 = 1;
        const FULL: i32 = 2;
        const LIMITED: i32 = 3;
        // A non-elevated token is Standard whatever its type says; this is
        // the ordinary UAC administrator running unelevated (Limited).
        for token_type in [0, DEFAULT, FULL, LIMITED, 99] {
            assert_eq!(
                classify_elevation(false, token_type),
                ElevationKind::Standard,
                "type {token_type}"
            );
        }
        // Run as administrator with UAC consent.
        assert_eq!(classify_elevation(true, FULL), ElevationKind::Elevated);
        // Elevated with no linked limited token: UAC off or exempt account.
        assert_eq!(
            classify_elevation(true, DEFAULT),
            ElevationKind::AlwaysElevated
        );
        // Contradictory or unknown types never claim that UAC is off.
        for token_type in [0, LIMITED, 99, -1] {
            assert_eq!(
                classify_elevation(true, token_type),
                ElevationKind::Elevated,
                "type {token_type}"
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_hosts_are_never_elevated() {
        assert_eq!(elevation_kind(), Ok(ElevationKind::Standard));
        assert_eq!(is_elevated(), Ok(false));
    }

    #[cfg(windows)]
    #[test]
    fn raw_token_type_constant_matches_windows_sys() {
        use windows_sys::Win32::Security::TokenElevationTypeDefault;
        assert_eq!(TOKEN_ELEVATION_TYPE_DEFAULT, TokenElevationTypeDefault);
    }

    #[cfg(windows)]
    #[test]
    fn elevation_kind_agrees_with_the_elevated_flag() {
        let elevated = is_elevated().unwrap();
        let kind = elevation_kind().unwrap();
        eprintln!("Current process elevation kind: {kind:?}");
        assert_eq!(kind == ElevationKind::Standard, !elevated);
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
