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
//! use [`AsyncStreamSender::push_or_block`] which blocks the producer
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
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

/// Backing storage shared between the [`BoundedAsyncStream`] consumer and
/// every [`AsyncStreamSender`] producer.
struct State<T> {
    buffer: VecDeque<T>,
    waker: Option<Waker>,
    /// Set to `true` when every sender has been dropped. The consumer's
    /// `next()` then returns `None` once the buffer drains.
    closed: bool,
    /// Set to `true` when the stream is dropped — wakes any blocked
    /// producers so they can bail out instead of waiting forever.
    consumer_gone: bool,
    sender_count: usize,
    #[cfg(test)]
    blocked_producers: usize,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    capacity_available: Condvar,
    capacity: usize,
}

impl<T> Shared<T> {
    fn lock_state(&self) -> MutexGuard<'_, State<T>> {
        self.state
            .lock()
            .unwrap_or_else(|_| panic!("BoundedAsyncStream state mutex poisoned"))
    }

    fn lock_state_for_drop(&self) -> MutexGuard<'_, State<T>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(test)]
    fn wait_for_blocked_producers(&self, expected: usize) {
        let (state, timeout) = self
            .capacity_available
            .wait_timeout_while(
                self.lock_state(),
                std::time::Duration::from_secs(5),
                |state| state.blocked_producers < expected,
            )
            .unwrap_or_else(|_| panic!("BoundedAsyncStream state mutex poisoned"));
        assert!(
            !timeout.timed_out(),
            "timed out waiting for {expected} blocked producer(s)"
        );
        drop(state);
    }
}

/// A bounded, lossy-by-default, executor-agnostic async stream.
///
/// Items are pushed by one or more [`AsyncStreamSender`] handles and pulled
/// asynchronously via [`BoundedAsyncStream::next`].
///
/// See the [module-level docs](crate::stream) for the full design rationale.
pub struct BoundedAsyncStream<T> {
    shared: Arc<Shared<T>>,
}

/// Producer handle for a [`BoundedAsyncStream`].
///
/// Cheap to clone (`Arc` under the hood). Drop the last `AsyncStreamSender`
/// to close the stream; the consumer's `next()` will yield `None` once the
/// buffer is empty.
pub struct AsyncStreamSender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for AsyncStreamSender<T> {
    fn clone(&self) -> Self {
        let mut state = self.shared.lock_state();
        state.sender_count = state
            .sender_count
            .checked_add(1)
            .expect("AsyncStreamSender count overflow");
        drop(state);

        Self {
            shared: Arc::clone(&self.shared),
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

        let shared = Arc::new(Shared {
            capacity,
            capacity_available: Condvar::new(),
            state: Mutex::new(State {
                buffer: VecDeque::with_capacity(capacity),
                waker: None,
                closed: false,
                consumer_gone: false,
                sender_count: 1,
                #[cfg(test)]
                blocked_producers: 0,
            }),
        });

        let stream = Self {
            shared: Arc::clone(&shared),
        };
        let sender = AsyncStreamSender { shared };
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
    ///
    /// # Panics
    ///
    /// Panics if the shared state mutex is poisoned.
    #[must_use]
    pub fn try_next(&self) -> Option<T> {
        let item = {
            let mut state = self.shared.lock_state();
            state.buffer.pop_front()
        };
        if item.is_some() {
            self.shared.capacity_available.notify_one();
        }
        item
    }

    /// Returns `true` if the stream has been closed (all senders dropped).
    /// Note: a closed stream may still have buffered items to drain.
    ///
    /// # Panics
    ///
    /// Panics if the shared state mutex is poisoned.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.lock_state().closed
    }

    /// Returns the number of items currently buffered (0..=capacity).
    ///
    /// # Panics
    ///
    /// Panics if the shared state mutex is poisoned.
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.shared.lock_state().buffer.len()
    }

