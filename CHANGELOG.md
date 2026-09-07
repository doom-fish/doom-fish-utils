# Changelog

## [0.4.0] - 2026-09-07

### Changed (breaking)

- `UnitCompletion::callback` is now an `unsafe extern "C" fn`. Its ABI and
  argument order are unchanged, but Rust callback slots declared as safe
  `extern "C" fn` must migrate to an unsafe function pointer. Added the
  matching `ffi_callbacks::UnitCompletionCallback` alias.
- Completion context documentation now states the exact-live, exactly-once
  contract explicitly. The consumed atomic only rejects duplicates while the
  backing allocation remains live and does not make dangling storage safe.

### Added

- `panic_safe::catch_user_panic_result<R, F>(site, f) -> Option<R>` for
  result-returning extern callbacks. It shares callback, diagnostic, and
  panic-payload destruction containment with `catch_user_panic`; callers map
  `None` to their ABI-safe fallback.
- `panic_safe::catch_user_panic_result_with_cleanup<S, R, F, C>(
  site, state, f, cleanup) -> Option<R>`, where `F: FnMut(&mut S) -> R` and
  `C: FnMut(&mut S)`. It runs the cleanup body immediately after the callback
  boundary, before best-effort destruction of the callback closure, cleanup
  closure, and state. It returns `None` if any catchable protected phase
  panics.

### Fixed

- `BoundedAsyncStream` now guards buffer fullness, consumer lifetime, sender
  count, and condition-variable waits with one state mutex. Every consumer
  drain path and consumer drop notifies blocked producers without a
  check/wait lost-wakeup window. Wakers and overwritten or cleared user items
  are released after unlocking, and poisoned state no longer appears as a
  successful push or normal stream close.
- `catch_user_panic` now contains callback, diagnostic, and panic-payload
  destruction failures without claiming it can recover from Rust's
  process-aborting double-panic case. All opaque callback, cleanup, state,
  result, and payload aggregates must follow the standard rule that their
  destruction does not produce multiple panics.
- Fixed the `futures-stream` SPSC test imports for `poll_fn` and `Pin`.

## [0.3.2] - 2026-05-20

### Added

- `ffi_string::take_owned_cstring` and `ffi_string::take_owned_cstring_c` — thin helpers that take an already-obtained Swift/Objective-C C-string pointer plus a per-crate free function, return `Option<String>`, and always release the pointer. Centralises the `take_string(ptr) -> Option<String>` pattern that ~26 doom-fish sibling crates had been duplicating locally.

## [0.3.1] - 2026-05-20

- Clippy hygiene sweep: cleared all `-D warnings` lints across the crate. No public API change.

## [0.3.0] - 2026-05-19

### Added

- Added `spsc::SpscRing<T, N>` with `SpscProducer`, `SpscConsumer`, non-blocking `push`, lossy `push_overwrite`, `pop`, `pop_async`, and a feature-gated `futures_core::Stream` wrapper for real-time callback → async-consumer handoff.
- Added concurrent stress coverage for sequence preservation, overwrite-oldest semantics, non-blocking producer behavior, async wakeups, and the feature-gated stream wrapper.

## [0.2.1] - 2026-05-19

- Bump MSRV from 1.70 to 1.76 to match fleet baseline.

All notable changes to `doom-fish-utils` are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] — 2026-05-18

### Added

- `ffi_callbacks` module with six shared callback aliases for cross-crate Swift / Obj-C bridges: `JsonCallback`, `AsyncCallback`, `SimpleCallback`, `DropCallback`, `StreamEventCallback`, and `AsyncCb`.

## [0.1.1] — 2026-05-17

### Fixed

- `stream`: `AsyncStreamSender::drop` previously used `Arc::strong_count` to
  detect the last sender, which has an inherent TOCTOU race — two concurrently
  dropped last senders could both observe a count ≠ 2 and neither would mark
  the stream `closed`, leaving the consumer blocked forever. Replaced with an
  explicit `AtomicUsize sender_count` stored in `BackPressure`; the sender that
  atomically decrements from 1 → 0 is the unambiguous last sender and sets
  `closed = true` under the state mutex before waking the consumer.
  `AsyncStreamSender::clone` now increments the counter atomically.
- `completion`: `AsyncCompletion::complete_ok` and `complete_err` doc strings
  incorrectly referred to `AsyncCompletion::new()` (which does not exist); the
  correct constructor is `AsyncCompletion::create()`.
- `completion`: `UnitCompletion::callback` (`extern "C"`) was not wrapped in
  `catch_user_panic`, so a mutex-poison panic inside `complete_ok`/`complete_err`
  could unwind across the FFI boundary — undefined behaviour. The body is now
  wrapped in `catch_user_panic("UnitCompletion::callback", …)`.
- `README.md` / `CHANGELOG.md`: corrected the `panic_safe` module entry to
  reference the actual public function `catch_user_panic` (not the non-existent
  `panic_safe<F, R>` that was referenced previously).

## [0.1.0] — 2026-05-17

Initial release.

### Added

- `completion` module — `SyncCompletion<T>`, `AsyncCompletion<T>`,
  `AsyncCompletionFuture<T>`, `error_from_cstr`. Both sync and async
  completion handlers carry an `AtomicBool` `consumed` guard against
  Swift firing the callback twice on the same context pointer.
- `ffi_string` module — `ffi_string_from_buffer`,
  `ffi_string_from_buffer_or_empty`, `ffi_string_owned`,
  `ffi_string_owned_or_empty`. The `_owned` family is now generic over
  the deallocator so any crate can pass its own
  `_free_string` `extern "C"` function (e.g. `acf_free_string`,
  `sc_free_string`).
- `four_char_code` module — `FourCharCode` newtype with `Display`,
  `from_bytes`, `as_u32` helpers.
- `panic_safe` module — `catch_user_panic<F: FnOnce()>` wrapper that catches Rust
  panics inside `extern "C"` callbacks and reports them to stderr.
  Also exports `log_callback_panic` for callers that already hold a panic payload.
- `stream` module — `BoundedAsyncStream<T>`, `AsyncStreamSender<T>`,
  `NextItem<'_, T>`. Executor-agnostic, bounded async stream lifted
  and generalised from the `screencapturekit-rs` `AsyncSCStream`
  pattern. Lossy-by-default (drops oldest on overflow) with an opt-in
  back-pressure `push_or_block` method. Implements
  `futures_core::Stream` under the `futures-stream` feature.

### Origins

All four primary modules (`completion`, `ffi_string`, `four_char_code`,
`panic_safe`) were previously housed in `apple-cf-rs::utils` and have
been hoisted into this dedicated crate so they can be shared without
pulling in the full Core* binding surface. `apple-cf-rs` v0.7.0 turns
its `utils` module into a back-compat re-export.
