//! Behavioural tests of the Preview 1 imports, driven through a guest that
//! imports all 46 functions (`all_imports.wat`). Each test builds a sandboxed
//! preopen at `/sandbox` (descriptor 3) next to a `/outside` directory the guest
//! must never reach.

extern crate std;

use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec::Vec,
};
use wasmi::{Engine, Instance, Linker, Module, Store, Val};
use xila::{
    file_system::{AccessFlags, Flags, PathOwned},
    task::TaskIdentifier,
    virtual_file_system::{File, SynchronousDirectory, SynchronousFile, VirtualFileSystem},
};

use crate::host::{
    store::GlobalStore,
    wasi::{DirectoryVariant, Error, FileSystemItem, FileVariant, Prestat, WasiContext},
};

const WAT: &str = include_str!("all_imports.wat");
const ROOT: i32 = 3;

/// The 46 functions of `wasi_snapshot_preview1`.
const PREVIEW_ONE: [&str; 46] = [
    "args_get",
    "args_sizes_get",
    "clock_res_get",
    "clock_time_get",
    "environ_get",
    "environ_sizes_get",
    "fd_advise",
    "fd_allocate",
    "fd_close",
    "fd_datasync",
    "fd_fdstat_get",
    "fd_fdstat_set_flags",
    "fd_fdstat_set_rights",
    "fd_filestat_get",
    "fd_filestat_set_size",
    "fd_filestat_set_times",
    "fd_pread",
    "fd_prestat_dir_name",
    "fd_prestat_get",
    "fd_pwrite",
    "fd_read",
    "fd_readdir",
    "fd_renumber",
    "fd_seek",
    "fd_sync",
    "fd_tell",
    "fd_write",
    "path_create_directory",
    "path_filestat_get",
    "path_filestat_set_times",
    "path_link",
    "path_open",
    "path_readlink",
    "path_remove_directory",
    "path_rename",
    "path_symlink",
    "path_unlink_file",
    "poll_oneoff",
    "proc_exit",
    "proc_raise",
    "random_get",
    "sched_yield",
    "sock_accept",
    "sock_recv",
    "sock_send",
    "sock_shutdown",
];

fn errno(error: Error) -> i32 {
    i32::from(error)
}
fn i(value: i32) -> Val {
    Val::I32(value)
}
fn l(value: i64) -> Val {
    Val::I64(value)
}

struct Guest {
    store: Store<GlobalStore>,
    instance: Instance,
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    clock: &'static xila::time::Manager<'static>,
}

