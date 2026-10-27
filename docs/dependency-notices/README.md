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

To verify coverage without building or changing `dist`:

```powershell
python scripts/package-notices.py .superpowers/sdd/diskscope/cargo-metadata.json .superpowers/sdd/diskscope/notice-check
```
