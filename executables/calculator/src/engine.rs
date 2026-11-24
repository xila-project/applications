use embedded_i18n::t;
use nanoexpr::{Context, ErrorKind, Evaluator, Exact, Functions, Math, Number, PercentSign};

type CalculatorNumber = Exact<14, 99>;

pub fn evaluate_expression(input: &str, radian_mode: bool) -> Result<CalculatorNumber, String> {
    let functions = Functions(|name: &str, arguments: &[CalculatorNumber]| match name {
        "sqr" | "cube" | "pow10" | "inv" | "sin" | "cos" | "tan" if arguments.len() != 1 => {
            Some(Err(ErrorKind::ArgumentCount))
        }
        "sqr" | "cube" | "pow10" | "inv" | "sin" | "cos" | "tan" => {
            let value = arguments[0];
            match name {
                "sqr" => Some(value.multiply(value)),
                "cube" => Some(
                    value
                        .multiply(value)
                        .and_then(|square| square.multiply(value)),
                ),
                "pow10" => {
                    CalculatorNumber::parse("10").and_then(|ten| Math.call("pow", &[ten, value]))
                }
                "inv" => Some(CalculatorNumber::from_bool(true).and_then(|one| one.divide(value))),
                "sin" | "cos" | "tan" if !radian_mode => {
                    let pi: CalculatorNumber =
                        Math.variable("pi").expect("nanoexpr Math defines pi");
                    let degrees_to_radians =
                        pi.divide(CalculatorNumber::parse("180").expect("180 is representable"));
                    degrees_to_radians
                        .and_then(|factor| value.multiply(factor))
                        .ok()
                        .and_then(|radians| Math.call(name, &[radians]))
                }
                "sin" | "cos" | "tan" => None,
                _ => None,
            }
        }
        _ => None,
    });
    let context = (functions, Math);
    Evaluator::<_, 4>::new(&context)
        .percent_sign(PercentSign::Percentage)
        .implicit_multiplication(true)
        .evaluate::<CalculatorNumber>(input)
        .map_err(localize_error)
}

/// A decimal expression for a uniformly distributed value in `[0, 1)` with ten fractional
/// digits, made from the top bits of `hash` by integer scaling so that no floating-point
/// formatting code is linked into the guest. The result is truncated, so it never reaches 1.
pub fn random_fraction(hash: u64) -> String {
    let fraction = ((u128::from(hash) * 10_000_000_000) >> 64) as u64;
    format!("0.{fraction:010}")
}

fn localize_error(error: nanoexpr::Error) -> String {
    let message = match error.kind {
        ErrorKind::UnexpectedCharacter => t!("errors.unexpected_character"),
        ErrorKind::UnexpectedToken => t!("errors.unexpected_token"),
        ErrorKind::UnexpectedEnd => t!("errors.unexpected_end_of_expression"),
        ErrorKind::UnclosedParenthesis => t!("errors.unclosed_parenthesis"),
        ErrorKind::InvalidNumber => t!("errors.invalid_number"),
        ErrorKind::UnknownVariable => t!("errors.unknown_variable"),
        ErrorKind::UnknownFunction => t!("errors.unknown_function"),
        ErrorKind::TooManyArguments => t!("errors.too_many_arguments"),
        ErrorKind::ArgumentCount => t!("errors.wrong_number_of_arguments"),
        ErrorKind::DivisionByZero => t!("errors.division_by_zero"),
        ErrorKind::Domain => t!("errors.value_outside_function_domain"),
        ErrorKind::Overflow => t!("errors.calculation_overflow"),
        ErrorKind::TooDeep => t!("errors.expression_nested_too_deeply"),
        ErrorKind::Unsupported => t!("errors.unsupported_operation"),
        ErrorKind::TooLong => t!("errors.expression_too_long"),
        ErrorKind::Inexact => t!("errors.inexact_result"),
        _ => t!("errors.calculation_error"),
    };
    format!("{message} ({}..{})", error.span.start, error.span.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_exact_fraction_results() {
        assert_eq!(
            evaluate_expression("0.1 + 0.2", true).unwrap().to_string(),
            "3/10"
        );
        assert_eq!(
            evaluate_expression("1 / 3 + 1 / 6", true)
                .unwrap()
                .to_string(),
            "1/2"
        );
        assert_eq!(evaluate_expression("5!", true).unwrap().to_string(), "120");
    }

    #[test]
    fn preserves_calculator_function_buttons_and_percent() {
        assert_eq!(
            evaluate_expression("sqr(5)", true).unwrap().to_string(),
            "25"
        );
        assert_eq!(
            evaluate_expression("cube(3)", true).unwrap().to_string(),
            "27"
        );
        assert_eq!(
            evaluate_expression("pow10(2)", true).unwrap().to_string(),
            "100"
        );
        assert_eq!(
            evaluate_expression("inv(4)", true).unwrap().to_string(),
            "1/4"
        );
        assert_eq!(evaluate_expression("25%", true).unwrap().to_string(), "1/4");
    }

    #[test]
    fn falls_back_to_ti_precision_decimal_for_irrational_results() {
        let result = evaluate_expression("sqrt(2)", true).unwrap();
        assert!(!result.is_exact());
        assert_eq!(result.to_string(), "1.4142135623731");
    }

    #[test]
    fn converts_degrees_without_losing_decimal_precision() {
        let result = evaluate_expression("sin(90)", false).unwrap();
        assert_eq!(result.to_string(), "1");
    }

    #[test]
    fn reports_localized_errors_with_the_input_span() {
        assert_eq!(
            evaluate_expression("1 / 0", true).unwrap_err(),
            format!("{} (0..5)", t!("errors.division_by_zero"))
        );
        assert_eq!(
            evaluate_expression("sqrt(-1)", true).unwrap_err(),
            format!("{} (0..8)", t!("errors.value_outside_function_domain"))
        );
    }

    #[test]
    fn random_fractions_stay_in_the_unit_interval() {
        assert_eq!(random_fraction(0), "0.0000000000");
        assert_eq!(random_fraction(u64::MAX), "0.9999999999");
        assert_eq!(random_fraction(1 << 63), "0.5000000000");
        assert_eq!(random_fraction(u64::MAX / 10), "0.0999999999");
        assert_eq!(random_fraction(1 << 40), "0.0000000596");
        for hash in [0, 1, u64::MAX - 1, u64::MAX, 0xdead_beef_cafe_f00d] {
            let text = random_fraction(hash);
            assert_eq!(text.len(), 12);
            assert!(evaluate_expression(&text, true).is_ok());
        }
    }

    #[test]
    fn displays_fractions_signs_and_large_or_small_values_without_floating_point() {
        let display = |input: &str| evaluate_expression(input, true).unwrap().to_string();
        assert_eq!(display("1/3"), "1/3");
        assert_eq!(display("-1/3"), "-1/3");
        assert_eq!(display("1e20"), "1e20");
        assert_eq!(display("-1e-7"), "-1/10000000");
        assert_eq!(display("0/5"), "0");
        assert_eq!(display("-0"), "0");
        assert_eq!(display("sqrt(2) * -1"), "-1.4142135623731");
    }
}
