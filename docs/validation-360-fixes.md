# DiskScope 0.3.0 — 360 review remediation

Date: 26 October 2026. Windows x64; Rust 1.99.1; current working tree including prior uncommitted 0.3.0 work. No commits were created and prior source changes were preserved.

## Changes

| Review item | Resolution and regression |
|---|---|
| F1: folder filters traverse unrelated inventory | Match once inside the requested scope; normalize extension filters once; allocate no match flags without a filter. Local matches use scoped sparse IDs; whole-scan filters use a bitmap. Tests count visits/evaluations/storage with unrelated-tree growth, search-all, and nested Other highlighting. |
| F2: failed history pruning blocks later saves | Retry retention before write admission, preserve the startup catalog if pruning fails, and invalidate removed comparisons. Real Windows non-delete-sharing fixtures reproduced failures and now recover after releasing the handle. |
| F3: Types text consumes clicks | Nonselectable text lets the row activate its extension lens. Rendered application tests click extension, size, count and whitespace. |
| F4: nested icicle Other navigates to root | Navigate to the aggregate's containing node, also for keyboard activation. Rendered regression verifies the resulting Explorer root. |
| F5: benchmark uses obsolete executable | Default derives from the manifest; scan-report version is checked; absolute executable path, version and SHA256 are recorded. Explicit historical overrides remain supported. End-to-end script tests exercise current, stale and historical reports. |
| F6: invalid checksum names | Normalize package paths and derive contained relative paths consistently on PowerShell 5.1 and 7, including trailing separators and dot segments. |
| F7: raw NTFS pre-1970 date becomes epoch | Checked subtraction preserves unknown dates. FILETIME boundary regressions cover pre-epoch, epoch and one second after. |

Additional improvements:

- Cached normal-scan root queries use the scanner's maintained allocation-completeness flag. Public models retain independent validation.
- History comparison matches exact parent/native-name keys, constructs paths on demand, and tracks singleton file identities without allocating one vector per identity. Wire format, path-first semantics and hardlink ambiguity handling remain unchanged.
- Explorer launch resolves the shared Windows directory using GetSystemWindowsDirectoryW and keeps native arguments intact, independent of PATH and environment variables.
- Recycling retains a top-down chain of ordinary directory handles through Shell completion. FILE_LIST_DIRECTORY makes the read-only sharing guards effective; attribute-only handles were insufficient in the regression. Preparation remains lock-free after return, and permanent deletion retains its existing same-handle guarantee without new directory-list requirements.
- eframe/egui_extras migrated from 0.33.3 to 0.34.3. The font stack now uses skrifa/vello; ttf-parser, owned_ttf_parser and ab_glyph are absent from Cargo.lock. OS appearance/persistence/keyboard behavior was checked. New background callbacks run in App::logic even while the window is invisible.
- Table sizing handles text-scale changes, window/inspector resizing and gradual 1-point shrinking. A serialized-window-state regression also checks restoration into a smaller window.
- History documentation and UI wording now account for explicit network or redirected destinations that can store metadata off this PC.

## Measurements

Same optimized in-memory query fixture, one selected file, five calls per model:

| Total nodes | Before | After |
|---:|---:|---:|
| 10,004 | 0.415 ms | 0.013 ms |
| 100,004 | 4.259 ms | 0.003 ms |
| 1,000,004 | 50.449 ms | 0.003 ms |

These are local query measurements, not whole-drive speed or input-to-render latency. Whole-scan matching flags use 125,008 bytes for 1,000,002 nodes. Broad matching/unmatched queries remained around 218/209 ms in a small separate benchmark; no universal throughput claim is made.

The dedicated history allocation regression compares 20,000 files under a 2,400-character common prefix: approximately 10,266,748 bytes of peak extra heap, below its generous 32 MiB bound. It includes snapshot decoding and indexes. This is a fixture measurement, not a cap on arbitrary scans.

## Verification

All Cargo commands use scripts/dev.ps1, the lockfile and cached dependencies after the explicitly fetched GUI upgrade.

