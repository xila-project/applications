use xila::{
    file_system::{
        BaseOperations, CharacterDevice, Context, IoErrorKind, MountOperations, Operation,
        Position, Result, Size, Waiter,
    },
    shared::utf8_chunks,
};

use crate::terminal::Terminal;

fn map_error(error: crate::Error) -> IoErrorKind {
    match error {
        // A contended try-lock: the caller can wait and retry.
        crate::Error::WouldBlock => IoErrorKind::WouldBlock,
        _ => IoErrorKind::InputOutput,
    }
}

impl BaseOperations for Terminal {
    fn open(&self, context: &mut Context) -> Result<()> {
        context.set_private_data(alloc::boxed::Box::new(Waiter::new()));
        Ok(())
    }
    fn clone_context(&self, context: &Context) -> Result<Context> {
        context
            .get_private_data_of_type::<Waiter>()
            .ok_or(IoErrorKind::BadDescriptor)?;
        Ok(Context::new(Some(Waiter::new())))
    }
    fn close(&self, context: &mut Context) -> Result<()> {
        self.cancel_operation(context, Operation::Read);
        context.take_private_data();
        Ok(())
    }
    fn cancel_operation(&self, context: &mut Context, operation: Operation) {
        if operation == Operation::Read
            && let Some(waiter) = context.get_private_data_mutable_of_type::<Waiter>()
        {
            self.input_waiters.cancel(waiter);
        }
    }
    fn read(
        &self,
        context: &mut Context,
        cx: &mut core::task::Context<'_>,
        buffer: &mut [u8],
        _: Size,
    ) -> core::task::Poll<Result<usize>> {
        use core::task::Poll;
        use portable_atomic::Ordering;
        let waiter = context
            .get_private_data_mutable_of_type::<Waiter>()
            .ok_or(IoErrorKind::BadDescriptor)?;
        let count = self.read_input(buffer).map_err(map_error)?;
        if count > 0 || buffer.is_empty() || self.input_closed.load(Ordering::Acquire) {
            return Poll::Ready(Ok(count));
        }
        self.input_waiters.register(waiter, cx.waker())?;
        let count = self.read_input(buffer).map_err(map_error)?;
        if count > 0 || self.input_closed.load(Ordering::Acquire) {
            Poll::Ready(Ok(count))
        } else {
            Poll::Pending
        }
    }
    fn write(
        &self,
        _: &mut Context,
        _: &mut core::task::Context<'_>,
        buffer: &[u8],
        _: Size,
    ) -> core::task::Poll<Result<usize>> {
        xila::file_system::ready(|| {
            for string in utf8_chunks(buffer) {
                self.print(string).map_err(map_error)?;
            }
            Ok(buffer.len())
        })
    }
    fn set_position(&self, _: &mut Context, _: Size, _: &Position) -> Result<Size> {
        Err(IoErrorKind::Unsupported)
    }
}
impl MountOperations for Terminal {}
impl CharacterDevice for Terminal {}
