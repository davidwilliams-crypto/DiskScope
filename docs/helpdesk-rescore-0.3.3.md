# DiskScope 0.3.3 post-fix assessment

Assessment date: 30 October 2026. **The all-90/A target is not yet met.**
The user confirmed that an approved signing certificate and a clean client
Windows/ScreenConnect lab are unavailable. The code fixes and automated tests
below are complete; required operational evidence remains pending. This is an
unpublished qualification candidate, not approval for unrestricted helpdesk use.

## What changed

- Incident reports now carry optional session-only ticket/endpoint labels and
  observed hostname/volume identity, with API source, times and explicit failures.
  Redaction removes identifiers and labels. Optional native attribution runs away
  from the UI and never extends the CLI deadline.
- A bounded session action journal records request, cancellation, native-call
  timing, outcome and reconciliation separately. Dismissing a card preserves
  verification; starting another scan records any interruption. Export before
  closing is required. Existing helpdesk restrictions still disable file actions.
- Background process-private/system-memory sampling gates scan admission. Unknown,
  stale or insufficient headroom produces explicit Partial coverage and prevents
  complete-history capture. Generation-bound refusal evidence survives later
  healthy samples. These sampled guards are not hard memory limits.
- AccessKit receives polite, throttled status announcements, visible form labels
  are linked, and the export dialog keeps its essential fields/Save control above
  the initial scroll area. Layout tests cover 900x640 at 100%/150% text size.
- Fixed a hosted-test-discovered rescan race: a previous view revision could
  settle a new generation's selection before its focus request arrived. Revision
  reservation and a deterministic regression now reject that stale answer.
- Candidate-bound qualification tools retain hashes, output, timings, resource
  samples and honest pending decisions. Hosted Windows installer tests cover
  fresh lifecycle cases and a separately guarded published-baseline binary
  upgrade/rollback stage. Missing manual results never become passes by averaging.
- Added engineering invariants, release/incident procedures, qualification
  instructions and a manual evidence template.

## Provisional implementation re-score and operational qualification

These are coarse, provisional implementation judgments, not confirmed operational
grades or measured reliability percentages. A provisional 90–100 corresponds to
A-quality work in the stated implemented/tested scope; 80–89 indicates a material
remaining gap; 70–79 indicates major qualification missing. Every row's operational
qualification is still pending. A required pending manual gate prevents confirming
that area at 90/A for real-world deployment. Scores are deliberately not averaged.