impl Guest {
    async fn new() -> Self {
        let task_manager = xila::task::test_manager();
        let users_manager = Box::leak(Box::new(xila::users::Manager::new()));
        let clock = Box::leak(Box::new(
            xila::time::Manager::new(&testing::TimeDevice).unwrap(),
        ));
        let device = xila::file_system::MemoryDevice::<512>::new_static(1024 * 512);
        let file_system = xila::little_fs::FileSystem::new_format(device, 256).unwrap();
        let vfs: &'static VirtualFileSystem = Box::leak(Box::new(
            xila::virtual_file_system::initialize(task_manager, users_manager, clock, file_system)
                .unwrap(),
        ));
        let task = task_manager.get_current_task_identifier().await;
        for path in ["/sandbox", "/outside"] {
            vfs.create_directory(task, &path).await.unwrap();
        }
        File::write_to_path(vfs, task, "/outside/secret", b"secret")
            .await
            .unwrap();
        let context = Box::leak(Box::new(xila::executable::ExecutableContext {
            task_manager,
            users_manager,
            virtual_file_system: vfs,
            graphics_manager: None,
            network_manager: None,
            time_manager: clock,
        }));

        let mut wasi = WasiContext {
            files: Default::default(),
            rights: Default::default(),
            next_fd: 4,
            arguments: Default::default(),
            environment: Default::default(),
            task,
            prestats: Default::default(),
            exit_code: None,
        };
        wasi.files.insert(
            ROOT as u32,
            FileSystemItem::Directory(DirectoryVariant {
                confinement_root: None,
                path: PathOwned::try_from("/sandbox").unwrap(),
                directory: SynchronousDirectory::open(vfs, task, "/sandbox").unwrap(),
            }),
        );
        wasi.prestats.insert(
            ROOT as u32,
            Prestat {
                name: b"/".to_vec(),
            },
        );
        let standard_output = SynchronousFile::open(
            vfs,
            task,
            "/outside/secret",
            Flags::new(AccessFlags::READ_WRITE, None, None),
        )
        .unwrap();
        wasi.files.insert(
            1,
            FileSystemItem::StandardOutput(FileVariant {
                file: standard_output,
            }),
        );

        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        crate::host::wasi::register::add_wasi_to_linker(&mut linker).unwrap();
        let module = Module::new(&engine, wat::parse_str(WAT).unwrap().as_slice()).unwrap();
        let mut store = Store::new(
            &engine,
            GlobalStore {
                control: Default::default(),
                context,
                #[cfg(feature = "graphics")]
                graphics: crate::host::graphics::GraphicsState::new(None),
                pending: None,
                wasi,
            },
        );
        let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
        Self {
            store,
            instance,
            vfs,
            task,
            clock,
        }
    }

    fn try_call(&mut self, name: &str, arguments: &[Val]) -> Result<i32, wasmi::Error> {
        let function = self.instance.get_func(&self.store, name).unwrap();
        let mut result = [Val::I32(0)];
        let results = if function.ty(&self.store).results().is_empty() {
            &mut result[..0]
        } else {
            &mut result[..]
        };
        function.call(&mut self.store, arguments, results)?;
        Ok(result[0].i32().unwrap_or(0))
    }

    fn call(&mut self, name: &str, arguments: &[Val]) -> i32 {
        self.try_call(name, arguments).unwrap()
    }

    fn memory(&self) -> wasmi::Memory {
        self.instance.get_memory(&self.store, "memory").unwrap()
    }

    fn put(&mut self, address: usize, bytes: &[u8]) {
        let memory = self.memory();
        memory.data_mut(&mut self.store)[address..address + bytes.len()].copy_from_slice(bytes);
    }

    fn get(&self, address: usize, length: usize) -> Vec<u8> {
        self.memory().data(&self.store)[address..address + length].to_vec()
    }

    fn u64_at(&self, address: usize) -> u64 {
        u64::from_le_bytes(self.get(address, 8).try_into().unwrap())
    }

    fn u32_at(&self, address: usize) -> u32 {
        u32::from_le_bytes(self.get(address, 4).try_into().unwrap())
    }

    /// Write a path into guest memory and return its (pointer, length) arguments.
    fn text(&mut self, slot: usize, text: &str) -> [Val; 2] {
        let address = 4096 + slot * 256;
        self.put(address, text.as_bytes());
        [i(address as i32), i(text.len() as i32)]
    }

    fn with_path(&mut self, name: &str, directory: i32, path: &str) -> i32 {
        let [pointer, length] = self.text(0, path);
        self.call(name, &[i(directory), pointer, length])
    }

    /// `path_open` below the preopen; the descriptor or the errno.
    fn open_with(&mut self, path: &str, oflags: i32, rights: i64) -> Result<i32, i32> {
        self.sync_all();
        let [pointer, length] = self.text(0, path);
        let status = self.call(
            "path_open",
            &[
                i(ROOT),
                i(1),
                pointer,
                length,
                i(oflags),
                l(rights),
                l(0),
                i(0),
                i(8000),
            ],
        );
        match status {
            0 => Ok(self.u32_at(8000) as i32),
            error => Err(error),
        }
    }

    /// Open (and create) a file for reading and writing.
    fn create(&mut self, path: &str) -> i32 {
        self.open_with(path, 1, 2 | 64).unwrap()
    }

    fn open(&mut self, path: &str, rights: i64) -> Result<i32, i32> {
        self.open_with(path, 0, rights)
    }

    /// Put `bytes` at the start of a new file and return its descriptor.
    fn file_with(&mut self, path: &str, bytes: &[u8]) -> i32 {
        let fd = self.create(path);
        self.put(1200, bytes);
        self.put(1024, &1200u32.to_le_bytes());
        self.put(1028, &(bytes.len() as u32).to_le_bytes());
        assert_eq!(
            self.call("fd_pwrite", &[i(fd), i(1024), i(1), l(0), i(1100)]),
            0
        );
        fd
    }

    fn pread(&mut self, fd: i32, offset: i64, length: usize) -> Result<Vec<u8>, i32> {
        self.put(1024, &1500u32.to_le_bytes());
        self.put(1028, &(length as u32).to_le_bytes());
        match self.call("fd_pread", &[i(fd), i(1024), i(1), l(offset), i(1100)]) {
            0 => Ok(self.get(1500, self.u32_at(1100) as usize)),
            error => Err(error),
        }
    }

    fn pwrite(&mut self, fd: i32, offset: i64, bytes: &[u8]) -> Result<usize, i32> {
        self.put(1500, bytes);
        self.put(1024, &1500u32.to_le_bytes());
        self.put(1028, &(bytes.len() as u32).to_le_bytes());
        match self.call("fd_pwrite", &[i(fd), i(1024), i(1), l(offset), i(1100)]) {
            0 => Ok(self.u32_at(1100) as usize),
            error => Err(error),
        }
    }

    fn tell(&mut self, fd: i32) -> Result<u64, i32> {
        match self.call("fd_tell", &[i(fd), i(1100)]) {
            0 => Ok(self.u64_at(1100)),
            error => Err(error),
        }
    }

    fn rename(&mut self, from: &str, to: &str) -> i32 {
        let [from_pointer, from_length] = self.text(0, from);
        let [to_pointer, to_length] = self.text(1, to);
        self.call(
            "path_rename",
            &[
                i(ROOT),
                from_pointer,
                from_length,
                i(ROOT),
                to_pointer,
                to_length,
            ],
        )
    }

    fn link(&mut self, lookup: i32, from: &str, to: &str) -> i32 {
        let [from_pointer, from_length] = self.text(0, from);
        let [to_pointer, to_length] = self.text(1, to);
        self.call(
            "path_link",
            &[
                i(ROOT),
                i(lookup),
                from_pointer,
                from_length,
                i(ROOT),
                to_pointer,
                to_length,
            ],
        )
    }

    fn symlink(&mut self, target: &str, link: &str) {
        let [target_pointer, target_length] = self.text(0, target);
        let [link_pointer, link_length] = self.text(1, link);
        assert_eq!(
            self.call(
                "path_symlink",
                &[
                    target_pointer,
                    target_length,
                    i(ROOT),
                    link_pointer,
                    link_length
                ]
            ),
            0
        );
    }

    fn path_set_times(
        &mut self,
        lookup: i32,
        path: &str,
        atime: i64,
        mtime: i64,
        flags: i32,
    ) -> i32 {
        let [pointer, length] = self.text(0, path);
        self.call(
            "path_filestat_set_times",
            &[
                i(ROOT),
                i(lookup),
                pointer,
                length,
                l(atime),
                l(mtime),
                i(flags),
            ],
        )
    }

    /// LittleFS keeps written data in the handle until it is synchronised;
    /// make every open file visible to path-based inspection.
    fn sync_all(&mut self) {
        for item in self.store.data_mut().wasi.files.values_mut() {
            if let FileSystemItem::File(FileVariant { file }) = item {
                file.flush().unwrap();
            }
        }
    }

    /// Size, access and modification seconds of a path of the sandbox.
    async fn stat(&mut self, path: &str) -> Option<(u64, u64, u64)> {
        self.sync_all();
        let path = format!("/sandbox/{path}");
        self.vfs
            .get_statistics_for_task(self.task, &path.as_str())
            .await
            .ok()
            .map(|s| (s.size, s.access.as_u64(), s.modification.as_u64()))
    }

    async fn exists(&mut self, path: &str) -> bool {
        self.stat(path).await.is_some()
    }

    async fn contents(&mut self, path: &str) -> Vec<u8> {
        let size = self.stat(path).await.unwrap().0 as usize;
        let path = format!("/sandbox/{path}");
        let mut bytes = alloc::vec![0; size];
        File::read_slice_from_path(self.vfs, self.task, path.as_str(), &mut bytes)
            .await
            .unwrap();
        bytes
    }

    /// Size of an open file as its own descriptor reports it.
    fn size_of(&mut self, fd: i32) -> u64 {
        assert_eq!(self.call("fd_filestat_get", &[i(fd), i(1300)]), 0);
        self.u64_at(1300 + 32)
    }

    fn fdstat(&mut self, fd: i32) -> Result<(u8, u16, u64, u64), i32> {
        match self.call("fd_fdstat_get", &[i(fd), i(1100)]) {
            0 => Ok((
                self.get(1100, 1)[0],
                u16::from_le_bytes(self.get(1102, 2).try_into().unwrap()),
                self.u64_at(1108),
                self.u64_at(1116),
            )),
            error => Err(error),
        }
    }
}

const READ: i64 = 1 << 1;
const WRITE: i64 = 1 << 6;
const SEEK: i64 = 1 << 2;

#[xila::task::test(task_path = xila::task)]
async fn the_guest_imports_every_preview_one_function_and_each_one_is_provided() {
    let engine = Engine::default();
    let module = Module::new(&engine, wat::parse_str(WAT).unwrap().as_slice()).unwrap();
    let mut imported: Vec<String> = module
        .imports()
        .map(|import| {
            assert_eq!(import.module(), "wasi_snapshot_preview1");
            assert!(import.ty().func().is_some());
            import.name().into()
        })
        .collect();
    imported.sort();
    let mut expected: Vec<String> = PREVIEW_ONE.iter().map(|name| (*name).into()).collect();
    expected.sort();
    assert_eq!(imported, expected);
    // Instantiation fails on the first import the host does not define.
    Guest::new().await;
}

#[xila::task::test(task_path = xila::task)]
async fn a_real_guest_creates_resizes_renames_and_removes() {
    let mut guest = Guest::new().await;
    assert_eq!(guest.call("scenario", &[]), 0);
    assert!(!guest.exists("d").await);
    assert!(!guest.exists("a.txt").await);
}

#[xila::task::test(task_path = xila::task)]
async fn fd_advise_validates_and_otherwise_succeeds() {
    let mut guest = Guest::new().await;
    let fd = guest.create("f");
    for advice in 0..6 {
        assert_eq!(guest.call("fd_advise", &[i(fd), l(0), l(10), i(advice)]), 0);
    }
    assert_eq!(
        guest.call("fd_advise", &[i(fd), l(0), l(0), i(6)]),
        errno(Error::Inval)
    );
    assert_eq!(
        guest.call("fd_advise", &[i(fd), l(0), l(0), i(-1)]),
        errno(Error::Inval)
    );
    assert_eq!(
        guest.call("fd_advise", &[i(99), l(0), l(0), i(0)]),
        errno(Error::Badf)
    );
    assert_eq!(
        guest.call("fd_advise", &[i(ROOT), l(0), l(0), i(0)]),
        errno(Error::Isdir)
    );
}

