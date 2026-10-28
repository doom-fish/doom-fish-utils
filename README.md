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
| [`json`](src/json.rs) | JSON values, a strict parser, type conversions, and a compact writer. Requires the `json` feature. |
| [`panic_safe`](src/panic_safe.rs) | Callback/panic-payload containment plus `catch_user_panic_result_with_cleanup(...)`, which runs explicit cleanup before best-effort destruction phases. |
| [`spsc`](src/spsc.rs) | `SpscRing<T, N>` — lock-free, bounded single-producer/single-consumer ring for real-time callback threads feeding async consumers. |
| [`stream`](src/stream.rs) | `BoundedAsyncStream<T>` — executor-agnostic, bounded, lossy-by-default async stream lifted from the screencapturekit-rs `AsyncSCStream` pattern. Generic over any item type. |

## Design tenets

- **Atomic ring storage.** The crate owns the `spsc` ring and does not depend
  on `crossbeam-queue`. The ring allocates its slots once. Push, overwrite,
  and pop do not use a mutex. A registered waker can still execute code
  that takes a lock.
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

## AtomicWaker

`AtomicWaker` comes from futures-core 0.3.34 and always builds.
The adapted source and upstream MIT OR Apache-2.0 licence files are in `src/atomic_waker/`.
The source retains the upstream copyright notices.
The crate has no normal third-party dependency unless you enable `futures-stream`.

## Optional features

- `json` adds JSON values, a parser, type conversions, and a writer without additional dependencies.
- `futures-stream` — adds `futures_core::Stream` wrappers for
  `BoundedAsyncStream<T>` and `spsc::SpscConsumer<T, N>` so either
  consumer can be used directly with `futures::StreamExt` / `tokio_stream`.

The JSON parser accepts at most 128 nested arrays or objects.
It stores integer literals outside the `i64` range as `JsonValue::Float`.

## Stability

This crate is the foundation of every doom-fish Apple-SDK binding crate.
Before 1.0, breaking changes advance the minor version; after 1.0, they
advance the major version. Patch releases remain backward compatible.

## License

Dual-licensed under MIT OR Apache-2.0.
