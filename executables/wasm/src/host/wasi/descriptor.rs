//! Descriptor-level imports that change or synchronise an open descriptor:
//! `fd_sync`, `fd_datasync`, `fd_advise`, `fd_allocate`, `fd_filestat_set_size`,
//! `fd_filestat_set_times`, `fd_fdstat_set_flags`, `fd_fdstat_set_rights` and
//! `fd_renumber`.

use wasmi::Caller;
use xila::file_system::{AccessFlags, StateFlags};

use crate::{
    define_wasi_module,
    host::{
        store::GlobalStore,
        wasi::{
            Error,
            context::FileSystemItem,
            error::{WasiResult, vfs_error, wrap_function},
            path::set_directory_times,
            rights::{self, right},
            time::{realtime_nanoseconds, requested_times},
        },
    },
};

/// `fdflags` bits of `fd_fdstat_set_flags`.
mod fdflags {
    pub const APPEND: u32 = 1 << 0;
    pub const DSYNC: u32 = 1 << 1;
    pub const NONBLOCK: u32 = 1 << 2;
    pub const RSYNC: u32 = 1 << 3;
    pub const SYNC: u32 = 1 << 4;
}

/// Number of `advice` values of `fd_advise` (normal, sequential, random,
/// willneed, dontneed, noreuse).
const ADVICE_COUNT: i32 = 6;

/// Flush an open file. Directories have nothing to flush: the file system
/// commits directory entries as part of each namespace operation.
fn flush(caller: &mut Caller<'_, GlobalStore>, fd: i32, needed: u64) -> Result<(), Error> {
    caller.data().wasi.require_rights(fd, needed)?;
    match caller.data_mut().wasi.files.get_mut(&(fd as u32)) {
        None => Err(Error::Badf),
        Some(item) => match item.as_synchronous_file_mut() {
            Some(file) => file.flush().map_err(vfs_error),
            None => Ok(()),
        },
    }
}

