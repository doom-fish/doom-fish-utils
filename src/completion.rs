//! Synchronous completion utilities for async FFI callbacks
//!
//! This module provides a generic mechanism for blocking on async Swift FFI callbacks
//! and propagating results (success or error) back to Rust synchronously.
//!
//! # Example
//!
//! ```no_run
//! use doom_fish_utils::completion::SyncCompletion;
//!
//! // Create completion for a String result
//! let (completion, _context) = SyncCompletion::<String>::new();
//!
//! // In real use, context would be passed to FFI callback
//! // The callback would signal completion with a result
//!
//! // Block until callback completes (would hang without callback)
//! // let result = completion.wait();
//! ```

use std::ffi::{c_char, c_void, CStr};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::panic_safe::catch_user_panic;

// ============================================================================
// Synchronous Completion (blocking)
// ============================================================================

/// Internal state for tracking synchronous completion.
///
/// The result is wrapped in a single `Option` rather than tracking
/// `(completed: bool, result: Option<Result<…>>)` separately so that the
/// "completed but no result" state is unrepresentable: `None` means
/// "not yet completed" and `Some(_)` means "completed with this result".
struct SyncCompletionState<T> {
    result: Option<Result<T, String>>,
}

/// Backing storage for `SyncCompletion`.
struct SyncCompletionInner<T> {
    /// Rejects a duplicate callback only while this allocation is still live.
    /// Reading the flag already requires dereferencing the raw context, so it
    /// cannot validate dangling or reused pointers.
    consumed: AtomicBool,
    state: Mutex<SyncCompletionState<T>>,
    cvar: Condvar,
}

/// A synchronous completion handler for async FFI callbacks
///
/// This type provides a way to block until an async callback completes
/// and retrieve the result. It uses `Arc<...>` internally for thread-safe
/// signaling between the callback and the waiting thread. The raw context
/// returned by [`Self::new`] is exact-live and one-shot; its atomic consumed
/// flag can only diagnose a duplicate callback while the allocation remains
/// live.
pub struct SyncCompletion<T> {
    inner: Arc<SyncCompletionInner<T>>,
}

/// Raw pointer type for passing to FFI callbacks
pub type SyncCompletionPtr = *mut c_void;

impl<T> SyncCompletion<T> {
    /// Create a new completion handler and return the context pointer for FFI
    ///
    /// Returns a tuple of (completion, `context_ptr`) where:
    /// - `completion` is used to wait for and retrieve the result
    /// - `context_ptr` should be passed to the FFI callback
    #[must_use]
    pub fn new() -> (Self, SyncCompletionPtr) {
        let inner = Arc::new(SyncCompletionInner {
            consumed: AtomicBool::new(false),
            state: Mutex::new(SyncCompletionState { result: None }),
            cvar: Condvar::new(),
        });
        let raw = Arc::into_raw(Arc::clone(&inner));
        (Self { inner }, raw as SyncCompletionPtr)
    }

