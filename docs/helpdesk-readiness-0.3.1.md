# DiskScope 0.3.1 helpdesk readiness assessment

Assessment date: 30 October 2026. Released application source: `e2de0c98435c1aa9ee9a0895c262cc51f18efed4`, tag `v0.3.1`. Main `dbd2822` differs only in README/deployment documentation. This is an assessment and remediation proposal; it does not change the published application or release assets.

## Decision

**Do not approve unrestricted helpdesk cleanup on arbitrary customer endpoints yet.** The application is a credible candidate for a controlled diagnostic pilot, starting on IT-owned endpoints. Correct the CI failures and establish the pilot operating procedure before expanding to customer machines. Keep the normal scanner, use standard-user access where adequate, and keep NTFS preview off.

For the initial pilot, exclude DiskScope file actions and use the team's established cleanup/backup procedure. The current GUI has no centrally enforced diagnostic-only mode: this exclusion is an operating instruction, not a product control. CLI `--scan` offers a diagnostic execution path without GUI file-action commands and does not write GUI history; its requested JSON output still writes a report. Ordinary GUI scanning stores metadata history/preferences, even from the portable package.

**Overall judgment: approximately 75/C for unrestricted helpdesk deployment; approximately 85/B for narrowly scoped diagnostic use.** These are engineering readiness judgments, not measured reliability percentages or estimates of the chance of data loss. Scores are intentionally coarse. A = 90–100, B = 80–89, C = 70–79, D = 60–69, F below 60. A critical safety/release gate cannot be offset by a high score elsewhere.

## Fresh verification and important correction

- The public release is published as a preview, has five uploaded assets, and identifies the expected binary source commit. Its installer and portable ZIP digests match the packages verified locally. Application and installer Authenticode status is **NotSigned**.
- The released commit's [Windows validation run](https://github.com/davidwilliams-crypto/DiskScope/actions/runs/41111560632) is **failed**, as is the [main run](https://github.com/davidwilliams-crypto/DiskScope/actions/runs/41111447049). Earlier publication correctly reported CI as queued; it must not now be represented as green.
- The hosted **Rust 1.92 compiler-floor build passed**.
- The no-GUI job fails before tests: a PowerShell switch produces a scalar string for the one-element argument list. Native splatting expands it into characters. This was reproduced locally with an argument-printing command: `--no-default-features` becomes `'-', '-', 'n', 'o', ...`. Material source: `.github/workflows/windows.yml:23–28`.
- The default and diagnostics jobs each reach 124 passing binary tests and one failure. `actual_token_probe_fails_closed_and_privileged_sessions_do_not_persist` expects `Admin` while the hosted runner reports `AlwaysAdmin` (`src/app/management.rs:1755`). The application explicitly distinguishes always-elevated sessions; the test expectation depends on the host token environment.
- These findings establish workflow/test assumptions that need correction. They do **not** establish a newly reproduced wrong-target deletion or broken customer scan. They still leave independent release assurance incomplete; later steps do not run after Validate fails.

Previously executed evidence for this code is retained in [the implementation validation](validation-investigation-ux-2026-10-30.md): 398 default / 401 diagnostic / 273 no-GUI local tests, all-target Clippy, 11 native file-action fixtures, two optimized query stress tests, minimum-compiler build, payload/provenance checks and native fixture scan. The 0.3.1 binary suite passed 125 tests. Installer lifecycle tests passed 4,348 custom-path/default-shortcut checks and 4,338 default-path/no-shortcut checks. Many installer checks are per-file assertions; those counts are not thousands of independent scenarios or users.

## Grades and the route to A

