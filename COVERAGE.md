# Coverage

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
