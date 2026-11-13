//! Preview 1 socket imports.
//!
//! Preview 1 has no way to create a socket: it can only operate on sockets the
//! host pre-opened, and Xila pre-opens none. These imports exist so guests
//! that reference them link, and they classify the descriptor exactly as a
//! host without sockets must: `EBADF` for a descriptor that is not open,
//! `ENOTSOCK` for one that is open but is no socket. No argument beyond the
//! descriptor is looked at, because no call can get past that check.
//!
//! Future work: Core's network subsystem is reachable as
//! `ExecutableContext::network_manager`. Pre-opening a listening socket would
//! add a socket variant to `FileSystemItem` (and its `fd_fdstat_get` filetype),
//! after which these imports validate `flags`/`how` and drive that subsystem.

use wasmi::Caller;

use crate::{
    define_wasi_module,
    host::{
        store::GlobalStore,
        translation::WasmUsize,
        wasi::{Error, error::WasiResult},
    },
};

fn not_a_socket(caller: &Caller<'_, GlobalStore>, fd: i32) -> Error {
    if caller.data().wasi.files.contains_key(&(fd as u32)) {
        Error::Notsock
    } else {
        Error::Badf
    }
}

define_wasi_module! {
    module: "wasi_snapshot_preview1";

    fn sock_accept(caller: Caller<GlobalStore>, fd: i32, _flags: i32, _accepted: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        Ok(i32::from(not_a_socket(&caller, fd)))
    }

    fn sock_recv(caller: Caller<GlobalStore>, fd: i32, _iovs: WasmUsize, _count: WasmUsize, _flags: i32, _received: WasmUsize, _output_flags: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        Ok(i32::from(not_a_socket(&caller, fd)))
    }

    fn sock_send(caller: Caller<GlobalStore>, fd: i32, _iovs: WasmUsize, _count: WasmUsize, _flags: i32, _sent: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        Ok(i32::from(not_a_socket(&caller, fd)))
    }

    fn sock_shutdown(caller: Caller<GlobalStore>, fd: i32, _how: i32) -> Result<WasiResult, wasmi::Error> {
        Ok(i32::from(not_a_socket(&caller, fd)))
    }
}
