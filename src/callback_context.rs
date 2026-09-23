use std::ffi::c_void;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::panic_safe::{catch_user_panic, catch_user_panic_result};

struct Inner<T> {
    active: AtomicBool,
    value: T,
}

pub struct CallbackContext<T: Send + Sync + 'static> {
    inner: Arc<Inner<T>>,
}

impl<T: Send + Sync + 'static> CallbackContext<T> {
    pub const RETAIN: unsafe extern "C" fn(*mut c_void) = Self::retain;
    pub const RELEASE: unsafe extern "C" fn(*mut c_void) = Self::release;

    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Inner {
                active: AtomicBool::new(true),
                value,
            }),
        }
    }

    #[must_use]
    pub fn as_ptr(&self) -> *mut c_void {
        Arc::as_ptr(&self.inner).cast_mut().cast()
    }

    #[must_use]
    pub fn retained_ptr(&self) -> *mut c_void {
        Arc::into_raw(Arc::clone(&self.inner)).cast_mut().cast()
    }

    pub fn deactivate(&self) {
        self.inner.active.store(false, Ordering::Release);
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.inner.active.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn get(&self) -> &T {
        &self.inner.value
    }

    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn with<R>(ptr: *mut c_void, site: &str, f: impl FnOnce(&T) -> R) -> Option<R> {
        if ptr.is_null() {
            return None;
        }
        let inner = unsafe { &*ptr.cast::<Inner<T>>() };
        if !inner.active.load(Ordering::Acquire) {
            return None;
        }
        catch_user_panic_result(site, || f(&inner.value))
    }

    unsafe extern "C" fn retain(ptr: *mut c_void) {
        if !ptr.is_null() {
            unsafe { Arc::increment_strong_count(ptr.cast::<Inner<T>>()) };
        }
    }

    unsafe extern "C" fn release(ptr: *mut c_void) {
        if !ptr.is_null() {
            catch_user_panic("CallbackContext::RELEASE", || unsafe {
                Arc::decrement_strong_count(ptr.cast::<Inner<T>>());
            });
        }
    }
}

impl<T: Send + Sync + 'static> Drop for CallbackContext<T> {
    fn drop(&mut self) {
        self.deactivate();
    }
}

