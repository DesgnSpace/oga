fn main() {
    println!("cargo:rerun-if-env-changed=OGA_BUILD_STAMP");
    if let Ok(stamp) = std::env::var("OGA_BUILD_STAMP") {
        println!("cargo:rustc-env=OGA_BUILD_STAMP={stamp}");
    }
}