    /// Returns the buffer capacity, as passed to [`Self::new`].
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Drops all currently buffered items without closing the stream.
    ///
    /// # Panics
    ///
    /// Panics if the shared state mutex is poisoned.
    pub fn clear_buffer(&self) {
        let mut cleared = VecDeque::with_capacity(self.shared.capacity);
        {
            let mut state = self.shared.lock_state();
            std::mem::swap(&mut state.buffer, &mut cleared);
        }
        self.shared.capacity_available.notify_all();
        drop(cleared);
    }

    fn poll_next_item(&self, cx: &Context<'_>) -> Poll<Option<T>> {
        let mut next_waker = Some(cx.waker().clone());
        let (result, replaced_waker, made_room) = {
            let mut state = self.shared.lock_state();

            let outcome = match state.buffer.pop_front() {
                Some(item) => (Poll::Ready(Some(item)), None, true),
                None if state.closed => (Poll::Ready(None), None, false),
                None => {
                    let replaced_waker = match state.waker.as_ref() {
                        Some(existing) if existing.will_wake(cx.waker()) => None,
                        _ => state
                            .waker
                            .replace(next_waker.take().expect("next waker present")),
                    };
                    (Poll::Pending, replaced_waker, false)
                }
            };
            drop(state);
            outcome
        };

        drop(next_waker);
        drop(replaced_waker);
        if made_room {
            self.shared.capacity_available.notify_one();
        }
        result
    }
}

impl<T> Drop for BoundedAsyncStream<T> {
    fn drop(&mut self) {
        let stale_waker = {
            let mut state = self.shared.lock_state_for_drop();
            state.consumer_gone = true;
            state.waker.take()
        };
        self.shared.capacity_available.notify_all();
        drop(stale_waker);
    }
}

impl<T> AsyncStreamSender<T> {
    /// Push an item; drops the oldest queued item if the buffer is at
    /// capacity. This is the lossy default.
    ///
    /// # Panics
    ///
    /// Panics if the shared state mutex is poisoned.
    pub fn push(&self, item: T) {
        let (overwritten, waker) = {
            let mut state = self.shared.lock_state();
            let overwritten = if state.buffer.len() >= self.shared.capacity {
                state.buffer.pop_front()
            } else {
                None
            };
            state.buffer.push_back(item);
            (overwritten, state.waker.take())
        };

        if let Some(waker) = waker {
            waker.wake();
        }
        drop(overwritten);
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
    /// Panics if the shared state mutex is poisoned.
    pub fn push_or_block(&self, item: T) -> Result<(), T> {
        let mut state = self.shared.lock_state();
        if !state.consumer_gone && state.buffer.len() >= self.shared.capacity {
            #[cfg(test)]
            {
                state.blocked_producers += 1;
                self.shared.capacity_available.notify_all();
            }

            state = self
                .shared
                .capacity_available
                .wait_while(state, |state| {
                    !state.consumer_gone && state.buffer.len() >= self.shared.capacity
                })
                .unwrap_or_else(|_| panic!("BoundedAsyncStream state mutex poisoned"));

            #[cfg(test)]
            {
                state.blocked_producers -= 1;
                self.shared.capacity_available.notify_all();
            }
        }

        if state.consumer_gone {
            drop(state);
            return Err(item);
        }

        state.buffer.push_back(item);
        let waker = state.waker.take();
        drop(state);

        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }

    /// Returns the number of items currently buffered.
    ///
    /// # Panics
    ///
    /// Panics if the shared state mutex is poisoned.
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.shared.lock_state().buffer.len()
    }

    /// Returns `true` if the consumer has been dropped.
    ///
    /// # Panics
    ///
    /// Panics if the shared state mutex is poisoned.
    #[must_use]
    pub fn is_consumer_gone(&self) -> bool {
        self.shared.lock_state().consumer_gone
    }
}

impl<T> Drop for AsyncStreamSender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut state = self.shared.lock_state_for_drop();
            state.sender_count -= 1;
            if state.sender_count == 0 {
                state.closed = true;
                state.waker.take()
            } else {
                None
            }
        };

        if let Some(waker) = waker {
            waker.wake();
        }
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
        self.stream.poll_next_item(cx)
    }
}

