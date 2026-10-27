# C: drive performance investigation — 2026-10-26

DiskScope 0.2.1 completed five ordinary-user C: reporting runs in a median **30.633 seconds**, reporting about **3.79 million files**. The original 0.2.0 run timed out at 600 seconds with only 1.61 million files. The final WinDirStat reporting-command median was **43.522 seconds**. These products export different amounts of data and have slightly different coverage, so this does **not** establish an isolated scan-engine advantage. Final native keyboard/pointer verification remains blocked by the app-control tool's approval timeout.

## Final ordinary release comparison

Five sequential alternating pairs, five-second gaps, normal enumeration, four workers, warm/OS-managed caches. Neither builds nor another benchmark scan ran concurrently. Every child process was directly verified unelevated using `OpenProcessToken` and `TokenElevation`; all ten probes succeeded. This verifies elevation, not every token privilege or integrity detail.

| Pair | Order | DiskScope report-and-exit | WinDirStat report-and-exit |
| --- | --- | ---: | ---: |
| 1 | DiskScope, WinDirStat | 30.918 s | 43.362 s |
| 2 | WinDirStat, DiskScope | 30.509 s | 44.099 s |
| 3 | DiskScope, WinDirStat | 30.633 s | 43.788 s |
| 4 | WinDirStat, DiskScope | 30.924 s | 42.968 s |
| 5 | DiskScope, WinDirStat | 30.475 s | 43.522 s |
| Median | | **30.633 s** | **43.522 s** |

| Metric | DiskScope 0.2.1 | WinDirStat 2.9.3 |
| --- | ---: | ---: |
| Wall-time range | 30.475–30.924 s | 42.968–44.099 s |
| Median CPU time | 127.781 s | 130.141 s |
| Files range | 3,791,571–3,791,572 | 3,793,069–3,793,071 |
| Folders range | 756,026–756,036 | 756,086–756,096 |
| Median logical bytes | 1,807,949,626,195 | 1,821,105,653,110 |
| Access failures reported | 281 in every run | Not available in the root export |
| Skipped directory reparse entries | 431 in every run | Not available in the root export |
| Median first nonempty result | 201.631 ms | Not measured |
| Median sampled private-memory peak | 2,379,395,072 bytes | 930,983,936 bytes |
| Median observed OS working-set peak | 1,700,945,920 bytes | 972,251,136 bytes |

All five DiskScope runs correctly returned partial-results exit 4. All five WinDirStat runs returned export-success exit 0; that does not prove complete coverage. Paired differences were 1,497–1,500 fewer files, 50–60 fewer folders and 13.156 GB fewer logical bytes in DiskScope. The live inputs were not identical.

WinDirStat writes its entire tree (roughly 1.7 GB JSON), while DiskScope writes a summary and 100 largest entries. WinDirStat's output file first appeared around 31 seconds, before the full export finished; file appearance is **not** an exact scan-completion boundary. The final timings therefore compare reporting workflows, not isolated traversal or equivalent serialization. Five runs support observed medians/ranges, not statistical significance or a universal speed claim. DiskScope uses materially more memory in these measurements.

Final measured ordinary executable SHA-256: `e5193aab5a974187063de703a4622c2ffa99c81bff47c0b4422107ad24f3c9be`. Diagnostic counters/timers are disabled. Before the final bounded Largest index, the prior ordinary build (`adaaf4cb...`) had a five-run median of 41.640 s versus 43.280 s for WinDirStat; all those earlier runs are retained locally rather than discarded.

## Conditions

Windows 11 Pro build 26300, AMD Ryzen 9 7950X3D (32 logical processors), approximately 64 GB RAM. C: is NTFS, capacity 2,047,344,635,904 bytes. Live filesystem, ordinary user token, OS-managed caches. Scans run sequentially; builds and large report parsing do not overlap timed trials. No elevation, cache flushing or cleanup.

DiskScope baseline is the shipped 0.2.0 binary, SHA-256 `2018fc1f5061976f9389dca4d542571ae7b7b8bf91e68cded81e3754af5491b0`. WinDirStat is the byte-identical installed-version 2.9.3 executable, SHA-256 `c6925752fe688aa72115723f23dbb95a7e1c55f094a7d7f30acb5f509a38045b`, in a separate portable directory. Its writable INI selects ordinary enumeration, four Normal-priority workers, hardlink accounting and no automatic elevation or backup privileges. User-installed preferences are not modified.

