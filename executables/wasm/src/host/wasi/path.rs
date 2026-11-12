use alloc::vec::Vec;
use wasmi::Caller;
use xila::{
    file_system::{AccessFlags, CreateFlags, Flags, Kind, Path, PathOwned, StateFlags, Time},
    task,
};

use crate::{
    define_wasi_module,
    host::{
        store::GlobalStore,
        translation::{GuestPointer, GuestSlice, WasmUsize, borrow_memory, get_memory},
        wasi::{
            Error,
            context::{DirectoryVariant, FileSystemItem, FileVariant},
            error::{WasiResult, vfs_error, wrap_function},
            rights::right,
            time::{realtime_nanoseconds, requested_times},
            types::Filestat,
        },
    },
};

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

fn make_filestat(stats: &xila::file_system::Statistics) -> Filestat {
    Filestat {
        dev: 0,
        ino: stats.inode,
        filetype: kind_to_filetype(&stats.kind),
        nlink: stats.links,
        size: stats.size,
        atime: stats.access.as_u64().saturating_mul(1_000_000_000),
        mtime: stats.modification.as_u64().saturating_mul(1_000_000_000),
        ctime: stats.creation.as_u64().saturating_mul(1_000_000_000),
    }
}

fn copy_guest_path(
    caller: &mut Caller<'_, GlobalStore>,
    ptr: WasmUsize,
    len: WasmUsize,
) -> Result<Vec<u8>, Error> {
    let memory = get_memory(caller).ok_or(Error::Fault)?;
    let guest = borrow_memory(&memory, caller);
    Ok(guest
        .bytes(GuestSlice::new(ptr, len))
        .ok_or(Error::Fault)?
        .to_vec())
}

fn base_path(
    caller: &Caller<'_, GlobalStore>,
    fd: i32,
) -> Result<(xila::task::TaskIdentifier, PathOwned, PathOwned), Error> {
    let item = caller
        .data()
        .wasi
        .files
        .get(&(fd as u32))
        .ok_or(Error::Badf)?;
    let FileSystemItem::Directory(dir) = item else {
        return Err(Error::Notdir);
    };
    Ok((
        caller.data().wasi.task,
        dir.path.clone(),
        dir.confinement_root.as_ref().unwrap_or(&dir.path).clone(),
    ))
}

fn resolve_guest_path(directory: &PathOwned, path: &str) -> Result<PathOwned, Error> {
    if path.split('/').any(|component| component == "..") {
        return Err(Error::Notcapable);
    }
    let relative = path.strip_prefix('/').unwrap_or(path);
    directory
        .clone()
        .join(Path::from_str(relative))
        .ok_or(Error::Inval)
}

fn guest_relative<'a>(directory: &Path, path: &'a Path) -> Result<&'a Path, Error> {
    let relative = path
        .strip_prefix_absolute(directory)
        .ok_or(Error::Notcapable)?;
    Ok(Path::new(relative.as_str().trim_start_matches('/')))
}

fn check_guest_symlink_target(
    directory: &PathOwned,
    link: &PathOwned,
    target: &str,
) -> Result<(), Error> {
    if target.starts_with('/') {
        return Err(Error::Notcapable);
    }

    let link_parent = link.go_parent().ok_or(Error::Inval)?;
    let relative_parent = link_parent
        .strip_prefix_absolute(directory.as_ref())
        .ok_or(Error::Notcapable)?;
    let mut depth = relative_parent
        .get_components()
        .filter(|component| matches!(component, xila::file_system::Component::Normal(_)))
        .count();
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." if depth == 0 => return Err(Error::Notcapable),
            ".." => depth -= 1,
            _ => depth += 1,
        }
    }
    Ok(())
}

/// A guest path confined beneath the preopen of its base directory.
struct Confined {
    task: xila::task::TaskIdentifier,
    /// The preopen the path may never leave.
    root: PathOwned,
    /// The path joined to its base directory.
    path: PathOwned,
    /// The guest path only names its base directory (`.`, `./`, `/`).
    names_base: bool,
}

impl Confined {
    /// The path relative to the preopen, for the `*_beneath` operations.
    fn relative(&self) -> Result<&Path, Error> {
        guest_relative(&self.root, &self.path)
    }
}

