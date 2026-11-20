use std::path::Path;

use graphics_bindgen::{enumeration, guest, guest_constants, registry::GraphicsRegistry};
use quote::quote;

use crate::utilities::file::write_generated_rust;

pub fn generate(output_path: &Path, registry: &GraphicsRegistry<'_>, bindings: &str) {
    let calls = guest::calls(registry);
    let enumeration = enumeration::generate_calls(registry.calls());
    let constants = guest_constants::generate(bindings).expect("invalid guest constants");
    let enumeration_path = output_path.join("enumeration.generated.rs");
    write_generated_rust(&enumeration_path, quote! { #enumeration #constants });

    let types_path = output_path.join("types.generated.rs");
    std::fs::write(&types_path, guest::TYPES_SOURCE).expect("failed to write guest types");

    // Core's safety comments also name the native transport.
    let functions = guest::rust_functions(&calls).to_string().replace(
        "Pointers must be valid for the call, and the call must run in a native process.",
        "Pointers must be valid for the call.",
    );
    write_generated_rust(
        &output_path.join("functions.generated.rs"),
        format!("use crate::prelude::*; use super::prelude::call; {functions}"),
    );

    #[cfg(feature = "c_bindings")]
    generate_c(output_path, &enumeration_path, &types_path, &calls);
}

#[cfg(feature = "c_bindings")]
fn generate_c(
    output_path: &Path,
    enumeration_path: &Path,
    types_path: &Path,
    calls: &[guest::GuestCall],
) {
    let functions = calls.iter().map(|call| {
        let function = call.c_function();
        let variant = &call.variant;
        let mut parameters: Vec<_> = call
            .inputs
            .iter()
            .map(|input| {
                let (name, ty) = (&input.name, &input.ty);
                quote! { #name: #ty }
            })
            .collect();
        let words = call.inputs.iter().map(|input| {
            let name = &input.name;
            match input.word {
                guest::Word::Bytes => quote! { as_usize(#name) },
                guest::Word::Number if input.ty == syn::parse_quote!(usize) => quote! { #name },
                guest::Word::Number => quote! { #name as usize },
            }
        });
        let result = match &call.output {
            Some(ty) => {
                parameters.push(quote! { __result_pointer: *mut #ty });
                quote! { __result_pointer.cast() }
            }
            None => quote! { core::ptr::null_mut() },
        };
        quote! {
            #[unsafe(no_mangle)]
            pub extern "C" fn #function(#(#parameters),*) -> i32 {
                unsafe { call_raw(FunctionCall::#variant, &[#(#words),*], #result) }
            }
        }
    });
    let functions_path = output_path.join("c_functions.generated.rs");
    write_generated_rust(
        &functions_path,
        quote! {
            use crate::prelude::*;
            use super::prelude::call_raw;
            #(#functions)*
        },
    );

    let configuration = cbindgen::Config {
        language: cbindgen::Language::C,
        include_guard: Some("__XILA_GRAPHICS_GENERATED_H_INCLUDED".into()),
        sys_includes: vec!["stdarg.h".into(), "stdbool.h".into(), "stdint.h".into()],
        no_includes: true,
        export: cbindgen::ExportConfig {
            prefix: Some(guest::C_PREFIX.into()),
            include: vec!["EventCode".into()],
            ..Default::default()
        },
        enumeration: cbindgen::EnumConfig {
            rename_variants: cbindgen::RenameRule::QualifiedScreamingSnakeCase,
            ..Default::default()
        },
        ..Default::default()
    };

    cbindgen::Builder::new()
        // Read the generated sources directly so host features stay disabled.
        .with_src(functions_path)
        .with_src(enumeration_path)
        .with_src(output_path.join("event_code.rs"))
        .with_src(types_path)
        .with_src(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/guest/graphics/prelude.rs"))
        .with_config(configuration)
        .generate()
        .expect("failed to generate xila_graphics.h")
        .write_to_file(output_path.join("xila_graphics.h"));

    println!("cargo:rerun-if-env-changed=XILA_CHECK_WASM_GRAPHICS_HEADER");
    if std::env::var_os("XILA_CHECK_WASM_GRAPHICS_HEADER").is_some() {
        let test = "tests/graphics_header.c";
        println!("cargo:rerun-if-changed={test}");
        let status = std::process::Command::new("clang")
            .args(["-std=c11", "-Werror", "-fsyntax-only", "-I"])
            .arg(output_path)
            .arg(test)
            .status()
            .expect("failed to run Clang for the Wasm graphics header check");
        assert!(status.success(), "Wasm graphics header check failed");
    }
}
