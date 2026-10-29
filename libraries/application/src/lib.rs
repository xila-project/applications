//! Small, optional building blocks for message-driven applications.
//!
//! The core owns its state; each frontend translates its own inputs and renders
//! the updated core. No event loop, allocator, runtime, or platform is required.
#![no_std]

/// Interface-independent application state and transitions.
pub trait Application {
    type Message;
    type Error;

    fn update(&mut self, message: Self::Message) -> Result<(), Self::Error>;
}

/// Maps one platform input to a message and presents application state.
pub trait Frontend<A: Application> {
    type Input;
    type Error;

    /// Return `None` for an input that does not require an update or repaint.
    fn translate(&mut self, input: Self::Input) -> Result<Option<A::Message>, Self::Error>;
    fn render(&mut self, application: &A) -> Result<(), Self::Error>;
}

/// Identifies which part of processing failed.
#[derive(Debug, PartialEq, Eq)]
pub enum DispatchError<ApplicationError, FrontendError> {
    Application(ApplicationError),
    Frontend(FrontendError),
}

/// Render initial state or repaint independently of a message.
pub fn render<A: Application, F: Frontend<A>>(
    application: &A,
    frontend: &mut F,
) -> Result<(), F::Error> {
    frontend.render(application)
}

/// Translate one input, update the core, and render once after a successful
/// update. `Ok(false)` means the input was ignored. A render failure happens
/// after the state has changed and does not roll the update back.
pub fn dispatch<A: Application, F: Frontend<A>>(
    application: &mut A,
    frontend: &mut F,
    input: F::Input,
) -> Result<bool, DispatchError<A::Error, F::Error>> {
    let Some(message) = frontend.translate(input).map_err(DispatchError::Frontend)? else {
        return Ok(false);
    };
    application
        .update(message)
        .map_err(DispatchError::Application)?;
    frontend
        .render(application)
        .map_err(DispatchError::Frontend)?;
    Ok(true)
}
