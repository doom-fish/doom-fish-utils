//! Single-producer single-consumer lock-free bounded ring buffer.
//!
//! Designed for the `CoreAudio` render-thread → async-consumer producer-consumer
//! pattern. The producer path never takes a mutex and never allocates after the
//! ring has been constructed.
//!
//! Internally this wrapper uses a pre-allocated bounded queue plus an
//! [`AtomicWaker`](futures_util::task::AtomicWaker) so the consumer can await
//! the next item without forcing the producer to block.
//!
//! # Example
//!
//! ```no_run
//! use doom_fish_utils::spsc::SpscRing;
//!
//! # async fn run() {
//! let (producer, consumer) = SpscRing::<u32, 256>::new();
//!
//! producer.push(1).unwrap();
//! assert_eq!(consumer.pop_async().await, Some(1));
//! # }
//! ```

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use crossbeam_queue::ArrayQueue;
use futures_util::task::AtomicWaker;

struct Inner<T> {
    queue: ArrayQueue<T>,
    producer_closed: AtomicBool,
    waker: AtomicWaker,
}

/// Constructor namespace for a bounded single-producer single-consumer ring.
///
/// `N` is the maximum supported capacity. Use [`Self::new`] to allocate a ring
/// with exactly `N` slots, or [`Self::with_capacity`] to choose a smaller
/// runtime capacity while keeping the type-level upper bound.
#[derive(Debug, Default)]
pub struct SpscRing<T, const N: usize>(PhantomData<T>);

/// Producer half of an [`SpscRing`].
pub struct SpscProducer<T, const N: usize> {
    inner: Arc<Inner<T>>,
}

/// Consumer half of an [`SpscRing`].
pub struct SpscConsumer<T, const N: usize> {
    inner: Arc<Inner<T>>,
}

/// Future returned by [`SpscConsumer::pop_async`].
#[must_use = "futures do nothing unless awaited or polled"]
pub struct PopFuture<'a, T, const N: usize> {
    consumer: &'a SpscConsumer<T, N>,
}

/// Feature-gated [`futures_core::Stream`] wrapper around an [`SpscConsumer`].
#[cfg(feature = "futures-stream")]
#[cfg_attr(docsrs, doc(cfg(feature = "futures-stream")))]
#[must_use = "streams do nothing unless polled"]
pub struct SpscConsumerStream<'a, T, const N: usize> {
    consumer: &'a SpscConsumer<T, N>,
}

#[allow(clippy::new_ret_no_self)]
impl<T, const N: usize> SpscRing<T, N> {
    /// Creates a ring with capacity `N`.
    ///
    /// # Panics
    ///
    /// Panics if `N` is 0.
    #[must_use]
    pub fn new() -> (SpscProducer<T, N>, SpscConsumer<T, N>) {
        Self::with_capacity(N)
    }

    /// Creates a ring with a runtime capacity up to the type-level maximum `N`.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is 0 or larger than `N`.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> (SpscProducer<T, N>, SpscConsumer<T, N>) {
        assert!(N > 0, "SpscRing capacity must be > 0");
        assert!(capacity > 0, "SpscRing capacity must be > 0");
        assert!(
            capacity <= N,
            "SpscRing capacity {capacity} exceeds type maximum {N}"
        );

        let inner = Arc::new(Inner {
            queue: ArrayQueue::new(capacity),
            producer_closed: AtomicBool::new(false),
            waker: AtomicWaker::new(),
        });

        (
            SpscProducer {
                inner: Arc::clone(&inner),
            },
            SpscConsumer { inner },
        )
    }
}

impl<T, const N: usize> fmt::Debug for SpscProducer<T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpscProducer")
            .field("buffered", &self.buffered_count())
            .field("capacity", &self.capacity())
            .finish_non_exhaustive()
    }
}

impl<T, const N: usize> fmt::Debug for SpscConsumer<T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpscConsumer")
            .field("buffered", &self.buffered_count())
            .field("capacity", &self.capacity())
            .field("is_closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

impl<T, const N: usize> fmt::Debug for PopFuture<'_, T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PopFuture").finish_non_exhaustive()
    }
}

#[cfg(feature = "futures-stream")]
impl<T, const N: usize> fmt::Debug for SpscConsumerStream<'_, T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpscConsumerStream").finish_non_exhaustive()
    }
}

impl<T, const N: usize> SpscProducer<T, N> {
    /// Attempts to push an item into the ring without blocking.
    ///
    /// # Errors
    ///
    /// Returns `Err(item)` if the ring is currently full.
    pub fn push(&self, item: T) -> Result<(), T> {
        match self.inner.queue.push(item) {
            Ok(()) => {
                self.inner.waker.wake();
                Ok(())
            }
            Err(item) => Err(item),
        }
    }

    /// Pushes an item into the ring, overwriting the oldest buffered entry if
    /// necessary.
    ///
    /// Returns the displaced oldest item when an overwrite happens.
    pub fn push_overwrite(&self, item: T) -> Option<T> {
        let dropped = self.inner.queue.force_push(item);
        self.inner.waker.wake();
        dropped
    }

    /// Returns the current buffered item count.
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.inner.queue.len()
    }

    /// Returns the runtime capacity of the ring.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.queue.capacity()
    }
}

