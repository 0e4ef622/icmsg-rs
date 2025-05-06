//! The `poll` macro copied from the `futures` crate.

use core::{
    pin::Pin,
    task::{Context, Poll},
};

macro_rules! poll {
    ($x:expr $(,)?) => {
        $crate::poll::poll($x).await
    };
}

#[doc(hidden)]
pub fn poll<F: Future + Unpin>(future: F) -> PollOnce<F> {
    PollOnce { future }
}

#[allow(missing_debug_implementations)]
#[doc(hidden)]
pub struct PollOnce<F: Future + Unpin> {
    future: F,
}

impl<F: Future + Unpin> Future for PollOnce<F> {
    type Output = Poll<F::Output>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(Pin::new(&mut self.get_mut().future).poll(cx))
    }
}
