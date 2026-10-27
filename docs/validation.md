# DiskScope 0.1 preview validation

Validated on this Windows x64 machine on 2026-10-26: AMD Ryzen 9 7950X3D (16 cores/32 logical processors), approximately 63.1 GiB visible RAM, Windows 11 Pro build 26300; the fixture resides on a fixed NTFS D: volume. Rust 1.99.1/MSVC, egui/eframe 0.33.3, Glow. This is local preview evidence; clean-machine installation and full-drive raw-NTFS parity are not established.

## Automated verification

Fresh integrated run:

```powershell
.\scripts\dev.ps1 -CargoArgs @('test','--offline','--locked','--all-targets')
.\scripts\dev.ps1 -CargoArgs @('clippy','--offline','--locked','--all-targets','--','-D','warnings')
.\scripts\dev.ps1 -CargoArgs @('fmt','--all','--check')
.\scripts\dev.ps1 -CargoArgs @('build','--release','--offline','--locked')
```

Tests: **85 passed, 0 failed, 2 ignored**: 62 library, nine binary/UI/argument, two CLI process integration and 12 scanner integration tests. Clippy passed with warnings denied. Release build passed. The ignored tests are an explicit million-node benchmark and a read-only C: metadata eligibility probe, both exercised separately.

Hardlink and directory-junction fixture creation is denied inside the tool filesystem sandbox. The final full suite ran outside that sandbox to allow these temporary fixtures; all 12 scanner tests passed. This does not add elevation or privilege changes to the app.

The final review identified three UI integration defects and three display issues; the consolidated fix adds immediate generation-local cancellation feedback, admitted-view allocation, a distinct search-result percentage scope, active engine explanations, complete accounting caveats and an accurate current-scan root label. Focused regressions passed before the final integrated run. Independent scoped re-review accepted all six fixes and found no new regression in the fix diff.

Coverage includes additive logical accounting, hardlink identity deduplication, sparse allocation, Unicode and unpaired UTF-16 paths, reparse skipping, missing roots, partial child failures, overflow, pending/cancelled unknown allocation, bounded result delivery, cancellation, replacement/waiting states, stale NTFS results, clean fallback, query paging/sorting/search, deep traversal and treemap bounds. Raw metadata tests cover checked FILE parsing/fixups/attributes/runs/reconstruction plus actual Windows overlapped file-offset reads and pending-I/O cancellation/drain.

## Native interface

The actual DiskScopeApp ran through the native eframe Glow renderer against a real fixture: 11 files, five folders, 17.5 MiB logical, no errors. Inputs exercised linked list/map selection, light theme and Map focus. Saved renderer screenshots:

- `research/diskscope-native-dark.png`
- `research/diskscope-native-light-map.png`
- `research/diskscope-native-benchmark.png`

Windows desktop capture timed out; the screenshots came from egui's native renderer screenshot event through an isolated QA wrapper. They are not browser mockups. Folder picker and Explorer reveal compile and execute off the UI thread; their external windows were not exercised by that capture route.

Across 336 real `DiskScopeApp.update` callbacks on the benchmark fixture, P95 CPU time was **3.645 ms**, maximum **11.105 ms**. Only two callbacks began while the scan state was active, so this does not prove a 120-active-frame responsiveness target. It excludes rendering, presentation and end-to-end input latency.

## Repeated comparison with WinDirStat

Original binary: official WinDirStat **2.9.3 x64**, SHA-256 `c6925752fe688aa72115723f23dbb95a7e1c55f094a7d7f30acb5f509a38045b`, byte-identical to the installed Store app.

Fixture generator: `scripts/create-benchmark-fixture.py`. Root: `research/benchmark-fixture`; **16,384 files, 256 folders, 67,171,587 logical bytes**. Every run of both applications agreed on these counts and **95,356,488 allocated bytes**; this fixture contains no hardlinks. DiskScope reported Complete and zero errors for all five runs.

Command: `scripts/benchmark.ps1` with five runs per app, alternating order. Both use the same folder and user permissions. Cache is warm/OS managed; no cache flush was attempted. WinDirStat uses its saved settings. Reports are written outside the scanned fixture.

| End-to-end report command | Five wall times (ms) | Median (ms) |
|---|---|---:|
| DiskScope `--scan ... --backend normal --json ...` | 335.078, 275.342, 338.248, 665.297, 475.535 | **338.248** |
| WinDirStat `/SaveTo ...json ...` | 554.965, 490.216, 520.081, 788.307, 709.035 | **554.965** |

DiskScope's internal scan duration median was **189.687 ms**; first nonempty result median **196.282 ms**. The raw timing/report files remain in ignored `research/benchmark-release/`. The final executable SHA-256 is `8cf520efafa7834c78cabc1916276a884d2b1f37ab3c18facd09f1d0c434d6d4`. An earlier five-run series before the UI fixes measured 295.232 ms versus 726.714 ms; variability reinforces the narrow interpretation below.

**Interpretation:** DiskScope completed this particular report workflow sooner. The work is not identical: WinDirStat exports its full 16,641-row tree, whereas DiskScope exports a summary and the largest 100 files. Startup/export costs are included. These results do not establish an isolated engine advantage, a full-drive advantage, or the design's matched-workload 20% acceleration objective.

## Scale and known limits

- A release query over a synthetic one-million-file model returned 128 rows and 2,000 map rectangles in **614.221 ms**, with stable normalized-name tie sorting. An earlier 59–70 ms measurement lacked that required name sorting and is superseded. An earlier observed process peak was about 201 MiB; current model memory is proportional to discovered entries and long names/paths increase it.
- Worker count, queues, entry batches, pending commands, visible pages and map output are bounded. Active view publication is limited to five per second. Large queries run on the coordinator without in-flight cancellation; they can delay progress/state publication while UI methods remain nonblocking.
- A synchronous normal filesystem call can hold a background worker until Windows returns. Replacement scans show Preparing while waiting for worker admission. No claim is made that every network/filesystem driver can be interrupted.
- A read-only C: raw-volume probe returned Windows error 5, Access denied. The final release CLI was also exercised against C: with NTFS requested and a one-second timeout: it visibly reported the raw access denial, selected normal fallback, retained partial results, and exited with the expected timeout code 3 without requesting elevation. No elevation was attempted. NTFS preview success is covered with synthetic parsing/reconstruction and coordinator handoff tests, but no successful live raw-volume accounting or throughput comparison was possible. Unsupported layouts and failures use a fresh normal attempt without mixing raw totals.
- Scans observe a live filesystem, not a snapshot. ADS and filesystem metadata are excluded. Directory reparse descendants are skipped. Lossy strings are used for display/search/JSON display fields; native operation paths retain their Windows representation.
- Import inspection with Microsoft's dumpbin found Windows system libraries, OpenGL and `VCRUNTIME140.dll`/Universal CRT dependencies. The application launches on this development machine; no full-drive, cold-cache, remote-share, clean-machine runtime or broad hardware/driver compatibility qualification has been completed. The app remains a preview.
