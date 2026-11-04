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
| [`memchr`](src/memchr/mod.rs) | Byte search (`memchr`, `memchr2`, `memchr3`, `memrchr` and the iterators) and substring search (`memmem`) with SIMD. Requires the `memchr` feature. |
| [`panic_safe`](src/panic_safe.rs) | Callback/panic-payload containment plus `catch_user_panic_result_with_cleanup(...)`, which runs explicit cleanup before best-effort destruction phases. |
| [`regex`](src/regex/mod.rs) | Regular expressions with the syntax, the match semantics and the API of the `regex` crate, for `&str` and for `&[u8]`. Requires the `regex` feature. |
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

## Byte search

The `memchr` feature adds the `memchr` module. It has no dependencies.
The names and the results are the same as in the `memchr` crate.

| Item | Operations |
| --- | --- |
| `memchr`, `memchr2`, `memchr3` | The first position of one, two or three bytes |
| `memrchr`, `memrchr2`, `memrchr3` | The last position of one, two or three bytes |
| `memchr_iter`, `memchr2_iter`, `memchr3_iter` | All positions, in order. `Memchr`, `Memchr2` and `Memchr3` also iterate from the end. |
| `memrchr_iter`, `memrchr2_iter`, `memrchr3_iter` | All positions, in reverse order |
| `memmem::find`, `memmem::rfind` | The first or the last position of a substring |
| `memmem::find_iter`, `memmem::rfind_iter` | All positions of a substring that do not overlap |
| `memmem::Finder`, `memmem::FinderRev` | `new`, `find` or `rfind`, `find_iter` or `rfind_iter`, `into_owned`, `as_ref`, `needle` |
| `memmem::CaseInsensitiveFinder` | `new`, `find`, `rfind`, `find_iter`, `into_owned`, `needle` |

- On aarch64, the search uses NEON. On x86_64, it uses SSE2, and AVX2 when the processor has AVX2.
  Other targets use a search on `usize` words.
- `memmem` finds candidates with SIMD on a pair of rare needle bytes. A static table of byte frequencies selects the pair.
  The search then compares the full needle at each candidate.
- If the candidates are too frequent, `memmem` continues with the Two-Way algorithm. Thus the worst case is linear.
- An empty needle matches at each position. `find_iter` then gives each position from 0 to the length of the haystack.
- `CaseInsensitiveFinder` ignores the case of ASCII letters only. It compares all other bytes exactly.
- A `Finder` keeps the needle and the search tables. Make it one time and use it for many haystacks.

## Regular expressions

The `regex` feature adds the `regex` module and enables `memchr`. It has no other dependencies.
`regex::Regex` searches `&str`, and `regex::bytes::Regex` searches `&[u8]`.
The syntax, the match semantics and the API are those of the `regex` crate 1.x.
To change from the `regex` crate, change only the import path, for example to `doom_fish_utils::regex::bytes::Regex`.

| Item | Operations |
| --- | --- |
| `Regex` | `new`, `as_str`, `is_match`, `is_match_at`, `find`, `find_at`, `find_iter`, `captures`, `captures_at`, `captures_iter`, `captures_len`, `capture_names`, `shortest_match`, `shortest_match_at`, `replace`, `replace_all`, `replacen`, `split`, `splitn`, `Clone`, `Debug`, `Display`, `FromStr`, `TryFrom` |
| `RegexBuilder` | `new`, `case_insensitive`, `multi_line`, `dot_matches_new_line`, `swap_greed`, `ignore_whitespace`, `unicode`, `size_limit`, `dfa_size_limit`, `nest_limit`, `build` |
| `Match` | `start`, `end`, `range`, `len`, `is_empty`, and `as_str` or `as_bytes` |
| `Captures` | `get`, `get_match`, `name`, `len`, `iter`, `expand`, and an index with a group number or a group name |
| `Matches`, `CaptureMatches`, `Split`, `SplitN`, `CaptureNames`, `SubCaptureMatches` | The iterators of the `regex` crate |
| `Replacer`, `NoExpand`, `ReplacerRef` | Replacement text with `$1`, `${name}` and `$$`, a closure, or text without expansion |
| `escape` | Puts a backslash before each meta character of a text |
| `Error` | `Syntax` and `CompiledTooBig`, with `Display` and `std::error::Error` |

