# Xila Applications

Official Xila applications live in this workspace. The guest binaries are calculator, clock, pixel painter, text editor, synth and weather; the other crates are native applications linked into a platform runtime. `xila-wasm` remains the Core host runtime and guest API. Core and the Platforms test harness are pinned to Git revisions in `Cargo.toml`.

Install [mise](https://mise.jdx.dev/) and the native development packages listed in the Core development instructions. `mise run build:applications` produces the Wasm guests in `target/wasm32-wasip1/release-wasm`. `mise run check:guest` checks the guest target. `mise run test:workspace` runs host tests; install Node.js 24 and run `npm ci` first for Core's graphics font converter. The Platforms seed generator accepts the built binaries through its `--applications-dir` option.

This branch is part of a coordinated migration. The package publication workflow, local multi-repository overrides, and full integration and deployment checks are being completed before the migration can be considered ready.
