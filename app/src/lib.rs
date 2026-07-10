//! Chip-agnostic application logic.
//!
//! `#![no_std]` means this crate doesn't link the Rust standard library
//! (there's no OS to provide files, threads, or a heap by default) — only
//! `core`, which works on bare metal. Every module here is written against
//! portable traits and can be reused unchanged on any board.

#![no_std]

pub mod blink;
pub mod http;
