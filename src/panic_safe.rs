//! Panic-safety helpers for the C ABI boundary.
//!
//! Rust panics that unwind across `extern "C"` into Swift are undefined
//! behaviour. Extern callbacks that invoke user code must use the
//! appropriate helper in this module to catch supported panics and report
//! a best-effort diagnostic without unwinding into their caller.
//!
//! A destructor that panics while another panic is already unwinding
//! aborts the process before `catch_unwind` can recover. Callbacks with
//! potentially panicking teardown state must use
//! [`catch_user_panic_result_with_cleanup`] so callback execution,
//! cleanup, and best-effort destruction happen in explicit phases.
//!
//! No helper can safely contain multiple destructor panics from one opaque
//! aggregate without leaking arbitrary user state. Callback closures,
//! cleanup closures, state, results, panic payloads, and body-local values
//! must uphold Rust's standard destructor invariant: their aggregate
//! destruction must not produce a second panic while already unwinding.
//!
//! This is intentionally a single shared helper rather than ad-hoc
//! `catch_unwind` calls so the diagnostic format and the
//! payload-destruction defence-in-depth stay consistent.

use std::any::Any;
use std::panic::AssertUnwindSafe;

/// Run `f` and swallow any panic it produces.
///
/// On panic, writes a best-effort diagnostic to stderr identifying the
/// callback site and the panic message (when the payload is a `&str` or
/// `String`). Diagnostics and panic-payload destruction are contained by
/// an outer unwind boundary. A panic payload whose destruction produces one
/// panic is contained; multiple panicking destructors within one payload
/// aggregate can still abort as required by Rust's double-panic semantics.
///
/// `AssertUnwindSafe` is required because trait objects are not
/// generally `UnwindSafe` and we accept the user's responsibility for
/// their own state consistency on panic.
///
/// # Panicking capture destructors
///
/// This function cannot recover if `f` panics and destruction of one of
/// its captures also panics during that unwind; Rust aborts on that double
/// panic. Keep potentially panicking teardown state out of `f` and use
/// [`catch_user_panic_result_with_cleanup`] instead.
///
/// `f`, its captures, body-local values, and its panic payload must not
/// produce multiple destructor panics during one aggregate destruction.
pub fn catch_user_panic<F: FnOnce()>(site: &str, f: F) {
    let _ = catch_user_panic_result(site, f);
}

/// Run a result-returning callback and convert any contained panic to `None`.
///
/// Returns `Some(result)` when `f` completes normally. On panic, reports a
/// best-effort diagnostic, contains panic-payload destruction, and returns
/// `None` so the caller can provide the ABI-appropriate fallback value.
///
/// Like [`catch_user_panic`], this cannot contain a capture destructor
/// that panics while `f` is already unwinding. Use
/// [`catch_user_panic_result_with_cleanup`] for potentially panicking
/// teardown state.
///
/// This helper contains one panic from panic-payload destruction, but cannot
/// contain multiple panicking fields within one opaque aggregate.
#[must_use = "return an ABI-safe fallback when the callback panics"]
pub fn catch_user_panic_result<R, F: FnOnce() -> R>(site: &str, f: F) -> Option<R> {
    let boundary_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        match std::panic::catch_unwind(AssertUnwindSafe(f)) {
            Ok(result) => Some(result),
            Err(payload) => {
                log_callback_panic(site, payload.as_ref());
                drop(payload);
                None
            }
        }
    }));

    match boundary_result {
        Ok(result) => result,
        Err(payload) => {
            log_callback_panic(site, payload.as_ref());
            drop_payload_best_effort(payload);
            None
        }
    }
}

/// Run a callback and an explicit library-owned cleanup phase.
///
/// `state`, `f`, and `cleanup` are retained in this function's frame while
/// each closure body is invoked by mutable reference. The cleanup body runs
/// immediately after the callback boundary and before any attempt to destroy
/// the callback closure or state. The callback closure, cleanup closure, and
/// state are then destroyed one at a time under best-effort boundaries.
///
/// Returns `Some(result)` only when the callback body, callback-closure
/// destruction, cleanup body, cleanup-closure destruction, and state
/// destruction all complete normally. If a later phase fails after the
/// callback produced a result, that result is also destroyed under a separate
/// boundary before this function returns `None`.
///
/// This sequencing does not make arbitrary user destruction safe. `F`, `C`,
/// `S`, `R`, their fields, and body-local values must uphold the standard
/// destructor invariant: one opaque aggregate destruction must not produce
/// multiple panics. A second destructor panic during the same drop glue aborts
/// before `catch_unwind` can recover.
#[must_use = "return an ABI-safe fallback when any protected phase panics"]
pub fn catch_user_panic_result_with_cleanup<S, R, F, C>(
    site: &str,
    mut state: S,
    mut f: F,
    mut cleanup: C,
) -> Option<R>
where
    F: FnMut(&mut S) -> R,
    C: FnMut(&mut S),
{
    let callback_result = catch_user_panic_result(site, || f(&mut state));
    let cleanup_succeeded = catch_user_panic_result(site, || cleanup(&mut state)).is_some();
    let callback_drop_succeeded = catch_user_panic_result(site, || drop(f)).is_some();
    let cleanup_drop_succeeded = catch_user_panic_result(site, || drop(cleanup)).is_some();
    let state_drop_succeeded = catch_user_panic_result(site, || drop(state)).is_some();

    if callback_drop_succeeded
        && cleanup_succeeded
        && cleanup_drop_succeeded
        && state_drop_succeeded
    {
        callback_result
    } else {
        if let Some(result) = callback_result {
            catch_user_panic(site, || drop(result));
        }
        None
    }
}

