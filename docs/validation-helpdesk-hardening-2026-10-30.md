# Helpdesk hardening validation — 0.3.2 candidate

This record supports the 0.3.2 preview, implementing the remediation proposed in the 0.3.1 assessment. Publication does not satisfy the real-world qualification gates below or certify production readiness. Published 0.3.1 assets are unchanged. MIT licensing was explicitly selected by the owner on 30 October 2026; third-party obligations remain separate.

## Implemented controls

- `--helpdesk [FOLDER]`: diagnostic-only GUI, normal backend, no file actions, no automatic history/preferences/window-state persistence. Companion `diskscope-policy.json` can enforce helpdesk/no-history restrictions; flags merge monotonically. Unknown fields, duplicate fields, non-object JSON, malformed/oversized/unreadable files abort startup. This is an application operating control, not protection against a local administrator editing/replacing the executable or policy.
- `--no-history [FOLDER]`: no automatic profile persistence; explicit report exports still write their destination. Policy is applied before eframe initialization. In-app elevated relaunch is disabled for restricted sessions so it cannot silently lose flags. For elevated diagnostics, explicitly launch with the same restriction and retain the companion policy.
- Both GUI and managed CLI helpdesk scans use one million nodes / 256 MiB estimated inventory admission. Ordinary normal scans use five million nodes / 1 GiB estimated admission. A limit yields partial results, unknown unique allocation and an error; no complete baseline is saved. These estimates are not actual process working-set limits or proof of safety on 4-GB endpoints. Experimental raw NTFS remains outside the helpdesk profile.
- History requires a 256 MiB reserve plus incoming metadata, including before creating history storage. Unknown free space/overflow fails closed. One incident baseline can be protected within the existing five-file cap; damaged/locked/stale protection metadata prevents automatic reclamation. Explicit repair releases protection and reloads snapshots without clearing them. Explicit Clear removes saved history. Older executables do not understand the new marker; do not share history folders across versions.
- Intentional GUI ticket export defaults to redaction. Full export requires choosing full mode; CLI output contains paths. Schema 1 includes executable SHA-256, access/session, observed UTC timestamps, coverage/errors/accounting, bounded visible rows, requested-view context and current action review. Frozen display exports the displayed view with current scan coverage and published revision. Stale other-generation rows are suppressed. A report is neither a complete inventory nor a restorable comparison baseline. Endpoint/volume identities are explicitly uncollected. Native display paths may be lossy/truncated and must not be used to perform file actions.
- Export uses create-new and sync semantics; it never overwrites an existing file. A failed write can leave an incomplete newly created report; failure is surfaced and that file must not be treated as valid evidence. GUI work occurs on a background worker. Startup GUI failures are visible and exit nonzero; CLI errors never open blocking dialogs.
- Recycle confirmation now describes a request, never a guaranteed recycle. Existing native identity/ancestor/token rules remain. Diagnostic policy is checked again at final action commit, not only at buttons/shortcuts.
- CI PowerShell arguments are arrays in every configuration; token classification tests accept legitimate host variants without weakening persistence policy. Pinned compiler/action/advisory checker, fresh advisory/license/source gates, disposable native fixture allowlist, actual Cargo artifact selection and headless wrapper smoke improve release gating.
- Distribution includes MIT LICENSE, notices, helpdesk runbook, qualification checklist and hash/context-checked deployment helpers. Trusted publisher identity remains external.

## Fresh verification

All runs below use disposable fixtures, not customer file actions.

| Check | Result |
|---|---|
| Default full Rust suite | 432 passed, 14 ignored |
| Diagnostics full Rust suite | 435 passed, 14 ignored |
| No-GUI full Rust suite | 301 passed, 14 ignored |
| GUI tests within the default count | 131 passed, including restricted storage, keyboard/preparation/final commit refusal, frozen-view export, recovery UI, private-session onboarding/compact export, compact viewport, themes/text scaling and AccessKit checks |
| Explicit native file-action allowlist | All 11 passed separately, including actual recycle/owner-window/veto and disposable permanent-delete/junction safeguards |
| Formatting / all-target Clippy | All three feature configurations passed; warnings denied |
| History regressions within suites | 39 passed, including low/unknown space, overflow, bounded retention, locks/corruption, protected baseline and in-window recovery |
| Evidence/CLI/profiling targeted checks | 9 evidence, 31 CLI and 5 profiling tests passed before final integration |
| Deployment wrapper checks on actual release executable | PowerShell 7: 51 assertions; Windows PowerShell 5.1: 50 assertions; real junction/known-answer inventory, no-clobber, waited native process/UTF-8 streams, error forwarding and invalid policy covered |
| Release helper regressions | 36 checks passed |
| Existing tooling and actual alternate-target artifact/provenance integration | Passed |
| Advisory/license/source gate | Passed with pinned cargo-deny 0.20.2, Windows all-features/locked metadata, RustSec `2295466c4e9df26647fd67d8fa1c705ac2fc7f81` |
| Minimum compiler | Rust 1.92.0 Windows x64 optimized GUI build passed; exact installed standard-library notices/provenance added for packaging |
| Million-node optimized query | Passed; 122.94 ms for the measured query, 128 rows/2,000 rectangles. Excludes model construction, filesystem scan and GUI rendering |
| Native GUI smoke | Actual 0.3.2 window launched, accessible controls exposed, ordinary fixture scanned, GUI redacted JSON exported/inspected with root absent. This was a smoke of the same core export path before final onboarding text/compact-menu polish; full Narrator/remote/DPI work remains pending |
| Known-answer release scan | Three CLI helpdesk-policy runs matched 16,384 files / 256 folders / 67,171,587 logical bytes. Wall time 113.6–199.7 ms, first result 25.7–31.3 ms; sampled private memory 9.4–10.0 MiB and observed working-set peak ~17.1 MiB. Development PC/warm filesystem caches/ordinary fixture only; 5 ms sampling can miss memory peaks |
| Installer lifecycle | Candidate installer/ZIP built; lifecycle smoke intentionally refused its preflight because an existing DiskScope Start-menu folder would be affected. Existing installation left intact. Fresh clean-VM install/reinstall/locked-file/uninstall/retention and upgrade/rollback qualification is pending; do not reuse 0.3.1 lifecycle counts as a new pass |

