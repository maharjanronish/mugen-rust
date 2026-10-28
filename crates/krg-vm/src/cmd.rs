//! Port origin: ikemen-src/src/input.go — the complete command system:
//! `CommandStepKey`/`CommandKey` (input.go:16-69), `InputBuffer`
//! (input.go:674-1385), `InputReader` SOCD + button assist (input.go:427-671),
//! `CommandStep` (input.go:2017), `Command` (input.go:2088-2709),
//! `CommandList` (input.go:2713-3091) — plus the `.cmd` file compiler
//! (compiler.go:8607-8769).
//!
//! Hierarchy: `CommandList > Command > CommandStep > CommandStepKey`.
//!
//! The Ikemen command model (v0.99+ input refactor) works on *signed timers*:
//! each of the 17 buffer keys (6 dirs + neutral + 10 buttons) stores how many
//! ticks it has been held (positive) or released (negative). `+1`/`-1` mark
//! the transition frame — the exact edge the matcher looks for — and conflict
//! rules between opposing directions are re-derived from those timers on
//! every `State()` read, so no separate edge-event bookkeeping exists.
//!
//! Rust representation notes (same semantics, better layout):
//! * Go's 17 named `int32` pairs (`Ub`/`Up`…) become two fixed `[i32; 17]`
//!   arrays with named indices — zero heap traffic, cache-friendly scans.
//! * `CommandList::Step` replaces Go's per-frame `map[string]bool` with a
//!   scratch `Vec` of completed name indices (no hashing in the hot path).

use crate::ini::{read_ini_section, IniSection};
use std::collections::HashMap;

// ═══════════════════════════════════════════════════════════════════════════
// CommandKey (input.go:25) — enum order is load-bearing (range checks!)
// ═══════════════════════════════════════════════════════════════════════════

/// Port of `CommandKey` (input.go:25) — enum order is load-bearing (the
/// IsDirection/IsButton range checks depend on it). Variant names keep
/// MUGEN's lowercase button names on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum CommandKey {
    U = 0,
    D,
    B,
    F,
    L,
    R,
    UB,
    UF,
    DB,
    DF,
    UL,
    UR,
    DL,
    DR,
    N,
    a,
    b,
    c,
    x,
    y,
    z,
    s,
    d,
    w,
    m,
}

impl Default for CommandKey {
    /// Go zero value: CK_U (input.go:28).
    fn default() -> Self {
        CommandKey::U
    }
}

/// input.go:55-57 — `IsDirectionPress`
pub fn key_is_direction(k: CommandKey) -> bool {
    matches!(
        k,
        CommandKey::U
            | CommandKey::D
            | CommandKey::B
            | CommandKey::F
            | CommandKey::L
            | CommandKey::R
            | CommandKey::UB
            | CommandKey::UF
            | CommandKey::DB
            | CommandKey::DF
            | CommandKey::UL
            | CommandKey::UR
            | CommandKey::DL
            | CommandKey::DR
            | CommandKey::N
    )
}

/// input.go:65 — `IsButtonPress` range (CK_a..=CK_m)
pub fn key_is_button(k: CommandKey) -> bool {
    matches!(
        k,
        CommandKey::a
            | CommandKey::b
            | CommandKey::c
            | CommandKey::x
            | CommandKey::y
            | CommandKey::z
            | CommandKey::s
            | CommandKey::d
            | CommandKey::w
            | CommandKey::m
    )
}

/// input.go:419 — `CommandKeyRemap`: per-character button remapping.
#[derive(Debug, Clone, Copy)]
pub struct CommandKeyRemap {
    pub a: CommandKey,
    pub b: CommandKey,
    pub c: CommandKey,
    pub x: CommandKey,
    pub y: CommandKey,
    pub z: CommandKey,
    pub s: CommandKey,
    pub d: CommandKey,
    pub w: CommandKey,
    pub m: CommandKey,
}

