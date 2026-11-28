# Linked hexdump

Enable the `native-linked` feature of `xila-hexdump`. Mount
`hexdump::HexdumpExecutable` at `/applications/executables/hexdump` with the
firmware's executable mount helper. The crate uses the linked runner without
the native ELF loader. The program receives read and write access to the
user's home directory. The POSIX layer uses that directory
(`XilaStartup.home`) as its root and resolves absolute paths relative to it.
One instance can run at a time because the POSIX library uses shared state.
Keep `--gc-sections` in the firmware link flags.

Build the SDK from the same Core revision that the firmware uses. In the Core
checkout, set `XILA_SDK_TARGET` and run `mise run sdk:build`. Use
`xtensa-esp32s3-elf` for Rust's `xtensa-esp32s3-none-elf`. For the other
supported targets, use the Rust target name.

Set `XILA_SDK_DIRECTORY` to the absolute path of the built SDK directory before
building the firmware. Keep the SDK compiler and linker tools on `PATH`.
The crate uses Python 3 and Core's SDK build tool to compile the packaged
`hexdump.c` and link it with the SDK libraries. The archive exports only the
program's entry symbol.

The Applications lockfile uses an older Core revision. To build with
`native-linked`, use a firmware workspace that selects a Core revision with
SDK `link-example` support.

Declare Core with `branch = "main"`, the same Cargo source as Applications.
Record the selected commit in the consumer's lockfile. Declaring Core with
`rev` creates a separate Cargo source and causes a
`links = "xila_abi_application"` conflict.
