# DiskScope engineering and UX review

Review date: 30 October 2026. Reviewed source: `927a064b3e6bb2860f627fa74d762495708d09aa` (DiskScope 0.3.0 preview).

DiskScope has a credible technical foundation, but it is not ready to claim leadership in user experience. The strongest opportunity is an evidence-led storage investigation: understand what occupies space, understand what changed, choose an informed action, and verify the outcome. Improving that complete journey should take precedence over adding more visualizations.

This review identified eight source-supported defects, one important release qualification gap, and several design and accessibility gaps. Findings are recommendations for remediation; no production changes were made. Passing tests establish a baseline, not an absence of defects.

## Scope and evidence

Three independent review streams examined scanner/data engineering, native file-action/release safety, and interaction/accessibility design. Findings were consolidated and checked against the source. Coverage includes engine/concurrency, accounting, model/query, NTFS, history, CLI, GUI state, input, themes, file actions, elevation/privacy, build and distribution scripts, and existing validation evidence.

Fresh checks on Windows:

| Check | Result |
|---|---|
| Ordinary locked default test suite, normal permissions | 372 passed, 0 failed, 14 intentionally ignored |
| Formatting, all packages | Passed |
| Locked Clippy, all targets, warnings denied | Passed |
| Current debug application build | Passed |
| Native welcome-screen observation | Current build inspected visually and through Windows accessibility tree |

The restricted test run had 15 Windows API/path/access failures. The same ordinary suite passed with normal permissions, so those failures are not counted as application bugs. Logs are retained locally in `research/review-tests-2026-10-30.log` and `research/review-tests-native-2026-10-30.log` (ignored research files).

Live UI observation covered the welcome layout and named navigation/path/buttons. Native input automation did not establish a completed fixture workflow; results-page findings below are source analysis, not observed end-to-end usability results. Existing screenshot artifacts and historical validation are not represented as fresh observations.

No destructive, ignored Shell-action, installer, full-volume, UAC, or system-settings tests were run. There was no clean-VM qualification, fresh dependency-advisory audit, Narrator qualification, representative-user study, or matched competitor benchmark. Findings described as source-confirmed have not all been reproduced by a new executable regression test. There is no claim that every defect has been found.

## Findings to fix

Priority convention: P1 is high priority because the behavior undermines trust in a consequential workflow; P2 is a material correctness or usability defect. Priority is distinct from confidence and does not imply demonstrated data loss.

### F1 P1 Selecting another folder can suppress refresh after a file action

