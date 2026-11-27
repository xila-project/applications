use core::task::{Context as TaskContext, Poll};
use portable_atomic::Ordering;
use xila::{
    file_system::{
        BaseOperations, CharacterDevice, Context, ControlCommand, ControlCommandIdentifier,
        IoErrorKind, MountOperations, Operation, Position, Result, Size, Waiter,
        character_device::{GET_TERMINAL_ATTRIBUTES, IS_A_TERMINAL, SET_TERMINAL_ATTRIBUTES},
    },
    shared::AnyByLayout,
};

use crate::terminal::Terminal;

fn map_error(error: crate::Error) -> IoErrorKind {
    match error {
        // A contended try-lock: the caller can wait and retry.
        crate::Error::WouldBlock => IoErrorKind::WouldBlock,
        _ => IoErrorKind::InputOutput,
    }
}

#[derive(Default)]
struct DeviceWaiters {
    input: Waiter,
    graphics: Waiter,
}

impl Terminal {
    fn poll_graphics<T>(
        &self,
        context: &mut Context,
        task: &TaskContext<'_>,
        mut operation: impl FnMut() -> Result<T>,
    ) -> Poll<Result<T>> {
        let result = operation();
        if !matches!(result, Err(IoErrorKind::WouldBlock)) {
            return Poll::Ready(result);
        }
        let waiters = context
            .get_private_data_mutable_of_type::<DeviceWaiters>()
            .ok_or(IoErrorKind::BadDescriptor)?;
        self.graphics_waiters
            .register(&mut waiters.graphics, task.waker())?;
        match operation() {
            Err(IoErrorKind::WouldBlock) if self.input_closed.load(Ordering::Acquire) => {
                Poll::Ready(Err(IoErrorKind::BrokenPipe))
            }
            Err(IoErrorKind::WouldBlock) => Poll::Pending,
            result => Poll::Ready(result),
        }
    }
}

impl BaseOperations for Terminal {
    fn open(&self, context: &mut Context) -> Result<()> {
        context.set_private_data(alloc::boxed::Box::new(DeviceWaiters::default()));
        Ok(())
    }
    fn clone_context(&self, context: &Context) -> Result<Context> {
        context
            .get_private_data_of_type::<DeviceWaiters>()
            .ok_or(IoErrorKind::BadDescriptor)?;
        Ok(Context::new(Some(DeviceWaiters::default())))
    }
    fn close(&self, context: &mut Context) -> Result<()> {
        if let Some(waiters) = context.get_private_data_mutable_of_type::<DeviceWaiters>() {
            self.input_waiters.cancel(&mut waiters.input);
            self.graphics_waiters.cancel(&mut waiters.graphics);
        }
        context.take_private_data();
        Ok(())
    }
    fn cancel_operation(&self, context: &mut Context, operation: Operation) {
        if let Some(waiters) = context.get_private_data_mutable_of_type::<DeviceWaiters>() {
            match operation {
                Operation::Read => self.input_waiters.cancel(&mut waiters.input),
                Operation::Write | Operation::Control => {
                    self.graphics_waiters.cancel(&mut waiters.graphics);
                }
                _ => {}
            }
        }
    }
    fn read(
        &self,
        context: &mut Context,
        task: &mut TaskContext<'_>,
        buffer: &mut [u8],
        _: Size,
    ) -> Poll<Result<usize>> {
        let waiters = context
            .get_private_data_mutable_of_type::<DeviceWaiters>()
            .ok_or(IoErrorKind::BadDescriptor)?;
        if buffer.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut read = || {
            let closed = self.input_closed.load(Ordering::Acquire);
            match self.read_input(buffer) {
                Ok(count) if count != 0 || closed => Poll::Ready(Ok(count)),
                Err(crate::Error::WouldBlock) if closed => Poll::Ready(Ok(0)),
                Ok(_) | Err(crate::Error::WouldBlock) => Poll::Pending,
                Err(error) => Poll::Ready(Err(map_error(error))),
            }
        };
        let result = read();
        if result.is_ready() {
            return result;
        }
        self.input_waiters
            .register(&mut waiters.input, task.waker())?;
        read()
    }
    fn write(
        &self,
        context: &mut Context,
        task: &mut TaskContext<'_>,
        buffer: &[u8],
        _: Size,
    ) -> Poll<Result<usize>> {
        self.poll_graphics(context, task, || {
            self.print(buffer).map_err(map_error)?;
            Ok(buffer.len())
        })
    }
    fn control(
        &self,
        context: &mut Context,
        task: &mut TaskContext<'_>,
        command: ControlCommandIdentifier,
        input: &AnyByLayout,
        output: &mut AnyByLayout,
    ) -> Poll<Result<()>> {
        self.poll_graphics(context, task, || match command {
            IS_A_TERMINAL::IDENTIFIER => {
                *IS_A_TERMINAL::cast_output(output)? = 1;
                Ok(())
            }
            GET_TERMINAL_ATTRIBUTES::IDENTIFIER => {
                *GET_TERMINAL_ATTRIBUTES::cast_output(output)? =
                    self.terminal_attributes().map_err(map_error)?;
                Ok(())
            }
            SET_TERMINAL_ATTRIBUTES::IDENTIFIER => {
                let attributes = *SET_TERMINAL_ATTRIBUTES::cast_input(input)?;
                if !attributes.has_only_known_flags() {
                    return Err(IoErrorKind::InvalidArgument);
                }
                self.set_terminal_attributes(attributes).map_err(map_error)
            }
            _ => Err(IoErrorKind::Unsupported),
        })
    }
    fn set_position(&self, _: &mut Context, _: Size, _: &Position) -> Result<Size> {
        Err(IoErrorKind::Unsupported)
    }
}
impl MountOperations for Terminal {}
impl CharacterDevice for Terminal {}
