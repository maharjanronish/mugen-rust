//! The Kurogane minimal touch controller.
//!
//! # Design — 4 buttons + pad + START, every MUGEN signal reachable
//!
//! MUGEN/Ikemen content can use up to 14 input signals (input.go:358).
//! On a phone, six attack buttons are clutter — Kurogane ships **four**
//! physical attack buttons and synthesizes the rest as **chords** (both
//! signals are emitted simultaneously, so `QCF+Z` and `X+Y`-style commands
//! both fire):
//!
//! | Physical | Emits |
//! |---|---|
//! | X (light punch) | X |
//! | Y (heavy punch) | Y |
//! | A (light kick)  | A |
//! | B (heavy kick)  | B |
//! | X+Y together | + Z (heavy punch) |
//! | A+B together | + C (heavy kick) |
//! | A+X together | + D (Ikemen extra) |
//! | B+Y together | + W (Ikemen extra) |
//! | A+B+X+Y all four | + M (Ikemen extra) |
//! | START | S |
//! | 8-way pad | PU/PD/PL/PR (physical dirs; engine maps F/B by facing) |
//!
//! Double-tap dashes (`F,F`), quarter-circles and jumps work natively:
//! MUGEN command syntax reads them from the raw direction stream, so no
//! hidden controller magic is needed or wanted.
//!
//! # Layout ownership
//! Zone geometry lives HERE (Rust) in normalized screen units; the UI layer
//! queries [`TouchController::zones`] and only renders. One source of truth.
//!
//! Coordinates passed to [`TouchController::touch`] are view pixels together
//! with the view size; the controller normalizes internally.

use crate::bits;
use std::collections::HashMap;

/// Touch lifecycle phase (matches Android action codes: 0 down, 1 move, 2 up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

/// Renderable zone descriptor (pixel space).
#[derive(Debug, Clone, Copy)]
pub struct ZoneInfo {
    /// 0 = d-pad, 1 = button, 2 = start.
    pub kind: i32,
    /// Button index for kind 1: 0=A, 1=B, 3=X, 4=Y (bit position - 4).
    pub param: i32,
    /// Center in pixels.
    pub cx: f32,
    pub cy: f32,
    /// Radius in pixels.
    pub r: f32,
    /// Currently held.
    pub held: bool,
    /// Pad only: direction (-1|0|1) for the knob visual.
    pub ex: f32,
    pub ey: f32,
}

// ── Layout constants (normalized: centers as fractions of W/H, radii of the
//    shorter side) — tuned for landscape phones. ─────────────────────────────
const PAD_CX: f32 = 0.135;
const PAD_CY: f32 = 0.60;
const PAD_R: f32 = 0.115;
const BTN_R: f32 = 0.070;
const START_CX: f32 = 0.945;
const START_CY: f32 = 0.085;
const START_R: f32 = 0.028;
/// (button bit, cx, cy) — punches top row, kicks bottom row.
const BTNS: [(u32, f32, f32); 4] = [
    (bits::X, 0.720, 0.50),
    (bits::Y, 0.865, 0.50),
    (bits::A, 0.720, 0.70),
    (bits::B, 0.865, 0.70),
];
/// Hit-test generosity: buttons are easier to hit than they look.
const BTN_HIT_SCALE: f32 = 1.35;
/// Pad deadzone as a fraction of the pad radius (component test).
const PAD_DEADZONE: f32 = 0.40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cap {
    None,
    Pad,
    Btn(u32),
    Start,
}

/// The controller. Feed touches in, call [`TouchController::tick`] once per
/// frame (60 fps), read the emitted [`bits`](crate::bits).
#[derive(Debug, Default)]
pub struct TouchController {
    /// Pointer id → captured zone (captured on down; kept until up).
    pointers: HashMap<u64, Cap>,
    /// Physical direction bits currently held (from the pad).
    raw_dirs: u32,
    /// Physical button bits currently held (A/B/X/Y/S).
    raw_btns: u32,
    /// Last frame's emitted bits (after chord synthesis).
    held: u32,
    /// Frame before last (for edge detection).
    prev: u32,
}

