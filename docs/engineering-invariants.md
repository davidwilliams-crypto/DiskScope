# DiskScope engineering invariants and ownership

The repository owner owns release integration and assignment of engineering work.
Endpoint qualification, accessibility review and helpdesk pilot acceptance still
need named human owners; a source change does not assign those responsibilities.

## Contracts to preserve

- Native paths/identities are action inputs; display paths and exported metadata
  are never reconstructed into destructive targets. Identity/size/time and native
  ancestor/token guards are rechecked at the action boundary.
- Helpdesk restrictions merge monotonically and are applied before framework
  persistence initializes. No-history covers automatic preferences, window state
  and history; explicit exports and independent OS/session records remain possible.
- User cancellation and resource cancellation are distinct. Resource admission,
  worker obsolescence, refusal evidence, asynchronous metadata and view updates
  must match the current generation. An obsolete scan cannot stop its replacement.
- Memory observations are background samples, not hard allocation limits. Initial
  observation absence is Preparing; stale/unknown/low headroom is explicit Partial.
  Partial, failed and cancelled inventories cannot become complete baselines.
- Catalog ownership, byte/node/retention limits, free-space admission and protection
  metadata remain fail-closed. Older executables do not honor baseline protection.
  Whole-drive scalability requires architecture and measurements, not raised caps.
- Optional attribution never blocks a CLI deadline or probes from a UI frame.
  GUI requests/results are bounded and newest unsent work is retained for retry.
  Native observations are not proof of endpoint ownership; redaction hides them.
- Action journal requests, native calls and outcomes are distinct. Cancellation
  before execution has no invented call-start time. UTC is process-observed and
  may be unavailable/adjusted. Reconciliation can be interrupted; that is explicit.
- The journal is bounded session memory. Dismissal/navigation does not erase it;
  closing the app does. Full/redacted exports disclose row/error/text/retention
  truncation and refuse existing output files. Logical change is not reclaimed space.
- Command-line and GUI Windows subsystem behaviors differ. Deployment helpers wait
  explicitly, preserve argument boundaries/UTF-8, enforce timeouts where configured
  and forward actual process status. Successful test harnesses do not leak expected
  negative-case exit codes. A misspelled test filter returning zero tests is no pass.
- AccessKit names, focus and polite state feedback are tested independently of
  full Narrator/remote qualification. Do not count simulated controls as human task
  success or count an ignored native test as executed.

## Release and incident cadence

Before every release, retain exact source identity, locked dependency/advisory/
license results, feature-matrix logs, actual payload parity, hashes and applicable
installer/deployment qualification. Never overwrite or retag an existing release.
Run qualification on the exact candidate; a signature and checksum have different
trust meanings. Record pending gates and supported scope in release notes.

For a production incident, preserve the exact binary hash, token/session/context,
coverage, limits and report/privacy mode. Reproduce with disposable fixtures, fix
the responsible invariant, add a regression that fails before the fix, and rerun
affected plus integration gates. Reassess the qualified scope if devices, providers,
Windows builds, dependencies, installers or permissions change. Do not retry away
an unexplained safety failure or silently add an advisory exclusion.
