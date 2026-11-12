fn main() {
    println!("cargo:rerun-if-changed=src/c_demo.c");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if !matches!(os.as_str(), "linux" | "macos" | "windows") {
        return;
    }
    cc::Build::new()
        .file("src/c_demo.c")
        .include(std::env::var("DEP_XILA_ABI_APPLICATION_INCLUDE").unwrap())
        .flag("-std=c11")
        .flag("-Wall")
        .flag("-Wextra")
        .flag("-Werror")
        .flag("-pedantic")
        .compile("xila_c_demo");
}
