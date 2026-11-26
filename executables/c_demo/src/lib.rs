#![no_std]
#![cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    all(
        target_os = "none",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "riscv32",
            target_arch = "xtensa"
        )
    )
))]

// The `native-linked` runner drives the C entry on a stackful coroutine, which
// is supported on Linux (x86-64 and AArch64) and on bare-metal targets
// (x86-64, AArch64, ARM, RISC-V 32, and Xtensa). On macOS, Windows, and Wasm
// targets this crate stays empty.
#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(
        target_os = "none",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "riscv32",
            target_arch = "xtensa"
        )
    )
))]
mod linked {
    extern crate alloc;

    use alloc::{boxed::Box, string::String, vec::Vec};
    use core::num::NonZeroUsize;
    use executable::{ExecutableTrait, LinkedHome, LinkedOptions, Standard};

    // Debug host builds need extra headroom for the ABI call chain; embedded
    // firmware runs optimized builds with tighter RAM budgets.
    #[cfg(target_os = "none")]
    const STACK_SIZE: usize = 16 * 1024;
    #[cfg(not(target_os = "none"))]
    const STACK_SIZE: usize = 64 * 1024;

    #[allow(improper_ctypes)]
    unsafe extern "C" {
        fn c_demo_main(startup: *const abi_application::XilaStartup) -> i32;
    }

    pub struct CDemoExecutable;

    impl ExecutableTrait for CDemoExecutable {
        fn main(standard: Standard<'static>, arguments: Vec<String>) -> executable::MainFuture {
            Box::pin(main(standard, arguments))
        }
    }

    /// Run the linked C program as a native process on the calling task.
    ///
    /// The program gets the user's home with read and write rights and runs on
    /// its own stackful coroutine, so dropping the returned future cancels it
    /// safely even while it is suspended in a blocking call.
    pub async fn main(
        standard: Standard<'static>,
        arguments: Vec<String>,
    ) -> Result<(), NonZeroUsize> {
        // SAFETY: `c_demo_main` is linked into the firmware: it obeys the
        // `XilaStartup` rules, fits on the coroutine stack (`char line[128]` is
        // its largest local; the ABI call chain needs the rest in debug builds)
        // and returns without unwinding.
        unsafe {
            executable::run_linked(
                c_demo_main,
                LinkedOptions::new()
                    .stack_size(STACK_SIZE)
                    .home(LinkedHome::User),
                standard,
                arguments,
            )
            .await
        }
    }
}

#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(
        target_os = "none",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "riscv32",
            target_arch = "xtensa"
        )
    )
))]
pub use linked::*;
