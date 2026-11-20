#[cfg(all(feature = "guest", feature = "graphics"))]
mod guest;
#[cfg(all(feature = "guest", feature = "graphics"))]
mod utilities;

fn main() {
    println!("cargo:rerun-if-changed=build/main.rs");
    #[cfg(all(feature = "guest", feature = "graphics"))]
    {
        println!("cargo:rerun-if-changed=build/guest.rs");
        println!("cargo:rerun-if-changed=build/utilities");
        println!("cargo:rerun-if-changed=src/guest/graphics/prelude.rs");
    }
    #[cfg(feature = "graphics")]
    generate();
}

/// Guest bindings come from Core's graphics ABI declaration, the same one
/// that generates the host executor's call table, so the two cannot disagree.
#[cfg(feature = "graphics")]
fn generate() {
    let output_path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    // The event contract is Core's source, shared with the executor.
    std::fs::write(
        output_path.join("event_code.rs"),
        graphics_bindgen::EVENT_CODE_SOURCE,
    )
    .expect("failed to write the event contract");
    #[cfg(feature = "guest")]
    {
        let catalog = graphics_bindgen::abi::catalog().expect("invalid LVGL graphics ABI");
        let registry = graphics_bindgen::abi::registry(&catalog)
            .unwrap_or_else(|error| panic!("invalid graphics registry: {error}"));
        std::fs::write(
            output_path.join("graphics_abi_inventory.json"),
            registry.inventory_json(&catalog),
        )
        .expect("failed to write graphics ABI inventory");
        guest::generate(&output_path, &registry);
    }
}
