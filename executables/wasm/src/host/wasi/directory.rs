use wasmi::Caller;

use crate::{
    define_wasi_module,
    host::{
        store::GlobalStore,
        translation::{GuestPointer, GuestSlice, WasmUsize, borrow_memory, get_memory},
        wasi::{
            Error,
            error::{WasiResult, kind_error, wrap_function},
            rights::right,
        },
    },
};

/// Size of the fixed part of a directory record: next cookie, inode, name length, type, padding.
const RECORD_HEADER_LENGTH: usize = 24;

fn kind_to_filetype(kind: &xila::file_system::Kind) -> u8 {
    use xila::file_system::Kind;
    match kind {
        Kind::Directory => 3,
        Kind::CharacterDevice => 2,
        Kind::BlockDevice => 1,
        Kind::Pipe => 10,
        Kind::Socket => 11,
        Kind::SymbolicLink => 7,
        _ => 4,
    }
}

define_wasi_module! {
    module: "wasi_snapshot_preview1";

    fn fd_readdir(caller: Caller<GlobalStore>, fd: i32, buf_ptr: WasmUsize, buf_len: WasmUsize, cookie: u64, nread_ptr: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::FD_READDIR)?;
            let mut caller = caller;
            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            {
                let guest = borrow_memory(&memory, &mut caller);
                guest.validate_slice(GuestSlice::<u8>::new(buf_ptr, buf_len)).ok_or(Error::Fault)?;
                guest.validate_pointer(GuestPointer::<WasmUsize>::new(nread_ptr)).ok_or(Error::Fault)?;
            }
            if cookie == u64::MAX {
                borrow_memory(&memory, &mut caller).write(GuestPointer::new(nread_ptr), 0 as WasmUsize).ok_or(Error::Fault)?;
                return Ok(());
            }
            let start = usize::try_from(cookie).map_err(|_| Error::Inval)?;
            // Walk the directory entry by entry: skip to the cookie, then write each
            // record straight into the guest buffer. Only the current entry is held
            // on the host, so the space used does not grow with the directory.
            {
                let directory = caller.data_mut().wasi.get_synchronous_directory(fd as u32).ok_or(Error::Badf)?;
                directory.rewind().map_err(kind_error)?;
                for _ in 0..start {
                    if directory.read().map_err(kind_error)?.is_none() {
                        return Err(Error::Inval.into());
                    }
                }
            }
            let capacity = buf_len as usize;
            let mut written = 0usize;
            let mut index = start;
            loop {
                let entry = {
                    let directory = caller.data_mut().wasi.get_synchronous_directory(fd as u32).ok_or(Error::Badf)?;
                    directory.read().map_err(kind_error)?
                };
                let Some(entry) = entry else { break };
                let name = entry.name.as_bytes();
                let name_length = u32::try_from(name.len()).map_err(|_| Error::Overflow)?;
                let record_length = RECORD_HEADER_LENGTH.checked_add(name.len()).ok_or(Error::Overflow)?;
                // A record that does not fit is written truncated so the buffer is
                // filled completely; the guest sees a full buffer and retries from the
                // previous record's cookie with a larger one.
                let available = capacity - written;
                let fitted = record_length.min(available);
                let next = u64::try_from(index + 1).unwrap_or(u64::MAX);
                let mut header = [0u8; RECORD_HEADER_LENGTH];
                header[0..8].copy_from_slice(&next.to_le_bytes());
                header[8..16].copy_from_slice(&entry.inode.to_le_bytes());
                header[16..20].copy_from_slice(&name_length.to_le_bytes());
                header[20] = kind_to_filetype(&entry.kind);
                if fitted > 0 {
                    let mut guest = borrow_memory(&memory, &mut caller);
                    let record = guest
                        .bytes_mut(GuestSlice::new(buf_ptr + written as WasmUsize, fitted as WasmUsize))
                        .ok_or(Error::Fault)?;
                    let header_fitted = fitted.min(RECORD_HEADER_LENGTH);
                    record[..header_fitted].copy_from_slice(&header[..header_fitted]);
                    record[header_fitted..].copy_from_slice(&name[..fitted - header_fitted]);
                }
                written += fitted;
                index += 1;
                if fitted < record_length { break; }
            }
            borrow_memory(&memory, &mut caller)
                .write(GuestPointer::new(nread_ptr), written as WasmUsize)
                .ok_or(Error::Fault)?;
            Ok(())
        })
    }
}
