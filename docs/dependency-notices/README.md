# Supplemental dependency notices

These complete license and notice texts supplement files missing from published
Cargo packages. `scripts/package-notices.py` combines them with notices already
present in the Cargo cache when preparing the portable package.

Each package directory contains `SOURCES.json` with its exact package/version,
unmodified Cargo license expression, `.cargo_vcs_info.json` commit, source URLs,
and SHA-256 hashes. The packaging script rejects mismatched metadata, revisions,
or hashes. Upstream license files were retrieved at those immutable revisions;
bundled-font texts and registry copyright comments came from the matching cached
crate contents. Extracted registry notices retain their original comment text
and identify the corresponding source files and submodule revisions.

The conservative inventory includes resolved Windows build/development
dependencies. It preserves all discovered license alternatives rather than
assigning new license terms. `LICENSES/` directories and all bundled-font text
notices are included. No dependency source code or font binaries are copied into
the portable notice directory.

`rust-1.99.1/` additionally preserves the complete installed
`COPYRIGHT-library.html`, which includes standard-library MIT/Apache texts and
the library's own dependency/exception notices. Its provenance records the full
Rust commit, owning installer component, pinned distribution archive URL/hash,
and notice hash. The compiler binary is excluded. The build script supplies its
actual compiler release/commit to the notice collector and rejects a mismatch.

The Geist and Geist Mono typefaces embedded in the executable are not Cargo
packages. Their files, SIL Open Font License text and provenance (release tag,
commit, archive and per-file SHA-256) live in `assets/fonts/`. The collector
verifies those hashes and packages `OFL.txt` and `SOURCES.json` under
`licenses/geist-font-<version>/`; the font binaries are not copied there.

Four supplements required by the Direct3D 12 (`wgpu`) backend are not plain
upstream files, and `SOURCES.json` says so for each:

- `egui-wgpu-0.34.3/` and `spirv-0.4.0+sdk-1.4.341.0/` hold the repositories'
  own licence files at each crate's recorded commit.
- `gpu-descriptor-0.3.2/` and `gpu-descriptor-types-0.2.0/` publish only a short
  dual-licence `COPYING` whose referred licence files are absent from their
  repository. The notice is kept exactly as upstream wrote it; the Apache-2.0
  text is the standard text from apache.org and the MIT notice is assembled from
  upstream's copyright line and the standard permission text.
- `hexf-parse-0.2.1/` is CC0-1.0 and declares that only through the `license`
  field of its manifest; it publishes no licence file. The supplement holds the
  standard CC0 1.0 legal code and the crate's own manifest.

CC0-1.0 was added to the technical allowlist in `scripts/helpdesk-deny.toml`
for that crate, with the owner's approval on 27 November 2026.

To verify coverage without building or changing `dist`, capture the same Cargo
metadata the release script uses and run the collector into the ignored
`target` directory. `target` does not exist in a checkout that was never built,
so create it first, and set the console to UTF-8 as `scripts/build.ps1` does:
otherwise PowerShell decodes Cargo's UTF-8 output with the console code page and
corrupts non-ASCII paths (for example a user profile with accented characters).

```powershell
New-Item -ItemType Directory -Force target | Out-Null
$previous = [Console]::OutputEncoding
try {
    [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
    cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc |
        Set-Content -Encoding utf8 target\cargo-metadata.json
} finally {
    [Console]::OutputEncoding = $previous
}
python scripts/package-notices.py target\cargo-metadata.json target\notice-check
```

Write the metadata as UTF-8, as above: Windows PowerShell 5.1 would redirect
with `>` as UTF-16, which the collector cannot read. Add `--offline` to the
`cargo metadata` command when the registry cache is already populated. Without
extra arguments the collector packages the checked-in Rust 1.99.1 supplement
without comparing it to any compiler; pass the compiler's `release:` and
`commit-hash:` values from `rustc -Vv` as two more arguments to verify it against
the compiler you have, as `scripts/build.ps1` does.
