fn get_git_hash() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn main() {
    println!("cargo:rustc-env=GIT_HASH={}", get_git_hash());
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

    if std::env::var("CARGO_CFG_TEST").is_ok() {
        cc::Build::new()
            .file(std::path::Path::new(&manifest_dir).join("src/tests/trigger.c"))
            .compile("trigger");
    }

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