**Evidence:** [folder-picker result](../src/app.rs#L426), [action binding](../src/app/management.rs#L563), [completion rescan guard](../src/app/management.rs#L638).

The picker immediately replaces `root_path`, although results and generation still refer to the previous scan. Action preparation binds to `snapshot.progress.root`; completion requires that binding root to equal `root_path` before rescanning.

**Trigger:** Scan folder A, select B through Browse without starting its scan, then act on a file still displayed from A. If the file is removed, the guard skips the expected rescan. The path input also names B above results belonging to A.

**Impact:** A removed file can remain in results presented as current, and the application loses the usual post-action reconciliation. This is a state-consistency defect, not evidence that the wrong file is deleted: the action still binds to the selected row's path and identity.

**Fix direction:** Separate the draft path from the active scanned root. Results, action bindings, volume replies, and action reconciliation must use the scanned root. Clearly mark a draft path as unscanned.

**Acceptance:** A deterministic test injects a picker result for B while displaying A, then injects a successful action outcome for A without deleting a real file. A is refreshed, its removed row disappears after reconciliation, and B remains only a draft. Repeat for changed/unknown action outcomes that require reconciliation.

### F2 P2 Active allocation can appear complete before child directories finish

**Evidence:** [directory completion](../src/engine.rs#L432), [progress allocation](../src/engine.rs#L501), [cached root shortcut](../src/view.rs#L191).

`directory_done` marks a directory allocation complete when its own enumeration ends. Child workers may still be pending. The progress calculation and cached root query trust the root flag and can expose a partial allocation sum as known. The shortcut comment explicitly assumes a stronger invariant than this code maintains. Terminal finalization recomputes subtree completeness, so this finding does not establish incorrect completed totals.

**Fix direction:** Distinguish successful completion of a directory listing from completion of its subtree, or track pending descendants. Preserve query performance without substituting a full-tree walk on every progress update.

**Acceptance:** A controlled event fixture sends a known file and subdirectory, marks the root listing done, and holds the child worker. Root progress and view allocation remain unknown. Complete the child and verify allocation becomes known; repeat with child failure/cancellation and compare cached and pure queries.

### F3 P2 Relative GUI roots scan but break history and file actions

**Evidence:** [GUI argument acceptance](../src/cli.rs#L108), [scan admission](../src/app.rs#L336), [model paths](../src/model.rs#L192), [history root validation](../src/history.rs#L1204), [action path validation](../src/file_actions.rs#L443).

Launching with `DiskScope.exe .`, or entering a relative folder path, admits that path unchanged. Scanning can succeed and generate relative row paths. History requires absolute roots, and native file actions require absolute local drive paths, so these otherwise valid results cannot use those features.

**Fix direction:** Resolve relative paths once at scan admission. Retain native Windows path handling and the existing rejection of junction/link scan roots; do not accidentally weaken those policies through normalization.

**Acceptance:** Relative and absolute references to the same fixture have equivalent scan, history, comparison and action-preparation behavior. Include spaces, Unicode, drive-relative input, and junction roots. No file removal is necessary to test preparation.

### F4 P2 Documented minimum Rust version cannot build the default GUI

**Evidence:** [package declaration](../Cargo.toml#L6), [build instructions](../README.md#L53), locked `eframe` and `egui` 0.34.3 package manifests.

The project declares and documents Rust 1.89. Its default GUI dependencies declare Rust 1.92. Users following the documented minimum cannot build the default application on 1.89–1.91.

**Fix direction:** Establish the actual minimum across the locked dependency graph and update the package declaration and instructions, or select compatible dependencies.

**Acceptance:** A clean build with the declared minimum compiler and locked dependencies runs in CI. Metadata inspection established this mismatch; this review did not install or run an older compiler.

### F5 P2 Packaging can select an older executable after a successful build

**Evidence:** [Cargo invocation and hard-coded copy](../scripts/build.ps1#L44), especially line 61.

Cargo honors configured target directories and build targets, while the packaging script always copies `target\release\diskscope.exe`. With `CARGO_TARGET_DIR` pointing elsewhere, Cargo can build successfully and packaging can copy an older executable left at the default path. Self-generated package checksums prove internal consistency, not that this executable came from the current build.

**Fix direction:** Select the executable from Cargo's reported build artifacts, or explicitly control target/output configuration and verify the expected target. Tie release provenance to the source revision, build configuration and executable digest.

**Acceptance:** Exercise an alternate target directory with an older default executable present. The package contains the newly built executable. A missing or unexpected artifact fails clearly. Also cover an explicit build target. This review verified the path mismatch and Cargo metadata behavior, not a newly produced stale package.

### F6 P2 Empty-state copy confuses terminal, filtered and active scans

**Evidence:** [file-table empty state](../src/app.rs#L1076), [map empty state](../src/app.rs#L1351), [Types table](../src/app.rs#L1135).

After generation zero, any empty map says it will appear as file sizes are discovered, including completed empty/zero-byte, stopped, or failed scans. Table empty-state logic considers text search but not active size/type/date filters. Types lacks equivalent explanatory recovery guidance.

**Fix direction:** Use a shared state model combining scan state, coverage, active filters, result count and drawable logical bytes.

**Acceptance:** Preparing, discovering, complete empty, zero-byte, no matches, failed, stopped and partial states each show accurate language and a useful next action. Terminal states never promise incoming results. Clearing filters is offered when filters caused the empty result.

### F7 P2 Comparison enables incompatible scan pairs

**Evidence:** [Compare enablement](../src/app/changes.rs#L78), [scan picker](../src/app/changes.rs#L282), [backend compatibility rejection](../src/history.rs#L1749).

The interface offers all scans and enables Compare for any two distinct IDs. The backend then rejects different roots or incompatible backends. Closed picker labels also reduce root identity to the basename, making identically named folders difficult to distinguish.

**Fix direction:** Group history by root and restrict choices to compatible baselines. Show full selected-root identity, before/after timestamps, backend compatibility and eligibility before submission.

**Acceptance:** An invalid pair cannot be submitted. Explain why a scan is unavailable, distinguish same-named folders, and clearly communicate deliberate reverse chronological comparisons if supported.

### F8 P2 Escape behavior differs from the documented dialog contract

**Evidence:** [Escape handler](../src/app/controls.rs#L265), [help text](../src/app/controls.rs#L430).

Help says Escape closes a dialog. The global handler closes commands, filters, help and settings, but omits the comparison and error windows. It also resets several flags together instead of implementing a clear foremost-window dismissal policy.

**Fix direction:** Establish one dialog/focus dismissal policy, including action-confirmation cancellation behavior.

**Acceptance:** Escape closes the foremost dismissible window and returns keyboard focus to its invoking control. Add meaningful input tests for comparison/errors and multiple open windows.

## Release qualification gap

**R1 High priority: Native recycling hardening lacks the required Windows qualification.** [The existing validation document](validation-recycle-bin-hardening.md#L37) records a blank Windows matrix. Ordinary tests intentionally omit Shell veto/owned-window and destructive integration fixtures. A green default suite therefore does not qualify the current native recycling behavior.

Required evidence includes default happy path; immediate-delete policy; oversized and boundary files; cached policy changes; removable versus fixed-reporting USB; prompt ownership/cancellation; redirected known folders; long paths; and supported filesystem variants. Record unsupported combinations explicitly. Use disposable fixtures and controlled test environments. A source review cannot certify Shell decisions.

This is a release gate and residual risk, not a newly reproduced data-loss defect. The implementation already performs substantial preflight checks, same-handle permanent-delete verification, identity checks, explicit confirmations and post-action outcome classification. Those protections should be retained.

## UX and accessibility assessment

### Visual and information hierarchy

The live welcome screen has a restrained palette, readable grouping, named controls and a prominent starting action. It also spends substantial space on an empty scan summary, duplicate ready status, and a large instructional card. Choosing a drive is still framed primarily as entering a path or opening a folder picker. The prominent administrator restart competes with the actual first task before a coverage problem has occurred.

Design a stronger start around available drives, free-space context, and recent locations, with Browse as an alternative. Keep elevated access available contextually when coverage explains its benefit. These are design judgments to validate with users, not proven defects or reasons to remove the existing access controls.

The current Overview, Explorer, Largest, Types and Changes divide related questions across destinations. Preserve the connected selection and browse state, but simplify the question being answered at each moment. Display the active scanned root, scope, filters, freshness and coverage beside the result they qualify.

### Compact layout is not qualified

The app supports a 900 × 640 minimum window and 150% global zoom. Explorer combines a fixed sidebar, inspector minimum, content margins and table-column minima ([sidebar](../src/app/ux.rs#L67), [inspector](../src/app/ux.rs#L440), [columns](../src/app.rs#L935)). Overview has a stacking breakpoint; Explorer has no comparable compact strategy.

This is a strong source-supported layout risk, not a freshly observed clipping defect. Current rendering tests do not establish that every essential control fits. Qualify all supported text scales at the minimum window and representative Windows DPI levels. Collapse navigation and move the inspector to a drawer at an explicit width threshold, preserving filename, size, selection and action access.

### Accessibility needs task qualification

AccessKit is enabled. The live welcome tree contains named navigation, commands, path input, starting actions, settings and help. Map tiles explicitly supply name/size/type, Button role, focus indication and keyboard activation ([map semantics](../src/app.rs#L1261)). This is useful groundwork.

File/type rows use click-sensed rows and labels without an explicit row selection/hierarchy/expansion model ([file rows](../src/app.rs#L929), [type rows](../src/app.rs#L1139)). Filter text fields have no explicit association with their visible labels ([filters](../src/app/controls.rs#L40)). Symbol controls and palette selection also need native announcement verification. This is insufficient evidence to declare those surfaces inaccessible; it is equally insufficient to claim full accessibility.

Qualify a complete workflow with keyboard and Narrator: choose root, scan, hear completion/failure, navigate/expand, search, identify a selected item and size, inspect/cancel an action, and compare history. Prefer one tab stop with arrow navigation for a file list, announced selection/expansion, clear filter names, and restrained announcements of asynchronous outcomes.

Full scan explanations and error paths are truncated with hover disclosure ([scan detail](../src/app.rs#L762), [error path](../src/app.rs#L1583)). Add keyboard-accessible expand/copy options. Recovery must not require hovering over a long, ambiguous path.

## Competitive baseline

Research consulted official product pages on 30 October 2026. This is a documented capability comparison, not hands-on competitor testing, a performance ranking, or evidence that unlisted competitor capabilities are absent. Edition differences matter.

| Product | Documented baseline | Implication for DiskScope |
|---|---|---|
| [WizTree](https://diskanalyzer.com/about) and [search guide](https://diskanalyzer.com/guide) | Direct MFT scanning, treemap, ranked file view, detailed filters, CSV/MFT import/export | Speed, maps and search are established expectations. Measure speed/coverage on identical workloads rather than claiming superiority. |
| [TreeSize visual tour](https://www.jam-software.com/treesize/screens.shtml) | Browsing during scans, multiple charts, top files, file-age/type analysis, comparison; scheduling in Professional | History, dark themes and charts alone are not novel. Ease of understanding a comparison is a better differentiation target. |
| [WinDirStat official repository](https://github.com/windirstat/windirstat) | Lists, file-type statistics, interactive visualizations; repository describes duplicate detection and filesystem monitoring | A modern coat of paint on a treemap will not establish category leadership. Avoid feature-count competition without workflow evidence. |

The opportunity is a coherent trust-and-outcome workflow. This is a strategic hypothesis; the sources do not prove that no competitor already delivers it well.

## Proposed product direction

**Product promise:** Understand where space goes, make an informed decision, and see what actually changed.

1. **Start with the user's storage question.** Select a drive/location with clear free-space context. Make the difference between a draft location and scanned data unmistakable.
2. **Keep one investigation workspace.** Connect the distribution map, identifiable file list and selection details. Provide clear routes for “What occupies space?”, “Which files?” and “What changed?” without losing scope or history.
3. **Expose evidence and uncertainty.** Put complete/partial/stopped, scan time and missing coverage beside totals. Keep logical, allocated and observed available-space measures precise. Explain differences at the decision point, with optional detail.
4. **Offer a reviewable action.** Show path, size, relevant limitations, action eligibility and exact operation. Preserve the existing distinct permanent-delete confirmation and identity verification.
5. **Finish with a persistent outcome.** Retain action outcome, verification status, path disappearance, folder logical change and observed volume availability before/after with timestamps. Make “unknown” explicit where measurement cannot establish a result.

The final step is currently missing: completion produces a notice and rescan ([completion effects](../src/app/management.rs#L286)), while Overview shows the latest volume capacity rather than a durable before/after outcome ([volume display](../src/app/ux.rs#L249)).

Recycling normally retains storage until the Bin is emptied; a logical reduction is not proof of reclaimed disk space. Volume changes can include unrelated disk activity. An outcome panel must explain this instead of congratulating the user with an invented reclaimed-byte count. Do not automatically empty the Bin to make the number look better.

Saved scans should appear as a per-location timeline with visible baseline eligibility. Explain why a partial/errored scan cannot become a baseline, and how the global five-scan retention policy affects the current investigation. Do not interpret missing coverage as confirmed shrinkage.

If a future natural-language assistant is considered, it should translate intent into inspectable filters and explain the metadata supporting each suggestion. It must not imply content knowledge from metadata, label old files as disposable, or execute cleanup based solely on generated advice. Such a feature is optional; a better investigation workflow does not require it.

## Delivery sequence and acceptance

| Order | Work | Completion evidence |
|---|---|---|
| 1 | Fix F1 and F2; qualify native action safety | Deterministic state/event regressions; Windows fixture evidence; no unexplained stale results or premature known allocation |
| 2 | Fix F3–F8 and build/release provenance | Targeted path, state, dialog and picker tests; declared-minimum clean build; artifact-selection fixture |
| 3 | Define compact workspace, coverage hierarchy and action outcome panel | Reviewable interactive prototype with minimum-size/error/empty/partial states; task-based accessibility and layout checks |
| 4 | Implement and evaluate the investigation journey | Representative-user tasks and matched competitor trials with fixed datasets and permissions |
| 5 | Broaden release qualification | Clean Windows machines, supported filesystems/DPI, installer upgrade/uninstall, privacy, dependency advisories and signed-artifact strategy |

Suggested product targets below are proposed gates, not measured results or statistical proof:

- At least 90% task success in a representative novice study for choose → scan → identify → inspect → act/cancel → explain outcome; report participant count, failures and uncertainty.
- At least 90% correctly explain logical reduction versus recycling versus observed available-space change.
- All essential workflows operable at 900 × 640 with 100/125/150% text scales and representative Windows DPI settings, with no clipped essential controls.
- Complete keyboard/Narrator workflow; full paths and error recovery available without hover.
- No invalid comparison submissions; baseline eligibility and coverage visible before interpreting change.
- On fixed test hardware and datasets, measure time to first useful result, completed scan time, peak private bytes, query responsiveness and Stop acknowledgment; specify budgets before implementation and report cold/warm runs separately.
- Competitor task trials measure success, time to a correct answer, error recovery, confidence and preference. Claim a UX advantage only when the evidence supports it.

## Engineering strengths and remaining limits

Retain bounded channels, generation admission, cancellable delivery, native paths, checked accounting, conservative NTFS fallback, complete-only history capture, history integrity verification, separate privileged persistence policy and file identity validation. These are substantial assets.

Important remaining qualification areas include raw-volume accounting parity, unusual/blocked filesystem providers, sector-size variants, long paths, sparse/compressed/hardlinked files, cloud placeholders and redirected folders, memory at millions of records, and malformed or contended history. Existing tests cover several synthetic cases; synthetic coverage is not a matched real-volume qualification.

Distribution still needs a deliberate signing/provenance strategy and a decision on the project's source license, both already acknowledged by the repository. No license decision was made in this review. The repository has no checked-in `.github/workflows` gate, so the fresh checks and declared-minimum/release checks should become repeatable Windows CI rather than remaining one developer's local evidence.

The review supports fixing concrete defects and investing in a measured design program. It does not support calling the application issue-free, faster than its competitors, fully accessible, or a trend setter today.