impl Default for CommandKeyRemap {
    /// `NewCommandKeyRemap` (input.go:423) — identity mapping.
    fn default() -> Self {
        CommandKeyRemap {
            a: CommandKey::a,
            b: CommandKey::b,
            c: CommandKey::c,
            x: CommandKey::x,
            y: CommandKey::y,
            z: CommandKey::z,
            s: CommandKey::s,
            d: CommandKey::d,
            w: CommandKey::w,
            m: CommandKey::m,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// CommandStepKey (input.go:16)
// ═══════════════════════════════════════════════════════════════════════════

/// Port of `CommandStepKey` (input.go:16). One key of one step, plus its
/// prefix modifiers: `/` hold (optionally with charge ticks), `~` release,
/// `$` lenient direction, `*` tolerant direction, and the shared `>` stored
/// on the step itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommandStepKey {
    pub key: CommandKey,
    pub slash: bool,
    pub tilde: bool,
    pub dollar: bool,
    pub star: bool,
    pub chargetime: i32,
}

impl CommandStepKey {
    /// input.go:55
    pub fn is_direction_press(&self) -> bool {
        !self.tilde && key_is_direction(self.key)
    }
    /// input.go:59
    pub fn is_direction_release(&self) -> bool {
        self.tilde && key_is_direction(self.key)
    }
    /// input.go:63
    pub fn is_button_press(&self) -> bool {
        !self.tilde && key_is_button(self.key)
    }
    /// input.go:67
    pub fn is_button_release(&self) -> bool {
        self.tilde && key_is_button(self.key)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// InputBuffer (input.go:674)
// ═══════════════════════════════════════════════════════════════════════════

/// Named buffer slots. Go fields: Ub Db Lb Rb Bb Fb Nb ab..mb / Up Dp..mp.
pub const KB_U: usize = 0;
pub const KB_D: usize = 1;
pub const KB_L: usize = 2;
pub const KB_R: usize = 3;
pub const KB_B: usize = 4; // back  (facing-relative)
pub const KB_F: usize = 5; // forward (facing-relative)
pub const KB_N: usize = 6;
pub const KB_A: usize = 7;
pub const KB_BTN_B: usize = 8; // the B *button* (Go `bb`)
pub const KB_C: usize = 9;
pub const KB_X: usize = 10;
pub const KB_Y: usize = 11;
pub const KB_Z: usize = 12;
pub const KB_S: usize = 13;
pub const KB_BTN_D: usize = 14; // Ikemen extra button `d`
pub const KB_W: usize = 15;
pub const KB_M: usize = 16;

pub const BUFFER_KEYS: usize = 17;

/// Buffer index for a button `CommandKey` — only valid for `key_is_button`.
pub fn button_slot(k: CommandKey) -> usize {
    match k {
        CommandKey::a => KB_A,
        CommandKey::b => KB_BTN_B,
        CommandKey::c => KB_C,
        CommandKey::x => KB_X,
        CommandKey::y => KB_Y,
        CommandKey::z => KB_Z,
        CommandKey::s => KB_S,
        CommandKey::d => KB_BTN_D,
        CommandKey::w => KB_W,
        CommandKey::m => KB_M,
        _ => KB_U,
    }
}

#[inline]
fn imin(a: i32, b: i32) -> i32 {
    if a < b {
        a
    } else {
        b
    }
}
#[inline]
fn imax(a: i32, b: i32) -> i32 {
    if a > b {
        a
    } else {
        b
    }
}

/// Port of `InputBuffer` (input.go:674): current (`*b`) and previous (`*p`)
/// hold timers for all 17 keys. `>0` = held for n ticks (1 = press frame),
/// `<0` = released for n ticks (-1 = release frame), 0 = untouched at reset.
#[derive(Debug, Clone, Copy, Default)]
pub struct InputBuffer {
    pub cur: [i32; BUFFER_KEYS],
    pub prev: [i32; BUFFER_KEYS],
}

impl InputBuffer {
    /// input.go:688 `Reset`
    pub fn reset(&mut self) {
        *self = InputBuffer::default();
    }

    /// Port of `updateInputTime` (input.go:697). `B`/`F` are the
    /// facing-relative back/forward (computed by the caller from L/R).
    #[allow(clippy::too_many_arguments)]
    pub fn update_input_time(
        &mut self,
        u: bool,
        d: bool,
        l: bool,
        r: bool,
        b: bool,
        f: bool,
        a: bool,
        bb: bool,
        c: bool,
        x: bool,
        y: bool,
        z: bool,
        s: bool,
        dd: bool,
        w: bool,
        m: bool,
    ) {
        self.prev = self.cur;

        #[inline]
        fn update(held: bool, t: &mut i32) {
            if held != (*t > 0) {
                *t = if held { 1 } else { -1 };
                return;
            }
            *t += if held { 1 } else { -1 };
        }

        update(u, &mut self.cur[KB_U]);
        update(d, &mut self.cur[KB_D]);
        update(l, &mut self.cur[KB_L]);
        update(r, &mut self.cur[KB_R]);
        update(b, &mut self.cur[KB_B]);
        update(f, &mut self.cur[KB_F]);
        let nodir = !(u || d || l || r || b || f);
        update(nodir, &mut self.cur[KB_N]);
        update(a, &mut self.cur[KB_A]);
        update(bb, &mut self.cur[KB_BTN_B]);
        update(c, &mut self.cur[KB_C]);
        update(x, &mut self.cur[KB_X]);
        update(y, &mut self.cur[KB_Y]);
        update(z, &mut self.cur[KB_Z]);
        update(s, &mut self.cur[KB_S]);
        update(dd, &mut self.cur[KB_BTN_D]);
        update(w, &mut self.cur[KB_W]);
        update(m, &mut self.cur[KB_M]);
    }

    /// Port of `State` (input.go:764) — the hold/release state of any key
    /// combination, with opposing-direction conflict rules. Returns the
    /// "time" of the state: >0 held since n ticks, 0 not in that state.
    /// Release states return positive time since the release edge.
    pub fn state(&self, ck: &CommandStepKey) -> i32 {
        let dollar = ck.dollar && !ck.star;
        let star = !ck.dollar && ck.star;
        let simple = !dollar && !star;
        let c = &self.cur;
        let p = &self.prev;

        // ── Hold simple directions (input.go:771-848) ─────────────────────
        if !ck.tilde && simple && key_is_direction(ck.key) && ck.key != CommandKey::N {
            return match ck.key {
                CommandKey::U => {
                    let conflict = -imax(c[KB_B], imax(c[KB_D], c[KB_F]));
                    imin(conflict, c[KB_U])
                }
                CommandKey::D => {
                    let conflict = -imax(c[KB_B], imax(c[KB_U], c[KB_F]));
                    imin(conflict, c[KB_D])
                }
                CommandKey::B => {
                    let conflict = -imax(c[KB_D], imax(c[KB_U], c[KB_F]));
                    imin(conflict, c[KB_B])
                }
                CommandKey::F => {
                    let conflict = -imax(c[KB_D], imax(c[KB_U], c[KB_B]));
                    imin(conflict, c[KB_F])
                }
                CommandKey::L => {
                    let conflict = -imax(c[KB_D], imax(c[KB_U], c[KB_R]));
                    imin(conflict, c[KB_L])
                }
                CommandKey::R => {
                    let conflict = -imax(c[KB_D], imax(c[KB_U], c[KB_L]));
                    imin(conflict, c[KB_R])
                }
                CommandKey::UF => {
                    let conflict = -imax(c[KB_D], c[KB_B]);
                    let intended = imin(c[KB_U], c[KB_F]);
                    imin(conflict, intended)
                }
                CommandKey::UB => {
                    let conflict = -imax(c[KB_D], c[KB_F]);
                    let intended = imin(c[KB_U], c[KB_B]);
                    imin(conflict, intended)
                }
                CommandKey::DF => {
                    let conflict = -imax(c[KB_U], c[KB_B]);
                    let intended = imin(c[KB_D], c[KB_F]);
                    imin(conflict, intended)
                }
                CommandKey::DB => {
                    let conflict = -imax(c[KB_U], c[KB_F]);
                    let intended = imin(c[KB_D], c[KB_B]);
                    imin(conflict, intended)
                }
                CommandKey::UL => {
                    let conflict = -imax(c[KB_D], c[KB_R]);
                    let intended = imin(c[KB_U], c[KB_L]);
                    imin(conflict, intended)
                }
                CommandKey::UR => {
                    let conflict = -imax(c[KB_D], c[KB_L]);
                    let intended = imin(c[KB_U], c[KB_R]);
                    imin(conflict, intended)
                }
                CommandKey::DL => {
                    let conflict = -imax(c[KB_U], c[KB_R]);
                    let intended = imin(c[KB_D], c[KB_L]);
                    imin(conflict, intended)
                }
                CommandKey::DR => {
                    let conflict = -imax(c[KB_U], c[KB_L]);
                    let intended = imin(c[KB_D], c[KB_R]);
                    imin(conflict, intended)
                }
                _ => c[KB_N],
            };
        }
        if !ck.tilde && simple && ck.key == CommandKey::N {
            return c[KB_N];
        }

        // ── Hold star directions (input.go:851-899) ───────────────────────
        if !ck.tilde && star && key_is_direction(ck.key) && ck.key != CommandKey::N {
            return match ck.key {
                CommandKey::U => c[KB_U],
                CommandKey::D => c[KB_D],
                CommandKey::B => c[KB_B],
                CommandKey::F => c[KB_F],
                CommandKey::L => c[KB_L],
                CommandKey::R => c[KB_R],
                CommandKey::UB => imax(c[KB_U], c[KB_B]),
                CommandKey::UF => imax(c[KB_U], c[KB_F]),
                CommandKey::DB => imax(c[KB_D], c[KB_B]),
                CommandKey::DF => imax(c[KB_D], c[KB_F]),
                CommandKey::UL => imax(c[KB_U], c[KB_L]),
                CommandKey::UR => imax(c[KB_U], c[KB_R]),
                CommandKey::DL => imax(c[KB_D], c[KB_L]),
                CommandKey::DR => imax(c[KB_D], c[KB_R]),
                _ => 0,
            };
        }

        // ── Hold dollar directions (input.go:903-979) ─────────────────────
        // Mugen's "$": ignore conflicting directions.
        if !ck.tilde && dollar && key_is_direction(ck.key) && ck.key != CommandKey::N {
            let min_card = imin(
                c[KB_U].wrapping_abs(),
                imin(c[KB_D].wrapping_abs(), imin(c[KB_B].wrapping_abs(), c[KB_F].wrapping_abs())),
            );
            let min_lr =
                imin(c[KB_U].wrapping_abs(), imin(c[KB_D].wrapping_abs(), imin(c[KB_L].wrapping_abs(), c[KB_R].wrapping_abs())));
            return match ck.key {
                CommandKey::U if c[KB_U] > 0 => min_card,
                CommandKey::D if c[KB_D] > 0 => min_card,
                CommandKey::B if c[KB_B] > 0 => min_card,
                CommandKey::F if c[KB_F] > 0 => min_card,
                CommandKey::L if c[KB_L] > 0 => min_lr,
                CommandKey::R if c[KB_R] > 0 => min_lr,
                CommandKey::UB if c[KB_U] > 0 && c[KB_B] > 0 => min_card,
                CommandKey::UF if c[KB_U] > 0 && c[KB_F] > 0 => min_card,
                CommandKey::DB if c[KB_D] > 0 && c[KB_B] > 0 => min_card,
                CommandKey::DF if c[KB_D] > 0 && c[KB_F] > 0 => min_card,
                CommandKey::UL if c[KB_U] > 0 && c[KB_L] > 0 => min_lr,
                CommandKey::UR if c[KB_U] > 0 && c[KB_R] > 0 => min_lr,
                CommandKey::DL if c[KB_D] > 0 && c[KB_L] > 0 => min_lr,
                CommandKey::DR if c[KB_D] > 0 && c[KB_R] > 0 => min_lr,
                _ => 0,
            };
        }

        // ── Release simple directions (input.go:982-1088) ─────────────────
        // Guard: already not held, or still held in the previous frame
        // (prevents UF from triggering ~U — issue #2626).
        if ck.tilde && simple && key_is_direction(ck.key) && ck.key != CommandKey::N {
            return match ck.key {
                CommandKey::U if c[KB_U] < 0 || p[KB_U] > 0 => {
                    let conflict = -imax(c[KB_B], imax(c[KB_D], c[KB_F]));
                    -imin(conflict, c[KB_U])
                }
                CommandKey::D if c[KB_D] < 0 || p[KB_D] > 0 => {
                    let conflict = -imax(c[KB_B], imax(c[KB_U], c[KB_F]));
                    -imin(conflict, c[KB_D])
                }
                CommandKey::B if c[KB_B] < 0 || p[KB_B] > 0 => {
                    let conflict = -imax(c[KB_D], imax(c[KB_U], c[KB_F]));
                    -imin(conflict, c[KB_B])
                }
                CommandKey::F if c[KB_F] < 0 || p[KB_F] > 0 => {
                    let conflict = -imax(c[KB_D], imax(c[KB_U], c[KB_B]));
                    -imin(conflict, c[KB_F])
                }
                CommandKey::L if c[KB_L] < 0 || p[KB_L] > 0 => {
                    let conflict = -imax(c[KB_D], imax(c[KB_U], c[KB_R]));
                    -imin(conflict, c[KB_L])
                }
                CommandKey::R if c[KB_R] < 0 || p[KB_R] > 0 => {
                    let conflict = -imax(c[KB_D], imax(c[KB_U], c[KB_L]));
                    -imin(conflict, c[KB_R])
                }
                CommandKey::UF
                    if (c[KB_U] < 0 || p[KB_U] > 0) && (c[KB_F] < 0 || p[KB_F] > 0) =>
                {
                    let conflict = -imax(c[KB_D], c[KB_B]);
                    let intended = imin(c[KB_U], c[KB_F]);
                    -imin(conflict, intended)
                }
                CommandKey::UB
                    if (c[KB_U] < 0 || p[KB_U] > 0) && (c[KB_B] < 0 || p[KB_B] > 0) =>
                {
                    let conflict = -imax(c[KB_D], c[KB_F]);
                    let intended = imin(c[KB_U], c[KB_B]);
                    -imin(conflict, intended)
                }
                CommandKey::DF
                    if (c[KB_D] < 0 || p[KB_D] > 0) && (c[KB_F] < 0 || p[KB_F] > 0) =>
                {
                    let conflict = -imax(c[KB_U], c[KB_B]);
                    let intended = imin(c[KB_D], c[KB_F]);
                    -imin(conflict, intended)
                }
                CommandKey::DB
                    if (c[KB_D] < 0 || p[KB_D] > 0) && (c[KB_B] < 0 || p[KB_B] > 0) =>
                {
                    let conflict = -imax(c[KB_U], c[KB_F]);
                    let intended = imin(c[KB_D], c[KB_B]);
                    -imin(conflict, intended)
                }
                CommandKey::UL
                    if (c[KB_U] < 0 || p[KB_U] > 0) && (c[KB_L] < 0 || p[KB_L] > 0) =>
                {
                    let conflict = -imax(c[KB_D], c[KB_R]);
                    let intended = imin(c[KB_U], c[KB_L]);
                    -imin(conflict, intended)
                }
                CommandKey::UR
                    if (c[KB_U] < 0 || p[KB_U] > 0) && (c[KB_R] < 0 || p[KB_R] > 0) =>
                {
                    let conflict = -imax(c[KB_D], c[KB_L]);
                    let intended = imin(c[KB_U], c[KB_R]);
                    -imin(conflict, intended)
                }
                CommandKey::DL
                    if (c[KB_D] < 0 || p[KB_D] > 0) && (c[KB_L] < 0 || p[KB_L] > 0) =>
                {
                    let conflict = -imax(c[KB_U], c[KB_R]);
                    let intended = imin(c[KB_D], c[KB_L]);
                    -imin(conflict, intended)
                }
                CommandKey::DR
                    if (c[KB_D] < 0 || p[KB_D] > 0) && (c[KB_R] < 0 || p[KB_R] > 0) =>
                {
                    let conflict = -imax(c[KB_U], c[KB_L]);
                    let intended = imin(c[KB_D], c[KB_R]);
                    -imin(conflict, intended)
                }
                _ => 0,
            };
        }
        if ck.tilde && simple && ck.key == CommandKey::N {
            return -c[KB_N];
        }

        // ── Release star directions (input.go:1091-1164) ──────────────────
        if ck.tilde && star && key_is_direction(ck.key) && ck.key != CommandKey::N {
            return match ck.key {
                CommandKey::U if c[KB_U] < 0 || p[KB_U] > 0 => -c[KB_U],
                CommandKey::D if c[KB_D] < 0 || p[KB_D] > 0 => -c[KB_D],
                CommandKey::B if c[KB_B] < 0 || p[KB_B] > 0 => -c[KB_B],
                CommandKey::F if c[KB_F] < 0 || p[KB_F] > 0 => -c[KB_F],
                CommandKey::L if c[KB_L] < 0 || p[KB_L] > 0 => -c[KB_L],
                CommandKey::R if c[KB_R] < 0 || p[KB_R] > 0 => -c[KB_R],
                // Diagonals trigger if either component is released
                CommandKey::UB
                    if (c[KB_U] < 0 || p[KB_U] > 0) || (c[KB_B] < 0 || p[KB_B] > 0) =>
                {
                    -imax(c[KB_U], c[KB_B])
                }
                CommandKey::UF
                    if (c[KB_U] < 0 || p[KB_U] > 0) || (c[KB_F] < 0 || p[KB_F] > 0) =>
                {
                    -imax(c[KB_U], c[KB_F])
                }
                CommandKey::DB
                    if (c[KB_D] < 0 || p[KB_D] > 0) || (c[KB_B] < 0 || p[KB_B] > 0) =>
                {
                    -imax(c[KB_D], c[KB_B])
                }
                CommandKey::DF
                    if (c[KB_D] < 0 || p[KB_D] > 0) || (c[KB_F] < 0 || p[KB_F] > 0) =>
                {
                    -imax(c[KB_D], c[KB_F])
                }
                CommandKey::UL
                    if (c[KB_U] < 0 || p[KB_U] > 0) || (c[KB_L] < 0 || p[KB_L] > 0) =>
                {
                    -imax(c[KB_U], c[KB_L])
                }
                CommandKey::UR
                    if (c[KB_U] < 0 || p[KB_U] > 0) || (c[KB_R] < 0 || p[KB_R] > 0) =>
                {
                    -imax(c[KB_U], c[KB_R])
                }
                CommandKey::DL
                    if (c[KB_D] < 0 || p[KB_D] > 0) || (c[KB_L] < 0 || p[KB_L] > 0) =>
                {
                    -imax(c[KB_D], c[KB_L])
                }
                CommandKey::DR
                    if (c[KB_D] < 0 || p[KB_D] > 0) || (c[KB_R] < 0 || p[KB_R] > 0) =>
                {
                    -imax(c[KB_D], c[KB_R])
                }
                _ => 0,
            };
        }

        // ── Release dollar directions (input.go:1168-1284) ─────────────────
        if ck.tilde && dollar && key_is_direction(ck.key) && ck.key != CommandKey::N {
            return match ck.key {
                CommandKey::U if c[KB_U] < 0 || p[KB_U] > 0 => {
                    if c[KB_U] < 0 {
                        -c[KB_U]
                    } else {
                        imin(
                            c[KB_D].wrapping_abs(),
                            imin(c[KB_B].wrapping_abs(), c[KB_F].wrapping_abs()),
                        )
                    }
                }
                CommandKey::D if c[KB_D] < 0 || p[KB_D] > 0 => {
                    if c[KB_D] < 0 {
                        -c[KB_D]
                    } else {
                        imin(
                            c[KB_U].wrapping_abs(),
                            imin(c[KB_B].wrapping_abs(), c[KB_F].wrapping_abs()),
                        )
                    }
                }
                CommandKey::B if c[KB_B] < 0 || p[KB_B] > 0 => {
                    if c[KB_B] < 0 {
                        -c[KB_B]
                    } else {
                        imin(
                            c[KB_U].wrapping_abs(),
                            imin(c[KB_D].wrapping_abs(), c[KB_F].wrapping_abs()),
                        )
                    }
                }
                CommandKey::F if c[KB_F] < 0 || p[KB_F] > 0 => {
                    if c[KB_F] < 0 {
                        -c[KB_F]
                    } else {
                        imin(
                            c[KB_U].wrapping_abs(),
                            imin(c[KB_D].wrapping_abs(), c[KB_B].wrapping_abs()),
                        )
                    }
                }
                CommandKey::L if c[KB_L] < 0 || p[KB_L] > 0 => {
                    if c[KB_L] < 0 {
                        -c[KB_L]
                    } else {
                        imin(
                            c[KB_U].wrapping_abs(),
                            imin(c[KB_D].wrapping_abs(), c[KB_R].wrapping_abs()),
                        )
                    }
                }
                CommandKey::R if c[KB_R] < 0 || p[KB_R] > 0 => {
                    if c[KB_R] < 0 {
                        -c[KB_R]
                    } else {
                        imin(
                            c[KB_U].wrapping_abs(),
                            imin(c[KB_D].wrapping_abs(), c[KB_L].wrapping_abs()),
                        )
                    }
                }
                CommandKey::UB
                    if (c[KB_U] < 0 || p[KB_U] > 0) && (c[KB_B] < 0 || p[KB_B] > 0) =>
                {
                    if c[KB_U] < 0 || c[KB_B] < 0 {
                        -imin(c[KB_U], c[KB_B])
                    } else {
                        imin(c[KB_D].wrapping_abs(), c[KB_F].wrapping_abs())
                    }
                }
                CommandKey::UF
                    if (c[KB_U] < 0 || p[KB_U] > 0) && (c[KB_F] < 0 || p[KB_F] > 0) =>
                {
                    if c[KB_U] < 0 || c[KB_F] < 0 {
                        -imin(c[KB_U], c[KB_F])
                    } else {
                        imin(c[KB_D].wrapping_abs(), c[KB_B].wrapping_abs())
                    }
                }
                CommandKey::DB
                    if (c[KB_D] < 0 || p[KB_D] > 0) && (c[KB_B] < 0 || p[KB_B] > 0) =>
                {
                    if c[KB_D] < 0 || c[KB_B] < 0 {
                        -imin(c[KB_D], c[KB_B])
                    } else {
                        imin(c[KB_U].wrapping_abs(), c[KB_F].wrapping_abs())
                    }
                }
                CommandKey::DF
                    if (c[KB_D] < 0 || p[KB_D] > 0) && (c[KB_F] < 0 || p[KB_F] > 0) =>
                {
                    if c[KB_D] < 0 || c[KB_F] < 0 {
                        -imin(c[KB_D], c[KB_F])
                    } else {
                        imin(c[KB_U].wrapping_abs(), c[KB_B].wrapping_abs())
                    }
                }
                CommandKey::UL
                    if (c[KB_U] < 0 || p[KB_U] > 0) && (c[KB_L] < 0 || p[KB_L] > 0) =>
                {
                    if c[KB_U] < 0 || c[KB_L] < 0 {
                        -imin(c[KB_U], c[KB_L])
                    } else {
                        imin(c[KB_D].wrapping_abs(), c[KB_R].wrapping_abs())
                    }
                }
                CommandKey::UR
                    if (c[KB_U] < 0 || p[KB_U] > 0) && (c[KB_R] < 0 || p[KB_R] > 0) =>
                {
                    if c[KB_U] < 0 || c[KB_R] < 0 {
                        -imin(c[KB_U], c[KB_R])
                    } else {
                        imin(c[KB_D].wrapping_abs(), c[KB_L].wrapping_abs())
                    }
                }
                CommandKey::DL
                    if (c[KB_D] < 0 || p[KB_D] > 0) && (c[KB_L] < 0 || p[KB_L] > 0) =>
                {
                    if c[KB_D] < 0 || c[KB_L] < 0 {
                        -imin(c[KB_D], c[KB_L])
                    } else {
                        imin(c[KB_U].wrapping_abs(), c[KB_R].wrapping_abs())
                    }
                }
                CommandKey::DR
                    if (c[KB_D] < 0 || p[KB_D] > 0) && (c[KB_R] < 0 || p[KB_R] > 0) =>
                {
                    if c[KB_D] < 0 || c[KB_R] < 0 {
                        -imin(c[KB_D], c[KB_R])
                    } else {
                        imin(c[KB_U].wrapping_abs(), c[KB_L].wrapping_abs())
                    }
                }
                _ => 0,
            };
        }

        // ── Buttons (input.go:1286-1376) ──────────────────────────────────
        let slot = button_slot(ck.key);
        if key_is_button(ck.key) {
            if !ck.tilde {
                return c[slot];
            }
            if c[slot] < 0 || p[slot] > 0 {
                return -c[slot];
            }
            return 0;
        }

        // ── Special $N (input.go:1378-1382) ────────────────────────────────
        // Mugen's bugged "$N": min abs across the 4 B/F cardinals and all
        // buttons except `m`. L/R not included, `mb` excluded — kept exactly.
        if dollar && ck.key == CommandKey::N {
            let mut m = c[KB_U].wrapping_abs();
            m = imin(m, c[KB_D].wrapping_abs());
            m = imin(m, c[KB_B].wrapping_abs());
            m = imin(m, c[KB_F].wrapping_abs());
            for &slot in &[
                KB_A,
                KB_BTN_B,
                KB_C,
                KB_X,
                KB_Y,
                KB_Z,
                KB_S,
                KB_BTN_D,
                KB_W,
            ] {
                m = imin(m, c[slot].wrapping_abs());
            }
            return m;
        }

        0
    }

    /// Port of `StateCharge` (input.go:1388) — how long a key has been *held*
    /// (for `/` charge steps). Unlike `State`, this only reports hold time.
    pub fn state_charge(&self, ck: &CommandStepKey) -> i32 {
        let dollar = ck.dollar && !ck.star;
        let star = !ck.dollar && ck.star;
        let simple = !dollar && !star;
        let c = &self.cur;
        let p = &self.prev;

        // Ignore a direction that was just pressed (input.go:1392)
        #[inline]
        fn ignore_recent(buf: i32) -> i32 {
            if buf == 1 {
                i32::MIN
            } else {
                buf
            }
        }

        // Hold dollar or star cardinals (input.go:1404)
        if !ck.tilde && (dollar || star)
            && matches!(
                ck.key,
                CommandKey::U | CommandKey::D | CommandKey::B | CommandKey::F | CommandKey::L | CommandKey::R
            )
        {
            return match ck.key {
                CommandKey::U => c[KB_U],
                CommandKey::D => c[KB_D],
                CommandKey::B => c[KB_B],
                CommandKey::F => c[KB_F],
                CommandKey::L => c[KB_L],
                _ => c[KB_R],
            };
        }

        // Release dollar or star cardinals (input.go:1429): previous state
        if ck.tilde && (dollar || star)
            && matches!(
                ck.key,
                CommandKey::U | CommandKey::D | CommandKey::B | CommandKey::F | CommandKey::L | CommandKey::R
            )
        {
            return match ck.key {
                CommandKey::U => p[KB_U],
                CommandKey::D => p[KB_D],
                CommandKey::B => p[KB_B],
                CommandKey::F => p[KB_F],
                CommandKey::L => p[KB_L],
                _ => p[KB_R],
            };
        }

        // Hold simple directions (input.go:1454-1530)
        if !ck.tilde && simple && key_is_direction(ck.key) && ck.key != CommandKey::N {
            return match ck.key {
                CommandKey::U => imax(0, imin(-imax(c[KB_D], imax(c[KB_B], c[KB_F])), c[KB_U])),
                CommandKey::D => imax(0, imin(-imax(c[KB_U], imax(c[KB_B], c[KB_F])), c[KB_D])),
                CommandKey::B => imax(0, imin(-imax(c[KB_U], imax(c[KB_D], c[KB_F])), c[KB_B])),
                CommandKey::F => imax(0, imin(-imax(c[KB_U], imax(c[KB_D], c[KB_B])), c[KB_F])),
                CommandKey::L => imax(0, imin(-imax(c[KB_U], imax(c[KB_D], c[KB_R])), c[KB_L])),
                CommandKey::R => imax(0, imin(-imax(c[KB_U], imax(c[KB_D], c[KB_L])), c[KB_R])),
                CommandKey::UF => imax(
                    0,
                    imin(-imax(c[KB_D], c[KB_B]), imin(c[KB_U], c[KB_F])),
                ),
                CommandKey::UB => imax(
                    0,
                    imin(-imax(c[KB_D], c[KB_F]), imin(c[KB_U], c[KB_B])),
                ),
                CommandKey::DF => imax(
                    0,
                    imin(-imax(c[KB_U], c[KB_B]), imin(c[KB_D], c[KB_F])),
                ),
                CommandKey::DB => imax(
                    0,
                    imin(-imax(c[KB_U], c[KB_F]), imin(c[KB_D], c[KB_B])),
                ),
                CommandKey::UL => imax(
                    0,
                    imin(-imax(c[KB_D], c[KB_R]), imin(c[KB_U], c[KB_L])),
                ),
                CommandKey::UR => imax(
                    0,
                    imin(-imax(c[KB_D], c[KB_L]), imin(c[KB_U], c[KB_R])),
                ),
                CommandKey::DL => imax(
                    0,
                    imin(-imax(c[KB_U], c[KB_R]), imin(c[KB_D], c[KB_L])),
                ),
                CommandKey::DR => imax(
                    0,
                    imin(-imax(c[KB_U], c[KB_L]), imin(c[KB_D], c[KB_R])),
                ),
                _ => c[KB_N],
            };
        }
        if !ck.tilde && simple && ck.key == CommandKey::N {
            return c[KB_N];
        }

        // Release simple directions (input.go:1533-1643)
        if ck.tilde && simple && key_is_direction(ck.key) && ck.key != CommandKey::N {
            return match ck.key {
                CommandKey::U => {
                    let (b, d, f) = (ignore_recent(c[KB_B]), ignore_recent(c[KB_D]), ignore_recent(c[KB_F]));
                    imax(0, imin(-imax(b, imax(d, f)), p[KB_U]))
                }
                CommandKey::D => {
                    let (u, b, f) = (ignore_recent(c[KB_U]), ignore_recent(c[KB_B]), ignore_recent(c[KB_F]));
                    imax(0, imin(-imax(u, imax(b, f)), p[KB_D]))
                }
                CommandKey::B => {
                    let (u, d, f) = (ignore_recent(c[KB_U]), ignore_recent(c[KB_D]), ignore_recent(c[KB_F]));
                    imax(0, imin(-imax(u, imax(d, f)), p[KB_B]))
                }
                CommandKey::F => {
                    let (u, d, b) = (ignore_recent(c[KB_U]), ignore_recent(c[KB_D]), ignore_recent(c[KB_B]));
                    imax(0, imin(-imax(u, imax(d, b)), p[KB_F]))
                }
                CommandKey::L => {
                    let (u, d, r) = (ignore_recent(c[KB_U]), ignore_recent(c[KB_D]), ignore_recent(c[KB_R]));
                    imax(0, imin(-imax(u, imax(d, r)), p[KB_L]))
                }
                CommandKey::R => {
                    let (u, d, l) = (ignore_recent(c[KB_U]), ignore_recent(c[KB_D]), ignore_recent(c[KB_L]));
                    imax(0, imin(-imax(u, imax(d, l)), p[KB_R]))
                }
                CommandKey::UF => {
                    let (d, b) = (ignore_recent(c[KB_D]), ignore_recent(c[KB_B]));
                    imax(0, imin(-imax(d, b), imin(p[KB_U], p[KB_F])))
                }
                CommandKey::UB => {
                    let (d, f) = (ignore_recent(c[KB_D]), ignore_recent(c[KB_F]));
                    imax(0, imin(-imax(d, f), imin(p[KB_U], p[KB_B])))
                }
                CommandKey::DB => {
                    let (u, f) = (ignore_recent(c[KB_U]), ignore_recent(c[KB_F]));
                    imax(0, imin(-imax(u, f), imin(p[KB_D], p[KB_B])))
                }
                CommandKey::DF => {
                    let (u, b) = (ignore_recent(c[KB_U]), ignore_recent(c[KB_B]));
                    imax(0, imin(-imax(u, b), imin(p[KB_D], p[KB_F])))
                }
                CommandKey::UL => {
                    let (d, r) = (ignore_recent(c[KB_D]), ignore_recent(c[KB_R]));
                    imax(0, imin(-imax(d, r), imin(p[KB_U], p[KB_L])))
                }
                CommandKey::UR => {
                    let (d, l) = (ignore_recent(c[KB_D]), ignore_recent(c[KB_L]));
                    imax(0, imin(-imax(d, l), imin(p[KB_U], p[KB_R])))
                }
                CommandKey::DL => {
                    let (u, r) = (ignore_recent(c[KB_U]), ignore_recent(c[KB_R]));
                    imax(0, imin(-imax(u, r), imin(p[KB_D], p[KB_L])))
                }
                CommandKey::DR => {
                    let (u, l) = (ignore_recent(c[KB_U]), ignore_recent(c[KB_L]));
                    imax(0, imin(-imax(u, l), imin(p[KB_D], p[KB_R])))
                }
                _ => 0,
            };
        }
        if ck.tilde && simple && ck.key == CommandKey::N {
            return p[KB_N];
        }

        // Hold star diagonals (input.go:1647)
        if !ck.tilde && star
            && matches!(
                ck.key,
                CommandKey::UB | CommandKey::UF | CommandKey::DB | CommandKey::DF | CommandKey::UL | CommandKey::UR | CommandKey::DL | CommandKey::DR
            )
        {
            return match ck.key {
                CommandKey::UB => imax(c[KB_U], c[KB_B]),
                CommandKey::UF => imax(c[KB_U], c[KB_F]),
                CommandKey::DB => imax(c[KB_D], c[KB_B]),
                CommandKey::DF => imax(c[KB_D], c[KB_F]),
                CommandKey::UL => imax(c[KB_U], c[KB_L]),
                CommandKey::UR => imax(c[KB_U], c[KB_R]),
                CommandKey::DL => imax(c[KB_D], c[KB_L]),
                _ => imax(c[KB_D], c[KB_R]),
            };
        }

        // Release star diagonals (input.go:1677)
        if ck.tilde && star
            && matches!(
                ck.key,
                CommandKey::UB | CommandKey::UF | CommandKey::DB | CommandKey::DF | CommandKey::UL | CommandKey::UR | CommandKey::DL | CommandKey::DR
            )
        {
            return match ck.key {
                CommandKey::UB
                    if (c[KB_U] < 0 || p[KB_U] > 0) || (c[KB_B] < 0 || p[KB_B] > 0) =>
                {
                    imax(p[KB_U], p[KB_B])
                }
                CommandKey::UF
                    if (c[KB_U] < 0 || p[KB_U] > 0) || (c[KB_F] < 0 || p[KB_F] > 0) =>
                {
                    imax(p[KB_U], p[KB_F])
                }
                CommandKey::DB
                    if (c[KB_D] < 0 || p[KB_D] > 0) || (c[KB_B] < 0 || p[KB_B] > 0) =>
                {
                    imax(p[KB_D], p[KB_B])
                }
                CommandKey::DF
                    if (c[KB_D] < 0 || p[KB_D] > 0) || (c[KB_F] < 0 || p[KB_F] > 0) =>
                {
                    imax(p[KB_D], p[KB_F])
                }
                CommandKey::UL
                    if (c[KB_U] < 0 || p[KB_U] > 0) || (c[KB_L] < 0 || p[KB_L] > 0) =>
                {
                    imax(p[KB_U], p[KB_L])
                }
                CommandKey::UR
                    if (c[KB_U] < 0 || p[KB_U] > 0) || (c[KB_R] < 0 || p[KB_R] > 0) =>
                {
                    imax(p[KB_U], p[KB_R])
                }
                CommandKey::DL
                    if (c[KB_D] < 0 || p[KB_D] > 0) || (c[KB_L] < 0 || p[KB_L] > 0) =>
                {
                    imax(p[KB_D], p[KB_L])
                }
                CommandKey::DR
                    if (c[KB_D] < 0 || p[KB_D] > 0) || (c[KB_R] < 0 || p[KB_R] > 0) =>
                {
                    imax(p[KB_D], p[KB_R])
                }
                _ => 0,
            };
        }

        // Hold dollar diagonals (input.go:1724) — allow conflicts
        if !ck.tilde && dollar
            && matches!(
                ck.key,
                CommandKey::UB | CommandKey::UF | CommandKey::DB | CommandKey::DF | CommandKey::UL | CommandKey::UR | CommandKey::DL | CommandKey::DR
            )
        {
            return match ck.key {
                CommandKey::UB => imin(c[KB_U], c[KB_B]),
                CommandKey::UF => imin(c[KB_U], c[KB_F]),
                CommandKey::DB => imin(c[KB_D], c[KB_B]),
                CommandKey::DF => imin(c[KB_D], c[KB_F]),
                CommandKey::UL => imin(c[KB_U], c[KB_L]),
                CommandKey::UR => imin(c[KB_U], c[KB_R]),
                CommandKey::DL => imin(c[KB_D], c[KB_L]),
                _ => imin(c[KB_D], c[KB_R]),
            };
        }

        // Release dollar diagonals (input.go:1754)
        if ck.tilde && dollar
            && matches!(
                ck.key,
                CommandKey::UB | CommandKey::UF | CommandKey::DB | CommandKey::DF | CommandKey::UL | CommandKey::UR | CommandKey::DL | CommandKey::DR
            )
        {
            return match ck.key {
                CommandKey::UB => imin(p[KB_U], p[KB_B]),
                CommandKey::UF => imin(p[KB_U], p[KB_F]),
                CommandKey::DB => imin(p[KB_D], p[KB_B]),
                CommandKey::DF => imin(p[KB_D], p[KB_F]),
                CommandKey::UL => imin(p[KB_U], p[KB_L]),
                CommandKey::UR => imin(p[KB_U], p[KB_R]),
                CommandKey::DL => imin(p[KB_D], p[KB_L]),
                _ => imin(p[KB_D], p[KB_R]),
            };
        }

        // Buttons (input.go:1783-1851)
        if key_is_button(ck.key) {
            let slot = button_slot(ck.key);
            if !ck.tilde {
                return c[slot];
            }
            return p[slot];
        }

        0
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// InputReader (input.go:427) — SOCD resolution + button assist
// ═══════════════════════════════════════════════════════════════════════════

/// Port of `InputReader` (input.go:427). Per-player state for resolving
/// Simultaneous Opposing Cardinal Directions (U+D, B+F) and the optional
/// button-assist frame leniency.
#[derive(Debug, Clone, Copy, Default)]
pub struct InputReader {
    pub socd_allow: [bool; 4], // U, D, B, F
    pub socd_first: [bool; 4],
    pub button_assist_buffer: [bool; 9],
}

/// SOCD config values (Input.SOCDResolution):
/// 0 = allow both (MUGEN default), 1 = last priority, 2 = absolute priority
/// (offense over defense), 3 = first priority, other = deny both (neutral).
pub const SOCD_ALLOW: i32 = 0;
pub const SOCD_LAST: i32 = 1;
pub const SOCD_ABSOLUTE: i32 = 2;
pub const SOCD_FIRST: i32 = 3;

impl InputReader {
    /// Port of `SocdResolution` (input.go:498). Resolves U/D and B/F
    /// conflicts per the configured method; L/R pairs pass through B/F.
    #[allow(clippy::too_many_arguments)]
    pub fn socd_resolution(
        &mut self,
        mut u: bool,
        mut d: bool,
        mut b: bool,
        mut f: bool,
        method: i32,
    ) -> (bool, bool, bool, bool) {
        // resolveUD (input.go:502)
        if method == SOCD_LAST || method == SOCD_FIRST {
            if u || d {
                if !u {
                    self.socd_first[0] = false;
                }
                if !d {
                    self.socd_first[1] = false;
                }
                if !self.socd_first[0] && !self.socd_first[1] {
                    if d {
                        self.socd_first[1] = true;
                    } else {
                        self.socd_first[0] = true;
                    }
                }
            } else {
                self.socd_first[0] = false;
                self.socd_first[1] = false;
            }
        }
        if d && u {
            match method {
                SOCD_ALLOW => {
                    self.socd_allow[0] = true;
                    self.socd_allow[1] = true;
                }
                SOCD_LAST => {
                    if self.socd_first[0] {
                        self.socd_allow[0] = false;
                        self.socd_allow[1] = true;
                    } else {
                        self.socd_allow[0] = true;
                        self.socd_allow[1] = false;
                    }
                }
                SOCD_ABSOLUTE => {
                    self.socd_allow[0] = true; // offense (U) over defense
                    self.socd_allow[1] = false;
                }
                SOCD_FIRST => {
                    if self.socd_first[0] {
                        self.socd_allow[0] = true;
                        self.socd_allow[1] = false;
                    } else {
                        self.socd_allow[0] = false;
                        self.socd_allow[1] = true;
                    }
                }
                _ => {
                    self.socd_allow[0] = false;
                    self.socd_allow[1] = false;
                }
            }
        } else {
            self.socd_allow[0] = true;
            self.socd_allow[1] = true;
        }

        // resolveBF (input.go:562)
        if method == SOCD_LAST || method == SOCD_FIRST {
            if b || f {
                if !b {
                    self.socd_first[2] = false;
                }
                if !f {
                    self.socd_first[3] = false;
                }
                if !self.socd_first[2] && !self.socd_first[3] {
                    if b {
                        self.socd_first[2] = true;
                    } else {
                        self.socd_first[3] = true;
                    }
                }
            } else {
                self.socd_first[2] = false;
                self.socd_first[3] = false;
            }
        }
        if b && f {
            match method {
                SOCD_ALLOW => {
                    self.socd_allow[2] = true;
                    self.socd_allow[3] = true;
                }
                SOCD_LAST => {
                    if self.socd_first[3] {
                        self.socd_allow[2] = true;
                        self.socd_allow[3] = false;
                    } else {
                        self.socd_allow[2] = false;
                        self.socd_allow[3] = true;
                    }
                }
                SOCD_ABSOLUTE => {
                    self.socd_allow[2] = false; // F (offense) wins
                    self.socd_allow[3] = true;
                }
                SOCD_FIRST => {
                    if self.socd_first[3] {
                        self.socd_allow[2] = false;
                        self.socd_allow[3] = true;
                    } else {
                        self.socd_allow[2] = true;
                        self.socd_allow[3] = false;
                    }
                }
                _ => {
                    self.socd_allow[2] = false;
                    self.socd_allow[3] = false;
                }
            }
        } else {
            self.socd_allow[2] = true;
            self.socd_allow[3] = true;
        }

        u = u && self.socd_allow[0];
        d = d && self.socd_allow[1];
        b = b && self.socd_allow[2];
        f = f && self.socd_allow[3];
        (u, d, b, f)
    }

    /// Port of `ButtonAssistCheck` (input.go:635) — one extra frame of
    /// leniency when any button was held in the previous frame.
    pub fn button_assist_check(
        &mut self,
        current: [bool; 9],
        paused: bool,
        in_match: bool,
    ) -> [bool; 9] {
        let mut result = [false; 9];

        // Disable assist during pauses and screenpack inputs (input.go:639)
        if paused || !in_match {
            self.button_assist_buffer = [false; 9];
            return current;
        }

        let mut prev_any = false;
        for &b in self.button_assist_buffer.iter() {
            if b {
                prev_any = true;
                break;
            }
        }

        for i in 0..9 {
            result[i] = self.button_assist_buffer[i] || (current[i] && prev_any);
        }

        self.button_assist_buffer = current;
        result
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// CommandStep (input.go:2017)
// ═══════════════════════════════════════════════════════════════════════════

/// Port of `CommandStep` (input.go:2017) — one comma-separated step with
/// one or more keys combined by AND (`+`) or OR (`|`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandStep {
    pub keys: Vec<CommandStepKey>,
    pub greater: bool,
    pub or_logic: bool,
}

impl CommandStep {
    /// input.go:2024 `IsSingleDirection`
    pub fn is_single_direction(&self) -> bool {
        self.keys.len() == 1
            && (self.keys[0].is_direction_press() || self.keys[0].is_direction_release())
    }

    /// input.go:2031 `IsDirToButton` — can `self` and `next` be checked in
    /// the same frame (directional step immediately followed by a button)?
    pub fn is_dir_to_button(&self, next: &CommandStep) -> bool {
        // Not if second step must be held
        for k in &next.keys {
            if k.slash {
                return false;
            }
        }
        // Not if first step includes button press or release
        for k in &self.keys {
            if k.is_button_press() || k.is_button_release() {
                return false;
            }
        }
        // Not if both steps share keys
        for k in &self.keys {
            for n in &next.keys {
                if k.key == n.key {
                    return false;
                }
            }
        }
        // Yes if second step includes a button press
        for n in &next.keys {
            if n.is_button_press() {
                return true;
            }
        }
        // Yes if release direction then not release direction
        for k in &self.keys {
            if k.is_direction_release() {
                for n in &next.keys {
                    if !n.is_direction_release() {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// input.go:2072 `EqualSteps` — for ">" expansion
    pub fn equal_steps(&self, n: &CommandStep) -> bool {
        if self.greater != n.greater || self.keys.len() != n.keys.len() {
            return false;
        }
        self.keys == n.keys
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Command (input.go:2088)
// ═══════════════════════════════════════════════════════════════════════════

/// Port of `CommandSpec` (input.go:271) — the compile-time description a
/// `.cmd` `[Command]` section (or Lua) hands to `CommandList::add_command`.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub cmd: String,
    pub time: i32,
    pub buf_time: i32,
    pub buffer_hitpause: bool,
    pub buffer_pauseend: bool,
    pub step_time: i32,
}

/// Port of `Command` (input.go:2088) — one named command and its runtime
/// matching state (step completion + timers).
#[derive(Debug, Clone)]
pub struct Command {
    pub name: String,
    pub steps: Vec<CommandStep>,
    pub maxtime: i32,
    pub curtime: i32,
    pub maxbuftime: i32,
    pub curbuftime: i32,
    pub maxsteptime: i32,
    pub cursteptime: i32,
    pub autogreater: bool,
    pub buffer_hitpause: bool,
    pub buffer_pauseend: bool,
    pub buffer_shared: bool,
    pub completeframe: bool,
    pub completed: Vec<bool>,
    pub step_timers: Vec<i32>,
    pub loop_order: Vec<usize>,
}

impl Command {
    /// `newCommand` (input.go:2104)
    pub fn new() -> Self {
        Command {
            name: String::new(),
            steps: Vec::new(),
            maxtime: 1,
            curtime: 0,
            maxbuftime: 1,
            curbuftime: 0,
            maxsteptime: 0,
            cursteptime: 0,
            autogreater: false,
            buffer_hitpause: false,
            buffer_pauseend: false,
            buffer_shared: false,
            completeframe: false,
            completed: Vec::new(),
            step_timers: Vec::new(),
            loop_order: Vec::new(),
        }
    }

    /// Port of `ReadCommandSymbols` (input.go:2109). Parses the command
    /// string (`D, DF, F, a` / `/F` / `~30` / `$U` / `>F` / `a+b` / `x|y`…)
    /// into steps. Like Go, parsing **continues after recoverable errors**
    /// and the last error is returned; the partially built command is kept.
    pub fn read_command_symbols(
        &mut self,
        cmdstr: &str,
        kr: &CommandKeyRemap,
    ) -> Option<String> {
        let mut err: Option<String> = None;
        self.steps.clear();

        if cmdstr.trim().is_empty() {
            return None; // empty steps command (input.go:2111)
        }

        let steps = cmdstr.split(',');
        for (i, stepkeys) in steps.enumerate() {
            let stepkeys = stepkeys.trim();
            let mut cs = CommandStep::default();

            // First step may be blank (blank command definitions)
            if i > 0 && stepkeys.is_empty() {
                err = Some("Empty command step found".into());
                self.steps.push(cs);
                continue;
            }

            // AND ('+') or OR ('|') logic (input.go:2133)
            let key_parts: Vec<&str> = if stepkeys.contains('+') && stepkeys.contains('|') {
                err = Some("Cannot use both '+' and '|' in the same command step".into());
                self.steps.push(cs);
                continue;
            } else if stepkeys.contains('|') {
                cs.or_logic = true;
                stepkeys.split('|').collect()
            } else {
                cs.or_logic = false;
                stepkeys.split('+').collect()
            };

            for part in key_parts {
                let mut part: &str = part.trim();
                if part.is_empty() {
                    err = Some("Unexpected '+' with no key".into());
                    continue;
                }

                // Exactly 1 key per '+' section (diagonals count as one)
                let mut key_count = 0usize;
                let pb = part.as_bytes();
                let mut j = 0usize;
                while j < pb.len() {
                    let c0 = pb[j];
                    if (c0 == b'U' || c0 == b'D') && j + 1 < pb.len() {
                        let c1 = pb[j + 1];
                        if c1 == b'B' || c1 == b'F' || c1 == b'L' || c1 == b'R' {
                            key_count += 1;
                            j += 2;
                            continue;
                        }
                    }
                    if matches!(c0, b'U' | b'D' | b'B' | b'F' | b'L' | b'R' | b'N')
                        || matches!(c0, b'a' | b'b' | b'c' | b'x' | b'y' | b'z' | b's' | b'd' | b'w' | b'm')
                    {
                        key_count += 1;
                    }
                    j += 1;
                }
                if key_count < 1 {
                    err = Some("No keys found in command step".into());
                } else if key_count > 1 {
                    err = Some("Multiple keys found without '+' separator".into());
                }

                // Prefix symbols (input.go:2183)
                let (mut slash, mut tilde, mut dollar, mut star) = (false, false, false, false);
                let mut chargetime: i32 = 0;

                // nextChar (input.go:2193): slice one char, trim — Go quirk kept
                let mut next_char = |p: &mut &str| -> i32 {
                    if !p.is_empty() {
                        // byte-slice like Go; fall back to "" on non-ASCII
                        // (a stray UTF-8 byte just ends the token — the
                        // following key parse reports the symbol error)
                        *p = p.get(1..).unwrap_or("").trim();
                    }
                    let b = p.as_bytes();
                    if b.is_empty() {
                        -1
                    } else {
                        b[0] as i32
                    }
                };
                let get_char = |p: &str| -> i32 {
                    let b = p.as_bytes();
                    if b.is_empty() {
                        -1
                    } else {
                        b[0] as i32
                    }
                };

                while !part.is_empty() {
                    match get_char(part) as u8 as char {
                        '>' => {
                            if cs.greater {
                                err = Some("Duplicate '>' symbol found".into());
                            }
                            cs.greater = true;
                            next_char(&mut part);
                        }
                        '~' => {
                            if tilde {
                                err = Some("Duplicate '~' symbol found".into());
                            }
                            tilde = true;
                            next_char(&mut part);
                            let mut n = 0i32;
                            while get_char(part) >= b'0' as i32 && get_char(part) <= b'9' as i32 {
                                n = n * 10 + (get_char(part) - b'0' as i32);
                                next_char(&mut part);
                            }
                            if n > 0 {
                                chargetime = n;
                            }
                        }
                        '/' => {
                            if slash {
                                err = Some("Duplicate '/' symbol found".into());
                            }
                            slash = true;
                            next_char(&mut part);
                            let mut n = 0i32;
                            while get_char(part) >= b'0' as i32 && get_char(part) <= b'9' as i32 {
                                n = n * 10 + (get_char(part) - b'0' as i32);
                                next_char(&mut part);
                            }
                            if n > 0 {
                                chargetime = n;
                            }
                        }
                        '$' => {
                            if dollar {
                                err = Some("Duplicate '$' symbol found".into());
                            }
                            if star {
                                err = Some("'$' and '*' symbols cannot be used together".into());
                            }
                            dollar = true;
                            next_char(&mut part);
                        }
                        '*' => {
                            if star {
                                err = Some("Duplicate '*' symbol found".into());
                            }
                            if dollar {
                                err = Some("'$' and '*' symbols cannot be used together".into());
                            }
                            star = true;
                            next_char(&mut part);
                        }
                        _ => break, // ParseKey
                    }
                }

                // ParseKey (input.go:2268)
                let c0 = get_char(part) as u8 as char;
                match c0 {
                    'B' | 'D' | 'F' | 'L' | 'R' | 'U' | 'N' => {
                        let mut k = match c0 {
                            'U' => CommandKey::U,
                            'D' => CommandKey::D,
                            'B' => CommandKey::B,
                            'F' => CommandKey::F,
                            'L' => CommandKey::L,
                            'R' => CommandKey::R,
                            _ => CommandKey::N,
                        };
                        // Handle diagonals
                        let pb2 = part.as_bytes();
                        if pb2.len() > 1 {
                            let c1 = pb2[1] as char;
                            if (c0 == 'U' || c0 == 'D')
                                && (c1 == 'B' || c1 == 'F' || c1 == 'L' || c1 == 'R')
                            {
                                k = match (c0, c1) {
                                    ('U', 'B') => CommandKey::UB,
                                    ('D', 'B') => CommandKey::DB,
                                    ('U', 'F') => CommandKey::UF,
                                    ('D', 'F') => CommandKey::DF,
                                    ('U', 'L') => CommandKey::UL,
                                    ('D', 'L') => CommandKey::DL,
                                    ('U', 'R') => CommandKey::UR,
                                    ('D', 'R') => CommandKey::DR,
                                    _ => k,
                                };
                                next_char(&mut part);
                            }
                        }
                        cs.keys.push(CommandStepKey {
                            key: k,
                            slash,
                            tilde,
                            dollar,
                            star,
                            chargetime,
                        });
                        next_char(&mut part);
                    }
                    'a' | 'b' | 'c' | 'x' | 'y' | 'z' | 's' | 'd' | 'w' | 'm' => {
                        let k = match c0 {
                            'a' => kr.a,
                            'b' => kr.b,
                            'c' => kr.c,
                            'x' => kr.x,
                            'y' => kr.y,
                            'z' => kr.z,
                            's' => kr.s,
                            'd' => kr.d,
                            'w' => kr.w,
                            _ => kr.m,
                        };
                        // Buttons never carry $ or * (input.go:2362-2369)
                        cs.keys.push(CommandStepKey {
                            key: k,
                            slash,
                            tilde,
                            dollar: false,
                            star: false,
                            chargetime,
                        });
                        next_char(&mut part);
                    }
                    _ => {
                        err = Some(format!("Invalid symbol '{}' found", c0));
                        next_char(&mut part);
                    }
                }
            }

            self.steps.push(cs);
        }

        // Expand duplicate directions (input.go:2379)
        self.auto_greater_expand();

        // Step completion trackers (input.go:2382)
        self.completed = vec![false; self.steps.len()];
        self.step_timers = vec![0; self.steps.len()];

        // Evaluation order (input.go:2389): reverse prevents one input from
        // completing two consecutive steps; IsDirToButton runs forward so the
        // direction+button pair can complete in the same frame.
        self.loop_order.clear();
        let mut i = self.steps.len() as isize - 1;
        while i >= 0 {
            let iu = i as usize;
            if i > 0 && self.steps[iu - 1].is_dir_to_button(&self.steps[iu]) {
                let mut start = iu - 1;
                let end = iu;
                while start > 0 && self.steps[start - 1].is_dir_to_button(&self.steps[start]) {
                    start -= 1;
                }
                for j in start..=end {
                    self.loop_order.push(j);
                }
                i = start as isize - 1;
            } else {
                self.loop_order.push(iu);
                i -= 1;
            }
        }

        err
    }

    /// Port of `AutoGreaterExpand` (input.go:2415): consecutive identical
    /// direction steps expand into `X, >~X, >X`.
    pub fn auto_greater_expand(&mut self) {
        if !self.autogreater || self.steps.len() < 2 {
            return;
        }

        let need = (1..self.steps.len()).any(|i| {
            self.steps[i - 1].is_single_direction()
                && self.steps[i].is_single_direction()
                && self.steps[i - 1].equal_steps(&self.steps[i])
        });
        if !need {
            return;
        }

        let old = std::mem::take(&mut self.steps);
        let mut new_cmd: Vec<CommandStep> = Vec::with_capacity(old.len() * 2);
        new_cmd.push(old[0].clone());
        for i in 1..old.len() {
            if old[i - 1].is_single_direction()
                && old[i].is_single_direction()
                && old[i - 1].equal_steps(&old[i])
            {
                let k = old[i].keys[0];
                new_cmd.push(CommandStep {
                    keys: vec![CommandStepKey {
                        key: k.key,
                        slash: false,
                        tilde: !k.tilde,
                        dollar: k.dollar,
                        star: k.star,
                        chargetime: 0,
                    }],
                    greater: true,
                    or_logic: false,
                });
                new_cmd.push(CommandStep {
                    keys: old[i].keys.clone(),
                    greater: true,
                    or_logic: false,
                });
            } else {
                new_cmd.push(old[i].clone());
            }
        }
        self.steps = new_cmd;
    }

    /// Port of `ApplyBackwardCompatibility` (input.go:2469) — legacy-char
    /// quirks: `/` propagates to the whole step, hold-only commands get
    /// buffer time forced to 1. Warnings mirror Go's console log.
    pub fn apply_backward_compatibility(&mut self, char_name: &str, log: &mut Vec<String>) {
        let mut do_print = |msg: &str| {
            log.push(format!(
                "WARNING: Player {char_name}: command '{}' compatibility: {msg}",
                self.name
            ));
        };

        // '/' expands to all elements of the step (input.go:2480)
        let mut slash_modified = false;
        for step in self.steps.iter_mut() {
            if step.or_logic {
                continue;
            }
            let has_slash = step.keys.iter().any(|k| k.slash);
            if has_slash {
                for k in step.keys.iter_mut() {
                    if !k.slash {
                        k.slash = true;
                        slash_modified = true;
                    }
                }
            }
        }
        if slash_modified {
            do_print("slash propagated to whole step");
        }

        // Hold-only commands force buffer time 1 (input.go:2508)
        if !self.steps.is_empty() {
            let mut hold_only = true;
            'outer: for step in &self.steps {
                for k in &step.keys {
                    if !k.slash {
                        hold_only = false;
                        break 'outer;
                    }
                }
            }
            if hold_only && self.maxbuftime != 1 {
                self.maxbuftime = 1;
                do_print("buffer time forced to 1");
            }
        }
    }

    /// Port of `Clear` (input.go:2528)
    pub fn clear(&mut self, bufreset: bool) {
        self.curtime = 0;
        self.cursteptime = 0;
        if bufreset {
            self.curbuftime = 0;
        }
        for c in self.completed.iter_mut() {
            *c = false;
        }
        for t in self.step_timers.iter_mut() {
            *t = 0;
        }
    }

    /// Port of `GreaterCheckFail` (input.go:1868) — did any recently-changed
    /// key invalidate a `>` (direct-input) step?
    fn greater_check_fail(&self, i: usize, ibuf: &InputBuffer) -> bool {
        // Which directional group applies (input.go:1871)
        let mut use_lr = false;
        for sk in &self.steps[i].keys {
            if matches!(
                sk.key,
                CommandKey::L | CommandKey::R | CommandKey::UL | CommandKey::UR | CommandKey::DL | CommandKey::DR
            ) {
                use_lr = true;
            }
        }

        let check_key = |k: CommandKey| -> bool {
            // Press
            if ibuf.state(&CommandStepKey { key: k, ..Default::default() }) == 1 {
                let allowed = self.steps[i]
                    .keys
                    .iter()
                    .any(|sk| sk.key == k && !sk.tilde);
                if !allowed {
                    return true;
                }
                return false;
            }
            // Release
            if ibuf.state(&CommandStepKey { key: k, tilde: true, ..Default::default() }) == 1 {
                let allowed = self.steps[i].keys.iter().any(|sk| sk.key == k && sk.tilde);
                if !allowed {
                    return true;
                }
            }
            false
        };

        for k in [CommandKey::U, CommandKey::D] {
            if check_key(k) {
                return true;
            }
        }
        if use_lr {
            for k in [
                CommandKey::L,
                CommandKey::R,
                CommandKey::UL,
                CommandKey::UR,
                CommandKey::DL,
                CommandKey::DR,
            ] {
                if check_key(k) {
                    return true;
                }
            }
        } else {
            for k in [
                CommandKey::B,
                CommandKey::F,
                CommandKey::UF,
                CommandKey::UB,
                CommandKey::DF,
                CommandKey::DB,
            ] {
                if check_key(k) {
                    return true;
                }
            }
        }

        for k in [
            CommandKey::a,
            CommandKey::b,
            CommandKey::c,
            CommandKey::x,
            CommandKey::y,
            CommandKey::z,
            CommandKey::s,
            CommandKey::d,
            CommandKey::w,
            CommandKey::m,
        ] {
            if check_key(k) {
                return true;
            }
        }

        false
    }

    /// Port of `Step` (input.go:2543) — update this command against the
    /// current input buffer for one tick. Sets `curbuftime` when complete.
    pub fn step(
        &mut self,
        ibuf: &InputBuffer,
        ai: bool,
        is_helper: bool,
        mut hpbuf: bool,
        mut pausebuf: bool,
        mut extratime: i32,
    ) {
        // Skip hitpause buffering (input.go:2545)
        if !self.buffer_hitpause {
            hpbuf = false;
            extratime = 0;
        }
        // Skip pause buffering (input.go:2551)
        if !self.buffer_pauseend {
            pausebuf = false;
            extratime = 0;
        }
        // Decrease buffer timer if not paused (input.go:2557)
        if self.curbuftime > 0 && !hpbuf && !pausebuf {
            self.curbuftime -= 1;
        }
        // Blank commands (input.go:2562)
        if self.steps.is_empty() {
            return;
        }

        // Step timers (input.go:2567)
        let mut anydone = false;
        for i in 0..self.steps.len() {
            if self.completed[i] {
                self.step_timers[i] += 1;
                if self.maxsteptime > 0 && self.step_timers[i] > self.maxsteptime {
                    self.completed[i] = false;
                    self.step_timers[i] = 0;
                    continue; // don't flag anydone
                }
                anydone = true;
            }
        }
        if anydone {
            self.curtime += 1;
        } else if self.curtime > 0 {
            self.clear(false);
        }

        // Match inputs to steps in the predetermined order (input.go:2590)
        for &i in &self.loop_order {
            // Skip if previous step is not complete
            if i > 0 && !self.completed[i - 1] {
                continue;
            }

            let mut input_matched: bool;

            // MUGEN AI can't use commands without '/' on helpers (input.go:2599)
            if ai && is_helper {
                let has_slash = self.steps[i].keys.iter().any(|k| k.slash);
                if !has_slash {
                    return;
                }
            }

            if self.steps[i].or_logic {
                input_matched = false;
                for k in &self.steps[i].keys {
                    let t = ibuf.state(k);
                    let mut key_ok = if k.slash { t > 0 } else { t == 1 };
                    if key_ok && k.chargetime > 1 && ibuf.state_charge(k) < k.chargetime {
                        key_ok = false;
                    }
                    if key_ok {
                        input_matched = true;
                        break;
                    }
                }
            } else {
                input_matched = true;
                for k in &self.steps[i].keys {
                    let t = ibuf.state(k);
                    let mut key_ok = if k.slash { t > 0 } else { t == 1 };
                    if key_ok && k.chargetime > 1 && ibuf.state_charge(k) < k.chargetime {
                        key_ok = false;
                    }
                    if !key_ok {
                        input_matched = false;
                        break;
                    }
                }
            }

            // ">" steps (input.go:2662)
            if self.steps[i].greater
                && i > 0
                && self.steps.len() >= 2
                && self.completed[i - 1]
                && !self.completed[i]
                && self.greater_check_fail(i, ibuf)
            {
                input_matched = false;
                self.completed[i - 1] = false;
                self.step_timers[i - 1] = 0;
            }

            if input_matched {
                self.completed[i] = true;
                self.step_timers[i] = 0;
                if i > 0 {
                    // Clear previous step so it can't refresh the current one
                    self.completed[i - 1] = false;
                    self.step_timers[i - 1] = 0;
                }
                if i == 0 {
                    self.curtime = 0; // start the window
                }
            }
        }

        // Complete when the last step completes (input.go:2689)
        self.completeframe = !self.completed.is_empty() && *self.completed.last().unwrap();

        if !self.completeframe {
            if ai {
                return; // AI ignores timers
            }
            if self.curtime < self.maxtime {
                return;
            }
        }

        self.clear(false);
        if self.completeframe {
            self.curbuftime = imax(self.curbuftime, self.maxbuftime + extratime);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// CommandList (input.go:2713)
// ═══════════════════════════════════════════════════════════════════════════

/// Port of `CommandList` (input.go:2713) with the input buffer and per-player
/// reader state. Default values from `NewCommandList` (input.go:2726).
#[derive(Debug, Clone)]
pub struct CommandList {
    pub buffer: InputBuffer,
    pub reader: InputReader,
    /// name → index into `commands`
    pub names: HashMap<String, usize>,
    /// commands[group][variant]
    pub commands: Vec<Vec<Command>>,
    pub default_time: i32,          // 15
    pub default_step_time: i32,     // -1 (defaults to time)
    pub default_auto_greater: bool, // true
    pub default_buffer_time: i32,   // 1
    pub default_buffer_hitpause: bool, // true
    pub default_buffer_pauseend: bool, // true
    pub default_buffer_shared: bool,   // true
    /// Scratch list of completed name-indices (replaces Go's per-frame map)
    completed_names: Vec<usize>,
}

impl Default for CommandList {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandList {
    /// `NewCommandList` (input.go:2726)
    pub fn new() -> Self {
        CommandList {
            buffer: InputBuffer::default(),
            reader: InputReader::default(),
            names: HashMap::new(),
            commands: Vec::new(),
            default_time: 15,
            default_step_time: -1,
            default_auto_greater: true,
            default_buffer_time: 1,
            default_buffer_hitpause: true,
            default_buffer_pauseend: true,
            default_buffer_shared: true,
            completed_names: Vec::new(),
        }
    }

    /// Port of `AddCommand` (input.go:2741): compile a command string with
    /// this list's defaults and add it under `name`.
    pub fn add_command(&mut self, name: &str, spec: &CommandSpec) -> Result<(), String> {
        let cmdstr = spec.cmd.trim();
        if cmdstr.is_empty() {
            return Ok(()); // nothing to add, not an error
        }

        let mut cm = Command::new();
        cm.name = name.to_string();

        if let Some(err) = cm.read_command_symbols(cmdstr, &CommandKeyRemap::default()) {
            return Err(err);
        }

        cm.maxtime = spec.time;
        cm.maxbuftime = spec.buf_time;
        cm.buffer_hitpause = spec.buffer_hitpause;
        cm.buffer_pauseend = spec.buffer_pauseend;
        cm.maxsteptime = spec.step_time;

        self.add(cm);
        Ok(())
    }

    /// Port of `Add` (input.go:3031) — commands with the same name group.
    pub fn add(&mut self, c: Command) {
        let idx = match self.names.get(&c.name) {
            Some(&i) => i,
            None => {
                self.commands.push(Vec::new());
                self.commands.len() - 1
            }
        };
        let name = c.name.clone();
        self.commands[idx].push(c);
        self.names.insert(name, idx);
    }

    /// Port of `InputUpdate` (input.go:2770) for a *character* (the
    /// screenpack `char == nil` path is not needed on-device yet).
    ///
    /// `buttons` = [U D L R a b c x y z s d w m]. The platform layer
    /// (touch controller / AI / net) produces these upstream — the
    /// `InputBits`-to-buttons conversion happens in the runtime crate.
    #[allow(clippy::too_many_arguments)]
    pub fn input_update(
        &mut self,
        buttons: [bool; 14],
        local_human: bool,
        fb_flip: bool,
        input_flag: u32,
        paused: bool,
        in_match: bool,
        button_assist_cfg: bool,
        socd_method: i32,
    ) -> bool {
        let mut u = buttons[0];
        let mut d = buttons[1];
        let mut l = buttons[2];
        let mut r = buttons[3];
        let mut a = buttons[4];
        let mut b = buttons[5];
        let mut c = buttons[6];
        let mut x = buttons[7];
        let mut y = buttons[8];
        let mut z = buttons[9];
        let mut s = buttons[10];
        let mut dd = buttons[11];
        let mut w = buttons[12];
        let mut m = buttons[13];

        // Button assist: local human input only (input.go:472, 2933)
        if local_human && button_assist_cfg {
            let res = self
                .reader
                .button_assist_check([a, b, c, x, y, z, s, dd, w], paused, in_match);
            a = res[0];
            b = res[1];
            c = res[2];
            x = res[3];
            y = res[4];
            z = res[5];
            s = res[6];
            dd = res[7];
            w = res[8];
        }

        // AssertInput flags (input.go:2856)
        if input_flag > 0 {
            u = u || input_flag & (1 << 0) != 0;
            d = d || input_flag & (1 << 1) != 0;
            l = l || input_flag & (1 << 2) != 0;
            r = r || input_flag & (1 << 3) != 0;
            a = a || input_flag & (1 << 4) != 0;
            b = b || input_flag & (1 << 5) != 0;
            c = c || input_flag & (1 << 6) != 0;
            x = x || input_flag & (1 << 7) != 0;
            y = y || input_flag & (1 << 8) != 0;
            z = z || input_flag & (1 << 9) != 0;
            s = s || input_flag & (1 << 10) != 0;
            dd = dd || input_flag & (1 << 11) != 0;
            w = w || input_flag & (1 << 12) != 0;
            m = m || input_flag & (1 << 13) != 0;
        }

        // B/F with facing flip (input.go:2910)
        let (b_dir, f_dir) = if fb_flip { (r, l) } else { (l, r) };

        // SOCD (input.go:2917)
        let (u, d, b_dir, f_dir) =
            self.reader
                .socd_resolution(u, d, b_dir, f_dir, socd_method);
        let (l, r) = if fb_flip { (f_dir, b_dir) } else { (b_dir, f_dir) };

        self.buffer.update_input_time(
            u, d, l, r, b_dir, f_dir, a, b, c, x, y, z, s, dd, w, m,
        );

        // Go returns hadStepped only for screenpacks; chars return true
        true
    }

    /// Port of `Step` (input.go:2995) — advance every command one tick and
    /// deduplicate same-name completions ("piano" input protection).
    pub fn step(&mut self, ai: bool, is_helper: bool, hpbuf: bool, pausebuf: bool, extratime: i32) {
        self.completed_names.clear();
        // InputBuffer is Copy — snapshot it so commands can be mutated while
        // they read the shared buffer (Go shares the pointer; we copy 136 bytes).
        let buf = self.buffer;

        for group in self.commands.iter_mut() {
            for cmd in group.iter_mut() {
                cmd.step(&buf, ai, is_helper, hpbuf, pausebuf, extratime);
                if cmd.completeframe {
                    cmd.clear(false);
                    if let Some(&idx) = self.names.get(&cmd.name) {
                        self.completed_names.push(idx);
                    }
                    cmd.completeframe = false;
                }
            }
        }

        // Clear duplicates of completed ones (input.go:3014)
        // (borrow juggling: take the scratch list, keep its allocation)
        let done = std::mem::take(&mut self.completed_names);
        for &idx in done.iter() {
            self.clear_name_index(idx);
        }
        self.completed_names = done;
    }

    /// Port of `ClearName` (input.go:2981) by index.
    fn clear_name_index(&mut self, idx: usize) {
        if idx >= self.commands.len() {
            return;
        }
        for cmd in self.commands[idx].iter_mut() {
            if !cmd.completeframe && cmd.buffer_shared {
                cmd.clear(false);
            }
        }
    }

    /// Port of `Assert` (input.go:2962) — force a named command on for `time`.
    pub fn assert(&mut self, name: &str, time: i32) -> bool {
        let Some(&idx) = self.names.get(name) else {
            return false;
        };
        let mut found = false;
        for cmd in self.commands[idx].iter_mut() {
            if cmd.name == name {
                cmd.curbuftime = time;
                found = true;
            }
        }
        found
    }

    /// Port of `BufReset` (input.go:3019)
    pub fn buf_reset(&mut self) {
        self.buffer.reset();
        self.reader = InputReader::default();
        for group in self.commands.iter_mut() {
            for cmd in group.iter_mut() {
                cmd.clear(true);
            }
        }
    }

    /// `At` (input.go:3051)
    pub fn at(&self, i: usize) -> &[Command] {
        self.commands.get(i).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// `Get` (input.go:3059) — all variants of a named command.
    pub fn get(&self, name: &str) -> &[Command] {
        self.names.get(name).map(|&i| self.at(i)).unwrap_or(&[])
    }

    /// Port of `GetState` (input.go:3068) — the `command = "name"` trigger.
    pub fn get_state(&self, name: &str) -> bool {
        self.get(name).iter().any(|c| c.curbuftime > 0)
    }

    /// Port of `CopyList` (input.go:3079) — clone another list's compiled
    /// commands. Go re-allocates the per-command step state (completed /
    /// stepTimers, input.go:3086-3088) so the copy starts matching from
    /// scratch; `curbuftime` carries over exactly like Go's struct copy.
    pub fn copy_list(&mut self, src: &CommandList) {
        self.names = src.names.clone();
        self.commands = src.commands.clone();
        for group in self.commands.iter_mut() {
            for cmd in group.iter_mut() {
                cmd.completed = vec![false; cmd.completed.len()];
                cmd.step_timers = vec![0; cmd.step_timers.len()];
                cmd.curtime = 0;
                cmd.completeframe = false;
            }
        }
    }

    /// The names of all compiled commands (for `command = "..."` validation
    /// during CNS compilation).
    pub fn command_names(&self) -> Vec<&str> {
        self.names.keys().map(|s| s.as_str()).collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// .cmd file compiler (compiler.go:8607-8769)
// ═══════════════════════════════════════════════════════════════════════════

/// Compile a `.cmd` file's text into `list`: [Remap], [Defaults] and every
/// `[Command]` section (compiler.go:8636-8769).
///
/// * `ikemen_ver_zero` — legacy MUGEN chars (ikemenversion 0,0) keep their
///   command-definition errors and compatibility quirks (input.go:8755).
/// * `warnings` collects compatibility messages.
///
/// Port note: Go appends common cmd files (config `Common.Cmd`, e.g.
/// `data/common.cmd`) into the same string before parsing; the caller does
/// the same by concatenating texts and calling this once.
pub fn compile_cmd_text(
    list: &mut CommandList,
    text: &str,
    char_name: &str,
    ikemen_ver_zero: bool,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut lnidx = 0usize;

    let mut remap = true;
    let mut defaults = true;
    let mut kr = CommandKeyRemap::default();
    let mut cmds: Vec<IniSection> = Vec::new();

    while lnidx < lines.len() {
        let (is, name, _sub) = read_ini_section(&lines, &mut lnidx);
        match name.as_str() {
            "remap" => {
                // SectionName gives "remap" for a bare [Remap]
                if remap {
                    remap = false;
                    let mut rm = |k: &str, slot: &mut CommandKey| {
                        if let Some(v) = is.get(k) {
                            match v.to_lowercase().as_str() {
                                "x" => *slot = CommandKey::x,
                                "y" => *slot = CommandKey::y,
                                "z" => *slot = CommandKey::z,
                                "a" => *slot = CommandKey::a,
                                "b" => *slot = CommandKey::b,
                                "c" => *slot = CommandKey::c,
                                "s" => *slot = CommandKey::s,
                                "d" => *slot = CommandKey::d,
                                "w" => *slot = CommandKey::w,
                                "m" => *slot = CommandKey::m,
                                _ => {}
                            }
                        }
                    };
                    rm("x", &mut kr.x);
                    rm("y", &mut kr.y);
                    rm("z", &mut kr.z);
                    rm("a", &mut kr.a);
                    rm("b", &mut kr.b);
                    rm("c", &mut kr.c);
                    rm("s", &mut kr.s);
                    rm("d", &mut kr.d);
                    rm("w", &mut kr.w);
                    rm("m", &mut kr.m);
                }
            }
            "defaults" => {
                if defaults {
                    defaults = false;
                    let mut t = list.default_time;
                    is.read_i32("command.time", &mut [&mut t]);
                    list.default_time = t;
                    let mut st = list.default_step_time;
                    is.read_i32("command.steptime", &mut [&mut st]);
                    list.default_step_time = st;
                    let mut ag = list.default_auto_greater;
                    is.read_bool("command.autogreater", &mut [&mut ag]);
                    list.default_auto_greater = ag;
                    if let Some(v) = is.read_i32_1("command.buffer.time") {
                        list.default_buffer_time = imax(1, v);
                    }
                    let mut bh = list.default_buffer_hitpause;
                    is.read_bool("command.buffer.hitpause", &mut [&mut bh]);
                    list.default_buffer_hitpause = bh;
                    let mut bp = list.default_buffer_pauseend;
                    is.read_bool("command.buffer.pauseend", &mut [&mut bp]);
                    list.default_buffer_pauseend = bp;
                    let mut bs = list.default_buffer_shared;
                    is.read_bool("command.buffer.shared", &mut [&mut bs]);
                    list.default_buffer_shared = bs;
                }
            }
            _ => {
                if name.len() >= 7 && &name[..7] == "command" {
                    cmds.push(is);
                }
            }
        }
    }

    // Parse commands (compiler.go:8717)
    for is in cmds {
        let mut cm = Command::new();

        // Name
        let name = match is.get_text("name") {
            Ok(Some(n)) => n,
            Ok(None) => return Err("Command section missing name".into()),
            Err(e) => {
                let raw = is.get("name").unwrap_or("");
                return Err(format!("name: {raw}: {e}"));
            }
        };
        cm.name = name;

        // Defaults (compiler.go:8729)
        cm.maxtime = list.default_time;
        cm.maxbuftime = list.default_buffer_time;
        cm.maxsteptime = list.default_step_time;
        cm.autogreater = list.default_auto_greater;
        cm.buffer_hitpause = list.default_buffer_hitpause;
        cm.buffer_pauseend = list.default_buffer_pauseend;
        cm.buffer_shared = list.default_buffer_shared;

        // Specific parameters
        if let Some(t) = is.read_i32_1("time") {
            cm.maxtime = t;
        }
        if let Some(st) = is.read_i32_1("steptime") {
            cm.maxsteptime = st;
        }
        if cm.maxsteptime <= 0 {
            cm.maxsteptime = cm.maxtime;
        }
        if let Some(ag) = is.read_bool_1("autogreater") {
            cm.autogreater = ag;
        }
        if let Some(bt) = is.read_i32_1("buffer.time") {
            cm.maxbuftime = imax(1, bt);
        }
        if let Some(bh) = is.read_bool_1("buffer.hitpause") {
            cm.buffer_hitpause = bh;
        }
        if let Some(bp) = is.read_bool_1("buffer.pauseend") {
            cm.buffer_pauseend = bp;
        }
        if let Some(bs) = is.read_bool_1("buffer.shared") {
            cm.buffer_shared = bs;
        }

        // Parse the command string (compiler.go:8753)
        let cmdstr = is.get("command").unwrap_or("");
        if let Some(err) = cm.read_command_symbols(cmdstr, &kr) {
            if ikemen_ver_zero {
                // MUGEN characters ignore command definition errors
            } else {
                return Err(format!(
                    "name = {}\ncommand = {}\n{err}",
                    is.get("name").unwrap_or(""),
                    cmdstr
                ));
            }
        }

        // Backward compatibility quirks (compiler.go:8764)
        if ikemen_ver_zero {
            cm.apply_backward_compatibility(char_name, warnings);
        }

        list.add(cm);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a fresh list from raw cmd text.
    fn list_from(text: &str) -> CommandList {
        let mut l = CommandList::new();
        let mut w = Vec::new();
        compile_cmd_text(&mut l, text, "test", false, &mut w).unwrap();
        l
    }

    /// Feed one frame of [U D L R a b c x y z s d w m] booleans.
    fn frame(l: &mut CommandList, b: [bool; 14]) {
        l.input_update(b, true, false, 0, false, true, false, SOCD_ALLOW);
        l.step(false, false, false, false, 0);
    }

    const NEUTRAL: [bool; 14] = [false; 14];

    /// Neutral frames so untouched buffer keys go negative (real matches
    /// always happen with "released for a while" conflict values, never
    /// against a freshly zeroed buffer).
    fn settle(l: &mut CommandList, n: usize) {
        for _ in 0..n {
            frame(l, NEUTRAL);
        }
    }

    #[test]
    fn buffer_timer_semantics() {
        let mut ib = InputBuffer::default();
        let up16 = |ib: &mut InputBuffer, u: bool, d: bool| {
            ib.update_input_time(u, d, false, false, false, false, false, false, false, false, false, false, false, false, false, false);
        };
        // frame 0: all neutral → untouched keys read -1
        up16(&mut ib, false, false);
        // press U: press edge = 1
        up16(&mut ib, true, false);
        assert_eq!(ib.cur[KB_U], 1);
        assert_eq!(ib.state(&CommandStepKey { key: CommandKey::U, ..Default::default() }), 1);
        // hold: 2
        up16(&mut ib, true, false);
        assert_eq!(ib.cur[KB_U], 2);
        assert_eq!(ib.state(&CommandStepKey { key: CommandKey::U, ..Default::default() }), 2);
        // release: -1, prev still positive → ~U reports 1
        up16(&mut ib, false, false);
        assert_eq!(ib.cur[KB_U], -1);
        assert_eq!(ib.prev[KB_U], 2);
        assert_eq!(ib.state(&CommandStepKey { key: CommandKey::U, tilde: true, ..Default::default() }), 1);
        // press U+D together: both are fresh edges but the conflict rule
        // cancels the U read (state < 1)
        up16(&mut ib, true, true);
        assert!(ib.state(&CommandStepKey { key: CommandKey::U, ..Default::default() }) < 1);
        // neutral buffer tracks "no direction" held
        up16(&mut ib, false, false);
        assert!(ib.state(&CommandStepKey { key: CommandKey::N, ..Default::default() }) > 0);
    }

    #[test]
    fn quarter_circle_press_motion() {
        // "D, DF, F, a" — all press steps (Ikemen's strict roll)
        let mut l = list_from(
            "[Command]\nname = \"QCF_a\"\ncommand = D, DF, F, a\ntime = 20\n",
        );
        settle(&mut l, 10);
        let mut f = NEUTRAL;
        f[1] = true; // D press
        frame(&mut l, f);
        assert!(!l.get_state("QCF_a"));
        f[3] = true; // D+R = DF (facing right, F = R)
        frame(&mut l, f);
        assert!(!l.get_state("QCF_a"));
        f[1] = false; // R only = F
        frame(&mut l, f);
        assert!(!l.get_state("QCF_a"));
        f[4] = true; // R + a — DirToButton lets F and a complete same frame
        frame(&mut l, f);
        assert!(l.get_state("QCF_a"), "press-roll QCF matched");
    }

    #[test]
    fn kfm_qcf_release_first_motion() {
        // The real kfm.cmd form: "~D, DF, F, a" — release D, then roll
        let mut l = list_from(
            "[Command]\nname = \"QCF_a\"\ncommand = ~D, DF, F, a\ntime = 15\n",
        );
        settle(&mut l, 10);
        let mut f = NEUTRAL;
        f[1] = true; // D press
        frame(&mut l, f);
        assert!(!l.get_state("QCF_a"));
        let mut f = NEUTRAL; // release D → ~D matches
        frame(&mut l, f);
        assert!(!l.get_state("QCF_a"));
        f[1] = true; // D+R press (DF)
        f[3] = true;
        frame(&mut l, f);
        let mut f = NEUTRAL;
        f[3] = true; // release D, hold R (F) — F completes
        frame(&mut l, f);
        assert!(!l.get_state("QCF_a"));
        f[4] = true; // press a
        frame(&mut l, f);
        assert!(l.get_state("QCF_a"), "kfm-style QCF matched");
    }

    #[test]
    fn buffer_time_window() {
        let mut l = list_from(
            "[Command]\nname = \"tap_x\"\ncommand = x\ntime = 1\nbuffer.time = 4\n",
        );
        settle(&mut l, 5);
        let mut f = NEUTRAL;
        f[7] = true; // x pressed
        frame(&mut l, f);
        assert!(l.get_state("tap_x"));
        let mut f = NEUTRAL;
        frame(&mut l, f);
        assert!(l.get_state("tap_x")); // 3 left
        frame(&mut l, f);
        assert!(l.get_state("tap_x"));
        frame(&mut l, f);
        assert!(l.get_state("tap_x")); // 1 left
        frame(&mut l, f);
        assert!(!l.get_state("tap_x")); // expired
    }

    #[test]
    fn charge_command_requires_hold_ticks() {
        let mut l = list_from("[Command]\nname = \"charge_b\"\ncommand = /30 B\n");
        settle(&mut l, 5);
        let mut f = NEUTRAL;
        f[2] = true; // hold back (L, facing right)
        for _ in 0..29 {
            frame(&mut l, f);
            assert!(!l.get_state("charge_b"), "no match before 30 ticks");
        }
        frame(&mut l, f); // 30th tick
        assert!(l.get_state("charge_b"), "matches at charge 30");
    }

    #[test]
    fn release_command_tilde() {
        let mut l = list_from("[Command]\nname = \"rel_x\"\ncommand = ~x\ntime = 15\n");
        settle(&mut l, 5);
        let mut f = NEUTRAL;
        f[7] = true; // press x
        frame(&mut l, f);
        assert!(!l.get_state("rel_x"));
        frame(&mut l, f); // still held
        assert!(!l.get_state("rel_x"));
        let mut f = NEUTRAL; // release
        frame(&mut l, f);
        assert!(l.get_state("rel_x"), "~x matches on release");
    }

    #[test]
    fn double_tap_forward_autogreater() {
        let mut l = list_from("[Command]\nname = \"FF\"\ncommand = F, F\ntime = 10\n");
        assert!(l.get("FF")[0].autogreater);
        // expanded: F, >~F, >F (input.go:2415)
        assert_eq!(l.get("FF")[0].steps.len(), 3);
        assert!(l.get("FF")[0].steps[1].greater);
        assert!(l.get("FF")[0].steps[1].keys[0].tilde);
        assert!(!l.get("FF")[0].steps[0].greater);

        settle(&mut l, 10);
        let mut f = NEUTRAL;
        f[3] = true; // F press (facing right)
        frame(&mut l, f);
        assert!(!l.get_state("FF"));
        let mut f = NEUTRAL; // release — >~F completes
        frame(&mut l, f);
        assert!(!l.get_state("FF"));
        let mut f = NEUTRAL;
        f[3] = true; // F press again — >F completes
        frame(&mut l, f);
        assert!(l.get_state("FF"), "F,F dash matched");
    }

    #[test]
    fn and_plus_or_logic() {
        let mut l = list_from(
            "[Command]\nname = \"ab\"\ncommand = a+b\n[Command]\nname = \"xy\"\ncommand = x|y\n",
        );
        settle(&mut l, 5);
        let mut f = NEUTRAL;
        f[4] = true; // a only → AND fails
        frame(&mut l, f);
        assert!(!l.get_state("ab"));
        let f = NEUTRAL; // release a
        frame(&mut l, f);
        let mut f = NEUTRAL;
        f[4] = true; // a+b same frame → both press edges
        f[5] = true;
        frame(&mut l, f);
        assert!(l.get_state("ab"), "a+b needs simultaneous press edges");
        let mut f = NEUTRAL;
        f[8] = true; // y (OR logic)
        frame(&mut l, f);
        assert!(l.get_state("xy"));
    }

    /// Verbatim subset of the real Ikemen GO v1.0.0 chars/kfm/kfm.cmd.
    #[test]
    fn real_kfm_cmd_subset() {
        let kfm_cmd = r#"
;-| Button Remapping |-----------------------------------------------------
[Remap]
x = x
y = y
z = z
a = a
b = b
c = c
s = s

;-| Default Values |-------------------------------------------------------
[Defaults]
command.time = 15
command.buffer.time = 1

;-| Super Motions |--------------------------------------------------------
[Command]
name = "TripleKFPalm"
command = ~D, DF, F, D, DF, F, x
time = 20

[Command]
name = "SmashKFUpper"
command = ~D, DB, B, D, DB, B, x
time = 20

;-| Special Motions |------------------------------------------------------
[Command]
name = "blocking"
command = $F,x
time = 3

[Command]
name = "QCF_a"
command = ~D, DF, F, a

[Command]
name = "QCF_xy"
command = ~D, DF, F, x+y

;-| Double Tap |-----------------------------------------------------------
[Command]
name = "FF"     ;Required (do not remove)
command = F, F
time = 10

[Command]
name = "BB"     ;Required (do not remove)
command = B, B
time = 10

;-| Dir + Button |---------------------------------------------------------
[Command]
name = "fwd_a"  ;Default
command = /F, a

;-| Hold Button |----------------------------------------------------------
[Command]
name = "holdx"
command = /x

;-| Hold Dir |-------------------------------------------------------------
[Command]
name = "holdfwd";Required (do not remove)
command = /$F
time = 1

[Command]
name = "holdback";Required (do not remove)
command = /$B
time = 1
"#;
        let mut l = list_from(kfm_cmd);
        // one group per unique name
        assert_eq!(l.commands.len(), 11);
        settle(&mut l, 10);

        // /$F: hold R (facing right) — dollar ignores U/D/B conflicts
        let mut f = NEUTRAL;
        f[3] = true;
        frame(&mut l, f);
        assert!(l.get_state("holdfwd"));
        assert!(!l.get_state("holdback"));
        // still matches with D also held (the "$" tolerance)
        f[1] = true;
        frame(&mut l, f);
        assert!(l.get_state("holdfwd"), "/$F survives conflicting dirs");

        // BB: B, B (facing right: B = L)
        let mut f = NEUTRAL;
        f[2] = true;
        frame(&mut l, f);
        let mut f = NEUTRAL;
        frame(&mut l, f);
        let mut f = NEUTRAL;
        f[2] = true;
        frame(&mut l, f);
        assert!(l.get_state("BB"), "B,B double tap matched");
    }

    #[test]
    fn compile_errors_and_ikemen_compat() {
        let mut l = CommandList::new();
        let mut w = Vec::new();
        // bad symbol → hard error for Ikemen chars
        let r = compile_cmd_text(
            &mut l,
            "[Command]\nname = \"bad\"\ncommand = Q\n",
            "test",
            false,
            &mut w,
        );
        assert!(r.is_err());
        // same text for a MUGEN char (ikemenversion 0,0) → accepted
        let mut l2 = CommandList::new();
        let r2 = compile_cmd_text(
            &mut l2,
            "[Command]\nname = \"bad\"\ncommand = Q\n",
            "test",
            true,
            &mut w,
        );
        assert!(r2.is_ok());
        assert!(!l2.get("bad").is_empty(), "command kept despite parse error");
    }

    #[test]
    fn backward_compat_slash_propagation() {
        // MUGEN-version compat (input.go:2469): '/' spreads across the
        // step ("/x+y" → both keys), and hold-only commands get buffer
        // time forced to 1 ("/x" with buffer.time = 3).
        let mut l = CommandList::new();
        let mut w = Vec::new();
        compile_cmd_text(
            &mut l,
            "[Command]\nname = \"xy\"\ncommand = /x+y\n[Command]\nname = \"holdx\"\ncommand = /x\nbuffer.time = 3\n",
            "kfm",
            true,
            &mut w,
        )
        .unwrap();
        let xy = &l.get("xy")[0];
        assert!(xy.steps[0].keys.iter().all(|k| k.slash), "slash propagated to y");
        // after propagation every key has '/', so the whole command counts
        // as hold-only → buffer forced to 1 with a warning
        let holdx = &l.get("holdx")[0];
        assert_eq!(holdx.maxbuftime, 1, "hold-only buffer forced to 1");
        assert!(w.iter().any(|m| m.contains("slash propagated")));
        assert!(w.iter().any(|m| m.contains("buffer time forced")));
    }

    #[test]
    fn buf_reset_clears_everything() {
        let mut l = list_from("[Command]\nname = \"holdx\"\ncommand = /x\n");
        settle(&mut l, 5);
        let mut f = NEUTRAL;
        f[7] = true;
        frame(&mut l, f);
        frame(&mut l, f);
        assert!(l.get_state("holdx"));
        l.buf_reset();
        assert!(!l.get_state("holdx"));
        assert_eq!(l.buffer.cur, [0; BUFFER_KEYS]);
    }

    #[test]
    fn copy_list_gives_fresh_runtime_state() {
        let mut src = list_from("[Command]\nname = \"x_cmd\"\ncommand = x\n");
        settle(&mut src, 5);
        let mut f = NEUTRAL;
        f[7] = true;
        frame(&mut src, f);
        assert!(src.get_state("x_cmd"));
        // let the 1-tick buffer expire on the template (Go's CopyList keeps
        // curbuftime values; in the engine templates are never stepped)
        frame(&mut src, NEUTRAL);
        assert!(!src.get_state("x_cmd"));
        let mut dst = CommandList::new();
        dst.copy_list(&src);
        assert!(!dst.get_state("x_cmd"));
        // the copy works independently
        frame(&mut dst, f);
        assert!(dst.get_state("x_cmd"));
    }

    #[test]
    fn socd_methods() {
        let mut ir = InputReader::default();
        // SOCD "last priority": press U first, then add D → D wins
        let (u, d, _, _) = ir.socd_resolution(true, false, false, false, SOCD_LAST);
        assert!(u && !d);
        let (u, d, _, _) = ir.socd_resolution(true, true, false, false, SOCD_LAST);
        assert!(!u && d, "later direction wins with SOCD_LAST");
        // deny both (any other method value)
        let mut ir2 = InputReader::default();
        let (u, d, _, _) = ir2.socd_resolution(true, true, false, false, 99);
        assert!(!u && !d);
        // allow (default): both pass
        let mut ir3 = InputReader::default();
        let (u, d, _, _) = ir3.socd_resolution(true, true, false, false, SOCD_ALLOW);
        assert!(u && d);
        // absolute: F (offense) wins over B
        let mut ir4 = InputReader::default();
        let (_, _, b, f) = ir4.socd_resolution(false, false, true, true, SOCD_ABSOLUTE);
        assert!(!b && f);
    }

    #[test]
    fn assert_command_forces_state() {
        let mut l = list_from("[Command]\nname = \"recovery\"\ncommand = x+y\n");
        settle(&mut l, 5);
        assert!(!l.get_state("recovery"));
        assert!(l.assert("recovery", 5));
        assert!(l.get_state("recovery"));
        // holds for 5 frames
        frame(&mut l, NEUTRAL);
        frame(&mut l, NEUTRAL);
        assert!(l.get_state("recovery"));
        for _ in 0..4 {
            frame(&mut l, NEUTRAL);
        }
        assert!(!l.get_state("recovery"));
    }
}