impl<T, const N: usize> Drop for SpscProducer<T, N> {
    fn drop(&mut self) {
        self.inner.producer_closed.store(true, Ordering::Release);
        self.inner.waker.wake();
    }
}

impl<T, const N: usize> SpscConsumer<T, N> {
    /// Attempts to pop the next buffered item without blocking.
    #[must_use]
    pub fn pop(&self) -> Option<T> {
        self.inner.queue.pop()
    }

    /// Returns a future that resolves to the next buffered item, or `None` once
    /// the producer has been dropped and the ring is empty.
    pub const fn pop_async(&self) -> PopFuture<'_, T, N> {
        PopFuture { consumer: self }
    }

    /// Returns the current buffered item count.
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.inner.queue.len()
    }

    /// Returns the runtime capacity of the ring.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.queue.capacity()
    }

    /// Returns `true` if the producer has been dropped.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.producer_closed.load(Ordering::Acquire)
    }

    #[cfg(feature = "futures-stream")]
    #[cfg_attr(docsrs, doc(cfg(feature = "futures-stream")))]
    pub const fn stream(&self) -> SpscConsumerStream<'_, T, N> {
        SpscConsumerStream { consumer: self }
    }

    fn poll_pop(&self, cx: &Context<'_>) -> Poll<Option<T>> {
        if let Some(item) = self.pop() {
            return Poll::Ready(Some(item));
        }

        if self.is_closed() {
            return Poll::Ready(None);
        }

        self.inner.waker.register(cx.waker());

        if let Some(item) = self.pop() {
            return Poll::Ready(Some(item));
        }

        if self.is_closed() {
            return Poll::Ready(None);
        }

        Poll::Pending
    }
}

impl<T, const N: usize> Future for PopFuture<'_, T, N> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.consumer.poll_pop(cx)
    }
}

#[cfg(feature = "futures-stream")]
impl<T, const N: usize> futures_core::Stream for SpscConsumerStream<'_, T, N> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.consumer.poll_pop(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;
    use std::pin::Pin;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::SpscRing;

    #[test]
    fn preserves_sequence_in_single_thread() {
        let (producer, consumer) = SpscRing::<u32, 4>::new();

        assert_eq!(producer.push(1), Ok(()));
        assert_eq!(producer.push(2), Ok(()));
        assert_eq!(producer.push(3), Ok(()));

        assert_eq!(consumer.pop(), Some(1));
        assert_eq!(consumer.pop(), Some(2));
        assert_eq!(consumer.pop(), Some(3));
        assert_eq!(consumer.pop(), None);
    }

    #[test]
    fn overwrite_drops_oldest_item() {
        let (producer, consumer) = SpscRing::<u32, 2>::new();

        assert_eq!(producer.push_overwrite(10), None);
        assert_eq!(producer.push_overwrite(20), None);
        assert_eq!(producer.push_overwrite(30), Some(10));

        assert_eq!(consumer.pop(), Some(20));
        assert_eq!(consumer.pop(), Some(30));
        assert_eq!(consumer.pop(), None);
    }

    #[test]
    fn producer_calls_return_immediately_when_full() {
        let (producer, _consumer) = SpscRing::<u64, 1>::new();
        assert_eq!(producer.push(7), Ok(()));

        let start = Instant::now();
        let mut expected_drop = Some(7);
        for value in 0..100_000 {
            assert_eq!(producer.push(value), Err(value));
            assert_eq!(producer.push_overwrite(value), expected_drop);
            expected_drop = Some(value);
        }

        assert!(
            start.elapsed() < Duration::from_secs(2),
            "producer operations took too long while the ring stayed full"
        );
    }

    #[test]
    fn pop_async_drains_then_closes() {
        let (producer, consumer) = SpscRing::<u32, 8>::new();
        producer.push(1).unwrap();
        producer.push(2).unwrap();
        drop(producer);

        assert_eq!(pollster::block_on(consumer.pop_async()), Some(1));
        assert_eq!(pollster::block_on(consumer.pop_async()), Some(2));
        assert_eq!(pollster::block_on(consumer.pop_async()), None);
    }

    #[test]
    fn concurrent_producer_consumer_preserve_order() {
        let (producer, consumer) = SpscRing::<u64, 1024>::new();
        let producer_thread = thread::spawn(move || {
            for expected in 0..50_000_u64 {
                let mut item = expected;
                loop {
                    match producer.push(item) {
                        Ok(()) => break,
                        Err(returned) => {
                            item = returned;
                            std::hint::spin_loop();
                        }
                    }
                }
            }
        });

        for expected in 0..50_000_u64 {
            let actual = pollster::block_on(consumer.pop_async());
            assert_eq!(actual, Some(expected));
        }
        assert_eq!(pollster::block_on(consumer.pop_async()), None);

        producer_thread.join().unwrap();
    }

    #[cfg(feature = "futures-stream")]
    #[test]
    fn stream_wrapper_yields_items() {
        use futures_core::Stream;

        let (producer, consumer) = SpscRing::<u32, 4>::new();
        let mut stream = consumer.stream();

        producer.push(11).unwrap();
        drop(producer);

        let first = pollster::block_on(poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)));
        let second = pollster::block_on(poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)));

        assert_eq!(first, Some(11));
        assert_eq!(second, None);
    }
}