    /// Wait for the completion callback and return the result
    ///
    /// This method blocks until the callback signals completion.
    ///
    /// # Errors
    ///
    /// Returns an error string if the callback signaled an error.
    pub fn wait(self) -> Result<T, String> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(result) = state.result.take() {
                return result;
            }
            state = self
                .inner
                .cvar
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    #[must_use]
    pub fn wait_timeout(self, timeout: Duration) -> Option<Result<T, String>> {
        let start = Instant::now();
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(result) = state.result.take() {
                return Some(result);
            }
            let remaining = timeout.checked_sub(start.elapsed())?;
            state = self
                .inner
                .cvar
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    #[must_use]
    pub fn context_ptr(&self) -> SyncCompletionPtr {
        Arc::as_ptr(&self.inner).cast_mut().cast()
    }

    /// Signal successful completion with a value
    ///
    /// # Safety
    ///
    /// `context` must be the exact live pointer returned by
    /// [`SyncCompletion::new`] or [`SyncCompletion::context_ptr`]. This
    /// consumes the callback-owned `Arc` reference and must be invoked
    /// exactly once for that context. `T` must be `Send` if this is called
    /// on a thread other than the one that waits for the result.
    pub unsafe fn complete_ok(context: SyncCompletionPtr, value: T) {
        Self::complete_with_result(context, Ok(value));
    }

    /// Signal completion with an error
    ///
    /// # Safety
    ///
    /// `context` must be the exact live pointer returned by
    /// [`SyncCompletion::new`] or [`SyncCompletion::context_ptr`]. This
    /// consumes the callback-owned `Arc` reference and must be invoked
    /// exactly once for that context. `T` must be `Send` if this is called
    /// on a thread other than the one that waits for the result.
    pub unsafe fn complete_err(context: SyncCompletionPtr, error: String) {
        Self::complete_with_result(context, Err(error));
    }

    /// Signal completion with a result
    ///
    /// # Safety
    ///
    /// `context` must be the exact pointer returned by
    /// [`SyncCompletion::new`] or [`SyncCompletion::context_ptr`], its
    /// allocation must remain live for this entire call, and foreign code
    /// must invoke this completion exactly once and never use the pointer
    /// afterward.
    ///
    /// `T` must be `Send` if this is called on a thread other than the one
    /// that waits for the result: the value moves to the waiting thread, and
    /// it is dropped on the calling thread if the waiter has already gone.
    ///
    /// The `consumed` flag is only defence in depth for a duplicate call
    /// while the allocation is still live. Checking that flag itself
    /// dereferences `context`; it does not make an already-freed, reused,
    /// or concurrently invalidated pointer safe.
    pub unsafe fn complete_with_result(context: SyncCompletionPtr, result: Result<T, String>) {
        if context.is_null() {
            return;
        }

        // Atomic guard against double-invocation. We deref the raw pointer
        // *without* taking ownership of the Arc reference; only the call
        // that wins the swap proceeds to `Arc::from_raw`.
        let inner_ref = unsafe { &*context.cast::<SyncCompletionInner<T>>() };
        if inner_ref.consumed.swap(true, Ordering::AcqRel) {
            eprintln!(
                "doom-fish-utils: SyncCompletion callback fired more than once; \
                 ignoring duplicate to avoid double-free"
            );
            return;
        }

        let inner = unsafe { Arc::from_raw(context.cast::<SyncCompletionInner<T>>()) };
        {
            // Poison-tolerant: this runs inside the FFI completion callback, so a
            // panic here would unwind across the `extern "C"` boundary (UB).
            let mut state = inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.result = Some(result);
        }
        inner.cvar.notify_one();
    }
}

impl<T> Default for SyncCompletion<T> {
    fn default() -> Self {
        Self::new().0
    }
}

// ============================================================================
// Asynchronous Completion (Future-based)
// ============================================================================

/// Internal state for tracking async completion
struct AsyncCompletionState<T> {
    result: Option<Result<T, String>>,
    waker: Option<Waker>,
}

/// Backing storage for `AsyncCompletion` — held behind an `Arc`. The
/// `consumed` flag has the same live-allocation limitation documented on
/// `SyncCompletionInner`.
struct AsyncCompletionInner<T> {
    consumed: AtomicBool,
    state: Mutex<AsyncCompletionState<T>>,
}

/// An async completion handler for FFI callbacks
///
/// This type provides a `Future` that resolves when an async callback completes.
/// It uses `Arc<Mutex>` internally for thread-safe signaling and waker management.
/// The raw context returned by [`Self::create`] is exact-live and one-shot.
pub struct AsyncCompletion<T> {
    _marker: std::marker::PhantomData<T>,
}

/// Future returned by `AsyncCompletion`
pub struct AsyncCompletionFuture<T> {
    inner: Arc<AsyncCompletionInner<T>>,
}

impl<T> AsyncCompletion<T> {
    /// Create a new async completion handler and return the context pointer for FFI
    ///
    /// Returns a tuple of (future, `context_ptr`) where:
    /// - `future` can be awaited to get the result
    /// - `context_ptr` should be passed to the FFI callback
    #[must_use]
    pub fn create() -> (AsyncCompletionFuture<T>, SyncCompletionPtr) {
        let inner = Arc::new(AsyncCompletionInner {
            consumed: AtomicBool::new(false),
            state: Mutex::new(AsyncCompletionState {
                result: None,
                waker: None,
            }),
        });
        let raw = Arc::into_raw(Arc::clone(&inner));
        (AsyncCompletionFuture { inner }, raw as SyncCompletionPtr)
    }

