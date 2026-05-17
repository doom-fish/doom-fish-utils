# doom-fish-utils

Framework-agnostic FFI utilities shared by every safe-Rust Apple-SDK
binding in the [doom-fish](https://github.com/doom-fish) family.

## What's in here

| Module | Purpose |
|--------|---------|
| [`completion`](src/completion.rs) | Sync + async completion handlers for callback-based FFI APIs. Provides `AsyncCompletion<T>` and `SyncCompletion<T>` with `AtomicBool` double-fire guards. |
| [`ffi_string`](src/ffi_string.rs) | Helpers for retrieving owned `String`s from buffer-writing or pointer-returning C / Swift APIs, with RAII-driven dealloc. |
| [`four_char_code`](src/four_char_code.rs) | `FourCharCode` newtype (used by pixel formats, `OSType` codes, AudioToolbox, VideoToolbox, etc.). |
| [`panic_safe`](src/panic_safe.rs) | `panic_safe(...)` wrapper for `extern "C"` callbacks so a Rust panic doesn't unwind into Swift / C code. |
| [`stream`](src/stream.rs) | `BoundedAsyncStream<T>` — executor-agnostic, bounded, lossy-by-default async stream lifted from the screencapturekit-rs `AsyncSCStream` pattern. Generic over any item type. |

## Design tenets

- **Executor-agnostic.** No tokio / async-std / smol dependencies; works
  anywhere `std::future::Future` works.
- **Defence in depth.** The async completion path uses an `AtomicBool`
  `consumed` flag to prevent double-fire UAF in the face of misbehaving
  Swift callbacks.
- **Panic-safe.** `extern "C"` callbacks pass through `panic_safe`
  wrappers so an unexpected Rust panic logs and returns rather than
  unwinding into Swift / C code.

## Optional features

- `futures-stream` — adds a `futures_core::Stream` impl on
  `BoundedAsyncStream<T>` so the stream can be used directly with
  `futures::StreamExt` / `tokio_stream`.

## Stability

This crate is the foundation of every doom-fish Apple-SDK binding crate.
Breaking changes ship as major version bumps; minor versions add modules
or non-breaking helpers.

## License

Dual-licensed under MIT OR Apache-2.0.