/// Check the base directory's right, copy the guest path and confine it.
/// An empty path names nothing (`ENOENT`); `..` is `ENOTCAPABLE` whether or not
/// it would stay inside.
fn confine(
    caller: &mut Caller<'_, GlobalStore>,
    fd: i32,
    ptr: WasmUsize,
    len: WasmUsize,
    needed: u64,
) -> Result<Confined, Error> {
    caller.data().wasi.require_rights(fd, needed)?;
    let bytes = copy_guest_path(caller, ptr, len)?;
    let text = core::str::from_utf8(&bytes).map_err(|_| Error::Ilseq)?;
    if text.is_empty() {
        return Err(Error::Noent);
    }
    let (task, directory, root) = base_path(caller, fd)?;
    Ok(Confined {
        task,
        path: resolve_guest_path(&directory, text)?,
        root,
        names_base: text.split('/').all(|part| part.is_empty() || part == "."),
    })
}

/// Set the timestamps of the directory open as `fd`, by path and beneath its preopen.
pub(super) fn set_directory_times(
    caller: &Caller<'_, GlobalStore>,
    fd: i32,
    access: Option<Time>,
    modification: Option<Time>,
) -> Result<(), Error> {
    let (task, directory, root) = base_path(caller, fd)?;
    let relative = guest_relative(&root, &directory)?;
    let vfs = caller.data().context.virtual_file_system;
    task::block_on(vfs.set_times_beneath(task, &relative, true, &root, access, modification))
        .map_err(vfs_error)
}

/// `lookupflags`: the only defined bit is `SYMLINK_FOLLOW`.
fn follow_symlinks(lookup: i32) -> Result<bool, Error> {
    match lookup {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Inval),
    }
}

