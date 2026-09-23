//! # doom-fish-utils
//!
//! Framework-agnostic FFI utilities shared by the doom-fish family of safe
//! Rust bindings to Apple SDKs.
//!
//! ## Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`callback_context`] | Reference-counted callback contexts: `RETAIN`/`RELEASE` trampolines for the foreign owner, a deactivation flag checked before every call, and panic containment |
//! | [`completion`] | Sync and async completion handlers for FFI callbacks |
//! | [`ffi_callbacks`] | Common unsafe `extern "C"` callback type aliases shared across bridge crates |
//! | [`ffi_string`] | Owned-string helpers around heap-allocated C strings |
//! | [`four_char_code`] | `FourCharCode` wrapper (used by pixel formats, `OSType` codes, etc.) |
//! | [`panic_safe`] | Contains supported callback and panic-payload failures, with explicit cleanup before best-effort destruction |
//! | [`spsc`] | Lock-free single-producer single-consumer rings for real-time callback → async-consumer handoff |
//! | [`stream`] | Executor-agnostic bounded async streams (waker + `VecDeque` + lossy oldest-drop policy) |
//!
//! ## Design tenets
//!
//! - **Executor-agnostic.** No tokio / async-std / smol dependencies; works
//!   anywhere `std::future::Future` works.
//! - **Defence in depth.** Completion contexts are exact-live and one-shot.
//!   Their atomic consumed flags only reject duplicates while the backing
//!   allocation remains live; they do not validate dangling raw pointers.
//! - **Panic-safe.** `extern "C"` callbacks pass through [`panic_safe`]
//!   wrappers. Explicit cleanup runs before opaque destruction; multiple
//!   destructor panics within one aggregate remain outside the contract.
//!
//! ## Stability
//!
//! This crate is the foundation of every doom-fish Apple-SDK binding crate.
//! Breaking changes ship as major version bumps; minor versions add modules
//! or non-breaking helpers.

#![doc(html_root_url = "https://docs.rs/doom-fish-utils/0.4.1")]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod callback_context;
pub mod completion;
pub mod ffi_callbacks;
pub mod ffi_string;
pub mod four_char_code;
pub mod panic_safe;
pub mod spsc;
pub mod stream;

pub use four_char_code::FourCharCode;
