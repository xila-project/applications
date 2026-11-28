#![no_std]
#![cfg(xila_linked_hexdump)]

// Platforms will measure peak stack use on the boards.
#[cfg(target_os = "none")]
const STACK_SIZE: usize = 16 * 1024;
#[cfg(not(target_os = "none"))]
const STACK_SIZE: usize = 64 * 1024;

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn xila_linked_hexdump(startup: *const abi_application::XilaStartup) -> i32;
}

executable::linked_executable!(
    /// Dump or reverse hexadecimal data with the SDK's POSIX example.
    ///
    /// The program receives read and write access to the user's home directory.
    /// File arguments must refer to paths under that directory.
    unsafe pub HexdumpExecutable => xila_linked_hexdump,
    exclusive,
    stack_size = STACK_SIZE,
);
