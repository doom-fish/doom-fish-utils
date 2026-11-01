pub mod mpsc;
mod mutex;
pub mod oneshot;

pub use mutex::{Mutex, MutexGuard, MutexLockFuture};

use std::future::Future;
use std::mem;
use std::pin::{pin, Pin};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub trait FutureExt: Future {
    fn boxed<'a>(self) -> BoxFuture<'a, Self::Output>
    where
        Self: Sized + Send + 'a,
    {
        Box::pin(self)
    }

    fn boxed_local<'a>(self) -> LocalBoxFuture<'a, Self::Output>
    where
        Self: Sized + 'a,
    {
        Box::pin(self)
    }

    fn now_or_never(self) -> Option<Self::Output>
    where
        Self: Sized,
    {
        struct Noop;

        impl Wake for Noop {
            fn wake(self: Arc<Self>) {}
        }

        let waker = Waker::from(Arc::new(Noop));
        match pin!(self).poll(&mut Context::from_waker(&waker)) {
            Poll::Ready(output) => Some(output),
            Poll::Pending => None,
        }
    }
}

impl<F: Future + ?Sized> FutureExt for F {}

pub trait Stream {
    type Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>>;
}

pub trait StreamExt: Stream {
    fn next(&mut self) -> Next<'_, Self>
    where
        Self: Unpin,
    {
        Next { stream: self }
    }
}

impl<S: Stream + ?Sized> StreamExt for S {}

pub struct Next<'a, S: ?Sized> {
    stream: &'a mut S,
}

impl<S: Stream + Unpin + ?Sized> Future for Next<'_, S> {
    type Output = Option<S::Item>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        Pin::new(&mut *self.stream).poll_next(cx)
    }
}

pub enum Either<A, B> {
    Left(A),
    Right(B),
}

pub const fn select<A, B>(left: A, right: B) -> Select<A, B>
where
    A: Future + Unpin,
    B: Future + Unpin,
{
    Select {
        futures: Some((left, right)),
    }
}

pub struct Select<A, B> {
    futures: Option<(A, B)>,
}

impl<A, B> Future for Select<A, B>
where
    A: Future + Unpin,
    B: Future + Unpin,
{
    type Output = Either<(A::Output, B), (B::Output, A)>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (mut left, mut right) = self.futures.take().expect("Select polled after completion");
        if let Poll::Ready(output) = Pin::new(&mut left).poll(cx) {
            return Poll::Ready(Either::Left((output, right)));
        }
        if let Poll::Ready(output) = Pin::new(&mut right).poll(cx) {
            return Poll::Ready(Either::Right((output, left)));
        }
        self.futures = Some((left, right));
        Poll::Pending
    }
}

pub fn join_all<I>(futures: I) -> JoinAll<I::Item>
where
    I: IntoIterator,
    I::Item: Future,
{
    JoinAll {
        slots: futures
            .into_iter()
            .map(|future| Slot::Pending(Box::pin(future)))
            .collect(),
    }
}

pub struct JoinAll<F: Future> {
    slots: Vec<Slot<F>>,
}

enum Slot<F: Future> {
    Pending(Pin<Box<F>>),
    Done(F::Output),
}

impl<F: Future> Unpin for JoinAll<F> {}

impl<F: Future> Future for JoinAll<F> {
    type Output = Vec<F::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Vec<F::Output>> {
        let mut done = true;
        for slot in &mut self.slots {
            if let Slot::Pending(future) = slot {
                match future.as_mut().poll(cx) {
                    Poll::Ready(output) => *slot = Slot::Done(output),
                    Poll::Pending => done = false,
                }
            }
        }
        if !done {
            return Poll::Pending;
        }
        Poll::Ready(
            mem::take(&mut self.slots)
                .into_iter()
                .filter_map(|slot| match slot {
                    Slot::Done(output) => Some(output),
                    Slot::Pending(_) => None,
                })
                .collect(),
        )
    }
}
