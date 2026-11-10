//! Keypad layouts, rendered as theme-styled buttons on an LVGL grid.
//!
//! Each layout is a list of rows; a row shorter than the widest one widens its
//! first key (for example `0`). Each key gets a shadcn variant from its role,
//! so digits, operators, functions and `=` read as distinct groups. Wide windows
//! show the scientific and basic pads side by side; compact windows show one
//! pad at a time and switch with the `fn` / `123` keys.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ButtonIdentifier {
    LeftParenthesis,
    RightParenthesis,
    Clear,
    Backspace,
    Factorial,
    Divide,
    Abs,
    Sin,
    Cos,
    Tan,
    Seven,
    Eight,
    Nine,
    Multiply,
    Percent,
    Sinh,
    Cosh,
    Tanh,
    Four,
    Five,
    Six,
    Subtract,
    Mod,
    Ln,
    Log10,
    Sqrt,
    One,
    Two,
    Three,
    Add,
    Random,
    Power,
    Square,
    Cube,
    TenPower,
    Zero,
    Decimal,
    Equals,
    Inverse,
    Pi,
    E,
}

/// Number of calculator operations (every [`ButtonIdentifier`]).
#[cfg(test)]
const OPERATION_COUNT: u8 = ButtonIdentifier::E as u8 + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Action(ButtonIdentifier),
    /// Compact layout: switch the single pad to scientific functions.
    ShowScientific,
    /// Compact layout: switch the single pad back to digits.
    ShowBasic,
}

/// Compact label for Clear; wider pages show the localized "Clear" text instead.
pub const CLEAR_LABEL: &str = "C";
/// `LV_SYMBOL_BACKSPACE`, part of the bundled icon font.
pub const BACKSPACE_LABEL: &str = "\u{F55A}";

