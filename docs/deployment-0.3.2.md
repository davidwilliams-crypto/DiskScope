# DiskScope 0.3.2 preview deployment

This is an unsigned Windows x64 preview for controlled helpdesk qualification.
Use the [testing checklist](helpdesk-qualification.md), [technician runbook](helpdesk-runbook.md)
and [validation record](validation-helpdesk-hardening-2026-10-30.md). Do not treat
publication or automated fixture counts as approval for unrestricted cleanup.

## Choose the intended context

- Installer: `DiskScope-0.3.2-windows-x64-setup.exe`, per-user installation into
  `%LOCALAPPDATA%\Programs\DiskScope`. Run under the intended user. SYSTEM installs
  into its own service profile, not the signed-in user's profile.
- Portable: extract the complete `DiskScope-0.3.2-windows-x64-portable.zip` and
  retain its LICENSE, third-party notices, runbooks and `helpdesk-tools` directory.
- GUI pilot: use `DiskScope.exe --helpdesk [FOLDER]` from the actual interactive
  desktop. This enforces normal scanning, disables file removal and automatic
  profile persistence. A plain launch retains ordinary interactive capabilities.
- Background/SYSTEM tools: use the packaged `helpdesk-tools/invoke-helpdesk.ps1`
  with `-Mode Scan`; it verifies an independently supplied executable hash,
  stages a companion helpdesk policy in a new directory, waits for completion,
  and forwards the actual application exit code. Full command templates are in
  the runbook. An interactive GUI must not be launched in session zero.

## Installer commands

Close an existing application before replacement. Setup does not launch a scan.

```powershell
$process = Start-Process -FilePath 'C:\IT\DiskScope-0.3.2-windows-x64-setup.exe' `
  -ArgumentList '/S /NoShortcuts' -WindowStyle Hidden -Wait -PassThru
$process.ExitCode
```

The current PC's existing DiskScope Start-menu folder prevented safe fixture
installer lifecycle testing. Qualify install/reinstall, locked-executable refusal,
upgrade, uninstall, preserved profile data and rollback on a clean disposable VM
before approving installer deployment. Prior 0.3.1 lifecycle tests are historical.

## Evidence, privacy and verification

Verify release files using `SHA256SUMS.txt`; extracted files have their own
manifest and `BUILD_PROVENANCE.json`. Checksums establish byte identity, not a
trusted publisher. Application and installer remain unsigned. A trusted signing
identity and managed-device EDR/SmartScreen acceptance remain qualification gates.

GUI ticket export defaults to redaction. CLI JSON contains paths. Reports refuse
existing filenames and are bounded summaries, not complete inventories. Use new
incident-specific destinations with approved access and retention. No-history
does not prevent explicit report files or independent Windows/EDR/session logs.

Helpdesk inventory admission is one million nodes / 256 MiB estimated storage;
ordinary normal scans use five million nodes / 1 GiB estimated storage. A cap
retains explicit partial results: narrow the folder scope before drawing conclusions.
These are not hard process-RAM guarantees. History stays bounded to focused
investigations; protected baselines are not understood by older executables.

Exit codes: 0 complete/successful GUI exit, 1 application/report failure, 2 usage
or policy error, 3 timeout, 4 partial results. Wrapper codes 20/21 identify
preflight/staging/context or launch failures. Refer to the runbook for escalation.

The GUI requires Windows x64 and an OpenGL 2.0-capable interactive renderer;
the runtime is statically linked. Qualify actual fleet builds, ScreenConnect,
token/UAC contexts, devices, low-resource conditions, Narrator/DPI and technician
task success before promotion. No endpoint-protection exclusions are prescribed.
