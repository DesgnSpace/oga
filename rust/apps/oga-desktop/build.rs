fn main() {
    println!("cargo:rerun-if-changed=binaries");
    let target = std::env::var("TARGET").expect("TARGET is set by Cargo");
    let sidecar = format!("binaries/oga-server-{target}");
    if std::env::var_os("TAURI_CONFIG").is_none() && !std::path::Path::new(&sidecar).is_file() {
        // Workspace checks use the development broker from target/debug.
        unsafe {
            std::env::set_var("TAURI_CONFIG", r#"{"bundle":{"externalBin":null}}"#);
        }
    }
    tauri_build::build();
}