impl TouchController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one touch event. `(x, y)` in view pixels; `w`/`h` the view size.
    pub fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, w: f32, h: f32) {
        match phase {
            TouchPhase::Down => {
                let cap = self.hit(x, y, w, h);
                self.pointers.insert(id, cap);
                self.apply(cap, x, y, w, h);
            }
            TouchPhase::Move => {
                if let Some(&cap) = self.pointers.get(&id) {
                    self.apply(cap, x, y, w, h);
                }
            }
            TouchPhase::Up => {
                if let Some(cap) = self.pointers.remove(&id) {
                    self.release(cap);
                }
            }
        }
    }

    /// Advance one frame: synthesize chords, update edges, return held bits.
    pub fn tick(&mut self) -> u32 {
        let mut out = self.raw_btns;
        if out & bits::X != 0 && out & bits::Y != 0 {
            out |= bits::Z;
        }
        if out & bits::A != 0 && out & bits::B != 0 {
            out |= bits::C;
        }
        if out & bits::A != 0 && out & bits::X != 0 {
            out |= bits::D;
        }
        if out & bits::B != 0 && out & bits::Y != 0 {
            out |= bits::W;
        }
        if out & (bits::A | bits::B | bits::X | bits::Y) == (bits::A | bits::B | bits::X | bits::Y) {
            out |= bits::M;
        }
        self.prev = self.held;
        self.held = out | self.raw_dirs;
        self.held
    }

    /// Currently held bits (as of the last tick).
    pub fn held(&self) -> u32 {
        self.held
    }

    /// Bits newly pressed this frame.
    pub fn pressed(&self) -> u32 {
        self.held & !self.prev
    }

    /// Bits released this frame.
    pub fn released(&self) -> u32 {
        self.prev & !self.held
    }

    /// Renderable zone geometry for a view of `w`×`h` pixels, with live
    /// held-state (call per frame — cheap).
    pub fn zones(&self, w: f32, h: f32) -> Vec<ZoneInfo> {
        let m = w.min(h);
        let mut v = Vec::with_capacity(6);
        v.push(ZoneInfo {
            kind: 0,
            param: 0,
            cx: PAD_CX * w,
            cy: PAD_CY * h,
            r: PAD_R * m,
            held: self.raw_dirs != 0,
            ex: dir_x(self.raw_dirs),
            ey: dir_y(self.raw_dirs),
        });
        for (b, cx, cy) in BTNS {
            v.push(ZoneInfo {
                kind: 1,
                param: (b.trailing_zeros() as i32) - 4, // A=0 B=1 C=2 X=3 Y=4 Z=5
                cx: cx * w,
                cy: cy * h,
                r: BTN_R * m,
                held: self.raw_btns & b != 0,
                ex: 0.0,
                ey: 0.0,
            });
        }
        v.push(ZoneInfo {
            kind: 2,
            param: 0,
            cx: START_CX * w,
            cy: START_CY * h,
            r: START_R * m,
            held: self.raw_btns & bits::S != 0,
            ex: 0.0,
            ey: 0.0,
        });
        v
    }

    /// Human-readable held-state (HUD string).
    pub fn describe(&self) -> String {
        let mut s = String::new();
        for i in 0..14u32 {
            let b = 1 << i;
            if self.held & b != 0 {
                if !s.is_empty() {
                    s.push(' ');
                }
                s.push_str(bits::name(b));
            }
        }
        s
    }

    // ── internals ──────────────────────────────────────────────────────────

    fn hit(&self, x: f32, y: f32, w: f32, h: f32) -> Cap {
        let m = w.min(h);
        for (b, cx, cy) in BTNS {
            if dist2(x, y, cx * w, cy * h) < (BTN_R * m * BTN_HIT_SCALE).powi(2) {
                return Cap::Btn(b);
            }
        }
        if dist2(x, y, START_CX * w, START_CY * h) < (START_R * m * BTN_HIT_SCALE).powi(2) {
            return Cap::Start;
        }
        if dist2(x, y, PAD_CX * w, PAD_CY * h) < (PAD_R * m * 1.10).powi(2) {
            return Cap::Pad;
        }
        Cap::None
    }

    fn apply(&mut self, cap: Cap, x: f32, y: f32, w: f32, h: f32) {
        match cap {
            Cap::None => {}
            Cap::Pad => {
                let (cx, cy) = (PAD_CX * w, PAD_CY * h);
                let r = PAD_R * w.min(h);
                let (dx, dy) = (x - cx, y - cy);
                let t = PAD_DEADZONE * r;
                let mut d = 0u32;
                if dx > t {
                    d |= bits::PR;
                } else if dx < -t {
                    d |= bits::PL;
                }
                if dy < -t {
                    d |= bits::PU;
                } else if dy > t {
                    d |= bits::PD;
                }
                self.raw_dirs = d;
            }
            Cap::Btn(b) => self.raw_btns |= b,
            Cap::Start => self.raw_btns |= bits::S,
        }
    }

    fn release(&mut self, cap: Cap) {
        match cap {
            Cap::None => {}
            Cap::Pad => self.raw_dirs = 0,
            Cap::Btn(b) => self.raw_btns &= !b,
            Cap::Start => self.raw_btns &= !bits::S,
        }
    }
}

