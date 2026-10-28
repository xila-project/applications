use example_counter_cli::handle_command;
use example_counter_core::Counter;

fn main() {
    let mut counter = Counter::default();
    for command in std::env::args().skip(1) {
        if handle_command(&mut counter, &command).is_err() {
            eprintln!("unknown command: {command}; use add, subtract, or reset");
            std::process::exit(2);
        }
    }
    println!("{}", counter.value());
}
