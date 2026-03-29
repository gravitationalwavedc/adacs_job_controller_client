fn main() {
    let subhook_path = "../src/third_party/mempatch/subhook";
    
    cc::Build::new()
        .file(format!("{}/subhook.c", subhook_path))
        .file(format!("{}/subhook_unix.c", subhook_path))
        .file(format!("{}/subhook_x86.c", subhook_path))
        .include(subhook_path)
        .define("SUBHOOK_STATIC", None)
        .compile("subhook");

    cc::Build::new()
        .file("src/tests/trigger.c")
        .compile("trigger");

    let bindings = bindgen::Builder::default()
        .header(format!("{}/subhook.h", subhook_path))
        .allowlist_function("subhook_.*")
        .allowlist_var("subhook_.*")
        .generate()
        .expect("Unable to generate subhook bindings");

    let out_path = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("subhook_bindings.rs"))
        .expect("Couldn't write bindings!");
}