#[xila::task::test(task_path = xila::task)]
async fn fd_allocate_grows_but_never_shrinks() {
    let mut guest = Guest::new().await;
    let fd = guest.file_with("f", b"0123456789");
    assert_eq!(guest.call("fd_allocate", &[i(fd), l(0), l(4)]), 0);
    assert_eq!(guest.stat("f").await.unwrap().0, 10);
    assert_eq!(guest.call("fd_allocate", &[i(fd), l(8), l(8)]), 0);
    assert_eq!(guest.stat("f").await.unwrap().0, 16);
    // The new bytes read as zero, and the old ones are untouched.
    assert_eq!(guest.pread(fd, 8, 8).unwrap(), b"89\0\0\0\0\0\0");
    assert_eq!(
        guest.call("fd_allocate", &[i(fd), l(0), l(0)]),
        errno(Error::Inval)
    );
    // 0xffff_ffff_ffff_ffff + 2 overflows the 64-bit file size.
    assert_eq!(
        guest.call("fd_allocate", &[i(fd), l(-1), l(2)]),
        errno(Error::Fbig)
    );
    assert_eq!(
        guest.call("fd_allocate", &[i(ROOT), l(0), l(1)]),
        errno(Error::Isdir)
    );
    assert_eq!(
        guest.call("fd_allocate", &[i(99), l(0), l(1)]),
        errno(Error::Badf)
    );
    let read_only = guest.open("f", READ).unwrap();
    assert_eq!(
        guest.call("fd_allocate", &[i(read_only), l(0), l(100)]),
        errno(Error::Badf)
    );
    assert_eq!(guest.stat("f").await.unwrap().0, 16);
}

#[xila::task::test(task_path = xila::task)]
async fn fd_sync_and_fd_datasync_flush_files_and_accept_directories() {
    let mut guest = Guest::new().await;
    let fd = guest.file_with("f", b"data");
    for name in ["fd_sync", "fd_datasync"] {
        assert_eq!(guest.call(name, &[i(fd)]), 0, "{name}");
        assert_eq!(guest.call(name, &[i(ROOT)]), 0, "{name} on a directory");
        assert_eq!(guest.call(name, &[i(99)]), errno(Error::Badf), "{name}");
        assert_eq!(guest.call(name, &[i(-1)]), errno(Error::Badf), "{name}");
    }
}

#[xila::task::test(task_path = xila::task)]
async fn positioned_io_never_touches_the_cursor() {
    let mut guest = Guest::new().await;
    let fd = guest.file_with("f", b"hello world");
    assert_eq!(guest.tell(fd), Ok(0));
    assert_eq!(guest.pread(fd, 6, 5).unwrap(), b"world");
    assert_eq!(guest.tell(fd), Ok(0));
    // A read that crosses the end is short; one that starts there is empty.
    assert_eq!(guest.pread(fd, 8, 100).unwrap(), b"rld");
    assert_eq!(guest.pread(fd, 11, 4).unwrap(), b"");
    assert_eq!(guest.pwrite(fd, 6, b"WORLD"), Ok(5));
    assert_eq!(guest.contents("f").await, b"hello WORLD");
    assert_eq!(guest.tell(fd), Ok(0));
    // The ordinary cursor still works independently: seek, then tell.
    guest.put(1024, &1500u32.to_le_bytes());
    guest.put(1028, &4u32.to_le_bytes());
    assert_eq!(guest.call("fd_read", &[i(fd), i(1024), i(1), i(1100)]), 0);
    assert_eq!(guest.tell(fd), Ok(4));
    assert_eq!(guest.pread(fd, 0, 2).unwrap(), b"he");
    assert_eq!(guest.tell(fd), Ok(4));
    assert_eq!(guest.call("fd_seek", &[i(fd), l(-1), i(2), i(1100)]), 0);
    assert_eq!(guest.tell(fd), Ok(10));
    // Writing past the end extends the file; the gap reads as zero.
    assert_eq!(guest.pwrite(fd, 14, b"!"), Ok(1));
    assert_eq!(guest.pread(fd, 10, 5).unwrap(), b"D\0\0\0!");
    assert_eq!(guest.tell(fd), Ok(10));
}

#[xila::task::test(task_path = xila::task)]
async fn positioned_io_handles_vectors_and_validates_before_moving_bytes() {
    let mut guest = Guest::new().await;
    let fd = guest.file_with("f", b"abcdefgh");
    // Two iovecs: 3 bytes then 10 bytes of buffer; the second read is short.
    guest.put(1024, &1500u32.to_le_bytes());
    guest.put(1028, &3u32.to_le_bytes());
    guest.put(1032, &1600u32.to_le_bytes());
    guest.put(1036, &10u32.to_le_bytes());
    assert_eq!(
        guest.call("fd_pread", &[i(fd), i(1024), i(2), l(1), i(1100)]),
        0
    );
    assert_eq!(guest.u32_at(1100), 7);
    assert_eq!(guest.get(1500, 3), b"bcd");
    assert_eq!(guest.get(1600, 4), b"efgh");
    // A vector outside memory, or an output pointer outside memory: EFAULT,
    // and nothing was written.
    guest.put(1032, &0xffff_fff0u32.to_le_bytes());
    assert_eq!(
        guest.call("fd_pwrite", &[i(fd), i(1024), i(2), l(0), i(1100)]),
        errno(Error::Fault)
    );
    assert_eq!(
        guest.call("fd_pwrite", &[i(fd), i(1024), i(1), l(0), i(0x1_0000)]),
        errno(Error::Fault)
    );
    assert_eq!(guest.contents("f").await, b"abcdefgh");
}

#[xila::task::test(task_path = xila::task)]
async fn positioned_io_rejects_the_wrong_kind_of_descriptor_and_access() {
    let mut guest = Guest::new().await;
    guest.file_with("f", b"data");
    let read_only = guest.open("f", READ).unwrap();
    let write_only = guest.open("f", WRITE).unwrap();
    assert_eq!(guest.pwrite(read_only, 0, b"x"), Err(errno(Error::Badf)));
    assert_eq!(guest.pread(write_only, 0, 1), Err(errno(Error::Badf)));
    assert_eq!(guest.pread(read_only, 0, 4).unwrap(), b"data");
    assert_eq!(guest.pread(ROOT, 0, 1), Err(errno(Error::Isdir)));
    assert_eq!(guest.pwrite(ROOT, 0, b"x"), Err(errno(Error::Isdir)));
    assert_eq!(guest.pread(99, 0, 1), Err(errno(Error::Badf)));
    // Standard streams have no position.
    assert_eq!(guest.pread(1, 0, 1), Err(errno(Error::Spipe)));
    assert_eq!(guest.pwrite(1, 0, b"x"), Err(errno(Error::Spipe)));
    assert_eq!(guest.tell(1), Err(errno(Error::Spipe)));
    assert_eq!(guest.tell(ROOT), Err(errno(Error::Isdir)));
    assert_eq!(guest.tell(99), Err(errno(Error::Badf)));
    assert_eq!(
        guest.call("fd_tell", &[i(read_only), i(0x1_0000)]),
        errno(Error::Fault)
    );
}

