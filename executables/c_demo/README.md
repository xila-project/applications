# Native C demo

This sample is linked into the native firmware; it is not an installable Wasm
package. Platforms mounts `CDemoExecutable` at
`/applications/executables/c_demo`. From its terminal, log in and run:

```text
/applications/executables/c_demo hello world
```

The Rust entry is the `native-linked` runner of `xila-executable`:
`executable::run_linked` starts the unique `c_demo_main` C entry as a full
native process on a dedicated stackful coroutine of the calling task (64 KiB on
desktop hosts, 16 KiB on bare metal), with the process capability, the standard
streams of the caller and READ | WRITE on the user's home. Dropping the future
cancels the program safely, even while it is suspended in a blocking call: the
coroutine stack is freed and the host state closes every handle the program
still held. The entry checks the startup and API table headers, calls through
`startup->api`, and closes the file it opens. It leaves borrowed capabilities
with the host. `c-demo.txt` in the user's home is overwritten on each run.

`build.rs` uses `cc` and the generated ABI header supplied through
`DEP_XILA_ABI_APPLICATION_INCLUDE`. C11 warnings are errors. A target C
compiler is required. Linux, macOS, Windows, and bare-metal targets supported
by `xila-stackful` (`x86_64`, `aarch64`, `arm`, `riscv32`, `xtensa`) compile
the crate, C entry included; Wasm targets build an empty Rust crate and skip C
compilation and native-linked dependencies. The `native-linked` runner itself
needs stack switching, so `CDemoExecutable` exists on Linux (x86-64, AArch64)
and those bare-metal targets; Applications' explicit WASI guest build list is
unchanged. The sample is freestanding C11: it uses no host libc (`stdio.h`,
`string.h`), only tiny formatting helpers, so filesystem and clock access all
go through Xila's C ABI.
