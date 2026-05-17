//! Executor-agnostic bounded async streams for FFI callbacks.
//!
//! `BoundedAsyncStream<T>` is a generic, runtime-agnostic stream primitive
//! designed for wrapping Apple SDK callback / delegate / KVO patterns:
//!
//! * **Bounded** — backed by a fixed-capacity `VecDeque`. When the buffer
//!   is full and a new item arrives from the producer, the **oldest**
//!   queued item is dropped to make room (lossy by design).
//! * **Waker-driven** — implements `std::future::Future` via a stored
//!   `Waker`; works with any executor (tokio, async-std, smol, futures,
//!   etc.) without requiring a runtime feature.
//! * **`Send + Sync`** — produces and consumes can live on different
//!   threads, locked by a single `Mutex`.
//!
//! The lossy-oldest-drop policy is the right default for real-time event
//! streams (UI input, frame capture, BLE notifications, location updates):
//! a slow consumer should always see the latest event, not a stale queue.
//! When you instead need back-pressure (every event must be delivered),
//! use [`BoundedAsyncStream::push_or_block`] which blocks the producer
//! until the consumer drains capacity.
//!
//! # Example
//!
//! ```no_run
//! use doom_fish_utils::stream::BoundedAsyncStream;
//! use std::sync::Arc;
//!
//! # async fn run() {
//! // 8-element ring buffer of `String` events.
//! let (stream, sender) = BoundedAsyncStream::<String>::new(8);
//!
//! // Producer side: typically a Swift delegate / extern "C" callback
//! // running on a background queue.
//! std::thread::spawn(move || {
//!     for i in 0..100 {
//!         sender.push(format!("event #{i}"));
//!     }
//!     drop(sender); // closes the stream
//! });
//!
//! // Consumer side: any async runtime.
//! while let Some(event) = stream.next().await {
//!     println!("got {event}");
//! }
//! # }
//! ```

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};

/// Backing storage shared between the [`BoundedAsyncStream`] consumer and
/// every [`AsyncStreamSender`] producer.
struct State<T> {
    buffer: VecDeque<T>,
    waker: Option<Waker>,
    capacity: usize,
    /// Set to `true` when every sender has been dropped. The consumer's
    /// `next()` then returns `None` once the buffer drains.
    closed: bool,
}

/// Notifies a producer that's blocked in [`AsyncStreamSender::push_or_block`]
/// that the consumer made room in the buffer.
struct BackPressure {
    cvar: Condvar,
    /// Set to `true` when the stream is dropped — wakes any blocked
    /// producers so they can bail out instead of waiting forever.
    consumer_gone: Mutex<bool>,
}

/// A bounded, lossy-by-default, executor-agnostic async stream.
///
/// Items are pushed by one or more [`AsyncStreamSender`] handles and pulled
/// asynchronously via [`BoundedAsyncStream::next`].
///
/// See the [module-level docs](crate::stream) for the full design rationale.
pub struct BoundedAsyncStream<T> {
    state: Arc<Mutex<State<T>>>,
    back_pressure: Arc<BackPressure>,
}

/// Producer handle for a [`BoundedAsyncStream`].
///
/// Cheap to clone (`Arc` under the hood). Drop the last `AsyncStreamSender`
/// to close the stream; the consumer's `next()` will yield `None` once the
/// buffer is empty.
pub struct AsyncStreamSender<T> {
    state: Arc<Mutex<State<T>>>,
    back_pressure: Arc<BackPressure>,
}

impl<T> Clone for AsyncStreamSender<T> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            back_pressure: Arc::clone(&self.back_pressure),
        }
    }
}

impl<T> fmt::Debug for BoundedAsyncStream<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundedAsyncStream")
            .field("buffered", &self.buffered_count())
            .field("capacity", &self.capacity())
            .field("is_closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl<T> fmt::Debug for AsyncStreamSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncStreamSender").finish_non_exhaustive()
    }
}

impl<T> BoundedAsyncStream<T> {
    /// Creates a new bounded stream with the given capacity.
    ///
    /// Returns the consumer side and a single producer; clone the sender
    /// to fan out to multiple producers.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is 0 — a zero-capacity buffer would drop every
    /// item before the consumer could observe it. Use capacity 1 if you
    /// genuinely want "latest only" semantics.
    #[must_use]
    pub fn new(capacity: usize) -> (Self, AsyncStreamSender<T>) {
        assert!(capacity > 0, "BoundedAsyncStream capacity must be > 0");

        let state = Arc::new(Mutex::new(State {
            buffer: VecDeque::with_capacity(capacity),
            waker: None,
            capacity,
            closed: false,
        }));
        let back_pressure = Arc::new(BackPressure {
            cvar: Condvar::new(),
            consumer_gone: Mutex::new(false),
        });

        let stream = Self {
            state: Arc::clone(&state),
            back_pressure: Arc::clone(&back_pressure),
        };
        let sender = AsyncStreamSender {
            state,
            back_pressure,
        };
        (stream, sender)
    }

    /// Returns a future that resolves to the next item, or `None` once the
    /// stream is closed and drained.
    #[must_use]
    pub const fn next(&self) -> NextItem<'_, T> {
        NextItem { stream: self }
    }

    /// Non-blocking pop. Returns `None` if the buffer is empty (regardless
    /// of whether the stream is open or closed).
    #[must_use]
    pub fn try_next(&self) -> Option<T> {
        self.state.lock().ok()?.buffer.pop_front()
    }

    /// Returns `true` if the stream has been closed (all senders dropped).
    /// Note: a closed stream may still have buffered items to drain.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state.lock().map_or(true, |s| s.closed)
    }

    /// Returns the number of items currently buffered (0..=capacity).
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.state.lock().map_or(0, |s| s.buffer.len())
    }

    /// Returns the buffer capacity, as passed to [`Self::new`].
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.state.lock().map_or(0, |s| s.capacity)
    }

    /// Drops all currently buffered items without closing the stream.
    pub fn clear_buffer(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.buffer.clear();
        }
    }
}