#[xila::task::test(task_path = xila::task)]
async fn fd_filestat_set_size_resizes_in_both_directions() {
    let mut guest = Guest::new().await;
    let fd = guest.file_with("f", b"0123456789");
    assert_eq!(guest.call("fd_filestat_set_size", &[i(fd), l(4)]), 0);
    assert_eq!(guest.size_of(fd), 4);
    assert_eq!(guest.pread(fd, 0, 10).unwrap(), b"0123");
    assert_eq!(guest.call("fd_filestat_set_size", &[i(fd), l(6)]), 0);
    assert_eq!(guest.size_of(fd), 6);
    assert_eq!(guest.pread(fd, 0, 10).unwrap(), b"0123\0\0");
    assert_eq!(guest.tell(fd), Ok(0));
    let read_only = guest.open("f", READ).unwrap();
    assert_eq!(
        guest.call("fd_filestat_set_size", &[i(read_only), l(1)]),
        errno(Error::Badf)
    );
    assert_eq!(
        guest.call("fd_filestat_set_size", &[i(ROOT), l(1)]),
        errno(Error::Isdir)
    );
    assert_eq!(
        guest.call("fd_filestat_set_size", &[i(99), l(1)]),
        errno(Error::Badf)
    );
}

const ATIM: i32 = 1;
const ATIM_NOW: i32 = 2;
const MTIM: i32 = 4;
const MTIM_NOW: i32 = 8;
const SECOND: i64 = 1_000_000_000;

#[xila::task::test(task_path = xila::task)]
async fn fd_filestat_set_times_applies_exactly_the_requested_fields() {
    let mut guest = Guest::new().await;
    let fd = guest.file_with("f", b"x");
    let call = |guest: &mut Guest, atime, mtime, flags| {
        guest.call(
            "fd_filestat_set_times",
            &[i(fd), l(atime), l(mtime), i(flags)],
        )
    };
    assert_eq!(call(&mut guest, 100 * SECOND, 200 * SECOND, ATIM | MTIM), 0);
    let (_, access, modification) = guest.stat("f").await.unwrap();
    assert_eq!((access, modification), (100, 200));
    // Only one field set: the other keeps its value.
    assert_eq!(call(&mut guest, 111 * SECOND, 999 * SECOND, ATIM), 0);
    let (_, access, modification) = guest.stat("f").await.unwrap();
    assert_eq!((access, modification), (111, 200));
    assert_eq!(
        call(&mut guest, 999 * SECOND, 222 * SECOND + 999_999_999, MTIM),
        0
    );
    let (_, access, modification) = guest.stat("f").await.unwrap();
    assert_eq!((access, modification), (111, 222));
    // No flag: nothing changes, and the values are not even read.
    assert_eq!(call(&mut guest, 5 * SECOND, 5 * SECOND, 0), 0);
    assert_eq!(guest.stat("f").await.unwrap().1, 111);
    // The invalid combinations.
    for flags in [ATIM | ATIM_NOW, MTIM | MTIM_NOW, 16, -1, 1 << 20] {
        assert_eq!(
            call(&mut guest, 1, 1, flags),
            errno(Error::Inval),
            "{flags}"
        );
    }
    let (_, access, modification) = guest.stat("f").await.unwrap();
    assert_eq!((access, modification), (111, 222));
    assert_eq!(
        guest.call("fd_filestat_set_times", &[i(99), l(0), l(0), i(ATIM)]),
        errno(Error::Badf)
    );
}

#[xila::task::test(task_path = xila::task)]
async fn the_now_flags_read_the_realtime_clock() {
    let mut guest = Guest::new().await;
    guest
        .clock
        .set_realtime(xila::time::RealtimeSample {
            timestamp: xila::time::Timestamp::from_nanos(1_700_000_000 * SECOND as i128 + 500),
            source: xila::time::ClockSource::Manual,
            resolution: core::time::Duration::from_secs(1),
            accuracy: None,
        })
        .unwrap();
    let fd = guest.file_with("f", b"x");
    assert_eq!(
        guest.call("fd_filestat_set_times", &[i(fd), l(1), l(1), i(ATIM_NOW)]),
        0
    );
    let (_, access, modification) = guest.stat("f").await.unwrap();
    assert!((1_700_000_000..1_700_000_100).contains(&access), "{access}");
    assert_ne!(modification, 1);
    assert_eq!(guest.path_set_times(0, "f", 1, 1, MTIM_NOW | ATIM), 0);
    let (_, access, modification) = guest.stat("f").await.unwrap();
    assert_eq!(access, 0);
    assert!(
        (1_700_000_000..1_700_000_100).contains(&modification),
        "{modification}"
    );
}

// ---------------------------------------------------------------------------
// Status flags, rights and renumbering
// ---------------------------------------------------------------------------

const APPEND: i32 = 1;
const DSYNC: i32 = 2;
const NONBLOCK: i32 = 4;
const SYNC: i32 = 16;

impl Guest {
    /// `fd_write` of `bytes` at the descriptor's cursor.
    fn write(&mut self, fd: i32, bytes: &[u8]) -> i32 {
        self.put(1500, bytes);
        self.put(1024, &1500u32.to_le_bytes());
        self.put(1028, &(bytes.len() as u32).to_le_bytes());
        self.call("fd_write", &[i(fd), i(1024), i(1), i(1100)])
    }

    fn open_directory(&mut self, path: &str) -> i32 {
        self.open_with(path, 2, 0).unwrap()
    }
}

#[xila::task::test(task_path = xila::task)]
async fn fd_fdstat_set_flags_changes_append_and_nonblocking() {
    let mut guest = Guest::new().await;
    let fd = guest.file_with("f", b"abc");
    assert_eq!(guest.fdstat(fd).unwrap().1, 0);
    assert_eq!(guest.call("fd_fdstat_set_flags", &[i(fd), i(APPEND)]), 0);
    assert_eq!(guest.fdstat(fd).unwrap().1, APPEND as u16);
    // The cursor is at 0, but an append-mode write goes to the end.
    assert_eq!(guest.write(fd, b"Z"), 0);
    assert_eq!(guest.pread(fd, 0, 8).unwrap(), b"abcZ");
    assert_eq!(
        guest.call("fd_fdstat_set_flags", &[i(fd), i(APPEND | NONBLOCK)]),
        0
    );
    assert_eq!(guest.fdstat(fd).unwrap().1, (APPEND | NONBLOCK) as u16);
    // The flags replace the previous set.
    assert_eq!(guest.call("fd_fdstat_set_flags", &[i(fd), i(NONBLOCK)]), 0);
    assert_eq!(guest.fdstat(fd).unwrap().1, NONBLOCK as u16);
    assert_eq!(guest.call("fd_fdstat_set_flags", &[i(fd), i(0)]), 0);
    assert_eq!(guest.fdstat(fd).unwrap().1, 0);
    assert_eq!(guest.call("fd_seek", &[i(fd), l(0), i(0), i(1100)]), 0);
    assert_eq!(guest.write(fd, b"Y"), 0);
    assert_eq!(guest.pread(fd, 0, 8).unwrap(), b"YbcZ");
}

