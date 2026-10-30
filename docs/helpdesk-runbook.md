# DiskScope diagnostic pilot runbook

Candidate workflow, 30 October 2026. This runbook describes the hardened checkout;
the published 0.3.2 packages do not gain the new 0.3.3 assurance changes retroactively.
Only promote a separately versioned candidate after the [qualification checklist](helpdesk-qualification.md)
and green gates for its exact source and package hash. No release or fleet rollout
is performed by these scripts.

## Technician procedure

1. Confirm the ticket, endpoint, signed-in user, intended scan folder, customer
   permission, report destination and retention owner. Start with a scoped folder
   on local fixed NTFS, normal backend, standard-user access. The initial pilot is
   diagnostic-only on IT-owned Windows x64 devices. Unsupported Windows builds,
   network filesystems, raw NTFS preview and customer file actions require separate
   qualification; a successful scan does not establish support.
2. Obtain the approved candidate executable and scripts from the managed toolbox.
   Independently obtain the executable SHA-256 from the release owner's approved
   channel; downloading a manifest beside a binary is not publisher authentication.
   Do not reuse the old 0.3.1 executable with these wrappers. Keep the complete
   approved portable distribution/notices with the toolbox; the wrapper stages
   only its executable and policy and does not redistribute a new release package.
3. Use the tenant's approved ScreenConnect tool permission and execution context.
   Verify the actual identity, token and session, not the tool's display label.
   For background/SYSTEM execution use the headless command below. For a GUI use
   the intended interactive user's desktop; never launch the GUI as SYSTEM or in
   session zero. Elevated access is for permission coverage, not a speed setting.
4. Verify the GUI says helpdesk mode before scanning. Record partial/denied/error
   coverage. Stop if the root, endpoint, mode or privacy setting is wrong. Large
   or old files are investigation leads, not approval to remove them. Helpdesk mode
   disables all GUI file actions and uses the normal scanner.
5. Save evidence to a new approved incident filename. CLI reports contain paths
   and filenames; GUI redaction should be chosen deliberately. Neither is a full
   inventory or a complete error log. Review schema limitations and coverage before
   attaching a report to a ticket. Transfer through the approved channel and apply
   the ticket's retention/access policy. No-history disables GUI history,
   preferences and window-memory storage; explicit reports, Windows/EDR logs,
   ScreenConnect logs and crash artifacts remain possible records.
6. Close the process, verify report handoff, and record outcome, scan scope,
   available space and unresolved coverage. Do not infer reclaimed bytes from
   logical file sizes or a recycle request. Use the established backup/cleanup
   procedure separately. Never remove a customer's profile/history automatically.

## ScreenConnect command templates

Use PowerShell 5.1 or 7; keep `helpdesk-tools.ps1` and `release-tools.ps1` beside
`invoke-helpdesk.ps1`. Paths below are examples. Pre-create the stage parent and
report parent with organization-approved ACLs; use a unique stage and report for
each ticket. The scripts reject existing stages/reports and reparse ancestors;
they do not defend an intentionally attacker-writable parent against concurrent
replacement. They do not download executables, elevate, install, change trust, or
overwrite customer files.

```powershell
# SYSTEM/background command: normal metadata scan, no GUI history.
& 'C:\IT\DiskScopeTools\invoke-helpdesk.ps1' `
  -Executable 'C:\IT\Approved\DiskScope.exe' `
  -ExpectedSha256 '<64 hex digits from independently approved record>' `
  -StageDirectory 'C:\IT\Sessions\INC1234-a' -Mode Scan `
  -Root 'C:\Users\Example\Downloads' `
  -ReportPath 'C:\IT\Evidence\INC1234-before.json' -TimeoutSeconds 300
exit $LASTEXITCODE
```

```powershell
# Launch from the actual interactive user session; returns when the app exits.
& 'C:\IT\DiskScopeTools\invoke-helpdesk.ps1' `
  -Executable 'C:\IT\Approved\DiskScope.exe' `
  -ExpectedSha256 '<64 approved hex digits>' `
  -StageDirectory 'C:\IT\Sessions\INC1234-gui' -Mode Helpdesk `
  -Root 'C:\Users\Example\Downloads'
