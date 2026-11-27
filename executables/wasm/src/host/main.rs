use core::num::{NonZeroU32, NonZeroUsize};

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::fmt::Write;
use getargs_derive::GetArgs;
use wasmi::{Caller, Engine, Linker, Store};
use xila::{
    executable::{ExecutableContext, Standard},
    file_system::{Kind, Path},
    log,
    virtual_file_system::File,
};

use crate::host::{
    engine::Translation,
    error::{Error, Result},
    integrity, linear_memory,
    store::GlobalStore,
    wasi::{self, DirectoryVariant, FileSystemItem, FileVariant, Prestat, WasiContext},
};

const DEFAULT_STACK_SIZE: usize = 4096;

const START_FUNCTION_NAME: &str = "_start";

#[cfg(test)]
mod parser_tests {
    use super::{WasmArguments, split_guest_arguments};
    use alloc::{
        string::{String, ToString},
        vec,
    };

    #[test]
    fn defaults_preserve_existing_behavior() {
        let parsed = WasmArguments::from_args(["app.wasm"].into_iter()).unwrap();
        assert_eq!(parsed.path, "app.wasm");
        assert_eq!(parsed.stack_size, 4096);
        assert_eq!(parsed.instruction_limit.get(), 100_000);
        assert_eq!(parsed.translation, super::Translation::DEFAULT);
    }

    #[test]
    fn selects_the_translation_mode() {
        for (value, expected) in [
            ("eager", super::Translation::Eager),
            ("lazy", super::Translation::Lazy),
        ] {
            let parsed =
                WasmArguments::from_args(["--translation", value, "app.wasm"].into_iter()).unwrap();
            assert_eq!(parsed.translation, expected);
        }
        assert!(
            WasmArguments::from_args(["--translation", "later", "app.wasm"].into_iter()).is_err()
        );
    }

    #[test]
    fn preserves_named_limits_and_inferred_short_flags() {
        for stack in ["--stack-size", "-s"] {
            let parsed = WasmArguments::from_args(
                [stack, "8192", "--instruction-limit", "42", "app.wasm"].into_iter(),
            )
            .unwrap();
            assert_eq!(parsed.stack_size, 8192);
            assert_eq!(parsed.instruction_limit.get(), 42);
        }
    }

    #[test]
    fn developer_mode_is_off_unless_requested() {
        let parsed = WasmArguments::from_args(["app.wasm"].into_iter()).unwrap();
        assert!(!parsed.developer_mode);
        let parsed =
            WasmArguments::from_args(["--developer-mode", "/temporary/app.wasm"].into_iter())
                .unwrap();
        assert!(parsed.developer_mode);
        assert_eq!(parsed.path, "/temporary/app.wasm");
    }

    #[test]
    fn rejects_invalid_instruction_limit() {
        for value in ["0", "many", "4294967296"] {
            let error =
                WasmArguments::from_args(["--instruction-limit", value, "app.wasm"].into_iter())
                    .err()
                    .unwrap();
            assert!(
                matches!(error, getargs_derive::Error::InvalidValue { value: actual, .. } if actual == value)
            );
        }
    }

    #[test]
    fn generated_help_and_version_do_not_require_a_path() {
        let error = WasmArguments::from_args(["--help"].into_iter())
            .err()
            .unwrap();
        assert!(error.is_help());
        assert!(error.to_string().contains("Usage: wasm"));
        let error = WasmArguments::from_args(["--version"].into_iter())
            .err()
            .unwrap();
        assert!(error.is_version());
    }

    #[test]
    fn legacy_install_hook_is_not_supported() {
        assert!(WasmArguments::from_args(["--install", "app.wasm"].into_iter()).is_err());
    }

    #[test]
    fn separates_guest_arguments_from_wasm_runner_options() {
        let arguments = vec![
            "--instruction-limit".to_string(),
            "42".to_string(),
            "/application/app.wasm".to_string(),
            "--".to_string(),
            "first".to_string(),
            "--guest-option".to_string(),
        ];
        let (runner, guest) = split_guest_arguments(&arguments);
        assert_eq!(runner.len(), 3);
        assert_eq!(
            guest,
            &[String::from("first"), String::from("--guest-option")]
        );
        let parsed =
            WasmArguments::from_args(runner.iter().map(|argument| argument.as_str())).unwrap();
        assert_eq!(parsed.path, "/application/app.wasm");
    }

