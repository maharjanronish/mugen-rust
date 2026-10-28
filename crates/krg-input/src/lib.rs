//! Kurogane input — the touch controller.
//!
//! * [`bits`] — Ikemen GO `InputBits` port (bit-exact, input.go:358).
//! * [`touch`] — 6-button controller (X/Y/Z punches, A/B/C kicks) with chord
//!   synthesis (X+Y→Z, A+B→C, A+X→D, B+Y→W, all-four→M), 8-way pad,
//!   multi-touch tracking, and a user-editable, persistent layout
//!   (drag-to-move / pinch-to-resize via the `edit_*` API).

pub mod bits;
pub mod touch;

pub use touch::{
    Layout, Placement, TouchController, TouchPhase, ZoneInfo, ZoneKind, RADIUS_MAX, RADIUS_MIN,
};
