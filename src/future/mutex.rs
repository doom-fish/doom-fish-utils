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
    waiters: BTreeMap<u64, Waker>,
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

    fn wake_first_waiter(state: std::sync::MutexGuard<'_, State>) {
        let waker = state.waiters.values().next().cloned();
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

pub struct MutexGuard<'a> {
    mutex: &'a Mutex,
}

impl Drop for MutexGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.mutex.state();
        state.locked = false;
        Mutex::wake_first_waiter(state);
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
        let mut state = mutex.state();
        if !state.locked {
            state.locked = true;
            if let Some(waiter) = self.waiter.take() {
                state.waiters.remove(&waiter);
            }
            return Poll::Ready(MutexGuard { mutex });
        }
        let waiter = *self.waiter.get_or_insert_with(|| {
            state.next_waiter += 1;
            state.next_waiter
        });
        state.waiters.insert(waiter, cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for MutexLockFuture<'_> {
    fn drop(&mut self) {
        let Some(waiter) = self.waiter.take() else {
            return;
        };
        let mut state = self.mutex.state();
        state.waiters.remove(&waiter);
        if !state.locked {
            Mutex::wake_first_waiter(state);
        }
    }
}
