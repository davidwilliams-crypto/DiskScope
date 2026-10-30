# DiskScope 0.3.1 deployment test build

This build packages the reliability and investigation UX changes described in
[validation](validation-investigation-ux-2026-10-30.md). It is a local Windows
x64 preview, not a published GitHub release. Packages are unsigned.

## Installer

Use `DiskScope-0.3.1-windows-x64-setup.exe`. Setup installs for the Windows
account that runs it, defaults to `%LOCALAPPDATA%\Programs\DiskScope`, and
does not launch the application or start a scan. Close an existing DiskScope
window before upgrading. Run it in the intended user's account.

Silent installation supports:

```powershell
$installer = 'C:\deployment\DiskScope-0.3.1-windows-x64-setup.exe'
$process = Start-Process -FilePath $installer -ArgumentList '/S /NoShortcuts' -WindowStyle Hidden -Wait -PassThru
$process.ExitCode
```

This is per-user installation; running it as SYSTEM installs into the service
account's profile rather than the signed-in user's profile. The installer is
not an all-users deployment package.

## ScreenConnect portable deployment

Transfer `DiskScope-0.3.1-windows-x64-portable.zip` to the endpoint. Extract the
whole archive to a writable local folder. Inside `DiskScope-0.3.1-portable`,
launch `DiskScope.exe` through the user's interactive desktop session.
No installation or separate Visual C++ runtime is required. Retain the
included notices and license files when redistributing the tool.

Launching the GUI only opens DiskScope; choose a drive/folder and Scan to begin.
For a background, read-only scan through a command session, use the CLI:

```powershell
$exe = 'C:\deployment\DiskScope-0.3.1-portable\DiskScope.exe'
$process = Start-Process -FilePath $exe -ArgumentList '--scan "C:\Users" --json "C:\deployment\scan.json" --backend normal' -WindowStyle Hidden -Wait -PassThru
$process.ExitCode
```

Run under an account with the intended file access. Exit codes: 0 complete,
1 failure, 2 invalid arguments, 3 timeout, 4 partial results. A service command
does not by itself display a GUI in the signed-in user's desktop session.

## Verification and data

`SHA256SUMS.txt` beside the installer/ZIP covers release files; the extracted
folder has its own file manifest and `BUILD_PROVENANCE.json`. Preferences and
GUI scan history use the current Windows profile even in portable mode.
Administrator/unknown-access GUI sessions do not persist them. CLI scans do
not write history. Windows 10+ x64 and OpenGL 2.0 graphics support are required
for the GUI. Verify behavior on a pilot endpoint before broader deployment.
