//! Provides [SignalStream] (turns Unix signals into a [Stream])

use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

#[cfg(unix)]
mod unix {
    use super::*;

    use tokio::signal::unix::Signal;

    /// A wrapper around [Signal] that implements [Stream].
    #[derive(Debug)]
    pub struct SignalStream {
        inner: Signal,
    }

    impl SignalStream {
        /// Create a new `SignalStream`.
        pub fn new(interval: Signal) -> Self {
            Self { inner: interval }
        }
    }

    impl Stream for SignalStream {
        type Item = ();

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<()>> {
            self.inner.poll_recv(cx)
        }
    }
}

#[cfg(unix)]
pub use unix::SignalStream;

#[cfg(windows)]
mod windows {
    use super::*;
    use tokio::signal::windows as win;

    pub struct SignalStream {
        int: win::CtrlC,
        term: win::CtrlBreak,
        close: win::CtrlClose,
    }

    impl SignalStream {
        pub fn new() -> Self {
            Self {
                int: win::ctrl_c().unwrap(),
                term: win::ctrl_break().unwrap(),
                close: win::ctrl_close().unwrap(),
            }
        }
    }

    impl Stream for SignalStream {
        type Item = ();

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<()>> {
            if let Poll::Ready(opt) = self.int.poll_recv(cx) {
                return Poll::Ready(opt);
            }
            if let Poll::Ready(opt) = self.term.poll_recv(cx) {
                return Poll::Ready(opt);
            }
            if let Poll::Ready(opt) = self.close.poll_recv(cx) {
                return Poll::Ready(opt);
            }

            Poll::Pending
        }
    }
}

#[cfg(windows)]
pub use windows::SignalStream;
