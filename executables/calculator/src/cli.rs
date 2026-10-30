//! Minimal native or WASI command-line frontend for the shared calculator model.
use xila_application::Application;
use xila_calculator::model::{CalculatorLogic, CalculatorModel, Message};

fn main() {
    let Some(expression) = std::env::args().nth(1) else {
        eprintln!("usage: calculator-cli EXPRESSION");
        std::process::exit(2);
    };
    let mut model = CalculatorModel::default();
    let mut logic = CalculatorLogic;
    for character in expression.chars() {
        let _ = logic.update(&mut model, Message::Character(character));
    }
    let _ = logic.update(&mut model, Message::Evaluate);
    let view = logic.view(&model);
    if view.error {
        eprintln!("{}", view.display);
        std::process::exit(1);
    }
    println!("{}", view.display);
}