impl<T> Drop for BoundedAsyncStream<T> {
    fn drop(&mut self) {
        if let Ok(mut consumer_gone) = self.back_pressure.consumer_gone.lock() {
            *consumer_gone = true;
        }
        self.back_pressure.cvar.notify_all();
    }
}

impl<T> AsyncStreamSender<T> {
    /// Push an item; drops the oldest queued item if the buffer is at
    /// capacity. This is the lossy default.
    pub fn push(&self, item: T) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };

        if state.buffer.len() >= state.capacity {
            state.buffer.pop_front();
        }
        state.buffer.push_back(item);

        if let Some(w) = state.waker.take() {
            w.wake();
        }
    }

    /// Push an item, blocking the current thread if the buffer is full
    /// until the consumer drains an item.
    ///
    /// Returns `Err(item)` if the consumer side has been dropped — the
    /// item is returned to the caller so it isn't leaked.
    ///
    /// # Errors
    ///
    /// Returns `Err(item)` if the consumer has been dropped.
    ///
    /// # Panics
    ///
    /// Panics if any mutex is poisoned by another thread panicking while
    /// holding it.
    pub fn push_or_block(&self, item: T) -> Result<(), T> {
        let mut item_slot = Some(item);
        let Ok(mut state_guard) = self.state.lock() else {
            return Err(item_slot.take().expect("item present"));
        };

        loop {
            // Bail out fast if the consumer is gone.
            if let Ok(consumer_gone) = self.back_pressure.consumer_gone.lock() {
                if *consumer_gone {
                    return Err(item_slot.take().expect("item present"));
                }
            }

            if state_guard.buffer.len() < state_guard.capacity {
                let item = item_slot.take().expect("item present");
                state_guard.buffer.push_back(item);
                if let Some(w) = state_guard.waker.take() {
                    w.wake();
                }
                return Ok(());
            }

            // Buffer full — wait for the consumer to make room. We must
            // release the state mutex before parking on the cvar, otherwise
            // the consumer can't push items in (and the consumer-drop signal
            // can't fire either).
            drop(state_guard);
            let Ok(consumer_gone) = self.back_pressure.consumer_gone.lock() else {
                return Err(item_slot.take().expect("item present"));
            };
            let wait_outcome = self.back_pressure.cvar.wait(consumer_gone);
            drop(wait_outcome);

            // Re-lock state and check again.
            state_guard = match self.state.lock() {
                Ok(g) => g,
                Err(_) => return Err(item_slot.take().expect("item present")),
            };
        }
    }

    /// Returns the number of items currently buffered.
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.state.lock().map_or(0, |s| s.buffer.len())
    }

    /// Returns `true` if the consumer has been dropped.
    #[must_use]
    pub fn is_consumer_gone(&self) -> bool {
        self.back_pressure.consumer_gone.lock().map_or(true, |g| *g)
    }
}

impl<T> Drop for AsyncStreamSender<T> {
    fn drop(&mut self) {
        // If this was the last sender, mark the stream closed and wake any
        // pending consumer so its `next()` returns `None`.
        let strong = Arc::strong_count(&self.state);
        if strong == 2 {
            // exactly one sender (`self`) + the consumer
            if let Ok(mut state) = self.state.lock() {
                state.closed = true;
                if let Some(w) = state.waker.take() {
                    w.wake();
                }
            }
        }
        // Wake any back-pressure-blocked clones so they bail out cleanly.
        self.back_pressure.cvar.notify_all();
    }
}

/// Future returned by [`BoundedAsyncStream::next`].
pub struct NextItem<'a, T> {
    stream: &'a BoundedAsyncStream<T>,
}

impl<T> fmt::Debug for NextItem<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NextItem").finish_non_exhaustive()
    }
}

impl<T> Future for NextItem<'_, T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Ok(mut state) = self.stream.state.lock() else {
            return Poll::Ready(None);
        };

        if let Some(item) = state.buffer.pop_front() {
            // Notify any push_or_block-blocked producers that there's room.
            self.stream.back_pressure.cvar.notify_all();
            return Poll::Ready(Some(item));
        }

        if state.closed {
            return Poll::Ready(None);
        }

        // Avoid the lost-wakeup race: when the executor re-polls with a
        // different waker (e.g. tokio::select! moves the future between
        // arms), the previous waker would otherwise remain stored and any
        // pending push would wake the wrong task. `will_wake` skips the
        // clone if the executor is reusing the same waker.
        let waker = cx.waker();
        match state.waker {
            Some(ref existing) if existing.will_wake(waker) => {}
            _ => state.waker = Some(waker.clone()),
        }
        Poll::Pending
    }
}

#[cfg(feature = "futures-stream")]
impl<T: 'static> futures_core::Stream for BoundedAsyncStream<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let Ok(mut state) = self.state.lock() else {
            return Poll::Ready(None);
        };

        if let Some(item) = state.buffer.pop_front() {
            self.back_pressure.cvar.notify_all();
            return Poll::Ready(Some(item));
        }

        if state.closed {
            return Poll::Ready(None);
        }

        let waker = cx.waker();
        match state.waker {
            Some(ref existing) if existing.will_wake(waker) => {}
            _ => state.waker = Some(waker.clone()),
        }
        Poll::Pending
    }
}
