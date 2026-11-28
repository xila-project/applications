use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(xila_linked_hexdump)");
    println!("cargo:rerun-if-changed=build.rs");
    if env::var_os("CARGO_FEATURE_NATIVE_LINKED").is_none() {
        return;
    }
    let target = env::var("TARGET").unwrap();
    let sdk_target = match target.as_str() {
        "xtensa-esp32s3-none-elf" => "xtensa-esp32s3-elf",
        "x86_64-unknown-linux-gnu"
        | "aarch64-unknown-linux-gnu"
        | "riscv32imac-unknown-none-elf"
        | "riscv32imafc-unknown-none-elf"
        | "thumbv7em-none-eabihf" => &target,
        _ => return,
    };
    println!("cargo:rerun-if-env-changed=XILA_SDK_DIRECTORY");
    println!("cargo:rerun-if-env-changed=PATH");
    let sdk = PathBuf::from(
        env::var_os("XILA_SDK_DIRECTORY")
            .expect("set XILA_SDK_DIRECTORY to the target's built SDK"),
    );
    let tool = PathBuf::from(
        env::var_os("DEP_XILA_ABI_APPLICATION_SDK_BUILD")
            .expect("use a Core revision that provides the SDK link-example command"),
    );
    println!("cargo:rerun-if-changed={}", tool.display());
    for relative in [
        "sdk.json",
        "include",
        "share/xila/example/hexdump/hexdump.c",
        "lib/crt0_linked.o",
        "lib/libxila_posix.a",
        "lib/libxila.a",
        "lib/libc.a",
    ] {
        println!("cargo:rerun-if-changed={}", sdk.join(relative).display());
    }

    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let status = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env_remove("LD_LIBRARY_PATH")
        .arg(tool)
        .args(["link-example", "--target", sdk_target, "--sdk"])
        .arg(sdk)
        .arg("--output")
        .arg(output.join("libxila_linked_hexdump.a"))
        .arg("hexdump")
        .status()
        .expect("start the SDK build tool with python3");
    assert!(
        status.success(),
        "the linked hexdump build failed: {status}"
    );

    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=xila_linked_hexdump");
    println!("cargo:rustc-cfg=xila_linked_hexdump");
}
