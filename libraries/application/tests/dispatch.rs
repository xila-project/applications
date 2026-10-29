use xila_application::{Application, DispatchError, Frontend, dispatch, render};

#[derive(Default)]
struct CounterData(u8);
struct CounterLogic;
impl Application for CounterLogic {
    type Model = CounterData;
    type Message = u8;
    type Error = &'static str;
    type Effect = ();
    type View<'a> = u8;
    fn update(&mut self, model: &mut CounterData, amount: u8) -> Result<(), Self::Error> {
        model.0 = model.0.checked_add(amount).ok_or("overflow")?;
        Ok(())
    }
    fn view<'a>(&'a self, model: &'a CounterData) -> u8 {
        model.0
    }
}
#[derive(Default)]
struct Console {
    output: u8,
    fail_render: bool,
}
impl Frontend<CounterLogic> for Console {
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
    fn render(&mut self, view: u8) -> Result<(), Self::Error> {
        if self.fail_render {
            return Err("render failed");
        }
        self.output = view;
        Ok(())
    }
}
#[test]
fn dispatch_updates_then_renders_and_ignores_unmapped_input() {
    let mut data = CounterData::default();
    let mut logic = CounterLogic;
    let mut ui = Console::default();
    render(&logic, &data, &mut ui).unwrap();
    assert_eq!(dispatch(&mut logic, &mut data, &mut ui, 3), Ok(true));
    assert_eq!((data.0, ui.output), (3, 3));
    assert_eq!(dispatch(&mut logic, &mut data, &mut ui, 0), Ok(false));
    assert_eq!((data.0, ui.output), (3, 3));
}
#[test]
fn dispatch_preserves_error_source_and_only_renders_after_successful_update() {
    let mut data = CounterData(254);
    let mut logic = CounterLogic;
    let mut ui = Console::default();
    assert_eq!(
        dispatch(&mut logic, &mut data, &mut ui, 255),
        Err(DispatchError::Frontend("bad input"))
    );
    assert_eq!(
        dispatch(&mut logic, &mut data, &mut ui, 2),
        Err(DispatchError::Application("overflow"))
    );
    assert_eq!((data.0, ui.output), (254, 0));
    ui.fail_render = true;
    assert_eq!(
        dispatch(&mut logic, &mut data, &mut ui, 1),
        Err(DispatchError::Frontend("render failed"))
    );
    assert_eq!(data.0, 255);
}

#[test]
fn render_can_borrow_model_data_without_copying() {
    struct Logic;
    impl Application for Logic {
        type Model = [u8; 3];
        type Message = ();
        type Error = core::convert::Infallible;
        type Effect = ();
        type View<'a> = &'a [u8];
        fn update(&mut self, _: &mut Self::Model, _: ()) -> Result<(), Self::Error> {
            Ok(())
        }
        fn view<'a>(&'a self, model: &'a Self::Model) -> &'a [u8] {
            model
        }
    }
    struct Screen {
        seen: *const u8,
    }
    impl Frontend<Logic> for Screen {
        type Input = ();
        type Error = core::convert::Infallible;
        fn translate(&mut self, _: ()) -> Result<Option<()>, Self::Error> {
            Ok(Some(()))
        }
        fn render(&mut self, view: &[u8]) -> Result<(), Self::Error> {
            self.seen = view.as_ptr();
            Ok(())
        }
    }
    let model = [1, 2, 3];
    let mut screen = Screen {
        seen: core::ptr::null(),
    };
    render(&Logic, &model, &mut screen).unwrap();
    assert_eq!(screen.seen, model.as_ptr());
}

#[test]
fn queued_updates_return_effects_without_repainting_until_requested() {
    use xila_application::process;
    struct EffectLogic;
    impl Application for EffectLogic {
        type Model = u8;
        type Message = u8;
        type Error = core::convert::Infallible;
        type Effect = u8;
        type View<'a> = u8;
        fn update(&mut self, model: &mut u8, value: u8) -> Result<u8, Self::Error> {
            *model += value;
            Ok(*model)
        }
        fn view<'a>(&'a self, model: &'a u8) -> u8 {
            *model
        }
    }
    #[derive(Default)]
    struct Display {
        renders: usize,
        value: u8,
    }
    impl Frontend<EffectLogic> for Display {
        type Input = u8;
        type Error = core::convert::Infallible;
        fn translate(&mut self, input: u8) -> Result<Option<u8>, Self::Error> {
            Ok(Some(input))
        }
        fn render(&mut self, view: u8) -> Result<(), Self::Error> {
            self.renders += 1;
            self.value = view;
            Ok(())
        }
    }
    let mut logic = EffectLogic;
    let mut model = 0;
    let mut display = Display::default();
    assert_eq!(
        process(&mut logic, &mut model, &mut display, 1),
        Ok(Some(1))
    );
    assert_eq!(
        process(&mut logic, &mut model, &mut display, 2),
        Ok(Some(3))
    );
    assert_eq!(display.renders, 0);
    render(&logic, &model, &mut display).unwrap();
    assert_eq!((display.renders, display.value), (1, 3));
}
