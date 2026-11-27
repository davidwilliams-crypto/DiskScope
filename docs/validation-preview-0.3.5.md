# DiskScope 0.3.5 test preview

27 November 2026. 0.3.5 replaces 0.3.4, which was published from a commit whose
Windows validation and hosted installer qualification had both failed. It
includes the correction made directly afterwards: an unopenable session-policy
entry now aborts startup instead of silently running with the unrestricted
default policy. The installer and portable ZIP are built from one commit on
`main` with locked Rust 1.92.0 dependencies and static CRT; exact identity is in
BUILD_PROVENANCE.json and SHA256SUMS.txt. The Release workflow requires the
release commit to pass Windows validation and hosted installer qualification
before it creates the release, and creates it as a draft. These are unsigned
test packages, not all-A certification.

## Included changes since 0.3.4

- Ordinary scans scale their inventory budget with installed memory, so a whole
  drive can finish instead of stopping at a fixed 1 GiB estimate. The node
  arena grows in bounded steps, and a limit that stops a scan names itself.
  Helpdesk limits are unchanged. See the
  [drive-scan evidence](validation-drive-scan-budget-2026-11-27.md).
- The installer removes the payload files of the installation it updates,
  as listed in that installation's own checksum manifest, before writing its
  own. Files the user added are not listed and stay.
- Application icon and version metadata embedded in the executable, and the
  same icon on the installer.
- Geist and Geist Mono bundled under the SIL Open Font License with recorded
  provenance.
- Interface rework: welcome screen with drive cards and recent locations, live
  scan telemetry, toast notifications, a command palette (Ctrl K), a map
  breadcrumb bar, a restructured Changes page and a new theme.
- `url` 2.5.9 and `form_urlencoded` 1.2.3, closing RUSTSEC-2026-0592. DiskScope's
  own source never parses a URL or opens a web link; the advisory reached the
  application only through a GUI dependency.

## Verification scope

Local, on the release commit with Rust 1.92.0: default **544 passed / 15
ignored**, diagnostics **547 passed / 15 ignored**, no-GUI **344 passed / 14
ignored**. All-target Clippy with warnings denied and `cargo fmt --check`
passed in all three configurations. The local suites use the same compiler as
the release build.

The installer's manifest handling was exercised against a temporary directory
before CI: listed files (including a read-only one) removed with their emptied
folder; unlisted files kept; wildcards, paths through `..`, a rooted path and a
trailing dot run ignored; a file outside the installation folder left
untouched; a manifest that does not list the application ignored; a listed
file held open by another handle aborting the update with nothing removed. The
script compiles under NSIS 3.12 with warnings denied. The qualification
harness's pure checks pass (35) under Windows PowerShell 5.1.

Hosted results are the workflow runs of the release commit: Windows validation
and installer lifecycle qualification are required before the draft exists.
The hosted upgrade/rollback case now records the files that only the candidate
ships, because the published 0.3.2 installer cannot know them.

## Remaining real-world gates

Approved signing/managed-client EDR, supported-current Windows client builds,
interactive standard/elevated/UAC contexts, Narrator/mixed-DPI, actual
ScreenConnect, provider/device and pressure/resource-class matrices, and
independent technician outcomes remain pending. The memory-scaled budget was
measured on one 63 GB machine; its margins on 8–16 GB machines are arithmetic
only. The manual Recycle Bin policy and media matrix remains open. No
certificate, EDR bypass, host security change or customer-file action is part
of this test build. Use the included helpdesk runbook/checklist for
qualification.
