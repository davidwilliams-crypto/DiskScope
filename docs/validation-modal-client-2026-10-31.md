# Modal Enter correction and isolated client qualification

31 October 2026. This candidate fixes the independent review's reproduced P2
modal-input defect. It is based on 1d94c584d7f453c3d18ecbeddb8378b5e9ef32bd;
new uncommitted search/UI work in the original checkout is intentionally excluded.
Exact compiler, source revision and binary identity are in BUILD_PROVENANCE.json.
It is an unsigned, unpublished qualification candidate, not all-A certification.

## Defect and regression evidence

When a scan-path field had focus, Ctrl+K and Enter in one input frame opened
Commands and disabled the background field. Disabling the field surrendered focus,
but raw lost-focus-plus-Enter handling still started a background scan. Both wide
and compact handlers now require their containing UI to be enabled before treating
raw Enter as path completion. This changes no enabled-field completion behavior.

Three actual application-frame regressions use real Tab traversal to establish
path-field focus. Before the fix, both modal cases failed with scan generation
0 -> 1; after it, generation and investigation root remain unchanged at 1440- and
1000-point widths. The positive enabled-Enter case still starts the requested root
in both layouts. The shared frame helper validates actual AccessKit Consumer trees.
This is frame-level reproduction, not a claim of native keystroke injection.

Baseline full default tests passed. After the fix, default: 485 passed / 15 ignored;
diagnostics: 488 passed / 15 ignored. Shared tests are not independent scenarios.
All-target default Clippy with warnings denied and formatting passed. Independent
review approved the narrow correction and regression coverage with no findings.
Other feature/source and exact-byte packaging/VM results must be recorded from their
own observed runs rather than inferred from these tests.

Logs retained in the original workspace research/local-pc-vm/:
modal-baseline.log, modal-enter-red.log, modal-enter-green.log,
modal-final-default-tests.log, modal-final-diagnostics-tests.log,
modal-final-clippy.log. Production source changes are the two enablement guards;
test code remains in its own cfg(test) module.

## Authorized client environment and remaining gates

The user authorized an independent clone of DD-Lab-Parity-Ref after the approved
Target1 disk proved absent. release.vhdx is only a data disk and is not a boot source.
An independent flattened disk was copied without changing the originals; all
source-chain/config hashes matched before/after. The new Gen2 VM has static 4GiB
RAM, 2 vCPU, no network adapter, no DVD, Secure Boot enabled and clone-local TPM.
Live PowerShell Direct reports Windows 11 Pro 26100.4652; C: is FullyDecrypted and
Unlocked. The original VM remains Off. This parity lab image is not a clean vendor
baseline. Offline disk mounting was refused by the existing token's privileges;
no host elevation/security policy changes were made to override that refusal.

PowerShell Direct is an administrative noninteractive token. There is no signed-in
guest desktop, so UI, UAC/standard interactive token, Narrator, mixed-DPI and real
ScreenConnect tasks are not established by boot or administrative CLI access.
Guest installer, accounting, low-memory-class and preservation results remain
pending exact-candidate execution and a separate retained outcome report.

Approved publisher signing/EDR acceptance, representative device/provider cases,
additional resource classes and independent technician outcomes remain qualification
gates. Use verified/failed/pending evidence; the earlier 90/95 judgments are not
calibrated probabilities or operational certification. The observed 30-50 task
pilot acceptance rule does not establish population reliability or rare-event safety.
