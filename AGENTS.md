# Applications development

Preserve guest and native application behavior, installation paths and translation IDs. Guest dependencies on `xila-wasm` must disable default host features. Use `mise run check:guest`, `mise run build:applications` and targeted host tests. The platform test harness is a development dependency; do not introduce duplicate Core crate identities or production dependencies on Platforms. Graphics host builds require `npm ci` and the pinned converter configured in `.cargo/config.toml`.
