# Coverage

## Unreleased

The `memchr` and `regex` modules have no unit tests in this crate.
Temporary programs compared them with simple searches and with the `regex` crate 1.13.
These programs are not in the repository.

| Module | Checks |
| --- | --- |
| `memchr` | 200,000 random haystacks, needles and alignments on NEON, SSE2, AVX2 and the `usize` search on wasm32. Each byte search, iterator and substring search agreed with a simple search. |
| `regex` | The last run compared 165,279 patterns on 2,612,208 haystacks on aarch64, x86_64 with SSE2 and AVX2, and wasm32. |
| `regex` | `is_match`, `find`, `find_at`, `find_iter`, the capture positions, `split` and `replace_all` agreed, also with builder options, long haystacks, small DFA caches and invalid UTF-8. |
| `regex` | All 101 regular expressions in the query predicates of a tree-sitter fork compiled and agreed. |
| `regex` | Sixteen threads used one `Regex` and its clones. A replacement closure used the same `Regex` again. |

The only differences were the differences that the README lists.
Clippy passed on macOS, Windows and wasm32 with all features. The crate builds with Rust 1.82.

Search speed on an Apple M4 Max, with 1 GiB of log text in memory:

| Search | Speed |
| --- | --- |
| `memchr` for a byte that is not in the text | 82 GB/s |
| `memmem` for `"connection refused"` | 24 GB/s |
| `Regex::new("connection refused")` | 25 GB/s |
| `Regex::new("(?i)error")` | 15 GB/s |
| `Regex::new(r"\w+@\w+\.com")` | 35 GB/s |
| `Regex::new("(a\|aa)*b")` on 100 MiB of `a` | 83 GB/s |

## Version 0.4.4

The `collections` and `future` modules have no unit tests in this crate.
A UI framework and its layout engine use these modules.
The tests of the UI framework, its scheduler and its sum tree passed with this version.
Clippy passed on macOS and Windows with all features.

## Version 0.4.2

`doom-fish-utils` provides common FFI utilities. It does not bind an SDK
surface, so an SDK symbol count does not apply.

| Module | Existing checks |
| --- | --- |
| `callback_context` | Reference counts, callback lifetime, deactivation, concurrent access, and panic containment. |
| `completion` | Sync and async results, duplicate callbacks, timeouts, late results, and poisoned state. |
| `ffi_callbacks` | Callback type compatibility through the completion checks. |
| `ffi_string` | Buffer results, string ownership, null pointers, and invalid UTF-8. |
| `four_char_code` | Byte conversion, integer conversion, text format, and invalid input. |
| `panic_safe` | Callback failures, cleanup order, result values, and destructor failures. |
| `spsc` | Item order, overwrite, full capacity, concurrent transfer, close, wake behavior, and the optional stream wrapper. |
| `stream` | Concurrent consumers, sender lifetime, wake behavior, back pressure, and poisoned state. |

The release checks passed 63 unit tests and seven documentation tests with
all features. Clippy passed on macOS and Windows. The AudioToolbox callback
tests and the AVAudio tap-buffer stress test also passed with this version.

## Atomic ring

The ring allocates all slots at construction. It does not allocate or use a
mutex to push, overwrite, or pop an item. Audio callbacks use this ring, so
a mutex could delay a real-time producer.

One atomic stamp controls each slot. An even stamp permits a write. The next
odd stamp permits a read. Atomic cursor operations reserve each position.
Release and acquire operations transfer each item between threads.

If the ring is full, an overwrite first reserves the oldest read position.
The slot stays unavailable to other producers until the replacement is
complete. The same slot protocol protects concurrent pop and overwrite
operations. It also supports concurrent calls through shared handles.

The ring keeps arbitrary runtime capacities, one-slot rings, and
`Send + Sync` handles when `T: Send`. An overwrite returns the displaced
item. The last owner drops all items that remain in the ring.

The existing SPSC tests cover capacities 1, 2, 4, 8, and 1024. The tests do
not exhaust all possible thread schedules. A registered async waker can
execute code that takes a lock or allocates on the producer thread.
