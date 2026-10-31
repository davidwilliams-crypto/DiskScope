# DiskScope: local PC testing and revised assessment

Later independent review, the modal correction and isolated client results are
recorded in [the Ref VM follow-up](qualification-ref-vm-2026-10-31.md). That report
uses verified/failed/pending scope gates instead of uncalibrated numerical grades.
The historical scores and artifact identities below describe the earlier candidate.

Evidence completed 30 October 2026 Halifax / 31 October UTC. **The all-90/A
deployment target is not met.** Actual desktop testing found and reproduced
defects, which were fixed and retested. The final unpublished candidate is
source `b4d12e16234ef132af67344dce98f26c34d5b4b0`, built clean with Rust 1.92.0.
Main and published 0.3.2 remain unchanged. This report supplements the
[earlier assessment](helpdesk-rescore-0.3.3.md), whose older artifact identities
and observations remain historical evidence.

## Fixes and observed results

- Modal dialogs now initialize and contain keyboard focus, block background
  shortcuts and restore only a currently rendered invoker. Static table cells
  no longer create anonymous focus stops; rows are named and column splitters
  support bounded keyboard resizing with visible focus.
- Native Settings -> 150% text -> Escape crashed the intermediate candidate
  with exit 101: AccessKit focus referred to a removed widget. The rejected
  candidate is retained and marked DO NOT DEPLOY. Current-render focus checks,
  safe fallback and a stable compact-menu invoker fix the reproduced cause.
  Regression frames now validate against the Windows AccessKit Consumer tree.
- All seven transient dialogs fit the available viewport and scroll, including
  keyboard/focus reveal. Export displays success/error inside the modal; busy
  Save retains focus and guards against duplicate work. Private sessions start
  maximized within the monitor work area.
- The exact final executable survived Settings at 150%, PageDown, Escape and
  compact-menu export. A native Unicode-path scan returned 3 files, 1 folder,
  4,107 logical bytes and zero errors. Redacted export omitted root, hostname
  and fictional incident labels. A repeated Save displayed the existing-file
  error without changing the report. Escape restored the stable menu button.
  Normal exit was code 0 with empty stderr. Native profile-directory existence,
  names, lengths, content hashes and LastWriteUTC were unchanged before/after.
  That comparison does not prove absence of reads or transient writes.

Raw exact-final evidence: `research/local-pc-desktop/final-native/`,
`research/local-pc-desktop/observations.ndjson` and
`research/local-pc-profile-native/comparison.json`. The observation stream also
contains earlier binaries; use the recorded hash boundaries. Earlier native
splitter/help/scan/filter/Delete-refusal cases and the rejected crash are
described with their own binary scope in
[the implementation validation](validation-local-pc-2026-10-31.md).

## Revised scores

Scores are coarse engineering judgments of implemented and tested scope, not
measured reliability percentages or operational sign-off. Ten of eighteen
implementation judgments reach 90; eight still fall below it. **No overall
all-A deployment qualification is established.** Required pending gates are not
converted to passes by averaging these scores.