Test counts are assertions/scenarios from automation, not user-success percentages. Standard suites intentionally ignore 14 tests; 11 native tests were explicitly exercised separately. Optimized query benchmarks, raw-volume eligibility observations and package/installer qualification are recorded separately if executed after this note. Hosted CI status for the published commit is reported on the GitHub release; the local results above do not establish hosted success.

Independent review found and prompted corrections to array-policy parsing, CLI policy propagation, frozen-view report coherence and nondestructive baseline recovery. Full-suite testing also caught a CLI blocking-dialog regression and outdated report/help contracts. Actual release-package testing caught GUI-subsystem process invocation returning asynchronously with stale PowerShell exit codes; deployment helpers now wait explicitly and preserve UTF-8 streams in both PowerShell versions. Native UI observation prompted private-session onboarding and compact-export improvements. These were corrected before final checks. Tests requiring native temporary-file rename/junction operations ran outside the sandbox after sandbox restrictions prevented those operations.

## Qualification still required before an A in every area

The complete executable checklist and acceptance criteria are in [helpdesk qualification](helpdesk-qualification.md); technicians use [the runbook](helpdesk-runbook.md). No local test substitutes for these results:

1. Real ScreenConnect user/admin/SYSTEM context, tenant roles, remote renderer, disconnect/reconnect, lock/unlock and UAC desktop behavior. CLI is the background path; GUI needs the intended interactive desktop.
2. Real standard/split-token/always-admin/alternate-credential/SYSTEM and UAC cancel/consent VM matrix. App-internal simulations do not qualify every Windows context.
3. Disposable recycle policy/media/capacity/prompt matrix: NukeOnDelete, oversized files and exact capacity boundaries, USB including fixed-reporting media/removal, quotas, ReFS/Dev Drive/exFAT, OneDrive/redirects and app/prompt closure. These actions remain excluded from the enforced helpdesk profile.
4. Representative Windows builds and devices, cloud placeholders, BitLocker, denied/locked files and disconnects. Supported scope must follow actual results, not a successful development-PC scan.
5. Measured 4/8/16-GB classes, large/near-full endpoints, peak memory, first useful result, response/Stop and graceful resource exhaustion. Admission estimates do not prove a hard process RAM cap, and free space can change after probing.
6. Full keyboard and Narrator task walkthroughs, high contrast, DPI scaling, mixed monitors and remote graphics. AccessKit/headless layout tests are useful partial evidence only.
7. Approved publisher certificate and timestamp, signature verification on managed devices, SmartScreen/EDR acceptance, and trusted deployment/distribution. No certificate or machine trust was fabricated or changed.
8. Pilot on IT-owned endpoints: proposed 3–5 technicians, 10–20 representative devices, 30–50 investigations. Measure at least 90% correct task completion and interpretation, with zero safety-critical misunderstandings and no unresolved high-severity failures. Record sample sizes and failures. This is feedback, not population reliability proof.
9. Clean hosted CI on the exact source candidate and package hashes, upgrade/uninstall/rollback including profile retention and format compatibility, and correct ticket privacy/retention/customer separation.

## Aspect disposition

The assessment's 16 areas are tracked by implemented controls and remaining evidence rather than automatically increased scores. Artifact integrity retains local A-level evidence subject to verifying the new candidate package. CI defects, supply-chain checking, session privacy, history protection and evidence gaps have concrete fixes. Scanning accuracy, action safety, permissions, privacy, history, resource resilience, compatibility, UX, accessibility, remote deployment, publisher trust, QA, supportability, documentation and maintainability each retain the relevant external gate above. Every area reaches 90/A only when its assessment acceptance criteria are demonstrated; until then the overall unrestricted rollout decision stays pending qualification.
