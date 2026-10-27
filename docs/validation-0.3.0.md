# DiskScope 0.3.0 validation — 2026-10-26

This release adds explicit Windows administrator relaunch, single-file Recycle Bin/permanent deletion and a separate persistence policy for privileged sessions. The 0.2.1 performance artifacts remain unchanged; those earlier reporting timings do not qualify the new administrator scan mode.

## Source and backend verification

| Check | Result |
|---|---|
| Locked/offline default test suite | 194 passed, 11 explicitly ignored |
| Locked/offline diagnostic-feature suite | 197 passed, 11 explicitly ignored |
| Formatting | `cargo fmt --all -- --check` passed |
| All-target Clippy, default and diagnostics | Both passed with warnings denied |
| Explicit permanent-action integration fixtures | 5 passed: selected file only, changed content, replaced path, open writer and preserved read-only attribute |
| Explicit Recycle Bin fixture | Passed with ordinary token and non-null recycled destination; sibling preserved |
| Explicit junction fixtures | 2 passed: preparation and post-confirmation commit refusal |
| Independent source review | Mapped-drive restriction finding fixed and re-reviewed; no remaining material findings |

Use `scripts/dev.ps1 -CargoArgs @('test','--offline','--locked')` and add `--features diagnostics` for the second configuration. Clippy uses `--all-targets` and `-- -D warnings`. The ignored action/junction fixtures require explicit authorization and are not activated by the ordinary suite. Those tests run outside the filesystem sandbox with an ordinary Windows token; sandboxed Shell/hard-link operations can fail despite the ordinary-token implementation working. Running outside the sandbox does not grant administrator access.

No user files are used for destructive tests. Backend deletion tests use uniquely named fixtures under this workspace; the successful Recycle Bin fixture remains in the Bin. The five permanent-action fixtures were repeated after the final test-cleanup change and passed.

Two classification tests verify the local-volume policy. Preparation and every commit/callback check both drive type and a direct local volume mapping; mapped network drives, SUBST aliases and unknown mappings are refused. The five permanent-action fixtures passed again after this guard was added. Ordinary sandboxed `QueryDosDevice` can be denied, so these native fixtures require an ordinary token outside the development sandbox.

Native input checking found filename/size label clicks being consumed by selectable text and Windows Shift+Delete being converted into a clipboard Cut event. Actual egui input regressions verify row selection, folder expansion/double-click/context-menu behavior, text-input preservation, and both held-modifier and complete-chord event sequences. A rendered-layout test checks sort/search controls precede the table. The final binary suite contains 45 passing tests, including 23 new management/input tests.

## Native and package verification

The tested release executable is SHA-256 `c77fa187dff9e41a10880585b7e5f6094babb2769a650be1bd7c7dcbe5df3b3b`.

Actual native checks passed for filename selection, Shift+Delete, both independent permanent-confirmation gates, Escape cancellation, scan-root text entry, Ctrl+K/Escape during a normal C: scan, pointer switching to Largest during scanning, Stop, F1/help, and wheel scrolling. The checkbox alone and typed `DELETE` alone left permanent deletion disabled; both enabled it. The dialog was cancelled. Before/after SHA-256 and size checks verified that all 11 UI fixture files (18,366,464 bytes) remained unchanged.

The normal C: mixed-input trial was deliberately stopped after approximately 23.74 seconds. Its next captured frame showed **Stopped · partial results**, retaining 2,271,638 files and 525,280 folders with 252 read errors. The Stop call plus screenshot round trip was 271 ms; this is an observation bound including automation overhead, **not measured input-to-render latency**. Keyboard commands and the pointer view change were visibly usable while discovery continued. No compiler or second scan ran during this trial. This cancelled functional test is not a new matched completion-speed comparison with WinDirStat.

The apparent missing query controls were confirmed to be a retained outer-pane scroll offset: scrolling upward restored search, sort and Whole scan. The helper's primary screenshot region was 15×15 pixels; the fresh native binding's full screenshot region enabled wheel input. This was an automation binding limit, not a failed app scroll. Minimum-width/resize behavior and a comprehensive Narrator pass were not qualified in this run.

The explicit Restart as administrator button opened its waiting modal. The user approved Windows UAC directly. A replacement window showed **Administrator**, preserved `C:\`, began a normal read-only scan, and displayed the disabled-history/preferences notice. The user subsequently interacted with that window; no elevated scan-speed or full-coverage comparison is inferred. The final administrator window was left open. Real UAC cancellation was not forced; backend classification and application cancellation/error handling are covered by tests. Privileged file deletion was not tested on user files.

The final-source normal-token Recycle Bin fixture was repeated with a ten-second bound: it passed in 0.09 seconds, confirmed the recycled destination, and preserved the sibling. No Shell confirmation was automatically answered. Existing Bin contents were preserved.

The packaged executable's command-line smoke scan passed: exit 0, complete, 11 files, 5 folders, 18,366,464 logical bytes and zero errors. The portable directory includes 319 notice/license files for 166 packages, this validation report and the historical C-drive performance report. Final payload/ZIP verification checks all 343 manifest digests, all 344 archived files and ZIP CRCs; the archive has a separate SHA-256 sidecar. ZIP timestamps older than 1980 are clamped to the format's minimum without changing any payload bytes.

## Behavioral limits

Administrator access can improve coverage and permit raw-volume access, but normal scanning is not guaranteed to become faster. More readable files can increase time and RAM. The experimental NTFS backend remains opt-in and lacks a qualified elevated C-drive accounting comparison.

Permanent deletion acts on a pinned handle after identity, exact timestamp, size, regular-file and reparse checks. It does not override file ownership, ACLs, sharing locks or read-only attributes. Recycle Bin is unavailable in administrator mode because the Windows Shell action resolves a path; the normal-token action has no permanent-delete fallback. Only single local regular files can be acted on. Removing a hard-link alias may reclaim no storage.

Elevated and unknown-access GUI sessions start with default preferences and an unavailable history service. eframe persistence is directed to the Windows NUL device and app save/load paths are disabled. The current process token determines the policy; command-line flags cannot claim administrator mode.

Native app-control approval and Windows UAC consent remain human actions. A successful Shell process launch does not itself prove that the elevated window is ready.

## Memory explanation

The previous ordinary C-drive reporting workload retained about 4.55 million metadata records. At 160 bytes per Node, used structs alone occupy about 728 MB, before names, child IDs, file-identity indexing and unused allocation capacity. The inventory enables navigation and filtering without repeated disk reads; the identity index supports hard-link accounting.

The recorded 2.38 GB private committed peak was brief. Late-run physical working set was approximately 1.09 GB; these are different counters and do not establish steady-state GUI residency. The sampled jump/drop closely matches a large vector reallocation, but capacity was not instrumented, so attribution remains an inference. No memory-layout change is mixed into this release. More RAM is useful when it avoids repeated work; unused or transient allocation alone does not improve stability. See the source repository's `docs/memory-2026-10-26.md` for calculations and limits.