| Area | Previous | Now | Evidence and remaining work |
|---|---:|---:|---|
| Normal scanning/accounting | 90 | 90 / A | Known-answer native/CLI and filesystem fixtures pass; device/provider matrix remains. |
| Enforced diagnostic safety | 95 | 95 / A | Helpdesk restrictions and persistence guards pass; retain disabled file actions for diagnostic use. |
| Action-enabled cleanup | 70 | 70 / C | Native owned-data action/journal fixtures pass; token, Bin-policy, provider and interruption matrix remains. |
| Permissions/elevation | 85 | 85 / B | Refusal guards pass; client standard/split/elevated/alternate/SYSTEM and UAC outcomes remain. |
| Privacy | 90 | 90 / A | Actual redacted export, no-history evidence and profile preservation add coverage; redirected/locked/roaming and token cases remain. |
| History/repeat investigations | 85 | 90 / A | Tagged 0.3.2/current codec interoperability, protected baseline, retention, Clear and actual Controller persistence pass. Old shipped GUI and large durable-history workloads remain unqualified. |
| Performance/resources | 85 | 85 / B | Real 96,384-file fixtures, sampled memory, actual inventory refusal and cancellation add evidence; 4/8/16GB pressure, near-full volumes and million-file filesystem work remain. |
| Windows/device compatibility | 80 | 80 / B | Existing Windows 11 host and hosted Server tests pass; supported clean clients, BitLocker, device/provider/live-volume cases remain. |
| Investigation UX | 85 | 90 / A | Native focus, scaling, export-feedback and crash defects fixed/retested. Technician task completion, interpretation and competitive differentiation remain unmeasured. |
| Accessibility | 85 | 85 / B | Keyboard and actual AccessKit tree coverage improved; spoken Narrator completion, mixed DPI and remote accessibility remain. |
| ScreenConnect deployment | 80 | 80 / B | PS7/5.1 helpers and qualification pass in their bounded scope; actual tenant/session/token/reconnect trials remain. |
| Artifact integrity | 95 | 95 / A | Clean provenance and all 406 ZIP payload files verified. Hash integrity does not establish publisher trust. |
| Installer lifecycle | 85 | 85 / B | Final-source hosted fresh install/reinstall/uninstall and binary upgrade/rollback pass; supported client/token/runtime-history qualification remains. |
| Publisher trust | 70 | 70 / C | Executable and installer are unsigned. Approved signing, timestamp/chain and actual managed-client EDR/trust acceptance remain. |
| Automated QA | 95 | 95 / A | Three feature configurations, warning-denied lint, native fixtures and hosted gates pass; disclosed ignored cases and manual gates remain. |
| Supportability/evidence | 90 | 95 / A | Exact-candidate evidence, native export, contextual failures and journal preservation verified; fleet investigation/pilot review remains. |
| Documentation | 90 | 90 / A | Candidate identities, failed-candidate disposition, checklist, runbook and limitations retained; operational reviewer sign-off remains. |
| Maintainability | 90 | 90 / A | Deterministic regression frames, public table APIs, AccessKit consumer validation and independent diff review add regression protection. |

## Final tests and artifact identity

- Default: **482 passed, 15 ignored**; diagnostics: **485 passed, 15 ignored**;
  no-GUI: **320 passed, 14 ignored**. Shared tests must not be summed as unique
  scenarios. All-target Clippy with warnings denied passed in all three
  configurations; formatting and independent code review passed. Local suites
  used Rust 1.99.1; final build/hosted compiler-floor checks used 1.92.0.
- The explicitly selected native journal timing/dismissal fixture passed again
  on final source. Earlier eleven native owned-data action/recycle fixtures and
  history/privacy tests are scoped in the linked validation records. Native
  filesystem execution was required where sandboxed Windows operations refused;
  application protections were not weakened.
- Exact final executable scanned 16,384 files / 64 folders / 67,128,066 bytes
  and 80,000 files / 0 folders / 4,031 bytes. Independent formula checks verified
  complete totals, zero errors, largest-100 sizes/order and honest truncation.
  End-to-end processes took 129ms and 140ms. Highest sampled private memory was
  31,584,256 bytes (3-4 samples at 10ms); short peaks may be missed. These are
  warm local fixtures on a roughly 64GB host, not low-memory or fleet benchmarks.
  See `research/local-pc-resources-final/`. Earlier 17-run sequences retain their
  separate executable hashes; their unchanged content digests are not presented
  as a newly repeated final-binary full-content audit.
- Exact-executable qualification on both PowerShell 7 and 5.1 retained **six
  automated passes, twelve manual pending**, A eligibility false and numerical
  readiness null. Package hashes bind the PS7 record; package hashing alone
  executes no installer. See `research/local-pc-desktop/qualification-final-ps*/`.
- ZIP verification checked all **406 files**, including executable, qualification
  docs, MIT license and dependency/Rust notices, against the staged payload.
  Evidence: `research/local-pc-desktop/final-artifact-verification.json`.

| Local artifact | SHA-256 |
|---|---|
| `dist/candidates/0.3.3-pc-final/DiskScope.exe` | `8c67102c001b3345118baa11b6f4537c64d5d120da22c052114736fabb58a6c6` |
| `dist/candidates/releases/0.3.3-pc-final/DiskScope-0.3.3-windows-x64-setup.exe` | `cd61f9782a2c1d65eba6b6bd2a8ec2f53b96f17c45162a3d793795453855a693` |
| `dist/candidates/releases/0.3.3-pc-final/DiskScope-0.3.3-windows-x64-portable.zip` | `fd26dc6195b497ac6fffa7e3ae311a7804db2d8c9cd324b13407490c4bf0ce2c` |