```

The GUI command line is `DiskScope.exe --helpdesk [FOLDER]`, or
`DiskScope.exe --no-history [FOLDER]`. The folder is positional: do not append
`--scan` to a GUI switch. `--no-history` alone preserves ordinary action
availability and is outside the initial diagnostic-only pilot. Headless syntax
is `DiskScope.exe --scan FOLDER --json NEW_REPORT --backend normal --timeout 300`.

The wrapper creates `diskscope-policy.json` adjacent to its staged executable:
`{"helpdesk":true,"no_history":true}` for Helpdesk/Scan, or
`{"helpdesk":false,"no_history":true}` for NoHistory. The application merges
restrictions with switches; malformed/unknown policy fields fail closed. Retain
this companion during handoff and elevation. Protect the stage with approved
ACLs; this is a portable deployment policy, not a tamper-proof endpoint control.

| Exit | Meaning and technician response |
|---|---|
| 0 | Scan complete, or GUI exited successfully. Inspect coverage/report before conclusions. |
| 1 | Application failure or report I/O failure. Preserve diagnostic text, choose a valid new destination or escalate. |
| 2 | Application argument/policy failure. Check candidate version, policy and exact command. |
| 3 | Timeout. Treat any report as incomplete; narrow scope or increase the approved budget. |
| 4 | Partial/cancelled scan. Preserve errors/coverage; do not claim complete totals. |
| 20 | Wrapper preflight/context/hash/signature/staging/report-destination rejection. Nothing is launched. A failed stage may remain for review; never reuse it. |
| 21 | Wrapper could not launch the staged application. Preserve error and escalate. |

PowerShell parameter-binding failures happen before script execution and use the
PowerShell host's own failure code. The wrapper forwards application exit codes;
it does not treat a GUI's successful exit as a completed investigation.

## Release owner gates

Run the following on an isolated candidate checkout with the approved Rust 1.92.0
toolchain. Preserve logs, source commit/dirty fingerprint, compiler version,
Cargo.lock hash, package provenance and final artifact hashes. No gate publishes.

```powershell
cargo test --locked
cargo test --locked --features diagnostics
cargo test --locked --no-default-features
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features diagnostics -- -D warnings
cargo clippy --locked --all-targets --no-default-features -- -D warnings
cargo fmt --all -- --check
./scripts/check-rust-floor.ps1
./scripts/test-tooling.ps1
./scripts/test-release.ps1
./scripts/test-helpdesk-tooling.ps1 -RequireJunction
powershell.exe -NoProfile -File ./scripts/test-helpdesk-tooling.ps1 -RequireJunction
./scripts/test-build-artifacts.ps1 -Integration
./scripts/test-native-fixtures.ps1
```

Check each native command's exit code before proceeding when running manually.
CI fails on a native command failure. The native-fixture script explicitly selects
seven disposable permanent-delete/identity/junction tests, one at a time; it
checks that each exact test exists. It never runs all ignored tests. On an
authorized interactive standard-user local NTFS lab machine additionally run
`./scripts/test-native-fixtures.ps1 -IncludeRecycle`; capture the verdict/token
output. Hosted always-admin runs cannot prove standard-user Shell recycling.
Recycle fixtures can leave only their selected disposable fixture in that user's
Recycle Bin; do not empty the entire Bin as cleanup. No customer-file test is allowed.

Install the pinned checker once in the controlled builder with
`cargo install cargo-deny --version 0.20.2 --locked`, then run
`./scripts/check-supply-chain.ps1`. The script enforces its version, locked
resolution, Windows target, all features, advisory/yanked/license/source gates and
no advisory ignore list. Online runs fetch RustSec; offline evidence is limited
to the cached database (maximum seven days old). Keep the database revision and
time with the gate log. Dependency/font notices remain required in every package.
Sources: [cargo-deny 0.20.2](https://github.com/EmbarkStudios/cargo-deny/releases/tag/0.20.2),
[license configuration](https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html),
[advisory configuration](https://embarkstudios.github.io/cargo-deny/checks/advisories/cfg.html).

The project owner approved MIT licensing for this candidate; `LICENSE` and Cargo
package metadata must ship alongside the separate dependency/font notices.
New advisory/license failures require owner review and remediation before
promotion, not an automatic allowlist expansion.

Build/package into a new candidate directory using the established release
scripts; never point them at the published 0.3.1 directories. Run the actual
candidate through `./scripts/test-helpdesk-tooling.ps1 -Executable 'ABSOLUTE\DiskScope.exe'`
to exercise a known-answer disposable CLI scan and unchanged existing-report
rejection. Run installer lifecycle/portable parity checks from the packaging
procedure on disposable lab profiles. Verify the downloaded candidate against
the tested package and inner executable digests. A green hosted run, signed
publisher trust and physical/fleet qualification remain distinct gates.

Signing is optional tooling and a required promotion decision for an approved
signed channel. Only the release owner uses the approved protected signing
identity/timestamp service. After signing, regenerate hashes and repeat payload
checks. This read-only preflight verifies both app and installer:

```powershell
./scripts/verify-release-signatures.ps1 `
  -Path 'C:\Candidate\DiskScope.exe','C:\Candidate\DiskScope-setup.exe' `
  -ApprovedSignerThumbprint '<40 hex certificate thumbprint independently approved>' `
  -SignTool 'C:\Program Files (x86)\Windows Kits\10\bin\<SDK>\x64\signtool.exe'
```

It requires Windows `Valid` trust, exact approved signer thumbprint, a timestamp,
and `signtool verify /pa /all /tw /v` success. It never creates/imports a certificate
or changes machine trust. Unsigned/self-signed-untrusted candidates fail. Check
trust/revocation on representative managed pilot devices as well. References:
[Get-AuthenticodeSignature](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.security/get-authenticodesignature),
[SignTool](https://learn.microsoft.com/en-us/windows/win32/seccrypto/signtool).

## Stop, rollback and escalation

Stop the pilot for wrong-target behavior, unexpected permanent removal, policy
bypass, metadata exposure, repeated crashes, unresponsive Stop or EDR incident.
Preserve exact hash/command/context, redacted evidence, timestamps and endpoint
logs. Notify the designated incident/release owner using the established process.
Do not disable endpoint protection or clear customer data to make a test pass.

Close DiskScope before replacing a candidate. Restore the separately retained
previous approved executable/package in a new directory; this does not establish
history/preferences format downgrade compatibility. Do not overwrite an existing
stage or delete profile data as rollback. Uninstall intentionally preserves profile
data. Record any explicit technician-approved removal of owned deployment/evidence
files under the organization's retention policy; the wrapper performs no cleanup.
