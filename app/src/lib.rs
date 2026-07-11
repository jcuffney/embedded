//! Chip-agnostic application logic.
//!
//! `#![no_std]` means this crate doesn't link the Rust standard library
//! (there's no OS to provide files, threads, or a heap by default) — only
//! `core`, which works on bare metal. Every module here is written against
//! portable traits and can be reused unchanged on any board.

#![no_std]

// The LIBRARY never links std — but the test binary always does (it runs on
// the host, and the test harness itself needs std). This line makes that
// explicit, letting `#[cfg(test)]` modules use Vec, String, etc. It compiles
// to nothing outside `cargo test`.
#[cfg(test)]
extern crate std;

pub mod blink;
pub mod http;