| Area | Prior assessment | Provisional implementation score | Evidence and what is still required for 90/A |
|---|---:|---:|---|
| Normal scanning and accounting | 90 | 90 / A | Known-answer CLI/native GUI and native filesystem fixtures pass. Additional providers/device cells require their own qualification. |
| Enforced diagnostic safety | 95 | 95 / A | Helpdesk actions remain disabled; policy and persistence regressions pass. Action-enabled use is assessed separately below. |
| Action-enabled cleanup qualification | 70 | 70 / C | Eleven native fixtures and one actual journal/action fixture pass locally. Full token, Bin-policy, cloud/network/removable and interruption matrix remains untested. Keep helpdesk actions disabled. |
| Permissions and elevation | 85 | 85 / B | Existing guards/regressions remain green. Need standard/split-token/elevated/alternate-credential/SYSTEM and UAC-consent/cancel results on supported clients. |
| Privacy | 90 | 90 / A | No-history/policy/redaction/no-clobber tests plus actual redacted GUI export pass. Redirected/locked/roaming profiles and fleet token/session cases still need operational review. |
| History and repeat investigations | 85 | 85 / B | Existing baseline protection/bounds remain; journal/reconciliation evidence improved. Need cross-version runtime data tests and representative scoped investigations. Whole-drive durable history is not established; supporting it beyond present caps would require further storage architecture work. |
| Performance and resources | 80 | 85 / B | Real native sampler/low-budget wiring and deterministic pressure/staleness/generation tests pass. Need measured 4/8/16GB, million-node, pressure, near-full-volume and cancellation-tail trials. Raising caps cannot replace these results. |
| Windows and device compatibility | 75 | 80 / B | Local native tests and hosted Windows Server lifecycle evidence add coverage. Supported client builds, BitLocker, providers, physical devices and live-changing volumes remain pending. |
| Investigation UX | 85 | 85 / B | Selection race, incident workflow, reconciliation and compact export layout fixed. Need observed technician tasks: >=90% correct completion and interpretation, zero safety-critical misconceptions. No trend-setting or competitor superiority claim is established by these tests. |
| Accessibility | 75 | 85 / B | Linked labels, focus restoration, status tree and compact-layout checks pass; native Windows accessibility exposes completed scan status. Actual Narrator, keyboard task matrix, mixed DPI and remote trials remain pending. |
| ScreenConnect deployment | 80 | 80 / B | Actual candidate helpers pass on PS7/5.1, including junction refusal, quoting, no-clobber and CLI behavior. Need the real tenant's roles, user/elevated/SYSTEM launch, reconnect, UAC and background handoff tests. |
| Artifact integrity | 95 | 95 / A | Clean source provenance, exact SHA-256 records and all 406 ZIP payload hashes verified. This establishes integrity, not approved publisher trust. |
| Installer lifecycle | 75 | 85 / B | Hosted custom/default path and shortcut cases, binary upgrade/rollback and uninstall pass. Need client standard-user/SYSTEM behavior and actual history-format compatibility/rollback evidence. |
| Publisher trust | 70 | 70 / C | Executable and installer are NotSigned. Need approved certificate, timestamp/chain/revocation checks, SmartScreen/Defender and actual fleet EDR acceptance. No self-signed trust or protection bypass was introduced. |
| Automated QA | 90 | 95 / A | Three feature configurations, all-target linting, native allowlisted fixtures, deployment/harness tests, exact-candidate qualification and hosted gates. Skips and missing manual cells are disclosed. |
| Supportability and evidence | 85 | 90 / A | Actual native attribution, bounded incident journal, timestamps, resource/refusal diagnostics, redaction and real GUI/CLI exports verified. Session journal must be exported before closing. |
| Documentation | 90 | 90 / A | Qualification gates, evidence schema, invariants, runbook, exact candidate identity and pending limits are documented. Real endpoint/security/accessibility/helpdesk reviewers still need assignment and sign-off. |
| Maintainability | 85 | 90 / A | Identity, resources, journal and accessibility responsibilities are separated; invariant documentation, deterministic race tests and independent review strengthen regression protection. |

**Zero areas are confirmed operational A.** Eight provisional component judgments
reach 90+, while ten remain below 90; twelve required manual gates remain pending.
Rerunning successful local tests cannot fill those evidence gaps. No unresolved
automated failure is accepted as a pass.

| Area(s) | Operational qualification | Required gate IDs still pending |
|---|---|---|
| Normal accounting; Windows/device compatibility | Pending | `devices-accounting` |
| Diagnostic safety; permissions/elevation | Pending | `token-uac`, `persistence-privacy` |
| Action-enabled cleanup | Pending | `native-actions` |
| Privacy | Pending | `persistence-privacy`, `token-uac` |
| History; supportability/evidence | Pending | `history-evidence`, `technician-pilot` |
| Performance/resources | Pending | `resource-classes` |
| Investigation UX | Pending | `technician-pilot` |
| Accessibility | Pending | `keyboard-narrator` |
| ScreenConnect | Pending | `screenconnect` |
| Artifact integrity; QA; documentation; maintainability | Pending | `release-validation` (exact release and reviewer approval) |
| Installer lifecycle | Pending | `installer-rollback` (client/runtime data coverage) |
| Publisher trust | Pending | `publisher-edr` |

## Exact local candidate and tests

Built clean from source `e00a76a5e079ddea0f26fd3885b3dbca6834abaf` using
Rust 1.92.0, locked dependencies, optimized Windows x64 build. Published main and
0.3.2 were not changed. Later qualification-script/documentation changes do not
change these retained bytes. Application source and manifests are identical
between this build commit and harness-correction commit `c5bbd5a`.

| Artifact | SHA-256 |
|---|---|
| DiskScope.exe | `c6f67d031ce0406b2fae76714875fb9f6b790ea1b83040575d7114c99fa5277f` |
| DiskScope-0.3.3-windows-x64-setup.exe | `9fcdb00c69f8e6f65c3d65a43c482246c880991d5701cd0b5f3051da1fd4a4b1` |
| DiskScope-0.3.3-windows-x64-portable.zip | `4d2c86c7f37d1e55d39acb61deae7e8c77820c9c8e4f62ffcf2f9d3d1e9a2109` |

