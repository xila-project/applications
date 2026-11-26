fn main() {
    println!("cargo:rerun-if-changed=src/c_demo.c");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let desktop = matches!(os.as_str(), "linux" | "macos" | "windows");
    let bare_metal = os == "none"
        && matches!(
            arch.as_str(),
            "x86_64" | "aarch64" | "arm" | "riscv32" | "xtensa"
        );
    if !desktop && !bare_metal {
        return;
    }

    let mut build = cc::Build::new();
    build
        .file("src/c_demo.c")
        .include(std::env::var("DEP_XILA_ABI_APPLICATION_INCLUDE").unwrap())
        .flag("-std=c11")
        .flag("-Wall")
        .flag("-Wextra")
        .flag("-Werror")
        .flag("-pedantic");

    if bare_metal {
        build.flag("-ffreestanding");
        if arch == "xtensa" {
            // `CALL8` only reaches ±512 KiB; external helpers like `__udivdi3`
            // live in the firmware binary and may sit farther away in flash.
            build.flag("-mlongcalls");
        }
        let target = std::env::var("TARGET").unwrap_or_default();
        let target_abi = std::env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
        if let Some((architecture_flag, abi_flag)) = riscv_compiler_flags(&target, &target_abi) {
            build.flag(&architecture_flag).flag(&abi_flag);
        }
    }

    build.compile("xila_c_demo");
}

/// Derives `-march=` and `-mabi=` flags for bare-metal RISC-V targets so C
/// objects match Rust's floating-point calling convention (for example `ilp32f`
/// on `riscv32imafc-unknown-none-elf`).
fn riscv_compiler_flags(target: &str, target_abi: &str) -> Option<(String, String)> {
    let architecture = target.split('-').next()?;
    let (width, extensions) = if let Some(rest) = architecture.strip_prefix("riscv32") {
        (32u8, rest)
    } else {
        let rest = architecture.strip_prefix("riscv64")?;
        (64u8, rest)
    };

    if extensions.is_empty()
        || !matches!(extensions.as_bytes()[0], b'i' | b'e' | b'g')
        || !extensions
            .bytes()
            .all(|byte| matches!(byte, b'i' | b'e' | b'm' | b'a' | b'f' | b'd' | b'c' | b'g'))
    {
        return None;
    }

    let abi = match target_abi {
        "ilp32" | "ilp32e" | "ilp32f" | "ilp32d" | "lp64" | "lp64f" | "lp64d" => target_abi,
        _ if width == 32 => {
            if extensions.starts_with('e') {
                "ilp32e"
            } else if extensions.contains('g') || extensions.contains('d') {
                "ilp32d"
            } else if extensions.contains('f') {
                "ilp32f"
            } else {
                "ilp32"
            }
        }
        _ => {
            if extensions.contains('g') || extensions.contains('d') {
                "lp64d"
            } else if extensions.contains('f') {
                "lp64f"
            } else {
                "lp64"
            }
        }
    };

    Some((
        format!("-march=rv{width}{extensions}"),
        format!("-mabi={abi}"),
    ))
}
