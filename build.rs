//! Embeds the application icon and version metadata into DiskScope.exe.
//! The icon source of truth is packaging/icon.ico (scripts/generate-icon.py).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=packaging/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("packaging/icon.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    resource.set("ProductName", "DiskScope");
    resource.set("FileDescription", "DiskScope disk usage analyzer");
    resource.set("FileVersion", &version);
    resource.set("ProductVersion", &version);
    resource.set("LegalCopyright", "MIT License");
    if let Err(error) = resource.compile() {
        panic!(
            "Windows resource compilation failed: {error}. Compiling DiskScope for Windows \
             requires the Windows SDK resource compiler (rc.exe) or windres to embed its icon."
        );
    }
}
