# Applications development

Preserve the xila-wasm host runtime, guest API, ABI translation tests, and guest and native application behavior, installation paths and translation IDs. Guest dependencies on `xila-wasm` must disable default host features. Use `mise run check:guest`, `mise run build:applications` and targeted host tests. The platform test harness is a development dependency; do not introduce duplicate Core crate identities or production dependencies on Platforms. Graphics host builds require `npm ci` and the pinned converter configured in `.cargo/config.toml`.

- **Rust version.** Pin Rust 1.97.0 for development and CI and declare 1.97 as the MSRV (`rust-version`), following the esp-rs Xtensa toolchain base. Use only language features and `std` or `core` APIs available in Rust 1.97. Raise the version only when esp-rs publishes a new base, and update every Xila repository together. Rust native executables that use `build-std` use a separate nightly toolchain (xila-project/core#687).

## Releases

Applications version independently and release signed `.xpk` bundles under
`dev.xila.<name>-vX.Y.Z`. Before 1.0, a breaking change bumps the minor version and
compatible changes bump the patch. Each bundle declares its minimum Xila
version as `major.minor`. Before Xila 1.0, the running and declared minor
versions must match. From 1.0 onward, the major versions must match and the
running minor must be at least the declared minor. Rebuild and republish all
bundles for each Xila 0.x minor release. The Wasm guest SDK follows the Xila
version.

See [the Xila release policy](https://github.com/xila-project/core/issues/276).
