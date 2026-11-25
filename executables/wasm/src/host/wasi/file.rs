use wasmi::Caller;
use xila::file_system::{AccessFlags, ControlCommandIdentifier, Position, StateFlags};
use xila::shared::AnyByLayout;

use crate::{
    define_wasi_module,
    host::{
        store::GlobalStore,
        translation::{GuestPointer, GuestSlice, WasiVector, WasmUsize, borrow_memory, get_memory},
        wasi::{
            Error,
            context::{FileSystemItem, FileVariant},
            error::{CombinedError, WasiResult, kind_error, status, wrap_function},
            path::make_filestat,
            rights::right,
            types::{Fdstat, Prestat as PrestatAbi, PrestatDir},
        },
    },
};

/// Stack-cached iovec entries to avoid re-reading guest memory (or observing
/// guest-buffer overwrites) in the common case without heap allocation.
const INLINE_IOVECS: usize = 16;

/// Maximum input or output payload size accepted by the generic descriptor control ABI.
const MAX_CONTROL_PAYLOAD: usize = 256;

#[repr(align(16))]
struct AlignedControlPayload([u8; MAX_CONTROL_PAYLOAD]);

const CONTROL_PAYLOAD_ALIGNMENT: usize = core::mem::align_of::<AlignedControlPayload>();

fn spans_overlap(
    first_start: usize,
    first_length: usize,
    second_start: usize,
    second_length: usize,
) -> Option<bool> {
    let first_end = first_start.checked_add(first_length)?;
    let second_end = second_start.checked_add(second_length)?;
    Some(first_start < second_end && second_start < first_end)
}

