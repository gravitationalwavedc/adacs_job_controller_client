fn main() {
    // Use absolute path based on CARGO_MANIFEST_DIR
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let subhook_path = std::path::Path::new(&manifest_dir).join("third_party/mempatch/subhook");

    cc::Build::new()
        .file(subhook_path.join("subhook.c"))
        .file(subhook_path.join("subhook_unix.c"))
        .file(subhook_path.join("subhook_x86.c"))
        .include(&subhook_path)
        .define("SUBHOOK_STATIC", None)
        .compile("subhook");

    cc::Build::new()
        .file(std::path::Path::new(&manifest_dir).join("src/tests/trigger.c"))
        .compile("trigger");

    let bindings = bindgen::Builder::default()
        .header(subhook_path.join("subhook.h").to_str().unwrap().to_string())
        .allowlist_function("subhook_.*")
        .allowlist_var("subhook_.*")
        .generate()
        .expect("Unable to generate subhook bindings");

    let out_path = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("subhook_bindings.rs"))
        .expect("Couldn't write bindings!");
}
