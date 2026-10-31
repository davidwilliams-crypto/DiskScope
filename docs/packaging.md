# Windows installer and portable release

DiskScope 0.3.1 has two distribution formats under `dist/releases/0.3.1`:

- `DiskScope-0.3.1-windows-x64-setup.exe`: installer for the current Windows account.
- `DiskScope-0.3.1-windows-x64-portable.zip`: extract the whole archive, then open `DiskScope.exe` inside `DiskScope-0.3.1-portable`.

Both contain the same application and dependency notices. `SHA256SUMS.txt` beside the downloads verifies the release files; a second manifest inside the portable folder covers its application files and notices. The installer adds `Uninstall.exe` and `INSTALLER_NOTICES.txt`. Keep notices with redistributed packages. A distribution license for DiskScope's own source has not been selected.

The unpacked portable build remains available at `dist/0.3.1/DiskScope.exe`. Earlier versions are retained.

## Requirements and behavior

Windows 10 or later on an Intel/AMD x64 PC and a graphics driver supporting OpenGL 2.0 or later are required. This installer rejects 32-bit Windows and Windows on ARM. The MSVC C runtime is linked statically through `.cargo/config.toml`; the executable imports only Windows system DLLs. A separate Visual C++ Redistributable is not required.

Setup defaults to `%LOCALAPPDATA%\Programs\DiskScope`, requires no administrator elevation, and registers DiskScope in Windows Installed apps. The Start menu shortcut is selected by default; the desktop shortcut is optional and initially off. Setup does not launch the application, schedule startup tasks, or start a scan. To update, close DiskScope and run the newer installer into the existing installation directory. To move an installed copy, uninstall it first.

Uninstall removes only files named in its compiled payload inventory, its shortcuts, and its registration. It does not recursively remove the program directory. Files you added to that directory are preserved, as are history under `%LOCALAPPDATA%\DiskScope\history` and preferences under `%APPDATA%\DiskScope\data\app.ron`. An explicit `DISKSCOPE_HISTORY_DIR` override is also untouched. The portable version requires no installation but still uses these profile locations; it is not a self-contained settings profile on a USB drive.

