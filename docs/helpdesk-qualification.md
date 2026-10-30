# Helpdesk candidate real-world qualification

Status: **pending fleet/VM/device qualification**, 30 October 2026. Automated unit
tests, fixture counts and documentation do not earn an A grade or establish a
supported platform. Use the [runbook](helpdesk-runbook.md) with a separately
versioned candidate; published 0.3.1 remains unchanged.

For every row record owner, ticket/run ID, UTC start/end, source commit, complete
package/executable SHA-256, OS edition/build, CPU/RAM, disk/filesystem/free space,
driver/provider, identity/token/session, local/remote method, exact command,
expected versus actual outcome, coverage/errors, resource measurements, evidence
location, pass/fail/blocked and reviewer. Do not mark an internal test skip as a
successful physical scenario. Sanitise endpoint names and native paths before
sharing evidence. Assign owners before execution; roles below are proposals.

| Gate and proposed owner | Real-world cases to execute | Required evidence/pass condition |
|---|---|---|
| Release / engineering | All default/diagnostics/no-GUI tests and all-target Clippy, floor build, formatting, tooling, actual package/provenance, fresh advisory/license/source and installed/portable parity checks on exact candidate. | Clean hosted checks and retained logs; no missing gates; owner-approved MIT project license and dependency notices shipped; dependency blockers resolved; downloaded hashes match tested artifacts. |
| Token/UAC / endpoint QA | Standard user; split-token admin unelevated/elevated; always-admin/UAC-off VM; alternate credential UAC; SYSTEM/session zero/background. Consent and cancel. Folder with spaces/Unicode, stale root and inaccessible root. | Correct access classification and intended native root. Helpdesk/no-history restrictions and companion policy survive relaunch or relaunch refuses safely. No invisible SYSTEM GUI; CLI exits usefully. No standard profile persistence in private/privileged contexts. |
| Persistence/privacy / security | Fresh profile; redirected/roaming/OneDrive profile; locked/denied profile; existing ordinary preferences/history; malformed/unknown companion fields; attempts to bypass restrictions by switches, keyboard, final confirmation, second process and elevation. Explicit export in full/redacted mode. | No private-session history/preferences/window-state files read/written before framework startup or at exit; existing data unchanged. Mode visible; policy fail closed. Evidence no-clobber and redaction verified; no cross-customer report reuse. Export/schema limits disclosed. |
| Native actions / safety QA | Disposable VM fixtures only: standard/elevated/SYSTEM, fixed NTFS, policy-enabled/disabled Bin, full Bin/quota, removable USB, network, cloud provider, read-only/locked file, hardlink, replaced file, reparse ancestor changed after confirmation, prompts/cancel, owner window lost, device disconnect. Keep sibling canaries and hashes. | Initial helpdesk mode refuses every file action. Any future action-enabled supported cell has zero wrong-target/sibling changes; exact cancellation/refusal and honest recycled/deletion-requested/permanent-removal outcomes. Unsupported cells refuse; do not infer this matrix from seven automated fixtures. |
| Devices/accounting / filesystem QA | Actual fleet Windows builds, local fixed NTFS; sparse/compressed/hardlinks; long/Unicode/unpaired-native names; denied/locked/live-changing trees; junctions; cloud placeholders hydration/offline; BitLocker locked/unlocked; USB removal and reused drive letter; mapped/UNC providers only if proposed support. | Known-answer fixture accounting and representative-volume reconciliation with Windows tool measurements, explaining logical/allocated/available-space differences. No unintended hydration, traversal or stale-target action; partial coverage explicit. Unsupported providers documented/refused. |
| Resource limits / performance QA | 4/8/16 GB classes; >1 million nodes and representative multi-million inventory; large directories; low-memory/commit pressure; constrained nearly-full disposable VHD; unknown free-space probe, exact reserve boundary and concurrent free-space loss. Cancel during preparation/scanning/finalization and export/history. | Predeclare per-class peak private-commit budget and maximum inventory before run. Suggested pilot ceilings: 4GB class 512MiB scoped inventory; 8GB 1GiB; 16GB 2GiB, subject to measurement and scope reduction. First useful result <=5s on supported scoped fixture, UI/Stop acknowledgement <=2s; log tail latency and storage. No crash, hang, lost existing baseline or pressure-worsening history writes. Failure/partial/cancellation honest. Do not fill a customer disk. |
| History/evidence / support QA | Scoped eligible before/after; >1-million-node and >128MiB snapshot; incomplete scan; protected baseline with unrelated-root retention; busy lock/corruption; denied/low/unknown space; cross-version import/rollback; restart and incident export. | Baseline preserved or rejection explained; no false whole-drive history promise. Large scans can use bounded durable evidence without claiming full inventory. Before/action/after metadata distinguish actual available-space change from logical size and unrelated concurrent activity. |
| Remote / deployment lead | Actual ScreenConnect tenant roles and user/elevated/SYSTEM launch methods; portable fresh stage; reconnect, lock/unlock, remote UAC consent/cancel, disconnect while scanning, resolution/mixed-DPI change, remote graphics fallback, process exit and rollback. | Recorded actual identity/session; mode and root preserved. GUI visible only in approved user desktop; no stuck invisible process. Headless report/exit works for background. No EDR bypass. Candidate detection and operator handoff work. References: [ScreenConnect role permissions](https://docs.connectwise.com/ScreenConnect_Documentation/Get_started/Administration_page/Security_page/Define_user_roles_and_permissions/List_of_role-based_security_permissions). |
| Keyboard/Narrator / accessibility QA | Actual Narrator with keyboard only: open root, scan/Stop, navigate/select/filter/sort, discover partial/errors, read accounting and mode, compare history, export/redact/cancel, any qualified action review/cancel. 900x640; 100/125/150% and actual fleet mixed DPI; remote session. | All essential content/tasks reachable without hover/mouse; correct names/roles/states and changing progress/result announcements. No focus traps; focus restored after dialog/cancel. Zero safety-critical inaccessible controls or misleading outcome announcements. Automated accessibility tree checks are supporting evidence only. |
| Publisher/EDR / release + endpoint security | Approved certificate signing and timestamp; app/installer trust and revocation verification on online/offline managed pilot machines; actual SmartScreen, Defender and fleet EDR policy; install/launch/scan/export/uninstall. | Exact independently approved signer and valid chain; no self-signed trust installation. Record signature timestamp, policy, detection/quarantine and response. No disabling protection or ad hoc exclusions. EDR acceptance is measured independently from signing. |
| Installer/rollback / deployment QA | Per-user install in intended profile versus SYSTEM; custom/default path, shortcuts, running app, existing unrelated files, upgrade/downgrade, uninstall with retained history, fresh portable fallback. | Existing customer/unrelated files and profile metadata preserved. No process replacement while running. Downgrade data-format compatibility explicitly qualified or unsupported. Previous approved package retained; no retag/overwrite of published packages. |

## Measured diagnostic pilot

Start with 3–5 technicians and 10–20 IT-owned representative endpoints, initially
30–50 investigations. Include reproducible known-answer tasks and realistic cases:
identify the actual scan root; find a growth source; explain logical versus
allocated versus available space; recognize denied/partial coverage; narrow scope
after resource/history rejection; export the correct incident evidence; resume a
scoped follow-up; and hand off without interpreting a large/old file as safe to
delete. Include keyboard/Narrator and ScreenConnect tasks for users who perform
them. Establish expected answers and budgets before each trial; record attempts,
correctness, completion time, assistance, mistakes, coverage, peak memory,
freezes/crashes, policy/context failures and privacy incidents. Keep denominators
and failures, not just successful demonstrations.

Promotion requires all supported critical scenarios above to pass; zero unresolved
high-severity identity, safety, privacy or accessibility failures; at least **90%
correct task completion** and **90% correct interpretation** of coverage/accounting/
outcomes, with **zero safety-critical misconceptions**. Publish numerator,
denominator, endpoint mix and every unresolved limitation. This pilot is useful
operational evidence, not a measured population reliability percentage. A 90/A
readiness score needs area-specific evidence; a high average cannot cancel a
critical failed gate. Do not expand into action-enabled cleanup, additional
filesystems/devices or customer endpoints until their cells are explicitly approved.

Stop immediately for wrong-target modification, unexpected permanent removal,
policy bypass, customer metadata exposure, repeated crash/hang or endpoint
security incident. Preserve evidence and roll back as the runbook specifies.
The release owner, helpdesk lead, endpoint security and accessibility QA reviewer
must record the supported scope and pending cells before any promotion decision.