    /// Signal successful completion with a value
    ///
    /// # Safety
    ///
    /// `context` must be the exact live pointer returned by
    /// [`AsyncCompletion::create`]. This consumes the callback-owned `Arc`
    /// reference and must be invoked exactly once for that context. `T` must
    /// be `Send` if this is called on a thread other than the one that polls
    /// the future.
    pub unsafe fn complete_ok(context: SyncCompletionPtr, value: T) {
        Self::complete_with_result(context, Ok(value));
    }

    /// Signal completion with an error
    ///
    /// # Safety
    ///
    /// `context` must be the exact live pointer returned by
    /// [`AsyncCompletion::create`]. This consumes the callback-owned `Arc`
    /// reference and must be invoked exactly once for that context. `T` must
    /// be `Send` if this is called on a thread other than the one that polls
    /// the future.
    pub unsafe fn complete_err(context: SyncCompletionPtr, error: String) {
        Self::complete_with_result(context, Err(error));
    }

    /// Signal completion with a result
    ///
    /// # Safety
    ///
    /// `context` must be the exact pointer returned by
    /// [`AsyncCompletion::create`], its allocation must remain live for
    /// this entire call, and foreign code must invoke this completion
    /// exactly once and never use the pointer afterward.
    ///
    /// `T` must be `Send` if this is called on a thread other than the one
    /// that polls the future: the value moves to that thread, and it is
    /// dropped on the calling thread if the future has already been dropped.
    ///
    /// The `consumed` flag only rejects a duplicate call while the
    /// allocation is still live. It cannot validate an already-freed,
    /// reused, or concurrently invalidated pointer.
    pub unsafe fn complete_with_result(context: SyncCompletionPtr, result: Result<T, String>) {
        if context.is_null() {
            return;
        }

        let inner_ref = unsafe { &*context.cast::<AsyncCompletionInner<T>>() };
        if inner_ref.consumed.swap(true, Ordering::AcqRel) {
            eprintln!(
                "doom-fish-utils: AsyncCompletion callback fired more than once; \
                 ignoring duplicate to avoid double-free"
            );
            return;
        }

        let inner = unsafe { Arc::from_raw(context.cast::<AsyncCompletionInner<T>>()) };

        let waker = {
            // Poison-tolerant: this runs inside the FFI completion callback, so a
            // panic here would unwind across the `extern "C"` boundary (UB).
            let mut state = inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.result = Some(result);
            state.waker.take()
        };

        if let Some(w) = waker {
            w.wake();
        }

        // Drop the Arc here - the refcount was incremented in create() via Arc::clone(),
        // so the data stays alive via the AsyncCompletionFuture's Arc until it's dropped.
        // Dropping here decrements the refcount from the into_raw() call.
    }
}

impl<T> Future for AsyncCompletionFuture<T> {
    type Output = Result<T, String>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        state.result.take().map_or_else(
            || {
                // Avoid the lost-wakeup race: when the executor re-polls
                // with a different waker (e.g. tokio::select! moves the
                // future between arms), the previous waker would otherwise
                // remain stored and any pending callback would wake the
                // wrong task. `will_wake` skips the clone if the executor
                // is reusing the same waker.
                let waker = cx.waker();
                match state.waker {
                    Some(ref existing) if existing.will_wake(waker) => {}
                    _ => state.waker = Some(waker.clone()),
                }
                Poll::Pending
            },
            Poll::Ready,
        )
    }
}

// ============================================================================
// Shared Utilities
// ============================================================================

