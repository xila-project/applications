fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo:rustc-link-arg-bins=-zstack-size=8192");
        println!("cargo:rustc-link-arg-bins=--export=__heap_base");
        println!("cargo:rustc-link-arg-bins=--export=__data_end");
    }
}
