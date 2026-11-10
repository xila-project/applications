embedded_i18n::i18n!("locales");

#[cfg(target_arch = "wasm32")]
mod device;
#[cfg(target_arch = "wasm32")]
mod interface;

#[cfg(target_arch = "wasm32")]
fn main() {
    if let Err(error) = interface::run() {
        eprintln!("Digits graphics error: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    println!("Digits is a Xila Wasm application. Build for wasm32-wasip1.");
}
