//! Optional building blocks for separating application data, logic and display.
//!
//! A model contains data, application logic updates it, and a frontend maps
//! input to messages and renders a read-only view. No platform, event loop,
//! allocator or runtime is required.
#![no_std]

/// Application logic, independent of the storage and presentation adapters.
pub trait Application {
    type Model;
    type Message;
    type Error;
    /// A view may borrow model data or be a small value computed from it.
    type View<'a>
    where
        Self: 'a,
        Self::Model: 'a;

    fn update(
        &mut self,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Result<(), Self::Error>;
    fn view<'a>(&'a self, model: &'a Self::Model) -> Self::View<'a>;
}

/// Maps platform input to messages and presents the application's view.
pub trait Frontend<A: Application> {
    type Input;
    type Error;

    /// Return `None` for an input that does not require an update or repaint.
    fn translate(&mut self, input: Self::Input) -> Result<Option<A::Message>, Self::Error>;
    fn render(&mut self, view: A::View<'_>) -> Result<(), Self::Error>;
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
    model: &A::Model,
    frontend: &mut F,
) -> Result<(), F::Error> {
    frontend.render(application.view(model))
}

/// Translate input, update data, and render once after a successful update.
/// `Ok(false)` means the input was ignored. A render failure happens after the
/// model has changed and does not roll the update back.
pub fn dispatch<A: Application, F: Frontend<A>>(
    application: &mut A,
    model: &mut A::Model,
    frontend: &mut F,
    input: F::Input,
) -> Result<bool, DispatchError<A::Error, F::Error>> {
    let Some(message) = frontend.translate(input).map_err(DispatchError::Frontend)? else {
        return Ok(false);
    };
    application
        .update(model, message)
        .map_err(DispatchError::Application)?;
    frontend
        .render(application.view(model))
        .map_err(DispatchError::Frontend)?;
    Ok(true)
}