Paths relative to this checkout: executable/payload `dist/candidates/0.3.3/`;
installer/ZIP/checksums `dist/candidates/releases/0.3.3/`. Both executable and
installer explicitly observed **NotSigned**.

- Local suites use Rust **1.99.1**; the candidate build and hosted feature/floor
  checks use the declared minimum **1.92.0**. Default: **464 passed**, 15 ignored;
  diagnostics: **467 passed**, 15 ignored;
  no-GUI: **320 passed**, 14 ignored. Configurations share tests; do not sum them
  as independent scenarios. Doc tests contain no cases.
- All-target Clippy with warnings denied passed in all three configurations;
  formatting and independent final diff review passed.
- Eleven explicitly allowlisted native NTFS action/recycle fixtures passed, plus
  one explicitly selected actual native action-journal timing/dismissal fixture.
  These were run separately from ignored defaults. The two query benchmarks below
  were separately selected. The remaining read-only raw NTFS metadata probe was
  explicitly invoked: it observed **Access denied (Windows error 5)** under the
  existing unelevated token. Its observational harness exited successfully, but
  this is a refusal result, not successful raw-volume eligibility/qualification.
  No elevation or disk write was attempted.
- Actual-candidate deployment tooling: PS7 **51 checks**, PS5.1 **50 checks**,
  both requiring junction checks. Qualification harness with actual-candidate
  integration: **26 checks** per host. Corrected upgrade preflight: **31 synthetic/pure
  checks** per host; those preflight checks execute no installer.
- Exact executable qualification: **six automated passed, twelve manual pending,
  exit 2** on each PowerShell host. Retained at
  `research/qualification-final-ps7/qualification.json` and
  `research/qualification-final-ps5/qualification.json`. The PS7 record binds the
  package hashes above. Numerical readiness remains null and A eligibility false;
  the tools do not ingest or invent manual sign-off.
- Actual native GUI: helpdesk mode, known-answer Unicode/space path, 3 files,
  2 folders, **4,107 logical bytes**, zero errors, native accessible completion
  status, visible export controls and a saved redacted JSON report. Verified root
  and native hostname redaction, complete coverage and available memory samples.
  Evidence: `research/assurance-final-native-redacted.json`. This is one local
  desktop smoke, not Narrator/DPI/fleet qualification.
- ZIP: every one of **406 entries** matched its exact staged file; project MIT,
  dependency and Rust standard-library notices are present.
- Separately selected optimized no-GUI query tests passed: a synthetic million-node
  model returned 128 rows and at most 2,000 map rectangles in **106.916ms** query
  time; the 250,000-node ranking microbenchmark matched full-sort results for
  different sizes/ties and first/middle pages. These exclude model construction,
  cloning, rendering and filesystem I/O. Logs are
  `research/assurance-final-million-query.log` and
  `research/assurance-final-ranking.log`. They establish query behavior on this
  host, not whole-drive scans or low-memory-class qualification.

Local native context was Windows 11 Pro x64 **10.0.26300/build26300**, interactive,
unelevated and not SYSTEM; visible memory **66,180,476KiB**. This is one existing
developer environment, not a clean client VM, 4/8/16GB test class or representative
fleet sample. Read-only context: `research/assurance-final-local-context.json`.

Native filesystem tests run with approved native execution because sandbox
restrictions deny some Windows rename/handle/junction operations. Production
guards were not weakened. NSIS startup also required native execution locally.
No existing installed application, trust store or security policy was changed.

## Hosted exact-source checks

