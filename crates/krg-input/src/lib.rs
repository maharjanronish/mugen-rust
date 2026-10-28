//! Kurogane input — the minimal touch controller.
//!
//! * [`bits`] — Ikemen GO `InputBits` port (bit-exact, input.go:358).
//! * [`touch`] — 4-button controller with chord synthesis (X+Y→Z, A+B→C,
//!   A+X→D, B+Y→W, all-four→M), 8-way pad, multi-touch tracking.

pub mod bits;
pub mod touch;

pub use touch::{TouchController, TouchPhase, ZoneInfo};