    #[test]
    fn guest_exit_status_preserves_representable_codes() {
        assert!(super::guest_exit_status(0).is_ok());
        assert_eq!(super::guest_exit_status(7).unwrap_err().get(), 7);
        assert_eq!(super::guest_exit_status(300).unwrap_err().get(), 255);
    }
}

#[derive(GetArgs)]
#[command(name = "wasm", about = "Run a WebAssembly executable.", version)]
struct WasmArguments<'a> {
    /// Path to the executable in the Xila filesystem.
    path: &'a str,
    /// Legacy stack-size setting; currently managed by wasmi rather than applied here.
    #[arg(short = 's', long, default_value_t = DEFAULT_STACK_SIZE)]
    stack_size: usize,
    /// Maximum number of instructions per execution slice.
    #[arg(long, default_value_t = NonZeroU32::new(100_000).unwrap())]
    instruction_limit: NonZeroU32,
    /// Run an executable outside installed packages without integrity verification.
    /// Installed package executables are always verified.
    #[arg(long = "developer-mode")]
    developer_mode: bool,
    /// When to translate functions: all while loading, or each at its first call.
    #[arg(long, value_enum, default_value_t = Translation::DEFAULT)]
    translation: Translation,
}

/// Run one guest with an independently owned stop handle. Join this future
/// before replacing its module or reclaiming its VFS and graphics resources.
pub async fn execute_with_control(
    standard: Standard<'static>,
    arguments: Vec<String>,
    control: xila::synchronization::Arc<super::ExecutionControl>,
) -> core::result::Result<(), NonZeroUsize> {
    execute(
        standard,
        arguments,
        control,
        super::DEFAULT_MEMORY_GROWTH_ALLOWANCE,
    )
    .await
}

pub(super) async fn execute(
    mut standard: Standard<'static>,
    arguments: Vec<String>,
    control: xila::synchronization::Arc<super::ExecutionControl>,
    memory_growth_allowance: usize,
) -> core::result::Result<(), NonZeroUsize> {
    // - Admission: guests are untrusted, so they never run with an identity
    //   that a launcher granted, such as the Shell's `sudo`. Core marks the
    //   task launched with that identity and every task it starts, and the
    //   mark cannot be cleared, so this covers the runner given to `sudo` as
    //   well as any native program that `sudo` ran and its descendants. A
    //   task whose mark cannot be read is refused too.
    let task_manager = standard.context.task_manager;
    let task = task_manager.get_current_task_identifier().await;
    if !matches!(task_manager.is_identity_granted(task).await, Ok(false)) {
        log::warning!("WebAssembly guest refused: the task runs with a granted identity");
        writeln!(
            standard.error(),
            "Error: WebAssembly guests do not run with an identity granted by sudo"
        )
        .ok();
        return Err(NonZeroUsize::new(1).unwrap());
    }

    let (runtime_arguments, guest_arguments) = split_guest_arguments(&arguments);
    let arguments = match WasmArguments::from_args(runtime_arguments.iter().map(|s| s.as_str())) {
        Ok(args) => args,
        Err(error) if error.is_help() || error.is_version() => {
            writeln!(standard.out(), "{error}").ok();
            return Ok(());
        }
        Err(e) => {
            writeln!(standard.error(), "Error parsing arguments: {}", e).unwrap();
            return Err(NonZeroUsize::new(1).unwrap());
        }
    };

    let path = arguments.path;
    match main_inner(
        standard.duplicate().await.unwrap(),
        arguments,
        guest_arguments.to_vec(),
        control,
        memory_growth_allowance,
    )
    .await
    {
        Ok(()) | Err(Error::Runtime(0)) => Ok(()),
        Err(Error::Runtime(code)) => guest_exit_status(code),
        Err(Error::Interrupted) => Err(NonZeroUsize::new(1).unwrap()),
        Err(Error::Integrity(error)) => {
            writeln!(standard.error(), "wasm: refusing to run {path}: {error}").ok();
            Err(NonZeroUsize::new(1).unwrap())
        }
        Err(error) => {
            writeln!(standard.error(), "Error: {:?}", error).unwrap();
            Err(NonZeroUsize::new(1).unwrap())
        }
    }
}

