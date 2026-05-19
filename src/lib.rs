//! # doom-fish-utils
//!
//! Framework-agnostic FFI utilities shared by the doom-fish family of safe
//! Rust bindings to Apple SDKs.
//!
//! ## Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`completion`] | Sync and async completion handlers for FFI callbacks |
//! | [`ffi_callbacks`] | Common `extern "C"` callback type aliases shared across bridge crates |
//! | [`ffi_string`] | Owned-string helpers around heap-allocated C strings |
//! | [`four_char_code`] | `FourCharCode` wrapper (used by pixel formats, `OSType` codes, etc.) |
//! | [`panic_safe`] | Catches panics inside `extern "C"` callbacks so they don't unwind across the FFI boundary |
//! | [`spsc`] | Lock-free single-producer single-consumer rings for real-time callback → async-consumer handoff |
//! | [`stream`] | Executor-agnostic bounded async streams (waker + `VecDeque` + lossy oldest-drop policy) |
//!
//! ## Design tenets
//!
//! - **Executor-agnostic.** No tokio / async-std / smol dependencies; works
//!   anywhere `std::future::Future` works.
//! - **Defence in depth.** The async completion path uses an `AtomicBool`
//!   `consumed` flag to prevent double-fire UAF in the face of misbehaving
//!   Swift callbacks.
//! - **Panic-safe.** `extern "C"` callbacks pass through [`panic_safe`]
//!   wrappers so an unexpected Rust panic logs and returns rather than
//!   unwinding into Swift / C code.
//!
//! ## Stability
//!
//! This crate is the foundation of every doom-fish Apple-SDK binding crate.
//! Breaking changes ship as major version bumps; minor versions add modules
//! or non-breaking helpers.

#![doc(html_root_url = "https://docs.rs/doom-fish-utils/0.3.0")]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod completion;
pub mod ffi_callbacks;
pub mod ffi_string;
pub mod four_char_code;
pub mod panic_safe;
pub mod spsc;
pub mod stream;

pub use four_char_code::FourCharCode;