#[xila::task::test(task_path = xila::task)]
async fn fd_fdstat_set_flags_rejects_what_it_cannot_honour() {
    let mut guest = Guest::new().await;
    let fd = guest.create("f");
    for flags in [DSYNC, 8, SYNC, APPEND | SYNC] {
        assert_eq!(
            guest.call("fd_fdstat_set_flags", &[i(fd), i(flags)]),
            errno(Error::Notsup),
            "{flags}"
        );
    }
    for flags in [32, -1, 1 << 16] {
        assert_eq!(
            guest.call("fd_fdstat_set_flags", &[i(fd), i(flags)]),
            errno(Error::Inval),
            "{flags}"
        );
    }
    assert_eq!(guest.fdstat(fd).unwrap().1, 0);
    assert_eq!(
        guest.call("fd_fdstat_set_flags", &[i(ROOT), i(0)]),
        errno(Error::Isdir)
    );
    assert_eq!(
        guest.call("fd_fdstat_set_flags", &[i(99), i(0)]),
        errno(Error::Badf)
    );
}

#[xila::task::test(task_path = xila::task)]
async fn dropped_rights_are_reported_and_enforced_and_cannot_be_regained() {
    let mut guest = Guest::new().await;
    let fd = guest.file_with("f", b"data");
    let (filetype, _, base, inheriting) = guest.fdstat(fd).unwrap();
    assert_eq!((filetype, base, inheriting), (4, u64::MAX, u64::MAX));

    let kept = READ | (1 << 2) | (1 << 5) | (1 << 21); // read, seek, tell, filestat_get
    assert_eq!(
        guest.call("fd_fdstat_set_rights", &[i(fd), l(kept), l(0)]),
        0
    );
    let (_, _, base, inheriting) = guest.fdstat(fd).unwrap();
    assert_eq!((base, inheriting), (kept as u64, 0));

    assert_eq!(guest.pread(fd, 0, 4).unwrap(), b"data");
    assert_eq!(guest.tell(fd), Ok(0));
    assert_eq!(guest.pwrite(fd, 0, b"x"), Err(errno(Error::Notcapable)));
    assert_eq!(guest.write(fd, b"x"), errno(Error::Notcapable));
    assert_eq!(
        guest.call("fd_filestat_set_size", &[i(fd), l(0)]),
        errno(Error::Notcapable)
    );
    assert_eq!(guest.call("fd_sync", &[i(fd)]), errno(Error::Notcapable));
    assert_eq!(
        guest.call("fd_datasync", &[i(fd)]),
        errno(Error::Notcapable)
    );
    assert_eq!(
        guest.call("fd_advise", &[i(fd), l(0), l(0), i(0)]),
        errno(Error::Notcapable)
    );
    assert_eq!(
        guest.call("fd_allocate", &[i(fd), l(0), l(1)]),
        errno(Error::Notcapable)
    );
    assert_eq!(
        guest.call("fd_filestat_set_times", &[i(fd), l(0), l(0), i(ATIM)]),
        errno(Error::Notcapable)
    );
    assert_eq!(
        guest.call("fd_fdstat_set_flags", &[i(fd), i(0)]),
        errno(Error::Notcapable)
    );
    assert_eq!(guest.size_of(fd), 4);
    // Seeking needs FD_SEEK, except that learning the position needs only FD_TELL.
    assert_eq!(guest.call("fd_seek", &[i(fd), l(1), i(0), i(1100)]), 0);
    assert_eq!(
        guest.call("fd_fdstat_set_rights", &[i(fd), l(READ | (1 << 5)), l(0)]),
        0
    );
    assert_eq!(
        guest.call("fd_seek", &[i(fd), l(1), i(0), i(1100)]),
        errno(Error::Notcapable)
    );
    assert_eq!(guest.call("fd_seek", &[i(fd), l(0), i(1), i(1100)]), 0);
    // Rights can only shrink.
    assert_eq!(
        guest.call("fd_fdstat_set_rights", &[i(fd), l(READ | WRITE), l(0)]),
        errno(Error::Notcapable)
    );
    assert_eq!(
        guest.call("fd_fdstat_set_rights", &[i(fd), l(READ), l(1)]),
        errno(Error::Notcapable)
    );
    assert_eq!(
        guest.call("fd_fdstat_set_rights", &[i(99), l(0), l(0)]),
        errno(Error::Badf)
    );
    // Other descriptors of the same file are unaffected.
    let other = guest.open("f", READ | WRITE).unwrap();
    assert_eq!(guest.fdstat(other).unwrap().2, u64::MAX);
}

#[xila::task::test(task_path = xila::task)]
async fn a_directory_with_reduced_rights_limits_what_it_opens_and_does() {
    let mut guest = Guest::new().await;
    File::write_to_path(guest.vfs, guest.task, "/sandbox/f", b"data")
        .await
        .unwrap();
    assert_eq!(guest.with_path("path_create_directory", ROOT, "sub"), 0);
    let directory = guest.open_directory("sub");
    // Keep opening files and reading them, but nothing that changes names.
    let path_open = 1 << 13;
    assert_eq!(
        guest.call(
            "fd_fdstat_set_rights",
            &[i(directory), l(path_open | (1 << 14)), l(READ | SEEK)]
        ),
        0
    );
    assert_eq!(
        guest.with_path("path_create_directory", directory, "x"),
        errno(Error::Notcapable)
    );
    assert_eq!(
        guest.with_path("path_unlink_file", directory, "x"),
        errno(Error::Notcapable)
    );
    assert_eq!(
        guest.with_path("path_remove_directory", directory, "x"),
        errno(Error::Notcapable)
    );
    assert_eq!(
        guest.call(
            "path_filestat_get",
            &[i(directory), i(0), i(4096), i(1), i(1300)]
        ),
        errno(Error::Notcapable)
    );
    // Opening is allowed as long as the request stays within the inheriting rights.
    File::write_to_path(guest.vfs, guest.task, "/sandbox/sub/f", b"data")
        .await
        .unwrap();
    let open = |guest: &mut Guest, name: &str, oflags: i32, base: i64| {
        let [pointer, length] = guest.text(0, name);
        guest.call(
            "path_open",
            &[
                i(directory),
                i(1),
                pointer,
                length,
                i(oflags),
                l(base),
                l(0),
                i(0),
                i(8000),
            ],
        )
    };
    assert_eq!(open(&mut guest, "f", 0, READ | SEEK), 0);
    let child = guest.u32_at(8000) as i32;
    assert_eq!(guest.fdstat(child).unwrap().2, (READ | SEEK) as u64);
    assert_eq!(guest.pread(child, 0, 4).unwrap(), b"data");
    assert_eq!(guest.pwrite(child, 0, b"x"), Err(errno(Error::Notcapable)));
    assert_eq!(
        open(&mut guest, "f", 0, READ | WRITE),
        errno(Error::Notcapable)
    );
    // Creating needs PATH_CREATE_FILE, which this directory lacks.
    assert_eq!(open(&mut guest, "new", 1, READ), errno(Error::Notcapable));
    assert!(!guest.exists("sub/new").await);
}

