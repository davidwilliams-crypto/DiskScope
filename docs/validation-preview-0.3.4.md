# DiskScope 0.3.4 test preview

31 October 2026. This combines the completed local search/query-performance and
native UI redesign with the reviewed modal Enter correction and preceding 0.3.3
assurance work. The original checkout was snapshotted read-only and preserved.
The installer and portable ZIP are built from one clean source commit with locked
Rust 1.92.0 dependencies and static CRT; exact identity is in BUILD_PROVENANCE.json
and SHA256SUMS.txt. These are unsigned test packages, not all-A certification.

## Included changes

- More space for file/table/map investigation, unified toolbar, aligned compact
  rows and a usable inspector at larger text sizes; all three themes retained.
- Quoted/negative terms, name/path/extension fields and exact logical-size search;
  invalid-input messages, Ctrl+F focus and F1 examples. Queries retain bounded
  candidates and maps avoid noncompetitive heap maintenance.
- Current-render modal focus restoration, bounded/scrolled dialogs, named rows,
  keyboard splitters and visible export outcomes. Both path fields reject raw
  Enter while disabled by a modal; enabled Enter still starts the intended root.
- Existing helpdesk restrictions, no-history behavior, redaction, no-clobber
  exports, resource-admission guards and action evidence remain enforced.

## Verification scope

Fresh combined-source checks passed: default **509 passed / 15 ignored**,
diagnostics **512 passed / 15 ignored**, no-GUI **335 passed / 14 ignored**.
All-target Clippy with warnings denied passed in all three configurations;
formatting, release/version tooling and independent integration review passed.
Shared tests are not separate independent scenarios. The local suites use Rust
1.99.1; the release build/floor uses 1.92.0. Pure qualification-harness checks are
not actual product qualification.

Exact-byte package, known-answer CLI, hosted and owned-client installer results
are recorded separately from historical 0.3.3 results. Prior query timing and UI
screenshot evidence describe their original artifacts, not automatically these
new bytes. Raw fresh logs are retained under research/integration-0.3.4/.

See [search behavior](investigation-search.md),
[UI design evidence](validation-ui-redesign-2026-10-31.md),
[query-performance evidence](validation-investigation-performance-2026-10-31.md) and
[prior isolated client qualification](qualification-ref-vm-2026-10-31.md).

## Remaining real-world gates

Approved signing/managed-client EDR, supported-current Windows client builds,
interactive standard/elevated/UAC contexts, Narrator/mixed-DPI, actual ScreenConnect,
provider/device and pressure/resource-class matrices, and independent technician
outcomes remain pending. The earlier 4GB disconnected client VM is Pro24H2 legacy
compatibility evidence, not completion of the current supported-client matrix.
No certificate, EDR bypass, host security change or customer-file action is part
of this test build. Use the included helpdesk runbook/checklist for qualification.
