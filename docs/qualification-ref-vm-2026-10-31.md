# Isolated Windows 11 client qualification: 31 October 2026

Status: the bounded administrative/noninteractive guest sequence is complete and
passed. This is an unsigned, unpublished candidate, not approval for broad helpdesk
deployment or a calibrated 90/A grade. Missing contexts remain pending below.

## Candidate and independent-review correction

The independent reviewer reproduced a P2 modal-input defect: when the edited root
field had focus, Ctrl+K plus Enter in one frame could start a background scan. Both
wide and compact handlers now require an enabled UI for raw Enter completion.
Actual Tab-driven frame regressions failed on both paths before the fix and pass
after it; ordinary Enter still starts the intended root. Independent review approved
the narrow fix with no actionable finding. No destructive-action bypass was observed.

Clean candidate source: `e751c1f55627630311230778ba288e21b214d0cc`, Rust 1.92.0.
The isolated branch is `codex/modal-enter-qualification`. New uncommitted search/UI
changes in the original D: checkout were preserved and are **not included or
qualified** by this candidate. Published main and 0.3.2 remain unchanged.

| Local artifact | SHA-256 |
|---|---|
| `dist/candidates/0.3.3-vm-modal/DiskScope.exe` | `9938c652255a14e9600da6d11439e14ecf83ddbbe849e44c2c4cb0a741bc677c` |
| `dist/candidates/releases/0.3.3-vm-modal/DiskScope-0.3.3-windows-x64-setup.exe` | `e0f760796ad98bfb8fd5856b29b0d4ffade43a78c69ce35713263e4028d7f68d` |
| `dist/candidates/releases/0.3.3-vm-modal/DiskScope-0.3.3-windows-x64-portable.zip` | `e2ef23b14d1c878514b6e5618bb21d2348d6dbd25f30fe7c3d0b80c55a6fbf7f` |

All 406 ZIP entries matched the exact staged payload. Both executable and installer
are NotSigned. PS7 and PS5.1 exact-executable qualification each retained six
automated passes and twelve broad manual gates pending; numerical readiness remains
null and A eligibility false. The PS7 record also binds local package hashes.

Default: **485 passed / 15 ignored**; diagnostics: **488 passed / 15 ignored**.
All-target default warning-denied Clippy and formatting passed. These local suites
used Rust 1.99.1; the optimized candidate and hosted floor use 1.92.0. Shared tests
are not separate independent scenarios. Full baseline default tests passed before
changes. New regressions cover wide/compact modal refusal and enabled completion.