#[xila::task::test(task_path = xila::task)]
async fn fd_renumber_replaces_the_target_and_moves_rights() {
    let mut guest = Guest::new().await;
    let first = guest.file_with("a", b"aaa");
    let second = guest.file_with("b", b"bbb");
    assert_eq!(
        guest.call(
            "fd_fdstat_set_rights",
            &[i(first), l(READ | (1 << 2)), l(0)]
        ),
        0
    );
    assert_eq!(guest.call("fd_renumber", &[i(first), i(second)]), 0);
    // `first` is gone, `second` is now the old `first`: same file, same reduced rights.
    assert_eq!(guest.tell(first), Err(errno(Error::Badf)));
    assert_eq!(guest.pread(second, 0, 3).unwrap(), b"aaa");
    assert_eq!(guest.fdstat(second).unwrap().2, (READ | (1 << 2)) as u64);
    // Neither descriptor has to be anything in particular; both must be open.
    assert_eq!(guest.call("fd_renumber", &[i(second), i(second)]), 0);
    assert_eq!(
        guest.call("fd_renumber", &[i(second), i(77)]),
        errno(Error::Badf)
    );
    assert_eq!(
        guest.call("fd_renumber", &[i(99), i(second)]),
        errno(Error::Badf)
    );
    assert_eq!(
        guest.call("fd_renumber", &[i(-1), i(second)]),
        errno(Error::Badf)
    );
    assert_eq!(guest.pread(second, 0, 3).unwrap(), b"aaa");
    // Renumbering over an unrestricted descriptor drops the restriction of the old one.
    let third = guest.file_with("c", b"ccc");
    assert_eq!(guest.call("fd_renumber", &[i(third), i(second)]), 0);
    assert_eq!(guest.fdstat(second).unwrap().2, u64::MAX);
    // New descriptors never collide with a renumbered one.
    let fresh = guest.create("d");
    assert!(![first, second, third, ROOT, 1].contains(&fresh));
    assert_eq!(guest.pread(second, 0, 3).unwrap(), b"ccc");
}

#[xila::task::test(task_path = xila::task)]
async fn a_renumbered_preopen_stays_a_confined_preopen() {
    let mut guest = Guest::new().await;
    let file = guest.create("f");
    let prestat = |guest: &mut Guest, fd: i32| match guest.call("fd_prestat_get", &[i(fd), i(1100)])
    {
        0 => Ok(guest.u32_at(1104)),
        error => Err(error),
    };
    assert_eq!(prestat(&mut guest, ROOT), Ok(1));
    // Moving the preopen to another descriptor moves its name and its confinement.
    assert_eq!(guest.call("fd_renumber", &[i(ROOT), i(file)]), 0);
    assert_eq!(prestat(&mut guest, ROOT), Err(errno(Error::Badf)));
    assert_eq!(prestat(&mut guest, file), Ok(1));
    assert_eq!(
        guest.call("fd_prestat_dir_name", &[i(file), i(1100), i(1)]),
        0
    );
    assert_eq!(guest.get(1100, 1), b"/");
    assert_eq!(
        guest.with_path("path_create_directory", ROOT, "x"),
        errno(Error::Badf)
    );
    assert_eq!(guest.with_path("path_create_directory", file, "x"), 0);
    assert_eq!(
        guest.with_path("path_create_directory", file, "absolute/x"),
        errno(Error::Noent)
    );
    assert_eq!(
        guest.with_path("path_create_directory", file, "../x"),
        errno(Error::Notcapable)
    );
    assert!(guest.exists("x").await);
    assert!(
        guest
            .vfs
            .get_statistics_for_task(guest.task, &"/x")
            .await
            .is_err()
    );
}

#[xila::task::test(task_path = xila::task)]
async fn renumbering_over_a_preopen_closes_it_and_ends_its_preopen_status() {
    let mut guest = Guest::new().await;
    let file = guest.file_with("f", b"data");
    assert_eq!(guest.call("fd_renumber", &[i(file), i(ROOT)]), 0);
    assert_eq!(
        guest.call("fd_prestat_get", &[i(ROOT), i(1100)]),
        errno(Error::Badf)
    );
    assert_eq!(guest.pread(ROOT, 0, 4).unwrap(), b"data");
    assert_eq!(
        guest.with_path("path_create_directory", ROOT, "x"),
        errno(Error::Notdir)
    );
    assert_eq!(guest.pread(file, 0, 4), Err(errno(Error::Badf)));
}

// ---------------------------------------------------------------------------
// Path operations stay beneath their preopen
// ---------------------------------------------------------------------------

impl Guest {
    /// A sandbox with links that lead outside it, and something worth escaping to.
    async fn with_escape_routes() -> Self {
        let guest = Self::new().await;
        for (target, link) in [
            ("/outside", "/sandbox/absolute"),
            ("../outside", "/sandbox/parent"),
            ("absolute", "/sandbox/chain"),
        ] {
            guest
                .vfs
                .create_symlink(
                    guest.task,
                    xila::file_system::Path::new(target),
                    xila::file_system::Path::new(link),
                )
                .await
                .unwrap();
        }
        guest
    }

    /// Everything outside the sandbox is exactly as it was created.
    async fn assert_outside_untouched(&self) {
        let mut entries = Vec::new();
        let mut directory = SynchronousDirectory::open(self.vfs, self.task, "/outside").unwrap();
        while let Some(entry) = directory.read().unwrap() {
            entries.push(entry.name.to_string());
        }
        entries.sort();
        assert_eq!(entries, [".", "..", "secret"]);
        let statistics = self
            .vfs
            .get_statistics_for_task(self.task, &"/outside/secret")
            .await
            .unwrap();
        assert_eq!(statistics.size, 6);
    }

    /// Create a file of the sandbox from the host side (closed again at once).
    async fn seed(&self, name: &str, bytes: &[u8]) {
        File::write_to_path(
            self.vfs,
            self.task,
            format!("/sandbox/{name}").as_str(),
            bytes,
        )
        .await
        .unwrap();
    }
}

#[xila::task::test(task_path = xila::task)]
async fn path_rename_moves_replaces_and_renames_links_themselves() {
    let mut guest = Guest::with_escape_routes().await;
    guest.seed("a", b"first").await;
    assert_eq!(guest.rename("a", "b"), 0);
    assert!(!guest.exists("a").await);
    assert_eq!(guest.contents("b").await, b"first");
    // A file replaces an existing file.
    guest.seed("c", b"second").await;
    assert_eq!(guest.rename("b", "c"), 0);
    assert!(!guest.exists("b").await);
    assert_eq!(guest.contents("c").await, b"first");
    // Directories move with their contents, also between directory descriptors.
    assert_eq!(guest.with_path("path_create_directory", ROOT, "d"), 0);
    assert_eq!(guest.with_path("path_create_directory", ROOT, "e"), 0);
    guest.seed("d/x", b"inside").await;
    let directory = guest.open_directory("d");
    let other = guest.open_directory("e");
    let [from, from_length] = guest.text(0, "x");
    let [to, to_length] = guest.text(1, "y");
    assert_eq!(
        guest.call(
            "path_rename",
            &[i(directory), from, from_length, i(other), to, to_length]
        ),
        0
    );
    assert_eq!(guest.contents("e/y").await, b"inside");
    assert_eq!(guest.rename("e", "f"), 0);
    assert_eq!(guest.contents("f/y").await, b"inside");
    // A link is renamed, not what it points to.
    assert_eq!(guest.rename("chain", "moved"), 0);
    assert!(!guest.exists("chain").await);
    assert!(
        guest
            .vfs
            .get_link_statistics(&"/sandbox/moved")
            .await
            .is_ok()
    );
    // Renaming onto itself changes nothing.
    assert_eq!(guest.rename("c", "c"), 0);
    assert_eq!(guest.contents("c").await, b"first");
    assert_eq!(guest.rename("f", "f"), 0);
    assert_eq!(guest.contents("f/y").await, b"inside");
    // Missing source.
    assert_eq!(guest.rename("missing", "z"), errno(Error::Noent));
    assert_eq!(guest.rename("", "z"), errno(Error::Noent));
    guest.assert_outside_untouched().await;
}

