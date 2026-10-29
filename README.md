# Xila Applications

Official Xila applications live in this workspace. The guest binaries are calculator, clock, pixel painter, text editor, synth and weather; the other crates are native applications linked into a platform runtime. `xila-wasm` provides the host runtime and guest API in this workspace; Platforms selects its host feature when assembling runtimes. Core and the Platforms test harness are pinned to Git revisions in `Cargo.toml`.

Install [mise](https://mise.jdx.dev/) and the native development packages listed in the Core development instructions. `mise run build:applications` produces the Wasm guests in `target/wasm32-wasip1/release-wasm`. `mise run check:guest` checks the guest target. `mise run test:workspace` runs host tests; install Node.js 24 and run `npm ci` first for Core's graphics font converter. The Platforms seed generator accepts the built binaries through its `--applications-dir` option.

This branch is part of a coordinated migration. The package publication workflow, local multi-repository overrides, and full integration and deployment checks are being completed before the migration can be considered ready.

For adjacent local checkouts, generate a Cargo config without changing the committed lockfile permanently:

```sh
python3 tools/local_overrides.py --core ../core --platforms ../platforms --output /tmp/xila-local.toml
cargo metadata --config /tmp/xila-local.toml --format-version 1
cargo test --config /tmp/xila-local.toml -p xila-command-line-shell --test integration_test --features xila/graphics_rendering_xrgb8888
```

Cargo records patched paths in `Cargo.lock`; regenerate the lockfile without the local config before committing. Use the same Core checkout for all patches, including transitive platform dependencies.

Editable v1 developer package manifests live in `packaging/`. These contain application identity, entrypoint and ABI but **no generated hashes**. The reusable package-manager CI workflow stages each compiled WASI binary, builds and verifies a canonical `.app` bundle, and archives it as `.xpk` with a SHA-256 sidecar. Pull requests test the new bundle format without publishing them. A version tag matching all application versions can publish release assets. These archives are unsigned development packages: their checksums establish content integrity, not publisher identity. ABI version 1 denotes the initial versioned application ABI; older unversioned runtimes do not declare that compatibility.

CI uses separate mr-boxington target caches for host and WASI builds. On the first host Clippy run, mr-boxington reported 0 hits, 953 compilations without usable prior inputs or predictions, 26 bypasses and 0 seconds of estimated compiler work saved. This was a cold run; it does not establish warm-build reuse. Check subsequent job diagnostics and Cargo's build output before drawing cache conclusions.

`mise run localization:lint` checks application catalogs, placeholders, plural forms and semantic IDs. Tolgee keeps the existing `libraries/application_metadata` namespace for the moved shortcuts catalog; use `mise run translate:pull` to place that namespace in `libraries/application_shortcuts/locales` after a pull. No translation synchronization is performed automatically by CI.

## Native package manager

The `executables/package` crate implements the **native Xila CLI-only** command `package install` / `package list` (registered in the native Platforms example), with bounded-memory TAR + SHA-256 verification at **installation**, not at launch. It trusts `/configuration/system/packages/trusted.pub` by default, accepts an explicitly supplied `--public-key`, and has an explicit `--allow-untrusted` override for unknown/unsigned publishers. Even with that override, invalid TARs or SHA-256 mismatches always fail. See [package CLI documentation](executables/package/README.md). CLI installation does not yet create launcher shortcuts, manage updates or download packages.
