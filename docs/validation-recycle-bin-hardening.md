# Recycle Bin hardening — validation

Date: 27 October 2026. Follows the [27 October 2026 360 review](review-360-2026-10-27.md), open item 1.

**30 October update:** All 11 automated native file-action fixtures now pass on Windows, including real recycling, owned prompts and Shell veto preservation. See [current validation](validation-investigation-ux-2026-10-30.md). The historical evidence below is retained.

**27 November update:** rows 1, 4, 5, 6, 9 and 14 were run on a standard user's PC; results and the remaining checklist are at the end of this document. Row 9 changed the product: **Windows does not recycle a file whose path reaches 260 characters**, so the preflight now refuses such paths before Windows is asked. Rows 2, 3, 7, 8, 10, 11, 12, 13, 15 and 16 are still open and must not be treated as qualified.

## The problem

"Move to Recycle Bin" asks the Windows Shell to recycle one file. When the Shell cannot recycle a file it **deletes it**. No documented `IFileOperation` flag turns that into a failure; `FOF_WANTNUKEWARNING` only makes the Shell warn. The cases are a drive with no Recycle Bin (USB flash drives and memory cards), a file larger than the Bin's maximum size, a volume set to "Remove files immediately", and the `NoRecycleFiles` policy. In 0.3.0 the sink noticed the destruction only after the file was gone, reported a generic error and did not rescan. The user had confirmed only the simple Recycle dialog, not the checkbox and typed `DELETE` that permanent deletion requires.

0.3.0 validated the happy path only (see the erratum in [0.3.0 validation](validation-0.3.0.md)).

## What changed

| Layer | Behaviour |
|---|---|
| Preflight (`recycle_policy`) | Refuses Recycle only on positive evidence: the drive is not `DRIVE_FIXED`; `NoRecycleFiles` is set in HKLM or HKCU; the volume's `NukeOnDelete` is set; or the file's size is at least the Bin maximum (explicit `MaxCapacity` in MiB, otherwise the default of 10% of the first 40 GiB plus 5% of the rest, rounded down to whole MiB as Windows records it). A setting that is missing is a Windows default and never blocks. A setting that cannot be read, or an unresolved volume, yields `Unverified` and never blocks. |
| When it runs | While the file is verified (before the confirmation dialog, so a refusal replaces the dialog), and again at click time (the authoritative check, in case a drive was removed or a limit lowered). |
| Sink veto | `PreDeleteItem` fails with `E_ABORT` when Windows clears the recycle request bit (reported for "Remove files immediately"; not yet observed here) or queues more than the one approved item. |
| Owner window | The main window is passed to `IFileOperation::SetOwnerWindow` (validated as a live window of this process) so a Shell prompt of its own is modal and in front of DiskScope. Best effort. |
| Settlement | The sink no longer decides the outcome. After the Shell returns and the pinned handles are released, the path is reopened and compared with the approved file's identity, size and time, so a re-created path or a delete-pending name cannot pass for the intact file. `Ok` requires a confirmed Recycle Bin destination. A different or missing file with no destination is `DeletedNotRecycled`; a different, missing or unreadable one otherwise is `RecycleOutcomeUnconfirmed`; the approved file still being there keeps the recorded reason. |
| Interface | A refusal or a surprising outcome opens an acknowledgement dialog that names the file and, for a refusal, points to the separate **Delete permanently…** flow. Whenever the file may be gone the list rescans. The confirmation and the running dialog say Windows makes the final decision and to choose **No** on any Shell prompt to delete permanently. |

Flags are unchanged from the validated fixture. `FOF_NOCONFIRMATION` and `FOF_NO_UI` are deliberately not used: they would answer the Shell's own prompt Yes on the user's behalf.

