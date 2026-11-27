//! Guards for facts Cargo.lock must keep true but Cargo cannot express.
//!
//! Cargo resolves each dependency on its own. When one crate pins a major
//! version of another crate and a sibling crate accepts a range that spans
//! several majors, the sibling can be resolved onto the older copy that some
//! third crate still needs. The two then see different types with the same
//! names, and the build breaks with trait-bound errors far from the cause.
use std::collections::BTreeMap;

/// `name version` → the dependency list exactly as Cargo.lock spells it.
type Packages = BTreeMap<String, Vec<String>>;

/// Parses Cargo.lock into packages and their dependency lines.
fn packages(lock: &str) -> Packages {
    let mut packages = Packages::new();
    let mut name: Option<String> = None;
    let mut key: Option<String> = None;
    let mut dependencies = Vec::new();
    for line in lock.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name = \"") {
            let (value, _) = rest.split_once('"').expect("closing quote");
            name = Some(value.to_owned());
        } else if let Some(rest) = line.strip_prefix("version = \"") {
            let (version, _) = rest.split_once('"').expect("closing quote");
            let name = name.take().expect("version follows name");
            key = Some(format!("{name} {version}"));
            // A package without a `dependencies` block is complete here.
            packages.insert(key.clone().unwrap(), Vec::new());
        } else if line.starts_with("dependencies = [") {
            dependencies.clear();
        } else if line == "]" {
            if let Some(key) = &key {
                packages.insert(key.clone(), std::mem::take(&mut dependencies));
            }
        } else if let Some(entry) = line.strip_prefix('"')
            && let Some(entry) = entry
                .strip_suffix("\",")
                .or_else(|| entry.strip_suffix('"'))
            && key.is_some()
        {
            dependencies.push(entry.to_owned());
        }
    }
    packages
}

/// The version of the `windows` crate a package is resolved onto.
fn windows_of(deps: &[String]) -> Option<&str> {
    deps.iter()
        .find_map(|dep| dep.strip_prefix("windows "))
        .map(str::trim)
}

/// The one crate in the wgpu tree whose `windows` requirement is a range
/// rather than a pin: wgpu-hal pins one major version, gpu-allocator accepts
/// several. Resolved onto an older copy that some other crate still needs,
/// the two fail to compile. gpu-descriptor is checked for the same reason.
fn wgpu_tree_agrees_on_windows(packages: &Packages) -> Result<(), String> {
    let Some(wgpu_hal) = packages
        .iter()
        .find_map(|(key, deps)| key.starts_with("wgpu-hal ").then_some(deps))
    else {
        return Ok(()); // No wgpu backend in this dependency graph.
    };
    let wgpu_hal_windows =
        windows_of(wgpu_hal).ok_or("wgpu-hal no longer depends on the windows crate")?;
    for (key, deps) in packages {
        let shares_the_windows_types_with_wgpu_hal =
            key.starts_with("gpu-allocator ") || key.starts_with("gpu-descriptor");
        if shares_the_windows_types_with_wgpu_hal
            && let Some(version) = windows_of(deps)
            && version != wgpu_hal_windows
        {
            return Err(format!(
                "{key} is resolved onto windows {version}, but wgpu-hal uses windows \
                 {wgpu_hal_windows}. Correct the `windows` entry of {key} in Cargo.lock \
                 to the version inside its declared range that wgpu-hal pins; a blanket \
                 `cargo update` undoes this."
            ));
        }
    }
    Ok(())
}

#[test]
fn gpu_allocator_is_resolved_onto_the_same_windows_crate_as_wgpu_hal() {
    if let Err(error) = wgpu_tree_agrees_on_windows(&packages(include_str!("../Cargo.lock"))) {
        panic!("{error}");
    }
}

#[test]
fn the_guard_flags_a_mismatched_windows_version() {
    let mismatched = "[[package]]
name = \"wgpu-hal\"
version = \"29.0.4\"
dependencies = [
 \"windows 0.62.2\",
 \"windows-core 0.62.2\",
]

[[package]]
name = \"gpu-allocator\"
version = \"0.28.0\"
dependencies = [
 \"windows 0.61.3\",
]
";
    let error = wgpu_tree_agrees_on_windows(&packages(mismatched)).unwrap_err();
    assert!(error.contains("gpu-allocator 0.28.0"), "{error}");
    assert!(
        error.contains("windows 0.61.3") && error.contains("0.62.2"),
        "{error}"
    );

    let agreed = mismatched.replace("\"windows 0.61.3\"", "\"windows 0.62.2\"");
    assert!(wgpu_tree_agrees_on_windows(&packages(&agreed)).is_ok());
}

#[test]
fn the_guard_passes_without_a_wgpu_backend_and_the_parser_reads_every_package() {
    let lock = "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\nsource = \"x\"\ndependencies = [\n \"b 2.0.0\",\n]\n\n[[package]]\nname = \"b\"\nversion = \"2.0.0\"\n";
    let packages = packages(lock);
    assert_eq!(
        packages.len(),
        2,
        "a package without dependencies is still read"
    );
    assert_eq!(packages["a 1.0.0"], vec!["b 2.0.0".to_owned()]);
    assert!(packages["b 2.0.0"].is_empty());
    assert!(wgpu_tree_agrees_on_windows(&packages).is_ok());
}
