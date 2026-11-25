embedded_i18n::i18n!("locales");

#[cfg(target_arch = "wasm32")]
mod interface;
#[cfg(target_arch = "wasm32")]
mod settings;
#[cfg(target_arch = "wasm32")]
mod widgets;

#[cfg(target_arch = "wasm32")]
fn main() {
    let mut interface = match interface::Interface::new() {
        Ok(interface) => interface,
        Err(error) => {
            std::eprintln!("Home Assistant could not create its window: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = interface.run() {
        std::eprintln!("Home Assistant stopped on a graphics error: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    println!("This executable is intended to be run in a WASM environment.");
}
