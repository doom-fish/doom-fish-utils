# doom-fish-utils

Framework-agnostic FFI utilities shared by every safe-Rust Apple-SDK
binding in the [doom-fish](https://github.com/doom-fish) family.

## What's in here

| Module | Purpose |
|--------|---------|
| [`callback_context`](src/callback_context.rs) | `CallbackContext<T>` — reference-counted context for delegate, observer and stream callbacks: the foreign owner holds its reference through the `RETAIN`/`RELEASE` trampolines, `with` skips deactivated contexts and contains panics, and dropping the Rust handle deactivates the context. |
| [`completion`](src/completion.rs) | Sync + async completion handlers for callback-based FFI APIs. Raw completion contexts are exact-live and one-shot; duplicate guards only apply while their allocation remains live. |
| [`ffi_callbacks`](src/ffi_callbacks.rs) | Shared unsafe `extern "C"` callback type aliases (`JsonCallback`, `AsyncCallback`, `UnitCompletionCallback`, `SimpleCallback`, `DropCallback`, `StreamEventCallback`, `AsyncCb`) reused across bridge crates. |
| [`ffi_string`](src/ffi_string.rs) | Helpers for retrieving owned `String`s from buffer-writing or pointer-returning C / Swift APIs, with RAII-driven dealloc. |
| [`four_char_code`](src/four_char_code.rs) | `FourCharCode` newtype (used by pixel formats, `OSType` codes, AudioToolbox, VideoToolbox, etc.). |
| [`panic_safe`](src/panic_safe.rs) | Callback/panic-payload containment plus `catch_user_panic_result_with_cleanup(...)`, which runs explicit cleanup before best-effort destruction phases. |
| [`spsc`](src/spsc.rs) | `SpscRing<T, N>` — lock-free, bounded single-producer/single-consumer ring for real-time callback threads feeding async consumers. |
| [`stream`](src/stream.rs) | `BoundedAsyncStream<T>` — executor-agnostic, bounded, lossy-by-default async stream lifted from the screencapturekit-rs `AsyncSCStream` pattern. Generic over any item type. |

## Design tenets

- **Executor-agnostic.** No tokio / async-std / smol dependencies; works
  anywhere `std::future::Future` works.
- **Defence in depth.** Completion contexts are exact-live and one-shot.
  Their `AtomicBool` flags reject duplicates only while the backing
  allocation remains live; they do not validate dangling raw pointers.
- **Panic-safe.** `extern "C"` callbacks pass through `panic_safe`
  wrappers so supported callback panics log and return rather than
  unwinding into Swift / C code. Potentially panicking teardown state
  uses an explicit cleanup body before opaque values are destroyed.
  Multiple destructor panics within one aggregate remain process-aborting
  and are outside the helpers' contract.

## Optional features

- `futures-stream` — adds `futures_core::Stream` wrappers for
  `BoundedAsyncStream<T>` and `spsc::SpscConsumer<T, N>` so either
  consumer can be used directly with `futures::StreamExt` / `tokio_stream`.

## Stability

This crate is the foundation of every doom-fish Apple-SDK binding crate.
Before 1.0, breaking changes advance the minor version; after 1.0, they
advance the major version. Patch releases remain backward compatible.

## License

Dual-licensed under MIT OR Apache-2.0.