The packages are Authenticode-signed only when repository signing is configured (see [Release pipeline](#release-pipeline)); otherwise they are unsigned and Windows may display an unknown-publisher or SmartScreen prompt. SHA-256 hashes check file integrity. Publisher authentication is provided independently by the GitHub artifact attestations attached to every workflow-built package: verify a download with `gh attestation verify <file> --repo davidwilliams-crypto/DiskScope`.

## Release pipeline

Published releases are built by `.github/workflows/release.yml`, not on a local machine. Pushing a tag `v<version>` that matches `Cargo.toml` builds the exact checked-out tree on a disposable GitHub-hosted Windows runner using the pinned compiler (`scripts/install-pinned-toolchain.ps1` verifies the exact upstream release and commit hash) and the digest-pinned NSIS compiler, packages both formats, attests every published file with GitHub artifact attestations, verifies checksums again, and creates the GitHub release. A `workflow_dispatch` run performs the same attested build without publishing.

Optional Authenticode signing is inactive by default and changes no unsigned behavior. To enable it, set repository variable `AUTHENTICODE_SIGNING` to `true` and provide secrets `AUTHENTICODE_CERT_PFX_BASE64` and `AUTHENTICODE_CERT_PASSWORD` (a code-signing PFX); optional variable `AUTHENTICODE_TIMESTAMP_URL` defaults to DigiCert's timestamp server, and setting `AUTHENTICODE_SIGNER_THUMBPRINT` promotes `scripts/verify-release-signatures.ps1` to a release gate. Signing runs before packaging so both the installer and the portable ZIP embed the signed executable, then signs the installer container through `scripts/sign-release.ps1`, which regenerates both checksum manifests. `BUILD_PROVENANCE.json` keeps the pre-signing executable digest; the signed bytes remain covered by the refreshed manifests and the attestations.

Digest rotation ownership: the pinned cargo-deny package (`scripts/install-cargo-deny.ps1`), the pinned compiler identity (`scripts/install-pinned-toolchain.ps1`) and the NSIS archive digest are recorded in this repository and must never be updated from a download response. The published-binary baseline digests in `scripts/test-installer-upgrade.ps1` belong to the release manager: when a new published release becomes the upgrade baseline, recompute its digests from that release's own `SHA256SUMS.txt` and update the version and hashes together in one change.

## Building

Use the existing Rust/MSVC build requirements, Python 3.9+, and NSIS 3.12. The compiler is detected on PATH or under `C:\Program Files (x86)\NSIS`; `-NsisCompiler` accepts an explicit compiler path. No compiler is downloaded or installed by these scripts.

The validated release uses Rust 1.99.1. Standard-library notices are verified against the compiler's release and commit, so packaging with another Rust version requires its matching notice supplement under `docs/dependency-notices` first. Ordinary Cargo builds do not run this packaging notice check.

```powershell
# Build the application and both distribution formats using cached dependencies.
.\scripts\package-release.ps1 -Offline

# Package a previously built portable folder after checking every manifest entry.
.\scripts\package-release.ps1 -SkipBuild

# Regression checks; the installer test refuses to disturb an existing installation.
.\scripts\test-release.ps1
.\scripts\test-installer.ps1
.\scripts\test-installer.ps1 -DefaultDirectory -NoShortcuts
```

`build.ps1` first checks, before compiling anything, that the first line of `scripts/portable-readme.txt` begins with `DiskScope <version>` for the Cargo.toml version, so a version bump that forgot the README fails at once instead of after the release build. It also restores `PATH`, `CARGO_HOME`, `RUSTUP_HOME` and `CARGO_INCREMENTAL` when it finishes, so running it does not change the calling PowerShell session.

Packaging explicitly builds `diskscope` for `x86_64-pc-windows-msvc`. It copies the single matching executable reported by Cargo's JSON artifact stream and verifies its digest after copying. Cargo-configured target/output directories are supported; a missing, ambiguous, unsuccessful or unexpected artifact fails instead of falling back to `target/release/diskscope.exe`.

Each new build includes `BUILD_PROVENANCE.json`, covered by `SHA256SUMS.txt`: Git revision, dirty-tree flag, current source-file hashes, Cargo.lock digest, compiler/Cargo versions, explicit build arguments, Cargo configuration hashes, build environment overrides, artifact features/profile and executable digest. Source changes during compilation or packaging stop the build. The source inventory requires a Git checkout; ignored build outputs/local tools are excluded. This record supports inspection and reproducibility investigation; it is unsigned and does not establish publisher identity. `-SkipBuild` preserves an existing package, including legacy packages that predate provenance. See [30 October release integrity checks](release-integrity-2026-10-30.md) for fresh evidence and pending checks.

The release script derives its version from Cargo.toml, verifies all payload hashes and required notices, copies an isolated snapshot under `target/release-packaging`, and packages both formats from that snapshot. Unexpected files, mismatched versions, reparse points, path traversal, and overlapping input/output paths are rejected. The ZIP uses stable entry timestamps. NSIS warnings are treated as build failures. Completed artifacts are copied to the release directory and their hashes checked again. Build snapshots are retained for inspection under the ignored `target` directory.

Silent setup supports `/S` and `/NoShortcuts`. An optional `/D=absolute path` must be the final argument, without quotation marks around the path, as required by NSIS. Without `/D`, updates use the registered installation location. Do not move an installation by passing a different directory. Normal uninstall is `Uninstall.exe /S`; the Installed apps entry supplies the quoted command automatically.

## Verification

The static-CRT build passed 217 all-feature Rust tests, with 11 intentionally ignored tests. Inspection with MSVC `dumpbin /DEPENDENTS` confirmed removal of VCRUNTIME140.dll and the dynamic CRT imports. A packaged CLI scan completed successfully.

The release helper suite passed 28 checks in PowerShell 7.6.5 and Windows PowerShell 5.1. It covers tampered files, mismatched versions, hidden extras, unsafe/duplicate manifest paths, junctions and output aliases, NSIS escaping, nonrecursive uninstall instructions, reproducible ZIPs, and extracted payload checksums. Independent review identified the output-junction alias issue; the fix and regression test are included.

The production installer passed installation to a previously nonexistent folder containing spaces, shortcut/registration checks, a known-answer CLI scan, update using the remembered location, refusal to move an existing installation, refusal to overwrite or uninstall a locked executable, and complete uninstall. Every installed payload file matched the extracted portable ZIP. Unknown files inside and outside the installation survived, and read-only before/after snapshots confirmed that real preferences/history did not change. Tests exposed and fixed NSIS's existence-dependent path-normalization behavior; the installer now uses the Windows lexical normalization API with checked output size.

Fresh installation to the default per-user Programs directory also passed, with shortcuts disabled; the test then uninstalled and removed only its own sentinel and empty directories. Custom-directory lifecycle tests passed in Windows PowerShell 5.1 as well. The complete release build was run successfully under both supported PowerShell versions. Test-created installations, registration, and shortcuts were removed after verification.

Testing on this development PC does not establish compatibility across all graphics drivers or constitute a clean-VM qualification. Failed installs leave partial files for repair; upgrades preserve obsolete files absent from the new inventory rather than guessing which files to delete.

## Build references

- [NSIS command-line conventions](https://nsis.sourceforge.io/Docs/Chapter3.html)
- [NSIS scripting reference](https://nsis.sourceforge.io/Docs/Chapter4.html)
- [Rust static and dynamic C runtimes](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes)
- [Rust Windows MSVC platform requirements](https://doc.rust-lang.org/stable/rustc/platform-support/windows-msvc.html)
