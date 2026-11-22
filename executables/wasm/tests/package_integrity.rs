#![cfg(feature = "host")]
//! Load-time integrity of installed package executables (applications#30).
testing::instantiate_global_allocator!();

use manifest::{FORMAT_VERSION, MANIFEST_PATH, Manifest, ManifestFile, XilaVersion};
use verification::Sha256Verifier;
use wasm::integrity::{self, IntegrityError};
use xila::{
    executable::Standard,
    file_system::{Path, Permissions},
    synchronization::Arc,
    task::TaskIdentifier,
    virtual_file_system::{Directory, File, IoErrorKind, VirtualFileSystem},
};

const IDENTIFIER: &str = "org.example.integrity";
const EXECUTABLE: &str = "executables/application.wasm";
const BUNDLE: &str = "/applications/bundles/org.example.integrity";
const PATH: &str = "/applications/bundles/org.example.integrity/executables/application.wasm";

fn guest() -> Vec<u8> {
    wat::parse_str(
        r#"(module
          (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
          (func (export "_start") i32.const 0 call $exit))"#,
    )
    .unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    let mut hash = Sha256Verifier::new();
    hash.update(bytes).unwrap();
    hash.finalize()
        .unwrap()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn manifest_for(name: &str, path: &str, bytes: &[u8]) -> Vec<u8> {
    manifest::encode(&Manifest {
        schema: None,
        format_version: FORMAT_VERSION,
        name: name.into(),
        version: "1.0.0".into(),
        target: "wasm32-wasip1".into(),
        xila_version: XilaVersion::new(0, 1),
        capabilities: vec![],
        files: vec![ManifestFile {
            path: path.into(),
            size: bytes.len() as u64,
            sha256: sha256(bytes),
        }],
        application: None,
        metadata: None,
    })
    .unwrap()
    .into_bytes()
}

async fn create_directory(file_system: &VirtualFileSystem, task: TaskIdentifier, path: &str) {
    match Directory::create(file_system, task, Path::new(path)).await {
        Ok(()) | Err(IoErrorKind::AlreadyExists) => {}
        Err(error) => panic!("cannot create {path}: {error:?}"),
    }
}

/// Lay out a bundle as the installer publishes it, sealed read-only.
async fn install(
    file_system: &VirtualFileSystem,
    task: TaskIdentifier,
    manifest: Option<&[u8]>,
    executable: &[u8],
) {
    for directory in [
        "/applications",
        "/applications/bundles",
        BUNDLE,
        &format!("{BUNDLE}/.xila"),
        &format!("{BUNDLE}/executables"),
    ] {
        create_directory(file_system, task, directory).await;
    }
    let executable_path = format!("{BUNDLE}/{EXECUTABLE}");
    File::write_to_path(file_system, task, executable_path.as_str(), executable)
        .await
        .unwrap();
    let manifest_path = format!("{BUNDLE}/{MANIFEST_PATH}");
    if let Some(manifest) = manifest {
        File::write_to_path(file_system, task, manifest_path.as_str(), manifest)
            .await
            .unwrap();
        seal(file_system, task, &manifest_path, 0o444).await;
    }
    seal(file_system, task, &executable_path, 0o444).await;
}

async fn seal(file_system: &VirtualFileSystem, task: TaskIdentifier, path: &str, mode: u16) {
    file_system
        .set_permissions(
            task,
            Path::new(path),
            Permissions::from_octal(mode).unwrap(),
        )
        .await
        .unwrap();
}

/// Replace a sealed file's content as a privileged attacker would.
async fn overwrite(
    file_system: &VirtualFileSystem,
    task: TaskIdentifier,
    path: &str,
    bytes: &[u8],
) {
    seal(file_system, task, path, 0o644).await;
    File::write_to_path(file_system, task, path, bytes)
        .await
        .unwrap();
    seal(file_system, task, path, 0o444).await;
}

async fn run(standard: &Standard<'static>, arguments: &[&str]) -> Result<(), usize> {
    wasm::execute_with_control(
        standard.duplicate().await.unwrap(),
        arguments
            .iter()
            .map(|argument| argument.to_string())
            .collect(),
        Arc::new(wasm::ExecutionControl::new()),
    )
    .await
    .map_err(|status| status.get())
}

async fn read(
    file_system: &VirtualFileSystem,
    task: TaskIdentifier,
) -> Result<Vec<u8>, IntegrityError> {
    integrity::read_package_executable(file_system, task, IDENTIFIER, EXECUTABLE).await
}

#[xila::task::test(task_path = xila::task)]
async fn valid_package_executable_is_verified_and_runs() {
    let standard = testing::initialize(false, false).await;
    let file_system = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    let binary = guest();
    install(
        file_system,
        task,
        Some(&manifest_for(IDENTIFIER, EXECUTABLE, &binary)),
        &binary,
    )
    .await;

    assert_eq!(read(file_system, task).await.unwrap(), binary);
    assert_eq!(run(&standard, &[PATH]).await, Ok(()));
    // Developer mode never changes how an installed executable is treated.
    assert_eq!(run(&standard, &["--developer-mode", PATH]).await, Ok(()));
}

#[xila::task::test(task_path = xila::task)]
async fn tampered_truncated_or_overlong_executables_are_refused() {
    let standard = testing::initialize(false, false).await;
    let file_system = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    let binary = guest();
    install(
        file_system,
        task,
        Some(&manifest_for(IDENTIFIER, EXECUTABLE, &binary)),
        &binary,
    )
    .await;

    let mut tampered = binary.clone();
    *tampered.last_mut().unwrap() ^= 0x01;
    let mut overlong = binary.clone();
    overlong.push(0);
    let truncated = &binary[..binary.len() - 1];
    for (bytes, expected) in [
        (tampered.as_slice(), IntegrityError::DigestMismatch),
        (overlong.as_slice(), IntegrityError::SizeMismatch),
        (truncated, IntegrityError::SizeMismatch),
        (&[][..], IntegrityError::SizeMismatch),
    ] {
        overwrite(file_system, task, PATH, bytes).await;
        assert_eq!(read(file_system, task).await, Err(expected));
        assert_eq!(run(&standard, &[PATH]).await, Err(1));
        assert_eq!(run(&standard, &["--developer-mode", PATH]).await, Err(1));
    }

    // Swapping in another valid module with a matching size still fails.
    let other = wat::parse_str(
        r#"(module
          (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
          (func (export "_start") i32.const 1 call $exit))"#,
    )
    .unwrap();
    assert_eq!(other.len(), binary.len());
    overwrite(file_system, task, PATH, &other).await;
    assert_eq!(
        read(file_system, task).await,
        Err(IntegrityError::DigestMismatch)
    );

    overwrite(file_system, task, PATH, &binary).await;
    assert_eq!(run(&standard, &[PATH]).await, Ok(()));
}

#[xila::task::test(task_path = xila::task)]
async fn missing_invalid_or_mismatched_manifests_are_refused() {
    let standard = testing::initialize(false, false).await;
    let file_system = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    let binary = guest();
    install(file_system, task, None, &binary).await;

    assert_eq!(
        read(file_system, task).await,
        Err(IntegrityError::MissingManifest)
    );
    assert_eq!(run(&standard, &[PATH]).await, Err(1));
    assert_eq!(run(&standard, &["--developer-mode", PATH]).await, Err(1));

    let manifest_path = format!("{BUNDLE}/{MANIFEST_PATH}");
    let cases: [(Vec<u8>, IntegrityError); 3] = [
        (b"{".to_vec(), IntegrityError::InvalidManifest),
        (
            manifest_for("org.example.other", EXECUTABLE, &binary),
            IntegrityError::PackageMismatch,
        ),
        (
            manifest_for(IDENTIFIER, "executables/other.wasm", &binary),
            IntegrityError::NotDeclared,
        ),
    ];
    for (manifest, expected) in cases {
        match File::write_to_path(file_system, task, manifest_path.as_str(), &manifest).await {
            Ok(()) => {}
            Err(_) => overwrite(file_system, task, &manifest_path, &manifest).await,
        }
        assert_eq!(read(file_system, task).await, Err(expected));
        assert_eq!(run(&standard, &[PATH]).await, Err(1));
    }

    // A manifest that a non-root user may rewrite is not authoritative.
    overwrite(
        file_system,
        task,
        &manifest_path,
        &manifest_for(IDENTIFIER, EXECUTABLE, &binary),
    )
    .await;
    assert_eq!(read(file_system, task).await.unwrap(), binary);
    seal(file_system, task, &manifest_path, 0o666).await;
    assert_eq!(
        read(file_system, task).await,
        Err(IntegrityError::Unprotected)
    );
    assert_eq!(run(&standard, &[PATH]).await, Err(1));
}

#[xila::task::test(task_path = xila::task)]
async fn unmanaged_executables_require_developer_mode() {
    let standard = testing::initialize(false, false).await;
    let file_system = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    let path = "/temporary/development.wasm";
    File::write_to_path(file_system, task, path, &guest())
        .await
        .unwrap();

    assert_eq!(run(&standard, &[path]).await, Err(1));
    assert_eq!(run(&standard, &["--developer-mode", path]).await, Ok(()));

    // Aliases of installed paths and interrupted installations never run.
    for refused in [
        "/applications/bundles/./org.example.integrity/executables/application.wasm",
        "/applications//bundles/org.example.integrity/executables/application.wasm",
        "/applications/bundles/.incoming-1/executables/application.wasm",
    ] {
        assert_eq!(
            run(&standard, &["--developer-mode", refused]).await,
            Err(1),
            "{refused}"
        );
    }
}
