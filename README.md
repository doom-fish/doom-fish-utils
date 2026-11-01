# doom-fish-utils

Framework-agnostic FFI utilities shared by every safe-Rust Apple-SDK
binding in the [doom-fish](https://github.com/doom-fish) family.

## What's in here

| Module | Purpose |
|--------|---------|
| [`callback_context`](src/callback_context.rs) | `CallbackContext<T>` — reference-counted context for delegate, observer and stream callbacks: the foreign owner holds its reference through the `RETAIN`/`RELEASE` trampolines, `with` skips deactivated contexts and contains panics, and dropping the Rust handle deactivates the context. |
| [`collections`](src/collections/mod.rs) | `ArrayVec<T, CAP>`, `SmallVec<T, N>`, `SlotMap<K, V>` and `SecondaryMap<K, V>`, with the `new_key_type!` macro. Requires the `collections` feature. |
| [`completion`](src/completion.rs) | Sync + async completion handlers for callback-based FFI APIs. Raw completion contexts are exact-live and one-shot; duplicate guards only apply while their allocation remains live. |
| [`ffi_callbacks`](src/ffi_callbacks.rs) | Shared unsafe `extern "C"` callback type aliases (`JsonCallback`, `AsyncCallback`, `UnitCompletionCallback`, `SimpleCallback`, `DropCallback`, `StreamEventCallback`, `AsyncCb`) reused across bridge crates. |
| [`ffi_string`](src/ffi_string.rs) | Helpers for retrieving owned `String`s from buffer-writing or pointer-returning C / Swift APIs, with RAII-driven dealloc. |
| [`future`](src/future/mod.rs) | `oneshot` and `mpsc` channels, an async `Mutex`, the `Stream`, `StreamExt` and `FutureExt` traits, `select`, `join_all` and boxed future types. Requires the `future` feature. |
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

## Collections

The `collections` feature adds four collection types and a key macro. They have no dependencies.
Each type has only the operations that its users need.

| Item | Operations |
| --- | --- |
| `ArrayVec<T, CAP>` | `new` (a `const fn`), `as_slice`, `push`, `pop`, `insert`, `clear`, `drain`, slice access, `Extend`, `FromIterator`, `IntoIterator`, `Clone`, `Debug`, `Default` |
| `SmallVec<T, N>` | `new` (a `const fn`), `inline_size`, `as_slice`, `push`, `pop`, `remove`, `truncate`, `clear`, `reserve`, `extend_from_slice`, `dedup`, `retain`, `drain`, slice access, `From<Vec<T>>`, `From<&[T]>`, `Extend`, `FromIterator`, `IntoIterator`, `Clone`, `Debug`, `Default`, comparison, `Hash` |
| `SlotMap<K, V>` | `with_capacity`, `len`, `is_empty`, `insert`, `remove`, `clear`, `get`, `get_mut`, `keys`, `values`, `values_mut`, `Index`, `IndexMut`, `Clone`, `Debug`, `Default` |
| `SecondaryMap<K, V>` | `with_capacity`, `insert`, `remove`, `get`, `get_mut`, `get_disjoint_mut`, `Clone`, `Debug`, `Default` |
| `KeyData`, `Key`, `DefaultKey`, `new_key_type!` | `KeyData::as_ffi`, `KeyData::from_ffi`, `Key::data`, and a key type for each `new_key_type!` declaration |

- `ArrayVec` keeps its items inline. `push`, `insert` and `extend` panic when the vector is full. `drain` moves the items out immediately.
- `SmallVec` keeps up to `N` items inline. When it grows past `N`, it moves its items to a `Vec`, and it stays on the heap after that.
- `SlotMap` gives a generational key for each value. A key from a removed value does not find a later value in the same slot.
- If you index a `SlotMap` with a key that is not valid, the map panics with the message "invalid SlotMap key used".
- `SecondaryMap` stores values for the keys of a `SlotMap`. It ignores an insert with a key that is older than the stored key.
- `KeyData::as_ffi` puts the slot index in the low 32 bits and the version in the high 32 bits.

## Futures

The `future` feature adds channels, a lock, and future helpers for any executor. They have no dependencies.
Each item has only the operations that its users need.

| Item | Operations |
| --- | --- |
| `oneshot::channel` | `Sender::send`, `Sender::is_canceled`, `Sender::cancellation`, and `Receiver` as a future of `Result<T, Canceled>`. `Canceled` has `Display`, `Error` and equality. |
| `mpsc::unbounded` | `UnboundedSender::unbounded_send`, `UnboundedSender::same_receiver`, `Clone`, `UnboundedReceiver::recv`, `UnboundedReceiver::try_recv`, and `UnboundedReceiver` as a `Stream` |
| `mpsc::channel` | `Sender::try_send`, `Sender::is_closed`, `Receiver::recv`, `Receiver::try_recv`, `Receiver::close` |
| `Mutex` | `lock` (a future), `try_lock`, and a `MutexGuard` that unlocks when you drop it |
| `Stream`, `StreamExt` | `poll_next` and `next` |
| `FutureExt` | `boxed`, `boxed_local`, `now_or_never` |
| `select`, `Either` | The first future that completes, and the other future |
| `join_all` | The outputs of all futures, in the order of the input |
| `BoxFuture`, `LocalBoxFuture` | Pinned and boxed futures, with and without `Send` |

- The `mpsc` channels keep their items in a `VecDeque` behind a `Mutex`. The `oneshot` channel keeps its value behind a `Mutex`. The waiting sides use the crate's `AtomicWaker`. Each send and receive has a constant cost.
- A send wakes the receiver. The last sender drop also wakes the receiver, so that the receiver can see the end of the channel.
- If you drop a receiver, the channel closes and drops all items that it holds. Each later send then fails.
- `mpsc::channel(buffer)` holds `buffer + 1` items, because the one sender has a guaranteed slot. Its `Sender` has no `Clone`.
- If you drop the `oneshot` receiver, the sender's `cancellation` future completes and `is_canceled` returns `true`.
- `Mutex` has no data. It wakes one waiter, in the order of the first `lock` poll, when it unlocks.
- `select` polls the first future before the second future. `join_all` polls each future that did not complete.

## Optional features

- `collections` adds the `collections` module without additional dependencies.
- `future` adds the `future` module without additional dependencies.
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
