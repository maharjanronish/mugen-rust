//! Port origin: ikemen-src/src/input.go — the MUGEN command system.
//!
//! * `CommandKey`/`CommandStepKey`/`CommandStep` — input.go:16-53, 2015-2085
//! * `InputBuffer` — input.go:674-769 (`updateInputTime`), `State`
//!   (762-1385), `StateCharge` (1388-1690): per-key press/release frame
//!   counters with conflict (SOCD-ish) resolution *inside* the matcher.
//! * `Command` — input.go:2088-2709: `ReadCommandSymbols` (2109), Step
//!   (2543) with step sequencing, `>` checks, charge, buffering.
//! * `CommandList` — input.go:2711-3091: defaults, add/step/clear-name,
//!   same-name variants (piano-input reset), hitpause/pause buffering.
//! * cmd-file reader — compiler.go:8636-8769 ([Remap]/[Defaults]/[Command])
//! * `AiInput` — input.go:1941-2013 (AI button jamming)
//! * `LehmerRng` — common.go:30 `Random` (Schrage Lehmer, bit-exact)
//!
//! Semantics preserved exactly:
//! * buffer counters: +1 per held frame, -1 per released frame,
//!   ±1 on the change frame (updateInputTime, input.go:718)
//! * `slash` (`/x`) matches `state > 0`, plain presses `state == 1`
//! * conflicts: a cardinal direction reads negative if a perpendicular
//!   direction is held (U conflicts with B/D/F; L/R conflict with U/D and
//!   each other — NOT with B/F, input.go:774)
//! * `>` steps invalidate if any key changed in between (GreaterCheckFail)
//! * steps evaluate in reverse order except `IsDirToButton` runs, which
//!   evaluate forwards so a direction+press lands on the same frame
//! * command completion buffers `maxbuftime + extratime` frames;
//!   hitpause/pause buffering only if the command opted in

use crate::ini::{atoi, read_ini_section, split_and_trim};
use std::collections::HashMap;

// ── CommandKey (input.go:25-53) ─────────────────────────────────────────────

/// Key identifiers — ordinal order is load-bearing (`IsButtonPress` uses
/// `key >= CK_a`), mirrors Go's iota block exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum CommandKey {
    U = 0,
    D = 1,
    B = 2,
    F = 3,
    L = 4,
    R = 5,
    Ub = 6,
    Uf = 7,
    Db = 8,
    Df = 9,
    Ul = 10,
    Ur = 11,
    Dl = 12,
    Dr = 13,
    N = 14,
    A = 15,
    Bb = 16,
    C = 17,
    X = 18,
    Y = 19,
    Z = 20,
    S = 21,
    Dd = 22, // Ikemen extra button "d"
    W = 23,  // Ikemen extra button "w"
    M = 24,  // Ikemen extra button "m"
}

impl CommandKey {
    /// Parse a remap value ("x", "start", "d"…) — input.go:419 CommandKeyRemap.
    pub fn from_button_name(s: &str) -> Option<CommandKey> {
        Some(match s.trim().to_lowercase().as_str() {
            "a" => CommandKey::A,
            "b" => CommandKey::Bb,
            "c" => CommandKey::C,
            "x" => CommandKey::X,
            "y" => CommandKey::Y,
            "z" => CommandKey::Z,
            "s" | "start" => CommandKey::S,
            "d" => CommandKey::Dd,
            "w" => CommandKey::W,
            "m" => CommandKey::M,
            _ => return None,
        })
    }
    pub fn is_direction(self) -> bool {
        self <= CommandKey::N
    }
}

/// input.go:16 — one key of one command step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandStepKey {
    pub key: CommandKey,
    pub slash: bool,
    pub tilde: bool,
    pub dollar: bool,
    pub star: bool,
    pub chargetime: i32,
}

impl CommandStepKey {
    /// input.go:55 — press directions: U..N without tilde
    pub fn is_direction_press(&self) -> bool {
        !self.tilde && self.key.is_direction()
    }
    /// input.go:59
    pub fn is_direction_release(&self) -> bool {
        self.tilde && self.key.is_direction()
    }
    /// input.go:63 — button keys are the tail of the enum
    pub fn is_button_press(&self) -> bool {
        !self.tilde && self.key >= CommandKey::A
    }
    /// input.go:67
    pub fn is_button_release(&self) -> bool {
        self.tilde && self.key >= CommandKey::A
    }
}

/// input.go:419 — per-character button remap (cmd [Remap] section).
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
    fn default() -> Self {
        CommandKeyRemap {
            a: CommandKey::A,
            b: CommandKey::Bb,
            c: CommandKey::C,
            x: CommandKey::X,
            y: CommandKey::Y,
            z: CommandKey::Z,
            s: CommandKey::S,
            d: CommandKey::Dd,
            w: CommandKey::W,
            m: CommandKey::M,
        }
    }
}

// ── InputBuffer (input.go:674) ───────────────────────────────────────────────

/// Slot layout: directions U D B F L R N (0..6), buttons a b c x y z s d w m
/// (7..16). `Ub`→`cur[KU]`, `Up`→`prev[KU]`, `ab`→`cur[KA]`, `db`(button)→
/// `cur[KDB]`. Go distinguishes `Db`/`db` by case; we by constant.
pub const KU: usize = 0;
pub const KD: usize = 1;
pub const KB: usize = 2;
pub const KF: usize = 3;
pub const KL: usize = 4;
pub const KR: usize = 5;
pub const KN: usize = 6;
pub const KA: usize = 7;
pub const KBB: usize = 8;
pub const KC: usize = 9;
pub const KX: usize = 10;
pub const KY: usize = 11;
pub const KZ: usize = 12;
pub const KS: usize = 13;
pub const KDB: usize = 14; // "d" button buffer
pub const KW: usize = 15;
pub const KM: usize = 16;

/// input.go:674 — per-key press (`cur`) / previous-frame (`prev`) counters.
/// >0: held for N frames; <0: released for N frames; ±1: change frame.
#[derive(Debug, Clone)]
pub struct InputBuffer {
    pub cur: [i32; 17],
    pub prev: [i32; 17],
}

impl Default for InputBuffer {
    fn default() -> Self {
        InputBuffer { cur: [0; 17], prev: [0; 17] }
    }
}

#[inline(always)]
fn maxi(a: i32, b: i32) -> i32 {
    if a > b { a } else { b }
}
#[inline(always)]
fn mini(a: i32, b: i32) -> i32 {
    if a < b { a } else { b }
}
#[inline(always)]
fn absi(a: i32) -> i32 {
    if a < 0 { -a } else { a }
}

