DiskScope 0.3.4 preview - Windows x64

Requires Windows 10 or later on an Intel/AMD x64 PC and an OpenGL 2.0-capable
graphics driver. The Visual C++ runtime is linked into the executable.
Extract the entire ZIP before running; no installation is required. Preferences
and scan history are still stored in your Windows user profile, not beside the EXE.
A separate setup installer is also available for this release.

For helpdesk use, launch DiskScope.exe --helpdesk [FOLDER]. This enforces normal
scanning, disables all file-removal actions, and never saves history, preferences
or window state. DiskScope.exe --no-history [FOLDER] disables profile persistence
while retaining ordinary interactive capabilities. Explicit evidence exports
still write the chosen destination. A companion diskscope-policy.json beside
the executable can enforce these restrictions; invalid policy aborts startup.
Use HELP_DESK_RUNBOOK.md and HELP_DESK_QUALIFICATION.md before a fleet rollout.
The release is MIT-licensed; see LICENSE and the separate third-party notices.

Normal scans stop with explicit partial results when inventory admission limits
are reached: 5 million nodes / 1 GiB estimated storage in ordinary mode,
1 million nodes / 256 MiB estimated storage in helpdesk mode. These estimates
are not measured process RAM limits. Investigate smaller folders when limited.
Use Export ticket evidence for a bounded JSON summary. Redaction is on by
default in the GUI; CLI JSON includes paths. Exports refuse existing files.

Incident & evidence supports session-only ticket/endpoint labels, observed
native hostname/volume metadata, and a bounded timestamped action journal.
Redacted reports omit these identifiers and journal details. Explicit export
is required before closing; outcome dismissal does not erase the journal.
Windows memory observations proactively stop scan admission on low/unknown
headroom, retaining partial results. Sampling can miss peaks and is not a hard
RAM cap. Native status feedback is exposed as a polite accessibility live
region; complete Narrator and real-world qualification are still required.

Run DiskScope.exe, choose a folder or drive, then select Scan.
Opening the application does not start a scan. Stop scanning keeps partial results.
File actions are available for selected local regular files after scanning stops.
Recycle Bin is the default in standard mode. DiskScope checks first and stops when it
can tell Windows would delete the file instead of recycling it (best effort); Windows
makes the final decision, so if it asks about permanent deletion, choose No.
Permanent deletion requires a separate confirmation, a checkbox and typing DELETE.
Folders and links cannot be deleted.
Hard links mean deleting one name may not reclaim its displayed size.

Run as administrator explicitly requests Windows UAC consent and opens a new
window for the current scan root. Cancelling UAC leaves the original window open.
Administrator mode can read more protected metadata and delete permitted files;
it does not bypass locks, ownership, ACLs or read-only attributes. It is not a
guaranteed speed increase. Recycle Bin actions require standard mode; elevated
permanent deletion checks file identity through a pinned handle.
Recycling holds ancestor directories against rename and reparse-point writers
while Windows performs the action. It still uses the Shell's final leaf-name
operation, rather than the atomic object guarantee of deletion by handle.

Overview explains leading folders and file types. Explore connects filters,
selection, a treemap or icicle view, and up to four pinned comparisons.
Changes compares successful local scan snapshots. Ctrl+K opens commands.
Dark/light/high-contrast themes, text scaling and reduced motion are available.

GUI history stores metadata only, normally under %LOCALAPPDATA%\DiskScope\history.
DISKSCOPE_HISTORY_DIR selects another location when explicitly set; network or
redirected locations can store metadata off this PC. Changes shows the location.
At most five complete scans are retained: up to one million nodes and 128 MiB
per snapshot, with a 768 MiB total storage budget including staging. Oversized
or incomplete scans are not saved as baselines. CLI scans do not save history.
History requires 256 MiB available space after incoming metadata. One incident
baseline can be protected from automatic retention within the five-file cap.
Explicit Clear removes protection. Older DiskScope versions do not honor the
new protection marker: do not share a live history folder across versions.
Administrator and unknown-access sessions do not save history or preferences.
The Changes workspace includes a history-clear control. File contents are not
stored or uploaded. Old modification times do not imply unwanted files.

Directory junctions and other directory reparse points are displayed but not
followed. Logical totals count each file entry, including hardlink aliases.
Unique allocation deduplicates file identity. Unknown allocation and read errors
are reported. Alternate data streams and filesystem metadata are outside totals.

The optional NTFS preview is experimental. It attempts read-only raw metadata
access for exact local drive roots, never requests elevation automatically, and falls back
to the normal scanner when unsupported or denied. Leave it off for routine use.

Repeatable command-line scan (PowerShell, from this package directory):
  $p = Start-Process .\DiskScope.exe -ArgumentList '--scan "C:\some folder" --json "D:\scan.json" --backend normal' -PassThru -Wait
  $p.ExitCode

Exit codes: 0 complete, 1 failure, 2 argument/build-mode error, 3 timeout,
4 partial results. --timeout SECONDS defaults to 600.

Package contents:
  DiskScope.exe             Portable application
  VALIDATION.md             Verification evidence, measurements, and limitations
  PERFORMANCE.md            Local C-drive comparison and measurement conditions
  packaging.md              Installer/portable usage and packaging verification
  THIRD_PARTY_NOTICES.md    Cargo dependency inventory and notice locations
  licenses\                 Full dependency, bundled-font, and Rust library notices
  SHA256SUMS.txt            SHA-256 hashes of the other package files

Keep the notices and licenses directory with the executable when redistributing.
For ScreenConnect transfer the portable ZIP, extract the entire folder and run
DiskScope.exe in the user's interactive desktop session. No installation or
separate Visual C++ runtime is required. A background service command alone
does not display the GUI on the user's desktop.
A distribution license for DiskScope's own source has not been selected.