| Check | Result |
|---|---|
| test --offline --locked --all-features --quiet | 217 passed, 0 failed, 11 ignored |
| test --offline --locked --no-default-features --quiet | 165 passed, 0 failed, 11 ignored |
| test --offline --locked --all-features --example profile_scan --quiet | 8 passed |
| file_actions::tests with --ignored --test-threads=1 | 8 passed using disposable workspace files only |
| clippy --offline --locked --all-targets --all-features -- -D warnings | Passed |
| fmt --all -- --check | Passed after formatting |
| scripts/test-tooling.ps1 and scripts/test-benchmark.ps1 | Passed on PowerShell 7.6.5 and Windows PowerShell 5.1 |
| Full GUI render tests | All three themes at 100%, 125%, 150%; file/Types/map interaction; resize transitions; invisible-window callbacks; persistence gates passed |
| Dependency notice preflight | 359 notice files covering 185 resolved Windows packages, plus Rust 1.99.1 notices |

The all-feature total comprises 134 library, 52 binary, 2 CLI, 15 history, 1 history end-to-end, 1 allocation and 12 scanner tests. Configurations overlap; totals must not be added as distinct tests. Eight of the eleven ordinarily ignored tests were separately run; the remaining three are two explicit query benchmarks and the raw NTFS eligibility probe. Separate optimized query probes supplied the measurements above.

Ordinary Windows file-action and hardlink tests require execution outside the restricted sandbox because its drive/device queries and hardlink permissions differ. Those native runs passed. The real recycling fixture used a standard token, confirmed a Recycle Bin destination and preserved an unselected sibling. Other fixtures verified changed/replaced files, open writers, read-only preservation and junction rejection. No user documents were used for destructive tests.

The updated lockfile was compared with the official RustSec database downloaded for the review: matching vulnerability ranges remain patched or unaffected. Three informational D-Bus maintenance notices remain in cross-platform lockfile dependencies; no Windows runtime path was established for them. This was a database comparison, not cargo-audit or a third-party source audit.

Independent review covered query correctness, history semantics, native handle lifetimes, scripts, persistence and UI migration. The review found and resolved the gradual-resize edge. No actionable findings remained in the final review.

## Limits

- Windows Shell owns the final recycle leaf-name operation. Retained ancestors mitigate the reviewed ancestor replacement race; this is not atomic same-object recycling and does not lock DOS drive mappings. Distinct-principal adversarial exploitation has not been qualified. Permanent deletion still verifies and acts on the same handle.
- Recycle outcomes are now verified against the filesystem, and a file the Shell deleted instead of recycling is typed, reported and rescanned; see [Recycle Bin hardening](validation-recycle-bin-hardening.md).
- Raw NTFS remains experimental; a successful full-volume parity run was not performed.
- No interactive UAC consent, full Narrator qualification or fuzz/sanitizer campaign was performed. The GUI smoke is a bounded interaction check.
- Full-drive performance claims from older documents are not updated by these local measurements.

## Sources

- [egui 0.34.3 font dependency manifest](https://raw.githubusercontent.com/emilk/egui/0.34.3/crates/epaint/Cargo.toml)
- [eframe 0.34.3 changes](https://raw.githubusercontent.com/emilk/egui/0.34.3/crates/eframe/CHANGELOG.md)
- [Microsoft CreateFile sharing behavior](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)

## Release package

The subsequent installer/portable packaging pass adds static CRT linking, so the current executable does not require a separate VC runtime installation. That build again passed all 217 all-feature tests. Packaging prerequisites, lifecycle checks, and unsigned-distribution details are recorded in [packaging and verification](packaging.md). The application behavior changes reviewed above remain the same.

`scripts/build.ps1 -Offline -PackageDirectory '<checkout>\dist\0.3.0\'` completed using the checkout's absolute path with a trailing separator, exercising the corrected checksum path handling. The package is `dist/0.3.0/DiskScope.exe`; the prior executable was retained locally under `target/review-remediation/baseline/DiskScope.exe`.

All 444 package-file checksums were independently recomputed and every manifest path resolved. The packaged CLI scanned two generated files in a disposable workspace directory: 2 files, 1 subfolder, 5,121 logical bytes, complete, zero errors.

The actual release GUI scanned a separate tiny fixture and displayed Largest at 125% startup, 100%, 100% to 125%, and back to 100%. Numeric size/share/file columns remained visible. Screenshots are in `target/review-remediation/release-gui-125.png` and `release-gui-100.png`. The original Graphite/100% appearance was restored and confirmed in stored preferences, and only the owned smoke window was closed. History/scan fixtures were isolated; eframe preferences use the OS known-folder path rather than APPDATA overrides. A final copy-only change makes the history privacy wording consistent with configurable storage.
