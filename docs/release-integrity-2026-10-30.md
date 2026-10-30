# Release integrity verification — 30 October 2026

This implements review findings F4 and F5. It does not qualify native recycling, signing identity, licensing, installer behavior on clean machines, or GUI usability.

## Compiler floor

`cargo metadata --locked --offline --format-version 1 --filter-platform x86_64-pc-windows-msvc` with the default features showed a maximum declared dependency `rust_version` of **1.92**, required by eframe, egui, egui-winit, egui_extras, egui_glow, epaint, epaint_default_fonts, emath and ecolor 0.34.3. The resolved graph includes build/development dependencies. Other inspected requirements include UUID 1.89 and image/ICU/home 1.88. Cargo.toml and README now require 1.92 for the supported Windows application.

`scripts/check-rust-floor.ps1` computes the maximum among resolved dependency nodes and rejects a lower package declaration. The regression includes an active dependency requiring 1.95 while another package requires 1.92 and an inactive package requires 1.99; it also treats `1.95` and `1.95.0` equally.

Only Rust **1.99.1** is installed locally. No minimum compiler was installed or run. The new Windows CI floor job builds the default locked release on **1.92.0**. Its first remote run remains required to establish a successful clean minimum-compiler build. Unfiltered all-platform offline metadata could not run because Linux-only crates are uncached; the floor statement is limited to the supported Windows default graph.

## Artifact selection and provenance

The focused regression was written before the helpers and failed because artifact selection did not exist. After implementation, it selects and copies the reported artifact with an older executable at the guessed default path. Missing, unexpected-package, ambiguous, failed, unfinished and missing-file results are rejected. A real dependency-free Cargo fixture sets `CARGO_TARGET_DIR` to a path containing spaces and explicitly builds for Windows x64. The copied executable prints `fresh artifact`; the stale default file remains unchanged. No application files or user data are executed or removed by this fixture.

The regression also reads source working-tree hashes, records the source revision and build target, checks the packaged executable digest, and checks that the checksum inventory includes provenance. New package builds reject source changes during compilation/packaging. Configuration and tool versions are captured with the build arguments. Source hashes supplement the revision for dirty developer builds. No signature or trusted publisher attestation is implied.

## Fresh checks

| Command | Result |
| --- | --- |
| `scripts/test-tooling.ps1` | Passed under PowerShell 7 and Windows PowerShell 5.1 |
| `scripts/test-build-artifacts.ps1 -Integration` | Passed under PowerShell 7 and Windows PowerShell 5.1, using real Cargo 1.99.1 |
| `scripts/check-rust-floor.ps1` | Passed under PowerShell 7 and Windows PowerShell 5.1; maximum 1.92 |
| `scripts/test-release.ps1` | 35 checks passed under PowerShell 7 and Windows PowerShell 5.1 with ordinary native Windows access; the sandboxed attempt stopped at the disposable junction fixture with Access denied |
| Workflow YAML parse | Passed with local PyYAML |

The workflow adds Windows default, diagnostics and no-GUI locked tests and all-target Clippy, plus formatting and tooling regressions. No remote workflow has run in this local session. Full application packaging, Rust application suites, formatting and Clippy are integration-stage checks while other implementation tasks continue editing the shared checkout. No installer was run and nothing was published.

## Integration follow-up

After source edits stop, run `scripts/build.ps1 -Offline -PackageDirectory dist/release-integrity-check`. To exercise alternate Cargo output on the full application, set `CARGO_TARGET_DIR` to an absolute path under the workspace (for example `target/release-integrity-cargo`) for that invocation and restore/remove it afterward. Explicit x64 targeting adds an `x86_64-pc-windows-msvc/release` segment. It can require recompilation even if the host release cache is warm.

Read `BUILD_PROVENANCE.json`; verify its executable digest matches both the packaged `DiskScope.exe` and the Cargo artifact in that alternate directory, `build.target` is `x86_64-pc-windows-msvc`, the lockfile/config/source hashes match the checkout, and its entry is present in `SHA256SUMS.txt`. Run `Assert-ReleasePayload` to verify all payload hashes. A stable source checkout is necessary because provenance rejects edits made during the build.
