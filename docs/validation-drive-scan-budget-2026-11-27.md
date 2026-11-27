# Whole-drive scan budget validation

27 November 2026. Evidence for the memory-scaled ordinary scan budget, measured
on one developer PC with a release build of the working tree (version string
0.3.4 plus this change). Helpdesk limits are unchanged and were not re-measured.

## What changed

- `ScanLimits::scaled`: an ordinary session charges its inventory estimate at
  most one third of installed RAM (`GlobalMemoryStatusEx.ullTotalPhys`, read once
  per process), never below the previous fixed 1 GiB and never above the shared
  8 GiB ceiling. The node cap is that budget divided by the per-entry charge,
  between five and sixteen million. Unknown or very small RAM keeps the previous
  default exactly.
- `ScanLimits::memory_budget`: a non-helpdesk budget's private-bytes tripwire is
  its inventory budget clamped between the previous 2 GiB and 8 GiB. The NTFS
  preview, which never charges the estimate, keeps the fixed 2 GiB tripwire and
  takes the scaled one back if it falls back to normal enumeration. The
  coordinator records the tripwire it enforces, and evidence reports that
  value rather than reconstructing it.
- `Model::reserve_node_slot`: the node arena grows by a quarter once it holds
  more than 1,024 nodes instead of doubling.
- A limit that stops a scan now names itself in the status line, the recorded
  error and the evidence refusal reason. "No complete baseline was saved." is
  said only when the coordinator holds history capture.

## Measurements

Windows 11 build 26300, 63.1 GB installed RAM, non-elevated, backend "Normal
(Windows directory metadata)", root `C:\`. Peak memory was sampled every 100 ms
from a separate process; the first run's peak was not sampled.

| Run | Limits in force | Result |
|---|---|---|
| Before, fixed default | 1 GiB estimate, 5M nodes, 2 GiB tripwire | Stopped after 43.3 s with "Estimated inventory admission budget reached. Scan resource limit reached; partial inventory retained. Choose a smaller folder." 1,462,379 files and 429,510 folders (1,891,889 nodes). 349 errors: 348 access denied and the limit. Private bytes 521 MB at the end. |
| After, scaled | 8 GiB estimate, 16M nodes, 8 GiB tripwire | Enumerated the whole drive in 66.2 s: 3,004,502 files and 655,543 folders (3,660,045 nodes), 1,563.54 GiB logical. No resource limit. 635 errors: 377 access denied (non-elevated) and 258 entries under a Go build-cache temporary folder that was deleted while the scan ran. Peak private bytes 1,446.9 MiB over 531 samples; peak working set 881 MiB; private bytes 956.7 MiB at the end. |

The drive held 3,660,045 nodes on the day, so the capped run covered 52% of it.
[The 26 October performance record](performance-c-drive-2026-10-26.md) counted
4,547,609 nodes on an earlier state of the same drive and is not the denominator.

Peak private memory was about 395 bytes per node. The
[memory audit](memory-2026-10-26.md) recorded a 2.38 GB peak for a 4.5-million-node
scan before admission existed, about 528 bytes per node; bounding the arena's
growth is what accounts for the difference.

## Regression tests

- `ordinary_limits_scale_with_installed_memory_between_fixed_bounds`
- `scaled_inventory_budgets_lift_the_memory_tripwire_only_as_far_as_the_ceiling`
- `the_ntfs_preview_keeps_the_fixed_memory_tripwire`
- `evidence_can_read_the_tripwire_the_coordinator_actually_enforces`
- `inventory_limits_are_named_by_the_scan_that_hit_them`
- `a_limit_reports_a_missing_baseline_only_when_history_could_have_saved_one`
- `limit_sizes_read_as_units_with_the_exact_byte_count`
- `node_arena_spare_capacity_stays_bounded_as_it_grows`, which fails with
  "spare capacity 122143 of 262144" when the arena is allowed to double again
- `installed_memory_probe_reports_a_plausible_total_and_is_stable`

## Not established

- Other machines. Arithmetic only: 8 GiB of RAM gives a 2.67 GiB budget, about
  five million nodes at the observed charge, with a 2.67 GiB tripwire; 16 GiB
  gives 5.33 GiB and about ten million nodes. Those margins are not measured.
- Elevated scans, spinning disks, low-memory or heavy-pressure hosts.
- Memory used by queries after a scan completes is not tripwired, as before.
- The scaled budget is an estimate of about twice a node's real cost, not an
  allocation limit; sampled tripwires can still miss a short peak.
