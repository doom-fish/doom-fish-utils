use std::collections::BTreeMap;
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::sync::PoisonError;
use std::task::{Context, Poll, Waker};

#[derive(Default)]
pub struct Mutex {
    state: std::sync::Mutex<State>,
}

#[derive(Default)]
struct State {
    locked: bool,
    next_waiter: u64,
    waiters: BTreeMap<u64, Option<Waker>>,
}

impl State {
    fn next_waker(&mut self) -> Option<Waker> {
        self.waiters.values_mut().next().and_then(Option::take)
    }
}

impl Mutex {
    pub const fn lock(&self) -> MutexLockFuture<'_> {
        MutexLockFuture {
            mutex: self,
            waiter: None,
        }
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_>> {
        let locked = mem::replace(&mut self.state().locked, true);
        (!locked).then(|| MutexGuard { mutex: self })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct MutexGuard<'a> {
    mutex: &'a Mutex,
}

impl Drop for MutexGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.mutex.state();
        state.locked = false;
        let next = state.next_waker();
        drop(state);
        if let Some(waker) = next {
            waker.wake();
        }
    }
}

pub struct MutexLockFuture<'a> {
    mutex: &'a Mutex,
    waiter: Option<u64>,
}

impl<'a> Future for MutexLockFuture<'a> {
    type Output = MutexGuard<'a>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<MutexGuard<'a>> {
        let mutex = self.mutex;
        let incoming;
        let mut state = mutex.state();
        if state.locked {
            if self
                .waiter
                .and_then(|id| state.waiters.get(&id))
                .and_then(Option::as_ref)
                .is_some_and(|waker| waker.will_wake(cx.waker()))
            {
                return Poll::Pending;
            }
            drop(state);
            incoming = cx.waker().clone();
            state = mutex.state();
            if state.locked {
                let waiter = *self.waiter.get_or_insert_with(|| {
                    state.next_waiter += 1;
                    state.next_waiter
                });
                let previous = state.waiters.insert(waiter, Some(incoming));
                drop(state);
                drop(previous);
                return Poll::Pending;
            }
        }
        state.locked = true;
        let previous = self.waiter.take().and_then(|id| state.waiters.remove(&id));
        drop(state);
        drop(previous);
        Poll::Ready(MutexGuard { mutex })
    }
}

impl Drop for MutexLockFuture<'_> {
    fn drop(&mut self) {
        let Some(waiter) = self.waiter.take() else {
            return;
        };
        let mut state = self.mutex.state();
        let removed = state.waiters.remove(&waiter);
        let next = if state.locked {
            None
        } else {
            state.next_waker()
        };
        drop(state);
        if let Some(waker) = next {
            waker.wake();
        }
        drop(removed);
    }
}