#[xila::task::test(task_path = xila::task)]
async fn path_rename_cannot_move_the_preopen_or_a_directory_into_itself() {
    let mut guest = Guest::new().await;
    guest.seed("a", b"data").await;
    for (from, to) in [
        (".", "x"),
        ("a", "."),
        ("./", "x"),
        ("a", "/"),
        ("a", "sub/.."),
    ] {
        let status = guest.rename(from, to);
        assert!(
            [errno(Error::Busy), errno(Error::Notcapable)].contains(&status),
            "{from} -> {to}: {status}"
        );
    }
    assert!(guest.exists("a").await);
    assert_eq!(guest.with_path("path_create_directory", ROOT, "d"), 0);
    assert_eq!(guest.with_path("path_create_directory", ROOT, "d/c"), 0);
    assert_eq!(guest.rename("d", "d/c/moved"), errno(Error::Inval));
    assert!(guest.exists("d/c").await);
}

#[xila::task::test(task_path = xila::task)]
async fn path_rename_cannot_reach_through_links_or_parents() {
    let mut guest = Guest::with_escape_routes().await;
    guest.seed("a", b"data").await;
    let escape = errno(Error::Notcapable);
    for (from, to) in [
        ("absolute/secret", "stolen"),
        ("parent/secret", "stolen"),
        ("chain/secret", "stolen"),
        ("../outside/secret", "stolen"),
        ("a", "absolute/planted"),
        ("a", "parent/planted"),
        ("a", "chain/planted"),
        ("a", "../planted"),
        ("a", "sub/../../planted"),
    ] {
        assert_eq!(guest.rename(from, to), escape, "{from} -> {to}");
    }
    assert!(guest.exists("a").await);
    assert!(!guest.exists("stolen").await);
    // A directory descriptor opened through a link keeps the original confinement.
    assert_eq!(guest.with_path("path_create_directory", ROOT, "sub"), 0);
    guest.symlink("sub", "inside");
    let inside = guest.open_directory("inside");
    assert_eq!(
        guest.with_path("path_create_directory", inside, "../planted"),
        escape
    );
    let [from, from_length] = guest.text(0, "../a");
    let [to, to_length] = guest.text(1, "moved");
    assert_eq!(
        guest.call(
            "path_rename",
            &[i(inside), from, from_length, i(ROOT), to, to_length]
        ),
        escape
    );
    guest.assert_outside_untouched().await;
}

#[xila::task::test(task_path = xila::task)]
async fn path_remove_directory_removes_only_empty_directories() {
    let mut guest = Guest::with_escape_routes().await;
    assert_eq!(guest.with_path("path_create_directory", ROOT, "d"), 0);
    guest.seed("d/x", b"data").await;
    guest.seed("file", b"data").await;
    let remove =
        |guest: &mut Guest, path: &str| guest.with_path("path_remove_directory", ROOT, path);
    assert_eq!(remove(&mut guest, "d"), errno(Error::Notempty));
    assert_eq!(remove(&mut guest, "file"), errno(Error::Notdir));
    assert_eq!(remove(&mut guest, "missing"), errno(Error::Noent));
    assert_eq!(remove(&mut guest, ""), errno(Error::Noent));
    // A link is not a directory, even one that points at a directory.
    assert_eq!(remove(&mut guest, "absolute"), errno(Error::Notdir));
    assert!(
        guest
            .vfs
            .get_link_statistics(&"/sandbox/absolute")
            .await
            .is_ok()
    );
    assert_eq!(guest.with_path("path_unlink_file", ROOT, "d/x"), 0);
    assert_eq!(remove(&mut guest, "d"), 0);
    assert!(!guest.exists("d").await);
    // The preopen itself, and everything outside it, is out of reach.
    for path in [".", "./", "/", "d/.."] {
        let status = remove(&mut guest, path);
        assert!(
            [errno(Error::Inval), errno(Error::Notcapable)].contains(&status),
            "{path}: {status}"
        );
    }
    for path in [
        "absolute/x",
        "parent/x",
        "chain/x",
        "../outside",
        "sub/../../outside",
    ] {
        assert_eq!(remove(&mut guest, path), errno(Error::Notcapable), "{path}");
    }
    assert!(guest.exists("").await);
    guest.assert_outside_untouched().await;
    assert_eq!(
        guest.with_path("path_remove_directory", 99, "x"),
        errno(Error::Badf)
    );
    assert_eq!(
        guest.with_path("path_remove_directory", 1, "x"),
        errno(Error::Notdir)
    );
}

#[xila::task::test(task_path = xila::task)]
async fn path_link_checks_everything_and_reports_that_hard_links_are_unsupported() {
    let mut guest = Guest::with_escape_routes().await;
    guest.seed("a", b"data").await;
    assert_eq!(guest.with_path("path_create_directory", ROOT, "d"), 0);
    // A well-formed request fails because the file system has no hard links.
    assert_eq!(guest.link(1, "a", "b"), errno(Error::Notsup));
    assert_eq!(guest.link(0, "a", "b"), errno(Error::Notsup));
    assert!(!guest.exists("b").await);
    // Everything that can be wrong with the request is reported first.
    assert_eq!(guest.link(1, "missing", "b"), errno(Error::Noent));
    assert_eq!(guest.link(1, "a", "d"), errno(Error::Exist));
    assert_eq!(guest.link(1, "a", "a"), errno(Error::Exist));
    assert_eq!(guest.link(1, "d", "b"), errno(Error::Perm));
    assert_eq!(guest.link(2, "a", "b"), errno(Error::Inval));
    assert_eq!(guest.link(-1, "a", "b"), errno(Error::Inval));
    assert_eq!(guest.link(1, "", "b"), errno(Error::Noent));
    assert_eq!(guest.link(1, "a", ""), errno(Error::Noent));
    for (from, to) in [
        ("absolute/secret", "b"),
        ("parent/secret", "b"),
        ("../outside/secret", "b"),
        ("a", "absolute/b"),
        ("a", "parent/b"),
        ("a", "../b"),
        ("a", "d/../../b"),
    ] {
        assert_eq!(
            guest.link(1, from, to),
            errno(Error::Notcapable),
            "{from} -> {to}"
        );
    }
    // Following the final link of the source would leave the preopen, not following links to it
    // is fine: the link itself is inside.
    assert_eq!(guest.link(1, "absolute", "b"), errno(Error::Notcapable));
    assert_eq!(guest.link(0, "absolute", "b"), errno(Error::Notsup));
    let [pointer, length] = guest.text(0, "a");
    let [other, other_length] = guest.text(1, "b");
    assert_eq!(
        guest.call(
            "path_link",
            &[
                i(99),
                i(1),
                pointer.clone(),
                length.clone(),
                i(ROOT),
                other.clone(),
                other_length.clone()
            ]
        ),
        errno(Error::Badf)
    );
    assert_eq!(
        guest.call(
            "path_link",
            &[i(ROOT), i(1), pointer, length, i(99), other, other_length]
        ),
        errno(Error::Badf)
    );
    guest.assert_outside_untouched().await;
}

