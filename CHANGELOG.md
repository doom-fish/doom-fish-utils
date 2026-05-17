# Changelog

All notable changes to `doom-fish-utils` are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
- `panic_safe` module — `panic_safe<F, R>` wrapper that catches Rust
  panics inside `extern "C"` callbacks and reports them to stderr
  before returning a caller-supplied default.
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