fn guest_exit_status(code: i32) -> core::result::Result<(), NonZeroUsize> {
    if code == 0 {
        Ok(())
    } else {
        Err(NonZeroUsize::new(code.clamp(1, 255) as usize).unwrap())
    }
}

fn split_guest_arguments(arguments: &[String]) -> (&[String], &[String]) {
    match arguments.iter().position(|argument| argument == "--") {
        Some(separator) => (&arguments[..separator], &arguments[separator + 1..]),
        None => (arguments, &[]),
    }
}

async fn read_file(path: &str, context: &'static ExecutableContext) -> Result<Vec<u8>> {
    let virtual_file_system = context.virtual_file_system;

    let task = context.task_manager.get_current_task_identifier().await;

    let path = Path::new(path);

    let statistics = virtual_file_system.get_statistics(&path).await?;

    if statistics.kind != Kind::File {
        return Err(Error::NotAWasmFile);
    }

    let capacity = (statistics.size as usize).saturating_add(linear_memory::SOURCE_HEADROOM);
    let mut buffer = Vec::with_capacity(capacity);

    File::read_from_path(virtual_file_system, task, &path, &mut buffer).await?;

    Ok(buffer)
}

/// Read the bytes to compile under the integrity policy described in
/// [`integrity`]: installed package executables are always verified against
/// their installed manifest; other executables need developer mode.
async fn read_executable(
    path: &str,
    developer_mode: bool,
    context: &'static ExecutableContext,
) -> Result<Vec<u8>> {
    match integrity::classify(path)? {
        integrity::Origin::Package {
            identifier,
            executable,
        } => {
            let task = context.task_manager.get_current_task_identifier().await;
            Ok(integrity::read_package_executable(
                context.virtual_file_system,
                task,
                identifier,
                executable,
            )
            .await?)
        }
        integrity::Origin::Unmanaged if developer_mode => {
            log::warning!("Running unverified WebAssembly executable {path} in developer mode");
            read_file(path, context).await
        }
        integrity::Origin::Unmanaged => Err(integrity::IntegrityError::Unmanaged.into()),
    }
}

