# DiskScope 0.2 validation

This release implements the research-driven Overview, connected filters, comparisons, local scan history, icicle option and appearance/accessibility controls approved on 2026-10-26. It remains a Windows x64 preview. Research motivates these design choices; actual novice/expert usability improvement is not yet established by a user study.

## Integration evidence

The independent query review accepted both specification and quality: combined filters retain scope, map geometry remains unfiltered, Overview/breakdowns/comparisons are bounded, focus paths retain Windows units, and icicle bands cap depth/output. Covering tests passed: 18 view, four icicle and three treemap tests. The explicit current release million-node query completed in 616.479 ms, returning 128 rows and 2,000 rectangles; node structs alone use 160,000,160 bytes, excluding names/children/query scratch. This is a synthetic background-query measurement, not a disk scan or input-latency result. The earlier approximately 15.3-second debug timing is superseded for release performance. A full million-node history persistence/comparison stress run was not performed.

The independent real-files history test uses the normal scanner twice and persists/reloads metadata. It verifies 420→700 logical bytes (+280), one added item, one removal, one modified item, one identity-preserving move, and +300 bytes in a child folder. Runtime generation associations clear on reload.

History tests cover invalid lengths/schema/policy/checksum/totals, hierarchy/path validation, unpaired UTF-16, compatible backend families, conservative hardlink move handling, bounded change results, complete-only capture, retention, and replacing the history directory at the same path. The independent history review found a multiple-owner retention issue. Its accepted correction holds OS-backed ownership across load/save/retention/clear. A second window explicitly reports history unavailable. Tests exercise six attempted captures, competing clear, staging-budget reservation and immediate reopening.

Final integrated checks on this Windows x64 machine: **130 passed, zero failed, two ignored** (79 library, 22 binary, 2 CLI process, 14 history, 1 real-files history, 12 scanner). The ignored cases are the explicit query benchmark and raw C: probe. Hardlink/junction fixtures ran outside the restrictive tool sandbox; the app does not request elevation.

```powershell
.\scripts\dev.ps1 -CargoArgs @('test','--offline','--locked','--all-targets','--quiet')
.\scripts\dev.ps1 -CargoArgs @('fmt','--all','--check')
.\scripts\dev.ps1 -CargoArgs @('clippy','--offline','--locked','--all-targets','--','-D','warnings')
.\scripts\dev.ps1 -CargoArgs @('build','--release','--offline','--locked')
```

Formatting, all-target lint with warnings denied and the final release GUI build pass. Fresh independent whole-change review and its single consolidated correction review accept specification, quality and runtime integration. Corrections include canonical no-extension lenses/editing, whole-scan type percentages, restored result pages, Changes in navigation history and measured contrast/DPI layout issues.

## Native UI and accessibility evidence

The actual app ran through native eframe Glow with AccessKit against real fixtures. Final captures cover Overview, Explore and Changes in dark/light themes, high contrast at 150% in-app zoom on 150% Windows scaling (225% effective size), command navigation and Changed item→map focus. Representative saved images are `research/diskscope-0.2-overview.png`, `diskscope-0.2-explore-light.png`, `diskscope-0.2-changes.png` and `diskscope-0.2-high-contrast.png`. These are native renderer outputs, not browser mockups.

A real second scan modifies a private copy of the 11-file fixture: grow a video by 1 MiB, add a 2 MiB file, remove a note and rename a source file. Changes reports 18,366,464→21,495,808 logical bytes (+3,129,344), with one addition/removal/modification/move. Changed-item focus clears an old video/name filter and selects the new ISO under Downloads; Back restores the prior context. Native raw-input keyboard assertions cover Ctrl+K, command selection/Enter and Alt+Back/Forward. Actual Windows OS keyboard input and UI Automation separately verified palette activation and reaching populated Largest results. OS-level category-filter mouse interaction was not verified by that tool route.

Windows UI Automation and the actual egui AccessKit tree expose map controls as labelled Buttons with Click/Focus actions and finite bounds. Narrator audio/task qualification remains untested. Numerical palette checks across enumerated text/background combinations have minimum contrast Dark 4.62:1, Light 4.66:1 and high contrast 9.29:1; white map labels on brightest checked hover fills are at least 5.05:1. This is a palette smoke check, not accessibility certification.

Debug native small-fixture update-callback CPU P95 ranged 1.307–2.476ms, maximum 14.121ms. This excludes rendering/presentation/input latency and does not establish sustained full-drive responsiveness.

## Package evidence

Release executable SHA-256: `2018fc1f5061976f9389dca4d542571ae7b7b8bf91e68cded81e3754af5491b0`. Versioned output is `dist/0.2.0`, preserving the 0.1 package. Notice collection verifies all 166 filtered Windows dependency packages with 319 notice files, supplemental immutable-source provenance and the complete Rust 1.99.1 standard-library copyright/license notice. All 342 package checksums verified. The packaged 0.2.0 executable completed the real 11-file/five-folder fixture scan with 18,366,464 logical bytes, zero errors and exit 0. This smoke check confirms local launch/accounting, not clean-machine qualification.

## Storage and accounting

GUI history is metadata-only and local: `%LOCALAPPDATA%\DiskScope\history`, or the explicit `DISKSCOPE_HISTORY_DIR` override for isolated QA. CLI scans remain history-disabled. At most five complete, error-free, nonoverflowed snapshots are retained, with caps of1,000,000 nodes/128 MiB per snapshot and 768 MiB including staging. Oversized or incompatible data is rejected with a message. File contents are not read by history or uploaded.

Normal and normal-fallback history may be compared. Raw NTFS is a separate family until parity is validated. A move requires an unambiguous file identity across both snapshots; hardlink aliases and unknown identities limit inference. Folder changes overlap hierarchically and are not additive/reclaimable totals. Map area and comparisons default to per-entry logical bytes; unique allocation remains separately labelled. ADS and filesystem metadata remain excluded. Modification time does not establish that a file is unused or unwanted.

## Qualification limits

- The NTFS preview, broad hardware/driver coverage and clean-machine runtime remain subject to the 0.1 validation limits. No new full-drive speed advantage is claimed.
- AccessKit is enabled, custom interactive controls have accessibility semantics, and keyboard/appearance routes are part of native QA. Actual Narrator listening tests and user studies must be explicitly distinguished from automated checks.
- A responsive UI cannot interrupt every Windows filesystem call. Large queries and history comparisons remain background work; their measured latency is distinct from UI processing/input latency.
- Icicle is an evaluation alternative. Logical weights repeat across hierarchy bands; summing every displayed band would double count. Treemap remains the default.