/// Shared body of `fd_read` and `fd_write`.
///
/// Validates the output pointer, the iovec table, and every iovec buffer
/// before moving any byte, then reads or writes straight into or out of guest
/// memory with zero host allocation. Stops on the first short transfer; an
/// error after bytes have already moved returns the transferred count.
fn transfer(
    caller: Caller<'_, GlobalStore>,
    fd: i32,
    iovs: WasmUsize,
    count: WasmUsize,
    output: WasmUsize,
    write: bool,
) -> Result<(), CombinedError> {
    let needed = if write {
        right::FD_WRITE
    } else {
        right::FD_READ
    };
    caller.data().wasi.require_rights(fd, needed)?;
    let mut caller = caller;
    let memory = get_memory(&caller).ok_or(Error::Fault)?;
    let table = GuestSlice::<WasiVector>::new(iovs, count);
    let mut snapshot = [WasiVector::new(0, 0); INLINE_IOVECS];
    let count_usize = usize::try_from(count).map_err(|_| Error::Fault)?;
    {
        let guest = borrow_memory(&memory, &mut caller);
        guest
            .validate_pointer(GuestPointer::<WasmUsize>::new(output))
            .ok_or(Error::Fault)?;
        let values = guest.values(table).ok_or(Error::Fault)?;
        for (index, vector) in values.enumerate() {
            guest
                .validate_slice(GuestSlice::<u8>::new(vector.buffer, vector.length))
                .ok_or(Error::Fault)?;
            if index < INLINE_IOVECS {
                snapshot[index] = vector;
            }
        }
    }
    let needed_access = if write {
        AccessFlags::Write
    } else {
        AccessFlags::Read
    };
    let file = caller.data_mut().wasi.file(fd)?;
    if !file
        .get_access()
        .map_err(kind_error)?
        .contains(needed_access)
    {
        return Err(Error::Badf.into());
    }

    let mut total = 0u64;
    for index in 0..count_usize {
        let vector = if let Some(&vector) = snapshot.get(index) {
            vector
        } else {
            let entry = table
                .pointer_at(WasmUsize::try_from(index).map_err(|_| Error::Fault)?)
                .ok_or(Error::Fault)?;
            borrow_memory(&memory, &mut caller)
                .read(entry)
                .ok_or(Error::Fault)?
        };
        let start = usize::try_from(vector.buffer).map_err(|_| Error::Fault)?;
        let length = usize::try_from(vector.length).map_err(|_| Error::Fault)?;
        let end = start.checked_add(length).ok_or(Error::Fault)?;
        if start == end {
            continue;
        }
        let (bytes, state) = memory.data_and_store_mut(&mut caller);
        let file = state.wasi.file(fd)?;
        let moved = if write {
            file.write(bytes.get(start..end).ok_or(Error::Fault)?)
        } else {
            file.read(bytes.get_mut(start..end).ok_or(Error::Fault)?)
        };
        let moved = match moved {
            Ok(moved) => moved,
            Err(_) if total > 0 => break,
            Err(error) => return Err(kind_error(error).into()),
        };
        total = total.checked_add(moved as u64).ok_or(Error::Overflow)?;
        if moved < length {
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

    fn fd_read(caller: Caller<GlobalStore>, fd: i32, iovs: WasmUsize, iovs_len: WasmUsize, nread_ptr: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        status(transfer(caller, fd, iovs, iovs_len, nread_ptr, false))
    }

    fn fd_write(caller: Caller<GlobalStore>, fd: i32, iovs: WasmUsize, iovs_len: WasmUsize, nwritten_ptr: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        status(transfer(caller, fd, iovs, iovs_len, nwritten_ptr, true))
    }

    fn fd_control(caller: Caller<GlobalStore>, fd: i32, command: u32, input_ptr: WasmUsize, input_len: WasmUsize, output_ptr: WasmUsize, output_len: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            let fd = u32::try_from(fd).map_err(|_| Error::Badf)?;
            let is_directory = caller.data().wasi.is_directory(fd);
            if caller.data_mut().wasi.get_synchronous_file(fd).is_none() {
                return Err(if is_directory { Error::Isdir } else { Error::Badf }.into());
            }

            let input_length = usize::try_from(input_len).map_err(|_| Error::Inval)?;
            let output_length = usize::try_from(output_len).map_err(|_| Error::Inval)?;
            if input_length > MAX_CONTROL_PAYLOAD || output_length > MAX_CONTROL_PAYLOAD {
                return Err(Error::Inval.into());
            }
            let input_start = usize::try_from(input_ptr).map_err(|_| Error::Fault)?;
            let output_start = usize::try_from(output_ptr).map_err(|_| Error::Fault)?;
            if input_start % CONTROL_PAYLOAD_ALIGNMENT != 0
                || output_start % CONTROL_PAYLOAD_ALIGNMENT != 0
            {
                return Err(Error::Inval.into());
            }
            if spans_overlap(input_start, input_length, output_start, output_length).ok_or(Error::Fault)? {
                return Err(Error::Inval.into());
            }

            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            let input_slice = GuestSlice::<u8>::new(input_ptr, input_len);
            let output_slice = GuestSlice::<u8>::new(output_ptr, output_len);
            let mut input = AlignedControlPayload([0; MAX_CONTROL_PAYLOAD]);
            let mut output = AlignedControlPayload([0; MAX_CONTROL_PAYLOAD]);
            {
                let guest = borrow_memory(&memory, &mut caller);
                let guest_input = guest.bytes(input_slice).ok_or(Error::Fault)?;
                input.0[..input_length].copy_from_slice(guest_input);
                guest.validate_slice(output_slice).ok_or(Error::Fault)?;
            }

            // Both scratch buffers have 16-byte alignment. Their lengths are
            // bounded above, and they remain alive throughout the VFS call.
            let input_layout = unsafe {
                AnyByLayout::from_raw_parts(input.0.as_mut_ptr(), input_length)
            };
            let output_layout = unsafe {
                AnyByLayout::from_raw_parts(output.0.as_mut_ptr(), output_length)
            };
            caller.data_mut().wasi.get_synchronous_file(fd)
                .ok_or(Error::Badf)?
                .control_raw(ControlCommandIdentifier::from_raw(command), input_layout, output_layout)
                .map_err(kind_error)?;

            borrow_memory(&memory, &mut caller)
                .bytes_mut(output_slice)
                .ok_or(Error::Fault)?
                .copy_from_slice(&output.0[..output_length]);
            Ok(())
        })
    }

    fn fd_close(caller: Caller<GlobalStore>, fd: i32) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            let item = caller.data_mut().wasi.pop_file_system_item(fd as u32).ok_or(Error::Badf)?;
            caller.data_mut().wasi.rights.remove(&(fd as u32));
            caller.data_mut().wasi.prestats.remove(&(fd as u32));
            let vfs = caller.data().context.virtual_file_system;
            match item {
                FileSystemItem::File(file) | FileSystemItem::StandardInput(file) | FileSystemItem::StandardOutput(file) | FileSystemItem::StandardError(file) => file.file.close(vfs).map_err(kind_error)?,
                FileSystemItem::Directory(dir) => dir.directory.close(vfs).map_err(kind_error)?,
            }
            Ok(())
        })
    }

    fn fd_seek(caller: Caller<GlobalStore>, fd: i32, offset: i64, whence: i32, newoffset_ptr: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            // Learning the current position (`lseek(fd, 0, SEEK_CUR)`) needs only FD_TELL.
            let tell_only = offset == 0 && whence == 1;
            caller.data().wasi.require_rights(fd, if tell_only { right::FD_TELL } else { right::FD_SEEK })?;
            let mut caller = caller;
            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            let position = match whence { 0 if offset >= 0 => Position::Start(offset as u64), 0 => Err(Error::Inval)?, 1 => Position::Current(offset), 2 => Position::End(offset), _ => Err(Error::Inval)? };
            let is_directory = caller.data().wasi.is_directory(fd as u32);
            let new_position = match caller.data_mut().wasi.get_synchronous_file(fd as u32) {
                Some(file) => file.set_position(&position).map_err(kind_error)?,
                None if is_directory => return Err(Error::Isdir.into()),
                None => return Err(Error::Badf.into()),
            };
            borrow_memory(&memory, &mut caller).write(GuestPointer::new(newoffset_ptr), new_position).ok_or(Error::Fault)?;
            Ok(())
        })
    }

    fn fd_fdstat_get(caller: Caller<GlobalStore>, fd: i32, stat_ptr: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            let (rights_base, rights_inheriting) = caller.data().wasi.rights_of(fd as u32);
            let item = caller.data_mut().wasi.get_file_system_item(fd as u32).ok_or(Error::Badf)?;
            let filetype = item.file_type();
            let state = match item.as_synchronous_file_mut() {
                Some(file) => file.get_state().map_err(kind_error)?,
                None => StateFlags::empty(),
            };
            let flags = (if state.contains(StateFlags::Append) { 1 } else { 0 }) | (if state.contains(StateFlags::NonBlocking) { 4 } else { 0 });
            borrow_memory(&memory, &mut caller).write(GuestPointer::new(stat_ptr),
                Fdstat { filetype, flags, rights_base, rights_inheriting }).ok_or(Error::Fault)?;
            Ok(())
        })
    }

    fn fd_filestat_get(caller: Caller<GlobalStore>, fd: i32, stat_ptr: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::FD_FILESTAT_GET)?;
            let mut caller = caller;
            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            let (filetype, metadata) = {
                let item = caller.data_mut().wasi.get_file_system_item(fd as u32).ok_or(Error::Badf)?;
                let filetype = item.file_type();
                let metadata = match item {
                    FileSystemItem::File(FileVariant { file }) | FileSystemItem::StandardInput(FileVariant { file }) | FileSystemItem::StandardOutput(FileVariant { file }) | FileSystemItem::StandardError(FileVariant { file }) => file.get_statistics().map_err(kind_error)?,
                    FileSystemItem::Directory(dir) => dir.directory.get_statistics().map_err(kind_error)?,
                };
                (filetype, metadata)
            };
            borrow_memory(&memory, &mut caller).write(GuestPointer::new(stat_ptr),
                make_filestat(&metadata, filetype)).ok_or(Error::Fault)?;
            Ok(())
        })
    }

    fn fd_prestat_get(caller: Caller<GlobalStore>, fd: i32, prestat_ptr: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            let name_len = u32::try_from(caller.data().wasi.prestats.get(&(fd as u32)).ok_or(Error::Badf)?.name.len()).map_err(|_| Error::Overflow)?;
            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            borrow_memory(&memory, &mut caller).write(GuestPointer::new(prestat_ptr),
                PrestatAbi { tag: 0, dir: PrestatDir { pr_name_len: name_len } }).ok_or(Error::Fault)?;
            Ok(())
        })
    }

    fn fd_prestat_dir_name(caller: Caller<GlobalStore>, fd: i32, path_ptr: WasmUsize, path_len: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let name = caller.data().wasi.prestats.get(&(fd as u32)).ok_or(Error::Badf)?.name.clone();
            if name.len() > path_len as usize { return Err(Error::Nametoolong.into()); }
            let mut caller = caller;
            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            borrow_memory(&memory, &mut caller).bytes_mut(GuestSlice::new(path_ptr, path_len))
                .ok_or(Error::Fault)?[..name.len()].copy_from_slice(&name);
            Ok(())
        })
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_types, reason = "host-only unit tests")]
mod tests {
    extern crate std;