pub type Layout = &'static [&'static [(&'static str, Key)]];

use ButtonIdentifier::*;
use Key::{Action as A, ShowBasic, ShowScientific};

/// Wide layout, left pad: functions grouped by family, 3 columns.
pub const WIDE_SCIENTIFIC: Layout = &[
    &[("sin", A(Sin)), ("cos", A(Cos)), ("tan", A(Tan))],
    &[("sinh", A(Sinh)), ("cosh", A(Cosh)), ("tanh", A(Tanh))],
    &[("ln", A(Ln)), ("log", A(Log10)), ("sqrt", A(Sqrt))],
    &[("x^2", A(Square)), ("x^3", A(Cube)), ("10^x", A(TenPower))],
    &[("x!", A(Factorial)), ("1/x", A(Inverse)), ("|x|", A(Abs))],
    &[("pi", A(Pi)), ("e", A(E)), ("rand", A(Random))],
];

/// Wide layout, right pad: conventional digit block with operators on the right.
pub const WIDE_BASIC: Layout = &[
    &[
        (CLEAR_LABEL, A(Clear)),
        ("(", A(LeftParenthesis)),
        (")", A(RightParenthesis)),
        (BACKSPACE_LABEL, A(Backspace)),
    ],
    &[
        ("^", A(Power)),
        ("mod", A(Mod)),
        ("%", A(Percent)),
        ("/", A(Divide)),
    ],
    &[
        ("7", A(Seven)),
        ("8", A(Eight)),
        ("9", A(Nine)),
        ("*", A(Multiply)),
    ],
    &[
        ("4", A(Four)),
        ("5", A(Five)),
        ("6", A(Six)),
        ("-", A(Subtract)),
    ],
    &[("1", A(One)), ("2", A(Two)), ("3", A(Three)), ("+", A(Add))],
    &[("0", A(Zero)), (".", A(Decimal)), ("=", A(Equals))],
];

/// Compact layout, digits: five rows so every key stays reachable on 240 px tall screens.
pub const COMPACT_BASIC: Layout = &[
    &[
        (CLEAR_LABEL, A(Clear)),
        (BACKSPACE_LABEL, A(Backspace)),
        ("fn", ShowScientific),
        ("/", A(Divide)),
    ],
    &[
        ("7", A(Seven)),
        ("8", A(Eight)),
        ("9", A(Nine)),
        ("*", A(Multiply)),
    ],
    &[
        ("4", A(Four)),
        ("5", A(Five)),
        ("6", A(Six)),
        ("-", A(Subtract)),
    ],
    &[("1", A(One)), ("2", A(Two)), ("3", A(Three)), ("+", A(Add))],
    &[
        ("%", A(Percent)),
        ("0", A(Zero)),
        (".", A(Decimal)),
        ("=", A(Equals)),
    ],
];

/// Compact layout, functions: keeps editing keys so expressions can be fixed in place.
pub const COMPACT_SCIENTIFIC: Layout = &[
    &[
        ("123", ShowBasic),
        ("(", A(LeftParenthesis)),
        (")", A(RightParenthesis)),
        ("^", A(Power)),
        ("mod", A(Mod)),
    ],
    &[
        ("sin", A(Sin)),
        ("cos", A(Cos)),
        ("tan", A(Tan)),
        ("sqrt", A(Sqrt)),
        ("x^2", A(Square)),
    ],
    &[
        ("sinh", A(Sinh)),
        ("cosh", A(Cosh)),
        ("tanh", A(Tanh)),
        ("x^3", A(Cube)),
        ("10^x", A(TenPower)),
    ],
    &[
        ("ln", A(Ln)),
        ("log", A(Log10)),
        ("x!", A(Factorial)),
        ("1/x", A(Inverse)),
        ("|x|", A(Abs)),
    ],
    &[
        ("pi", A(Pi)),
        ("e", A(E)),
        ("rand", A(Random)),
        (BACKSPACE_LABEL, A(Backspace)),
        (CLEAR_LABEL, A(Clear)),
    ],
];

/// shadcn button variants, numbered as `lv_theme_shadcn_variant_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Variant {
    Primary = 0,
    Secondary = 1,
    Outline = 2,
    Ghost = 3,
    Destructive = 4,
}

/// Visual role of a key: the single commit action stands out, digits are
/// filled, operators and editing keys are outlined, functions stay quiet.
pub const fn variant(key: Key) -> Variant {
    match key {
        Key::Action(Equals) => Variant::Primary,
        Key::Action(Clear) => Variant::Destructive,
        Key::Action(
            Zero | One | Two | Three | Four | Five | Six | Seven | Eight | Nine | Decimal,
        ) => Variant::Secondary,
        Key::Action(
            Add | Subtract | Multiply | Divide | Power | Mod | Percent | LeftParenthesis
            | RightParenthesis | Backspace,
        )
        | Key::ShowScientific
        | Key::ShowBasic => Variant::Outline,
        Key::Action(_) => Variant::Ghost,
    }
}

/// Number of grid columns a layout needs.
pub fn columns(layout: Layout) -> usize {
    layout.iter().map(|row| row.len()).max().unwrap_or(0)
}

/// Grid cells as `(column, span, row)`; a short row's first key absorbs the gap.
pub fn cells(layout: Layout) -> impl Iterator<Item = (usize, usize, usize, &'static str, Key)> {
    let columns = columns(layout);
    layout.iter().enumerate().flat_map(move |(row, keys)| {
        let extra = columns - keys.len();
        keys.iter().enumerate().map(move |(index, &(label, key))| {
            let (column, span) = if index == 0 {
                (0, 1 + extra)
            } else {
                (index + extra, 1)
            };
            (column, span, row, label, key)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(layouts: &[Layout]) -> Vec<ButtonIdentifier> {
        layouts
            .iter()
            .flat_map(|layout| layout.iter().flat_map(|row| row.iter()))
            .filter_map(|(_, key)| match key {
                Key::Action(action) => Some(*action),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn wide_layout_reaches_every_operation_exactly_once() {
        let actions = actions(&[WIDE_SCIENTIFIC, WIDE_BASIC]);
        for id in 0..OPERATION_COUNT {
            assert_eq!(
                actions.iter().filter(|a| **a as u8 == id).count(),
                1,
                "operation {id}"
            );
        }
        assert!(
            WIDE_SCIENTIFIC
                .iter()
                .chain(WIDE_BASIC)
                .flat_map(|row| row.iter())
                .all(|(_, key)| matches!(key, Key::Action(_)))
        );
    }

    #[test]
    fn compact_layout_reaches_every_operation_and_can_switch_back() {
        let actions = actions(&[COMPACT_BASIC, COMPACT_SCIENTIFIC]);
        for id in 0..OPERATION_COUNT {
            assert!(actions.iter().any(|a| *a as u8 == id), "operation {id}");
        }
        assert!(
            COMPACT_BASIC
                .iter()
                .flat_map(|r| r.iter())
                .any(|(_, k)| *k == ShowScientific)
        );
        assert!(
            COMPACT_SCIENTIFIC
                .iter()
                .flat_map(|r| r.iter())
                .any(|(_, k)| *k == ShowBasic)
        );
    }

    #[test]
    fn compact_pads_fit_five_rows_and_wide_pads_align() {
        assert_eq!(COMPACT_BASIC.len(), 5);
        assert_eq!(COMPACT_SCIENTIFIC.len(), 5);
        assert_eq!(WIDE_SCIENTIFIC.len(), WIDE_BASIC.len());
    }

    #[test]
    fn digit_rows_keep_operators_on_the_right() {
        for layout in [WIDE_BASIC, COMPACT_BASIC] {
            let column: Vec<_> = layout
                .iter()
                .filter(|row| {
                    row.iter()
                        .any(|(_, k)| *k == A(Seven) || *k == A(One) || *k == A(Four))
                })
                .map(|row| row.last().unwrap().1)
                .collect();
            assert_eq!(column, [A(Multiply), A(Subtract), A(Add)]);
        }
    }

    #[test]
    fn short_rows_widen_their_first_key() {
        let last_row: Vec<_> = cells(WIDE_BASIC).filter(|cell| cell.2 == 5).collect();
        assert_eq!(
            last_row
                .iter()
                .map(|&(column, span, ..)| (column, span))
                .collect::<Vec<_>>(),
            [(0, 2), (2, 1), (3, 1)]
        );
        assert_eq!(last_row[0].4, A(Zero));
        for layout in [
            WIDE_SCIENTIFIC,
            WIDE_BASIC,
            COMPACT_BASIC,
            COMPACT_SCIENTIFIC,
        ] {
            for row in 0..layout.len() {
                let width: usize = cells(layout).filter(|c| c.2 == row).map(|c| c.1).sum();
                assert_eq!(width, columns(layout));
            }
        }
    }

    #[test]
    fn only_equals_is_primary_and_groups_are_distinct() {
        let all: Vec<Key> = [
            WIDE_SCIENTIFIC,
            WIDE_BASIC,
            COMPACT_BASIC,
            COMPACT_SCIENTIFIC,
        ]
        .iter()
        .flat_map(|layout| layout.iter().flat_map(|row| row.iter().map(|(_, k)| *k)))
        .collect();
        for key in all {
            assert_eq!(
                variant(key) == Variant::Primary,
                key == A(Equals),
                "{key:?}"
            );
        }
        assert_eq!(variant(A(Seven)), Variant::Secondary);
        assert_eq!(variant(A(Add)), Variant::Outline);
        assert_eq!(variant(A(Sin)), Variant::Ghost);
        assert_eq!(variant(A(Clear)), Variant::Destructive);
    }
}
