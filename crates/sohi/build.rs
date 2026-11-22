//! Embeds `assets/sohi.manifest` into `sohi.exe` via the MSVC linker.
//!
//! Done with linker flags instead of a resource-compiler crate, so the build
//! adds no dependencies.

fn main() {
    let manifest =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/sohi.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
        println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");
    }
}