    use super::*;
    use alloc::{boxed::Box, vec};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wasmi::{Config, Engine, Linker, Module, Store};
    use xila::{
        file_system::{
            self, AccessFlags, ControlDirectionFlags, DirectBaseOperations, DirectCharacterDevice,
            Flags, MountOperations, Path, PathOwned, Size,
        },
        shared::AnyByLayout,
        virtual_file_system::{SynchronousDirectory, SynchronousFile},
    };

    const CONTROL: ControlCommandIdentifier =
        ControlCommandIdentifier::new_with_size(ControlDirectionFlags::Write, 8, b'X', 7);
    static CONTROL_CALLS: AtomicUsize = AtomicUsize::new(0);

    struct ControlDevice;

    impl MountOperations for ControlDevice {}
    impl DirectCharacterDevice for ControlDevice {}

    impl DirectBaseOperations for ControlDevice {
        fn read(
            &self,
            _cx: &mut core::task::Context<'_>,
            _: &mut [u8],
            _: Size,
        ) -> core::task::Poll<file_system::Result<usize>> {
            xila::file_system::ready(|| Err(file_system::IoErrorKind::Unsupported))
        }

        fn write(
            &self,
            _cx: &mut core::task::Context<'_>,
            buffer: &[u8],
            _: Size,
        ) -> core::task::Poll<file_system::Result<usize>> {
            xila::file_system::ready(|| Ok(buffer.len()))
        }

        fn control(
            &self,
            _cx: &mut core::task::Context<'_>,
            command: ControlCommandIdentifier,
            input: &AnyByLayout,
            output: &mut AnyByLayout,
        ) -> core::task::Poll<file_system::Result<()>> {
            xila::file_system::ready(|| {
                CONTROL_CALLS.fetch_add(1, Ordering::SeqCst);
                if command != CONTROL {
                    return Err(file_system::IoErrorKind::Unsupported);
                }
                let input = input
                    .cast::<u32>()
                    .ok_or(file_system::IoErrorKind::InvalidArgument)?;
                let output = output
                    .cast_mutable::<u32>()
                    .ok_or(file_system::IoErrorKind::InvalidArgument)?;
                *output = input.wrapping_add(1);
                Ok(())
            })
        }
    }