Both include hidden/protected entries and file symlinks. DiskScope skips all directory reparse points; WinDirStat excludes junctions, mount points and directory symlinks but may follow other reparse tags. Access failures are expected. An export exit code does not establish complete coverage. Logical bytes count file entries; WinDirStat's exported physical allocation must not be equated to DiskScope's unique allocation.

## Initial headless trials

| Application | Launch-to-report-and-exit | CPU time | Peak sampled private memory | Files reported | Outcome |
| --- | ---: | ---: | ---: | ---: | --- |
| DiskScope 0.2.0, Largest 100 | 600.092 s | 526.609 s | 536,854,528 bytes | 1,608,046 | Timed out at 600 s; incomplete |
| WinDirStat 2.9.3, ordinary scan | 87.317 s | 178.938 s | 952,594,432 bytes | 3,797,320 | Export succeeded; accessible coverage |

DiskScope first nonempty result: 206.115 ms. The timeout report has 402,978 folders, 227 errors and 326 skipped directory reparse entries. WinDirStat reports 757,359 folders and 1,823,853,294,480 logical bytes. Its JSON is 1,715,704,360 bytes. DiskScope exports a summary and 100 largest entries; WinDirStat exports its entire tree. These are end-to-end reporting times with unequal export work, not isolated enumeration timings. No completion-speed ratio is inferred from the censored DiskScope run.

## Attribution

An opt-in diagnostic release preserved CLI Largest-100 requests, history-disabled Controller and 5 ms polling. It was limited to 120 seconds, so its totals are a partial diagnostic workload and have instrumentation overhead. It observed 987,739 files and 201,855 folders.

Completed coordinator spans: result queries 70.547 s, including sorting 68.664 s and subtree traversal 1.843 s; ingestion 0.439 s; scheduling 0.122 s; unconditional 2 ms sleeps 48.053 s. A final query/sort was still in flight at timeout. Sorting every matching file for a small page dominates query cost. Four workers accumulated 451.053 s in event delivery (overlapping wall time), indicating backpressure while the coordinator sorts and sleeps. Worker totals must not be added to coordinator wall totals.

Normal builds compile diagnostic spans/counters out. Instrumented timing is used to identify work; final comparisons must use ordinary release binaries.

The exact-page change first passed full-sort oracle tests across all sort keys/directions, native Windows name ties, filters, page limits and extreme offsets. Its two work-count regressions failed on the original implementation, then passed after the change. In a five-run alternating 250,000-node test-harness microbenchmark, median first-page ranking fell from 34.066 to 3.763 ms for varied sizes and 285.113 to 26.116 ms for equal sizes. The harness retains a test-only TLS comparison-counter check, so these are test-harness timings, not production scan-speed ratios.

Sequential intermediate diagnostics show why both ranking and result delivery needed attention:

| Diagnostic variant | Observed wall | Files at observation | Main attribution |
| --- | ---: | ---: | --- |
| Original Largest | 120.004 s (timeout) | 987,739 | Ranking 68.664 s; sleeps 48.053 s |
| Page ranking, original polling | 120.004 s (timeout) | 1,309,399 | Ranking 7.207 s; sleeps 104.792 s |
| Page ranking, sleep only on no progress | 319.392 s (finished with errors) | 3,791,655 | Queries 85.409 s; sleeps 229.196 s |
| Event-driven wait, pure Overview queries | 79.844 s (finished with errors) | 3,791,174 | Overview queries 49.724 s; event waits 24.069 s; literal sleeps zero |
| Event-driven wait, incremental Overview totals | 32.792 s (finished with errors) | 3,791,181 | Overview queries 0.059 s; summary updates 0.749 s; subtree-query traversal zero |
| Final bounded Largest index | 33.292 s (finished with errors) | 3,791,565 | Largest queries 0.039 s; summary/index updates 0.802 s; subtree-query traversal zero |

The 79.844-second Overview probe's external process time is 79.965 seconds and additionally includes a post-timing terminal scope query and report writing. Timing/diagnostics freeze before that extra query. Event waits unblock as results arrive; they are distinct from the old polling sleeps. A private Scan-owned root summary removes repeated type aggregation without changing the update cadence or the public pure view API. It updates from exactly the inserted IDs, resets on fallback/replacement, and retains the pure path for NTFS admission, filters and subfolders.

