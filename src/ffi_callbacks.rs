//! Common `extern "C"` callback type aliases used across the doom-fish family.
//!
//! These callback signatures recur in many crates' Swift / Obj-C bridges.
//! Centralising them here eliminates duplicate definitions and makes
//! cross-crate FFI signatures interchangeable.

use core::ffi::{c_char, c_void};

/// Callback that delivers a JSON-encoded payload as a heap-owned C string.
///
/// The receiver is responsible for freeing `json` (typically by passing it back
/// into a Swift bridge `free_*` helper or via the consuming wrapper's Drop impl).
pub type JsonCallback = unsafe extern "C" fn(json: *mut c_char, user_data: *mut c_void);

/// Callback that fires when a one-shot async operation completes.
///
/// `status` is the operation's exit code (0 = ok, non-zero = error code).
/// `error` is an optional `NSError` JSON payload (may be null on success).
pub type AsyncCallback = unsafe extern "C" fn(
    status: i32,
    error: *mut c_char,
    user_data: *mut c_void,
);

/// Fire-and-forget callback with no payload other than the `user_data` pointer.
pub type SimpleCallback = unsafe extern "C" fn(user_data: *mut c_void);

/// Drop notification — fires when the Swift side releases the bridged context.
/// Use this to clean up Rust state (typically `Box::from_raw(user_data)`).
pub type DropCallback = unsafe extern "C" fn(user_data: *mut c_void);

/// Stream-style callback that fires for each event in an ongoing subscription.
///
/// `event_json` is a heap-owned JSON-encoded event payload (caller-owned).
pub type StreamEventCallback = unsafe extern "C" fn(
    event_json: *mut c_char,
    user_data: *mut c_void,
);

/// Async callback variant used by avassetwriter / weatherkit / webkit bridges
/// that deliver a (`status`, `result_json`, `error`, `user_data`) tuple.
pub type AsyncCb = unsafe extern "C" fn(
    status: i32,
    result_json: *mut c_char,
    error: *mut c_char,
    user_data: *mut c_void,
);
