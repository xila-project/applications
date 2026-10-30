use crate::engine::evaluate_expression;
use embedded_i18n::translate;
use xila_application::Application;

#[derive(Default)]
pub struct CalculatorModel {
    expression: String,
    result: bool,
    error: bool,
    degrees: bool,
}

pub struct CalculatorLogic;

pub enum Message {
    Append(&'static str),
    Character(char),
    Operator(char),
    Random(String),
    Clear,
    Backspace,
    ToggleAngle,
    Evaluate,
}

pub struct View<'a> {
    pub display: &'a str,
    pub error: bool,
    pub radians: bool,
}

impl Application for CalculatorLogic {
    type Model = CalculatorModel;
    type Message = Message;
    type Error = core::convert::Infallible;
    type Effect = ();
    type View<'a> = View<'a>;

    fn update(&mut self, model: &mut CalculatorModel, message: Message) -> Result<(), Self::Error> {
        match message {
            Message::Clear => {
                model.error = false;
                model.expression.clear();
                model.result = false;
            }
            Message::Backspace => {
                model.error = false;
                model.expression.pop();
                model.result = false;
            }
            Message::ToggleAngle => model.degrees = !model.degrees,
            Message::Evaluate if !model.expression.is_empty() => {
                model.expression = match evaluate_expression(&model.expression, !model.degrees) {
                    Ok(result) => {
                        model.error = false;
                        result.to_string()
                    }
                    Err(error) => {
                        model.error = true;
                        format!(translate!("errors.calculation_with_details"), error)
                    }
                };
                model.result = true;
            }
            Message::Evaluate => {}
            Message::Operator(ch) => {
                model.error = false;
                model.result = false;
                model.expression.push(ch);
            }
            Message::Character(ch) => {
                model.error = false;
                if model.result {
                    model.expression.clear();
                    model.result = false;
                }
                model.expression.push(ch);
            }
            Message::Append(text) => {
                model.error = false;
                if model.result {
                    model.expression.clear();
                    model.result = false;
                }
                model.expression.push_str(text);
            }
            Message::Random(text) => {
                model.error = false;
                if model.result {
                    model.expression.clear();
                    model.result = false;
                }
                model.expression.push_str(&text);
            }
        }
        Ok(())
    }

    fn view<'a>(&'a self, model: &'a CalculatorModel) -> View<'a> {
        View {
            display: if model.expression.is_empty() {
                "0"
            } else {
                &model.expression
            },
            error: model.error,
            radians: !model.degrees,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xila_application::Application;
    #[test]
    fn expressions_restart_after_result_but_operators_continue_it() {
        let mut model = CalculatorModel::default();
        let mut logic = CalculatorLogic;
        for message in [
            Message::Character('2'),
            Message::Operator('+'),
            Message::Character('3'),
            Message::Evaluate,
        ] {
            logic.update(&mut model, message).unwrap();
        }
        assert_eq!(logic.view(&model).display, "5");
        logic.update(&mut model, Message::Operator('*')).unwrap();
        logic.update(&mut model, Message::Character('2')).unwrap();
        logic.update(&mut model, Message::Evaluate).unwrap();
        assert_eq!(logic.view(&model).display, "10");
        logic.update(&mut model, Message::Character('7')).unwrap();
        assert_eq!(logic.view(&model).display, "7");
    }
    #[test]
    fn errors_clear_and_external_random_values_follow_editing_rules() {
        let mut model = CalculatorModel::default();
        let mut logic = CalculatorLogic;
        logic.update(&mut model, Message::Append("1/0")).unwrap();
        logic.update(&mut model, Message::Evaluate).unwrap();
        assert!(logic.view(&model).display.contains("Error"));
        assert!(logic.view(&model).error);
        logic
            .update(&mut model, Message::Random("0.125".to_string()))
            .unwrap();
        assert_eq!(logic.view(&model).display, "0.125");
        assert!(!logic.view(&model).error);
        logic.update(&mut model, Message::Clear).unwrap();
        assert_eq!(logic.view(&model).display, "0");
    }

    #[test]
    fn empty_display_and_mode_are_separate_from_graphics() {
        let mut model = CalculatorModel::default();
        let mut logic = CalculatorLogic;
        assert_eq!(logic.view(&model).display, "0");
        assert!(logic.view(&model).radians);
        logic.update(&mut model, Message::ToggleAngle).unwrap();
        assert!(!logic.view(&model).radians);
        logic.update(&mut model, Message::Character('9')).unwrap();
        logic.update(&mut model, Message::Backspace).unwrap();
        assert_eq!(logic.view(&model).display, "0");
    }
}
