//! The WebAssembly feature set guests are validated against.
//!
//! The set is written out instead of inherited from Wasmi's defaults, so an
//! engine upgrade cannot silently widen or narrow what guests may use. The
//! enabled proposals are exactly what `rustc` and `clang` emit for
//! `wasm32-wasip1` by default (bulk memory, multi-value, mutable globals,
//! non-trapping float-to-int, reference types, sign extension), plus two that
//! cost nothing and that newer guest toolchains opt into (tail calls, extended
//! constants).
//!
//! | Proposal                | State    | Why                                              |
//! |-------------------------|----------|--------------------------------------------------|
//! | mutable globals         | enabled  | toolchain default (stack pointer, TLS)           |
//! | sign extension          | enabled  | toolchain default                                |
//! | non-trapping float→int  | enabled  | toolchain default                                |
//! | multi-value             | enabled  | toolchain default                                |
//! | bulk memory             | enabled  | toolchain default (`memcpy`/`memset`)            |
//! | reference types         | enabled  | toolchain default (tables, `externref`)          |
//! | tail calls              | enabled  | no cost; opt-in for guest toolchains             |
//! | extended constants      | enabled  | no cost; emitted by `-mcpu=lime1`                |
//! | memory64                | `memory_64` builds only | the host ABI has one address width |
//! | multi-memory            | disabled | the host ABI addresses the single `memory` export |
//! | SIMD, relaxed SIMD      | disabled | no guest uses it; the engine support is large    |
//! | custom page sizes       | disabled | no toolchain emits it                            |
//! | wide arithmetic         | disabled | no toolchain emits it                            |
//! | threads / atomics       | unsupported by Wasmi                                       |

use wasmi::{CompilationMode, Config};

/// Engine configuration shared by every guest instance.
pub(super) fn configuration() -> Config {
    let mut configuration = Config::default();
    configuration.consume_fuel(true);
    // Wasmi 2.0.0 cannot resume fuel exhaustion during lazy callee translation
    // (https://github.com/wasmi-labs/wasmi/pull/2032). Compile
    // before execution so even small scheduling slices can make progress.
    configuration.compilation_mode(CompilationMode::Eager);
    configuration
        .wasm_mutable_global(true)
        .wasm_sign_extension(true)
        .wasm_saturating_float_to_int(true)
        .wasm_multi_value(true)
        .wasm_bulk_memory(true)
        .wasm_reference_types(true)
        .wasm_tail_call(true)
        .wasm_extended_const(true)
        .wasm_multi_memory(false)
        .wasm_custom_page_sizes(false)
        .wasm_wide_arithmetic(false);
    #[cfg(feature = "memory_64")]
    configuration.wasm_memory64(true);
    configuration
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::configuration;
    use wasmi::{Engine, Module};

    fn validates(text: &str) -> bool {
        let bytes = wat::parse_str(text).unwrap();
        Module::new(&Engine::new(&configuration()), &bytes[..]).is_ok()
    }

    #[test]
    fn what_the_default_guest_toolchains_emit_is_accepted() {
        // bulk memory
        assert!(validates(
            "(module (memory 1) (data $d \"x\")
               (func (memory.copy (i32.const 0) (i32.const 1) (i32.const 2))
                     (memory.fill (i32.const 0) (i32.const 1) (i32.const 2))
                     (memory.init $d (i32.const 0) (i32.const 0) (i32.const 1))
                     (data.drop $d)))"
        ));
        // sign extension
        assert!(validates(
            "(module (func (param i32 i64) (result i32)
               (drop (i64.extend32_s (local.get 1))) (i32.extend8_s (local.get 0))))"
        ));
        // non-trapping float to int
        assert!(validates(
            "(module (func (param f64) (result i32) (i32.trunc_sat_f64_s (local.get 0))))"
        ));
        // multi-value
        assert!(validates(
            "(module (func $two (result i32 i32) (i32.const 1) (i32.const 2))
               (func (result i32) (call $two) (i32.add)))"
        ));
        // mutable globals, including an imported and an exported one
        assert!(validates(
            "(module (import \"env\" \"g\" (global (mut i32)))
               (global (export \"sp\") (mut i32) (i32.const 0)))"
        ));
        // reference types
        assert!(validates(
            "(module (table $t 2 externref) (table $f 1 funcref)
               (func (param externref) (table.set $t (i32.const 0) (local.get 0))
                     (drop (table.get $t (i32.const 1)))
                     (drop (ref.is_null (ref.null extern)))
                     (drop (table.grow $f (ref.null func) (i32.const 1)))))"
        ));
    }

    #[test]
    fn cost_free_extensions_are_accepted() {
        // tail calls
        assert!(validates(
            "(module (func $f (return_call $f)) (func (param funcref) (result i32) (i32.const 0)))"
        ));
        // extended constant expressions
        assert!(validates(
            "(module (global i32 (i32.add (i32.const 1) (i32.mul (i32.const 2) (i32.const 3)))))"
        ));
    }

    #[test]
    fn proposals_the_host_does_not_offer_are_rejected() {
        // SIMD
        assert!(!validates(
            "(module (func (result v128) (v128.const i32x4 0 0 0 0)))"
        ));
        // threads and atomics
        assert!(!validates(
            "(module (memory 1 1 shared) (func (result i32) (i32.atomic.load (i32.const 0))))"
        ));
        assert!(!validates(
            "(module (memory 1) (func (result i32) (i32.atomic.load (i32.const 0))))"
        ));
        // multi-memory
        assert!(!validates("(module (memory 1) (memory 1))"));
        // custom page sizes
        assert!(!validates("(module (memory 1 (pagesize 1)))"));
        // wide arithmetic
        assert!(!validates(
            "(module (func (param i64 i64) (result i64 i64)
               (i64.add128 (local.get 0) (local.get 1) (local.get 0) (local.get 1))))"
        ));
        // exception handling and garbage-collected structs
        assert!(!validates("(module (tag (param i32)))"));
        assert!(!validates("(module (type (struct (field i32))))"));
    }

    #[test]
    fn memory64_follows_the_guest_address_width() {
        let accepted = validates("(module (memory i64 1))");
        assert_eq!(accepted, cfg!(feature = "memory_64"));
    }
}
