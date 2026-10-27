# Xila Applications

Official Xila applications live in this workspace. The guest binaries are calculator, clock, pixel painter, text editor, synth and weather; the other crates are native applications linked into a platform runtime. `xila-wasm` remains the Core host runtime and guest API. Core and the Platforms test harness are pinned to Git revisions in `Cargo.toml`.

Install [mise](https://mise.jdx.dev/) and the native development packages listed in the Core development instructions. `mise run build:applications` produces the Wasm guests in `target/wasm32-wasip1/release-wasm`. `mise run check:guest` checks the guest target. `mise run test:workspace` runs host tests; install Node.js 24 and run `npm ci` first for Core's graphics font converter. The Platforms seed generator accepts the built binaries through its `--applications-dir` option.

This branch is part of a coordinated migration. The package publication workflow, local multi-repository overrides, and full integration and deployment checks are being completed before the migration can be considered ready.

For adjacent local checkouts, generate a Cargo config without changing the committed lockfile permanently:

```sh
python3 tools/local_overrides.py --core ../core --platforms ../platforms --output /tmp/xila-local.toml
cargo metadata --config /tmp/xila-local.toml --format-version 1
cargo test --config /tmp/xila-local.toml -p xila-command-line-shell --test integration_test --features xila/graphics_rendering_xrgb8888
```

Cargo records patched paths in `Cargo.lock`; regenerate the lockfile without the local config before committing. Use the same Core checkout for all patches, including transitive platform dependencies.

Package manifests live in `packaging/`. The package-manager workflow produces `.xpk` archives and SHA-256 files from the WASI build; pull requests validate artifacts without publishing them. A version tag matching all application versions can publish release assets. These archives are unsigned development packages: their checksums establish content integrity, not publisher identity. ABI version 1 denotes the initial versioned application ABI; older unversioned runtimes do not declare that compatibility.
