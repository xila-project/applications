use xila_application::{Confirmation, Confirmations};

#[test]
fn confirmations_are_independent_and_invalidated_by_an_edit() {
    assert_eq!(core::mem::size_of::<Confirmations>(), 1);
    let mut state = Confirmations::default();
    assert!(!state.is_confirmed(Confirmation::Open));
    state.confirm(Confirmation::Open);
    assert!(state.is_confirmed(Confirmation::Open));
    assert!(!state.is_confirmed(Confirmation::Close));
    state.confirm(Confirmation::Close);
    state.clear(Confirmation::Open);
    assert!(!state.is_confirmed(Confirmation::Open));
    assert!(state.is_confirmed(Confirmation::Close));
    state.changed();
    assert!(!state.is_confirmed(Confirmation::Close));
}
