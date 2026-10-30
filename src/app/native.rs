use super::*;

pub(super) fn reveal_command(
    path: &std::path::Path,
    is_dir: bool,
) -> Result<std::process::Command, String> {
    let mut command = std::process::Command::new(explorer_path()?);
    if is_dir {
        command.arg(path.as_os_str());
    } else {
        let mut argument = std::ffi::OsString::from("/select,");
        argument.push(path.as_os_str());
        command.arg(argument);
    }
    Ok(command)
}

/// The shared Windows installation directory (usually `C:\Windows`), asked
/// from the OS. PATH, the working directory and environment variables must
/// not select system executables or assets.
#[cfg(windows)]
pub(crate) fn windows_directory() -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;

    let mut wide = vec![0u16; 32_768];
    // SAFETY: the output buffer is initialized, writable and sized in WCHARs.
    let count =
        unsafe { GetSystemWindowsDirectoryW(wide.as_mut_ptr(), wide.len() as u32) } as usize;
    if count == 0 || count >= wide.len() {
        return Err("Windows could not report its installation directory.".into());
    }
    let directory = PathBuf::from(std::ffi::OsString::from_wide(&wide[..count]));
    if !directory.is_absolute() {
        return Err("Windows returned an invalid system directory.".into());
    }
    Ok(directory)
}

#[cfg(not(windows))]
pub(crate) fn windows_directory() -> Result<PathBuf, String> {
    Err("Only available on Windows.".into())
}

fn explorer_path() -> Result<PathBuf, String> {
    windows_directory()
        .map(|directory| directory.join("explorer.exe"))
        .map_err(|_| "Windows could not locate its Explorer executable.".into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn attribution_queue_bounds_pending_requests_and_drains_to_the_latest_root() {
        let (sender, receiver) = attribution_requests();
        sender.try_send((1, PathBuf::from("first"))).unwrap();
        assert!(matches!(
            sender.try_send((2, PathBuf::from("overflow"))),
            Err(mpsc::TrySendError::Full(_))
        ));
        let first = receiver.recv().unwrap();
        sender.try_send((3, PathBuf::from("latest"))).unwrap();
        assert_eq!(
            latest_attribution_request(first, &receiver),
            (3, PathBuf::from("latest"))
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn attribution_worker_returns_the_requested_generation_and_native_root_context() {
        let fixture = tempfile::tempdir().unwrap();
        let (results, receiver) = mpsc::sync_channel(1);
        let requests = attribution_worker(results);
        requests
            .try_send((41, fixture.path().to_path_buf()))
            .unwrap();
        let NativeResult::Attribution {
            generation,
            root,
            attribution,
        } = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
        else {
            panic!("unexpected native result");
        };
        assert_eq!(generation, 41);
        assert_eq!(root, fixture.path());
        assert!(attribution.finished_utc_epoch_ms.is_some());
        assert!(attribution.hostname.value.is_some());
        assert!(attribution.volume_serial.value.is_some());
    }

    #[test]
    fn explorer_uses_an_existing_absolute_system_executable_and_native_arguments() {
        use std::os::windows::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_wide(&[
            67, 58, 92, 102, 111, 108, 100, 101, 114, 92, 0xd800, 44, 32, 120,
        ]));
        for is_dir in [false, true] {
            let command = reveal_command(&path, is_dir).unwrap();
            let executable = std::path::Path::new(command.get_program());
            assert!(executable.is_absolute());
            assert!(executable.is_file());
            assert_eq!(executable.file_name().unwrap(), "explorer.exe");
            let mut expected = std::ffi::OsString::new();
            if !is_dir {
                expected.push("/select,");
            }
            expected.push(path.as_os_str());
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                vec![expected.as_os_str()]
            );
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct VolumeSpace {
    pub available: u64,
    pub total: u64,
}

#[derive(Clone, Debug)]
pub(super) struct Location {
    pub path: PathBuf,
    pub space: Option<VolumeSpace>,
}

pub(super) fn discover_locations(results: SyncSender<NativeResult>) {
    let _ = std::thread::Builder::new()
        .name("local-locations".into())
        .spawn(move || {
            let mut locations = Vec::new();
            #[cfg(windows)]
            {
                use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
                // SAFETY: no input pointers; returned bitmask names mounted drives.
                let drives = unsafe { GetLogicalDrives() };
                for letter in b'A'..=b'Z' {
                    if drives & (1 << (letter - b'A')) == 0 {
                        continue;
                    }
                    let path = PathBuf::from(format!("{}:\\", char::from(letter)));
                    let wide = [u16::from(letter), 58, 92, 0];
                    // SAFETY: native NUL-terminated path. Avoid probing network drives.
                    let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
                    if kind == 2 || kind == 3 {
                        locations.push(Location {
                            space: read_volume(&path),
                            path,
                        });
                    }
                }
            }
            let _ = results.send(NativeResult::Locations(locations));
        });
}

pub(super) fn volume_worker(results: SyncSender<NativeResult>) -> SyncSender<(u64, PathBuf)> {
    let (sender, receiver) = mpsc::sync_channel::<(u64, PathBuf)>(1);
    std::thread::Builder::new()
        .name("volume-space".into())
        .spawn(move || {
            while let Ok((generation, path)) = receiver.recv() {
                let volume = read_volume(&path);
                // Block rather than drop: the UI drains a few results per
                // frame, and a lost result would leave the volume line empty.
                if results
                    .send(NativeResult::Volume {
                        generation,
                        path,
                        volume,
                        measured_at: std::time::SystemTime::now(),
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .expect("volume worker");
    sender
}

type AttributionRequest = (u64, PathBuf);

fn attribution_requests() -> (
    SyncSender<AttributionRequest>,
    mpsc::Receiver<AttributionRequest>,
) {
    mpsc::sync_channel(1)
}

fn latest_attribution_request(
    mut latest: AttributionRequest,
    receiver: &mpsc::Receiver<AttributionRequest>,
) -> AttributionRequest {
    while let Ok(request) = receiver.try_recv() {
        latest = request;
    }
    latest
}

/// One provider call in flight and at most one queued request. A stalled native
/// provider cannot create more threads as the user navigates or rescans.
pub(super) fn attribution_worker(
    results: SyncSender<NativeResult>,
) -> SyncSender<AttributionRequest> {
    let (sender, receiver) = attribution_requests();
    std::thread::Builder::new()
        .name("endpoint-attribution".into())
        .spawn(move || {
            while let Ok(first) = receiver.recv() {
                let (generation, root) = latest_attribution_request(first, &receiver);
                let attribution = diskscope::identity::collect(&root);
                if results
                    .send(NativeResult::Attribution {
                        generation,
                        root,
                        attribution: Box::new(attribution),
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .expect("attribution worker");
    sender
}
#[cfg(windows)]
fn read_volume(path: &std::path::Path) -> Option<VolumeSpace> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut available, mut total, mut free) = (0, 0, 0);
    // SAFETY: NUL-terminated native path and valid writable integer pointers.
    let success = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            &mut total,
            &mut free,
        )
    };
    (success != 0).then_some(VolumeSpace { available, total })
}
#[cfg(not(windows))]
fn read_volume(_path: &std::path::Path) -> Option<VolumeSpace> {
    None
}