async fn main_inner(
    standard: Standard<'static>,
    arguments: WasmArguments<'_>,
    guest_arguments: Vec<String>,
    control: xila::synchronization::Arc<super::ExecutionControl>,
    memory_growth_allowance: usize,
) -> Result<()> {
    let context = standard.context;
    let WasmArguments {
        path,
        stack_size: _stack_size,
        instruction_limit,
        developer_mode,
        translation,
    } = arguments;

    let task = context.task_manager.get_current_task_identifier().await;

    // Refuse an executable before provisioning anything on its behalf.
    let engine = Engine::new(&super::engine::configuration(translation));
    let source = read_executable(path, developer_mode, context).await?;
    let (module, memory_plan) = linear_memory::compile(&engine, source, memory_growth_allowance)?;

    // Application directory provisioning is a convenience for the Wasm runner,
    // not a VFS permission override. The filesystem enforces ordinary UID/GID
    // and mode bits. Core does not discover or identify application bundles.
    let application_name = if let Some(rest) = path.strip_prefix("/applications/bundles/") {
        rest.split('/').next()
    } else {
        path.strip_prefix("/applications/executables/")
            .and_then(|file| file.rsplit('/').next())
            .map(|name| name.strip_suffix(".wasm").unwrap_or(name))
    }
    .filter(|name| {
        !name.is_empty()
            && name.len() <= 96
            && !matches!(*name, "." | "..")
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    });
    if let Some(application_name) = application_name {
        let user = context.task_manager.get_user(task).await?;
        // The directory lives in the home recorded in the user's account.
        let application_directory = context
            .virtual_file_system
            .ensure_user_application_directories(task, user, application_name)
            .await?;
        let configuration = application_directory
            .clone()
            .append("configuration")
            .ok_or(Error::InvalidPath)?;
        let data = application_directory
            .clone()
            .append("data")
            .ok_or(Error::InvalidPath)?;
        let cache = application_directory
            .append("cache")
            .ok_or(Error::InvalidPath)?;
        context
            .task_manager
            .set_environment_variables(
                task,
                &[
                    ("XILA_APPLICATION_CONFIGURATION", configuration.as_str()),
                    ("XILA_APPLICATION_DATA", data.as_str()),
                    ("XILA_APPLICATION_CACHE", cache.as_str()),
                ],
            )
            .await?;
    }

    let (_standard_in, standard_out, standard_error) = standard.split();
    let root_directory =
        DirectoryVariant::preopen(context.virtual_file_system, task, Path::from_str("/")).await;

    let mut memory_buffer = memory_plan
        .map(linear_memory::MemoryBuffer::allocate)
        .transpose()?;
    let mut store = Store::new(
        &engine,
        GlobalStore {
            control,
            context,
            pending: None,
            #[cfg(feature = "graphics")]
            graphics: super::graphics::GraphicsState::new(context.graphics_manager),
            wasi: WasiContext {
                files: alloc::collections::BTreeMap::new(),
                rights: Default::default(),
                next_fd: 3,
                arguments: {
                    let mut arguments = alloc::vec![path.to_string()];
                    arguments.extend(guest_arguments);
                    arguments
                },
                environment: context
                    .task_manager
                    .get_environment_variables(task)
                    .await?
                    .into_iter()
                    .map(|variable| {
                        (
                            variable.get_name().to_string(),
                            variable.get_value().to_string(),
                        )
                    })
                    .collect(),
                task,
                prestats: Default::default(),
                exit_code: None,
            },
        },
    );

    {
        let data = store.data_mut();
        data.wasi.files.insert(
            0,
            FileSystemItem::StandardInput(FileVariant {
                file: _standard_in.into_synchronous_file(),
            }),
        );
        data.wasi.files.insert(
            1,
            FileSystemItem::StandardOutput(FileVariant {
                file: standard_out.into_synchronous_file(),
            }),
        );
        data.wasi.files.insert(
            2,
            FileSystemItem::StandardError(FileVariant {
                file: standard_error.into_synchronous_file(),
            }),
        );
        match root_directory {
            Err(error) => {
                // Without a confined root the guest gets no file system at all.
                log::warning!("WebAssembly guest runs without a preopened directory: {error}");
            }
            Ok(root_directory) => {
                data.wasi
                    .files
                    .insert(3, FileSystemItem::Directory(root_directory));
                data.wasi.next_fd = 4;
                data.wasi.prestats.insert(
                    3,
                    Prestat {
                        name: b"/".to_vec(),
                    },
                );
            }
        }
    }

    let mut linker = Linker::<GlobalStore>::new(&engine);

    wasi::register::add_wasi_to_linker(&mut linker)?;
    #[cfg(feature = "graphics")]
    super::graphics::wasmi::add_to_linker(&mut linker)?;

    linker.func_wrap(
        "host",
        "hello",
        |caller: Caller<'_, GlobalStore>, param: i32| {
            log::information!(
                "Hello from WebAssembly! Got {} from WebAssembly and my host state is: {}",
                param,
                caller.data()
            );
        },
    )?;

    if let Some(memory_buffer) = memory_buffer.as_mut() {
        // SAFETY: This links the buffer once. The store and linker are
        // declared after the buffer and drop before it on every path.
        unsafe { memory_buffer.link(&mut store, &mut linker) }?;
    }
    store.set_fuel(instruction_limit.get() as u64)?;
    let start = match linker.instantiate_and_start(&mut store, &module) {
        Ok(instance) => instance.get_typed_func::<(), ()>(&store, START_FUNCTION_NAME)?,
        Err(e) => {
            return if let Some(code) = store.data().wasi.exit_code {
                if code == 0 {
                    Ok(())
                } else {
                    Err(Error::Runtime(code))
                }
            } else {
                Err(Error::Wasm(e))
            };
        }
    };
    drop(linker);
    drop(module);
    super::execution::run(&mut store, start, instruction_limit).await
}