These bytes are an **unsigned, unpublished qualification candidate**. Do not use
the similarly named `0.3.3-pc-qualified` intermediate directory; it contains the
rejected crash build. Documentation updates after b4d do not change final bytes.

## Final hosted results

[Windows validation](https://github.com/davidwilliams-crypto/DiskScope/actions/runs/41156261692)
passed all five jobs: default, diagnostics, no-GUI, supply-chain and compiler
floor. Configuration-specific skipped steps are deliberate and are not extra
passes. [Installer qualification](https://github.com/davidwilliams-crypto/DiskScope/actions/runs/41156261691)
passed on clean b4d source, Windows Server 2022 build 20348, hosted image
`win22` / `20261026.347.1`.

Three actual lifecycle cases passed: custom path/default shortcuts, custom
path/no shortcuts, and default directory. The separate baseline 0.3.2 install,
candidate upgrade, binary rollback and uninstall all passed, including exact
payload/CLI checks and opaque canary preservation. The 2,903 assertions in that
sequence are mostly per-file checks, not independent deployment scenarios.
Failure and cleanup_failure are null. Both hosted PowerShell qualification
records preserve six automated passes/twelve manual pending.

Hosted hashes differ from the local build: executable
`9216943591e0321f147b1afb9661ce1f53978ef8b018c6d86add66b5ac4e5041`,
installer `c745d99a0935bec7729aa55db59e1a7de4afbaf142deb5d69ff5786c59ad913e`,
ZIP `e9af26632dac8e10aa2a989f472d9b23d114e2d5fd68004b2f1f0049def69750`.
Do not transfer exact-byte installer claims to local packages. Downloaded evidence
is under `target/hosted-qualification-b4d12e1/`; GitHub retention is 14 days.

## Work required before confirming operational 90/A

1. **Client qualification environment:** the approved DD-Lab-Target1 has a missing
   configured OS disk, so no clone could be created. `release.vhdx` is only a
   data disk. A verified Windows 11 Pro 26100 DD-Lab-Parity-Ref disk chain is
   available, but using that alternate source awaits authorization. The concrete
   plan makes an independent, network-disconnected 4GB VM and preserves all
   originals. No original VM was started or changed. An existing lab image is
   not automatically a clean vendor baseline.
2. **Publisher/endpoint security:** obtain the approved certificate, sign and
   timestamp a new candidate, rehash/retest those new bytes, and verify real
   fleet trust/EDR behavior. Malwarebytes registration and Defender's Not running
   status were observed read-only; no actual EDR acceptance scan was completed.
   No protection, service, exclusion, certificate store or trust policy changed.
3. **Actual contexts:** execute supported client token/UAC, provider/device,
   privacy/profile, installer/runtime-history, constrained-resource and native
   action cells. Keep cleanup disabled for diagnostic qualification. A unit test
   or Server lifecycle is insufficient evidence for these client cases.
4. **Remote/accessibility:** run real ScreenConnect launch/handoff/reconnect and
   Narrator/keyboard/mixed-DPI tasks. Narrator's first-run ShellHost control
   approval timed out; it was exited normally and no spoken-feedback pass is
   claimed. AccessKit correctness alone cannot complete that gate.
5. **Helpdesk tasks:** run 30-50 investigations with 3-5 technicians on 10-20
   representative IT-owned endpoints. Require at least 90% correct task completion
   and interpretation, zero safety-critical misconceptions, and no unresolved
   critical safety/privacy/accessibility defect. Record time and resource budgets
   before execution; retain and fix failures, then repeat affected cases.

Use the [complete checklist](helpdesk-qualification.md),
[manual evidence template](qualification-manual-template.json),
[automation guide](qualification-automation.md) and [runbook](helpdesk-runbook.md).
All twelve broad manual gates remain pending because local component evidence
does not complete their full matrices. Signing, real tenant access, representative
hardware and independent user outcomes cannot be manufactured by retesting this PC.
