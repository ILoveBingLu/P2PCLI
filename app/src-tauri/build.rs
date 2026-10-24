fn main() {
    let target = std::env::var("TARGET").expect("Cargo target is missing");
    let sidecar = if target.contains("windows") {
        Some(format!("binaries/psmux-{target}.exe"))
    } else if target.contains("apple-darwin") {
        Some(format!("binaries/tmux-{target}"))
    } else {
        None
    };
    if let Some(sidecar) = sidecar {
        println!("cargo:rerun-if-changed={sidecar}");
        assert!(std::path::Path::new(&sidecar).is_file(),
            "Bundled session component is missing. Run pnpm run prepare:session-tools --target {target} before Cargo, or use pnpm run dev/build.");
    }
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");
    let windows = tauri_build::WindowsAttributes::new()
        .app_manifest(include_str!("windows-app-manifest.xml"));
    let attributes = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attributes).expect("Tauri build script failed");
}
