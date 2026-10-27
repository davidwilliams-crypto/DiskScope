# DiskScope 0.2.1 validation

This Windows x64 preview is verified for the recorded read-only C: reporting workload. Final native keyboard/pointer qualification is incomplete because the app-control tool's approval timed out.

This performance update preserves the 0.2 interface and read-only metadata accounting. Changes target exact page ranking, event-driven worker-result delivery, incremental whole-scan type totals and a bounded 2,048-ID index for common Largest pages. Filtered/subtree queries and unsupported index requests retain their pure behavior; raw NTFS remains experimental and was not qualified for parity on this ordinary-user token.

Fresh source verification: **159 tests pass with ordinary features and 162 with diagnostics**; each configuration has three explicitly ignored probes. Formatting and all-target Clippy with warnings denied pass in both configurations. Tests cover exact full-sort page equivalence, Windows native-name ties, empty/extreme pages, incremental-summary versus pure snapshots, heap boundaries/storage/counts, mutation visibility, queue saturation, cursor traversal, cancellation/replacement, history, real filesystem scanning and NTFS fallback. A filesystem-sandbox hardlink fixture failure was resolved by running the full suites outside that tool restriction under the ordinary user token. No administrator privileges were enabled.

Independent static reviews found no Critical or Important defects. Final ordinary executable SHA-256: `e5193aab5a974187063de703a4622c2ffa99c81bff47c0b4422107ad24f3c9be`. Diagnostic timers/counters are disabled in that executable. Five final ordinary release C: runs had a median report-and-exit time of 30.633 s, with 3,791,571–3,791,572 files, 281 access failures and 431 skipped reparse entries. WinDirStat's full-tree reporting median was 43.522 s. Export work and accessible coverage differ; this is not an isolated scan-engine speed claim.

See `PERFORMANCE.md` in the portable package for measured C: results, exact scope differences, timing boundaries and resource use. Live drive scans with access failures retain partial results; export success does not mean every C: path was readable. End-to-end report timing includes different export work in the two products.

Native message-pump latency, automation round-trip durations and visible scan completion are separate measurements. The earlier 0.2 GUI and WinDirStat interactive trials are documented separately; no final 0.2.1 keyboard/pointer/paint or native Stop latency is claimed. The app-control tool returned `Computer Use app approval timed out` before that trial. No broad hardware qualification, Narrator listening qualification or usability-study claim is made.

The app performs no deletion or cleanup. Junctions and other directory reparse entries are displayed but not followed. Logical bytes count hardlink aliases; unique allocation deduplicates file identity and remains unknown when coverage/metadata is incomplete. ADS and filesystem metadata are excluded. History remains local, metadata-only, bounded, and restricted to successful complete scans; C: results with access failures are not saved as baselines.

## Portable package

The packaged executable matches the exact SHA-256 measured in all five final C: runs. Its packaged smoke scan returned exit 0, 11 files, five folders, 18,366,464 logical bytes and zero errors. The package contains 344 files, including a 343-entry payload checksum manifest. Notice collection covers all 166 resolved third-party packages with 319 notice files, plus the complete Rust 1.99.1 standard-library copyright/license notice. Package and archive integrity checks use the manifest; `PERFORMANCE.md` accompanies this validation document.