- [Final Windows validation](https://github.com/davidwilliams-crypto/DiskScope/actions/runs/41142539577): **all five jobs passed** on `c5bbd5a` (three feature configurations, supply-chain gates, minimum compiler). [The application-source build commit's earlier run](https://github.com/davidwilliams-crypto/DiskScope/actions/runs/41141858930) also passed all five jobs.
- [Final hosted installer qualification](https://github.com/davidwilliams-crypto/DiskScope/actions/runs/41142539689): **passed** on clean `c5bbd5a`. Three actual lifecycle cases passed: custom path/default shortcuts, custom path/no shortcuts, and default directory. The separate sequence passed baseline 0.3.2 install, candidate 0.3.3 upgrade, binary rollback to 0.3.2, and uninstall. Every installed stage matched all **406 payload files** and its known-answer CLI/version result. Opaque install/profile canaries survived, and cleanup had no failure. The JSON contains **2,903 assertions**, mostly per-file checks; these are not 2,903 independent deployment scenarios.
- The hosted environment was Windows Server 2022, image `win22` version `20261026.347.1`, OS `10.0.20348.0`. Exact hosted executable SHA-256: `c370ecac7a0b95616c70812124e7f534dbe9aee62706614ec0c3fb1f1e94b012`; installer: `06897026b14c46a00b6d1650ab741d721ae02c33d69c73d5a91c5ea02eb53e8d`; portable ZIP: `db87cac54cd662a80668b6fec73626a661308ab5109f9d60f06fd65bda0b7d91`. Both hosted PowerShell runs also retained six automated passes/twelve manual pending/A eligibility false. Downloaded evidence is under `target/hosted-qualification-c5bbd5a/`; GitHub artifacts are retained for 14 days.
- [The earlier failed installer run](https://github.com/davidwilliams-crypto/DiskScope/actions/runs/41141858935) is retained: fresh lifecycle cases passed, but binary upgrade qualification failed because the harness treated an identical shared HKCU registry view as an unrelated registration. Failure and cleanup refusal were preserved; the harness was corrected with strict complete-record equality and owned cleanup, 31 preflight regressions passed per host, and the fresh final run above passed. [Microsoft documents the shared registry behavior](https://learn.microsoft.com/en-us/windows/win32/winprog64/shared-registry-keys).

Hosted artifacts have their own build-time provenance and SHA-256; source
equivalence does not imply byte-identical local/hosted packages. Installer evidence
is limited to the actual hosted Server OS/account and exact hosted package hashes.
Opaque file/profile canaries establish preservation, not history-format behavior.

During this work an earlier hosted scan/restoration failure was investigated and
fixed with the deterministic revision regression above. An earlier installer-tool
download returned HTTP 200 HTML instead of its pinned ZIP; hash validation refused
it before extraction/execution. The HTTPS transfer was corrected and retains the
same independently verified pin. Neither failure was retried away or waived.

## Real-world work still required

Use [the complete helpdesk checklist](helpdesk-qualification.md),
[runbook](helpdesk-runbook.md), [manual evidence template](qualification-manual-template.json)
and [automation instructions](qualification-automation.md). Required reviewers:
release engineering, endpoint QA/security, deployment lead, accessibility QA and
helpdesk lead. Names, devices, approved signer and execution windows are not yet
assigned; no assignment or sign-off is fabricated here.

1. Obtain an approved code-signing identity. Sign and timestamp a new exact
   candidate, then independently qualify managed-client trust and EDR behavior.
   The signed bytes must be rehashed and retested; these unsigned hashes do not
   transfer to a signed artifact.
2. Provision disposable supported client Windows environments and representative
   devices. Execute token/UAC, privacy/profile, native action/provider, resource,
   history-format and installer/rollback cells. Use IT-owned data and retain all
   failures. Keep action-enabled cleanup outside the initial diagnostic pilot.
3. Run real ScreenConnect and actual Narrator/keyboard/mixed-DPI task trials.
   Neither a helper script nor an accessibility-tree assertion substitutes for
   the real execution context and user experience.
4. Conduct 30–50 investigations with 3–5 technicians on 10–20 representative
   IT-owned endpoints, with predeclared correct answers, resource budgets and
   task timing. Promotion requires >=90% correct task completion and interpretation,
   zero safety-critical misconceptions, no unresolved critical safety/privacy/
   accessibility failure, and every applicable gate reviewed as passed.
5. If representative whole-drive/history workloads exceed the present inventory
   and snapshot caps, keep them explicitly unsupported. Implement and qualify
   durable/bounded storage before claiming that broader workload; do not increase
   constants just to improve a grade. Current helpdesk use is scoped normal-backend
   diagnostic scanning, with no automatic history or file actions.

Retest any newly observed failure after fixing its cause, then rescore the affected
area against the retained evidence. Until these steps pass, this assessment does
not certify every area at 90/A or authorize broad customer deployment.
