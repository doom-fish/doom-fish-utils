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
    waiters: Vec<(u64, Waker)>,
    next_waiter: u64,
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

#[cfg(feature = "futures-stream")]
const STREAM_WAITER: u64 = 0;

impl<T> State<T> {
    fn register_waiter(
        &mut self,
        waiter: &mut Option<u64>,
        current: &Waker,
        next_waker: &mut Option<Waker>,
    ) -> Option<Waker> {
        let id = *waiter.get_or_insert_with(|| {
            let id = self.next_waiter;
            self.next_waiter = id.wrapping_add(1).max(1);
            id
        });
        match self
            .waiters
            .iter_mut()
            .find(|(registered, _)| *registered == id)
        {
            Some((_, existing)) if existing.will_wake(current) => None,
            Some((_, existing)) => next_waker
                .take()
                .map(|waker| std::mem::replace(existing, waker)),
            None => {
                self.waiters
                    .extend(next_waker.take().map(|waker| (id, waker)));
                None
            }
        }
    }

    fn remove_waiter(&mut self, waiter: Option<u64>) -> Option<Waker> {
        let waiter = waiter?;
        let index = self
            .waiters
            .iter()
            .position(|(registered, _)| *registered == waiter)?;
        Some(self.waiters.swap_remove(index).1)
    }
}

fn wake_waiters(waiters: Vec<(u64, Waker)>) {
    for (_, waker) in waiters {
        waker.wake();
    }
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
                waiters: Vec::new(),
                next_waiter: 1,
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
        NextItem {
            stream: self,
            waiter: None,
        }
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

    fn poll_next_item(&self, cx: &Context<'_>, waiter: &mut Option<u64>) -> Poll<Option<T>> {
        let mut next_waker = Some(cx.waker().clone());
        let (result, released_waker, made_room) = {
            let mut state = self.shared.lock_state();

            let outcome = match state.buffer.pop_front() {
                Some(item) => (
                    Poll::Ready(Some(item)),
                    state.remove_waiter(waiter.take()),
                    true,
                ),
                None if state.closed => {
                    (Poll::Ready(None), state.remove_waiter(waiter.take()), false)
                }
                None => {
                    let released = state.register_waiter(waiter, cx.waker(), &mut next_waker);
                    (Poll::Pending, released, false)
                }
            };
            drop(state);
            outcome
        };

        drop(next_waker);
        drop(released_waker);
        if made_room {
            self.shared.capacity_available.notify_one();
        }
        result
    }
}

impl<T> Drop for BoundedAsyncStream<T> {
    fn drop(&mut self) {
        let stale_wakers = {
            let mut state = self.shared.lock_state_for_drop();
            state.consumer_gone = true;
            std::mem::take(&mut state.waiters)
        };
        self.shared.capacity_available.notify_all();
        drop(stale_wakers);
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
        let (overwritten, waiters) = {
            let mut state = self.shared.lock_state();
            let overwritten = if state.buffer.len() >= self.shared.capacity {
                state.buffer.pop_front()
            } else {
                None
            };
            state.buffer.push_back(item);
            (overwritten, std::mem::take(&mut state.waiters))
        };

        wake_waiters(waiters);
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
        let waiters = std::mem::take(&mut state.waiters);
        drop(state);

        wake_waiters(waiters);
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
        let waiters = {
            let mut state = self.shared.lock_state_for_drop();
            state.sender_count -= 1;
            if state.sender_count == 0 {
                state.closed = true;
                std::mem::take(&mut state.waiters)
            } else {
                Vec::new()
            }
        };

        wake_waiters(waiters);
    }
}

/// Future returned by [`BoundedAsyncStream::next`].
pub struct NextItem<'a, T> {
    stream: &'a BoundedAsyncStream<T>,
    waiter: Option<u64>,
}

impl<T> fmt::Debug for NextItem<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NextItem").finish_non_exhaustive()
    }
}

impl<T> Future for NextItem<'_, T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.stream.poll_next_item(cx, &mut this.waiter)
    }
}

impl<T> Drop for NextItem<'_, T> {
    fn drop(&mut self) {
        if self.waiter.is_some() {
            let released = {
                let mut state = self.stream.shared.lock_state_for_drop();
                state.remove_waiter(self.waiter.take())
            };
            drop(released);
        }
    }
}