#[xila::task::test(task_path = xila::task)]
async fn path_filestat_set_times_follows_links_only_when_asked_and_stays_beneath() {
    let mut guest = Guest::with_escape_routes().await;
    assert_eq!(guest.with_path("path_create_directory", ROOT, "sub"), 0);
    guest.seed("f", b"data").await;
    guest.symlink("sub", "inside");
    assert_eq!(
        guest.path_set_times(1, "f", 100 * SECOND, 200 * SECOND, ATIM | MTIM),
        0
    );
    let (_, access, modification) = guest.stat("f").await.unwrap();
    assert_eq!((access, modification), (100, 200));
    // With SYMLINK_FOLLOW the target changes; without it the link does.
    assert_eq!(
        guest.path_set_times(1, "inside", 300 * SECOND, 400 * SECOND, ATIM | MTIM),
        0
    );
    let (_, access, modification) = guest.stat("sub").await.unwrap();
    assert_eq!((access, modification), (300, 400));
    assert_eq!(
        guest.path_set_times(0, "inside", 500 * SECOND, 600 * SECOND, ATIM | MTIM),
        0
    );
    let (_, access, modification) = guest.stat("sub").await.unwrap();
    assert_eq!((access, modification), (300, 400));
    let link = guest
        .vfs
        .get_link_statistics(&"/sandbox/inside")
        .await
        .unwrap();
    assert_eq!(
        (link.access.as_u64(), link.modification.as_u64()),
        (500, 600)
    );
    // Only one field.
    assert_eq!(guest.path_set_times(1, "f", 1, 700 * SECOND, MTIM), 0);
    let (_, access, modification) = guest.stat("f").await.unwrap();
    assert_eq!((access, modification), (100, 700));
    // The preopen itself can be addressed as ".".
    assert_eq!(
        guest.path_set_times(1, ".", 800 * SECOND, 900 * SECOND, ATIM | MTIM),
        0
    );
    let (_, access, modification) = guest.stat("").await.unwrap();
    assert_eq!((access, modification), (800, 900));
    // Argument errors, before anything is touched.
    for flags in [ATIM | ATIM_NOW, MTIM | MTIM_NOW, 16] {
        assert_eq!(
            guest.path_set_times(1, "f", 1, 1, flags),
            errno(Error::Inval)
        );
    }
    assert_eq!(
        guest.path_set_times(2, "f", 1, 1, ATIM),
        errno(Error::Inval)
    );
    assert_eq!(
        guest.path_set_times(1, "missing", 1, 1, ATIM),
        errno(Error::Noent)
    );
    // No flags still has to name something that exists.
    assert_eq!(guest.path_set_times(1, "f", 1, 1, 0), 0);
    assert_eq!(
        guest.path_set_times(1, "missing", 1, 1, 0),
        errno(Error::Noent)
    );
    assert_eq!(guest.path_set_times(1, "", 1, 1, ATIM), errno(Error::Noent));
    assert_eq!(guest.stat("f").await.unwrap().1, 100);
}

#[xila::task::test(task_path = xila::task)]
async fn path_filestat_set_times_cannot_reach_outside_the_preopen() {
    let mut guest = Guest::with_escape_routes().await;
    let before = guest
        .vfs
        .get_statistics_for_task(guest.task, &"/outside/secret")
        .await
        .unwrap();
    let escape = errno(Error::Notcapable);
    for path in [
        "absolute/secret",
        "parent/secret",
        "chain/secret",
        "../outside/secret",
        "../outside",
        "sub/../../outside",
    ] {
        assert_eq!(
            guest.path_set_times(1, path, 7 * SECOND, 7 * SECOND, ATIM | MTIM),
            escape,
            "{path}"
        );
    }
    // Following a link that leads out is an escape; the link itself is inside.
    for link in ["absolute", "parent", "chain"] {
        assert_eq!(
            guest.path_set_times(1, link, 7 * SECOND, 7 * SECOND, ATIM | MTIM),
            escape,
            "follow {link}"
        );
        assert_eq!(
            guest.path_set_times(0, link, 7 * SECOND, 7 * SECOND, ATIM | MTIM),
            0,
            "no follow {link}"
        );
    }
    let after = guest
        .vfs
        .get_statistics_for_task(guest.task, &"/outside/secret")
        .await
        .unwrap();
    assert_eq!(
        (after.access, after.modification),
        (before.access, before.modification)
    );
    let outside = guest
        .vfs
        .get_statistics_for_task(guest.task, &"/outside")
        .await
        .unwrap();
    assert_ne!(outside.modification.as_u64(), 7);
    // Descriptors of directories obey the same confinement.
    assert_eq!(guest.with_path("path_create_directory", ROOT, "sub"), 0);
    let sub = guest.open_directory("sub");
    let [pointer, length] = guest.text(0, "../../outside");
    assert_eq!(
        guest.call(
            "path_filestat_set_times",
            &[i(sub), i(1), pointer, length, l(1), l(1), i(ATIM)]
        ),
        escape
    );
    // A directory descriptor changes its own times.
    assert_eq!(
        guest.call(
            "fd_filestat_set_times",
            &[i(sub), l(11 * SECOND), l(12 * SECOND), i(ATIM | MTIM)]
        ),
        0
    );
    let (_, access, modification) = guest.stat("sub").await.unwrap();
    assert_eq!((access, modification), (11, 12));
    assert_eq!(
        guest.call(
            "fd_filestat_set_times",
            &[i(ROOT), l(21 * SECOND), l(1), i(ATIM)]
        ),
        0
    );
    assert_eq!(guest.stat("").await.unwrap().1, 21);
}

#[xila::task::test(task_path = xila::task)]
async fn proc_raise_follows_the_default_disposition_of_each_signal() {
    let mut guest = Guest::new().await;
    for signal in [0, 31, 99, -1] {
        assert_eq!(
            guest.call("proc_raise", &[i(signal)]),
            errno(Error::Inval),
            "{signal}"
        );
    }
    // chld, cont, urg, winch: ignored by default.
    for signal in [16, 17, 22, 27] {
        assert_eq!(guest.call("proc_raise", &[i(signal)]), 0, "{signal}");
    }
    // stop, tstp, ttin, ttou: no job control to suspend or resume the guest.
    for signal in [18, 19, 20, 21] {
        assert_eq!(
            guest.call("proc_raise", &[i(signal)]),
            errno(Error::Notsup),
            "{signal}"
        );
    }
    assert_eq!(guest.store.data().wasi.exit_code, None);
    // Everything else ends the guest, with the status a shell reports for a signalled child.
    for (signal, status) in [(15, 143), (6, 134), (9, 137), (2, 130), (30, 158)] {
        let mut guest = Guest::new().await;
        assert!(
            guest.try_call("proc_raise", &[i(signal)]).is_err(),
            "{signal}"
        );
        assert_eq!(guest.store.data().wasi.exit_code, Some(status), "{signal}");
    }
}

#[xila::task::test(task_path = xila::task)]
async fn socket_imports_fail_cleanly_because_no_socket_is_preopened() {
    let mut guest = Guest::new().await;
    let file = guest.create("f");
    let calls: [(&str, usize); 4] = [
        ("sock_accept", 3),
        ("sock_recv", 6),
        ("sock_send", 5),
        ("sock_shutdown", 2),
    ];
    for (name, arity) in calls {
        let call = |guest: &mut Guest, fd: i32| {
            let mut arguments = alloc::vec![i(fd)];
            // Garbage in every other argument must be neither dereferenced nor rejected first.
            arguments.extend((1..arity).map(|_| i(-1)));
            guest.call(name, &arguments)
        };
        assert_eq!(call(&mut guest, 99), errno(Error::Badf), "{name}");
        assert_eq!(call(&mut guest, -1), errno(Error::Badf), "{name}");
        for descriptor in [file, ROOT, 1] {
            assert_eq!(
                call(&mut guest, descriptor),
                errno(Error::Notsock),
                "{name} {descriptor}"
            );
        }
    }
}
