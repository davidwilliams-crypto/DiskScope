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

#[cfg(windows)]
fn explorer_path() -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;

    // Ask the OS for its shared installation directory. PATH, the working
    // directory and environment variables must not select the executable.
    let mut wide = vec![0u16; 32_768];
    // SAFETY: the output buffer is initialized, writable and sized in WCHARs.
    let count =
        unsafe { GetSystemWindowsDirectoryW(wide.as_mut_ptr(), wide.len() as u32) } as usize;
    if count == 0 || count >= wide.len() {
        return Err("Windows could not locate its Explorer executable.".into());
    }
    let directory = PathBuf::from(std::ffi::OsString::from_wide(&wide[..count]));
    if !directory.is_absolute() {
        return Err("Windows returned an invalid system directory.".into());
    }
    Ok(directory.join("explorer.exe"))
}

#[cfg(not(windows))]
fn explorer_path() -> Result<PathBuf, String> {
    Err("Explorer is only available on Windows.".into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

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

pub(super) fn volume_worker(results: SyncSender<NativeResult>) -> SyncSender<(u64, PathBuf)> {
    let (sender, receiver) = mpsc::sync_channel::<(u64, PathBuf)>(1);
    std::thread::Builder::new()
        .name("volume-space".into())
        .spawn(move || {
            while let Ok((generation, path)) = receiver.recv() {
                let volume = read_volume(&path);
                let _ = results.try_send(NativeResult::Volume {
                    generation,
                    path,
                    volume,
                });
            }
        })
        .expect("volume worker");
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