## Verified on Linux with a Windows type-check

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | Passed |
| `cargo clippy --locked --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings` | Passed |
| `cargo clippy --locked --no-default-features --all-targets --target x86_64-pc-windows-msvc -- -D warnings` | Passed |
| `cargo test --locked --all-features` (Linux) | 124 library and 58 binary tests passed |
| Pure rules | Default capacity reproduces the value Windows wrote (348,915,757,056 bytes gives 18,685 MiB) and the first size above it is refused; an ordinary fixed drive with default settings is not refused at the sizes sampled below the limit; unknown facts do not block in the cases tried; the `>=` boundary; every pair of `Setting::combine` inputs and each verdict rule individually; volume GUID parsing |
| Outcome rules | An exhaustive test over every combination of what the sink saw, the recorded error and the file's presence: success implies a confirmed Bin destination, and any error that does not say the file may be gone implies the file was seen present |
| Interface flow | A destroyed file rescans and opens an acknowledgement; a refused recycle keeps the scan; a successful recycle rescans; the acknowledgement closes with Escape and starts nothing |

None of the Windows-only code was executed. It is type-checked and linted only.

## Windows validation matrix

Run on a disposable fixture. Record the Windows build, the DiskScope commit, and for each row the `PreDeleteItem` flags and `PostDeleteItem` destination that the fixture logs. The Observed and Pass columns stay empty for every row that has not been run.

Automated first: `cargo test --locked -- --ignored --test-threads=1 file_actions::tests` (the veto hook is process-wide), then the manual rows.