fn dist2(x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let (dx, dy) = (x1 - x2, y1 - y2);
    dx * dx + dy * dy
}

fn dir_x(dirs: u32) -> f32 {
    (dirs & bits::PR != 0) as i32 as f32 - (dirs & bits::PL != 0) as i32 as f32
}
fn dir_y(dirs: u32) -> f32 {
    (dirs & bits::PD != 0) as i32 as f32 - (dirs & bits::PU != 0) as i32 as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::*;

    // Reference screen: 1600×900 landscape.
    const SW: f32 = 1600.0;
    const SH: f32 = 900.0;

    fn btn_xy(b: u32) -> (f32, f32) {
        let (bcx, bcy) = {
            let (_, bcx, bcy) = *BTNS.iter().find(|&&(bb, _, _)| bb == b).unwrap();
            (bcx, bcy)
        };
        (bcx * SW, bcy * SH)
    }

    fn hit_miss_screen() -> (f32, f32) {
        (SW / 2.0, SH / 2.0)
    }

    #[test]
    fn tap_x_emits_only_x() {
        let mut c = TouchController::new();
        let (x, y) = btn_xy(X);
        c.touch(1, TouchPhase::Down, x, y, SW, SH);
        assert_eq!(c.tick(), X);
    }

    #[test]
    fn chord_xy_emits_z() {
        let mut c = TouchController::new();
        let (xx, xy) = btn_xy(X);
        let (yx, yy) = btn_xy(Y);
        c.touch(1, TouchPhase::Down, xx, xy, SW, SH);
        c.touch(2, TouchPhase::Down, yx, yy, SW, SH);
        assert_eq!(c.tick(), X | Y | Z);
        // Release one side: chord drops, X stays.
        c.touch(2, TouchPhase::Up, yx, yy, SW, SH);
        assert_eq!(c.tick(), X);
    }

    #[test]
    fn chord_ab_emits_c() {
        let mut c = TouchController::new();
        let (ax, ay) = btn_xy(A);
        let (bx, by) = btn_xy(B);
        c.touch(1, TouchPhase::Down, ax, ay, SW, SH);
        c.touch(2, TouchPhase::Down, bx, by, SW, SH);
        assert_eq!(c.tick(), A | B | C);
    }

    #[test]
    fn chords_ax_d_and_by_w() {
        let mut c = TouchController::new();
        let (ax, ay) = btn_xy(A);
        let (xx, xy) = btn_xy(X);
        c.touch(1, TouchPhase::Down, ax, ay, SW, SH);
        c.touch(2, TouchPhase::Down, xx, xy, SW, SH);
        // A+X → D (and no Z: Y not held; no C: B not held).
        assert_eq!(c.tick(), A | X | D);
        let (bx, by) = btn_xy(B);
        let (yx, yy) = btn_xy(Y);
        c.touch(3, TouchPhase::Down, bx, by, SW, SH);
        c.touch(4, TouchPhase::Down, yx, yy, SW, SH);
        // All four: every chord fires.
        assert_eq!(c.tick(), A | B | X | Y | Z | C | D | W | M);
    }

    #[test]
    fn pad_right_and_diagonal() {
        let mut c = TouchController::new();
        let (px, py) = (PAD_CX * SW, PAD_CY * SH);
        c.touch(1, TouchPhase::Down, px + 80.0, py, SW, SH);
        assert_eq!(c.tick(), PR);
        // Drag to up-left.
        c.touch(1, TouchPhase::Move, px - 80.0, py - 80.0, SW, SH);
        assert_eq!(c.tick(), PL | PU);
        c.touch(1, TouchPhase::Up, px - 80.0, py - 80.0, SW, SH);
        assert_eq!(c.tick(), 0);
    }

    #[test]
    fn pad_deadzone() {
        let mut c = TouchController::new();
        let (px, py) = (PAD_CX * SW, PAD_CY * SH);
        c.touch(1, TouchPhase::Down, px + 10.0, py + 5.0, SW, SH);
        assert_eq!(c.tick(), 0);
    }

    #[test]
    fn start_button_and_multitouch() {
        let mut c = TouchController::new();
        let (px, py) = (PAD_CX * SW, PAD_CY * SH);
        let (xx, xy) = btn_xy(X);
        c.touch(1, TouchPhase::Down, px + 80.0, py, SW, SH); // pad
        c.touch(2, TouchPhase::Down, xx, xy, SW, SH); // button
        c.touch(3, TouchPhase::Down, START_CX * SW, START_CY * SH, SW, SH); // start
        assert_eq!(c.tick(), PR | X | S);
    }

    #[test]
    fn miss_does_nothing() {
        let mut c = TouchController::new();
        let (mx, my) = hit_miss_screen();
        c.touch(1, TouchPhase::Down, mx, my, SW, SH);
        assert_eq!(c.tick(), 0);
        c.touch(1, TouchPhase::Up, mx, my, SW, SH);
        assert_eq!(c.tick(), 0);
    }

    #[test]
    fn edges() {
        let mut c = TouchController::new();
        let (xx, xy) = btn_xy(X);
        c.touch(1, TouchPhase::Down, xx, xy, SW, SH);
        c.tick();
        assert_eq!(c.pressed(), X);
        assert_eq!(c.released(), 0);
        c.touch(1, TouchPhase::Up, xx, xy, SW, SH);
        c.tick();
        assert_eq!(c.pressed(), 0);
        assert_eq!(c.released(), X);
    }

    #[test]
    fn zones_layout_stays_inside_screen() {
        let c = TouchController::new();
        for z in c.zones(SW, SH) {
            assert!(z.cx - z.r >= -1.0 && z.cx + z.r <= SW + 1.0, "zone out of X bounds");
            assert!(z.cy - z.r >= -1.0 && z.cy + z.r <= SH + 1.0, "zone out of Y bounds");
        }
        assert_eq!(c.zones(SW, SH).len(), 6); // pad + 4 buttons + start
    }

    #[test]
    fn describe_lists_held() {
        let mut c = TouchController::new();
        let (xx, xy) = btn_xy(X);
        let (yx, yy) = btn_xy(Y);
        c.touch(1, TouchPhase::Down, xx, xy, SW, SH);
        c.touch(2, TouchPhase::Down, yx, yy, SW, SH);
        c.tick();
        assert_eq!(c.describe(), "X Y Z");
    }
}