    #[xila::task::test(task_path = xila::task)]
    async fn fd_control_forwards_opaque_command_and_payloads() {
        testing::instantiate_global_allocator!();

        xila::log::initialize(&testing::Logger).unwrap();
        let task_manager = xila::task::test_manager();
        let users_manager = Box::leak(Box::new(xila::users::Manager::new()));
        let time_manager = Box::leak(Box::new(
            xila::time::Manager::new(&testing::TimeDevice).unwrap(),
        ));
        let memory_device = xila::file_system::MemoryDevice::<512>::new_static(1024 * 512);
        let filesystem = xila::little_fs::FileSystem::new_format(
            memory_device,
            xila::little_fs::Options::new(256),
        )
        .unwrap();
        let vfs = Box::leak(Box::new(
            xila::virtual_file_system::initialize(
                task_manager,
                users_manager,
                time_manager,
                filesystem,
            )
            .unwrap(),
        ));
        let task = task_manager.get_current_task_identifier().await;
        xila::virtual_file_system::create_default_hierarchy(vfs, task)
            .await
            .unwrap();
        let context = Box::leak(Box::new(xila::executable::ExecutableContext {
            task_manager,
            users_manager,
            virtual_file_system: vfs,
            graphics_manager: None,
            network_manager: None,
            formats: &[],
            time_manager,
        }));
        vfs.mount_character_device(task, Path::from_str("/fd-control"), ControlDevice)
            .await
            .unwrap();
        let file = SynchronousFile::open(
            vfs,
            task,
            Path::from_str("/fd-control"),
            Flags::new(AccessFlags::READ_WRITE, None, None),
        )
        .unwrap();
        let readonly_file = SynchronousFile::open(
            vfs,
            task,
            Path::from_str("/fd-control"),
            Flags::new(AccessFlags::Read, None, None),
        )
        .unwrap();
        let directory = SynchronousDirectory::open(vfs, task, Path::from_str("/")).unwrap();

        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let mut linker = Linker::new(&engine);
        crate::host::wasi::register::add_wasi_to_linker(&mut linker).unwrap();
        let word = if cfg!(feature = "memory_64") {
            "i64"
        } else {
            "i32"
        };
        let wasm = wat::parse_str(alloc::format!(
            r#"(module
                (import "wasi_snapshot_preview1" "fd_control"
                    (func $control (param i32 i32 {word} {word} {word} {word}) (result i32)))
                (memory (export "memory") 1)
                (func (export "run") (param i32 i32 {word} {word} {word} {word}) (result i32)
                    i32.const 32 i32.const 42 i32.store
                    local.get 0 local.get 1 local.get 2 local.get 3 local.get 4 local.get 5
                    call $control)
            )"#,
        ))
        .unwrap();
        let module = Module::new(&engine, &wasm[..]).unwrap();
        let mut wasi = crate::host::wasi::WasiContext {
            files: Default::default(),
            rights: Default::default(),
            next_fd: 7,
            arguments: vec![],
            environment: vec![],
            task,
            prestats: Default::default(),
            exit_code: None,
        };
        wasi.files
            .insert(4, FileSystemItem::File(FileVariant { file }));
        wasi.files.insert(
            6,
            FileSystemItem::File(FileVariant {
                file: readonly_file,
            }),
        );
        wasi.files.insert(
            5,
            FileSystemItem::Directory(crate::host::wasi::context::DirectoryVariant {
                confinement_root: None,
                path: PathOwned::root(),
                directory,
            }),
        );
        let mut store = Store::new(
            &engine,
            GlobalStore {
                control: Default::default(),
                context,
                pending: None,
                wasi,
                #[cfg(feature = "graphics")]
                graphics: crate::host::graphics::GraphicsState::new(context.graphics_manager),
            },
        );
        store.set_fuel(10_000).unwrap();
        let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
        let run = instance
            .get_typed_func::<(i32, i32, WasmUsize, WasmUsize, WasmUsize, WasmUsize), i32>(
                &store, "run",
            )
            .unwrap();
        CONTROL_CALLS.store(0, Ordering::SeqCst);
        assert_eq!(
            run.call(&mut store, (4, CONTROL.raw() as i32, 32, 4, 48, 4))
                .unwrap(),
            0
        );
        let memory = instance.get_memory(&store, "memory").unwrap();
        assert_eq!(&memory.data(&store)[48..52], &43u32.to_le_bytes());
        assert_eq!(CONTROL_CALLS.load(Ordering::SeqCst), 1);

