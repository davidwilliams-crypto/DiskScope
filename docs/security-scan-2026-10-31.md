# Security scan — 2026-10-31 (pre-public-release)

Scope: full-tree audit of DiskScope 0.3.3 at `codex/helpdesk-assurance-0.3.3` (HEAD `1d94c58` plus the uncommitted working tree), covering dependencies, secrets, unsafe/FFI, dangerous code patterns, privacy/data handling, and the release pipeline.

**Verdict: no code vulnerabilities found. The product codebase is clear for public release. The blocking item was a release-process gap, not a code defect: public artifacts were unsigned (R1).**

**Update (same day): all five findings were remediated.** R1: `.github/workflows/release.yml` now builds published releases on CI with GitHub artifact attestations and an optional Authenticode path (`scripts/sign-release.ps1`). R2: cargo-deny installs from a digest-pinned package (`scripts/install-cargo-deny.ps1`) and the compiler is verified against the recorded upstream release and commit hash (`scripts/install-pinned-toolchain.ps1`). R3: matrix values reach scripts through the environment, every workflow job has a `timeout-minutes`, and digest-rotation ownership is documented in `docs/packaging.md` and `scripts/test-installer-upgrade.ps1`. R4: the README states that CLI/`--json` reports contain full detail by default. R5: both NTFS sites (`ntfs_windows.rs` extent page, `ntfs.rs` attribute-name ranges) are now locally provable rather than invariant-based. Full suite 506 passed / 0 failed after the changes; clippy and rustfmt clean. Authenticode still requires the operator to obtain a certificate and configure `AUTHENTICODE_SIGNING` — until then releases ship unsigned but with verifiable build provenance attestations.

## What was run

| Check | Tool / method | Result |
|---|---|---|
| Dependency advisories (RustSec, 1395 advisories) | `cargo audit` over all 402 locked crates | **0 vulnerabilities.** 3 unmaintained notices (`zbus_names`, `zvariant`, `zvariant_derive`) are Linux-only portal crates; `cargo tree -e normal --target x86_64-pc-windows-msvc` confirms **0 occurrences** in the shipped Windows graph. Already allowed by `scripts/helpdesk-deny.toml`. |
| Licenses, bans, sources | `cargo deny check --config scripts/helpdesk-deny.toml` (all features, Windows target) | **advisories ok, bans ok, licenses ok, sources ok.** Only benign duplicate-version notices. |
| Supply-chain shape | `Cargo.lock` review | All crates from crates.io; no git/path deps; no empty checksums; no workspace `build.rs`. |
| Secrets | Pattern sweep (cloud keys, tokens, private keys) over working tree **and all 35 commits of history** | **Clean.** All hits were `sk-` matching inside `helpdesk-…` filenames (false positives). |
| Unsafe / FFI | Manual review of all 94 `unsafe` sites in 9 files | All are Win32 FFI with documented SAFETY, sized buffers, NUL-termination checks. No pointer is derived from on-disk or other untrusted data. |
| Raw NTFS parser (`src/ntfs.rs`, `src/ntfs_windows.rs`) | Full line-by-line robustness audit | **Panic-safe against arbitrary malformed MFT data**: every on-disk offset/length flows through checked accessors or explicit bounds checks; loops have enforced progress and budgets; accounting uses checked arithmetic; NUL/`\`/`/`/`:` in parsed names are rejected (no path-escape into the model). Malformed volumes fail with a descriptive error, never a panic or OOB read. |
| Directory enumeration (`src/platform.rs`) | Manual review of the `FileIdExtdDirectoryInfo` walk | Record and name bounds validated before every read; stride validation, `checked_add`, termination proof. |
| File deletion / recycle (`src/file_actions.rs`) | Manual review | Hardened: handle-based identity snapshot + re-verification at commit (TOCTOU defense); reparse points/junctions refused; local fixed/removable volumes only, with `QueryDosDeviceW` defeating SUBST aliases; strict path grammar (reserved names, trailing dots/spaces, forbidden chars); delete via `SetFileInformationByHandle` on the verified handle; recycle outcomes confirmed against the bin. |
| Elevation (`src/admin.rs`) | Manual review | Relaunches only its own executable with the `runas` verb; Microsoft CRT-correct argument quoting; NUL-in-path refusal. |
| Explorer reveal (`src/app/native.rs`) | Manual review | `explorer.exe` resolved from `GetSystemWindowsDirectoryW` (immune to PATH hijack); arguments passed as a vector, no shell string. |
| Search (`src/search.rs`) | Manual review | Bounded (1024 chars, 32 terms) substring matching only — **no regex, so no ReDoS class**. Checked size arithmetic. |
| CLI (`src/cli.rs`, `src/main.rs`) | Manual review | `OsString`-native argument parsing; report exports use create-new semantics (never overwrite an existing file). |
| Production panics | Enumerated every `unwrap/expect/panic!` outside `#[cfg(test)]` | 9 total, all structural invariants (thread spawn, guarded `Option`s, non-empty-by-construction containers) — none reachable from untrusted input. |
| Network / telemetry | Repo-wide capability search + dependency graph | **None.** No socket/HTTP code; the `windows-sys` feature set contains no `Win32_Networking*` modules — the binary cannot open a network connection by construction. No update checks, no crash reporting, no analytics. |
| Privacy / data handling | Review of every production write site | Writes only to `%LOCALAPPDATA%\DiskScope` (bounded history), eframe prefs in `%APPDATA%`, and user-chosen export/action targets (create-new). Evidence exports are **redacted by default**; full detail requires explicit opt-in with an in-UI warning. `helpdesk`/`no_history` policy modes fail closed. History ownership files are symlink/hardlink-swap hardened. |

