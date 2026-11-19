fn main() {
    println!("cargo:rerun-if-changed=preview.manifest");
    let windows =
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("preview.manifest"));
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("Could not build desktop application resources");
}