| Area | Current grade | Evidence supporting the grade | Required to earn at least 90/A |
|---|---|---|---|
| Normal scanning and accounting | **85/B** | Native paths, explicit errors/partial coverage, hardlink-aware allocation, reparse avoidance and corrected subtree completeness. | Known-answer fixtures plus representative real-volume reconciliation for the declared support matrix; accurate explanations for every intentional discrepancy. Include sparse/compressed/hardlinked/native-name/cloud cases and live file mutation. |
| Destructive-action safety and qualification | **65/D** | Strong implementation and 11 passing native fixtures; material policy/media combinations remain unqualified. | Complete disposable policy/media/prompt matrix, or disable actions for unsupported/unverified contexts. Zero wrong-target changes, no unconfirmed permanent deletion presented as recycling, and preserved sibling/user data in every supported case. Add an enforceable diagnostic-only profile. |
| Elevation and permission boundaries | **85/B** | Real token checks, explicit UAC, fail-closed unknown mode and privileged persistence restrictions. | Standard, split-token admin, always-admin/UAC-off, alternate-credential and SYSTEM tests on disposable VMs; real consent/cancel behavior and correct target root. Test the promised distinction between access and speed. |
| Privacy and customer-data hygiene | **80/B** | Metadata-only history and explicit persistence policies; no file-content upload mechanism identified in this review. | No-history/session mode selectable before scanning, visible actual storage destination, documented handoff/retention procedure and privacy-aware evidence export. Test redirected profiles, uninstall retention and customer separation. |
| History and repeat investigations | **70/C** | Compatible comparisons, checksummed snapshots and corruption handling. | Qualify supported inventory size or narrow the comparison promise; protect named incident baselines from unrelated scans; durable export and explicit oversize/retention messaging. Demonstrate realistic full-drive follow-up investigations. |
| Performance and resource resilience | **70/C** | Bounded queues and useful synthetic query results; full inventory stays in memory. | Fresh release measurements on representative 4/8/16 GB endpoint classes, nearly-full disks and large inventories. Predeclare budgets for peak memory, first useful result, UI response and Stop acknowledgement. Add history free-space reserve/no-history option; test resource exhaustion gracefully. |
| Windows, filesystem and device compatibility | **60/D** | Development-PC validation and conservative fallback/refusal behavior. | A recorded supported matrix across actual fleet Windows builds, NTFS, cloud placeholders, redirects, USB, BitLocker and approved network cases. Test disconnects, stale drive letters, denied/locked files and unsupported providers. Unsupported cases must fail clearly. |
| Investigation UX | **80/B** | Compact layout, scan-root distinction, state-specific recovery, connected selection and honest outcome measures. | Actual helpdesk task trials: proposed ≥90% correct task completion and ≥90% correct interpretation of partial/logical/recycled/available space. Zero safety-critical misunderstandings. Validate long paths, mixed DPI and remote rendering with the release binary. |
| Accessibility | **65/D** | AccessKit and meaningful names, keyboard navigation, filter labels and focus regressions. | Complete keyboard/Narrator task qualification across scan, errors, navigation, filters, action review/cancel and history. Essential content at 900×640, 100/125/150% and fleet DPI; no required hover, focus traps or missing announcements. |
| Remote deployment and ScreenConnect | **75/C** | Portable distribution, per-user silent installer and JSON CLI. | Tested launch under intended user/admin/SYSTEM contexts, reconnect/lock/unlock/UAC, remote graphics and process lifecycle. Provide tested command templates, hash checks, exit-code handling, deployment detection and rollback for the actual helpdesk environment. |
| Artifact integrity and packaging | **90/A** | Exact-source provenance, Cargo artifact selection, release/inner payload manifests, installed/portable parity and lifecycle checks. | Maintain these controls automatically on each candidate; bind release assets to tested commit and independently verify downloaded hashes. This grade concerns integrity, not publisher authentication. |
| Publisher trust and supply-chain assurance | **70/C** | Locked dependencies, notices and provenance; current binaries unsigned and no fresh advisory audit recorded. | Sign and timestamp app/installer with an approved publisher identity; verify trust on managed pilot devices. Add dependency-advisory/license gates, protected signing credentials, approval and trusted artifact distribution. Signing alone does not prove safety or guarantee EDR acceptance. |
| Automated QA and release gating | **60/D** | Broad local tests and a passing hosted compiler floor, but both published-source/main matrices are red. | Fix argument-array handling and host-dependent token assertion without weakening production policy. All configurations and tooling pass on a clean runner; require green checks for release promotion, including actual payload tests. Exercise supported token variants deliberately. |
| Supportability, audit and incident handoff | **60/D** | Useful JSON errors/counts/accounting and session action review. | Ticket-ready export with schema/build identity, UTC, intended endpoint/context, scan root/coverage, errors, limitations and before/action/after evidence. Add actionable startup/crash diagnostics and support bundle redaction. Preserve appropriate incident evidence after the app closes. |
| Documentation and operating procedures | **80/B** | Deployment examples, exit codes and candid limitations. | A tested one-page technician runbook, support matrix, permission/privacy guidance, escalation and rollback procedure, and owner for each known issue. Distinguish identification from a decision that a file is safe to remove. Reconcile historical validation notes with current evidence. |
| Maintainability and product evolution | **80/B** | Modular scanner/view/history boundaries, regression coverage and explicit invariants. | Keep CI/release gates green, document key invariants/feature support, add deterministic host-independent tests and ownership, and separate experimental capabilities from the supported helpdesk profile. Use measured incident priorities rather than adding more chart types. |