## Findings

### R1 — HIGH (release process, not code): public artifacts are unsigned with no verifiable provenance

There is no release workflow; artifacts are built locally and published unsigned (docs/packaging.md says so explicitly, as does the README's "unsigned preview" note). `SHA256SUMS.txt` provides integrity only and is hosted beside the downloads it describes. For a public Windows GUI release, users cannot authenticate that a download came from you.

**Remediation before release:** add a tag-triggered release workflow that builds in CI and publishes with Authenticode signing and/or GitHub artifact attestations, plus a detached signature over `SHA256SUMS.txt`. The existing `verify-release-signatures.ps1` gate already expects a signer thumbprint.

### R2 — MEDIUM (CI): toolchain components pinned by version, not digest

CI runs `cargo install cargo-deny --version 0.20.2` and `rustup toolchain install 1.92.0` at run time — a crates.io or static.rust-lang.org compromise could alter validation. The NSIS download in `installer-qualification.yml` is the model to copy (exact byte count + hardcoded SHA-256 + post-extract version check).

**Remediation:** record expected SHA-256 digests for the cargo-deny install and toolchain components the way NSIS is pinned, or vendor the tools.

### R3 — LOW: CI hardening nits

- `'${{ matrix.features }}'` interpolated inside a `run:` block (`windows.yml:26`) — values are workflow-static today; pass via `env:` instead so a future edit can't turn it into an injection sink.
- No `timeout-minutes` on `windows.yml` jobs (the qualification workflow sets 60 correctly).
- Upgrade-test baseline digests (`test-installer-upgrade.ps1:22-23`) pin downloaded binaries; ownership of rotating them on each baseline bump should be documented.

### R4 — LOW (documentation): CLI reports contain full paths by default

`--json`/stdout reports are full-detail (paths, hostname, volume identifiers) unless redaction is requested. This is standard CLI behavior, but the public README/docs should say so explicitly, since the GUI defaults to redacted.

### R5 — INFO (defensive hardening, optional): two invariant-based safety spots in the NTFS reader

Safe today, but their safety rests on cross-function invariants rather than local checks: `page.last().unwrap()` (`src/ntfs_windows.rs:348`; callee rejects empty vectors) and `record[self.name.clone()]` range indexing (`src/ntfs.rs:677`, `:681`; range validated at capture). A `let-else`→`Err` and a `get()`-based read would make them locally provable if that code is ever touched again.

## Explicitly verified clean

CI workflow permissions (`contents: read` only), action SHA pinning, absence of `pull_request_target` and `secrets.*`, self-hosted-runner refusal guard, NSIS installer privilege model (`RequestExecutionLevel user`, HKCU-only, no embedded-binary execution, path normalization against traversal/device namespaces), test-suite containment (tempdir sandboxes; state-mutating fixtures opt-in and refuse elevation), `.gitattributes`, and the examples directory (synthetic data only, no PII).

## Toolchain note for reproducing

`cargo audit` and `cargo deny` were run with the repo-vendored toolchain (`.tools/cargo`, rustc 1.99.1 host / 1.92.0 target) against `Cargo.lock` at commit-time state including the uncommitted 0.3.3 changes.
