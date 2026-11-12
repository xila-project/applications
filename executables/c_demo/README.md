# Native C demo

This sample is linked into the native firmware; it is not an installable Wasm
package. Platforms mounts `CDemoExecutable` at
`/applications/executables/c_demo`. From its terminal, log in and run:

```text
/applications/executables/c_demo hello world
```

The Rust entry receives the current executable task's identity, provisions its
user home using the VFS convention, and grants READ | WRITE on that directory.
It moves standard input/output/error into ABI file objects, initializes the ABI
clock service and calls the unique `c_demo_main` C entry through `run_main`.
The entry borrows these objects; only the file it opens itself is closed in C.
`c-demo.txt` in the user's home is overwritten on each run.

`build.rs` uses `cc` and the generated ABI header supplied through
`DEP_XILA_ABI_APPLICATION_INCLUDE`. C11 warnings are errors. A desktop-target C
compiler is required. Linux, macOS and Windows build the implementation; all
other targets build an empty Rust crate and skip C compilation and host-only
dependencies. Applications' explicit WASI guest build list is unchanged. The
sample uses libc only for formatting, not filesystem or clock access.

While the Core startup API is unmerged, use Platforms' local-override generator:

```sh
python3 ../platforms/tools/dev/local_overrides.py --core ../core \
  --platforms ../platforms --applications . --output ../../target/work/overrides.toml
cargo clippy --config ../../target/work/overrides.toml -p xila-c-demo --all-targets -- -D warnings
```

Do not commit generated patches or the resulting local-path lockfile. After Core
merges, refresh the repository lockfile from Core main before merging this app;
then refresh Platforms' lockfile after Applications merges.