impl InputBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// input.go:688
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// input.go:697 `updateInputTime` — args are *physical* U/D/L/R (pad)
    /// plus engine B/F and the ten buttons. Neutral N is `!(any dir)`.
    pub fn update_input_time(
        &mut self,
        u: bool, d: bool, l: bool, r: bool, b: bool, f: bool,
        a: bool, b2: bool, c: bool, x: bool, y: bool, z: bool,
        s: bool, dd: bool, w: bool, m: bool,
    ) {
        self.prev.copy_from_slice(&self.cur);

        let mut upd = |held: bool, slot: &mut i32| {
            // Detect change (input.go:718-727)
            if held != (*slot > 0) {
                *slot = if held { 1 } else { -1 };
                return;
            }
            // Advance timer
            *slot += if held { 1 } else { -1 };
        };
        upd(u, &mut self.cur[KU]);
        upd(d, &mut self.cur[KD]);
        upd(l, &mut self.cur[KL]);
        upd(r, &mut self.cur[KR]);
        upd(b, &mut self.cur[KB]);
        upd(f, &mut self.cur[KF]);
        let nodir = !(u || d || l || r || b || f);
        upd(nodir, &mut self.cur[KN]);
        upd(a, &mut self.cur[KA]);
        upd(b2, &mut self.cur[KBB]);
        upd(c, &mut self.cur[KC]);
        upd(x, &mut self.cur[KX]);
        upd(y, &mut self.cur[KY]);
        upd(z, &mut self.cur[KZ]);
        upd(s, &mut self.cur[KS]);
        upd(dd, &mut self.cur[KDB]);
        upd(w, &mut self.cur[KW]);
        upd(m, &mut self.cur[KM]);
    }

    /// input.go:764 `State` — state of one command key this frame.
    /// Returns frame count (>0 held, <0 released, 0 n/a).
    /// NOTE: Go's `switch` falls through to the next `if` when the key
    /// doesn't match any case — mirrored here with Option returns.
    pub fn state(&self, ck: &CommandStepKey) -> i32 {
        let dollar = ck.dollar && !ck.star;
        let star = !ck.dollar && ck.star;
        let simple = !dollar && !star;

        let c = &self.cur;
        let p = &self.prev;

        // Hold simple directions (input.go:771-848)
        if !ck.tilde && simple {
            let r: Option<i32> = match ck.key {
                CommandKey::U => Some(mini(-maxi(c[KB], maxi(c[KD], c[KF])), c[KU])),
                CommandKey::D => Some(mini(-maxi(c[KB], maxi(c[KU], c[KF])), c[KD])),
                CommandKey::B => Some(mini(-maxi(c[KD], maxi(c[KU], c[KF])), c[KB])),
                CommandKey::F => Some(mini(-maxi(c[KD], maxi(c[KU], c[KB])), c[KF])),
                CommandKey::L => Some(mini(-maxi(c[KD], maxi(c[KU], c[KR])), c[KL])),
                CommandKey::R => Some(mini(-maxi(c[KD], maxi(c[KU], c[KL])), c[KR])),
                CommandKey::Uf => Some(mini(-maxi(c[KD], c[KB]), mini(c[KU], c[KF]))),
                CommandKey::Ub => Some(mini(-maxi(c[KD], c[KF]), mini(c[KU], c[KB]))),
                CommandKey::Df => Some(mini(-maxi(c[KU], c[KB]), mini(c[KD], c[KF]))),
                CommandKey::Db => Some(mini(-maxi(c[KU], c[KF]), mini(c[KD], c[KB]))),
                CommandKey::Ul => Some(mini(-maxi(c[KD], c[KR]), mini(c[KU], c[KL]))),
                CommandKey::Ur => Some(mini(-maxi(c[KD], c[KL]), mini(c[KU], c[KR]))),
                CommandKey::Dl => Some(mini(-maxi(c[KU], c[KR]), mini(c[KD], c[KL]))),
                CommandKey::Dr => Some(mini(-maxi(c[KU], c[KL]), mini(c[KD], c[KR]))),
                CommandKey::N => Some(c[KN]),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Hold star directions (input.go:851-899) — no conflicts
        if !ck.tilde && star {
            let r: Option<i32> = match ck.key {
                CommandKey::U => Some(c[KU]),
                CommandKey::D => Some(c[KD]),
                CommandKey::B => Some(c[KB]),
                CommandKey::F => Some(c[KF]),
                CommandKey::L => Some(c[KL]),
                CommandKey::R => Some(c[KR]),
                // diagonals accept either component
                CommandKey::Ub => Some(maxi(c[KU], c[KB])),
                CommandKey::Uf => Some(maxi(c[KU], c[KF])),
                CommandKey::Db => Some(maxi(c[KD], c[KB])),
                CommandKey::Df => Some(maxi(c[KD], c[KF])),
                CommandKey::Ul => Some(maxi(c[KU], c[KL])),
                CommandKey::Ur => Some(maxi(c[KU], c[KR])),
                CommandKey::Dl => Some(maxi(c[KD], c[KL])),
                CommandKey::Dr => Some(maxi(c[KD], c[KR])),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Hold dollar directions (input.go:901-979) — lenient 4-way
        if !ck.tilde && dollar {
            let m4 = mini(absi(c[KU]), mini(absi(c[KD]), mini(absi(c[KB]), absi(c[KF]))));
            let m4lr = mini(absi(c[KU]), mini(absi(c[KD]), mini(absi(c[KL]), absi(c[KR]))));
            let r: Option<i32> = match ck.key {
                CommandKey::U if c[KU] > 0 => Some(m4),
                CommandKey::D if c[KD] > 0 => Some(m4),
                CommandKey::B if c[KB] > 0 => Some(m4),
                CommandKey::F if c[KF] > 0 => Some(m4),
                CommandKey::L if c[KL] > 0 => Some(m4lr),
                CommandKey::R if c[KR] > 0 => Some(m4lr),
                CommandKey::Ub if c[KU] > 0 && c[KB] > 0 => Some(m4),
                CommandKey::Uf if c[KU] > 0 && c[KF] > 0 => Some(m4),
                CommandKey::Db if c[KD] > 0 && c[KB] > 0 => Some(m4),
                CommandKey::Df if c[KD] > 0 && c[KF] > 0 => Some(m4),
                CommandKey::Ul if c[KU] > 0 && c[KL] > 0 => Some(m4lr),
                CommandKey::Ur if c[KU] > 0 && c[KR] > 0 => Some(m4lr),
                CommandKey::Dl if c[KD] > 0 && c[KL] > 0 => Some(m4lr),
                CommandKey::Dr if c[KD] > 0 && c[KR] > 0 => Some(m4lr),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Release simple directions (input.go:981-1088)
        // Gate: `Ub < 0 || Up > 0` — released this frame or held last frame.
        if ck.tilde && simple {
            let rl = |gate: bool, conflict: i32, intended: i32| -> i32 {
                if gate { -mini(conflict, intended) } else { 0 }
            };
            let r: Option<i32> = match ck.key {
                CommandKey::U => Some(rl(
                    c[KU] < 0 || p[KU] > 0,
                    -maxi(c[KB], maxi(c[KD], c[KF])),
                    c[KU],
                )),
                CommandKey::D => Some(rl(
                    c[KD] < 0 || p[KD] > 0,
                    -maxi(c[KB], maxi(c[KU], c[KF])),
                    c[KD],
                )),
                CommandKey::B => Some(rl(
                    c[KB] < 0 || p[KB] > 0,
                    -maxi(c[KD], maxi(c[KU], c[KF])),
                    c[KB],
                )),
                CommandKey::F => Some(rl(
                    c[KF] < 0 || p[KF] > 0,
                    -maxi(c[KD], maxi(c[KU], c[KB])),
                    c[KF],
                )),
                CommandKey::L => Some(rl(
                    c[KL] < 0 || p[KL] > 0,
                    -maxi(c[KD], maxi(c[KU], c[KR])),
                    c[KL],
                )),
                CommandKey::R => Some(rl(
                    c[KR] < 0 || p[KR] > 0,
                    -maxi(c[KD], maxi(c[KU], c[KL])),
                    c[KR],
                )),
                CommandKey::Uf => Some(rl(
                    (c[KU] < 0 || p[KU] > 0) && (c[KF] < 0 || p[KF] > 0),
                    -maxi(c[KD], c[KB]),
                    mini(c[KU], c[KF]),
                )),
                CommandKey::Ub => Some(rl(
                    (c[KU] < 0 || p[KU] > 0) && (c[KB] < 0 || p[KB] > 0),
                    -maxi(c[KD], c[KF]),
                    mini(c[KU], c[KB]),
                )),
                CommandKey::Df => Some(rl(
                    (c[KD] < 0 || p[KD] > 0) && (c[KF] < 0 || p[KF] > 0),
                    -maxi(c[KU], c[KB]),
                    mini(c[KD], c[KF]),
                )),
                CommandKey::Db => Some(rl(
                    (c[KD] < 0 || p[KD] > 0) && (c[KB] < 0 || p[KB] > 0),
                    -maxi(c[KU], c[KF]),
                    mini(c[KD], c[KB]),
                )),
                CommandKey::Ul => Some(rl(
                    (c[KU] < 0 || p[KU] > 0) && (c[KL] < 0 || p[KL] > 0),
                    -maxi(c[KD], c[KR]),
                    mini(c[KU], c[KL]),
                )),
                CommandKey::Ur => Some(rl(
                    (c[KU] < 0 || p[KU] > 0) && (c[KR] < 0 || p[KR] > 0),
                    -maxi(c[KD], c[KL]),
                    mini(c[KU], c[KR]),
                )),
                CommandKey::Dl => Some(rl(
                    (c[KD] < 0 || p[KD] > 0) && (c[KL] < 0 || p[KL] > 0),
                    -maxi(c[KU], c[KR]),
                    mini(c[KD], c[KL]),
                )),
                CommandKey::Dr => Some(rl(
                    (c[KD] < 0 || p[KD] > 0) && (c[KR] < 0 || p[KR] > 0),
                    -maxi(c[KU], c[KL]),
                    mini(c[KD], c[KR]),
                )),
                CommandKey::N => Some(-c[KN]),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Release star directions (input.go:1090-1163)
        if ck.tilde && star {
            let r: Option<i32> = match ck.key {
                CommandKey::U if c[KU] < 0 || p[KU] > 0 => Some(-c[KU]),
                CommandKey::D if c[KD] < 0 || p[KD] > 0 => Some(-c[KD]),
                CommandKey::B if c[KB] < 0 || p[KB] > 0 => Some(-c[KB]),
                CommandKey::F if c[KF] < 0 || p[KF] > 0 => Some(-c[KF]),
                CommandKey::L if c[KL] < 0 || p[KL] > 0 => Some(-c[KL]),
                CommandKey::R if c[KR] < 0 || p[KR] > 0 => Some(-c[KR]),
                // diagonals: either component released
                CommandKey::Ub if (c[KU] < 0 || p[KU] > 0) || (c[KB] < 0 || p[KB] > 0) => Some(-maxi(c[KU], c[KB])),
                CommandKey::Uf if (c[KU] < 0 || p[KU] > 0) || (c[KF] < 0 || p[KF] > 0) => Some(-maxi(c[KU], c[KF])),
                CommandKey::Db if (c[KD] < 0 || p[KD] > 0) || (c[KB] < 0 || p[KB] > 0) => Some(-maxi(c[KD], c[KB])),
                CommandKey::Df if (c[KD] < 0 || p[KD] > 0) || (c[KF] < 0 || p[KF] > 0) => Some(-maxi(c[KD], c[KF])),
                CommandKey::Ul if (c[KU] < 0 || p[KU] > 0) || (c[KL] < 0 || p[KL] > 0) => Some(-maxi(c[KU], c[KL])),
                CommandKey::Ur if (c[KU] < 0 || p[KU] > 0) || (c[KR] < 0 || p[KR] > 0) => Some(-maxi(c[KU], c[KR])),
                CommandKey::Dl if (c[KD] < 0 || p[KD] > 0) || (c[KL] < 0 || p[KL] > 0) => Some(-maxi(c[KD], c[KL])),
                CommandKey::Dr if (c[KD] < 0 || p[KD] > 0) || (c[KR] < 0 || p[KR] > 0) => Some(-maxi(c[KD], c[KR])),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Release dollar directions (input.go:1166-1284)
        if ck.tilde && dollar {
            let r: Option<i32> = match ck.key {
                CommandKey::U if c[KU] < 0 || p[KU] > 0 => {
                    if c[KU] < 0 { Some(-c[KU]) } else { Some(mini(absi(c[KD]), mini(absi(c[KB]), absi(c[KF])))) }
                }
                CommandKey::D if c[KD] < 0 || p[KD] > 0 => {
                    if c[KD] < 0 { Some(-c[KD]) } else { Some(mini(absi(c[KU]), mini(absi(c[KB]), absi(c[KF])))) }
                }
                CommandKey::B if c[KB] < 0 || p[KB] > 0 => {
                    if c[KB] < 0 { Some(-c[KB]) } else { Some(mini(absi(c[KU]), mini(absi(c[KD]), absi(c[KF])))) }
                }
                CommandKey::F if c[KF] < 0 || p[KF] > 0 => {
                    if c[KF] < 0 { Some(-c[KF]) } else { Some(mini(absi(c[KU]), mini(absi(c[KD]), absi(c[KB])))) }
                }
                CommandKey::L if c[KL] < 0 || p[KL] > 0 => {
                    if c[KL] < 0 { Some(-c[KL]) } else { Some(mini(absi(c[KU]), mini(absi(c[KD]), absi(c[KR])))) }
                }
                CommandKey::R if c[KR] < 0 || p[KR] > 0 => {
                    if c[KR] < 0 { Some(-c[KR]) } else { Some(mini(absi(c[KU]), mini(absi(c[KD]), absi(c[KL])))) }
                }
                CommandKey::Ub
                    if (c[KU] < 0 || p[KU] > 0) && (c[KB] < 0 || p[KB] > 0) =>
                {
                    if c[KU] < 0 || c[KB] < 0 { Some(-mini(c[KU], c[KB])) } else { Some(mini(absi(c[KD]), absi(c[KF]))) }
                }
                CommandKey::Uf
                    if (c[KU] < 0 || p[KU] > 0) && (c[KF] < 0 || p[KF] > 0) =>
                {
                    if c[KU] < 0 || c[KF] < 0 { Some(-mini(c[KU], c[KF])) } else { Some(mini(absi(c[KD]), absi(c[KB]))) }
                }
                CommandKey::Db
                    if (c[KD] < 0 || p[KD] > 0) && (c[KB] < 0 || p[KB] > 0) =>
                {
                    if c[KD] < 0 || c[KB] < 0 { Some(-mini(c[KD], c[KB])) } else { Some(mini(absi(c[KU]), absi(c[KF]))) }
                }
                CommandKey::Df
                    if (c[KD] < 0 || p[KD] > 0) && (c[KF] < 0 || p[KF] > 0) =>
                {
                    if c[KD] < 0 || c[KF] < 0 { Some(-mini(c[KD], c[KF])) } else { Some(mini(absi(c[KU]), absi(c[KB]))) }
                }
                CommandKey::Ul
                    if (c[KU] < 0 || p[KU] > 0) && (c[KL] < 0 || p[KL] > 0) =>
                {
                    if c[KU] < 0 || c[KL] < 0 { Some(-mini(c[KU], c[KL])) } else { Some(mini(absi(c[KD]), absi(c[KR]))) }
                }
                CommandKey::Ur
                    if (c[KU] < 0 || p[KU] > 0) && (c[KR] < 0 || p[KR] > 0) =>
                {
                    if c[KU] < 0 || c[KR] < 0 { Some(-mini(c[KU], c[KR])) } else { Some(mini(absi(c[KD]), absi(c[KL]))) }
                }
                CommandKey::Dl
                    if (c[KD] < 0 || p[KD] > 0) && (c[KL] < 0 || p[KL] > 0) =>
                {
                    if c[KD] < 0 || c[KL] < 0 { Some(-mini(c[KD], c[KL])) } else { Some(mini(absi(c[KU]), absi(c[KR]))) }
                }
                CommandKey::Dr
                    if (c[KD] < 0 || p[KD] > 0) && (c[KR] < 0 || p[KR] > 0) =>
                {
                    if c[KD] < 0 || c[KR] < 0 { Some(-mini(c[KD], c[KR])) } else { Some(mini(absi(c[KU]), absi(c[KL]))) }
                }
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Hold buttons (input.go:1286-1321)
        if !ck.tilde {
            let r: Option<i32> = match ck.key {
                CommandKey::A => Some(c[KA]),
                CommandKey::Bb => Some(c[KBB]),
                CommandKey::C => Some(c[KC]),
                CommandKey::X => Some(c[KX]),
                CommandKey::Y => Some(c[KY]),
                CommandKey::Z => Some(c[KZ]),
                CommandKey::S => Some(c[KS]),
                CommandKey::Dd => Some(c[KDB]),
                CommandKey::W => Some(c[KW]),
                CommandKey::M => Some(c[KM]),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Release buttons (input.go:1323-1376)
        if ck.tilde {
            let r: Option<i32> = match ck.key {
                CommandKey::A if c[KA] < 0 || p[KA] > 0 => Some(-c[KA]),
                CommandKey::Bb if c[KBB] < 0 || p[KBB] > 0 => Some(-c[KBB]),
                CommandKey::C if c[KC] < 0 || p[KC] > 0 => Some(-c[KC]),
                CommandKey::X if c[KX] < 0 || p[KX] > 0 => Some(-c[KX]),
                CommandKey::Y if c[KY] < 0 || p[KY] > 0 => Some(-c[KY]),
                CommandKey::Z if c[KZ] < 0 || p[KZ] > 0 => Some(-c[KZ]),
                CommandKey::S if c[KS] < 0 || p[KS] > 0 => Some(-c[KS]),
                CommandKey::Dd if c[KDB] < 0 || p[KDB] > 0 => Some(-c[KDB]),
                CommandKey::W if c[KW] < 0 || p[KW] > 0 => Some(-c[KW]),
                CommandKey::M if c[KM] < 0 || p[KM] > 0 => Some(-c[KM]),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        0
    }

    /// input.go:1388 `StateCharge` — charge time of a key (for `~30a`).
    pub fn state_charge(&self, ck: &CommandStepKey) -> i32 {
        let dollar = ck.dollar && !ck.star;
        let star = !ck.dollar && ck.star;
        let simple = !dollar && !star;
        let c = &self.cur;
        let p = &self.prev;

        // Hold dollar/star directions: raw counter (input.go:1404-1426)
        if !ck.tilde && (dollar || star) {
            let r: Option<i32> = match ck.key {
                CommandKey::U => Some(c[KU]),
                CommandKey::D => Some(c[KD]),
                CommandKey::B => Some(c[KB]),
                CommandKey::F => Some(c[KF]),
                CommandKey::L => Some(c[KL]),
                CommandKey::R => Some(c[KR]),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Release dollar/star directions: previous-frame counter
        // (charge = how long it was held before release, input.go:1428-1451)
        if ck.tilde && (dollar || star) {
            let r: Option<i32> = match ck.key {
                CommandKey::U => Some(p[KU]),
                CommandKey::D => Some(p[KD]),
                CommandKey::B => Some(p[KB]),
                CommandKey::F => Some(p[KF]),
                CommandKey::L => Some(p[KL]),
                CommandKey::R => Some(p[KR]),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Hold simple directions (input.go:1453-…): strict = conflict-clamped,
        // clamped at 0.
        if !ck.tilde && simple {
            let strict_of = |conflict: i32, intended: i32| maxi(0, mini(conflict, intended));
            let r: Option<i32> = match ck.key {
                CommandKey::U => Some(strict_of(-maxi(c[KD], maxi(c[KB], c[KF])), c[KU])),
                CommandKey::D => Some(strict_of(-maxi(c[KU], maxi(c[KB], c[KF])), c[KD])),
                CommandKey::B => Some(strict_of(-maxi(c[KU], maxi(c[KD], c[KF])), c[KB])),
                CommandKey::F => Some(strict_of(-maxi(c[KU], maxi(c[KD], c[KB])), c[KF])),
                CommandKey::L => Some(strict_of(-maxi(c[KU], maxi(c[KD], c[KR])), c[KL])),
                CommandKey::R => Some(strict_of(-maxi(c[KU], maxi(c[KD], c[KL])), c[KR])),
                CommandKey::Uf => Some(strict_of(-maxi(c[KD], c[KB]), mini(c[KU], c[KF]))),
                CommandKey::Ub => Some(strict_of(-maxi(c[KD], c[KF]), mini(c[KU], c[KB]))),
                CommandKey::Df => Some(strict_of(-maxi(c[KU], c[KB]), mini(c[KD], c[KF]))),
                CommandKey::Db => Some(strict_of(-maxi(c[KU], c[KF]), mini(c[KD], c[KB]))),
                CommandKey::Ul => Some(strict_of(-maxi(c[KD], c[KR]), mini(c[KU], c[KL]))),
                CommandKey::Ur => Some(strict_of(-maxi(c[KD], c[KL]), mini(c[KU], c[KR]))),
                CommandKey::Dl => Some(strict_of(-maxi(c[KU], c[KR]), mini(c[KD], c[KL]))),
                CommandKey::Dr => Some(strict_of(-maxi(c[KU], c[KL]), mini(c[KD], c[KR]))),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Hold buttons (input.go:1518-1552 area) — raw counter
        if !ck.tilde {
            let r: Option<i32> = match ck.key {
                CommandKey::A => Some(c[KA]),
                CommandKey::Bb => Some(c[KBB]),
                CommandKey::C => Some(c[KC]),
                CommandKey::X => Some(c[KX]),
                CommandKey::Y => Some(c[KY]),
                CommandKey::Z => Some(c[KZ]),
                CommandKey::S => Some(c[KS]),
                CommandKey::Dd => Some(c[KDB]),
                CommandKey::W => Some(c[KW]),
                CommandKey::M => Some(c[KM]),
                _ => None,
            };
            if let Some(v) = r {
                return v;
            }
        }

        // Release buttons — previous counter, but a *just pressed* key
        // (cur == 1) reads MinInt32 so it can't satisfy a charge
        // (input.go:1392-1397 ignoreRecent + 1554-1600)
        if ck.tilde {
            let ign = |buf: i32| if buf == 1 { i32::MIN } else { buf };
            return match ck.key {
                CommandKey::A => ign(c[KA]),
                CommandKey::Bb => ign(c[KBB]),
                CommandKey::C => ign(c[KC]),
                CommandKey::X => ign(c[KX]),
                CommandKey::Y => ign(c[KY]),
                CommandKey::Z => ign(c[KZ]),
                CommandKey::S => ign(c[KS]),
                CommandKey::Dd => ign(c[KDB]),
                CommandKey::W => ign(c[KW]),
                CommandKey::M => ign(c[KM]),
                CommandKey::U => ign(c[KU]),
                CommandKey::D => ign(c[KD]),
                CommandKey::B => ign(c[KB]),
                CommandKey::F => ign(c[KF]),
                CommandKey::L => ign(c[KL]),
                CommandKey::R => ign(c[KR]),
                _ => 0,
            };
        }

        0
    }
}

// ── CommandStep (input.go:2015) ──────────────────────────────────────────────

/// One comma-separated step of a command (may be an AND or OR of keys).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandStep {
    pub keys: Vec<CommandStepKey>,
    pub greater: bool,
    pub or_logic: bool,
}

impl CommandStep {
    /// input.go:2024 — single key that is a direction press/release
    pub fn is_single_direction(&self) -> bool {
        self.keys.len() == 1 && (self.keys[0].is_direction_press() || self.keys[0].is_direction_release())
    }

    /// input.go:2031 — can `next` complete on the same frame as `self`?
    pub fn is_dir_to_button(&self, next: &CommandStep) -> bool {
        // Not if second step must be held
        for k in &next.keys {
            if k.slash {
                return false;
            }
        }
        // Not if first step includes a button
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
        // Yes if release-direction then non-release (includes buttons)
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

    /// input.go:2072 — equality for `>` expansion
    pub fn equal_steps(&self, n: &CommandStep) -> bool {
        if self.greater != n.greater || self.keys.len() != n.keys.len() {
            return false;
        }
        self.keys == n.keys
    }
}

// ── Command (input.go:2088) ──────────────────────────────────────────────────

/// One parsed command definition ("QCF+x" and friends).
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
    completed: Vec<bool>,
    step_timers: Vec<i32>,
    loop_order: Vec<usize>,
}

impl Default for Command {
    /// newCommand (input.go:2104)
    fn default() -> Self {
        Command {
            name: String::new(),
            steps: Vec::new(),
            maxtime: 1,
            curtime: 0,
            maxbuftime: 1,
            curbuftime: 0,
            maxsteptime: 0,
            cursteptime: 0,
            autogreater: true,
            buffer_hitpause: true,
            buffer_pauseend: true,
            buffer_shared: true,
            completeframe: false,
            completed: Vec::new(),
            step_timers: Vec::new(),
            loop_order: Vec::new(),
        }
    }
}

impl Command {
    /// input.go:2109 `ReadCommandSymbols` — compile `"D, DF, F, a"` text.
    pub fn read_command_symbols(&mut self, cmdstr: &str, kr: &CommandKeyRemap) -> Result<(), String> {
        if cmdstr.trim().is_empty() {
            self.steps.clear();
            return Ok(());
        }

        let mut first_err: Option<String> = None;
        macro_rules! set_err {
            ($e:expr) => { if first_err.is_none() { first_err = Some($e); } };
        }

        for (i, stepkeys_raw) in cmdstr.split(',').enumerate() {
            self.steps.push(CommandStep::default());
            let stepkeys = stepkeys_raw.trim();
            let cs = self.steps.last_mut().unwrap();

            // First step may be blank so blank commands can be defined
            if i > 0 && stepkeys.is_empty() {
                set_err!("Empty command step found".to_string());
                continue;
            }

            // AND ('+') or OR ('|') logic within the step
            let (or_logic, key_parts): (bool, Vec<&str>) =
                if stepkeys.contains('+') && stepkeys.contains('|') {
                    set_err!("Cannot use both '+' and '|' in the same command step".to_string());
                    (false, vec![])
                } else if stepkeys.contains('|') {
                    (true, stepkeys.split('|').collect())
                } else {
                    (false, stepkeys.split('+').collect())
                };
            cs.or_logic = or_logic;

            for part_trim in key_parts {
                let mut part: &str = part_trim.trim();
                if part.is_empty() {
                    set_err!("Unexpected '+' with no key".to_string());
                    continue;
                }

                // Exactly one key per '+' section (input.go:2152-2180).
                // Diagonals count as one key. Prefix symbols are skipped.
                {
                    let bytes = part.as_bytes();
                    let mut key_count = 0i32;
                    let mut j = 0usize;
                    while j < bytes.len() {
                        let c0 = bytes[j];
                        if (c0 == b'U' || c0 == b'D') && j + 1 < bytes.len() {
                            let c1 = bytes[j + 1];
                            if c1 == b'B' || c1 == b'F' || c1 == b'L' || c1 == b'R' {
                                key_count += 1;
                                j += 2;
                                continue;
                            }
                        }
                        if matches!(
                            c0,
                            b'U' | b'D' | b'B' | b'F' | b'L' | b'R' | b'N' | b'a' | b'b' | b'c'
                                | b'x' | b'y' | b'z' | b's' | b'd' | b'w' | b'm'
                        ) {
                            key_count += 1;
                        }
                        j += 1;
                    }
                    if key_count < 1 {
                        set_err!("No keys found in command step".to_string());
                    } else if key_count > 1 {
                        set_err!("Multiple keys found without '+' separator".to_string());
                    }
                }

                // Prefix symbols (input.go:2182-2264): > ~ / $ *
                let mut slash = false;
                let mut tilde = false;
                let mut dollar = false;
                let mut star = false;
                let mut chargetime = 0i32;

                let mut chars = part.char_indices().peekable();
                let mut consumed = 0usize;
                'prefix: loop {
                    let Some(&(idx, ch)) = chars.peek() else { break };
                    match ch {
                        '>' => {
                            if cs.greater {
                                set_err!("Duplicate '>' symbol found".to_string());
                            }
                            cs.greater = true;
                            consumed = idx + 1;
                            chars.next();
                        }
                        '~' => {
                            if tilde {
                                set_err!("Duplicate '~' symbol found".to_string());
                            }
                            tilde = true;
                            consumed = idx + 1;
                            chars.next();
                            // optional charge digits
                            let rest = &part[consumed..];
                            let digits: usize = rest
                                .bytes()
                                .take_while(|b| b.is_ascii_digit())
                                .map(|_| 1)
                                .sum();
                            if digits > 0 {
                                let n = atoi(&rest[..digits]);
                                if n > 0 {
                                    chargetime = n;
                                }
                                consumed += digits;
                                for _ in 0..digits {
                                    chars.next();
                                }
                            }
                        }
                        '/' => {
                            if slash {
                                set_err!("Duplicate '/' symbol found".to_string());
                            }
                            slash = true;
                            consumed = idx + 1;
                            chars.next();
                            let rest = &part[consumed..];
                            let digits: usize = rest
                                .bytes()
                                .take_while(|b| b.is_ascii_digit())
                                .map(|_| 1)
                                .sum();
                            if digits > 0 {
                                let n = atoi(&rest[..digits]);
                                if n > 0 {
                                    chargetime = n;
                                }
                                consumed += digits;
                                for _ in 0..digits {
                                    chars.next();
                                }
                            }
                        }
                        '$' => {
                            if dollar {
                                set_err!("Duplicate '$' symbol found".to_string());
                            }
                            if star {
                                set_err!("'$' and '*' symbols cannot be used together".to_string());
                            }
                            dollar = true;
                            consumed = idx + 1;
                            chars.next();
                        }
                        '*' => {
                            if star {
                                set_err!("Duplicate '*' symbol found".to_string());
                            }
                            if dollar {
                                set_err!("'$' and '*' symbols cannot be used together".to_string());
                            }
                            star = true;
                            consumed = idx + 1;
                            chars.next();
                        }
                        _ => break 'prefix,
                    }
                }
                part = &part[consumed..];

                // ParseKey (input.go:2266-2374)
                let c0 = part.bytes().next().unwrap_or(0);
                match c0 {
                    b'B' | b'D' | b'F' | b'L' | b'R' | b'U' | b'N' => {
                        let mut k = match c0 {
                            b'U' => CommandKey::U,
                            b'D' => CommandKey::D,
                            b'B' => CommandKey::B,
                            b'F' => CommandKey::F,
                            b'L' => CommandKey::L,
                            b'R' => CommandKey::R,
                            _ => CommandKey::N,
                        };
                        // Diagonals: second letter must be B/F/L/R (input.go:2289)
                        let bytes = part.as_bytes();
                        if bytes.len() > 1 {
                            let c1 = bytes[1];
                            if (c0 == b'U' || c0 == b'D')
                                && (c1 == b'B' || c1 == b'F' || c1 == b'L' || c1 == b'R')
                            {
                                k = match (c0, c1) {
                                    (b'U', b'B') => CommandKey::Ub,
                                    (b'U', b'F') => CommandKey::Uf,
                                    (b'U', b'L') => CommandKey::Ul,
                                    (b'U', b'R') => CommandKey::Ur,
                                    (b'D', b'B') => CommandKey::Db,
                                    (b'D', b'F') => CommandKey::Df,
                                    (b'D', b'L') => CommandKey::Dl,
                                    (b'D', b'R') => CommandKey::Dr,
                                    _ => k,
                                };
                                part = &part[1..];
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
                    }
                    b'a' | b'b' | b'c' | b'x' | b'y' | b'z' | b's' | b'd' | b'w' | b'm' => {
                        // buttons go through the remap; $/* ignored for buttons
                        let k = match c0 {
                            b'a' => kr.a,
                            b'b' => kr.b,
                            b'c' => kr.c,
                            b'x' => kr.x,
                            b'y' => kr.y,
                            b'z' => kr.z,
                            b's' => kr.s,
                            b'd' => kr.d,
                            b'w' => kr.w,
                            _ => kr.m,
                        };
                        cs.keys.push(CommandStepKey {
                            key: k,
                            slash,
                            tilde,
                            dollar: false,
                            star: false,
                            chargetime,
                        });
                    }
                    _ => {
                        set_err!(format!(
                            "Invalid symbol '{}' found",
                            char::from_u32(c0 as u32).unwrap_or('?')
                        ));
                    }
                }
            }
        }

        self.auto_greater_expand();

        // Completion trackers + loop order (input.go:2381-2407)
        self.completed = vec![false; self.steps.len()];
        self.step_timers = vec![0; self.steps.len()];
        self.loop_order.clear();
        let mut i = self.steps.len() as isize - 1;
        while i >= 0 {
            let iu = i as usize;
            if iu > 0 && self.steps[iu - 1].is_dir_to_button(&self.steps[iu]) {
                // forward order for an entire IsDirToButton run
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

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// input.go:2415 `AutoGreaterExpand` — consecutive identical direction
    /// steps become `X, >~X, >X` (when autogreater).
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
        let mut new_cmd: Vec<CommandStep> = Vec::with_capacity(self.steps.len() * 2);
        new_cmd.push(self.steps[0].clone());
        for i in 1..self.steps.len() {
            let (prev, curr) = (&self.steps[i - 1], &self.steps[i]);
            if prev.is_single_direction() && curr.is_single_direction() && prev.equal_steps(curr) {
                let k0 = curr.keys[0];
                new_cmd.push(CommandStep {
                    greater: true,
                    keys: vec![CommandStepKey {
                        key: k0.key,
                        tilde: !k0.tilde,
                        dollar: k0.dollar,
                        star: k0.star,
                        slash: false,
                        chargetime: 0,
                    }],
                    or_logic: false,
                });
                new_cmd.push(CommandStep {
                    greater: true,
                    keys: curr.keys.clone(),
                    or_logic: false,
                });
            } else {
                new_cmd.push(curr.clone());
            }
        }
        self.steps = new_cmd;
    }

    /// input.go:2469 `ApplyBackwardCompatibility` — Mugen-version regressions:
    /// `/` propagates to the whole step; hold-only commands get buffer 1.
    pub fn apply_backward_compatibility(&mut self) {
        // slash propagation (input.go:2480-2504)
        for step in &mut self.steps {
            if step.or_logic {
                continue; // didn't exist in Mugen
            }
            let has_slash = step.keys.iter().any(|k| k.slash);
            if has_slash {
                for k in &mut step.keys {
                    k.slash = true;
                }
            }
        }
        // hold-only commands → buffer time 1 (input.go:2506-2525)
        if !self.steps.is_empty() {
            let hold_only = self.steps.iter().all(|s| s.keys.iter().all(|k| k.slash));
            if hold_only && self.maxbuftime != 1 {
                self.maxbuftime = 1;
            }
        }
    }

    /// input.go:2528 `Clear`
    pub fn clear(&mut self, bufreset: bool) {
        self.curtime = 0;
        self.cursteptime = 0;
        if bufreset {
            self.curbuftime = 0;
        }
        for c in &mut self.completed {
            *c = false;
        }
        for t in &mut self.step_timers {
            *t = 0;
        }
    }

    /// input.go:1868 `GreaterCheckFail` — did any key outside the step
    /// change since the previous step completed?
    fn greater_check_fail(&self, i: usize, ibuf: &InputBuffer) -> bool {
        // Which directional groups to check (L/R vs B/F sets)
        let mut use_lr = false;
        for sk in &self.steps[i].keys {
            if matches!(
                sk.key,
                CommandKey::L | CommandKey::R | CommandKey::Ul | CommandKey::Ur | CommandKey::Dl | CommandKey::Dr
            ) {
                use_lr = true;
            }
        }

        // Recent changes must belong to the step (press and release forms)
        let check_key = |k: CommandKey| -> bool {
            let press = ibuf.state(&CommandStepKey { key: k, slash: false, tilde: false, dollar: false, star: false, chargetime: 0 }) == 1;
            if press {
                let allowed = self.steps[i]
                    .keys
                    .iter()
                    .any(|sk| sk.key == k && !sk.tilde);
                if !allowed {
                    return true;
                }
            }
            let release = ibuf.state(&CommandStepKey { key: k, slash: false, tilde: true, dollar: false, star: false, chargetime: 0 }) == 1;
            if release {
                let allowed = self.steps[i]
                    .keys
                    .iter()
                    .any(|sk| sk.key == k && sk.tilde);
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
        let dir_set: [CommandKey; 6] = if use_lr {
            [CommandKey::L, CommandKey::R, CommandKey::Ul, CommandKey::Ur, CommandKey::Dl, CommandKey::Dr]
        } else {
            [CommandKey::B, CommandKey::F, CommandKey::Ub, CommandKey::Uf, CommandKey::Db, CommandKey::Df]
        };
        for k in dir_set {
            if check_key(k) {
                return true;
            }
        }
        for k in [
            CommandKey::A, CommandKey::Bb, CommandKey::C, CommandKey::X, CommandKey::Y,
            CommandKey::Z, CommandKey::S, CommandKey::Dd, CommandKey::W, CommandKey::M,
        ] {
            if check_key(k) {
                return true;
            }
        }
        false
    }

    /// input.go:2543 `Step` — advance one frame.
    ///
    /// * `ai` — AI ignores maxtime expiry
    /// * `is_helper` — AI on helpers needs `/` keys (MUGEN internal AI rule)
    /// * `hpbuf`/`pausebuf` — hitpause/pause buffering active this frame
    /// * `extratime` — extra buffer ticks from pause end
    pub fn step(&mut self, ibuf: &InputBuffer, ai: bool, is_helper: bool, mut hpbuf: bool, mut pausebuf: bool, mut extratime: i32) {
        if !self.buffer_hitpause {
            hpbuf = false;
            extratime = 0;
        }
        if !self.buffer_pauseend {
            pausebuf = false;
            extratime = 0;
        }
        if self.curbuftime > 0 && !hpbuf && !pausebuf {
            self.curbuftime -= 1;
        }
        if self.steps.is_empty() {
            return;
        }

        // Expire completed steps (input.go:2566-2586)
        let mut anydone = false;
        for i in 0..self.steps.len() {
            if self.completed[i] {
                self.step_timers[i] += 1;
                if self.maxsteptime > 0 && self.step_timers[i] > self.maxsteptime {
                    self.completed[i] = false;
                    self.step_timers[i] = 0;
                    continue;
                }
                anydone = true;
            }
        }
        if anydone {
            self.curtime += 1;
        } else if self.curtime > 0 {
            self.clear(false);
        }

        // Match inputs (input.go:2588-2686)
        for &i in &self.loop_order {
            if i > 0 && !self.completed[i - 1] {
                continue;
            }

            // MUGEN's internal AI can't use commands without '/' on helpers
            if ai && is_helper {
                let has_slash = self.steps[i].keys.iter().any(|k| k.slash);
                if !has_slash {
                    return;
                }
            }

            let mut input_matched = if self.steps[i].or_logic {
                // OR: any key matches (input.go:2614-2636)
                let mut m = false;
                for k in &self.steps[i].keys {
                    let t = ibuf.state(k);
                    let mut ok = if k.slash { t > 0 } else { t == 1 };
                    if ok && k.chargetime > 1 && ibuf.state_charge(k) < k.chargetime {
                        ok = false;
                    }
                    if ok {
                        m = true;
                        break;
                    }
                }
                m
            } else {
                // AND: all keys must match (input.go:2637-2659)
                let mut m = true;
                for k in &self.steps[i].keys {
                    let t = ibuf.state(k);
                    let mut ok = if k.slash { t > 0 } else { t == 1 };
                    if ok && k.chargetime > 1 && ibuf.state_charge(k) < k.chargetime {
                        ok = false;
                    }
                    if !ok {
                        m = false;
                        break;
                    }
                }
                m
            };

            // `>` steps (input.go:2661-2668)
            if self.steps[i].greater
                && i > 0
                && self.steps.len() >= 2
                && self.completed[i - 1]
                && !self.completed[i]
            {
                if self.greater_check_fail(i, ibuf) {
                    input_matched = false;
                    self.completed[i - 1] = false;
                    self.step_timers[i - 1] = 0;
                }
            }

            if input_matched {
                self.completed[i] = true;
                self.step_timers[i] = 0;
                if i > 0 {
                    // clear previous so it can't refresh the current
                    self.completed[i - 1] = false;
                    self.step_timers[i - 1] = 0;
                }
                if i == 0 {
                    self.curtime = 0; // start the overall window
                }
            }
        }

        // Complete iff last step completed (input.go:2688-2689)
        self.completeframe = !self.completed.is_empty() && *self.completed.last().unwrap();

        if !self.completeframe {
            if ai {
                return;
            }
            if self.curtime < self.maxtime {
                return;
            }
        }

        self.clear(false);
        if self.completeframe {
            self.curbuftime = maxi(self.curbuftime, self.maxbuftime + extratime);
        }
    }

    /// Is the command currently buffered (trigger `command = "name"`)?
    pub fn is_buffered(&self) -> bool {
        self.curbuftime > 0
    }
}

// ── CommandList (input.go:2711) ──────────────────────────────────────────────

/// The whole set of a character's commands. Same-name commands append into
/// one slot ("variants"); the `command` trigger fires if ANY variant hits.
#[derive(Debug, Clone)]
pub struct CommandList {
    pub buffer: InputBuffer,
    names: HashMap<String, usize>,
    commands: Vec<Vec<Command>>,
    pub default_time: i32,
    pub default_step_time: i32,
    pub default_auto_greater: bool,
    pub default_buffer_time: i32,
    pub default_buffer_hitpause: bool,
    pub default_buffer_pauseend: bool,
    pub default_buffer_shared: bool,
}

impl Default for CommandList {
    /// NewCommandList (input.go:2726)
    fn default() -> Self {
        CommandList {
            buffer: InputBuffer::new(),
            names: HashMap::new(),
            commands: Vec::new(),
            default_time: 15,
            default_step_time: -1, // later defaults to same as time
            default_auto_greater: true,
            default_buffer_time: 1,
            default_buffer_hitpause: true,
            default_buffer_pauseend: true,
            default_buffer_shared: true,
        }
    }
}

impl CommandList {
    pub fn new() -> Self {
        Self::default()
    }

    /// CommandList.Add (input.go:3031) — same-name commands append.
    pub fn add(&mut self, c: Command) {
        let name = c.name.clone();
        let i = match self.names.get(&name) {
            Some(&i) if i < self.commands.len() => i,
            _ => {
                self.commands.push(Vec::new());
                self.commands.len() - 1
            }
        };
        self.commands[i].push(c);
        self.names.insert(name, i);
    }

    /// CommandList.AddCommand (input.go:2741) — compile + add from a spec.
    pub fn add_command(&mut self, name: &str, cmd: &str, time: i32, buftime: i32, steptime: i32, autogreater: bool, buffer_hitpause: bool, buffer_pauseend: bool, buffer_shared: bool) -> Result<(), String> {
        let cmdstr = cmd.trim();
        if cmdstr.is_empty() {
            return Ok(()); // nothing to add, but not an error
        }
        let mut cm = Command::default();
        cm.name = name.to_string();
        cm.autogreater = autogreater;
        cm.read_command_symbols(cmdstr, &CommandKeyRemap::default())?;
        cm.maxtime = time;
        cm.maxbuftime = buftime;
        cm.maxsteptime = steptime;
        cm.buffer_hitpause = buffer_hitpause;
        cm.buffer_pauseend = pausebuf_x(buffer_pauseend);
        cm.buffer_shared = buffer_shared;
        self.add(cm);
        Ok(())
    }

    /// CommandList.Step (input.go:2995) — step every command, collect
    /// completed names, clear same-name variants (piano-input rule).
    pub fn step(&mut self, ai: bool, is_helper: bool, hpbuf: bool, pausebuf: bool, extratime: i32) {
        let mut completed: Vec<String> = Vec::new();
        for ca in &mut self.commands {
            for c in ca.iter_mut() {
                c.step(&self.buffer, ai, is_helper, hpbuf, pausebuf, extratime);
                if c.completeframe {
                    c.clear(false);
                    completed.push(c.name.clone());
                    c.completeframe = false;
                }
            }
        }
        for name in completed {
            self.clear_name(&name);
        }
    }

    /// CommandList.ClearName (input.go:2981)
    fn clear_name(&mut self, name: &str) {
        let Some(&i) = self.names.get(name) else { return };
        for c in &mut self.commands[i] {
            if !c.completeframe && c.buffer_shared && c.name == name {
                c.clear(false);
            }
        }
    }

    /// CommandList.Assert (input.go:2962) — force a command buffered.
    pub fn assert(&mut self, name: &str, time: i32) -> bool {
        let Some(&i) = self.names.get(name) else { return false };
        let mut found = false;
        for c in &mut self.commands[i] {
            if c.name == name {
                c.curbuftime = time;
                found = true;
            }
        }
        found
    }

    /// CommandList.BufReset (input.go:3019)
    pub fn buf_reset(&mut self) {
        self.buffer.reset();
        for ca in &mut self.commands {
            for c in ca.iter_mut() {
                c.clear(true);
            }
        }
    }

    /// Command trigger (`command = "name"`): any variant buffered.
    pub fn get_state(&self, name: &str) -> bool {
        let Some(&i) = self.names.get(name) else { return false };
        self.commands[i].iter().any(|c| c.curbuftime > 0)
    }

    pub fn get(&self, name: &str) -> Option<&[Command]> {
        self.names.get(name).map(|&i| self.commands[i].as_slice())
    }

    /// CommandList.CopyList (input.go:3079) — deep copy for player copies.
    pub fn copy_from(&mut self, src: &CommandList) {
        self.names = src.names.clone();
        self.commands = src.commands.clone(); // Vec deep-copies; completed vectors included
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &[Command]> {
        self.commands.iter().map(|v| v.as_slice())
    }
}

#[inline(always)]
fn pausebuf_x(b: bool) -> bool {
    b
}

// ── cmd file reader (compiler.go:8636-8769) ──────────────────────────────────

/// One `[Command]` section.
#[derive(Debug, Clone, Default)]
pub struct CommandDef {
    pub name: String,
    pub cmd: String,
    pub time: Option<i32>,
    pub step_time: Option<i32>,
    pub auto_greater: Option<bool>,
    pub buffer_time: Option<i32>,
    pub buffer_hitpause: Option<bool>,
    pub buffer_pauseend: Option<bool>,
    pub buffer_shared: Option<bool>,
}

/// Result of parsing a cmd file's command sections.
#[derive(Debug, Default)]
pub struct CmdFileCommands {
    pub remap: CommandKeyRemap,
    pub commands: Vec<CommandDef>,
    pub default_time: Option<i32>,
    pub default_step_time: Option<i32>,
    pub default_auto_greater: Option<bool>,
    pub default_buffer_time: Option<i32>,
    pub default_buffer_hitpause: Option<bool>,
    pub default_buffer_pauseend: Option<bool>,
    pub default_buffer_shared: Option<bool>,
}

impl CmdFileCommands {
    /// compiler.go:8636-8769 — parse `[Remap]`/`[Defaults]`/`[Command*]`.
    pub fn parse(text: &str) -> Result<CmdFileCommands, String> {
        let mut out = CmdFileCommands::default();
        let lines = split_and_trim(text, "\n");
        let mut i = 0usize;
        loop {
            let (is, name, _sub) = read_ini_section(&lines, &mut i);
            if name.is_empty() {
                break;
            }
            let sec = name.trim_end_matches(']').to_lowercase();
            let sec = sec.trim().to_string();
            if sec == "remap" {
                for (k, v) in is.iter() {
                    if let Some(key) = CommandKey::from_button_name(v) {
                        match k.to_lowercase().as_str() {
                            "a" => out.remap.a = key,
                            "b" => out.remap.b = key,
                            "c" => out.remap.c = key,
                            "x" => out.remap.x = key,
                            "y" => out.remap.y = key,
                            "z" => out.remap.z = key,
                            "s" => out.remap.s = key,
                            "d" => out.remap.d = key,
                            "w" => out.remap.w = key,
                            "m" => out.remap.m = key,
                            _ => {}
                        }
                    }
                }
            } else if sec == "defaults" {
                for (k, v) in is.iter() {
                    let kl = k.to_lowercase();
                    match kl.as_str() {
                        "command.time" => out.default_time = Some(atoi(v)),
                        "command.steptime" => out.default_step_time = Some(atoi(v)),
                        "command.autogreater" => out.default_auto_greater = Some(atoi(v) != 0),
                        "command.buffer.time" => out.default_buffer_time = Some(maxi(1, atoi(v))),
                        "command.buffer.hitpause" => out.default_buffer_hitpause = Some(atoi(v) != 0),
                        "command.buffer.pauseend" => out.default_buffer_pauseend = Some(atoi(v) != 0),
                        "command.buffer.shared" => out.default_buffer_shared = Some(atoi(v) != 0),
                        _ => {}
                    }
                }
            } else if sec.starts_with("command") {
                // [Command] or [Command Foo]
                let name = is
                    .get_text("name")
                    .map_err(|e| e)?
                    .ok_or_else(|| "Command name not specified".to_string())?;
                let mut cd = CommandDef { name, ..Default::default() };
                if let Some(c) = is.get("command") {
                    cd.cmd = c.to_string();
                }
                if let Some(v) = is.read_i32_one("time") {
                    cd.time = Some(v);
                }
                if let Some(v) = is.read_i32_one("steptime") {
                    cd.step_time = Some(v);
                }
                if let Some(v) = is.read_bool_one("autogreater") {
                    cd.auto_greater = Some(v);
                }
                if let Some(v) = is.read_i32_one("buffer.time") {
                    cd.buffer_time = Some(maxi(1, v));
                }
                if let Some(v) = is.read_bool_one("buffer.hitpause") {
                    cd.buffer_hitpause = Some(v);
                }
                if let Some(v) = is.read_bool_one("buffer.pauseend") {
                    cd.buffer_pauseend = Some(v);
                }
                if let Some(v) = is.read_bool_one("buffer.shared") {
                    cd.buffer_shared = Some(v);
                }
                out.commands.push(cd);
            }
        }
        Ok(out)
    }

    /// Add all parsed commands into a CommandList using list+file defaults
    /// (compiler.go:8717-8768: defaults copied from the list, then per-command
    /// overrides; steptime <= 0 → time).
    pub fn build(&self, list: &mut CommandList) -> Result<(), String> {
        for cd in &self.commands {
            let time = cd.time.unwrap_or(list.default_time);
            let steptime_raw = cd.step_time.unwrap_or(list.default_step_time);
            let steptime = if steptime_raw <= 0 { time } else { steptime_raw };
            let autogreater = cd.auto_greater.unwrap_or(list.default_auto_greater);
            let buftime = cd.buffer_time.unwrap_or(list.default_buffer_time);
            let bhp = cd.buffer_hitpause.unwrap_or(list.default_buffer_hitpause);
            let bpe = cd.buffer_pauseend.unwrap_or(list.default_buffer_pauseend);
            let bsh = cd.buffer_shared.unwrap_or(list.default_buffer_shared);
            let mut cm = Command::default();
            cm.name = cd.name.clone();
            cm.autogreater = autogreater;
            cm.read_command_symbols(cd.cmd.trim(), &self.remap)?;
            cm.maxtime = time;
            cm.maxbuftime = buftime;
            cm.maxsteptime = steptime;
            cm.buffer_hitpause = bhp;
            cm.buffer_pauseend = bpe;
            cm.buffer_shared = bsh;
            list.add(cm);
        }
        Ok(())
    }
}

// ── AiInput (input.go:1941) ──────────────────────────────────────────────────

/// AI button jamming — presses random directions/buttons for a while.
#[derive(Debug, Clone, Default)]
pub struct AiInput {
    dir: i32,
    dirt: i32,
    at: i32, bt: i32, ct: i32, xt: i32, yt: i32, zt: i32, st: i32,
    dt: i32, wt: i32, mt: i32,
}

impl AiInput {
    /// input.go:1995 `Update`
    pub fn update(&mut self, intro: bool, level: f32, rng: &mut LehmerRng) {
        if intro {
            self.dirt = 0; self.at = 0; self.bt = 0; self.ct = 0;
            self.xt = 0; self.yt = 0; self.zt = 0; self.st = 0;
            self.dt = 0; self.wt = 0; self.mt = 0;
            return;
        }
        let (mut chance, mut time) = (15i32, 60i32);

        // jam a button for a while when its timer runs out (input.go:1967)
        fn jam(t: &mut i32, rng: &mut LehmerRng, chance: i32, time: i32) -> bool {
            *t -= 1;
            if *t <= 0 {
                if rng.rand(1, chance) == 1 {
                    *t = rng.rand(1, time);
                    return true;
                }
                *t = 0;
            }
            false
        }
        if jam(&mut self.dirt, rng, chance, time) {
            self.dir = rng.rand(0, 7);
        }
        chance = ((-11.25 * level + 165.0) * 7.0) as i32;
        time = 30;
        jam(&mut self.at, rng, chance, time);
        jam(&mut self.bt, rng, chance, time);
        jam(&mut self.ct, rng, chance, time);
        jam(&mut self.xt, rng, chance, time);
        jam(&mut self.yt, rng, chance, time);
        jam(&mut self.zt, rng, chance, time);
        jam(&mut self.dt, rng, chance, time);
        jam(&mut self.wt, rng, chance, time);
        chance = 3600; // start is jammed less often
        jam(&mut self.st, rng, chance, time);
    }

    /// input.go:1945 `Buttons` — 14 signals, InputBits order.
    pub fn buttons(&self) -> [bool; 14] {
        [
            self.u(), self.d(), self.l(), self.r(),
            self.a(), self.b(), self.c(),
            self.x(), self.y(), self.z(),
            self.s(), self.dd(), self.w(), self.m(),
        ]
    }

    // input.go:1999-2013
    pub fn u(&self) -> bool { self.dirt != 0 && (self.dir == 7 || self.dir == 0 || self.dir == 1) }
    pub fn d(&self) -> bool { self.dirt != 0 && (self.dir == 3 || self.dir == 4 || self.dir == 5) }
    pub fn l(&self) -> bool { self.dirt != 0 && (self.dir == 5 || self.dir == 6 || self.dir == 7) }
    pub fn r(&self) -> bool { self.dirt != 0 && (self.dir == 1 || self.dir == 2 || self.dir == 3) }
    pub fn a(&self) -> bool { self.at != 0 }
    pub fn b(&self) -> bool { self.bt != 0 }
    pub fn c(&self) -> bool { self.ct != 0 }
    pub fn x(&self) -> bool { self.xt != 0 }
    pub fn y(&self) -> bool { self.yt != 0 }
    pub fn z(&self) -> bool { self.zt != 0 }
    pub fn s(&self) -> bool { self.st != 0 }
    pub fn dd(&self) -> bool { self.dt != 0 }
    pub fn w(&self) -> bool { self.wt != 0 }
    pub fn m(&self) -> bool { self.mt != 0 }
}

// ── Random (common.go:25-52) ─────────────────────────────────────────────────

/// Schrage/Lehmer PRNG — bit-exact port of `sys.randseed` + `Random()`
/// (common.go:30). Deterministic across platforms, zero deps.
#[derive(Debug, Clone)]
pub struct LehmerRng {
    pub seed: i32,
}

impl LehmerRng {
    pub fn new(seed: i32) -> Self {
        LehmerRng { seed }
    }

    /// common.go:35 `Srand`-compliant advance
    pub fn random(&mut self) -> i32 {
        let w = self.seed / 127773;
        self.seed = (self.seed - w * 127773) * 16807 - w * 2836;
        if self.seed <= 0 {
            self.seed += i32::MAX - (self.seed == 0) as i32;
        }
        self.seed
    }

    /// common.go:43 `Rand` — inclusive [min, max]
    pub fn rand(&mut self, min: i32, max: i32) -> i32 {
        min + self.random() / (i32::MAX / (max - min + 1) + 1)
    }

    /// common.go:64 `RandF`
    pub fn rand_f(&self, _x: f32, _y: f32) -> f32 {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: CommandKey, tilde: bool, slash: bool, dollar: bool, star: bool) -> CommandStepKey {
        CommandStepKey { key: k, slash, tilde, dollar, star, chargetime: 0 }
    }

    /// Drive the buffer with plain bools (B/F = L/R, facing right).
    fn feed(ib: &mut InputBuffer, u: bool, d: bool, l: bool, r: bool, x: bool) {
        ib.update_input_time(u, d, l, r, l, r, false, false, false, x, false, false, false, false, false, false);
    }

    /// feed with the *A* button (QCF_a)
    fn feed_a(ib: &mut InputBuffer, u: bool, d: bool, l: bool, r: bool, a: bool) {
        ib.update_input_time(u, d, l, r, l, r, a, false, false, false, false, false, false, false, false, false);
    }

    #[test]
    fn buffer_counters() {
        let mut ib = InputBuffer::new();
        feed(&mut ib, false, false, false, true, false);
        assert_eq!(ib.cur[KR], 1); // change frame
        assert_eq!(ib.cur[KF], 1);
        assert_eq!(ib.cur[KN], -1); // neutral: untouched keys drift negative
        feed(&mut ib, false, false, false, true, false);
        assert_eq!(ib.cur[KR], 2); // held 2
        assert_eq!(ib.cur[KN], -2);
        feed(&mut ib, false, false, false, false, false);
        assert_eq!(ib.cur[KR], -1); // change frame
        assert_eq!(ib.cur[KN], 1);
    }

    #[test]
    fn state_conflicts_and_dollar() {
        let mut ib = InputBuffer::new();
        // U + D together: U reads negative (conflict)
        feed(&mut ib, true, true, false, false, false);
        let s = ib.state(&key(CommandKey::U, false, false, false, false));
        assert!(s < 0, "U with D held must conflict, got {s}");
        // $U ignores conflicts → positive
        let s = ib.state(&key(CommandKey::U, false, false, true, false));
        assert!(s > 0, "$U must read positive, got {s}");
        // *U ignores conflicts → positive
        let s = ib.state(&key(CommandKey::U, false, false, false, true));
        assert!(s > 0);
        // B/F do NOT conflict with U (only D and F conflict with U... per Go)
        // (input.go:774: U conflicts with B, D, F — no L/R)
    }

    #[test]
    fn qcf_detects() {
        let mut list = CommandList::new();
        list.add_command("QCF_a", "D, DF, F, a", 15, 1, -1, true, true, true, true).unwrap();
        // quarter-circle: D, D+F, F, press a — stepping every frame like Go
        let frames: [(bool, bool, bool, bool, bool); 7] = [
            (false, true, false, false, false),  // D 1
            (false, true, false, false, false),  // D 2
            (false, true, false, true, false),   // DF 1
            (false, true, false, true, false),   // DF 2
            (false, false, false, true, false),  // F 1
            (false, false, false, true, false),  // F 2
            (false, false, false, true, true),   // F + a → fires this frame
        ];
        for (u, d, _l, r, a) in frames {
            {
                let b = &mut list.buffer;
                feed_a(b, u, d, false, r, a);
            }
            list.step(false, false, false, false, 0);
        }
        assert!(list.get_state("QCF_a"), "QCF+a must fire");
    }

    #[test]
    fn double_tap_forward_walks() {
        // "F, F" via autogreater expansion → F, >~F, >F
        let mut list = CommandList::new();
        list.add_command("FF", "F, F", 15, 1, -1, true, true, true, true).unwrap();
        let frames = [true, true, false, true];
        for held in frames {
            {
                let b = &mut list.buffer;
                feed(b, false, false, false, held, false);
            }
            list.step(false, false, false, false, 0);
        }
        assert!(list.get_state("FF"), "F,F dash must fire");
    }

    #[test]
    fn release_detects() {
        let mut list = CommandList::new();
        list.add_command("rel_x", "~x", 15, 1, -1, true, true, true, true).unwrap();
        let b = &mut list.buffer;
        feed(b, false, false, false, false, true);
        feed(b, false, false, false, false, true);
        feed(b, false, false, false, false, false);
        list.step(false, false, false, false, 0);
        assert!(list.get_state("rel_x"));
    }

    #[test]
    fn parse_kfm_style_cmd_text() {
        let text = r#"
[Remap]
x = x

[Defaults]
command.time = 15
command.buffer.time = 1

[Command]
name = "holdfwd"
command = /F
time = 1

[Command]
name = "QCF_a"
command = D, DF, F, a
time = 15
buffer.time = 2

[Command]
name = "charge_b"
command = ~30B
time = 30
"#;
        let cmds = CmdFileCommands::parse(text).unwrap();
        assert_eq!(cmds.commands.len(), 3);
        let mut list = CommandList::new();
        cmds.build(&mut list).unwrap();
        assert_eq!(list.len(), 3);
        assert!(list.get("holdfwd").is_some());
        let hold = list.get("holdfwd").unwrap();
        // hold-only command → buffer forced to 1 via backward compat
        let mut hc = hold[0].clone();
        hc.apply_backward_compatibility();
        assert_eq!(hc.maxbuftime, 1);
    }

    #[test]
    fn piano_input_reset() {
        // Two variants of the same command: first completes → second cleared
        let mut list = CommandList::new();
        list.add_command("press", "x", 15, 10, -1, true, true, true, true).unwrap();
        list.add_command("press", "x+y", 15, 10, -1, true, true, true, true).unwrap();
        let b = &mut list.buffer;
        feed(b, false, false, false, false, true);
        list.step(false, false, false, false, 0);
        assert!(list.get_state("press"));
    }

    #[test]
    fn lehmer_matches_go_sequence() {
        // sys.randseed starts 1 in Ikemen: Random() sequence 16807, 282475249...
        let mut rng = LehmerRng::new(1);
        assert_eq!(rng.random(), 16807);
        assert_eq!(rng.random(), 282475249);
        assert_eq!(rng.random(), 1622650073);
        // Rand(0, 999) stays in range
        let mut r2 = LehmerRng::new(42);
        for _ in 0..1000 {
            let v = r2.rand(0, 999);
            assert!((0..=999).contains(&v));
        }
    }
}

