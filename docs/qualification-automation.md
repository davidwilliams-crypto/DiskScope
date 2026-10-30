# Candidate qualification automation

Run `scripts/qualify-helpdesk.ps1` on Windows PowerShell 5.1 or PowerShell 7 with
an exact executable, independently obtained SHA-256, and a new output directory:

```powershell
./scripts/qualify-helpdesk.ps1 -Executable 'D:\candidate\DiskScope.exe' `
    -ExpectedSha256 '<64 hex digits>' -OutputDirectory 'D:\evidence\new-run'
```

The existing output parent must be approved for evidence storage. The runner
creates only the new directory, an isolated candidate with diagnostic/no-history
policy, three fixture files totalling 4,107 bytes, and new reports. It retains
fixtures for inspection. It never installs/uninstalls an application, removes
existing files, signs code, or changes the trust store. Native application file
actions are not exercised. Optional `-PackageArtifact` paths add their SHA-256
records; hashing an installer does not test it. A sibling `BUILD_PROVENANCE.json`
is copied and its executable digest checked, retaining its source revision,
dirty-state file hashes and build time. A provenance record is a build claim,
not a signature or independent source audit.

`qualification.json` schema version 1 binds UTC timestamps, candidate hashes,
raw process outputs, explicit exit codes, known-answer report, no-clobber hashes,
fixture preservation and sampled direct-child process memory. The 10ms samples
can miss peaks. A fast or unavailable process with no positive sample remains
pending for the observation check. Small-fixture measurements do not establish
4GB support, UI/Stop responsiveness, whole-drive accounting, million-node
capacity, pressure protection or a reliability percentage. There are no retries.
Raw paths and unredacted reports can identify a test host; review before sharing.

Exit contracts:

| Exit | Meaning |
|---|---|
| 0 | Every required gate passed (reserved; this automated runner always leaves manual gates pending). |
| 1 | At least one failed automated check or failed setup/probe. |
| 2 | Qualification pending; inspect `automated_status` separately. |
| 20 | Output/preflight refused; existing evidence is preserved. |

Each check has `id`, `status` (`passed`, `failed`, `pending`), UTC observation,
expected result, actual result and reason. The fixed required gate list and
per-area decision live in `qualification-tools.ps1`. Absent rows are pending;
duplicates, unknown IDs and other statuses are rejected. A failed gate blocks
its area; an unavailable manual gate makes its area ineligible for 90/A. The
overall result requires every gate. No average, missing-row omission, passing
manifest, or successful synthetic test supplies a numerical score. `numerical_readiness`
and `measured_population_reliability` remain null. All-passed statuses establish
eligibility for reviewer assessment, not an automatically assigned grade.

The runner does not import manually edited pass flags. Copy
`qualification-manual-template.json`, fill the exact candidate/source hashes,
environment and case evidence, and have named owners/reviewers assess every
supported cell against [the qualification matrix](helpdesk-qualification.md).
Add cases for every applicable matrix scenario; one generic passing row cannot
represent an unexecuted matrix. Unsupported scenarios require an explicit
scope exclusion and review, not a passing or skipped result. With no approved
certificate or ScreenConnect/clean-client-VM lab, those gates remain pending.

Run `scripts/test-qualification.ps1` under both hosts. `-Executable` adds a real
candidate integration run, checks pending/nonpromotion and rejects output reuse.
Fake/unavailable children and synthetic gate matrices test only the harness's
failure handling; they are never candidate qualification evidence.

The `installer-qualification.yml` workflow runs on pushes only to the explicit
test branch `codex/helpdesk-assurance-0.3.3`, or on manual dispatch once registered
by GitHub. This permits its first run without a default-branch merge. It builds
the exact checked-out source using Rust 1.92.0 and packages from that payload. Both
PowerShell hosts must pass the helper suite with junction capability required;
an unavailable junction test fails this hosted gate. It downloads
the [official portable NSIS 3.12 ZIP](https://sourceforge.net/projects/nsis/files/NSIS%203/3.12/),
verifies SHA-256 `56581f90db321581c5381193d796fffcf2d24b2f8fed2160a6c6a3baa67f2c4f`,
requires its 2,362,938-byte ZIP and binary HTTP content type, then verifies
`makensis /VERSION`. The pin and size are independently recorded for the binary
`nsis-3.12.zip` (separately from the source tarball) in the
[MacPorts NSIS package manifest](https://github.com/macports/macports-ports/blob/master/devel/nsis/Portfile).
The official URL returned HTTP 200 HTML to `Invoke-WebRequest` in the first
hosted run. Independent local reproduction returned 143,732 bytes of HTML,
while `curl.exe` followed the official SourceForge mirror redirect to a ZIP
matching the original trusted hash and size; its extracted compiler reported
`v3.12`. The workflow uses curl with HTTPS-only redirects, bounded size/time,
and records whitelisted response headers, byte count, magic and hash before
trust or extraction. Cookies and redirect tokens are excluded from retained
diagnostics. A bad response still fails closed; the expected hash was not
changed and no failed installer/safety case is retried automatically.
No tool setup executable runs. Pinned action commits retain environment,
provenance, package hashes, raw CLI evidence and per-case lifecycle results as
14-day workflow artifacts. Preserve needed evidence before expiry.

Installer cases run only on a disposable GitHub-hosted Windows Server runner.
Production test preflight refuses existing installations, shortcuts, unsafe
paths and unregistered default directories. Cases cover custom/default paths,
shortcut choice, installed/portable payload parity, known-answer CLI scan,
reinstall, move/locked-file refusal, uninstall and preservation of unrelated
files/profile data. Those fresh lifecycle cases exclude upgrade/downgrade.

The separate hosted binary stage invokes:

```powershell
./scripts/test-installer-upgrade.ps1 `
    -Installer 'target/hosted-qualification/packages/DiskScope-0.3.3-windows-x64-setup.exe' `
    -PortableArchive 'target/hosted-qualification/packages/DiskScope-0.3.3-windows-x64-portable.zip' `
    -OutputDirectory 'target/hosted-qualification/binary-upgrade-rollback'
```

