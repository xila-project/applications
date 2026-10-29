use xila_application::{Application, DispatchError, Frontend, dispatch, render};

#[derive(Default)]
struct Counter(u8);
impl Application for Counter {
    type Message = u8;
    type Error = &'static str;
    fn update(&mut self, amount: u8) -> Result<(), Self::Error> {
        self.0 = self.0.checked_add(amount).ok_or("overflow")?;
        Ok(())
    }
}
#[derive(Default)]
struct Console {
    output: u8,
    fail_render: bool,
}
impl Frontend<Counter> for Console {
    type Input = u8;
    type Error = &'static str;
    fn translate(&mut self, input: u8) -> Result<Option<u8>, Self::Error> {
        if input == 255 {
            Err("bad input")
        } else if input == 0 {
            Ok(None)
        } else {
            Ok(Some(input))
        }
    }
    fn render(&mut self, app: &Counter) -> Result<(), Self::Error> {
        if self.fail_render {
            return Err("render failed");
        }
        self.output = app.0;
        Ok(())
    }
}
#[test]
fn dispatch_updates_then_renders_and_ignores_unmapped_input() {
    let mut app = Counter::default();
    let mut ui = Console::default();
    render(&app, &mut ui).unwrap();
    assert_eq!(dispatch(&mut app, &mut ui, 3), Ok(true));
    assert_eq!((app.0, ui.output), (3, 3));
    assert_eq!(dispatch(&mut app, &mut ui, 0), Ok(false));
    assert_eq!((app.0, ui.output), (3, 3));
}
#[test]
fn dispatch_preserves_error_source_and_only_renders_after_successful_update() {
    let mut app = Counter(254);
    let mut ui = Console::default();
    assert_eq!(
        dispatch(&mut app, &mut ui, 255),
        Err(DispatchError::Frontend("bad input"))
    );
    assert_eq!(
        dispatch(&mut app, &mut ui, 2),
        Err(DispatchError::Application("overflow"))
    );
    assert_eq!((app.0, ui.output), (254, 0));
    ui.fail_render = true;
    assert_eq!(
        dispatch(&mut app, &mut ui, 1),
        Err(DispatchError::Frontend("render failed"))
    );
    assert_eq!(app.0, 255);
}