| # | Scenario | Expected | Observed | Pass |
|---|---|---|---|---|
| 1 | Ordinary file on `C:` with default settings | Verdict `Likely`; flags `0x282`; non-null destination; Bin count +1; no extra prompt | 27 Nov 2026, fixed NTFS volume `D:`, Bin settings as Windows wrote them: `Likely`; flags `0x280` (no `0x2` bit on build 26300); `PostDeleteItem` `0x00027008` with a destination; one Bin record per file; no prompt, with and without an owner window | Yes |
| 2 | USB flash drive reporting `DRIVE_REMOVABLE` | Refused at preparation with an acknowledgement; file intact | | |
| 3 | USB hard disk reporting `DRIVE_FIXED` | Recycles, or record exactly what Windows does | | |
| 4 | Volume with "Remove files immediately" (`NukeOnDelete=1`) | Refused at preparation; with the preflight bypassed the sink flag is `0x202` and the veto holds | 27 Nov 2026: `Blocked(DeletesImmediately)` at preparation. Preflight bypassed: `PreDeleteItem` flags `0x200` (the recycle bit is clear), `PostDeleteItem` `0x80004004`, no destination, `RecycleUnavailable(ShellDeclined)`, file unchanged, no prompt | Yes |
| 5 | Bin maximum 1 MB and a 4 MB file; DiskScope started after the change | Refused as too large | 27 Nov 2026: `Blocked(TooLarge { size: 4194304, limit: 1048576 })` | Yes |
| 6 | Exactly-at-limit and one-over files | Boundary measured; DiskScope refuses at the limit | 27 Nov 2026: with the maximum at 1 MiB, exactly 1,048,576 bytes and one byte more are refused, one byte less is `Likely` | Yes |
| 7 | Bypass experiment: call the Shell directly for an oversized file | Record whether Windows prompts under `FOF_WANTNUKEWARNING` without `FOF_NOCONFIRMATION`, whether before or after `PreDeleteItem`, its default button, whether it is in front of DiskScope, and what No and close return. Answering No: `Cancelled`, file present. Answering Yes: `DeletedNotRecycled`, file gone, rescan | | |
| 8 | `NoRecycleFiles=1` in HKCU, then HKLM | Refused with the policy message | 27 Nov 2026: not run. On build 26300 a standard user cannot create the value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Policies\Explorer` (access denied); both hives need an administrator | Not run |
| 9 | Path of 260 characters or more | Record only; no rule | 27 Nov 2026: through 259 characters Windows recycles (flags `0x280`, destination present). From 260 Windows never called the sink: with one long folder name the operation was abandoned (`Cancelled`, file intact); with eight nested folders (357 characters) Windows showed its own "Delete File: Are you sure that you want to permanently delete this file?" with **Yes** first, unanswered after 20 s, file intact. Now refused by the preflight (`PathTooLong`, limit 260) | Recorded; rule added |
| 10 | ReFS or Dev Drive volume | Record only; no rule | | |
| 11 | exFAT or FAT32 fixed volume | Record only | | |
| 12 | "Display delete confirmation dialog" enabled | Record whether an extra prompt appears on the happy path and whether it is owned | | |
| 13 | Captured owner window on a real glow window | Non-null `HWND` belonging to this process | | |
| 14 | Bin maximum raised while DiskScope runs | Record whether the Shell's cached value makes it stricter than DiskScope's read | 27 Nov 2026: raised from 1 MiB to 64 MiB in the running process after one recycle; the preflight read 64, Windows recycled a 2 MiB file at once (flags `0x280`, destination present). No stricter cached value on build 26300 | Recorded |
| 15 | Minimise DiskScope while a Shell prompt is up; close DiskScope while one is up | Record behaviour | | |
| 16 | OneDrive-redirected known folder (Desktop or Documents) | Record only | | |

Decision rules after a run: if the veto test fails, the sink veto is advisory and the settlement is the safety net, so consider making the `NukeOnDelete` refusal authoritative at the Shell level too. If an oversized file is destroyed with no prompt although the preflight allowed it, record the cause (a cached limit, a quota-aware total, a known-folder Bin) and tighten that rule. If `SetOwnerWindow` misbehaves with the winit window, delete that one call; the loss is only the in-front guarantee. If the registry names or units differ from those recorded here, update the readers.

## Residual risks

- **A Windows prompt answered Yes.** If Windows shows its own "permanently delete?" prompt for a case the preflight missed, DiskScope cannot change its default button. The file is then deleted after a Windows confirmation that is lighter than typed `DELETE`. DiskScope reports it, rescans, and cannot undo it.
- **Silent destruction.** Every published oversized-file destruction was observed with `FOF_NOCONFIRMATION` set; the prompt itself was reproduced only for folders. If Windows silently deletes a case the preflight does not model, the file is lost with no chance to answer No. The preflight narrows this to unmodelled conditions and the settlement reports it afterwards.
- **Unmodelled causes.** A Bin root Windows cannot create, ReFS and Dev Drive behaviour, known-folder Bins, per-user quotas, very long paths, mounted VHDs, and USB sticks that report `DRIVE_FIXED` without a Bin.
- **Reverse-engineered facts.** The registry layout, value names, the MiB unit, the default-capacity formula and the strict size boundary come from one Windows 11 report and published descriptions, not from Microsoft documentation. A wrong name makes a check silently miss, and a miss falls back to the sink veto, the owned prompt and truthful reporting.
- **Bin size cache.** Windows is reported to cache the Bin size per process. A limit raised after the cache was filled makes Windows stricter than DiskScope's read. DiskScope itself re-reads the settings on every request.
- **Removable drives with a Bin.** The `RecycleBinDrives` policy makes Windows keep a Recycle Bin on removable drives. DiskScope refuses Recycle on any drive that is not fixed, so those users must use Delete permanently. The value's layout is community-sourced, so it is not honoured; refusing is the safe direction.
- **Other limits.** The `RecycleBinSize` policy percentage and per-folder Bins for locally redirected known folders (`BitBucket\KnownFolder`) can lower the real limit below DiskScope's estimate; they are not read. The total used for the default limit is the quota-aware one from `GetDiskFreeSpaceExW`, as Windows documents, so a disk quota can lower it. Machines whose system drive reports `DRIVE_REMOVABLE` (Windows To Go style) are refused.
- **Path, not identity.** Presence is checked by path. A path re-created by another process after a destroy reads as present and is reported as a generic failure, never as a false "deleted" claim.

## Run of 27 November 2026

Windows 11 build 10.0.26300, a standard interactive user, NTFS; DiskScope at the commit that adds this section (its fixtures ran on exactly that code). The fixtures lived on the `D:` volume of that PC, so the per-volume settings below are `D:`'s. The driver set one value at a time, ran that row's fixtures, and put the exact previous values back; restoration was proved by reading the values and by comparing `reg.exe query` output from before and after. Windows had already written `MaxCapacity` 50842 and `NukeOnDelete` 0 for that volume (and 99685 for a 1906.74 GiB `C:`; the default-capacity formula estimates 99673 there, so the estimate is the stricter of the two).

| Row | Setting in effect | What the fixtures logged | Result |
|---|---|---|---|
| 1 | As Windows wrote them | `Likely`; `SetOperationFlags 0x2098f404`; `PreDeleteItem` flags `0x280`; `PostDeleteItem` `0x00027008`, destination present; with the recycle bit forced clear the sink's `0x80004004` stopped the Shell and the file stayed | Pass |
| 4 | `NukeOnDelete=1` | Preparation: `Blocked(DeletesImmediately)`. With the preflight bypassed: `PreDeleteItem` flags `0x200`, `PostDeleteItem` `0x80004004`, no destination, `RecycleUnavailable(ShellDeclined)`, file unchanged | Pass |
| 5, 6 | `MaxCapacity=1` | `TooLarge { size: 4194304, limit: 1048576 }`; the limit itself and one byte over are refused, one byte under is not | Pass |
| 8 | `NoRecycleFiles` | Not runnable: the policy key is not writable by a standard user on this build | Not run |
| 9 | Nothing | Lengths 200, 250 and 259: recycled, flags `0x280`. Lengths 260 to 357: no sink callback at all. Separately, a 357-character path of eight nested folders brought up Windows' own permanent-delete question, **Yes** first, before any callback | Recorded; rule added |
| 14 | `MaxCapacity` 1, then 64 in the same process | After the raise the preflight read 64 and Windows recycled a 2 MiB file (flags `0x280`, destination present) | Recorded |

Two things Windows did that are worth knowing:

- It does not recycle a file whose path reaches 260 characters (`MAX_PATH`). Depending on the shape of the path it abandons the operation or asks to delete the file permanently first, and the sink veto never gets a turn because `PreDeleteItem` is not called. The preflight now refuses such paths (`RecycleBlock::PathTooLong`, limit `MAX_RECYCLE_PATH_UNITS`) and the acknowledgement points to **Delete permanently…**, which keeps its own checkbox and typed `DELETE`.
- Using the Recycle Bin while a smaller maximum is in effect makes Windows write `NeedToPurge=1` on the volume key. The driver removed it afterwards so the key read as before.

Four fixtures were added to `scripts/test-native-fixtures.ps1 -IncludeRecycle`. Each acts only when its row's setting is already in effect, so the gate is safe under default settings: `recycle_verdict_names_the_setting_that_refuses_and_stops_at_the_exact_boundary`, `sink_veto_holds_when_windows_would_remove_the_file_immediately`, `recycle_stops_at_the_path_length_where_windows_stops_recycling` and `bin_maximum_raised_while_running_is_honoured_by_windows` (the last writes the volume's maximum and puts it back; it runs only when the maximum is already 1 MiB).

### Checklist for the rows still open

Record for each: the Windows build, the DiskScope commit, and the `PreDeleteItem` flags and `PostDeleteItem` destination from the fixture logs (`-- --ignored --exact --test-threads=1 --nocapture`).

1. **Other media** (rows 2 and 3): a USB flash drive that reports `DRIVE_REMOVABLE`, then a USB hard disk that reports `DRIVE_FIXED`, both NTFS. Put the checkout on that volume, or point the fixtures at it, and run the gate with `-IncludeRecycle`. Row 2 must be refused at preparation with the file intact; row 3 records what Windows does.
2. **An administrator** (rows 8, 10 and 11): set `NoRecycleFiles=1` under `HKLM`, then under `HKCU`, and run the boundary fixture; create a ReFS or Dev Drive volume and an exFAT or FAT32 fixed volume and run the gate there, recording only. Do not use a cloud-sync virtual drive for row 11: it reports `DRIVE_FIXED` with FAT32 but belongs to the sync client.
3. **A person at the screen** (rows 7, 12, 13 and 15): with `MaxCapacity` at 1 MiB, call the Shell directly for a 4 MiB file and answer the question No, then Yes, noting when it appears, its default button, whether it is in front of DiskScope, and what each answer returns; switch on "Display delete confirmation dialog" and note whether the happy path gains a prompt; pass the real main window as the owner and record its handle; minimise and close DiskScope while a Shell prompt is up.
4. **OneDrive** (row 16): with Desktop or Documents redirected, record what Windows does with a placeholder.

Decision rules from the first run still open: whether a removable drive with the `RecycleBinDrives` policy recycles (the preflight refuses it today, which stays the safe direction), and whether the default-capacity formula needs a correction factor for volumes of a few terabytes.