/// Helper to extract error message from a C string pointer
///
/// # Safety
///
/// The `msg` pointer must be either null or point to a valid null-terminated C string.
#[must_use]
pub unsafe fn error_from_cstr(msg: *const c_char) -> String {
    if msg.is_null() {
        "Unknown error".to_string()
    } else {
        CStr::from_ptr(msg)
            .to_str()
            .map_or_else(|_| "Unknown error".to_string(), String::from)
    }
}

/// Unit completion - for operations that return success/error without a value
pub type UnitCompletion = SyncCompletion<()>;

impl UnitCompletion {
    /// C callback for operations that return (context, success, `error_msg`)
    ///
    /// This can be used directly wherever a
    /// [`crate::ffi_callbacks::UnitCompletionCallback`] is required.
    ///
    /// The body is wrapped in [`catch_user_panic`] so that a mutex-poison
    /// panic (or any other unexpected panic) does not unwind across the
    /// `extern "C"` boundary, which would be undefined behaviour.
    ///
    /// # Safety
    ///
    /// `context` must be the exact live pointer returned with this
    /// `UnitCompletion`, must be invoked exactly once, and must not be used
    /// after this call. The internal atomic flag does not protect storage
    /// that has already been freed or concurrently invalidated.
    ///
    /// When `success` is `false`, `msg` must be null or point to a valid
    /// NUL-terminated C string for the duration of this call. It is ignored
    /// when `success` is `true`.
    pub unsafe extern "C" fn callback(context: *mut c_void, success: bool, msg: *const c_char) {
        catch_user_panic("UnitCompletion::callback", || {
            if success {
                unsafe { Self::complete_ok(context, ()) };
            } else {
                let error = unsafe { error_from_cstr(msg) };
                unsafe { Self::complete_err(context, error) };
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{AsyncCompletion, SyncCompletion, UnitCompletion};

    #[test]
    fn unit_completion_callback_matches_shared_alias() {
        let callback: crate::ffi_callbacks::UnitCompletionCallback = UnitCompletion::callback;
        let _ = callback;
    }

    #[test]
    fn unit_completion_callback_completes_successfully() {
        let (completion, context) = UnitCompletion::new();

        unsafe { UnitCompletion::callback(context, true, ptr::null()) };

        assert_eq!(completion.wait(), Ok(()));
    }

    #[test]
    fn unit_completion_callback_reports_errors() {
        let (completion, context) = UnitCompletion::new();

        unsafe { UnitCompletion::callback(context, false, c"denied".as_ptr()) };

        assert_eq!(completion.wait(), Err("denied".to_string()));
    }

    struct DropCounter(Arc<AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn sync_completion_ignores_a_duplicate_callback() {
        let (completion, context) = SyncCompletion::<u32>::new();

        unsafe { SyncCompletion::<u32>::complete_ok(context, 1) };
        unsafe { SyncCompletion::<u32>::complete_ok(context, 2) };

        assert_eq!(completion.wait(), Ok(1));
    }

    #[test]
    fn sync_completion_waits_for_another_thread() {
        let (completion, context) = SyncCompletion::<String>::new();
        let context = context as usize;

        let callback = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            unsafe { SyncCompletion::<String>::complete_ok(context as *mut _, "done".to_string()) };
        });

        assert_eq!(completion.wait(), Ok("done".to_string()));
        callback.join().unwrap();
    }

    #[test]
    fn wait_timeout_returns_none_when_no_callback_arrives() {
        let (completion, _context) = SyncCompletion::<u32>::new();
        let timeout = Duration::from_millis(30);
        let start = Instant::now();

        assert_eq!(completion.wait_timeout(timeout), None);
        assert!(start.elapsed() >= timeout);
    }

    #[test]
    fn wait_timeout_returns_the_result() {
        let (completion, context) = SyncCompletion::<u32>::new();
        unsafe { SyncCompletion::<u32>::complete_err(context, "failed".to_string()) };

        assert_eq!(
            completion.wait_timeout(Duration::from_secs(5)),
            Some(Err("failed".to_string()))
        );

        let (completion, context) = SyncCompletion::<u32>::new();
        let context = context as usize;
        let callback = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            unsafe { SyncCompletion::<u32>::complete_ok(context as *mut _, 9) };
        });

        assert_eq!(completion.wait_timeout(Duration::MAX), Some(Ok(9)));
        callback.join().unwrap();
    }

    #[test]
    fn late_callback_after_timeout_releases_the_value() {
        let drops = Arc::new(AtomicUsize::new(0));
        let (completion, context) = SyncCompletion::<DropCounter>::new();

        assert!(completion.wait_timeout(Duration::from_millis(1)).is_none());
        unsafe {
            SyncCompletion::<DropCounter>::complete_ok(context, DropCounter(Arc::clone(&drops)));
        };

        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn default_sync_completion_can_be_completed() {
        let completion = SyncCompletion::<u32>::default();
        let context = completion.context_ptr();

        unsafe { SyncCompletion::<u32>::complete_ok(context, 5) };

        assert_eq!(completion.wait(), Ok(5));

        let (completion, context) = SyncCompletion::<u32>::new();
        assert_eq!(completion.context_ptr(), context);
        unsafe { SyncCompletion::<u32>::complete_ok(context, 6) };
        assert_eq!(completion.wait(), Ok(6));
    }

    #[test]
    fn wait_recovers_from_a_poisoned_lock() {
        let (completion, context) = SyncCompletion::<u32>::new();
        let inner = Arc::clone(&completion.inner);
        assert!(thread::spawn(move || {
            let _state = inner.state.lock().unwrap();
            panic!("poison completion state");
        })
        .join()
        .is_err());

        unsafe { SyncCompletion::<u32>::complete_ok(context, 3) };

        assert_eq!(completion.wait(), Ok(3));

        let (completion, _context) = SyncCompletion::<u32>::new();
        let inner = Arc::clone(&completion.inner);
        assert!(thread::spawn(move || {
            let _state = inner.state.lock().unwrap();
            panic!("poison completion state");
        })
        .join()
        .is_err());

        assert_eq!(completion.wait_timeout(Duration::from_millis(1)), None);
    }

    #[derive(Default)]
    struct CountingWake(AtomicUsize);

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn async_completion_resolves_with_the_value() {
        let (future, context) = AsyncCompletion::<u32>::create();

        unsafe { AsyncCompletion::<u32>::complete_ok(context, 42) };

        assert_eq!(pollster::block_on(future), Ok(42));
    }

    #[test]
    fn async_completion_wakes_a_pending_future() {
        let (mut future, context) = AsyncCompletion::<u32>::create();
        let probe = Arc::new(CountingWake::default());
        let waker = Waker::from(Arc::clone(&probe));
        let mut cx = Context::from_waker(&waker);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);

        let context = context as usize;
        thread::spawn(move || unsafe { AsyncCompletion::<u32>::complete_ok(context as *mut _, 8) })
            .join()
            .unwrap();

        assert_eq!(probe.0.load(Ordering::SeqCst), 1);
        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Ready(Ok(8)));
    }

    #[test]
    fn async_completion_resolves_with_the_error() {
        let (future, context) = AsyncCompletion::<u32>::create();

        unsafe { AsyncCompletion::<u32>::complete_err(context, "denied".to_string()) };

        assert_eq!(pollster::block_on(future), Err("denied".to_string()));
    }

    #[test]
    fn async_completion_ignores_a_duplicate_callback() {
        let drops = Arc::new(AtomicUsize::new(0));
        let (future, context) = AsyncCompletion::<DropCounter>::create();

        unsafe {
            AsyncCompletion::<DropCounter>::complete_ok(context, DropCounter(Arc::clone(&drops)));
        };
        unsafe { AsyncCompletion::<DropCounter>::complete_err(context, "duplicate".to_string()) };

        let value = pollster::block_on(future).unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(value);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropped_async_future_releases_a_late_value() {
        let drops = Arc::new(AtomicUsize::new(0));
        let (future, context) = AsyncCompletion::<DropCounter>::create();
        drop(future);

        unsafe {
            AsyncCompletion::<DropCounter>::complete_ok(context, DropCounter(Arc::clone(&drops)));
        };

        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
