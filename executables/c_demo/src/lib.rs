#![no_std]
#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
extern crate alloc;

use abi_application::{Directory, File, XILA_RIGHT_READ, XILA_RIGHT_WRITE, XilaStartup, run_main};
use alloc::{boxed::Box, string::String, vec::Vec};
use core::{fmt::Write, num::NonZeroUsize};
use executable::{ExecutableTrait, Standard};

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn c_demo_main(startup: *const XilaStartup) -> i32;
}

pub struct CDemoExecutable;

impl ExecutableTrait for CDemoExecutable {
    fn main(standard: Standard<'static>, arguments: Vec<String>) -> executable::MainFuture {
        Box::pin(main(standard, arguments))
    }
}

pub async fn main(
    mut standard: Standard<'static>,
    arguments: Vec<String>,
) -> Result<(), NonZeroUsize> {
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;
    let home = async {
        let user = context.task_manager.get_user(task).await.map_err(|_| ())?;
        let path = context
            .virtual_file_system
            .ensure_user_home(task, user)
            .await
            .map_err(|_| ())?;
        Directory::new(
            context.virtual_file_system,
            task,
            path.as_str(),
            XILA_RIGHT_READ | XILA_RIGHT_WRITE,
        )
        .map_err(|_| ())
    }
    .await;
    let home = match home {
        Ok(home) => home,
        Err(()) => {
            let _ = writeln!(standard.error(), "c_demo: cannot grant user home");
            return Err(NonZeroUsize::new(1).unwrap());
        }
    };
    abi_application::initialize(context.time_manager);
    let arguments: Vec<_> = arguments.iter().map(String::as_str).collect();
    let (input, output, error) = standard.split();
    // SAFETY: the linked entry borrows startup objects only until it returns.
    let result = unsafe {
        run_main(
            c_demo_main,
            &arguments,
            Some(File::new(input.into_synchronous_file())),
            Some(File::new(output.into_synchronous_file())),
            Some(File::new(error.into_synchronous_file())),
            Some(home),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(NonZeroUsize::new(result.unsigned_abs() as usize).unwrap())
    }
}
