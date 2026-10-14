use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("../engine/promptcraft");
    println!("cargo:rerun-if-changed={}", dir.join("promptcraft.cpp").display());
    println!("cargo:rerun-if-changed={}", dir.join("promptcraft.h").display());

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(&dir)
        .file(dir.join("promptcraft.cpp"))
        .warnings(true)
        .compile("promptcraft");
}