        // Write-directed controls require a writable descriptor, including
        // through the guest ABI. Rejection must not dispatch or copy output.
        assert_eq!(
            run.call(&mut store, (6, CONTROL.raw() as i32, 32, 4, 48, 4))
                .unwrap(),
            8
        );
        assert_eq!(CONTROL_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(&memory.data(&store)[48..52], &43u32.to_le_bytes());

        assert_eq!(
            run.call(&mut store, (-1, CONTROL.raw() as i32, 32, 4, 48, 4))
                .unwrap(),
            8
        );
        assert_eq!(
            run.call(&mut store, (99, CONTROL.raw() as i32, 32, 4, 48, 4))
                .unwrap(),
            8
        );
        assert_eq!(
            run.call(&mut store, (5, CONTROL.raw() as i32, 32, 4, 48, 4))
                .unwrap(),
            31
        );
        assert_eq!(
            run.call(&mut store, (4, CONTROL.raw() as i32, 65_536, 4, 48, 4))
                .unwrap(),
            21
        );
        assert_eq!(
            run.call(&mut store, (4, CONTROL.raw() as i32, 32, 4, 65_536, 4))
                .unwrap(),
            21
        );
        assert_eq!(
            run.call(&mut store, (4, CONTROL.raw() as i32, 32, 257, 48, 4))
                .unwrap(),
            28
        );
        assert_eq!(
            run.call(&mut store, (4, CONTROL.raw() as i32, 32, 4, 48, 257))
                .unwrap(),
            28
        );
        assert_eq!(
            run.call(&mut store, (4, CONTROL.raw() as i32, 33, 4, 48, 4))
                .unwrap(),
            28
        );
        assert_eq!(
            run.call(&mut store, (4, CONTROL.raw() as i32, 32, 4, 49, 4))
                .unwrap(),
            28
        );
        assert_eq!(
            run.call(&mut store, (4, CONTROL.raw() as i32, 32, 4, 32, 4))
                .unwrap(),
            28
        );
        let unsupported =
            ControlCommandIdentifier::new_with_size(ControlDirectionFlags::Write, 8, b'Z', 7).raw()
                as i32;
        assert_eq!(
            run.call(&mut store, (4, unsupported, 32, 4, 48, 4))
                .unwrap(),
            58
        );
        assert_eq!(CONTROL_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(&memory.data(&store)[48..52], &43u32.to_le_bytes());
    }

    #[test]
    fn raw_command_word_preserves_identifier_bits() {
        assert_eq!(ControlCommandIdentifier::from_raw(CONTROL.raw()), CONTROL);
    }

    struct StreamState {
        read_data: alloc::vec::Vec<u8>,
        read_offset: usize,
        read_chunk: usize,
        read_calls: usize,
        fail_read_on_call: usize,
        written: alloc::vec::Vec<u8>,
        write_chunk: usize,
        write_calls: usize,
        fail_write_on_call: usize,
    }

    struct StreamDevice(&'static std::sync::Mutex<StreamState>);

    impl MountOperations for StreamDevice {}
    impl DirectCharacterDevice for StreamDevice {}

    impl DirectBaseOperations for StreamDevice {
        fn read(
            &self,
            _cx: &mut core::task::Context<'_>,
            buffer: &mut [u8],
            _: Size,
        ) -> core::task::Poll<file_system::Result<usize>> {
            xila::file_system::ready(|| {
                let mut state = self.0.lock().unwrap();
                state.read_calls += 1;
                if state.read_calls == state.fail_read_on_call {
                    return Err(file_system::IoErrorKind::InputOutput);
                }
                let remaining = state.read_data.len().saturating_sub(state.read_offset);
                let count = buffer.len().min(state.read_chunk).min(remaining);
                let start = state.read_offset;
                buffer[..count].copy_from_slice(&state.read_data[start..start + count]);
                state.read_offset += count;
                Ok(count)
            })
        }

        fn write(
            &self,
            _cx: &mut core::task::Context<'_>,
            buffer: &[u8],
            _: Size,
        ) -> core::task::Poll<file_system::Result<usize>> {
            xila::file_system::ready(|| {
                let mut state = self.0.lock().unwrap();
                state.write_calls += 1;
                if state.write_calls == state.fail_write_on_call {
                    return Err(file_system::IoErrorKind::InputOutput);
                }
                let count = buffer.len().min(state.write_chunk);
                state.written.extend_from_slice(&buffer[..count]);
                Ok(count)
            })
        }
    }

    #[xila::task::test(task_path = xila::task)]
    async fn fd_read_and_fd_write_vectored_io_semantics() {
        let task_manager = xila::task::test_manager();
        let users_manager = Box::leak(Box::new(xila::users::Manager::new()));
        let time_manager = Box::leak(Box::new(
            xila::time::Manager::new(&testing::TimeDevice).unwrap(),
        ));
        let memory_device = xila::file_system::MemoryDevice::<512>::new_static(1024 * 512);
        let filesystem = xila::little_fs::FileSystem::new_format(
            memory_device,
            xila::little_fs::Options::new(256),
        )
        .unwrap();
        let vfs = Box::leak(Box::new(
            xila::virtual_file_system::initialize(
                task_manager,
                users_manager,
                time_manager,
                filesystem,
            )
            .unwrap(),
        ));
        let task = task_manager.get_current_task_identifier().await;
        xila::virtual_file_system::create_default_hierarchy(vfs, task)
            .await
            .unwrap();
        let context = Box::leak(Box::new(xila::executable::ExecutableContext {
            task_manager,
            users_manager,
            virtual_file_system: vfs,
            graphics_manager: None,
            network_manager: None,
            formats: &[],
            time_manager,
        }));

        let stream_state: &'static std::sync::Mutex<StreamState> =
            Box::leak(Box::new(std::sync::Mutex::new(StreamState {
                read_data: vec![],
                read_offset: 0,
                read_chunk: usize::MAX,
                read_calls: 0,
                fail_read_on_call: 0,
                written: vec![],
                write_chunk: usize::MAX,
                write_calls: 0,
                fail_write_on_call: 0,
            })));
        vfs.mount_character_device(task, Path::from_str("/stream"), StreamDevice(stream_state))
            .await
            .unwrap();

        let rw_file = SynchronousFile::open(
            vfs,
            task,
            Path::from_str("/data.bin"),
            Flags::new(
                AccessFlags::READ_WRITE,
                Some(xila::file_system::CreateFlags::CREATE_TRUNCATE),
                None,
            ),
        )
        .unwrap();
        let ro_file = SynchronousFile::open(
            vfs,
            task,
            Path::from_str("/data.bin"),
            Flags::new(AccessFlags::Read, None, None),
        )
        .unwrap();
        let wo_file = SynchronousFile::open(
            vfs,
            task,
            Path::from_str("/data.bin"),
            Flags::new(AccessFlags::Write, None, None),
        )
        .unwrap();
        let stdin_file = SynchronousFile::open(
            vfs,
            task,
            Path::from_str("/stream"),
            Flags::new(AccessFlags::Read, None, None),
        )
        .unwrap();
        let stdout_file = SynchronousFile::open(
            vfs,
            task,
            Path::from_str("/stream"),
            Flags::new(AccessFlags::Write, None, None),
        )
        .unwrap();
        let stream_rw = SynchronousFile::open(
            vfs,
            task,
            Path::from_str("/stream"),
            Flags::new(AccessFlags::READ_WRITE, None, None),
        )
        .unwrap();
        let directory = SynchronousDirectory::open(vfs, task, Path::from_str("/")).unwrap();

        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        crate::host::wasi::register::add_wasi_to_linker(&mut linker).unwrap();
        let word = if cfg!(feature = "memory_64") {
            "i64"
        } else {
            "i32"
        };
        let wasm = wat::parse_str(alloc::format!(
            r#"(module
                (import "wasi_snapshot_preview1" "fd_read"
                    (func $read (param i32 {word} {word} {word}) (result i32)))
                (import "wasi_snapshot_preview1" "fd_write"
                    (func $write (param i32 {word} {word} {word}) (result i32)))
                (import "wasi_snapshot_preview1" "fd_seek"
                    (func $seek (param i32 i64 i32 {word}) (result i32)))
                (memory (export "memory") 1)
                (func (export "read") (param i32 {word} {word} {word}) (result i32)
                    local.get 0 local.get 1 local.get 2 local.get 3 call $read)
                (func (export "write") (param i32 {word} {word} {word}) (result i32)
                    local.get 0 local.get 1 local.get 2 local.get 3 call $write)
                (func (export "seek") (param i32 i64 i32 {word}) (result i32)
                    local.get 0 local.get 1 local.get 2 local.get 3 call $seek)
            )"#
        ))
        .unwrap();
        let module = Module::new(&engine, &wasm[..]).unwrap();

        let mut wasi = crate::host::wasi::WasiContext {
            files: Default::default(),
            rights: Default::default(),
            next_fd: 9,
            arguments: vec![],
            environment: vec![],
            task,
            prestats: Default::default(),
            exit_code: None,
        };
        wasi.files.insert(
            0,
            FileSystemItem::StandardInput(FileVariant { file: stdin_file }),
        );
        wasi.files.insert(
            1,
            FileSystemItem::StandardOutput(FileVariant { file: stdout_file }),
        );
        wasi.files
            .insert(3, FileSystemItem::File(FileVariant { file: rw_file }));
        wasi.files
            .insert(4, FileSystemItem::File(FileVariant { file: ro_file }));
        wasi.files
            .insert(5, FileSystemItem::File(FileVariant { file: wo_file }));
        wasi.files
            .insert(6, FileSystemItem::File(FileVariant { file: stream_rw }));
        wasi.files.insert(
            7,
            FileSystemItem::Directory(crate::host::wasi::context::DirectoryVariant {
                confinement_root: None,
                path: PathOwned::root(),
                directory,
            }),
        );

        let mut store = Store::new(
            &engine,
            GlobalStore {
                control: Default::default(),
                context,
                pending: None,
                wasi,
                #[cfg(feature = "graphics")]
                graphics: crate::host::graphics::GraphicsState::new(context.graphics_manager),
            },
        );
        let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
        let memory = instance.get_memory(&store, "memory").unwrap();
        let read_fn = instance
            .get_typed_func::<(i32, WasmUsize, WasmUsize, WasmUsize), i32>(&store, "read")
            .unwrap();
        let write_fn = instance
            .get_typed_func::<(i32, WasmUsize, WasmUsize, WasmUsize), i32>(&store, "write")
            .unwrap();
        let seek_fn = instance
            .get_typed_func::<(i32, i64, i32, WasmUsize), i32>(&store, "seek")
            .unwrap();

        let word_bytes = core::mem::size_of::<WasmUsize>();
        let put_iovecs =
            |store: &mut Store<GlobalStore>, table: usize, entries: &[(usize, usize)]| {
                let bytes = memory.data_mut(store);
                for (index, (buf, len)) in entries.iter().enumerate() {
                    let at = table + index * 2 * word_bytes;
                    bytes[at..at + word_bytes].copy_from_slice(&(*buf as WasmUsize).to_le_bytes());
                    bytes[at + word_bytes..at + 2 * word_bytes]
                        .copy_from_slice(&(*len as WasmUsize).to_le_bytes());
                }
            };
        let read_usize = |store: &Store<GlobalStore>, offset: usize| -> usize {
            let mut raw = [0u8; core::mem::size_of::<WasmUsize>()];
            raw.copy_from_slice(&memory.data(store)[offset..offset + word_bytes]);
            WasmUsize::from_le_bytes(raw) as usize
        };

        const TABLE: usize = 512;
        const OUT: usize = 256;
        const SEEK_OUT: usize = 288;

        // 1. Multi-iovec write + zero-length iovecs mixed in, then multi-iovec read.
        memory.data_mut(&mut store)[2000..2003].copy_from_slice(b"hel");
        memory.data_mut(&mut store)[2100..2103].copy_from_slice(b"lo ");
        memory.data_mut(&mut store)[2200..2205].copy_from_slice(b"world");
        put_iovecs(
            &mut store,
            TABLE,
            &[(0, 0), (2000, 3), (100, 0), (2100, 3), (2200, 5), (300, 0)],
        );
        assert_eq!(
            write_fn
                .call(&mut store, (3, TABLE as WasmUsize, 6, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), 11);
        assert_eq!(
            seek_fn
                .call(&mut store, (3, 0, 0, SEEK_OUT as WasmUsize))
                .unwrap(),
            0
        );

        memory.data_mut(&mut store)[3000..3020].fill(0);
        put_iovecs(
            &mut store,
            TABLE,
            &[(0, 0), (3000, 2), (3010, 0), (3004, 4), (3010, 8)],
        );
        assert_eq!(
            read_fn
                .call(&mut store, (3, TABLE as WasmUsize, 5, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), 11);
        assert_eq!(&memory.data(&store)[3000..3002], b"he");
        assert_eq!(&memory.data(&store)[3004..3008], b"llo ");
        assert_eq!(&memory.data(&store)[3010..3018], b"world\0\0\0");

        // 2. More than INLINE_IOVECS (20 entries) write and read.
        let mut many_entries = vec![];
        for i in 0..20usize {
            let addr = 4000 + i * 4;
            memory.data_mut(&mut store)[addr] = b'a' + i as u8;
            many_entries.push((addr, 1));
        }
        assert_eq!(
            seek_fn
                .call(&mut store, (3, 0, 0, SEEK_OUT as WasmUsize))
                .unwrap(),
            0
        );
        put_iovecs(&mut store, TABLE, &many_entries);
        assert_eq!(
            write_fn
                .call(&mut store, (3, TABLE as WasmUsize, 20, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), 20);
        assert_eq!(
            seek_fn
                .call(&mut store, (3, 0, 0, SEEK_OUT as WasmUsize))
                .unwrap(),
            0
        );
        let mut read_many_entries = vec![];
        for i in 0..20usize {
            let addr = 5000 + i * 4;
            memory.data_mut(&mut store)[addr] = 0;
            read_many_entries.push((addr, 1));
        }
        put_iovecs(&mut store, TABLE, &read_many_entries);
        assert_eq!(
            read_fn
                .call(&mut store, (3, TABLE as WasmUsize, 20, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), 20);
        for i in 0..20usize {
            assert_eq!(memory.data(&store)[5000 + i * 4], b'a' + i as u8);
        }

        // 3. Zero-length iovec table (including null iovs pointer) and all-empty iovecs.
        {
            let mut st = stream_state.lock().unwrap();
            st.read_calls = 0;
            st.write_calls = 0;
        }
        assert_eq!(
            read_fn
                .call(&mut store, (6, 0, 0, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), 0);
        assert_eq!(
            write_fn
                .call(&mut store, (6, 0, 0, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), 0);
        put_iovecs(&mut store, TABLE, &[(100, 0), (200, 0)]);
        assert_eq!(
            read_fn
                .call(&mut store, (6, TABLE as WasmUsize, 2, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(
            write_fn
                .call(&mut store, (6, TABLE as WasmUsize, 2, OUT as WasmUsize))
                .unwrap(),
            0
        );
        {
            let st = stream_state.lock().unwrap();
            assert_eq!(st.read_calls, 0);
            assert_eq!(st.write_calls, 0);
        }

        // 4. Short transfers stop at the first short buffer; stdin and stdout work.
        {
            let mut st = stream_state.lock().unwrap();
            st.read_data = b"ABCDEFGH".to_vec();
            st.read_offset = 0;
            st.read_chunk = 3;
            st.read_calls = 0;
            st.fail_read_on_call = 0;
            st.written.clear();
            st.write_chunk = 3;
            st.write_calls = 0;
            st.fail_write_on_call = 0;
        }
        memory.data_mut(&mut store)[6000..6012].fill(0);
        put_iovecs(&mut store, TABLE, &[(6000, 4), (6004, 4), (6008, 4)]);
        assert_eq!(
            read_fn
                .call(&mut store, (0, TABLE as WasmUsize, 3, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), 3);
        assert_eq!(&memory.data(&store)[6000..6012], b"ABC\0\0\0\0\0\0\0\0\0");
        assert_eq!(stream_state.lock().unwrap().read_calls, 1);

        memory.data_mut(&mut store)[6100..6104].copy_from_slice(b"1234");
        memory.data_mut(&mut store)[6200..6204].copy_from_slice(b"5678");
        put_iovecs(&mut store, TABLE, &[(6100, 4), (6200, 4)]);
        assert_eq!(
            write_fn
                .call(&mut store, (1, TABLE as WasmUsize, 2, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), 3);
        {
            let st = stream_state.lock().unwrap();
            assert_eq!(st.write_calls, 1);
            assert_eq!(st.written, b"123");
        }

        // 5. Partial progress before an error reports transferred count; immediate error reports errno.
        {
            let mut st = stream_state.lock().unwrap();
            st.read_data = b"XYZWUV".to_vec();
            st.read_offset = 0;
            st.read_chunk = usize::MAX;
            st.read_calls = 0;
            st.fail_read_on_call = 2;
            st.written.clear();
            st.write_chunk = usize::MAX;
            st.write_calls = 0;
            st.fail_write_on_call = 2;
        }
        memory.data_mut(&mut store)[6000..6008].fill(0);
        put_iovecs(&mut store, TABLE, &[(6000, 4), (6004, 4)]);
        assert_eq!(
            read_fn
                .call(&mut store, (6, TABLE as WasmUsize, 2, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), 4);
        assert_eq!(&memory.data(&store)[6000..6008], b"XYZW\0\0\0\0");
        // Next read hits fail_read_on_call == 2 again if reset to 1: immediate failure returns EIO.
        {
            let mut st = stream_state.lock().unwrap();
            st.read_calls = 0;
            st.fail_read_on_call = 1;
        }
        assert_eq!(
            read_fn
                .call(&mut store, (6, TABLE as WasmUsize, 1, OUT as WasmUsize))
                .unwrap(),
            Error::Io as i32
        );

        put_iovecs(&mut store, TABLE, &[(6100, 4), (6200, 4)]);
        assert_eq!(
            write_fn
                .call(&mut store, (6, TABLE as WasmUsize, 2, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), 4);
        assert_eq!(stream_state.lock().unwrap().written, b"1234");
        {
            let mut st = stream_state.lock().unwrap();
            st.write_calls = 0;
            st.fail_write_on_call = 1;
        }
        assert_eq!(
            write_fn
                .call(&mut store, (6, TABLE as WasmUsize, 1, OUT as WasmUsize))
                .unwrap(),
            Error::Io as i32
        );

        // 6. Self-overwriting iovec table during fd_read uses the upfront snapshot.
        {
            let mut st = stream_state.lock().unwrap();
            st.read_data = alloc::vec![0xAA; 2 * 2 * word_bytes + 4];
            st.read_offset = 0;
            st.read_chunk = usize::MAX;
            st.read_calls = 0;
            st.fail_read_on_call = 0;
        }
        let table_bytes = 2 * 2 * word_bytes;
        memory.data_mut(&mut store)[6300..6304].fill(0);
        put_iovecs(&mut store, TABLE, &[(TABLE, table_bytes), (6300, 4)]);
        assert_eq!(
            read_fn
                .call(&mut store, (6, TABLE as WasmUsize, 2, OUT as WasmUsize))
                .unwrap(),
            0
        );
        assert_eq!(read_usize(&store, OUT), table_bytes + 4);
        assert!(
            memory.data(&store)[TABLE..TABLE + table_bytes]
                .iter()
                .all(|&b| b == 0xAA)
        );
        assert_eq!(&memory.data(&store)[6300..6304], &[0xAA; 4]);

        // 7. Out-of-bounds and overflowing iovecs/output pointers fail with EFAULT before moving any byte.
        {
            let mut st = stream_state.lock().unwrap();
            st.read_data = b"NEVER_TOUCHED".to_vec();
            st.read_offset = 0;
            st.read_calls = 0;
            st.fail_read_on_call = 0;
            st.written.clear();
            st.write_calls = 0;
            st.fail_write_on_call = 0;
        }
        // Invalid output pointers.
        put_iovecs(&mut store, TABLE, &[(6100, 4)]);
        for bad_out in [0usize, 65_536, 65_535] {
            assert_eq!(
                read_fn
                    .call(&mut store, (6, TABLE as WasmUsize, 1, bad_out as WasmUsize))
                    .unwrap(),
                Error::Fault as i32
            );
            assert_eq!(
                write_fn
                    .call(&mut store, (6, TABLE as WasmUsize, 1, bad_out as WasmUsize))
                    .unwrap(),
                Error::Fault as i32
            );
        }
        // Invalid iovec tables.
        for (bad_table, bad_count) in [
            (0 as WasmUsize, 1 as WasmUsize),
            (65_536 - word_bytes as WasmUsize, 1),
            (TABLE as WasmUsize, WasmUsize::MAX),
        ] {
            assert_eq!(
                read_fn
                    .call(&mut store, (6, bad_table, bad_count, OUT as WasmUsize))
                    .unwrap(),
                Error::Fault as i32
            );
            assert_eq!(
                write_fn
                    .call(&mut store, (6, bad_table, bad_count, OUT as WasmUsize))
                    .unwrap(),
                Error::Fault as i32
            );
        }
        // Second iovec buffer out of bounds / null / overflowing: first iovec must not transfer.
        for (bad_buf, bad_len) in [
            (0 as WasmUsize, 4 as WasmUsize),
            (65_536, 4),
            (65_534, 4),
            (100, WasmUsize::MAX),
        ] {
            let bytes = memory.data_mut(&mut store);
            bytes[TABLE..TABLE + word_bytes].copy_from_slice(&(6100 as WasmUsize).to_le_bytes());
            bytes[TABLE + word_bytes..TABLE + 2 * word_bytes]
                .copy_from_slice(&(4 as WasmUsize).to_le_bytes());
            bytes[TABLE + 2 * word_bytes..TABLE + 3 * word_bytes]
                .copy_from_slice(&bad_buf.to_le_bytes());
            bytes[TABLE + 3 * word_bytes..TABLE + 4 * word_bytes]
                .copy_from_slice(&bad_len.to_le_bytes());

            assert_eq!(
                read_fn
                    .call(&mut store, (6, TABLE as WasmUsize, 2, OUT as WasmUsize))
                    .unwrap(),
                Error::Fault as i32
            );
            assert_eq!(
                write_fn
                    .call(&mut store, (6, TABLE as WasmUsize, 2, OUT as WasmUsize))
                    .unwrap(),
                Error::Fault as i32
            );
        }
        {
            let st = stream_state.lock().unwrap();
            assert_eq!(st.read_calls, 0);
            assert_eq!(st.write_calls, 0);
            assert!(st.written.is_empty());
        }

        // 8. Descriptor kind and access errors (including with iovs_len == 0).
        put_iovecs(&mut store, TABLE, &[(6100, 4)]);
        for count in [0 as WasmUsize, 1] {
            assert_eq!(
                read_fn
                    .call(
                        &mut store,
                        (-1, TABLE as WasmUsize, count, OUT as WasmUsize)
                    )
                    .unwrap(),
                Error::Badf as i32
            );
            assert_eq!(
                read_fn
                    .call(
                        &mut store,
                        (99, TABLE as WasmUsize, count, OUT as WasmUsize)
                    )
                    .unwrap(),
                Error::Badf as i32
            );
            assert_eq!(
                read_fn
                    .call(&mut store, (7, TABLE as WasmUsize, count, OUT as WasmUsize))
                    .unwrap(),
                Error::Isdir as i32
            );
            assert_eq!(
                write_fn
                    .call(&mut store, (7, TABLE as WasmUsize, count, OUT as WasmUsize))
                    .unwrap(),
                Error::Isdir as i32
            );
            assert_eq!(
                write_fn
                    .call(&mut store, (4, TABLE as WasmUsize, count, OUT as WasmUsize))
                    .unwrap(),
                Error::Badf as i32
            );
            assert_eq!(
                read_fn
                    .call(&mut store, (5, TABLE as WasmUsize, count, OUT as WasmUsize))
                    .unwrap(),
                Error::Badf as i32
            );
        }
    }
}