/// Best-effort logger for panics caught at the C ABI boundary.
///
/// Public to support call sites that already have a panic payload
/// (e.g. those that need to dispatch multiple callbacks individually).
/// Most callers want [`catch_user_panic`] instead.
pub fn log_callback_panic(site: &str, payload: &(dyn Any + Send)) {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let message = payload.downcast_ref::<&'static str>().map_or_else(
            || {
                payload
                    .downcast_ref::<String>()
                    .map_or("<non-string panic payload>", String::as_str)
            },
            |message| *message,
        );
        eprintln!("doom-fish-utils: panic in {site} caught at C ABI boundary: {message}");
    }));
    if let Err(payload) = result {
        drop_payload_best_effort(payload);
    }
}

fn drop_payload_best_effort(payload: Box<dyn Any + Send>) {
    if let Err(undroppable_payload) = std::panic::catch_unwind(AssertUnwindSafe(|| drop(payload))) {
        // Leaking only this secondary payload avoids another attempted drop.
        // Multiple panicking fields in the original aggregate can still abort
        // before catch_unwind returns, as required by Rust's drop semantics.
        std::mem::forget(undroppable_payload);
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, panic_any, AssertUnwindSafe};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::{catch_user_panic, catch_user_panic_result, catch_user_panic_result_with_cleanup};

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn normal_panic_payload_is_dropped() {
        let dropped = Arc::new(AtomicBool::new(false));
        let payload = DropFlag(Arc::clone(&dropped));

        catch_user_panic("normal_panic_payload_is_dropped", move || {
            panic_any(payload);
        });

        assert!(dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn result_helper_preserves_success_and_maps_panic_to_none() {
        assert_eq!(catch_user_panic_result("result success", || 42), Some(42));
        assert_eq!(
            catch_user_panic_result("result panic", || -> u32 {
                panic!("result callback panic");
            }),
            None
        );
    }

    #[test]
    fn cleanup_helper_preserves_successful_result() {
        let cleanup_ran = Arc::new(AtomicBool::new(false));
        let cleanup_ran_in_closure = Arc::clone(&cleanup_ran);

        let result = catch_user_panic_result_with_cleanup(
            "cleanup_helper_preserves_successful_result",
            40_u32,
            |state| *state + 2,
            move |_state| {
                cleanup_ran_in_closure.store(true, Ordering::SeqCst);
            },
        );

        assert_eq!(result, Some(42));
        assert!(cleanup_ran.load(Ordering::SeqCst));
    }

    #[test]
    fn captured_release_runs_during_callback_unwind() {
        let released = Arc::new(AtomicBool::new(false));
        let guard = DropFlag(Arc::clone(&released));

        catch_user_panic("captured_release_runs_during_callback_unwind", move || {
            let _guard = guard;
            panic!("callback panic");
        });

        assert!(released.load(Ordering::SeqCst));
    }

    struct PanicOnDrop;

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("panic while dropping panic payload");
        }
    }

    #[test]
    fn single_panic_payload_destructor_is_contained() {
        let result = catch_unwind(AssertUnwindSafe(|| {
            catch_user_panic("single_panic_payload_destructor_is_contained", || {
                panic_any(PanicOnDrop);
            });
        }));

        assert!(result.is_ok());
    }

    struct ReleaseGuard {
        released: Arc<AtomicBool>,
    }

    impl Drop for ReleaseGuard {
        fn drop(&mut self) {
            self.released.store(true, Ordering::SeqCst);
            panic!("panic in captured release");
        }
    }

    #[test]
    fn single_capture_drop_panic_after_normal_return_is_contained() {
        let released = Arc::new(AtomicBool::new(false));
        let guard = ReleaseGuard {
            released: Arc::clone(&released),
        };

        let result = catch_unwind(AssertUnwindSafe(|| {
            catch_user_panic(
                "single_capture_drop_panic_after_normal_return_is_contained",
                move || {
                    let _guard = guard;
                },
            );
        }));

        assert!(result.is_ok());
        assert!(released.load(Ordering::SeqCst));
    }

    struct OrderedPanicOnDrop {
        sequence: Arc<AtomicUsize>,
        drop_order: Arc<AtomicUsize>,
    }

    impl Drop for OrderedPanicOnDrop {
        fn drop(&mut self) {
            let order = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
            self.drop_order.store(order, Ordering::SeqCst);
            panic!("single callback closure drop panic");
        }
    }

    #[test]
    fn cleanup_runs_before_single_callback_closure_drop_panic() {
        let sequence = Arc::new(AtomicUsize::new(0));
        let cleanup_order = Arc::new(AtomicUsize::new(0));
        let drop_order = Arc::new(AtomicUsize::new(0));
        let guard = OrderedPanicOnDrop {
            sequence: Arc::clone(&sequence),
            drop_order: Arc::clone(&drop_order),
        };
        let cleanup_sequence = Arc::clone(&sequence);
        let cleanup_order_in_closure = Arc::clone(&cleanup_order);

        let survived = catch_unwind(AssertUnwindSafe(|| {
            let result = catch_user_panic_result_with_cleanup(
                "cleanup_runs_before_single_callback_closure_drop_panic",
                (),
                move |_state| -> u32 {
                    let _guard = &guard;
                    panic!("callback panic");
                },
                move |_state| {
                    let order = cleanup_sequence.fetch_add(1, Ordering::SeqCst) + 1;
                    cleanup_order_in_closure.store(order, Ordering::SeqCst);
                },
            );
            assert_eq!(result, None);
        }));

        assert!(survived.is_ok());
        assert_eq!(cleanup_order.load(Ordering::SeqCst), 1);
        assert_eq!(drop_order.load(Ordering::SeqCst), 2);
    }
}