define_wasi_module! {
    module: "wasi_snapshot_preview1";

    fn fd_sync(caller: Caller<GlobalStore>, fd: i32) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            flush(&mut caller, fd, right::FD_SYNC)?;
            Ok(())
        })
    }

    fn fd_datasync(caller: Caller<GlobalStore>, fd: i32) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            flush(&mut caller, fd, right::FD_DATASYNC)?;
            Ok(())
        })
    }

    // Advice is a hint that Xila's file systems have no use for (no page
    // cache, no read-ahead), so a valid request on an open file is a success.
    fn fd_advise(caller: Caller<GlobalStore>, fd: i32, _offset: i64, _length: i64, advice: i32) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::FD_ADVISE)?;
            let mut caller = caller;
            caller.data_mut().wasi.file(fd)?;
            if !(0..ADVICE_COUNT).contains(&advice) {
                return Err(Error::Inval.into());
            }
            Ok(())
        })
    }

    // Like posix_fallocate: grow the file to `offset + length` when it is
    // shorter (new bytes read as zero) and never shrink it. A zero length is
    // invalid, and the descriptor must be writable.
    fn fd_allocate(caller: Caller<GlobalStore>, fd: i32, offset: i64, length: i64) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::FD_ALLOCATE)?;
            let mut caller = caller;
            let file = caller.data_mut().wasi.file(fd)?;
            if length as u64 == 0 {
                return Err(Error::Inval.into());
            }
            let end = (offset as u64).checked_add(length as u64).ok_or(Error::Fbig)?;
            if !file.get_access().map_err(vfs_error)?.contains(AccessFlags::Write) {
                return Err(Error::Badf.into());
            }
            if end > file.get_statistics().map_err(vfs_error)?.size {
                file.set_size(end).map_err(vfs_error)?;
            }
            Ok(())
        })
    }

    fn fd_filestat_set_size(caller: Caller<GlobalStore>, fd: i32, size: i64) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::FD_FILESTAT_SET_SIZE)?;
            let mut caller = caller;
            caller.data_mut().wasi.file(fd)?.set_size(size as u64).map_err(vfs_error)?;
            Ok(())
        })
    }

    fn fd_filestat_set_times(caller: Caller<GlobalStore>, fd: i32, atim: i64, mtim: i64, flags: i32) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::FD_FILESTAT_SET_TIMES)?;
            let mut caller = caller;
            let (access, modification) = requested_times(atim as u64, mtim as u64, flags, || realtime_nanoseconds(&caller))?;
            if access.is_none() && modification.is_none() {
                return Ok(());
            }
            if caller.data().wasi.is_directory(fd as u32) {
                // Directories carry no handle authority to update through:
                // address the directory by its path, still beneath its preopen.
                set_directory_times(&caller, fd, access, modification)?;
            } else {
                caller.data_mut().wasi.file(fd)?.set_times(access, modification).map_err(vfs_error)?;
            }
            Ok(())
        })
    }

    // The flags replace the descriptor's status flags. Append and nonblocking
    // are honoured; the synchronisation flags are not, because the file
    // systems offer no per-descriptor write-through mode.
    fn fd_fdstat_set_flags(caller: Caller<GlobalStore>, fd: i32, flags: i32) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::FD_FDSTAT_SET_FLAGS)?;
            let mut caller = caller;
            let flags = flags as u32;
            let file = caller.data_mut().wasi.file(fd)?;
            if flags & !(fdflags::APPEND | fdflags::DSYNC | fdflags::NONBLOCK | fdflags::RSYNC | fdflags::SYNC) != 0 {
                return Err(Error::Inval.into());
            }
            if flags & (fdflags::DSYNC | fdflags::RSYNC | fdflags::SYNC) != 0 {
                return Err(Error::Notsup.into());
            }
            let mut state = StateFlags::empty();
            if flags & fdflags::APPEND != 0 {
                state |= StateFlags::Append;
            }
            if flags & fdflags::NONBLOCK != 0 {
                state |= StateFlags::NonBlocking;
            }
            file.set_state(state).map_err(vfs_error)?;
            Ok(())
        })
    }

    // Rights can only be removed: asking for a right the descriptor no longer
    // holds is ENOTCAPABLE. See `rights` for the model.
    fn fd_fdstat_set_rights(caller: Caller<GlobalStore>, fd: i32, base: i64, inheriting: i64) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            let wasi = &mut caller.data_mut().wasi;
            if !wasi.files.contains_key(&(fd as u32)) {
                return Err(Error::Badf.into());
            }
            let (base, inheriting) = (base as u64, inheriting as u64);
            let held = wasi.rights_of(fd as u32);
            if base & !held.0 != 0 || inheriting & !held.1 != 0 {
                return Err(Error::Notcapable.into());
            }
            if (base, inheriting) == rights::ALL {
                wasi.rights.remove(&(fd as u32));
            } else {
                wasi.rights.insert(fd as u32, (base, inheriting));
            }
            Ok(())
        })
    }

    // Atomically makes `to` the descriptor `fd` was, closing what `to` was.
    // The environment offers no dup2: both descriptors must already be open,
    // so a guest cannot invent descriptor numbers. Rights, the preopen name
    // and a directory's confinement root move with the descriptor, so a
    // renumbered preopen stays a preopen, and the descriptor it replaced stops
    // being one.
    fn fd_renumber(caller: Caller<GlobalStore>, fd: i32, to: i32) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            let wasi = &mut caller.data_mut().wasi;
            let (from, to) = (fd as u32, to as u32);
            if !wasi.files.contains_key(&from) || !wasi.files.contains_key(&to) {
                return Err(Error::Badf.into());
            }
            if from == to {
                return Ok(());
            }
            let moved = wasi.files.remove(&from).ok_or(Error::Badf)?;
            let replaced = wasi.files.insert(to, moved).ok_or(Error::Badf)?;
            match wasi.rights.remove(&from) {
                Some(rights) => wasi.rights.insert(to, rights),
                None => wasi.rights.remove(&to),
            };
            match wasi.prestats.remove(&from) {
                Some(prestat) => wasi.prestats.insert(to, prestat),
                None => wasi.prestats.remove(&to),
            };
            let virtual_file_system = caller.data().context.virtual_file_system;
            match replaced {
                FileSystemItem::File(file)
                | FileSystemItem::StandardInput(file)
                | FileSystemItem::StandardOutput(file)
                | FileSystemItem::StandardError(file) => file.file.close(virtual_file_system),
                FileSystemItem::Directory(directory) => directory.directory.close(virtual_file_system),
            }
            .map_err(vfs_error)?;
            Ok(())
        })
    }
}