The final cached Overview diagnostic reports 3,791,181 files, 755,920 folders, 1,807,381,730,056 logical bytes, 281 access errors and 431 skipped directory reparse entries. First nonempty results appeared at 203.493 ms. External process time is 32.886 s, CPU time 131.828 s, peak sampled private commit 2,379,165,696 bytes and observed OS lifetime peak working set 1,697,734,656 bytes. This memory use is a material tradeoff; the implementation is not claimed to use less memory than WinDirStat. The cache adds persistent storage proportional to distinct normalized extension keys and one normalization/grouping operation per normal file, including Largest-only scans. These diagnostic figures remain separate from uninstrumented final comparisons.

Cache assertion-first verification initially found 65 subtree visits where zero were required. Differential tests then passed across interleaved batches, every root mode/sort/direction, filtering/focus boundaries, native extensions, hardlinks, unknown allocation, overflow and lifecycle changes. Public pure queries still observe direct public-model mutations.

The final Largest optimization keeps at most 2,048 file IDs (8 KiB of index payload) in a worst-first heap using the same ranking comparator. It covers unfiltered whole-scan Logical-descending pages within that prefix. Deeper pages, other orders, filters and subfolders retain pure queries. Total row counts still include every matching file, and IDs retrieve current metadata, including later hardlink flags. Its assertion-first regression also initially visited 65 nodes, then zero after implementation; exact snapshot, heap-boundary, native-name and lifecycle oracles passed. The diagnostic final Largest run reported 289 read failures rather than the final ordinary batch's 281, illustrating live-scan variation; that run is retained separately and not substituted into the ordinary comparison.

Full final source checks pass: **159 ordinary tests and 162 diagnostic tests**, each with three explicit ignored probes; formatting and both all-target Clippy configurations with warnings denied also pass. Independent static reviews of paging, event waiting, type summaries and the final heap found no Critical or Important defects.

## Coverage reconciliation

A bounded-memory parse of WinDirStat's 4,554,680 JSON records exactly reconciles files, folders, logical and exported physical bytes to its root totals. No unexpected record types occur. The Overview probe's 55 immediate-root rows also exactly reconcile its own totals.

Compared with the initial WinDirStat report, that DiskScope result contains 6,146 fewer files (0.161851%), 1,449 fewer folders (0.191323%) and 16,480,966,177 fewer logical bytes (0.903634%). Thirty-six of forty anonymous aggregate scopes agree. Most of the byte gap is in user-profile data, followed by installed application data. All 281 sampled DiskScope error messages are access denied. Those messages do not associate errors with scopes, and descendant reparse tags were not inventoried; permission coverage, directory-reparse policy and live changes remain possible causes. An exact-coverage engine-speed ratio is unsupported. No accounting or exclusion policy is weakened for the optimizations.

The later cached probe adds local-only error paths and fully reconciles its own 55 immediate-root rows. Relative to the same older WinDirStat snapshot it reports 6,139 fewer files, 1,439 fewer folders and 16,471,564,424 fewer logical bytes. Error grouping finds 150 access failures in shared application data, 76 in user profiles, 47 in the OS scope and eight elsewhere. The installed-application scope has a 1,666,124,629-byte deficit with no observed access failures, so access errors cannot by themselves explain all gaps. Without missing-entry and descendant-tag evidence, causation remains unresolved; no exact-coverage speed ratio follows.

## Native GUI protocol

Separate visible scan trials use a process-owned-window WM_NULL probe every 250 ms, bounded at one second, plus actual native keyboard/view interactions. WM_NULL measures message-pump service only. Automation action and snapshot round trips include automation overhead and are reported separately from input/render latency. Completion is bounded by observed busy and ready states, not inferred from CPU or report creation.

Raw reports, filenames, UI trees, screenshots and error paths stay in the ignored local investigation directory. Only aggregate measurements belong in this report.

Stock DiskScope's mixed interactive GUI trial was stopped after 302.853 seconds with 1,770,202 files and 436,354 folders. Ctrl+K/Escape, view switching, selection and maximize/restore were observed during scanning. In the explicit observed active interval, 1,165/1,165 WM_NULL probes responded: p50 0.079 ms, p95 5.75 ms, p99 24.85 ms, maximum 115.01 ms. These are message-pump measurements, not input-to-render timings. UIA-only stale progress observations were not backed by simultaneous idle pixel captures and do not establish stale rendering or a backend stall.

