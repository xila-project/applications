fn main() {
    println!("cargo:rerun-if-changed=../wasm_guest_build.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        // Reserve one Wasm page for the stack to reduce initial linear memory on devices.
        println!("cargo:rustc-link-arg-bins=-zstack-size=65536");
    }
}
