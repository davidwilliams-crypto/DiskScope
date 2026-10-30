# Reliability and investigation UX validation

30 October 2026. Implements the engineering and UX review on local branch `codex/reliability-investigation-ux`. The review describes the earlier `927a064` source; findings below are its disposition, not claims about the unchanged preview download.

## Review disposition

| Finding | Implementation and regression |
|---|---|
| F1 Draft path versus active scan | Picker keeps a separate native draft PathBuf. Scanned root, volume replies and action reconciliation retain the active root. Reconciliation preserves an unscanned draft. Injected folder/action result regression verifies refresh and removed-row disappearance. |
| F2 Allocation completeness | Successful listing and pending descendant counts tracked separately; subtree completion propagates once. Controlled event regressions keep progress/cached/pure query allocation unknown while children are pending and settle correctly on completion/error. |
| F3 Relative roots | Lexical absolute path admission preserves native Windows paths and scanner root-link refusal. Relative-root and unpaired-UTF16 picker regressions pass. |
| F4 Rust minimum | Declared Rust 1.92 matches locked Windows graph. Local Rust 1.92.0 default GUI build passed, in addition to the new CI compiler-floor gate. |
| F5 Release artifact | Cargo-reported executable selected by root package/binary identity; no guessed path fallback. Actual alternate-target fixture proves stale default binary is ignored. Checksummed provenance records source/config/compiler/artifact digests. |
| F6 Empty/recovery states | Shared state-specific copy for maps/files/types, filter reset and retry/error routes. Rendering regressions cover terminal empty maps and filtered results. |
| F7 History | Root grouping, shared backend compatibility, gated comparison, full path and baseline/retention explanation. Explicit pairs from another root override cached root selection; manual root changes still persist. |
| F8 Dialog Escape | Frontmost actual dialog closes with invoking focus restored; menus get first Escape. Comparison/error dialogs included; input and accessibility regressions pass. |

## UX delivered

- Starting experience offers locally discovered drives with availability and up to five recent locations, retaining privileged persistence restrictions. Drive probing runs off the UI thread and avoids network-drive enumeration.
- Persistent scanned-root/coverage context distinguishes a drafted path from the source of results. Full paths and scan details have keyboard-accessible disclosure/copy controls.
- Responsive workspace switches to compact navigation and a file-identification list; selection details and actions remain accessible below it. Existing rich tables/maps remain on wider windows.
- Compact and desktop file rows expose names/kinds/sizes/selection; arrow selection and folder activation work with keyboard. Filter edits have linked labels; type controls have meaningful accessible names. Stable-display pending updates remain available after resizing.
- Session-local action review records operation result, rescan eligibility, path-presence verification, logical-size delta and timestamped volume-availability observations. Both logical scans must be complete and use compatible backends to report a delta. Unknown measurements remain unknown. Volume changes are observations, not attributed reclaimed bytes. Recycling explicitly explains retained Bin storage.
- Compact outcome details open in a scrollable, dismissible overlay rather than displacing the file list. Tests cover opening/Escape at a 600 × 426.7 logical viewport (900 × 640 at 150%).

## Verification record

Full command output is retained under ignored `research/` logs. Run the documented `scripts/dev.ps1` helper with Cargo arguments.

| Check | Evidence |
|---|---|
| Default/diagnostics/no-GUI suites | Default **398 passed**, diagnostics **401 passed**, no-GUI **273 passed**; each ordinary run leaves 14 deliberately ignored tests. |
| Formatting and Clippy | Formatting plus all-target warnings-denied Clippy for default, diagnostics and no-GUI configurations. |
| Declared compiler | Rust 1.92.0 installed only in this project's `.tools`; locked/offline Windows x64 GUI build passed. Rustup's unrelated self-update lookup failed after successful toolchain installation; compiler build itself succeeded. |
| Compact minimum viewport | Filename and size remain in bounds at 900 × 640 equivalents for 100/125/150%; outcome overlay, repeated arrow navigation and pending update controls tested. |
| Themes and input | Existing all-theme/text-scale, table restoration, row click/double-click/context menu, command, confirmation and privileged persistence tests retained. |
| Release tooling | Alternate target/artifact/provenance fixture, compiler-floor metadata, tooling and 35-check release suite passed in PowerShell 7 and Windows PowerShell 5.1. Full application built and packaged using an alternate CARGO_TARGET_DIR; package, reported artifact and provenance executable hashes match; payload checksums verified. |
| Optimized stress tests | Both ignored view tests passed: million-node query and 250,000-node ranking cases. Observed million-node background query 122.465 ms, 128 returned rows, 2,000 bounded rectangles; excludes I/O, model construction and rendering and is not a competitor benchmark. |
| Packaged executable smoke test | Exit 0, complete, 11 files, 5 folders, 18,366,464 logical bytes, zero errors on the read-only UI fixture. |
| Raw NTFS eligibility probe | Exercised the remaining ignored read-only probe: standard-token access denied (Windows error 5). This is an observed access limit, not successful raw-volume parity validation. |
| Independent review | Found and corrected compact focus transfer, incomplete-before logical delta, hidden pending controls, lossy native picker path and explicit-history-pair root conflict; covering regressions pass. |

The native application was launched against the read-only research UI fixture with isolated workspace history. Windows accessibility inspection reported complete scan, **11 files / 17.5 MiB**, zero errors, root identity, available/capacity values, history eligibility and named map/navigation controls. Capture returned only tiny window fragments on this host, so this is not a fresh full-window visual qualification. No user files were removed during UI inspection.

## Native file action qualification

All **11 ignored native file-action fixtures** were explicitly run serially with a standard Windows token and normal API permissions. All passed: junction ancestor/pre-commit replacement refusal; permanent deletion selected-fixture only, changed file, replaced path, open writer and read-only attribute; recycle preflight; actual recycle; owned-window recycle; and Shell sink veto preserving the fixture and sibling.

Actual Shell logs: operation flags `0x2098f404`; PreDeleteItem flags `0x00000280`; successful PostDeleteItem `0x00270008` with recycled destination; veto result `0x80004004` with no destination and preserved file. Existing Bin contents were not emptied. Tests create and remove only their own disposable fixtures; successful recycled fixtures remain in the Bin.

This closes the automated native portion of R1. The older hardening matrix remains the manual hardware/policy checklist. It must not be marked fully qualified: immediate-delete policy toggles, oversized/cached-capacity changes, removable/fixed-reporting USB, ReFS/Dev Drive/exFAT, redirected folders, prompt timing/minimize/close and clean-VM/UAC cases were not exercised. No user's Recycle Bin or system security policy was changed for this task.

## Remaining external qualification

No claim of issue-free operation, full Narrator certification, full-volume NTFS parity, universal filesystem compatibility, signed distribution, clean-VM installer qualification or proven competitive UX superiority is made. Those require hardware/environments, signing ownership and representative-user/competitor studies beyond the local automated checks. The new CI workflow has been added locally, not run on GitHub. The source-license decision remains with the owner.

The product direction is implemented as a testable investigation workflow. Calling it a trend setter still requires evidence from users, especially correct understanding of logical change, recycling and observed available space.