The first WinDirStat GUI trial produced a completed-looking treemap between 33.279 and 40.965 seconds, but subsequent probes/close input were delayed. Compilation began after the premature completion report, so late readiness data is contended. The 41-second observation is only a visual completion candidate and is excluded from a clean usable-completion comparison. A clean repeat is required. Window geometry and foreground visibility also differed in that first pair.

The clean WinDirStat repeat used normal scanning with no competing benchmark workload, maximized to 3413×912. It first showed a completed-looking treemap 43.122 seconds after the trigger. A Users-row selection was first seen 51.085 seconds after its click; this is an observation bound, not precise input-to-render latency. Over the capped 300-second interval, 148 of 937 WM_NULL probes succeeded and 789 failed. Sixty failures had error 1460; 729 returned last-error zero and remain ambiguous abort-if-hung failures rather than automatically one-second timeouts. The target owned the foreground for 878/937 probes, including 730 failures. The first failed probe preceded the sole post-trigger UIA text read, which returned in about 55 ms. Ten other explicit captures were screenshot-only.

The preregistered post-input criterion—20 consecutive successful probes spanning at least five seconds with the target in the foreground—was not met within 300 seconds. This is an **automation-protocol outcome**, not proof of an intrinsic ordinary-use freeze. Screenshot/input observation may perturb the application. The additional no-Largest, screenshot-only control was not run after the app-control tooling stalled. DiskScope also renders a bounded scoped map rather than WinDirStat's own complete-file treemap, so render workloads are not equivalent.

The final DiskScope native input/Stop trial was not triggered. The native window/state call stalled; the alternate app-control route then returned `Computer Use app approval timed out`. Consequently final-version keyboard, pointer, paint latency and native Stop acknowledgement are **unverified**. Engine queries, cancellation/generation behavior and full C: throughput are separately covered by tests and the recorded diagnostic/release runs. No UI responsiveness speedup is inferred from background query time.

## Subsequent 0.3.0 native checks

The later administrator/file-actions release completed functional keyboard/pointer and C-drive Stop checks on its own final executable. See [0.3.0 validation](validation-0.3.0.md). Those checks do not retrospectively qualify the blocked 0.2.1 native trial, measure end-to-end input latency, or create a matched GUI completion-speed comparison.

## Reproducing the reporting workload

For the GUI-subsystem executable, explicitly wait for the process. `ArgumentList` preserves the trailing slash in `C:\` without hand-built Windows quoting:

```powershell
$scanInfo = [Diagnostics.ProcessStartInfo]::new()
$scanInfo.FileName = (Resolve-Path '.\dist\0.2.1\DiskScope.exe').Path
$scanInfo.UseShellExecute = $false
foreach ($scanArgument in @('--scan','C:\','--backend','normal','--timeout','600','--json','D:\diskscope-report.json')) {
    $scanInfo.ArgumentList.Add($scanArgument)
}
$scanProcess = [Diagnostics.Process]::Start($scanInfo)
$scanProcess.WaitForExit()
$scanProcess.ExitCode
```

The reference command uses the isolated official-version WinDirStat executable with `/SaveTo D:\windirstat-report.json C:\` and the controlled adjacent writable INI. Relevant keys are `AutoElevate=0`, `ShowElevationPrompt=0`, `UseFastScanEngine=0`, `UseBackupRestore=0`, `ScanningThreads=4`, `ProcessPriority=1`, `ProcessHardlinks=1`, `ExcludeJunctions=1`, `ExcludeVolumeMountPoints=1`, `ExcludeSymbolicLinksDirectory=1`, `ExcludeSymbolicLinksFile=0`; hidden/protected exclusions and Dropbox-ignore checking are disabled, and include/exclude name filters are empty. Do not change the user's installed preferences to reproduce a trial.

Opt-in attribution uses `cargo build --release --no-default-features --features diagnostics --example profile_scan`, then the explicit `--scan`, `--view overview|largest`, `--backend normal`, `--timeout` and `--json` arguments. That diagnostic example includes an additional post-timing coverage query, unlike ordinary CLI reporting. Raw reports, trial binaries, samplers and private analysis remain in the local ignored investigation directory. The portable package includes this aggregate report, its validation notes, dependency notices and payload checksums; no C: filenames or errors are bundled.