#[cfg(feature = "futures-stream")]
impl<T: 'static> futures_core::Stream for BoundedAsyncStream<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.poll_next_item(cx)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "futures-stream")]
    use std::future::poll_fn;
    use std::future::Future;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Barrier, TryLockError, Weak};
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    #[cfg(feature = "futures-stream")]
    use futures_core::Stream;

    use super::{AsyncStreamSender, BoundedAsyncStream, Shared};

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn spawn_blocking_push(
        sender: AsyncStreamSender<u32>,
        item: u32,
    ) -> (mpsc::Receiver<Result<(), u32>>, JoinHandle<()>) {
        let (result_tx, result_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let result = sender.push_or_block(item);
            result_tx.send(result).unwrap();
        });
        (result_rx, handle)
    }

    #[test]
    fn try_next_notifies_blocked_producer() {
        let (stream, sender) = BoundedAsyncStream::new(1);
        sender.push(1);
        let (result_rx, handle) = spawn_blocking_push(sender, 2);
        stream.shared.wait_for_blocked_producers(1);

        assert_eq!(stream.try_next(), Some(1));
        assert_eq!(result_rx.recv_timeout(TEST_TIMEOUT).unwrap(), Ok(()));
        handle.join().unwrap();
        assert_eq!(stream.try_next(), Some(2));
    }

    #[test]
    fn next_future_notifies_blocked_producer() {
        let (stream, sender) = BoundedAsyncStream::new(1);
        sender.push(1);
        let (result_rx, handle) = spawn_blocking_push(sender, 2);
        stream.shared.wait_for_blocked_producers(1);

        assert_eq!(pollster::block_on(stream.next()), Some(1));
        assert_eq!(result_rx.recv_timeout(TEST_TIMEOUT).unwrap(), Ok(()));
        handle.join().unwrap();
        assert_eq!(pollster::block_on(stream.next()), Some(2));
    }

    #[cfg(feature = "futures-stream")]
    #[test]
    fn stream_poll_notifies_blocked_producer() {
        let (mut stream, sender) = BoundedAsyncStream::new(1);
        sender.push(1);
        let (result_rx, handle) = spawn_blocking_push(sender, 2);
        stream.shared.wait_for_blocked_producers(1);

        let first = pollster::block_on(poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)));
        assert_eq!(first, Some(1));
        assert_eq!(result_rx.recv_timeout(TEST_TIMEOUT).unwrap(), Ok(()));
        handle.join().unwrap();

        let second = pollster::block_on(poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)));
        assert_eq!(second, Some(2));
    }

    #[test]
    fn clear_buffer_notifies_blocked_producer() {
        let (stream, sender) = BoundedAsyncStream::new(1);
        sender.push(1);
        let (result_rx, handle) = spawn_blocking_push(sender, 2);
        stream.shared.wait_for_blocked_producers(1);

        stream.clear_buffer();
        assert_eq!(result_rx.recv_timeout(TEST_TIMEOUT).unwrap(), Ok(()));
        handle.join().unwrap();
        assert_eq!(stream.try_next(), Some(2));
    }

    #[test]
    fn consumer_drop_returns_blocked_item() {
        let (stream, sender) = BoundedAsyncStream::new(1);
        sender.push(1);
        let (result_rx, handle) = spawn_blocking_push(sender, 2);
        stream.shared.wait_for_blocked_producers(1);

        drop(stream);
        assert_eq!(result_rx.recv_timeout(TEST_TIMEOUT).unwrap(), Err(2));
        handle.join().unwrap();
    }

    #[test]
    fn concurrent_sender_clone_drops_close_stream() {
        let (stream, sender) = BoundedAsyncStream::<u32>::new(1);
        let sender_clone = sender.clone();
        let barrier = Arc::new(Barrier::new(3));

        let first_barrier = Arc::clone(&barrier);
        let first = thread::spawn(move || {
            first_barrier.wait();
            drop(sender);
        });
        let second_barrier = Arc::clone(&barrier);
        let second = thread::spawn(move || {
            second_barrier.wait();
            drop(sender_clone);
        });

        barrier.wait();
        first.join().unwrap();
        second.join().unwrap();

        assert!(stream.is_closed());
        assert_eq!(pollster::block_on(stream.next()), None);
    }

    fn shared_state_is_unlocked<T>(shared: &Weak<Shared<T>>) -> bool {
        let Some(shared) = shared.upgrade() else {
            return true;
        };
        let unlocked = match shared.state.try_lock() {
            Ok(_state) => true,
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => false,
        };
        unlocked
    }

    struct ReentrantWaker {
        shared: Weak<Shared<u32>>,
        wake_was_unlocked: Arc<AtomicBool>,
        drop_was_unlocked: Arc<AtomicBool>,
    }

    impl Wake for ReentrantWaker {
        fn wake(self: Arc<Self>) {
            self.wake_was_unlocked
                .store(shared_state_is_unlocked(&self.shared), Ordering::SeqCst);
        }
    }

    impl Drop for ReentrantWaker {
        fn drop(&mut self) {
            self.drop_was_unlocked
                .store(shared_state_is_unlocked(&self.shared), Ordering::SeqCst);
        }
    }

    #[test]
    fn wakes_and_drops_waker_outside_state_mutex() {
        let (stream, sender) = BoundedAsyncStream::new(1);
        let wake_was_unlocked = Arc::new(AtomicBool::new(false));
        let drop_was_unlocked = Arc::new(AtomicBool::new(false));
        let probe = Arc::new(ReentrantWaker {
            shared: Arc::downgrade(&stream.shared),
            wake_was_unlocked: Arc::clone(&wake_was_unlocked),
            drop_was_unlocked: Arc::clone(&drop_was_unlocked),
        });
        let waker = Waker::from(Arc::clone(&probe));
        let mut next = stream.next();

        {
            let mut cx = Context::from_waker(&waker);
            assert_eq!(Pin::new(&mut next).poll(&mut cx), Poll::Pending);
        }
        drop(waker);
        drop(probe);

        sender.push(1);

        assert!(wake_was_unlocked.load(Ordering::SeqCst));
        assert!(drop_was_unlocked.load(Ordering::SeqCst));
    }

    struct ReentrantItem {
        shared: Weak<Shared<Self>>,
        unlocked_drops: Arc<AtomicUsize>,
        locked_drops: Arc<AtomicUsize>,
    }

    impl Drop for ReentrantItem {
        fn drop(&mut self) {
            if shared_state_is_unlocked(&self.shared) {
                self.unlocked_drops.fetch_add(1, Ordering::SeqCst);
            } else {
                self.locked_drops.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[test]
    fn overwritten_and_cleared_items_drop_outside_state_mutex() {
        let (stream, sender) = BoundedAsyncStream::new(1);
        let unlocked_drops = Arc::new(AtomicUsize::new(0));
        let locked_drops = Arc::new(AtomicUsize::new(0));

        let make_item = || ReentrantItem {
            shared: Arc::downgrade(&stream.shared),
            unlocked_drops: Arc::clone(&unlocked_drops),
            locked_drops: Arc::clone(&locked_drops),
        };

        sender.push(make_item());
        sender.push(make_item());
        assert_eq!(unlocked_drops.load(Ordering::SeqCst), 1);

        stream.clear_buffer();
        assert_eq!(unlocked_drops.load(Ordering::SeqCst), 2);
        assert_eq!(locked_drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn poisoned_state_does_not_masquerade_as_close_or_delivery() {
        let (stream, sender) = BoundedAsyncStream::new(1);
        let shared = Arc::clone(&stream.shared);
        assert!(thread::spawn(move || {
            let _state = shared.state.lock().unwrap();
            panic!("poison stream state");
        })
        .join()
        .is_err());

        assert!(catch_unwind(AssertUnwindSafe(|| sender.push(1))).is_err());
        assert!(catch_unwind(AssertUnwindSafe(|| stream.try_next())).is_err());
        assert!(catch_unwind(AssertUnwindSafe(|| pollster::block_on(stream.next()))).is_err());
    }
}
