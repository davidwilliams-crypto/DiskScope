# Open-item fixes: what to run on Windows

Date: 27 October 2026. Follows [the 360 review](review-360-2026-10-27.md) and [Recycle Bin hardening](validation-recycle-bin-hardening.md). This note covers the fixes for the remaining open items in four areas: scan history, the NTFS preview and scan engine, the interface and administrator detection, and the command line and tooling.

## Verified here

Four implementers worked in isolated git worktrees on disjoint files. A separate reviewer read each branch adversarially and a skeptic tried to refute every medium or high finding. No reviewer found a high-severity defect. The confirmed findings were fixed before merging: a misleading administrator message, an incomplete notices recipe, a misleading history status message, an inaccurate lost-folder notice, a comment that was wrong about `Idle`, and an unsafe helper that was not marked `unsafe`.

The implementers' own gate results were discarded. Builds that share one `CARGO_TARGET_DIR` across worktrees can run another worktree's compiled binaries, because cargo's artifact hashes do not include the worktree path. Everything below is from a clean rebuild of the merged tree (`cargo clean -p diskscope` for both targets first).

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | Passed |
| same, `--target x86_64-pc-windows-msvc`, with and without default features | Passed |
| `cargo test --locked --all-features` (Linux) | 199 library, 96 binary and 36 integration tests passed; 2 ignored (unchanged) |
| `cargo test --locked --no-default-features` (Linux) | Passed |
| Engine tests repeated 25 times in the merged tree | No failures |

New Linux tests cover each behaviour change. The history, NTFS and engine ones were checked against the old code and fail without the fix.

## History

Measured on Linux, release hash loop: peak extra heap while loading the catalog is 1.0 MiB regardless of node count, where the old code needed 60 MiB for 9 MB files and 240 MiB for 37 MB files.

Owed on Windows:

1. The sharing-violation test (`a_sharing_violation_never_deletes_and_the_next_start_recovers`) is type-checked only. It asserts that an exclusively locked valid snapshot is unreadable and neither listed nor deleted, that an undeletable older-format file is counted as stuck and kept, and that both recover after the handles are released.
2. Deleting the current directory entry while `ReadDir` iterates on NTFS is believed safe but was not run.
3. Throughput of the streaming checksum under antivirus or an indexer. The 865 MiB/s figure is a Linux hash loop, not an end-to-end Windows number.
4. Three tests call `set_len` on about 768 MiB; on NTFS that may allocate real space rather than stay sparse.

## NTFS preview and engine

Owed on Windows:

1. The edit in `ntfs_windows.rs` (shared parse scratch, in-place fixups on the aligned 4 MiB raw-volume buffer, the drive-root helper) is type-checked only.
2. Whether real `$UsnJrnl` records match the synthetic shape used in the tests. Until a whole-volume run succeeds on a standard system volume, the preview should still be treated as experimental and may fall back.
3. The `platform.rs` change that skips the per-call 64 KiB zeroing rests on documented Win32 behaviour (the record walk stops at `NextEntryOffset == 0`), not on a run.
4. A 4Kn volume: the update-sequence stride is 512 bytes on NTFS while the decoder derives the array length from the reported sector size. Unconfirmed; the preview would fall back safely.

## Interface and administrator detection

Owed on Windows:

1. With UAC off (`EnableLUA=0`), and as the built-in Administrator: the label reads "Administrator (always)", the hover text and the settings window show the explanation, "Restart as administrator" is absent, Move to Recycle Bin shows its reason and Delete permanently still works. Confirms that Windows reports an elevated token with `TokenElevationTypeDefault` there.
2. With UAC on, "Run as administrator" still shows plain "Administrator".
3. Delete a file inside a nested folder on a large tree: the same folder, sort and filters come back while the rescan runs and the nearest neighbour is selected.
4. Delete a folder's only file: the empty folder reopens. Remove the folder externally during the rescan: the parent opens with the notice.
5. The single-layout header, the disabled command-palette rows with their reasons, and the disabled "0 errors" button at 100% and 150% DPI and all three text scales. At the 900 px minimum width the totals line can still wrap with a very large scan.
6. Paths on real Windows: drive roots, case differences, `\\?\` paths.

## Command line and tooling

Owed on Windows (PowerShell was not available, so every script change was verified by reading only):

1. `scripts/build.ps1` and `scripts/dev.ps1`: the environment is restored after a run; `dev.ps1` falls back to `cargo` on PATH; the early readme-version check in `build.ps1` (`Assert-ReleaseReadmeVersion`) fails before compiling when `portable-readme.txt` and `Cargo.toml` disagree.
2. `scripts/test-release.ps1` gained seven checks that have not been executed. `Assert-Rejected` counts a check even when the operation throws for the wrong reason, so confirm each fails for the intended reason.
3. The recipe in `docs/dependency-notices/README.md` on a never-built checkout.
4. The console attach and the real GUI-subsystem exit codes (a real access-denied directory giving exit 4) were not exercised.

Behaviour changes to be aware of: `--scan ""` now exits 2 with "Scan root must not be empty" instead of scanning the empty path and exiting 1; a scan that ends cancelled with retained entries exits 4.