impl<T: Send + Sync + 'static> fmt::Debug for CallbackContext<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallbackContext")
            .field("active", &self.is_active())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    use super::{CallbackContext, Inner};

    struct Counted {
        drops: Arc<AtomicUsize>,
        hits: AtomicUsize,
    }

    impl Drop for Counted {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    type CountedContext = CallbackContext<Counted>;

    fn counted() -> (CountedContext, Arc<AtomicUsize>) {
        let drops = Arc::new(AtomicUsize::new(0));
        let context = CallbackContext::new(Counted {
            drops: Arc::clone(&drops),
            hits: AtomicUsize::new(0),
        });
        (context, drops)
    }

    fn hit(ptr: *mut c_void) -> Option<usize> {
        unsafe {
            CountedContext::with(ptr, "hit", |counted| {
                counted.hits.fetch_add(1, Ordering::SeqCst) + 1
            })
        }
    }

    #[test]
    fn retain_and_release_are_balanced() {
        let (context, drops) = counted();
        let ptr = context.as_ptr();

        for _ in 0..8 {
            unsafe { (CountedContext::RETAIN)(ptr) };
        }
        for _ in 0..8 {
            unsafe { (CountedContext::RELEASE)(ptr) };
        }

        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert!(context.is_active());
        assert_eq!(hit(ptr), Some(1));

        drop(context);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn value_survives_rust_handle_drop_while_retained() {
        let (context, drops) = counted();
        let ptr = context.retained_ptr();
        assert_eq!(ptr, context.as_ptr());
        assert_eq!(hit(ptr), Some(1));
        assert_eq!(hit(ptr), Some(2));

        drop(context);

        assert_eq!(drops.load(Ordering::SeqCst), 0);
        let inner = unsafe { &*ptr.cast::<Inner<Counted>>() };
        assert!(!inner.active.load(Ordering::SeqCst));
        assert_eq!(inner.value.hits.load(Ordering::SeqCst), 2);
        assert_eq!(hit(ptr), None);

        unsafe { (CountedContext::RELEASE)(ptr) };
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn release_frees_the_value_at_zero() {
        let (context, drops) = counted();
        let first = context.retained_ptr();
        let second = context.as_ptr();
        unsafe { (CountedContext::RETAIN)(second) };

        drop(context);
        assert_eq!(drops.load(Ordering::SeqCst), 0);

        unsafe { (CountedContext::RELEASE)(first) };
        assert_eq!(drops.load(Ordering::SeqCst), 0);

        unsafe { (CountedContext::RELEASE)(second) };
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn with_skips_null_and_deactivated_contexts() {
        let (context, drops) = counted();
        let called = AtomicBool::new(false);

        let null = unsafe {
            CountedContext::with(ptr::null_mut(), "null", |_| {
                called.store(true, Ordering::SeqCst);
            })
        };
        assert_eq!(null, None);
        unsafe {
            (CountedContext::RETAIN)(ptr::null_mut());
            (CountedContext::RELEASE)(ptr::null_mut());
        }

        assert_eq!(hit(context.as_ptr()), Some(1));

        context.deactivate();
        assert!(!context.is_active());
        let inactive = unsafe {
            CountedContext::with(context.as_ptr(), "inactive", |_| {
                called.store(true, Ordering::SeqCst);
            })
        };
        assert_eq!(inactive, None);
        assert!(!called.load(Ordering::SeqCst));
        assert_eq!(context.get().hits.load(Ordering::SeqCst), 1);

        drop(context);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn panic_inside_callback_is_contained() {
        let (context, _drops) = counted();

        let result = unsafe {
            CountedContext::with(context.as_ptr(), "panicking callback", |_| -> usize {
                panic!("callback panic");
            })
        };

        assert_eq!(result, None);
        assert!(context.is_active());
        assert_eq!(hit(context.as_ptr()), Some(1));
    }

    struct PanicOnDrop;

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("context value destructor panic");
        }
    }

    #[test]
    fn release_contains_a_panicking_destructor() {
        let context = CallbackContext::new(PanicOnDrop);
        let ptr = context.retained_ptr();
        drop(context);

        unsafe { (CallbackContext::<PanicOnDrop>::RELEASE)(ptr) };
    }

    #[test]
    fn concurrent_with_calls_from_several_threads() {
        const THREADS: usize = 8;
        const CALLS: usize = 2_000;

        let (context, drops) = counted();
        let barrier = Barrier::new(THREADS);

        thread::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(|| {
                    let ptr = context.retained_ptr();
                    barrier.wait();
                    for _ in 0..CALLS {
                        assert!(hit(ptr).is_some());
                    }
                    unsafe { (CountedContext::RELEASE)(ptr) };
                });
            }
        });

        assert_eq!(context.get().hits.load(Ordering::SeqCst), THREADS * CALLS);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(context);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    type Handler = Mutex<Box<dyn FnMut(u32) + Send>>;

    struct ForeignOwner {
        context: *mut c_void,
        release: unsafe extern "C" fn(*mut c_void),
    }

    impl ForeignOwner {
        unsafe fn register(
            context: *mut c_void,
            retain: unsafe extern "C" fn(*mut c_void),
            release: unsafe extern "C" fn(*mut c_void),
        ) -> Self {
            unsafe { retain(context) };
            Self { context, release }
        }

        fn deliver(&self, value: u32) {
            unsafe { trampoline(self.context, value) };
        }
    }

    impl Drop for ForeignOwner {
        fn drop(&mut self) {
            unsafe { (self.release)(self.context) };
        }
    }

    unsafe extern "C" fn trampoline(context: *mut c_void, value: u32) {
        let _ = unsafe {
            CallbackContext::<Handler>::with(context, "trampoline", |handler| {
                let mut handler = handler
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                handler(value);
            })
        };
    }

    #[test]
    fn closure_context_follows_the_foreign_owner_lifecycle() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        let handler: Handler = Mutex::new(Box::new(move |value| {
            sink.lock().unwrap().push(value);
        }));
        let context = CallbackContext::new(handler);
        let owner = unsafe {
            ForeignOwner::register(
                context.as_ptr(),
                CallbackContext::<Handler>::RETAIN,
                CallbackContext::<Handler>::RELEASE,
            )
        };

        owner.deliver(1);
        owner.deliver(2);
        drop(context);
        owner.deliver(3);
        assert_eq!(Arc::strong_count(&received), 2);

        drop(owner);
        assert_eq!(Arc::strong_count(&received), 1);
        assert_eq!(*received.lock().unwrap(), vec![1, 2]);
    }
}
