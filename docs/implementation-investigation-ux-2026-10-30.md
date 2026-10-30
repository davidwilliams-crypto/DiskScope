# Reliability and investigation UX implementation plan

**Goal:** Implement the findings and product direction in engineering-ux-review-2026-10-30.md.
**Architecture:** Keep the Rust/egui application, scanner admission and native safety boundaries. Distinguish draft paths from scanned roots, and add a session-local action verification model. Adapt existing connected views rather than replacing the renderer or introducing a network AI dependency.
**Spec:** engineering-ux-review-2026-10-30.md, approved for implementation by the user's request to fix all findings and implement its UX improvements.

## Decisions

- Keep current views and shared selection as one investigation workspace, with question-oriented navigation and scope/coverage facts.
- Make compact mode responsive to logical viewport width; details become an expandable section and navigation a compact header.
- Add recent locations and local drives to the start experience. No network requests or persistence of action outcomes. Existing privileged persistence policy remains.
- Action verification reports confirmed operation outcome, reconciliation state, scan logical change and observed volume availability change. Do not call logical changes reclaimed bytes or promise recycling releases storage.
- Windows safety matrix scenarios requiring missing hardware, isolated policy changes, human UAC or clean VMs stay explicitly unqualified; run safe disposable native fixtures here.
- Source license and code-signing identity remain owner decisions; add provenance and automated Windows gates without inventing either.
- Retain existing checkout/toolchain on a dedicated local branch because ignored local toolchains and native test fixtures are required; preserve the existing untracked review report.

## Tasks

- [x] T1 Engine subtree allocation completeness: controlled event regressions, efficient aggregate propagation and cached/pure agreement.
- [x] T2 Release integrity: actual locked compiler floor, Cargo artifact selection, provenance, Windows CI and tooling regression.
- [x] T3 History/dialog UX: compatible root-grouped timeline, full root identity, eligibility copy, foremost Escape and filter labels.
- [x] T4 Scan admission: separate draft from scanned root; normalize relative paths without resolving junctions; action reconciliation regression.
- [x] T5 Investigation UX: compact navigation/inspector, state-specific empty recovery, coverage/scope, accessible keyboard lists, full error disclosure, drive/recent start choices.
- [x] T6 Action verification: session outcome card, reconciliation and measurement state, unknown values, honest recycle explanation and regression tests.
- [x] T7 Integration validation: default/diagnostic/no-GUI suites, fmt/Clippy, native disposable action fixtures, fresh native UI and layout/input checks, release scripts, independent review/fix pass.
- [x] T8 Evidence: updated validation report and review disposition for every finding and untestable matrix condition.

## Review focus

- Draft location changes must never retarget action reconciliation or a volume reply.
- Partial, cancelled, failed and filtered results must never look like verified absence or reclaimed space.
- Minimum 900 × 640 viewport at 100/125/150% must retain usable filename/size/selection/actions.
- Keyboard selection and dialog dismissal preserve text editing, cancel safety and accessible names.
- Cargo output configuration cannot select a stale executable; default locked dependencies build on the declared floor.

## Verification

Each behavioral fix receives a focused regression run before and after implementation. Integrate with cargo test --locked, diagnostic/no-default feature suites, fmt and all-target Clippy. Run ignored native file-action tests only on their disposable fixtures with ordinary Windows permissions, serially. Do not change user's Recycle Bin policies or act on user data. Document fresh native observations separately from synthetic checks and pending hardware/VM/user studies.
