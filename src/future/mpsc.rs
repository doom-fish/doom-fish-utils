use super::Stream;
use crate::atomic_waker::AtomicWaker;
use std::collections::VecDeque;
use std::fmt;
use std::future::{poll_fn, Future};
use std::mem;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

struct Channel<T> {
    state: Mutex<State<T>>,
    receiver_waker: AtomicWaker,
}

struct State<T> {
    queue: VecDeque<T>,
    capacity: usize,
    senders: usize,
    open: bool,
}

impl<T> Channel<T> {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                capacity,
                senders: 1,
                open: true,
            }),
            receiver_waker: AtomicWaker::new(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut state = self.state();
        if !state.open {
            return Err(TrySendError { value, full: false });
        }
        if state.queue.len() >= state.capacity {
            return Err(TrySendError { value, full: true });
        }
        state.queue.push_back(value);
        drop(state);
        self.receiver_waker.wake();
        Ok(())
    }

    fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut state = self.state();
        match state.queue.pop_front() {
            Some(value) => Ok(value),
            None if state.open && state.senders > 0 => Err(TryRecvError::Empty),
            None => Err(TryRecvError::Closed),
        }
    }

    fn poll_recv(&self, cx: &Context<'_>) -> Poll<Option<T>> {
        let result = match self.try_recv() {
            Err(TryRecvError::Empty) => {
                self.receiver_waker.register(cx.waker());
                self.try_recv()
            }
            result => result,
        };
        match result {
            Ok(value) => Poll::Ready(Some(value)),
            Err(TryRecvError::Closed) => Poll::Ready(None),
            Err(TryRecvError::Empty) => Poll::Pending,
        }
    }

    fn close(&self) {
        self.state().open = false;
    }

    fn drop_sender(&self) {
        let mut state = self.state();
        state.senders -= 1;
        let last = state.senders == 0;
        drop(state);
        if last {
            self.receiver_waker.wake();
        }
    }

    fn drop_receiver(&self) {
        let mut state = self.state();
        state.open = false;
        let queue = mem::take(&mut state.queue);
        drop(state);
        drop(queue);
    }
}

#[must_use]
pub fn channel<T>(buffer: usize) -> (Sender<T>, Receiver<T>) {
    let channel = Channel::new(buffer.saturating_add(1));
    (Sender(channel.clone()), Receiver(channel))
}

#[must_use]
pub fn unbounded<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
    let channel = Channel::new(usize::MAX);
    (UnboundedSender(channel.clone()), UnboundedReceiver(channel))
}

pub struct Sender<T>(Arc<Channel<T>>);

impl<T> Sender<T> {
    pub fn try_send(&mut self, value: T) -> Result<(), TrySendError<T>> {
        self.0.send(value)
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        !self.0.state().open
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.0.drop_sender();
    }
}

pub struct UnboundedSender<T>(Arc<Channel<T>>);

impl<T> UnboundedSender<T> {
    pub fn unbounded_send(&self, value: T) -> Result<(), TrySendError<T>> {
        self.0.send(value)
    }

    #[must_use]
    pub fn same_receiver(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl<T> Clone for UnboundedSender<T> {
    fn clone(&self) -> Self {
        self.0.state().senders += 1;
        Self(self.0.clone())
    }
}

impl<T> Drop for UnboundedSender<T> {
    fn drop(&mut self) {
        self.0.drop_sender();
    }
}

pub struct Receiver<T>(Arc<Channel<T>>);

impl<T> Receiver<T> {
    pub fn recv(&mut self) -> impl Future<Output = Result<T, RecvError>> + '_ {
        poll_fn(|cx| self.0.poll_recv(cx).map(|value| value.ok_or(RecvError)))
    }

    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        self.0.try_recv()
    }

    pub fn close(&mut self) {
        self.0.close();
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.0.drop_receiver();
    }
}

pub struct UnboundedReceiver<T>(Arc<Channel<T>>);

impl<T> UnboundedReceiver<T> {
    pub fn recv(&mut self) -> impl Future<Output = Result<T, RecvError>> + '_ {
        poll_fn(|cx| self.0.poll_recv(cx).map(|value| value.ok_or(RecvError)))
    }

    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        self.0.try_recv()
    }
}

impl<T> Stream for UnboundedReceiver<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.0.poll_recv(cx)
    }
}

impl<T> Drop for UnboundedReceiver<T> {
    fn drop(&mut self) {
        self.0.drop_receiver();
    }
}

pub struct TrySendError<T> {
    value: T,
    full: bool,
}

impl<T> TrySendError<T> {
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T> fmt::Display for TrySendError<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(if self.full {
            "send failed because channel is full"
        } else {
            "send failed because receiver is gone"
        })
    }
}

pub struct RecvError;

pub enum TryRecvError {
    Empty,
    Closed,
}
