# DiskScope

A native Windows disk usage analyzer written in Rust with egui/eframe. The **0.3.0 preview** adds explicit administrator relaunch and confirmed single-file actions to the responsive interface and scanner. The original implementation was informed by a focused REA investigation of WinDirStat 2.9.3.

## Run

Download the [Windows x64 installer](https://github.com/davidwilliams-crypto/DiskScope/releases/download/v0.3.0/DiskScope-0.3.0-windows-x64-setup.exe) or [portable ZIP](https://github.com/davidwilliams-crypto/DiskScope/releases/download/v0.3.0/DiskScope-0.3.0-windows-x64-portable.zip) from the [0.3.0 preview release](https://github.com/davidwilliams-crypto/DiskScope/releases/tag/v0.3.0). Install the setup executable, or extract the entire ZIP and open `DiskScope.exe`. Both contain the same application. Windows 10 or later on an Intel/AMD x64 PC and an OpenGL 2.0-capable graphics driver are required; a separate Visual C++ runtime installation is not needed. The builds are unsigned; Windows may display a publisher or SmartScreen prompt.

Setup installs for the current Windows account, offers Start menu and desktop shortcuts, and registers an uninstaller. Uninstall preserves scan history, preferences, and unrelated files. The portable version requires no installation; it still saves history and preferences in your Windows profile. See [packaging and verification](docs/packaging.md) for build commands and package details.

Open `DiskScope.exe`, choose a folder or drive, then select **Scan**. Opening the app alone does not start a scan. A local package build places the executable at `dist/0.3.0/DiskScope.exe`.

- Overview with factual folder/type insights and real volume capacity when available.
- Connected Explorer, Largest and Types with explicit type/size/date filters and removable chips.
- Treemap and experimental icicle navigation, shared selection and folder breakdowns.
- Up to four pinned comparisons with aligned sizes; Back/Forward and Ctrl+K commands.
- Local complete-scan history and comparisons of growth, additions, removals and confidently identified moves.
- Dark, warm light and high-contrast themes, 100–150% text scaling, reduced motion and accessibility integration.
- Background scanning with immediate Stop acknowledgement, retained partial results, stable display and pending updates.
- Separate logical and unique allocated size accounting; incomplete allocation is shown as unknown.
- Native folder picker, Explorer reveal and copy path.
- Explicit **Run as administrator** with Windows UAC consent; cancelling keeps the current window open.
- Recycle Bin in standard mode and separately confirmed permanent deletion of selected local regular files. Before recycling, DiskScope checks the drive type, the Recycle Bin policy, the drive's "remove files immediately" setting and the file size against the drive's Recycle Bin limit; if Windows would delete the file instead of recycling it, DiskScope stops and nothing is changed. Permanent deletion is a separate action that requires a checkbox and typing `DELETE`; folders and links are excluded. File identity and metadata are rechecked before acting. No ACL, ownership or read-only override is attempted.

Directory junctions and other directory reparse points are displayed but not followed. Logical totals count each file entry, including hardlink aliases. Unique allocation deduplicates file identity. Alternate data streams and filesystem metadata are outside these totals. Permission/read failures retain partial results with an explicit error state; a scan whose root is itself a junction or link fails with an explicit error instead of reporting an empty complete scan. Allocation is tracked per folder: a folder whose own listing or any descendant failed shows unknown allocation, while unaffected sibling folders keep theirs.

GUI scan history contains metadata only and defaults to `%LOCALAPPDATA%\DiskScope\history`. An explicit `DISKSCOPE_HISTORY_DIR` override supports isolated testing or another storage location; a network path or redirected folder can store metadata off this PC. Changes displays the actual location and provides a clear-history control. History retains up to five complete error-free scans, at most 1,000,000 nodes/128 MiB each, within a 768 MiB budget including staging. A temporary retention lock is retried before the next save. CLI scans do not write history. One app window owns a history directory at a time; another window reports history unavailable. Move inference is limited by file identity/hardlinks; old modification dates do not imply unwanted files.

Administrator and unknown-access sessions do not save preferences or history. Administrator access can expose more files; it does not guarantee faster normal scanning or bypass Windows file permissions and locks. Recycle Bin actions require standard mode. Administrator permanent deletion uses the checked file handle to avoid redirecting the action through a replaced path. Removing a hard-link alias may reclaim no space. In administrator mode the permanent action is the only file action.

Recycling pins ordinary ancestor directories against rename and reparse-point writers during execution and fails if those guards cannot be obtained. Confirmation dialogs hold no file locks. Recycling still delegates the final leaf-name operation to Windows Shell; it does not provide the same atomic object guarantee as permanent deletion by handle. Windows, not DiskScope, makes the final Recycle Bin decision, and DiskScope's checks are best-effort: they refuse only on positive evidence and never block when a setting cannot be read. If Windows still asks in its own dialog whether to delete a file permanently, choose No; DiskScope never answers that dialog. If a file is removed without a Recycle Bin copy, or its state cannot be confirmed, DiskScope says so plainly and rescans.

The optional **NTFS preview** attempts a read-only raw metadata scan for exact local drive roots such as `C:\`. It never requests elevation automatically and falls back to the normal scanner when unsupported or denied. Raw-volume accounting parity has not been validated on this machine. Leave the preview disabled for routine use.

## Build

Windows x64, Rust stable with the MSVC target, Visual Studio C++ Build Tools and a Windows SDK are required. Dependencies are locked in `Cargo.lock`.

```powershell
cargo test --locked
cargo build --release --locked
.\target\release\diskscope.exe
```

The optional `scripts/dev.ps1` helper expects a local toolchain under the ignored `.tools` directory. This toolchain is not included in Git; use the standard Cargo commands above with your installed Rust toolchain, or provision the local toolchain before using this helper:

```powershell
.\scripts\dev.ps1 -CargoArgs @('test','--locked')
.\scripts\dev.ps1 -CargoArgs @('build','--release','--locked')
```

`scripts/build.ps1` builds and packages to `dist/<version>` (currently `dist/0.3.0`), reading the version from `Cargo.toml`; notice collection also needs Python 3.9 or newer and, for the standard-library notices, the exact Rust release recorded under `docs/dependency-notices` (1.99.1). Ordinary `cargo build` only needs Rust 1.89 or later. Use `cargo run -- 'C:\some folder'` to open a root directly. A first build needs access to crates.io or the already populated Cargo cache.

`scripts/package-release.ps1 -Offline` builds the application, installer, and portable ZIP; NSIS 3.12 is additionally required. Use `-SkipBuild` only to package an already built, checksum-verified portable directory. The Windows x64 target uses static CRT linking through `.cargo/config.toml`.

## Repeatable command-line scans

```powershell
$p = Start-Process .\dist\0.3.0\DiskScope.exe -ArgumentList '--scan "C:\some folder" --json "D:\scan.json" --backend normal' -PassThru -Wait
$p.ExitCode
```

The executable uses the Windows GUI subsystem; use `Start-Process -Wait` when automating it. It attaches to the console that launched it, so `--help`, `--version` and scan output without `--json` are visible in a terminal, although the prompt returns immediately. `--backend ntfs` selects the preview, and `--timeout SECONDS` defaults to 600. Exit codes: 0 complete, 1 failure, 2 argument/build-mode error, 3 timeout, 4 partial results. JSON path fields ending in `_display` are display strings; native filesystem operations retain Windows paths without a UTF-8 conversion.

## Evidence and limits

See the [27 October 2026 360 review](docs/review-360-2026-10-27.md), [360-review fixes and validation](docs/validation-360-fixes.md), [0.3.0 validation](docs/validation-0.3.0.md), [RAM analysis](docs/memory-2026-10-26.md), [0.2.1 validation](docs/validation-0.2.1.md), the [C-drive investigation](docs/performance-c-drive-2026-10-26.md), [0.2 UI validation](docs/validation-0.2.md) and the [earlier 0.1 measurements](docs/validation.md) for commands and coverage. Native keyboard/UI Automation/renderer checks do not establish a full Narrator qualification or measured novice/expert usability improvement. Timing a small cached folder or a UI callback does not establish full-drive scan speed or end-to-end input latency. No general claim that DiskScope is faster than WinDirStat is made.

The application uses original Rust source and interface assets. Local research captures, raw scan reports, toolchains, and build outputs are excluded from this repository. Third-party dependency notices accompany the packages. A distribution license for this project's own source has not been selected.