## The most important helpdesk-specific gaps

### Large drives may not have comparison baselines

History allows at most **1,000,000 nodes / 128 MiB per snapshot** and five complete snapshots globally (`src/history.rs:46–52`). The historical C-drive inventory contained **4,547,609 nodes** ([memory audit](memory-2026-10-26.md)). A similar successful scan cannot become a saved baseline. Other roots can also evict an incident's baseline. This is intentional bounded behavior, not corruption, but it limits a full-drive before/after helpdesk promise.

The A-grade choice is either scalable, deliberately retained incident snapshots, or a clearly scoped folder-investigation workflow with durable evidence exports. Do not promise full-drive history based only on a successful scan.

### Disk-full diagnosis must not worsen disk pressure

History has an application byte budget, not a current free-space reserve (`src/history.rs:611`). Scans can trigger metadata writes on a customer disk that is already nearly full. The old 4.55-million-node benchmark recorded approximately **2.38 GB peak private commit**; that is historical 0.2.1 evidence, not a fresh 0.3.1 GUI or 4-GB-PC measurement. Bound queues do not imply bounded total inventory memory. Predefine resource limits per supported endpoint class and implement graceful no-history/low-space behavior before claiming broad low-resource readiness.

### An action being accurately targeted does not make it appropriate

Identity checking can prevent acting on a replaced file. It cannot decide whether an application database, Windows component, customer document or old archive is safe to remove. Technician review, customer approval and the established backup/cleanup procedure remain necessary. Do not classify a file as disposable merely because it is large or old.

The recycle preflight/veto narrows risk, but unmodelled Shell/media behavior can still produce permanent deletion. Native endpoint qualification remains a separate gate from implementation quality. The confirmation's opening assurance that Windows “will move” a file should become attempt/request language, especially for an Unverified verdict; later caveats should not contradict the first sentence.

### Incident evidence is currently transient or incomplete

CLI JSON includes version, root, coverage/completion, backend, counts, accounting, timing and bounded errors/largest files. Its largest list is limited to 100, and it lacks a schema version, scan UTC, endpoint/token/volume identity and build digest (`src/cli.rs:42,359`). The last GUI action review is session-local and has no complete incident export. Add an intentional redacted export rather than treating console text or a screenshot as a durable action record. Filenames and paths can identify customers and sensitive work even without file contents.

### Remote session context needs qualification

The installer is per-user. SYSTEM is not the signed-in user's profile. The GUI requires an interactive desktop and suitable graphics; CLI is the appropriate background command path. ConnectWise exposes separate tool permissions for user and elevated contexts; validate the actual tenant role and launch method rather than assuming a generic “run tool” action has the intended token/session. [ConnectWise permissions](https://docs.connectwise.com/ScreenConnect_Documentation/Get_started/Administration_page/Security_page/Define_user_roles_and_permissions/List_of_role-based_security_permissions).

