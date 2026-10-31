# Native UI redesign — 31 October 2026

The dedicated design pass treats DiskScope as a storage investigation workspace: files, sizes, the map and the selected item have priority over navigation chrome. It retains Rust/egui, the amber accent and all three themes. It adds no dependencies or external assets.

## Changes

- Consolidated the page title and actions; retained the actual session/access persistence message, scan input, state, errors, scanned location and coverage.
- Removed separator-driven empty height and the reserved blank detail row. Full scan details remain in the state tooltip and the existing disclosure.
- Unified search, sort, scope, filters and display controls. Default empty filter/scope rows no longer consume the workspace. Active chips, pending Apply controls and navigation feedback remain visible.
- More viewport for the table and map, with alternating row surfaces and consistent spacing. Regular files no longer display misleading zero descendant-file/folder counts.
- The inspector shows current-folder figures and expandable accounting before selection; selected-item Reveal/Copy actions sit beside size information. Reclaimability qualifications and file-action guards remain intact.
- Compact file names and NAME header are left aligned; logical sizes share a right edge. List chrome is included in height budgeting so details/map retain a useful viewport. Decorative unsupported menu glyphs were removed.

## Layout evidence

At the same **1440×820 physical client**, the original NAME header was at **y584.5**. The regression now requires NAME above y350, Space map above y680 and a search field at least 360 logical points wide. It passes. This is a deterministic geometry test, not measured task-completion improvement.

A separate fourteen-row fixture exercises **1440×923 at 150% text** and **900×640 at 100%**. It verifies left-aligned name text, size edges within three logical points, and a details viewport at least 100 logical points high entirely inside the client. Existing physical-client, table resizing, all-theme/text-scale, focus, dialog and Windows AccessKit-consumer regressions are retained.

Independent review caught a missing-path notice hidden by the optional update controls. The notice now renders independently, with a real failed-focus regression in both layouts. The pending Updating cue also remains in the existing breadcrumb row. Native inspection exposed centered compact names and an almost collapsed details area; those received the explicit geometry regression and corrections.

## Native and build verification

The owned hierarchy at `research/ui-design-fixture` contains **14 files, seven folders and 141,557,760 logical bytes**. Its exact known-answer CLI scan is used for the UI candidate. Native checks use `--no-history` and generated data, with no file removals or changes to security/privacy settings.

Native checks cover wide Graphite, Overview, actual pointer selection and inspector actions, Warm light, High contrast, 150% text, compact presentation, Settings closure and search focus. Captures in the candidate directory show the interface; the fixture is synthetic and does not represent a real drive investigation. Actual Narrator and remote-session usability qualification remain outside these checks.

The final executable specifically passed the known-answer CLI scan, Graphite/warm-light wide presentation, warm-light/high-contrast 150% compact presentation, actual compact pointer selection, usable map-pane scrolling, Settings/Escape, Ctrl+F and F1/Escape focus restoration. `ext:zip` returned the expected five files / 64.0 MiB. Compact screenshots show a scrolled details pane; the complete map remains scrollable when the whole view cannot fit at once. Standard mouse and keyboard input were used for native interactions; element-index clicks were avoided after coordinate projection disagreed with the captured window.

| Final check | Result |
|---|---|
| `cargo test --locked` | 506 passed, 15 ignored, zero failures |
| `cargo test --locked --all-features` | 509 passed, 15 ignored, zero failures |
| All-target/all-feature Clippy, warnings denied | Passed |
| Rust 1.92 all-target/all-feature check | Passed |
| Formatting and diff whitespace checks | Passed |
| Independent review, including follow-up fixes | No outstanding actionable findings |
| Final Rust 1.92.0 default-feature release | Built; exact CLI/native checks passed |

Shared scenarios across configurations must not be added as unique tests. Ignored fixtures were not run or converted into passes. The final executable is `dist/candidates/0.3.3-ui-2026-10-31/DiskScope.exe`, SHA-256 `9cd578b5b79f944f6c8e0f0602c0e8f442a0a4362f73f75113572f13b04ce888`. Logs are under ignored `research/ui-redesign-*` and `research/ui-delivery-*`. The source changes remain uncommitted and reviewable; no publication or installer changes were made.

Source changes for this design pass are in `src/app.rs`, `src/app/ux.rs`, `src/app/investigation.rs` and `src/theme.rs`. The earlier search/performance work remains intact. No accounting, destructive-action, history-format or persistence policy was changed. The UI candidate is an unsigned, unpublished development build, separate from the earlier investigation candidate.
