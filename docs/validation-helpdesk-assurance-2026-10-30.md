# Helpdesk assurance — unpublished 0.3.3 candidate

This pass follows the 0.3.2 readiness review. Published 0.3.2 is unchanged.
The user confirmed there is no approved signing certificate or clean Windows/
ScreenConnect lab. Those missing results cannot earn 90/A by retrying code tests.
The current candidate is for controlled qualification, not unrestricted cleanup.

## Implementation and scope

- Optional native attribution observes hostname, volume GUID/serial and filesystem,
  with API source, observation timestamps, bounded buffers and explicit failures.
  Identifiers are observations, not endpoint ownership proof or durable action
  targets. Redacted reports omit native identity, ticket and endpoint labels.
  GUI collection uses one bounded worker; CLI collection never blocks its scan
  deadline waiting for an optional provider. Provider-critical-error suppression
  is scoped to the worker thread and restored, never a machine security setting.
- A bounded session journal distinguishes requests, cancellation before execution,
  native-call timing, failure and observed outcome. Current action reconciliation
  survives card dismissal, and interruption is explicit before any replacement
  scan. Records survive root changes. Long text, retention eviction and export
  truncation are disclosed. Explicit export is required before closing; there
  is no hidden automatic journal persistence, including ordinary sessions.
- Windows process-private commit and system-available physical memory are sampled
  on a background worker at 250 ms. Admission waits for an initial observation;
  unknown/stale (>2 seconds) measurements or pressure produce explicit partial
  results and prevent complete-baseline capture. Stops and retained refusal
  reasons are generation-bound. Default budgets are 2 GiB private/256 MiB system
  available/64 MiB reserve, helpdesk 768/512/64 MiB. These are sampled tripwires,
  not hard process RAM limits; model queries/native I/O can still outlive a sample.
- Scan state and throttled counts use a polite, atomic AccessKit status region.
  Terminal states are immediate. Inputs for incident context and permanent-delete
  confirmation have visible linked labels. These are implementation/accessibility
  tree checks; actual Narrator, DPI/remote and technician task qualification remain
  pending until observed.
- Candidate-bound qualification automation retains real outputs, hashes, timings,
  direct-child memory observations and explicit passed/failed/pending checks.
  Missing manual evidence never becomes an A through averaging or repeated runs.
  The hosted installer workflow is restricted to disposable GitHub-hosted Windows
  Server runners and an explicit test branch/dispatch. It does not qualify client
  Windows, ScreenConnect, UAC, Narrator, fleet EDR or publisher trust.

## Evidence and grading rules

Final command results, hosted runs and candidate hashes are recorded in the rescore
and generated `qualification.json`; do not infer a pass from this design text.
Known filesystem-sandbox denials on native rename/junction tests are verified by
rerunning the same disposable tests under approved native execution, not weakening
production checks. All destructive tests use explicitly created disposable files.

The [qualification automation](qualification-automation.md), [manual template](qualification-manual-template.json)
and [real-world checklist](helpdesk-qualification.md) define required evidence.
Engineering implementation may be assessed separately from operational readiness.
An area with required pending manual gates cannot be confirmed at 90/A. Broad
history remains bounded to focused folder investigations; no whole-drive scalability
or population reliability claim follows from synthetic query timing.