This invocation is for the guarded hosted workflow only. It downloads published
0.3.2 from this repository's official HTTPS release assets and requires installer
SHA-256 `807263cd323d6f154d4122723e7fe87e4999098246ca7b68fbfcf93cc4a03704`
and portable ZIP SHA-256
`f15b5553d781e9f90d9764bb55490733c43d09d8b69504a67367c6ca53ea6874`
before any baseline execution. It installs that baseline into a new fixture,
upgrades to the exact built candidate, rolls binaries back to 0.3.2, then
uninstalls. Each installed stage must match its complete required portable
payload, contain no stale/unexpected files beyond the declared canary and
installer files, and produce the expected CLI version and known-answer result.
Opaque install-directory and profile canaries must survive each transition and
uninstall. JSON records exact hashes, stage results, UTC times and failures.
This measures binary transitions and file preservation; the opaque profile
canary does not establish runtime history/snapshot-format compatibility.

The script independently requires the intended GitHub-hosted Windows repository,
event and run context, then refuses existing installation registrations in all
user/machine registry views, shortcuts, default/profile directories and running
DiskScope processes. Cleanup uses only a byte-verified uninstaller belonging to
the exact fixture registration, plus the unchanged owned profile canary and its
proven-empty, initially absent parent. It retains its bounded workspace fixture.
`test-installer-upgrade-preflight.ps1` passed 31 pure/synthetic checks on both
PowerShell 7 and Windows PowerShell 5.1: missing hosted context, self-hosted
execution, existing registration/path/process, and missing/unrelated/relative
uninstaller ownership are refused; positive assertion paths also run. The shared
HKCU uninstall record must appear identically in both logical registry views,
including every returned value and its type; mismatched paths, versions,
commands, values/types, extra fields and either machine registration fail.
[Microsoft documents HKCU\\SOFTWARE as shared across these views](https://learn.microsoft.com/en-us/windows/win32/winprog64/shared-registry-keys).
Cleanup ownership is captured after exact fixture-location and uninstaller-file
validation, before diagnostic outcome assertions, then rechecked before cleanup.
These
checks never execute an installer or write registry/profile data, and they
supply no evidence that actual upgrade/rollback passed. The workflow requires
them on both hosts before its native hosted stage.

The workflow cannot supersede a developer's installed 0.3.2 because native
installer qualification never executes locally. A green workflow supplies its
recorded hosted Server evidence only; the added binary stage remains unqualified
until it actually passes there. Runtime data-format compatibility, actual client
builds, ScreenConnect, UAC, Narrator, hardware, fleet EDR and approved publisher
signing still need their evidence and named review. Do not relabel a green CI
status as full installer/fleet qualification.
