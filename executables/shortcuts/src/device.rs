//! The emulation device transport for the WebAssembly guest: writes the
//! message bytes to `/devices/human_interface/emulation` and runs the
//! `GET_STATUS` control command through the `wasm::fd_control` import.

use std::{
    fs::{File, OpenOptions},
    io,
    os::fd::AsRawFd,
    thread::sleep,
    time::Duration,
};

use shortcuts::{
    Error, Result,
    channel::Channel,
    hid::{EMULATION_PATH, GET_STATUS},
};

/// Largest control payload the host accepts.
const MAXIMUM_CONTROL_PAYLOAD: usize = 256;

#[repr(align(16))]
struct Aligned<T>(T);

/// Maps a host error number to the application error. `WouldBlock` becomes
/// [`Error::Busy`]: the single bounded retry loop lives in
/// [`shortcuts::channel`], so it is not retried again here.
fn map_error(result: i32) -> Error {
    let error = io::Error::from_raw_os_error(result);
    match error.kind() {
        io::ErrorKind::NotFound => Error::Missing,
        // An interrupted call is retried like a busy one, by the single
        // bounded loop in `shortcuts::channel`.
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Error::Busy,
        io::ErrorKind::NotConnected => Error::NotConnected,
        _ => Error::Io,
    }
}

fn map_io_error(error: io::Error) -> Error {
    match error.kind() {
        io::ErrorKind::NotFound => Error::Missing,
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Error::Busy,
        io::ErrorKind::NotConnected => Error::NotConnected,
        _ => Error::Io,
    }
}

/// Runs one control command once; busy and interrupted answers are retried by
/// the bounded loop in [`shortcuts::channel`].
fn call_control(file: &File, command: u32, input: &[u8], output: &mut [u8]) -> Result<()> {
    let mut input_buffer = Aligned([0u8; MAXIMUM_CONTROL_PAYLOAD]);
    let mut output_buffer = Aligned([0u8; MAXIMUM_CONTROL_PAYLOAD]);
    input_buffer.0[..input.len()].copy_from_slice(input);
    let result = unsafe {
        wasm::fd_control(
            file.as_raw_fd(),
            command,
            input_buffer.0.as_ptr(),
            input.len() as u32,
            output_buffer.0.as_mut_ptr(),
            output.len() as u32,
        )
    };
    if result == 0 {
        output.copy_from_slice(&output_buffer.0[..output.len()]);
        return Ok(());
    }
    Err(map_error(result))
}

/// One opened emulation device.
pub struct Emulation {
    file: File,
}

impl Emulation {
    /// Opens the emulation device node. [`Error::Missing`] when this device
    /// has no such node.
    pub fn open() -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .open(EMULATION_PATH)
            .map_err(map_io_error)?;
        Ok(Self { file })
    }
}

impl Channel for Emulation {
    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        io::Write::write(&mut self.file, bytes).map_err(map_io_error)
    }

    fn control(&mut self, command: u32, input: &[u8], output: &mut [u8]) -> Result<()> {
        debug_assert!(command == GET_STATUS);
        debug_assert!(input.len() <= MAXIMUM_CONTROL_PAYLOAD);
        debug_assert!(output.len() <= MAXIMUM_CONTROL_PAYLOAD);
        call_control(&self.file, command, input, output)
    }

    fn pause(&mut self) {
        sleep(Duration::from_millis(1));
    }
}