Publisher signing and verification should use an approved identity and timestamp policy; Microsoft documents the signing and verification operations in [SignTool](https://learn.microsoft.com/en-us/windows/win32/seccrypto/signtool). A checksum downloaded with a binary proves agreement with that manifest, not an independently trusted publisher.

## Prioritized risk register

Likelihood describes exposure during unrestricted helpdesk use, not a measured incident rate. Proposed owners are roles to assign, not current assignments.

| Risk | Likelihood / impact | Level | Mitigation and proposed owner | Status |
|---|---|---|---|---|
| Unqualified recycle conditions remove data permanently | Uncertain / high | High | Exclude actions from initial supported profile; complete disposable matrix or fail closed. Engineering + QA lead. | Open |
| Technician equates large/old with safe to delete | Medium / high | High | Diagnostic-only pilot; runbook, customer approval and existing backup/cleanup process. Helpdesk lead. | Open |
| Red CI permits release without independent gates | High exposure / medium-high | High | Fix workflow/test assumptions; green checks mandatory for promotion. Engineering lead. | Open |
| Whole-drive scan exhausts memory or history worsens disk-full condition | Endpoint-dependent / high | High | Resource matrix, low-space reserve, no-history mode and monitoring. Engineering + endpoint QA. | Open |
| History baseline unavailable/evicted during an incident | High on large or mixed-root workflows / medium | High | Visible eligibility, pinned baselines or durable incident export. Product + engineering. | Open |
| SYSTEM/user context causes invisible GUI or unexpected data location | Medium / medium | Medium | ScreenConnect launch templates and actual context verification. Endpoint deployment lead. | Open |
| Customer metadata remains after technician handoff | Medium / medium-high | High | Explicit persistence/retention choices and approved report destination. Helpdesk + security owner. | Open |
| Unsigned app is blocked, quarantined or difficult to authenticate | Environment-dependent / medium | Medium | Approved signing/distribution and real managed-endpoint EDR tests; do not disable endpoint protection. Release + endpoint security lead. | Open |
| UI/graphics/accessibility failure in remote or scaled session | Uncertain / medium | Medium | Full remote/DPI/Narrator task matrix and headless diagnostic fallback. QA + accessibility owner. | Open |

## Recommended rollout and A-grade gates

1. **Release gate first:** correct the two CI defects, pass all matrix jobs on the exact next candidate and preserve successful logs. Do not silently change or retag existing 0.3.1 assets; publish a follow-up version if application behavior changes.
2. **Define an enforceable support profile:** Windows x64/local NTFS/normal backend as the initial scope. Add diagnostic-only/no-history operation. Leave experimental raw scanning and unqualified file actions outside that profile.
3. **Qualify on IT-owned endpoints:** use the actual portable package through ScreenConnect, test token/session combinations, graphics, EDR, low-space/low-memory, Stop, reconnect and upgrade/uninstall. Record OS, driver, RAM, token, artifact hash, result and limits.
4. **Close the native safety matrix:** disposable files only, approved VMs/devices/policies, expected refusal/cancel/outcome and sibling preservation. Zero wrong-target modification or misleading successful-recycle reporting in the declared supported scope.
5. **Make outcomes supportable:** implement incident export, timestamps/build identity, retention controls and an actionable failure path. Validate a larger-than-one-million-node scenario and baseline loss messaging.
6. **Run a measured helpdesk pilot:** proposed starting cohort of 3–5 technicians on 10–20 representative endpoints, initially diagnostic-only. Collect roughly 30–50 investigations before review. Record completion/correct-answer time, coverage, peak memory, freezes/crashes, context failures, baseline eligibility, user misunderstandings and privacy incidents. This cohort is feedback, not proof of a population-wide reliability percentage.
7. **Promote only against predefined gates:** all supported critical scenarios pass; no unresolved high-severity safety/identity/privacy failures; ≥90% correct completion of representative tasks and ≥90% correct interpretation of accounting/outcomes; zero safety-critical misconceptions. Publish sample sizes and failures. Complete the accessibility and remote-graphics matrices before assigning their A grades.

Keep the previous verified package available for operational rollback. Close the running process before replacement. Restoring an executable is not a supported history/preferences format rollback; test data-format compatibility separately, and remember uninstall deliberately preserves profile data. Do not clear a customer's metadata/profile as an automatic cleanup step.

The shortest route to A is stronger operational assurance, incident evidence and controlled supported scope—not more charts, larger test-count claims or a general-purpose AI chat layer. Competitive leadership still needs comparative user-task evidence.
