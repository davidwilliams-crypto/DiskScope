# Investigation search and query performance — 31 October 2026

This development change starts from `1d94c58` on the existing 0.3.3 development branch. It improves background queries and search without replacing the scanner, changing accounting, weakening file-action guards, or changing durable-history formats. It is not a published release or a claim of competitive superiority.

## Changes

- Depth-first traversal uses iterator frames proportional to depth instead of queuing every sibling.
- Early result pages use a bounded partition buffer and a progressively tighter ranking cutoff. Windows ending at or before result 8192 retain at most twice their window in live candidate IDs. Deep pages use the original full-candidate selection fallback. Totals count every matching file, including omitted rows.
- Overview and folder breakdown rank only their bounded visible items. Treemap and icicle reject noncompetitive heap candidates before doing heap maintenance, preserving weights, geometry and deterministic ID ties.
- Search compiles once per background query. AND terms, quoted phrases, exclusions, name/path/type fields and exact logical-size comparisons compose with the existing filters. ASCII name matching avoids a lowercase allocation; Unicode keeps lowercase matching. Path construction is lazy and limited to surviving explicit path predicates.
- Both layouts enforce 1024 characters; the parser also enforces 32 terms for non-GUI callers. Malformed input is explicit, returns zero matches and skips matching traversal. Search errors are separate from navigation notices. Requested-view exports disclose syntax validity using controlled messages while preserving redaction.
- Ctrl+F focuses search, including entering Largest from Overview/Changes. F1 works from focused search and Escape restores focus. Help descriptions wrap within their column. Invalid expressions display one correction message beside search.

## Reproducible query measurements

Windows x64, project-local Rust **1.99.1**, optimized release profile, one million synthetic direct-child files. `examples/profile_queries.rs` constructs one deterministic arena outside the timer, warms each case once, then records seven background-query durations. Before and after use the same fixture and public `build_view` path. Final benchmark checks exact returned IDs, matching counts and logical totals against a separate full-sort reference outside the timer. Unit oracles additionally cover every sort direction, ties, native unpaired UTF-16 names, pagination boundaries and fallback.

| Background query | Before median | After median | Ratio |
|---|---:|---:|---:|
| Largest, 100 rows | 68.121 ms | 20.561 ms | 3.31× |
| Broad name search, 1,000,000 matches | 115.751 ms | 41.817 ms | 2.77× |
| Selective name search, 1,180 matches | 107.293 ms | 47.609 ms | 2.25× |
| Overview, six children | 271.413 ms | 163.779 ms | 1.66× |

The initial bounded partition implementation regressed Largest/broad-search times; it was replaced with the cutoff version before delivery. Map heap admission provides a further large reduction. Final Overview ranged from 161.310 to 169.676 ms. Host load was not controlled and before/after are sequential series, not a randomized laboratory study.

These measurements exclude filesystem enumeration, model construction, rendering, input dispatch, process startup and history I/O. They do not measure full-drive scan speed, end-to-end latency, low-memory operation or performance against another product. The GUI's private root summary already accelerates some unfiltered queries; this benchmark deliberately exercises the public query path used by filtered queries and other uncached cases. The final series was repeated after the last source change.

Reproduce:

```powershell
.\scripts\dev.ps1 -CargoArgs @('run','--release','--locked','--example','profile_queries','--','1000000')
```

Deterministic work checks verify that a 100-row first page over 30,001 matches retains no more than 200 live result IDs (folder breakdown independently retains at most 12), and wide-tree traversal needs at most three iterator frames in that fixture. A decreasing-weight 100,000-entry map candidate stream performs only 100 heap updates for a 100-item budget, versus 100,000 before. These are algorithmic bounds, not process-RAM measurements. Vector capacity, matching bitmaps, the arena, expanded Explorer rows and deep-page fallback still consume memory.

## Verification

Validation logs and owned native fixtures are under ignored `research/investigation-*`. Shared tests across configurations must not be added as unique scenarios.

| Check | Result |
|---|---|
| Default suite, `cargo test --locked` | 502 passed, 15 ignored, zero failures |
| Diagnostics suite, `cargo test --locked --all-features` | 505 passed, 15 ignored, zero failures |
| No-GUI suite, `cargo test --locked --no-default-features` | 335 passed, 14 ignored, zero failures |
| All-target/all-feature Clippy with `-D warnings` | Passed |
| No-GUI all-target Clippy with `-D warnings` | Passed |
| `cargo fmt --all -- --check` and `git diff --check` | Passed |
| Rust 1.92 all-target/all-feature compatibility check | Passed; final default release also built successfully with 1.92.0 |
| Independent code review and follow-up | Reported issue fixed; no outstanding material findings |

The review found an F1/text-focus mismatch; a failing regression reproduced it before the fix. Native inspection then found help descriptions overlapping keys and duplicate search-error presentation after publication. Both received failing regressions and corrections. Independent follow-up review found no material issues in ranking, search, redaction, native-path preservation or the help wrapping fix.

The native fixture contains four owned files in two folders, with **7,548,576 logical bytes**. The final executable's CLI returned these exact complete totals with zero errors. Its native GUI returned only the 1,500,000-byte Downloads report for `path:Downloads ext:zip size:>=1.5MB -draft`. Ctrl+F, F1 while typing and Escape focus restoration passed. `size:elephant` published zero matches and exactly one visible correction message; correcting it restored the expected file. The immediately preceding executable, with the same help layout, also visually passed PageDown help examples and wrapped shortcut descriptions. Tests validate the final source's help layout and accessibility focus.

The delivered, unsigned development executable is `dist/candidates/0.3.3-investigation-2026-10-31/DiskScope.exe`, built with Rust **1.92.0**, default GUI features and the configured static CRT. Its SHA-256 is `54cf8a1f18c0e8ea8018a2aab523bacf6f31295b264db4e4aa027a404f1c8c99`. The release build and CLI record are `research/investigation-final-release.log` and `research/investigation-final-delivery-cli.json`. Native sessions used `--no-history` and only the owned fixture; no file-action testing or real-user-file removal was performed.

Remaining limits: this work does not qualify publisher signing, ScreenConnect, spoken Narrator completion, all device/token/provider cases, full-drive accounting parity for the NTFS preview or the existing action-enabled deployment matrix. Those remain described in the prior qualification documents. The local executable is an unsigned development candidate.