define_wasi_module! {
    module: "wasi_snapshot_preview1";

    fn path_remove_directory(caller: Caller<GlobalStore>, fd: i32, path_ptr: WasmUsize, path_len: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            let target = confine(&mut caller, fd, path_ptr, path_len, right::PATH_REMOVE_DIRECTORY)?;
            // `rmdir(".")` is invalid, and the preopen itself is never removable.
            if target.names_base {
                return Err(Error::Inval.into());
            }
            let relative = target.relative()?;
            let vfs = caller.data().context.virtual_file_system;
            let statistics = task::block_on(vfs.get_statistics_beneath(target.task, &relative, false, &target.root)).map_err(vfs_error)?;
            if statistics.kind != Kind::Directory {
                return Err(Error::Notdir.into());
            }
            task::block_on(vfs.remove_beneath(target.task, relative, &target.root)).map_err(vfs_error)?;
            Ok(())
        })
    }

    fn path_rename(caller: Caller<GlobalStore>, fd: i32, path_ptr: WasmUsize, path_len: WasmUsize, new_fd: i32, new_path_ptr: WasmUsize, new_path_len: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            let source = confine(&mut caller, fd, path_ptr, path_len, right::PATH_RENAME_SOURCE)?;
            let destination = confine(&mut caller, new_fd, new_path_ptr, new_path_len, right::PATH_RENAME_TARGET)?;
            // A base directory is in use by its descriptor: it cannot be moved or replaced.
            if source.names_base || destination.names_base {
                return Err(Error::Busy.into());
            }
            let vfs = caller.data().context.virtual_file_system;
            task::block_on(vfs.rename_beneath(source.task, &source.relative()?, &destination.relative()?, &source.root, &destination.root)).map_err(vfs_error)?;
            Ok(())
        })
    }

    // Xila's file systems have no hard links (LittleFS cannot represent
    // them), so after the paths are confined and checked a valid request is
    // ENOTSUP rather than a copy that would silently diverge.
    fn path_link(caller: Caller<GlobalStore>, fd: i32, lookup: i32, path_ptr: WasmUsize, path_len: WasmUsize, new_fd: i32, new_path_ptr: WasmUsize, new_path_len: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            let follow = follow_symlinks(lookup)?;
            let source = confine(&mut caller, fd, path_ptr, path_len, right::PATH_LINK_SOURCE)?;
            let destination = confine(&mut caller, new_fd, new_path_ptr, new_path_len, right::PATH_LINK_TARGET)?;
            let vfs = caller.data().context.virtual_file_system;
            task::block_on(vfs.link_beneath(source.task, &source.relative()?, follow, &destination.relative()?, &source.root, &destination.root)).map_err(|error| match error {
                // Linking a directory is EPERM, not the EISDIR a read of one reports.
                xila::virtual_file_system::Error::FileSystem(xila::file_system::Error::IsDirectory) => Error::Perm,
                error => vfs_error(error),
            })?;
            Ok(())
        })
    }

    fn path_filestat_set_times(caller: Caller<GlobalStore>, fd: i32, lookup: i32, path_ptr: WasmUsize, path_len: WasmUsize, atim: i64, mtim: i64, flags: i32) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let mut caller = caller;
            let follow = follow_symlinks(lookup)?;
            let (access, modification) = requested_times(atim as u64, mtim as u64, flags, || realtime_nanoseconds(&caller))?;
            let target = confine(&mut caller, fd, path_ptr, path_len, right::PATH_FILESTAT_SET_TIMES)?;
            let vfs = caller.data().context.virtual_file_system;
            let relative = target.relative()?;
            if access.is_none() && modification.is_none() {
                // Nothing to change, but the path must still name something.
                task::block_on(vfs.get_statistics_beneath(target.task, &relative, follow, &target.root)).map_err(vfs_error)?;
                return Ok(());
            }
            task::block_on(vfs.set_times_beneath(target.task, &relative, follow, &target.root, access, modification)).map_err(vfs_error)?;
            Ok(())
        })
    }

    fn path_create_directory(caller: Caller<GlobalStore>, fd: i32, path_ptr: WasmUsize, path_len: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::PATH_CREATE_DIRECTORY)?;
            let mut caller = caller;
            let path_bytes = copy_guest_path(&mut caller, path_ptr, path_len)?;
            let (task_id, directory, root) = base_path(&caller, fd)?;
            let relative = core::str::from_utf8(&path_bytes).map_err(|_| Error::Ilseq)?;
            let absolute_path = resolve_guest_path(&directory, relative)?;
            task::block_on(caller.data().context.virtual_file_system.create_directory_beneath(task_id, &guest_relative(&root, &absolute_path)?, &root)).map_err(vfs_error)?;
            Ok(())
        })
    }

    fn path_symlink(caller: Caller<GlobalStore>, old_path_ptr: WasmUsize, old_path_len: WasmUsize, fd: i32, new_path_ptr: WasmUsize, new_path_len: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::PATH_SYMLINK)?;
            let mut caller = caller;
            let target = copy_guest_path(&mut caller, old_path_ptr, old_path_len)?;
            let link = copy_guest_path(&mut caller, new_path_ptr, new_path_len)?;
            let (task_id, directory, root) = base_path(&caller, fd)?;
            let target = core::str::from_utf8(&target).map_err(|_| Error::Ilseq)?;
            let link = core::str::from_utf8(&link).map_err(|_| Error::Ilseq)?;
            let link = resolve_guest_path(&directory, link)?;
            check_guest_symlink_target(&root, &link, target)?;
            task::block_on(caller.data().context.virtual_file_system.create_symlink_beneath(task_id, Path::from_str(target), guest_relative(&root, &link)?, &root)).map_err(vfs_error)?;
            Ok(())
        })
    }

    fn path_readlink(caller: Caller<GlobalStore>, fd: i32, path_ptr: WasmUsize, path_len: WasmUsize, buf_ptr: WasmUsize, buf_len: WasmUsize, used_ptr: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::PATH_READLINK)?;
            let mut caller = caller;
            let path_bytes = copy_guest_path(&mut caller, path_ptr, path_len)?;
            let (task_id, directory, root) = base_path(&caller, fd)?;
            let relative = core::str::from_utf8(&path_bytes).map_err(|_| Error::Ilseq)?;
            let absolute_path = resolve_guest_path(&directory, relative)?;
            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            let output = GuestSlice::new(buf_ptr, buf_len);
            let used = GuestPointer::<WasmUsize>::new(used_ptr);
            let mut target = alloc::vec![0; 4096];
            let length = task::block_on(caller.data().context.virtual_file_system.read_symlink_beneath(task_id, guest_relative(&root, &absolute_path)?, &mut target, &root)).map_err(vfs_error)?;
            let mut guest = borrow_memory(&memory, &mut caller);
            guest.validate_pointer(used).ok_or(Error::Fault)?;
            let copied = core::cmp::min(length, buf_len as usize);
            guest.bytes_mut(output).ok_or(Error::Fault)?[..copied].copy_from_slice(&target[..copied]);
            guest.write(used, copied as WasmUsize).ok_or(Error::Fault)?;
            Ok(())
        })
    }

    fn path_unlink_file(caller: Caller<GlobalStore>, fd: i32, path_ptr: WasmUsize, path_len: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::PATH_UNLINK_FILE)?;
            let mut caller = caller;
            let path_bytes = copy_guest_path(&mut caller, path_ptr, path_len)?;
            let (task_id, directory, root) = base_path(&caller, fd)?;
            let relative = core::str::from_utf8(&path_bytes).map_err(|_| Error::Ilseq)?;
            let absolute_path = resolve_guest_path(&directory, relative)?;
            let vfs = caller.data().context.virtual_file_system;
            let statistics = task::block_on(vfs.get_statistics_beneath(task_id, &guest_relative(&root, &absolute_path)?, false, &root)).map_err(vfs_error)?;
            if statistics.kind == xila::file_system::Kind::Directory {
                return Err(Error::Isdir.into());
            }
            task::block_on(vfs.remove_beneath(task_id, guest_relative(&root, &absolute_path)?, &root)).map_err(vfs_error)?;
            Ok(())
        })
    }

    fn path_open(caller: Caller<GlobalStore>, fd: i32, _dirflags: i32, path_ptr: WasmUsize, path_len: WasmUsize, oflags: i32, rights_base: i64, rights_inheriting: i64, fdflags: i32, opened_fd_ptr: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            let wasi = &caller.data().wasi;
            wasi.require_rights(fd, right::PATH_OPEN)?;
            if oflags & 1 != 0 {
                wasi.require_rights(fd, right::PATH_CREATE_FILE)?;
            }
            if oflags & 8 != 0 {
                wasi.require_rights(fd, right::PATH_FILESTAT_SET_SIZE)?;
            }
            // Only a directory whose rights were reduced limits what it hands out.
            let reduced = wasi.rights.contains_key(&(fd as u32));
            if reduced && (rights_base as u64 | rights_inheriting as u64) & !wasi.rights_of(fd as u32).1 != 0 {
                return Err(Error::Notcapable.into());
            }
            let mut caller = caller;
            let path_bytes = copy_guest_path(&mut caller, path_ptr, path_len)?;
            let (task_id, directory, root) = base_path(&caller, fd)?;
            let relative = core::str::from_utf8(&path_bytes).map_err(|_| Error::Ilseq)?;
            let absolute_path = resolve_guest_path(&directory, relative)?;
            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            let opened_fd_out = GuestPointer::<i32>::new(opened_fd_ptr);
            borrow_memory(&memory, &mut caller).validate_pointer(opened_fd_out).ok_or(Error::Fault)?;
            let read = (rights_base & 2) != 0;
            let write = (rights_base & 64) != 0;
            let access = match (read, write) { (true, true) => AccessFlags::READ_WRITE, (true, false) => AccessFlags::Read, (false, true) => AccessFlags::Write, (false, false) => AccessFlags::Read };
            let create = if oflags & 1 != 0 { Some(if oflags & 4 != 0 { CreateFlags::CREATE_EXCLUSIVE } else if oflags & 8 != 0 { CreateFlags::CREATE_TRUNCATE } else { CreateFlags::Create }) } else { None };
            let state = if fdflags & 1 != 0 { Some(StateFlags::Append) } else { None };
            let vfs = caller.data().context.virtual_file_system;
            let item = if oflags & 2 != 0 {
                let directory_handle = task::block_on(vfs.open_directory_beneath(task_id, &guest_relative(&root, &absolute_path)?, &root)).map(|dir| dir.into_synchronous_directory()).map_err(vfs_error)?;
                FileSystemItem::Directory(DirectoryVariant { confinement_root: Some(root), directory: directory_handle, path: absolute_path })
            } else {
                let file = task::block_on(vfs.open_beneath(&guest_relative(&root, &absolute_path)?, Flags::new(access, create, state), task_id, &root)).map(|file| file.into_synchronous_file()).map_err(vfs_error)?;
                FileSystemItem::File(FileVariant { file })
            };
            let new_fd = caller.data_mut().wasi.insert_file_system_item(item).ok_or(Error::Mfile)?;
            if reduced {
                caller.data_mut().wasi.rights.insert(new_fd, (rights_base as u64, rights_inheriting as u64));
            }
            borrow_memory(&memory, &mut caller)
                .write(opened_fd_out, i32::try_from(new_fd).map_err(|_| Error::Overflow)?).ok_or(Error::Fault)?;
            Ok(())
        })
    }

    fn path_filestat_get(caller: Caller<GlobalStore>, fd: i32, flags: i32, path_ptr: WasmUsize, path_len: WasmUsize, buf_ptr: WasmUsize) -> Result<WasiResult, wasmi::Error> {
        wrap_function!({
            caller.data().wasi.require_rights(fd, right::PATH_FILESTAT_GET)?;
            let mut caller = caller;
            let path_bytes = copy_guest_path(&mut caller, path_ptr, path_len)?;
            let (task_id, directory, root) = base_path(&caller, fd)?;
            let relative = core::str::from_utf8(&path_bytes).map_err(|_| Error::Ilseq)?;
            let absolute_path = resolve_guest_path(&directory, relative)?;
            let vfs = caller.data().context.virtual_file_system;
            let stats = if flags & 1 != 0 {
                task::block_on(vfs.get_statistics_beneath(task_id, &guest_relative(&root, &absolute_path)?, true, &root))
            } else {
                task::block_on(vfs.get_statistics_beneath(task_id, &guest_relative(&root, &absolute_path)?, false, &root))
            }.map_err(vfs_error)?;
            let filestat = make_filestat(&stats);
            let memory = get_memory(&caller).ok_or(Error::Fault)?;
            borrow_memory(&memory, &mut caller).write(GuestPointer::new(buf_ptr), filestat).ok_or(Error::Fault)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::{Error, check_guest_symlink_target, resolve_guest_path};
    use xila::file_system::PathOwned;

    #[test]
    fn absolute_guest_paths_are_root_relative_to_the_preopen() {
        let root = PathOwned::root();
        let path = resolve_guest_path(&root, "/test.txt").unwrap();
        assert_eq!(<PathOwned as AsRef<str>>::as_ref(&path), "/test.txt");
    }

    #[test]
    fn parent_components_cannot_escape_the_preopen() {
        let root = PathOwned::root();
        assert!(matches!(
            resolve_guest_path(&root, "../outside"),
            Err(Error::Notcapable)
        ));
        assert!(matches!(
            resolve_guest_path(&root, "nested/../../outside"),
            Err(Error::Notcapable)
        ));
    }

    #[test]
    fn symlink_targets_are_confined_to_the_preopen() {
        let root = PathOwned::try_from("/sandbox").unwrap();
        let link = resolve_guest_path(&root, "nested/link").unwrap();
        assert_eq!(
            check_guest_symlink_target(&root, &link, "/secret"),
            Err(Error::Notcapable)
        );
        assert_eq!(
            check_guest_symlink_target(&root, &link, "../../outside"),
            Err(Error::Notcapable)
        );
        assert_eq!(
            check_guest_symlink_target(&root, &link, "../inside"),
            Ok(())
        );
        assert_eq!(
            resolve_guest_path(&root, "/outside").unwrap().as_str(),
            "/sandbox/outside"
        );
    }
    #[xila::task::test(task_path = xila::task)]
    async fn guests_cannot_follow_or_create_escaping_links() {
        use super::*;
        use crate::host::wasi::{DirectoryVariant, FileSystemItem, WasiContext};
        use alloc::{boxed::Box, vec};
        use wasmi::{Engine, Linker, Module, Store};
        let task_manager = xila::task::test_manager();
        let users_manager = Box::leak(Box::new(xila::users::Manager::new()));
        let time_manager = Box::leak(Box::new(
            xila::time::Manager::new(&testing::TimeDevice).unwrap(),
        ));
        let device = xila::file_system::MemoryDevice::<512>::new_static(1024 * 512);
        let fs = xila::little_fs::FileSystem::new_format(device, 256).unwrap();
        let vfs = Box::leak(Box::new(
            xila::virtual_file_system::initialize(task_manager, users_manager, time_manager, fs)
                .unwrap(),
        ));
        let task = task_manager.get_current_task_identifier().await;
        for path in ["/sandbox", "/sandbox/sub", "/secret"] {
            vfs.create_directory(task, &path).await.unwrap();
        }
        for (name, target) in [
            ("absolute", "/secret"),
            ("parent", "../secret"),
            ("inside", "sub"),
        ] {
            let link = alloc::format!("/sandbox/{name}");
            vfs.create_symlink(task, Path::new(target), Path::new(&link))
                .await
                .unwrap();
        }
        let context = Box::leak(Box::new(xila::executable::ExecutableContext {
            task_manager,
            users_manager,
            virtual_file_system: vfs,
            graphics_manager: None,
            network_manager: None,
            time_manager,
        }));
        let mut wasi = WasiContext {
            files: Default::default(),
            rights: Default::default(),
            next_fd: 4,
            arguments: vec![],
            environment: vec![],
            task,
            prestats: Default::default(),
            exit_code: None,
        };
        wasi.files.insert(
            3,
            FileSystemItem::Directory(DirectoryVariant {
                confinement_root: None,
                path: PathOwned::try_from("/sandbox").unwrap(),
                directory: xila::virtual_file_system::SynchronousDirectory::open(
                    vfs, task, "/sandbox",
                )
                .unwrap(),
            }),
        );
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        crate::host::wasi::register::add_wasi_to_linker(&mut linker).unwrap();
        let word = if cfg!(feature = "memory_64") {
            "i64"
        } else {
            "i32"
        };
        let bytes = wat::parse_str(alloc::format!(r#"(module
            (import "wasi_snapshot_preview1" "path_filestat_get" (func $stat (param i32 i32 {word} {word} {word}) (result i32)))
            (import "wasi_snapshot_preview1" "path_open" (func $open (param i32 i32 {word} {word} i32 i64 i64 i32 {word}) (result i32)))
            (import "wasi_snapshot_preview1" "path_symlink" (func $link (param {word} {word} i32 {word} {word}) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 1) "absoluteparentinside/secretnewsubgood.")
            (func (export "stat") (param {word} {word} i32) (result i32)
                i32.const 3 local.get 2 local.get 0 local.get 1 {word}.const 128 call $stat)
            (func (export "link") (param {word} {word} {word} {word}) (result i32)
                local.get 0 local.get 1 i32.const 3 local.get 2 local.get 3 call $link)
            (func (export "open") (param {word} {word}) (result i32)
                i32.const 3 i32.const 1 local.get 0 local.get 1 i32.const 2 i64.const 2 i64.const 0 i32.const 0 {word}.const 256 call $open)
            (func (export "nested_stat") (result i32)
                i32.const 256 i32.load i32.const 1 {word}.const 38 {word}.const 1 {word}.const 128 call $stat)
        )"#)).unwrap();
        let module = Module::new(&engine, &bytes[..]).unwrap();
        let mut store = Store::new(
            &engine,
            GlobalStore {
                context,
                #[cfg(feature = "graphics")]
                graphics: crate::host::graphics::GraphicsState::new(None),
                pending: None,
                wasi,
            },
        );
        let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
        let stat = instance
            .get_typed_func::<(WasmUsize, WasmUsize, i32), i32>(&store, "stat")
            .unwrap();
        assert_eq!(stat.call(&mut store, (1, 8, 1)).unwrap(), 76);
        assert_eq!(stat.call(&mut store, (9, 6, 1)).unwrap(), 76);
        assert_eq!(stat.call(&mut store, (1, 8, 0)).unwrap(), 0);
        assert_eq!(stat.call(&mut store, (15, 6, 1)).unwrap(), 0);
        let link = instance
            .get_typed_func::<(WasmUsize, WasmUsize, WasmUsize, WasmUsize), i32>(&store, "link")
            .unwrap();
        assert_eq!(link.call(&mut store, (21, 7, 28, 3)).unwrap(), 76);
        assert_eq!(link.call(&mut store, (31, 3, 34, 4)).unwrap(), 0);
        assert_eq!(stat.call(&mut store, (34, 4, 1)).unwrap(), 0);
        let open = instance
            .get_typed_func::<(WasmUsize, WasmUsize), i32>(&store, "open")
            .unwrap();
        assert_eq!(open.call(&mut store, (1, 8)).unwrap(), 76);
        assert_eq!(open.call(&mut store, (15, 6)).unwrap(), 0);
        // A guest-opened directory must never become a fresh ambient root.
        vfs.remove(task, "/sandbox/inside").await.unwrap();
        vfs.create_symlink(task, Path::new("/secret"), Path::new("/sandbox/inside"))
            .await
            .unwrap();
        let nested_stat = instance
            .get_typed_func::<(), i32>(&store, "nested_stat")
            .unwrap();
        assert_eq!(nested_stat.call(&mut store, ()).unwrap(), 76);
        assert!(
            vfs.get_link_statistics_for_task(task, &"/sandbox/new")
                .await
                .is_err()
        );
    }
}
