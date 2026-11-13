//! Positioned I/O: `fd_pread`, `fd_pwrite` and `fd_tell`.

use wasmi::Caller;
use xila::file_system::Position;

use crate::{
    define_wasi_module,
    host::{
        store::GlobalStore,
        translation::{GuestPointer, GuestSlice, WasiVector, WasmUsize, borrow_memory, get_memory},
        wasi::{
            Error,
            context::{FileSystemItem, WasiContext},
            error::{CombinedError, WasiResult, status, vfs_error, wrap_function},
            rights::right,
        },
    },
};

/// The file behind `fd`, if it has a position of its own. Standard streams are
/// not seekable (`ESPIPE`); directories are `EISDIR`.
fn positioned(
    wasi: &mut WasiContext,
    fd: i32,
) -> Result<&mut xila::virtual_file_system::SynchronousFile<'static>, Error> {
    if matches!(
        wasi.files.get(&(fd as u32)),
        Some(
            FileSystemItem::StandardInput(_)
                | FileSystemItem::StandardOutput(_)
                | FileSystemItem::StandardError(_)
        )
    ) {
        return Err(Error::Spipe);
    }
    wasi.file(fd)
}

/// Shared body of `fd_pread` and `fd_pwrite`.
///
/// Every iovec and the output pointer are validated before any byte moves, so
/// a bad guest pointer never leaves half an operation behind. Data goes
/// straight between guest memory and the file: nothing is allocated, and the
/// descriptor's own cursor is never read or moved. As in POSIX, an error after
/// some bytes were transferred reports the partial count instead.
fn transfer(
    caller: Caller<'_, GlobalStore>,
    fd: i32,
    iovs: WasmUsize,
    count: WasmUsize,
    offset: u64,
    output: WasmUsize,
    write: bool,
) -> Result<(), CombinedError> {
    let needed = if write {
        right::FD_WRITE
    } else {
        right::FD_READ
    } | right::FD_SEEK;
    caller.data().wasi.require_rights(fd, needed)?;
    let mut caller = caller;
    let memory = get_memory(&caller).ok_or(Error::Fault)?;
    let table = GuestSlice::<WasiVector>::new(iovs, count);
    {
        let guest = borrow_memory(&memory, &mut caller);
        guest
            .validate_pointer(GuestPointer::<WasmUsize>::new(output))
            .ok_or(Error::Fault)?;
        for vector in guest.values(table).ok_or(Error::Fault)? {
            guest
                .validate_slice(GuestSlice::<u8>::new(vector.buffer, vector.length))
                .ok_or(Error::Fault)?;
        }
    }
    positioned(&mut caller.data_mut().wasi, fd)?;

    let mut total = 0u64;
    for index in 0..count {
        let vector = {
            let guest = borrow_memory(&memory, &mut caller);
            guest.read(table.pointer_at(index).ok_or(Error::Fault)?)
        }
        .ok_or(Error::Fault)?;
        let position = offset.checked_add(total).ok_or(Error::Overflow)?;
        let (bytes, state) = memory.data_and_store_mut(&mut caller);
        let start = usize::try_from(vector.buffer).map_err(|_| Error::Fault)?;
        let end = start
            .checked_add(usize::try_from(vector.length).map_err(|_| Error::Fault)?)
            .ok_or(Error::Fault)?;
        let file = state.wasi.file(fd)?;
        let moved = if write {
            file.write_at(bytes.get(start..end).ok_or(Error::Fault)?, position)
        } else {
            file.read_at(bytes.get_mut(start..end).ok_or(Error::Fault)?, position)
        };
        let moved = match moved {
            Ok(moved) => moved,
            Err(_) if total > 0 => break,
            Err(error) => return Err(vfs_error(error).into()),
        };
        total = total.checked_add(moved as u64).ok_or(Error::Overflow)?;
        if moved < end - start {
            break;
        }
    }
    borrow_memory(&memory, &mut caller)
        .write(
            GuestPointer::new(output),
            WasmUsize::try_from(total).map_err(|_| Error::Overflow)?,
        )
        .ok_or(Error::Fault)?;
    Ok(())
}

define_wasi_module! {
    module: "wasi_snapshot_preview1";

    fn fd_pread(caller: Caller<GlobalStore>, fd: i32, iovs: WasmUsize, count: WasmUsize, offset: i64, output: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        status(transfer(caller, fd, iovs, count, offset as u64, output, false))
    }

    fn fd_pwrite(caller: Caller<GlobalStore>, fd: i32, iovs: WasmUsize, count: WasmUsize, offset: i64, output: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        status(transfer(caller, fd, iovs, count, offset as u64, output, true))
    }

    fn fd_tell(caller: Caller<GlobalStore>, fd: i32, output: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::FD_TELL)?;
            let mut caller = caller;
            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            borrow_memory(&memory, &mut caller)
                .validate_pointer(GuestPointer::<u64>::new(output))
                .ok_or(Error::Fault)?;
            let position = positioned(&mut caller.data_mut().wasi, fd)?
                .set_position(&Position::Current(0))
                .map_err(vfs_error)?;
            borrow_memory(&memory, &mut caller)
                .write(GuestPointer::new(output), position)
                .ok_or(Error::Fault)?;
            Ok(())
        })
    }
}
