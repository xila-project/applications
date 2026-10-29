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
    /// A typed request for work performed by the surrounding application.
    type Effect;
    /// A view may borrow model data or be a small value computed from it.
    type View<'a>
    where
        Self: 'a,
        Self::Model: 'a;

    fn update(
        &mut self,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Result<Self::Effect, Self::Error>;
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

/// Result of translating and applying one input.
pub type ProcessResult<Effect, ApplicationError, FrontendError> =
    Result<Option<Effect>, DispatchError<ApplicationError, FrontendError>>;

/// Translate and apply one input without rendering. Return an effect for the
/// caller to perform, or `None` when the frontend ignores the input. The caller
/// owns effect execution, error handling, scheduling and repaint batching.
pub fn process<A: Application, F: Frontend<A>>(
    application: &mut A,
    model: &mut A::Model,
    frontend: &mut F,
    input: F::Input,
) -> ProcessResult<A::Effect, A::Error, F::Error> {
    let Some(message) = frontend.translate(input).map_err(DispatchError::Frontend)? else {
        return Ok(None);
    };
    application
        .update(model, message)
        .map(Some)
        .map_err(DispatchError::Application)
}

/// Process and render one effect-free update. A render failure occurs after
/// the model changes and does not roll it back. For effects or batched
/// rendering, use `process` and `render` separately.
pub fn dispatch<A: Application<Effect = ()>, F: Frontend<A>>(
    application: &mut A,
    model: &mut A::Model,
    frontend: &mut F,
    input: F::Input,
) -> Result<bool, DispatchError<A::Error, F::Error>> {
    if process(application, model, frontend, input)?.is_none() {
        return Ok(false);
    }
    render(application, model, frontend).map_err(DispatchError::Frontend)?;
    Ok(true)
}

/// A separately confirmed destructive action.
#[derive(Clone, Copy)]
pub enum Confirmation {
    Open,
    Close,
}

/// Tracks confirmations for the current document contents without allocating.
/// Call `changed` whenever the contents or their saved state change.
#[derive(Default)]
pub struct Confirmations(u8);

impl Confirmations {
    fn mask(action: Confirmation) -> u8 {
        match action {
            Confirmation::Open => 1,
            Confirmation::Close => 2,
        }
    }

    pub fn is_confirmed(&self, action: Confirmation) -> bool {
        self.0 & Self::mask(action) != 0
    }

    pub fn confirm(&mut self, action: Confirmation) {
        self.0 |= Self::mask(action);
    }

    pub fn clear(&mut self, action: Confirmation) {
        self.0 &= !Self::mask(action);
    }

    pub fn changed(&mut self) {
        self.0 = 0;
    }
}
