use crate::atomic_waker::AtomicWaker;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

struct Inner<T> {
    state: Mutex<State<T>>,
    receiver_waker: AtomicWaker,
    sender_waker: AtomicWaker,
}

struct State<T> {
    value: Option<T>,
    sender_dropped: bool,
    receiver_dropped: bool,
}

impl<T> Inner<T> {
    fn state(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn take(&self) -> Poll<Result<T, Canceled>> {
        let mut state = self.state();
        match state.value.take() {
            Some(value) => Poll::Ready(Ok(value)),
            None if state.sender_dropped => Poll::Ready(Err(Canceled)),
            None => Poll::Pending,
        }
    }

    fn canceled(&self) -> bool {
        self.state().receiver_dropped
    }
}

#[must_use]
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner {
        state: Mutex::new(State {
            value: None,
            sender_dropped: false,
            receiver_dropped: false,
        }),
        receiver_waker: AtomicWaker::new(),
        sender_waker: AtomicWaker::new(),
    });
    (
        Sender {
            inner: inner.clone(),
        },
        Receiver { inner },
    )
}

pub struct Sender<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Sender<T> {
    pub fn send(self, value: T) -> Result<(), T> {
        let mut state = self.inner.state();
        if state.receiver_dropped {
            return Err(value);
        }
        state.value = Some(value);
        drop(state);
        Ok(())
    }

    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.inner.canceled()
    }

    pub fn cancellation(&mut self) -> Cancellation<'_, T> {
        Cancellation { sender: self }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.inner.state().sender_dropped = true;
        self.inner.receiver_waker.wake();
    }
}

pub struct Cancellation<'a, T> {
    sender: &'a mut Sender<T>,
}

impl<T> Future for Cancellation<'_, T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let inner = &self.sender.inner;
        if inner.canceled() {
            return Poll::Ready(());
        }
        inner.sender_waker.register(cx.waker());
        if inner.canceled() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

pub struct Receiver<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, Canceled>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<T, Canceled>> {
        if let Poll::Ready(result) = self.inner.take() {
            return Poll::Ready(result);
        }
        self.inner.receiver_waker.register(cx.waker());
        self.inner.take()
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let value = {
            let mut state = self.inner.state();
            state.receiver_dropped = true;
            state.value.take()
        };
        self.inner.sender_waker.wake();
        drop(value);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Canceled;

impl fmt::Display for Canceled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("oneshot canceled")
    }
}

impl std::error::Error for Canceled {}