[Windows validation](https://github.com/davidwilliams-crypto/DiskScope/actions/runs/41213977432)
and [hosted installer qualification](https://github.com/davidwilliams-crypto/DiskScope/actions/runs/41213976627)
passed on e751. The latter's three fresh lifecycle modes plus published 0.3.2 binary
upgrade/rollback/uninstall passed; failure and cleanup_failure are null. The
2,903 upgrade assertions are mostly payload checks, not deployment scenarios.
Both hosted CLI qualification hosts preserve pending manual gates. Environment:
Windows Server 2022 build 20348, image win22/20261026.347.1. Hosted hashes are separate:
exe `ebdb31215bc110104059f1502f768ecc014dcda8fccecc31bb28d6f6fe4717e7`,
installer `290d9572cea7d0bd9ebc57ef31bfe17cf866985e75a99e7a6ae1037e8acd87c5`,
ZIP `5a194b4fa3153780dc0aec877bd86da0d97063e324d0bf019de4b0a2225a5486`.
Downloaded records: original workspace `target/hosted-qualification-e751c1f/`.
Hosted success does not establish local-package installer behavior on a client.

## VM provenance and isolation

The user approved cloning DD-Lab-Parity-Ref. Target1's disk is missing;
release.vhdx is only a data disk and was not used as a boot source. The current Ref
leaf and its full chain were flattened into an independent dynamic os.vhdx with
empty ParentPath. All nine source-chain/configuration SHA-256 hashes matched before
and after copying and again after the owned clone booted. All original DD-Lab VMs
remained Off. No original disk linkage, firmware, TPM or VM configuration changed.

Owned VM: `DiskScope-QA-20261031-Ref-01`,
ID `ace3ddae-6d73-4220-9bc8-f8f057e229b7`, under original workspace
`research/local-pc-vm/DiskScope-QA-20261031-Ref-01/`. Gen2; static 4GiB RAM;
2 vCPU; zero network adapters; no DVD; no automatic startup/checkpoints; Secure
Boot On/MicrosoftWindows and new clone-local TPM. The roughly 19.2GB physical disk
is retained for follow-up. This is an existing parity lab image, not a clean vendor
baseline. No NIC was added to obtain updates, credentials or test tools.

Actual guest: Windows 11 Pro/Professional x64 **26100.4652**, PARITYREF,
4,293,808,128 visible physical bytes. PowerShell Direct uses the targeted lab's
existing Ref credential without printing or exporting its secret. Actual token
PARITYREF/labadmin is administrative. Live BitLocker reported C: FullyDecrypted,
0% encryption, ProtectionOff and Unlocked. Offline read-only mount was refused by
the host token's missing privilege; no privilege/UAC/security change overrode that.
The live query is the encryption observation, not an invented offline inspection.

Live registry DisplayVersion confirms **24H2** and EditionID Professional; Win32
caption confirms Windows 11 Pro (the compatibility ProductName registry value still
says Windows 10 Pro). As of 31 October 2026, this Pro release has reached end of
updates according to [Microsoft release information](https://learn.microsoft.com/en-us/windows/release-health/windows11-release-information)
and [the product lifecycle](https://learn.microsoft.com/en-us/lifecycle/products/windows-11-home-and-pro).
These are legacy-client results. A currently supported client image still needs
qualification; no update, OS upgrade or NIC was introduced to change this clone.

No interactive guest user was signed in. Disposable standard-account process launch
failed before app output with native status 0xC0000142 (DLL initialization failure).
Those accounts were not promoted, existing passwords were not reset, and no further
account retries were used to manufacture a standard-user pass. Administrative
PowerShell Direct does not qualify interactive standard-user, UAC or Narrator tasks.

A measured read-only clock comparison found guest UTC about **3,599.174 seconds
ahead** of host midpoint, with 9.95ms call latency. No clock/sync/timezone setting
changed. Retain host and guest timestamps as observed, not a synchronized timeline.
Process durations use monotonic Stopwatch measurements.

## Guest results

The independent guest-specific sequence completed **34 scoped check rows** with
**103 native child process records**. All check rows passed. These counts overlap
and include setup/refusal checks; they are not independent deployments, user trials
or a reliability percentage. Root independently inspected the raw sequence,
assertions, downloaded reports and final states, rather than relying only on the
worker's summary.

Actual native context was PARITYREF/labadmin, administrator/high integrity,
**SessionId 0, UserInteractive false**. Standard-user native initialization failed;
no standard/UAC/interactive pass is implied by the following table.

| Exercised scope | Result | Evidence and limits |
|---|---|---|
| Exact package transfer | Verified | All six pinned candidate/baseline/test-binary inputs and extracted candidate hash matched; rechecked unchanged after execution. |
| CLI known answer | Verified | Unicode/space root, 3 files, 2 folders, 4,107 logical bytes, complete, zero errors. |
| Existing report and missing root | Verified | Direct existing-report exit1/error80 with report hash unchanged; missing root exit1 with honest incomplete/error evidence. |
| Actual 4GiB workloads | Verified within fixtures | Three runs each: 16,384 files/64 folders/67,128,066 bytes; 80,000 files/0 folders/4,031 bytes. All complete with zero errors. |
| Independent largest-file/coverage checks | Verified | Root separately checked all six raw reports against the fixture formula: largest 100 sizes/order, Unicode canary, matching row counts and explicit truncation/non-full-inventory flags. |
| Native file-action safety source tests | Verified within fixtures | Seven explicitly selected permanent-delete/changed/replaced/open-writer/read-only/junction tests each actually ran and passed. No blanket ignored-test execution, Shell recycle or interactive action UI claim. |
| Modal correction source tests in guest | Verified | Three exact headless application-frame tests passed, including both layouts and positive enabled Enter. This is not native keystroke injection or a spoken-accessibility trial. |
| Silent installer lifecycle | Verified for admin Session0 | Custom path/default Start menu shortcut, custom path/NoShortcuts and per-user default path. Each passed exact 406 payload hashes, registration, installed CLI, reinstall, move refusal, locked update/uninstall refusal and final uninstall with unrelated sentinel preserved. |
| Pinned published binary upgrade/rollback | Verified for admin Session0 | Actual 0.3.2 install -> exact local candidate upgrade -> pinned0.3.2 binary rollback -> uninstall. Every stage matched all 406 files and expected CLI version/results. Program/profile opaque canaries survived; no runtime history-format claim. |
| Whole-fixture preservation | Verified | Before/after names, lengths and per-file-content digest aggregates matched for all 96,387 files/66 folders/67,136,204 logical bytes. |
| Targeted guest cleanup | Verified | Initially absent DiskScope registrations in all four HKCU/HKLM views absent after uninstall; no DiskScope process remained. The exact owned profile canary was copied to evidence then removed only with its initially absent empty parent. Targeted labadmin DiskScope paths returned absent; entire profile preservation is not claimed. |
| Original protection and final state | Verified | Nine original disk/config hashes unchanged, standalone clone disk/no NIC. Normal shutdown completed; clone and all ten original DD-Lab VMs Off. Clone disk, owned QA accounts and fixture/evidence data retained for follow-up. |

The six larger native CLI processes took **88-135ms** end to end on this guest.
Highest sampled private commit was **41,209,856 bytes** (about 39.3 MiB), with 6-9
samples per process at 10ms intervals. Short peaks can be missed. These mostly
zero-content warm fixtures in a real 4GiB VM do not establish forced memory/commit
pressure, admission-cap behavior, GUI responsiveness/Stop latency, a million-file
filesystem inventory, near-full disks, other resource classes or fleet performance.

The native source-test binaries were built from e751 using Rust 1.99.1, frozen and
hashed separately from the Rust 1.92 release application. Their hashes are retained
in `research/local-pc-vm/modal-candidate-artifacts.json` and
`modal-test-binaries/`. The guest's initially absent matching compiled manifest
path was created only as an owned source-fixture namespace; fixture guards were not
weakened. No source-test pass is presented as execution of a release GUI workflow.

Raw evidence lives in the original D: workspace under
`research/local-pc-vm/DiskScope-QA-20261031-Ref-01/`, notably:

- `admin-attempt2-native-sequence-summary-639290573037994136.json` and separately
  retained per-case/per-process JSON.
- `admin-attempt2-guest-evidence-639290573038761182/`: raw CLI reports and stdout/stderr.
- `admin-attempt2-fixtures-before-native-runs.json` and `...after-native-runs.json`.
- `admin-attempt2-after-native-candidate-integrity.json`, final guest/host state,
  `live-windows-release-metadata.json`, source-copy and after-boot hashes.
- Root independent recheck: `research/local-pc-vm/verify-client-outcomes.py` and
  `verified-e751-client-outcomes.json`; hosted recheck `modal-hosted-verification.json`.

The owned VM remains available under the path/ID above and is Off. No installation
or test account was created on the physical host by these VM qualification steps.

## Retained failures and boundaries

- Guest file-based .ps1 execution was denied by effective Restricted policy;
  every execution-policy scope was Undefined. That harness did not run. No Bypass,
  Unrestricted, trust or policy change was used, and denied script content was not
  lifted into an inline runner. The guest-specific sequence uses permitted direct
  APIs/cmdlets and native executables; its results are its own bounded evidence.
- Preparation first failed on an overlong disposable-account description, then on
  bool ArgumentList parsing and null Start-Process exit capture. Actual whoami
  exposed the admin token; that attempt was not counted as standard-user evidence.
  A direct native process runner corrected token/handle capture before candidate
  execution. Existing raw failures remain retained.
- The first native candidate sequence incorrectly expected wrapper exit20 from a
  direct-executable existing-report refusal. Actual exit1 with Windows error80
  was correct. Only the harness expectation was corrected; replacement attempt
  output names are unique and earlier evidence is retained.
- Automatic approval review rejected an Enter keystroke in an ambiguous persistent
  terminal showing an unrelated destructive shell suggestion. No deletion was
  issued. Pending input was canceled; explicit noninteractive file/command
  invocation replaced UI terminal automation. No host system-file deletion or
  security bypass was authorized or performed.

## Readiness framework

Use **verified / failed / pending for an explicit scope**, rather than treating
uncalibrated 85/90/95 judgments as measured readiness. Known-answer accounting,
identity hashes and specific safety guards have useful evidence; they cannot
substitute for actual supported token/device/context qualification or user outcomes.

A 30-50 investigation pilot with >=90% correct results is an observed acceptance
rule, not proof of population success >=90%. Predeclare unassisted first attempts,
representative task/endpoint strata, assistance and every failure; keep exploratory
training/fixes separate from fresh confirmation. Repeated technicians/endpoints and
coaching invalidate naive independent-trial confidence claims. Zero critical events
in a small pilot does not establish rare-event safety.

Actual interactive desktop/standard token/UAC, spoken accessibility, mixed DPI,
ScreenConnect, approved publisher/managed-client EDR acceptance, provider/device and
remaining resource-class/pressure matrices, and independent technician outcomes
remain pending. A currently supported client image and a user-authenticated guest
desktop are the next practical test prerequisites; no authentication UI was
automated. Action-enabled cleanup, raw NTFS and whole-drive durable-history
claims remain outside the initial diagnostic-only pilot. See the
[checklist](helpdesk-qualification.md), [runbook](helpdesk-runbook.md) and
[manual evidence template](qualification-manual-template.json).