The syntax:

- Literals and the escapes `\n \t \r \f \v \a \0`, `\x7F`, `\x{10FFFF}`, `\u{...}`, `\U{...}` and escaped meta characters
- `.` and the classes `[...]` with ranges, negation, nested classes, Perl classes, ASCII classes (`[[:alpha:]]`) and the operators `&&`, `--` and `~~`
- `\d \w \s \D \W \S`, and `\p{...}` and `\P{...}` for the properties in the list below
- The assertions `^ $ \A \z \b \B \< \>`, `\b{start}`, `\b{end}`, `\b{start-half}` and `\b{end-half}`
- The groups `( )`, `(?: )`, `(?P<name> )` and `(?<name> )`
- The flags `i`, `m`, `s`, `x`, `U` and `u`, with a scope (`(?i:...)`) and with negation (`(?-i)`)
- Alternation, and the repetitions `* + ? {n} {n,} {n,m}` with their lazy forms

A backreference or a look-around assertion gives a syntax error.
The time of each search is linear in the length of the haystack.
`regex::Regex` rejects a pattern that can match invalid UTF-8.
`regex::bytes::Regex` with Unicode disabled, for example with `(?-u)`, can match invalid UTF-8.

The engine:

- The parser makes an intermediate representation. The compiler makes a Thompson NFA over bytes. Unicode classes become UTF-8 byte sequences.
- If the pattern is a literal, `memmem` finds it. If it is a case-insensitive ASCII literal, `CaseInsensitiveFinder` finds it.
  If it is a small set of literals, a SIMD multi-literal search finds it.
- If each match starts with one literal from a small set, that set is a prefilter. The prefilter moves the search to the next candidate.
- If there is no prefix literal, a required inner literal can be the prefilter. A reverse DFA then finds the start of the match from the literal.
- A lazy DFA finds the end of the match. A reverse lazy DFA finds the start. The DFA cache of each thread has a limit (`dfa_size_limit`, 2 MiB).
- If the DFA cache fills too frequently, or if a Unicode word boundary meets a byte that is not ASCII, a PikeVM does the search.
- The PikeVM finds the capture groups in the span of the match.
- A `Regex` is `Send` and `Sync`. Each thread gets its own cache. A clone shares the compiled program and gets new caches.

Differences from the `regex` crate:

- The Unicode classes and the case folding come from the `char` methods of the Rust standard library.
  Thus the Unicode version is the version of the Rust compiler.
- `\w` is `char::is_alphanumeric`, `_`, the connector punctuation and the join controls.
  The `regex` crate also includes the combining marks that are not alphabetic (for example U+0301).
  It does not include the numbers of category No (for example `²` and `½`).
- `\d` is `char::is_numeric`, which includes the categories Nd, Nl and No. The `regex` crate includes only Nd.
- `\s` is `char::is_whitespace`. It is the same as in the `regex` crate.
- `\b`, `\B`, `\<` and `\>` use the `\w` of this crate.
- `(?i)` uses simple case folding from `char::to_lowercase` and `char::to_uppercase`, with three additional pairs from the Unicode data.
  `ı` (U+0131) does not match `I` or `i`.
  A new Rust compiler can know case pairs that the `regex` crate does not know.
- `\p{...}` accepts only `Any`, `ASCII`, `Alphabetic`, `Lowercase`, `Uppercase`, `White_Space`, `Cc`, `N`, `L`, `Lu`, `Ll` and `Nd`.
  `Lu` is the Uppercase property and `Ll` is the Lowercase property. `L` is alphabetic and not numeric. `Nd` is the same as `N`.
  Other names, for example the names of scripts, give an error.
- `\0` matches the NUL character. The `regex` crate rejects `\0`.
- The CRLF flag `R`, the `octal`, `crlf` and `line_terminator` builder options, `RegexSet`, `CaptureLocations` and `Captures::extract` are not available.
- The estimate of the compiled size is different. Thus a pattern near `size_limit` can give a different result.

## Optional features

- `collections` adds the `collections` module without additional dependencies.
- `future` adds the `future` module without additional dependencies.
- `json` adds JSON values, a parser, type conversions, and a writer without additional dependencies.
- `memchr` adds the `memchr` module without additional dependencies.
- `regex` adds the `regex` module and enables `memchr`. It adds no additional dependencies.
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
