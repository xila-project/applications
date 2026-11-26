#![no_std]
#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]

// The `native-linked` runner drives the C entry on a stackful coroutine, which
// desktop hosts provide on Linux (x86-64 and AArch64) only. On the other
// desktop hosts this crate stays empty, as on every non-host target.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod linked {
    extern crate alloc;

    use alloc::{boxed::Box, string::String, vec::Vec};
    use core::num::NonZeroUsize;
    use executable::{ExecutableTrait, LinkedHome, LinkedOptions, Standard};

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
                    .stack_size(64 * 1024)
                    .home(LinkedHome::User),
                standard,
                arguments,
            )
            .await
        }
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub use linked::*;
