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

For an optional approach to sharing application behavior between graphical and command-line frontends, see [Application cores and frontends](examples/application-architecture/README.md).

Package manifests live in `packaging/`. The package-manager workflow produces `.xpk` archives and SHA-256 files from the WASI build; pull requests validate artifacts without publishing them. A version tag matching all application versions can publish release assets. These archives are unsigned development packages: their checksums establish content integrity, not publisher identity. ABI version 1 denotes the initial versioned application ABI; older unversioned runtimes do not declare that compatibility.

CI uses separate mr-boxington target caches for host and WASI builds. On the first host Clippy run, mr-boxington reported 0 hits, 953 compilations without usable prior inputs or predictions, 26 bypasses and 0 seconds of estimated compiler work saved. This was a cold run; it does not establish warm-build reuse. Check subsequent job diagnostics and Cargo's build output before drawing cache conclusions.

`mise run localization:lint` checks application catalogs, placeholders, plural forms and semantic IDs. Tolgee keeps the existing `libraries/application_metadata` namespace for the moved shortcuts catalog; use `mise run translate:pull` to place that namespace in `libraries/application_shortcuts/locales` after a pull. No translation synchronization is performed automatically by CI.

Japanese (`ja`) catalogs are experimental and require native-speaker and
on-device layout review. Use `EMBEDDED_I18N_LOCALE=ja` and
`EMBEDDED_I18N_STRICT=1` for the guest build. A localized graphical runtime
must also collect these catalogs with `XILA_FONT_CATALOG_ROOTS` and use a Core
revision with Japanese font subsetting; see Core's embedded-font documentation
for the optional, verified Japanese source font. Catalog subsetting does not
provide arbitrary Japanese input or IME support.