#[cfg(feature = "futures-stream")]
impl<T: 'static> futures_core::Stream for BoundedAsyncStream<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.poll_next_item(cx, &mut Some(STREAM_WAITER))
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

    #[derive(Default)]
    struct CountingWake(AtomicUsize);

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn counting_waker() -> (Arc<CountingWake>, Waker) {
        let probe = Arc::new(CountingWake::default());
        let waker = Waker::from(Arc::clone(&probe));
        (probe, waker)
    }

    fn poll_once<F: Future + Unpin>(future: &mut F, waker: &Waker) -> Poll<F::Output> {
        Pin::new(future).poll(&mut Context::from_waker(waker))
    }

    #[test]
    fn every_waiting_consumer_is_woken() {
        let (stream, sender) = BoundedAsyncStream::new(4);
        let (first_probe, first_waker) = counting_waker();
        let (second_probe, second_waker) = counting_waker();
        let mut first = stream.next();
        let mut second = stream.next();

        assert_eq!(poll_once(&mut first, &first_waker), Poll::Pending);
        assert_eq!(poll_once(&mut second, &second_waker), Poll::Pending);

        sender.push(1);
        assert_eq!(first_probe.0.load(Ordering::SeqCst), 1);
        assert_eq!(second_probe.0.load(Ordering::SeqCst), 1);

        assert_eq!(poll_once(&mut second, &second_waker), Poll::Ready(Some(1)));
        assert_eq!(poll_once(&mut first, &first_waker), Poll::Pending);

        drop(sender);
        assert_eq!(first_probe.0.load(Ordering::SeqCst), 2);
        assert_eq!(second_probe.0.load(Ordering::SeqCst), 1);
        assert_eq!(poll_once(&mut first, &first_waker), Poll::Ready(None));
    }

    #[test]
    fn dropped_next_future_releases_its_waker() {
        let (stream, sender) = BoundedAsyncStream::<u32>::new(1);
        let (first_probe, first_waker) = counting_waker();
        let (second_probe, second_waker) = counting_waker();
        let mut first = stream.next();
        let mut second = stream.next();

        for _ in 0..3 {
            assert_eq!(poll_once(&mut first, &first_waker), Poll::Pending);
        }
        assert_eq!(poll_once(&mut second, &second_waker), Poll::Pending);
        assert_eq!(stream.shared.lock_state().waiters.len(), 2);

        drop(first);
        assert_eq!(stream.shared.lock_state().waiters.len(), 1);
        drop(first_waker);
        assert_eq!(Arc::strong_count(&first_probe), 1);

        sender.push(5);
        assert_eq!(first_probe.0.load(Ordering::SeqCst), 0);
        assert_eq!(second_probe.0.load(Ordering::SeqCst), 1);
        assert_eq!(poll_once(&mut second, &second_waker), Poll::Ready(Some(5)));
        drop(second);
        assert!(stream.shared.lock_state().waiters.is_empty());
    }

    #[test]
    fn concurrent_consumers_drain_every_item() {
        const CONSUMERS: usize = 4;
        const ITEMS: usize = 2_000;

        let (stream, sender) = BoundedAsyncStream::<usize>::new(4);
        let stream = Arc::new(stream);
        let (done_tx, done_rx) = mpsc::channel();
        let consumers = (0..CONSUMERS)
            .map(|_| {
                let stream = Arc::clone(&stream);
                let done_tx = done_tx.clone();
                thread::spawn(move || {
                    let mut received = 0;
                    while pollster::block_on(stream.next()).is_some() {
                        received += 1;
                    }
                    done_tx.send(received).unwrap();
                })
            })
            .collect::<Vec<_>>();

        for item in 0..ITEMS {
            assert_eq!(sender.push_or_block(item), Ok(()));
        }
        drop(sender);

        let received: usize = (0..CONSUMERS)
            .map(|_| done_rx.recv_timeout(TEST_TIMEOUT).expect("a consumer hung"))
            .sum();
        assert_eq!(received, ITEMS);
        for consumer in consumers {
            consumer.join().unwrap();
        }
    }

    #[cfg(feature = "futures-stream")]
    #[test]
    fn stream_poll_keeps_a_single_registration() {
        let (mut stream, sender) = BoundedAsyncStream::<u32>::new(1);
        let (first_probe, first_waker) = counting_waker();
        let (second_probe, second_waker) = counting_waker();

        let mut first_cx = Context::from_waker(&first_waker);
        assert_eq!(
            Pin::new(&mut stream).poll_next(&mut first_cx),
            Poll::Pending
        );
        let mut second_cx = Context::from_waker(&second_waker);
        assert_eq!(
            Pin::new(&mut stream).poll_next(&mut second_cx),
            Poll::Pending
        );
        assert_eq!(stream.shared.lock_state().waiters.len(), 1);

        sender.push(3);
        assert_eq!(first_probe.0.load(Ordering::SeqCst), 0);
        assert_eq!(second_probe.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            Pin::new(&mut stream).poll_next(&mut second_cx),
            Poll::Ready(Some(3))
        );
    }
}
