embedded_i18n::i18n!("locales");

#[cfg(any(target_arch = "wasm32", test))]
mod engine;

#[cfg(target_arch = "wasm32")]
mod interface;

#[cfg(target_arch = "wasm32")]
fn main() {
    let mut calculator_gui = interface::Interface::new().unwrap();
    unsafe {
        calculator_gui.run().unwrap();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    println!("{}", embedded_i18n::t!("errors.requires_wasm_runtime"));
}
