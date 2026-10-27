"""Package cached Cargo notices and checked-in supplements, without crate source."""
import hashlib
import json
import shutil
import sys
from pathlib import Path


def is_notice(path: Path, root: Path) -> bool:
    relative = path.relative_to(root)
    prefixes = ("license", "licence", "copying", "copyright", "notice", "ofl", "ufl")
    return (
        path.name.casefold().startswith(prefixes)
        or any(part.casefold() in {"licenses", "licences"} for part in relative.parts[:-1])
        or ("fonts" in relative.parts[:-1] and path.suffix.casefold() == ".txt")
    ) and path.suffix.casefold() not in {".rs", ".c", ".cpp", ".h", ".ttf", ".otf"}


def main() -> int:
    if len(sys.argv) not in {3, 4, 5}:
        print("Usage: package-notices.py CARGO_METADATA_JSON DESTINATION [RUST_RELEASE [RUST_COMMIT]]", file=sys.stderr)
        return 2
    metadata = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8-sig"))
    destination = Path(sys.argv[2])
    destination.mkdir(parents=True, exist_ok=True)
    supplemental_root = Path(__file__).resolve().parent.parent / "docs" / "dependency-notices"
    resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
    packages = [p for p in metadata["packages"] if p["id"] in resolved and p["source"] is not None]
    lines = [
        "# Third-party dependencies\n",
        "This inventory uses Cargo.lock and Cargo metadata filtered for Windows x64. "
        "It conservatively includes resolved build and development dependencies; "
        "it is not a claim that every listed package is embedded in the executable.\n",
        "License expressions below are copied from package metadata. Full upstream "
        "license texts, bundled-font notices, and available copyright/NOTICE files "
        "are in `licenses/`. Supplemental `SOURCES.json` files preserve exact "
        "upstream revisions, source URLs, and file hashes.\n",
    ]
    missing = []
    notice_count = 0
    for package in sorted(packages, key=lambda p: (p["name"], p["version"])):
        label = f"{package['name']}-{package['version']}"
        root = Path(package["manifest_path"]).parent
        target = destination / "licenses" / label
        files = {p.relative_to(root): p for p in root.rglob("*") if p.is_file() and is_notice(p, root)}
        explicit = package.get("license_file")
        if explicit and (root / explicit).is_file():
            source = (root / explicit).resolve()
            relative = source.relative_to(root.resolve()) if source.is_relative_to(root.resolve()) else Path(source.name)
            files[relative] = source
        supplement = supplemental_root / label
        provenance = supplement / "SOURCES.json"
        if provenance.is_file():
            sources = json.loads(provenance.read_text(encoding="utf-8"))
            if sources["package"] != label or sources["license"] != package.get("license"):
                raise ValueError(f"Supplement metadata does not match {label}")
            vcs_path = root / ".cargo_vcs_info.json"
            if vcs_path.is_file():
                vcs = json.loads(vcs_path.read_text(encoding="utf-8"))
                if sources["crate_vcs_commit"] != vcs["git"]["sha1"]:
                    raise ValueError(f"Supplement revision does not match {label}")
            for item in sources["files"]:
                relative = Path(item["file"])
                if relative.is_absolute() or ".." in relative.parts:
                    raise ValueError(f"Invalid supplement path for {label}: {relative}")
                source = supplement / relative
                if hashlib.sha256(source.read_bytes()).hexdigest() != item["sha256"]:
                    raise ValueError(f"Supplement checksum does not match {label}/{relative}")
                files[relative] = source
            files[Path("SOURCES.json")] = provenance
        notices = [relative for relative in files if relative.name != "SOURCES.json"]
        if not notices:
            missing.append(label)
        for relative, source in sorted(files.items(), key=lambda item: item[0].as_posix()):
            output = target / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, output)
        notice_count += len(notices)
        lines.append(
            f"- **{package['name']} {package['version']}**: "
            f"`{package.get('license') or 'see license file'}`; "
            f"[notices](licenses/{label}/); {package.get('repository') or package['source']}\n"
        )
    rust_release = sys.argv[3] if len(sys.argv) >= 4 else "1.99.1"
    rust_commit = sys.argv[4] if len(sys.argv) >= 5 else None
    rust_label = f"rust-{rust_release}"
    rust_supplement = supplemental_root / rust_label
    rust_sources_path = rust_supplement / "SOURCES.json"
    rust_sources = json.loads(rust_sources_path.read_text(encoding="utf-8"))
    if rust_sources["version"] != rust_release or (rust_commit and rust_sources["rustc_commit"] != rust_commit):
        raise ValueError("Rust standard-library notice version/commit does not match the compiler")
    rust_target = destination / "licenses" / rust_label
    rust_target.mkdir(parents=True, exist_ok=True)
    for item in rust_sources["files"]:
        relative = Path(item["file"])
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"Invalid Rust notice path: {relative}")
        source = rust_supplement / relative
        if hashlib.sha256(source.read_bytes()).hexdigest() != item["sha256"]:
            raise ValueError(f"Rust standard-library notice checksum does not match: {relative}")
        output = rust_target / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, output)
    shutil.copy2(rust_sources_path, rust_target / "SOURCES.json")
    lines.append(
        f"- **Rust Standard Library {rust_release}**: MIT OR Apache-2.0 with "
        "file/dependency exceptions documented in the complete "
        f"[standard-library copyright notice](licenses/{rust_label}/COPYRIGHT-library.html); "
        f"[source provenance](licenses/{rust_label}/SOURCES.json). "
        "The compiler executable is not redistributed.\n"
    )
    (destination / "THIRD_PARTY_NOTICES.md").write_text("\n".join(lines), encoding="utf-8")
    print(f"Collected {notice_count} notice files for {len(packages)} packages.")
    if missing:
        print("ERROR: Packages without full notice files: " + ", ".join(missing), file=sys.stderr)
        return 1
    print("Notice file coverage: all resolved third-party packages have notices.")
    print(f"Also packaged the complete Rust {rust_release} standard-library copyright/license notice.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
