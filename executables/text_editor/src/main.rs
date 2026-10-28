embedded_i18n::track_locales!();

#[cfg(target_arch = "wasm32")]
mod interface;
#[cfg(any(target_arch = "wasm32", test))]
mod storage;

#[cfg(target_arch = "wasm32")]
fn main() {
    if let Err(error) = interface::run() {
        eprintln!("Text Editor graphics error: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    println!("Text Editor is a Xila Wasm application. Build for wasm32-wasip1.");
}
