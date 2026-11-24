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
//! | custom page sizes       | enabled  | bundled guests use 1-byte pages to shrink initial memory |
//! | wide arithmetic         | disabled | no toolchain emits it                            |
//! | threads / atomics       | unsupported by Wasmi                                       |
//!
//! # Translation
//!
//! Guests are always validated in full before anything is instantiated, so
//! only Wasmi's validator ever sees bytes that were not accepted. What
//! [`Translation`] chooses is when each function body is translated to
//! Wasmi's internal code: all of them while loading ([`Translation::Eager`]),
//! or each one at its first call ([`Translation::Lazy`]). Lazy translation
//! lowers the load peak and the resident size of guests that never call most
//! of their code, at the cost of a translation pause on each first call.
//!
//! Translation charges no fuel (see [`configuration`]), so it runs to its end
//! without yielding to the executor. The pause at a first call is bounded:
//!
//! - it translates one function body, in a single pass roughly linear in the
//!   size of that body;
//! - a body is never larger than the module, which is read whole into memory
//!   before loading, so the available heap caps it (Wasmi 2.0 has no limit
//!   on the size of one body);
//! - each function is translated at most once, so a guest cannot repeat it.
//!
//! All lazy pauses together never do more work than eager translation does in
//! one go inside `Module::new`, which never yields either. A guest therefore
//! cannot hold the executor any longer than loading it eagerly already does.

use getargs_derive::ValueEnum;
use wasmi::{CompilationMode, Config, CustomFuelCosts};

/// When function bodies are translated to Wasmi's internal code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Translation {
    /// Translate every function while the module loads.
    Eager,
    /// Translate each function at its first call.
    Lazy,
}

impl Translation {
    /// The mode used when the command line does not choose one: lazy when the
    /// `lazy_translation` feature is enabled, so a target can make it its
    /// default, and eager otherwise.
    pub const DEFAULT: Self = if cfg!(feature = "lazy_translation") {
        Self::Lazy
    } else {
        Self::Eager
    };

    fn compilation_mode(self) -> CompilationMode {
        match self {
            Self::Eager => CompilationMode::Eager,
            // Validation stays eager: a module is accepted or refused as a
            // whole before it is instantiated, never halfway through a run.
            Self::Lazy => CompilationMode::LazyTranslation,
        }
    }
}

/// Wasmi's default for copying bytes (`memory.copy`, `table.grow`, ...).
const BYTES_COPIED_PER_FUEL: u32 = 64;

/// Engine configuration shared by every guest instance.
pub fn configuration(translation: Translation) -> Config {
    let mut configuration = Config::default();
    configuration.consume_fuel(true);
    configuration.compilation_mode(translation.compilation_mode());
    // Fuel is a scheduling slice here, not a budget. Wasmi charges lazy
    // translation to the running guest's fuel, and in Wasmi 2.0 running out of
    // fuel while translating a callee is a plain error rather than a resumable
    // pause (https://github.com/wasmi-labs/wasmi/pull/2055): the caller has
    // already run, so the guest cannot be restarted either. Translation is
    // bounded by the module size and happens once per function, so charge
    // nothing for it and the slice can never end inside a translation.
    configuration.fuel_cost(CustomFuelCosts {
        bytes_copied_per_fuel: BYTES_COPIED_PER_FUEL,
        fuel_per_bytes_translated: 0,
        fuel_per_bytes_validated: 0,
    });
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
        .wasm_custom_page_sizes(true)
        .wasm_wide_arithmetic(false);
    #[cfg(feature = "memory_64")]
    configuration.wasm_memory64(true);
    configuration
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::{Translation, configuration};
    use wasmi::{Engine, Linker, Module, Store};

    /// Validation does not depend on the translation mode: both must agree.
    fn validates(text: &str) -> bool {
        let bytes = wat::parse_str(text).unwrap();
        let eager = Module::new(&Engine::new(&configuration(Translation::Eager)), &bytes[..]);
        let lazy = Module::new(&Engine::new(&configuration(Translation::Lazy)), &bytes[..]);
        assert_eq!(eager.is_ok(), lazy.is_ok(), "{text}");
        eager.is_ok()
    }

    #[test]
    fn lazy_translation_still_validates_every_function_body() {
        // The invalid body is never called, but the module is still refused.
        assert!(!validates(
            "(module (func (export \"_start\")) (func (result i32) (i64.const 0)))"
        ));
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
    fn byte_pages_grow_in_bytes_up_to_the_declared_maximum() {
        let binary = wat::parse_str(
            r#"(module
                (memory (export "memory") 12 16 (pagesize 1))
                (data (i32.const 11) "x")
                (func (export "size") (result i32) memory.size)
                (func (export "grow") (param i32) (result i32) local.get 0 memory.grow))"#,
        )
        .unwrap();

        for translation in [Translation::Eager, Translation::Lazy] {
            let engine = Engine::new(&configuration(translation));
            let module = Module::new(&engine, &binary[..]).unwrap();
            let mut store = Store::new(&engine, ());
            store.set_fuel(10_000).unwrap();
            let instance = Linker::new(&engine)
                .instantiate_and_start(&mut store, &module)
                .unwrap();
            let size = instance.get_typed_func::<(), i32>(&store, "size").unwrap();
            let grow = instance.get_typed_func::<i32, i32>(&store, "grow").unwrap();
            let memory = instance.get_memory(&store, "memory").unwrap();

            assert_eq!(size.call(&mut store, ()).unwrap(), 12);
            assert_eq!(grow.call(&mut store, 4).unwrap(), 12);
            assert_eq!(grow.call(&mut store, 1).unwrap(), -1);
            assert_eq!(size.call(&mut store, ()).unwrap(), 16);
            assert_eq!(&memory.data(&store)[11..], b"x\0\0\0\0");
        }
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
