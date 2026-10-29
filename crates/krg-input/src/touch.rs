//! The Kurogane touch controller.
//!
//! # Design — 6 buttons + pad + START, every MUGEN signal reachable
//!
//! MUGEN/Ikemen content can use up to 14 input signals (input.go:358).
//! Kurogane ships **six** physical attack buttons in the classic
//! 3×2 grid (punches on top, kicks below) and synthesizes the Ikemen
//! extras as **chords** (both signals are emitted simultaneously, so
//! `QCF+Z` and `X+Y`-style commands both fire):
//!
//! | Physical | Emits |
//! |---|---|
//! | X / Y / Z (punch row) | X, Y, Z directly |
//! | A / B / C (kick row)  | A, B, C directly |
//! | Any two of X, Y, Z together | + the missing punch |
//! | Any two of A, B, C together | + the missing kick |
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
//! # Layout ownership & customization
//!
//! Zone geometry lives HERE (Rust) in normalized screen units; the UI layer
//! queries [`TouchController::zones`] and only renders. One source of truth.
//! The whole layout is user-editable at runtime (drag to move, pinch to
//! resize via the `edit_*` API) and persists as a tiny line-based text
//! file ([`Layout::to_text`] / [`Layout::from_text`]) — no JSON dependency
//! on the hot path.
//!
//! Coordinates passed to [`TouchController::touch`] and
//! [`TouchController::edit_move`] are view pixels together with the view
//! size; the controller normalizes internally (centers as fractions of W/H,
//! radii of the shorter side), so a saved layout survives rotation and
//! device changes.

use crate::bits;
use std::collections::HashMap;
use std::path::Path;

/// Touch lifecycle phase (matches Android action codes: 0 down, 1 move, 2 up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

/// Which control a [`Placement`] describes (wire value = `ZoneInfo.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneKind {
    /// The 8-way direction pad.
    Pad = 0,
    /// An attack button.
    Btn = 1,
    /// The START button.
    Start = 2,
}

impl ZoneKind {
    /// Map a wire `kind` + button `param` pair (as carried by [`ZoneInfo`])
    /// to the canonical `(kind, bit)` zone identity.
    pub fn from_wire(kind: i32, param: i32) -> Option<(ZoneKind, u32)> {
        match kind {
            0 => Some((ZoneKind::Pad, 0)),
            1 if (0..=5).contains(&param) => Some((ZoneKind::Btn, 1 << (param + 4))),
            2 => Some((ZoneKind::Start, bits::S)),
            _ => None,
        }
    }
}

/// Renderable zone descriptor (pixel space).
#[derive(Debug, Clone, Copy)]
pub struct ZoneInfo {
    /// 0 = d-pad, 1 = button, 2 = start.
    pub kind: i32,
    /// Button index for kind 1: 0=A, 1=B, 2=C, 3=X, 4=Y, 5=Z (bit position - 4).
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

// ── Default layout (normalized: centers as fractions of W/H, radii of the
//    shorter side) — tuned for landscape phones. ─────────────────────────────

/// Pad: bottom-left thumb rest.
const DEFAULT_PAD: (f32, f32, f32) = (0.135, 0.60, 0.115);
/// START: top-right corner.
const DEFAULT_START: (f32, f32, f32) = (0.945, 0.085, 0.028);
/// Attack button radius (slightly smaller than the old 4-button 0.070 so
/// the 3×2 grid fits the same corner with margin).
const DEFAULT_BTN_R: f32 = 0.062;
/// (button bit, cx, cy) — punches top row, kicks bottom row (bit order in
/// the grid: X Y Z / A B C, left to right).
const DEFAULT_BTNS: [(u32, f32, f32); 6] = [
    (bits::X, 0.700, 0.44),
    (bits::Y, 0.815, 0.44),
    (bits::Z, 0.930, 0.44),
    (bits::A, 0.700, 0.66),
    (bits::B, 0.815, 0.66),
    (bits::C, 0.930, 0.66),
];

/// Hit-test generosity: buttons are easier to hit than they look.
const BTN_HIT_SCALE: f32 = 1.35;
/// Pad direction ENGAGE threshold as a fraction of the pad radius
/// (component test). Same value as the classic deadzone: a finger beyond
/// 40% of the ring turns the direction on.
const PAD_DEADZONE: f32 = 0.40;
/// Pad direction RELEASE threshold — the hysteresis floor. A direction,
/// once engaged, only drops when the finger falls back under 20% of the
/// ring. Without this band, finger tremor at the deadzone boundary flips
/// directions dozens of times per second — each flip is a fresh press edge
/// in the MUGEN command buffer, so accidental `F,F` runs, `B,B` back-hops
/// (the "character goes up"), jump-starts from up-flicker and constant
/// crouch(10)↔stand(12) churn (the "janky laggy crouch"). A Schmitt
/// trigger is what a real D-pad's rubber dome does mechanically.
const PAD_RELEASE: f32 = 0.20;
/// Pad capture generosity (hit-test radius as a multiple of the drawn
/// ring). Users pushing a "joystick" naturally land the first touch on
/// the far edge of the visible ring — at 1.10 the drag started dead and
/// the character never moved ("can't move by dragging, tapping works").
const PAD_HIT_SCALE: f32 = 1.40;

/// User-editable radius bounds (normalized against the shorter side).
pub const RADIUS_MIN: f32 = 0.025;
pub const RADIUS_MAX: f32 = 0.20;
/// User-editable center bounds (normalized; buttons may hug screen edges).
const CENTER_MIN: f32 = 0.02;
const CENTER_MAX: f32 = 0.98;

/// One control's geometry + identity. `bit` is the canonical zone id:
/// 0 for the pad, the button bit for buttons, `bits::S` for START.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub kind: ZoneKind,
    pub bit: u32,
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
}

impl Placement {
    /// Canonical name used by the persistence format ("pad", "A".."Z", "start").
    fn key(&self) -> String {
        match self.kind {
            ZoneKind::Pad => "pad".to_string(),
            ZoneKind::Start => "start".to_string(),
            ZoneKind::Btn => bits::name(self.bit).to_string(),
        }
    }
}

/// The full controller layout: pad + 6 attack buttons + START.
#[derive(Debug, Clone)]
pub struct Layout {
    pub items: Vec<Placement>,
}

impl Default for Layout {
    fn default() -> Self {
        Self::default_layout()
    }
}

impl Layout {
    /// The factory layout: 8 placements in a stable order
    /// (pad, X, Y, Z, A, B, C, start).
    pub fn default_layout() -> Self {
        let mut items = Vec::with_capacity(8);
        items.push(Placement {
            kind: ZoneKind::Pad,
            bit: 0,
            cx: DEFAULT_PAD.0,
            cy: DEFAULT_PAD.1,
            r: DEFAULT_PAD.2,
        });
        for &(bit, cx, cy) in DEFAULT_BTNS.iter() {
            items.push(Placement {
                kind: ZoneKind::Btn,
                bit,
                cx,
                cy,
                r: DEFAULT_BTN_R,
            });
        }
        items.push(Placement {
            kind: ZoneKind::Start,
            bit: bits::S,
            cx: DEFAULT_START.0,
            cy: DEFAULT_START.1,
            r: DEFAULT_START.2,
        });
        Layout { items }
    }

    /// Find a placement by its wire identity (kind + button param), if any.
    pub fn find(&self, kind: ZoneKind, bit: u32) -> Option<&Placement> {
        self.items.iter().find(|p| p.kind == kind && p.bit == bit)
    }

    fn find_mut(&mut self, kind: ZoneKind, bit: u32) -> Option<&mut Placement> {
        self.items
            .iter_mut()
            .find(|p| p.kind == kind && p.bit == bit)
    }

    /// Serialize as a tiny line-based text file:
    ///
    /// ```text
    /// # kurogane controller layout v1
    /// # centers: fractions of view width/height; radius: fraction of shorter side
    /// pad 0.13500 0.60000 0.11500
    /// btn X 0.70000 0.44000 0.06200
    /// start 0.94500 0.08500 0.02800
    /// ```
    ///
    /// Buttons appear as `btn <letter> cx cy r` (letter = A|B|C|X|Y|Z).
    pub fn to_text(&self) -> String {
        let mut s = String::from(
            "# kurogane controller layout v1\n\
             # centers: fractions of view width/height; radius: fraction of shorter side\n",
        );
        for p in &self.items {
            let key = match p.kind {
                ZoneKind::Btn => format!("btn {}", p.key()),
                _ => p.key(),
            };
            s.push_str(&format!("{key} {:.5} {:.5} {:.5}\n", p.cx, p.cy, p.r));
        }
        s
    }

    /// Parse a layout file. Tolerant: starts from the defaults and overrides
    /// only the lines it understands, so a partial, hand-edited or future
    /// file degrades gracefully instead of bricking the controller.
    pub fn from_text(text: &str) -> Layout {
        let mut layout = Layout::default_layout();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let t: Vec<&str> = line.split_whitespace().collect();
            let parsed = match t.as_slice() {
                ["pad", cx, cy, r] => zone_from_parts(ZoneKind::Pad, 0, cx, cy, r),
                ["start", cx, cy, r] => zone_from_parts(ZoneKind::Start, bits::S, cx, cy, r),
                ["btn", name, cx, cy, r] => {
                    let bit = match *name {
                        "A" => bits::A,
                        "B" => bits::B,
                        "C" => bits::C,
                        "X" => bits::X,
                        "Y" => bits::Y,
                        "Z" => bits::Z,
                        _ => continue,
                    };
                    zone_from_parts(ZoneKind::Btn, bit, cx, cy, r)
                }
                _ => continue,
            };
            let Some((kind, bit, cx, cy, r)) = parsed else {
                continue;
            };
            if let Some(p) = layout.find_mut(kind, bit) {
                p.cx = cx;
                p.cy = cy;
                p.r = r;
            }
        }
        layout
    }
}

/// Parse + clamp one placement's numbers; None on any malformed part.
fn zone_from_parts(
    kind: ZoneKind,
    bit: u32,
    cx: &str,
    cy: &str,
    r: &str,
) -> Option<(ZoneKind, u32, f32, f32, f32)> {
    let (cx, cy, r): (f32, f32, f32) = (
        cx.parse::<f32>().ok()?,
        cy.parse::<f32>().ok()?,
        r.parse::<f32>().ok()?,
    );
    if !cx.is_finite() || !cy.is_finite() || !r.is_finite() {
        return None;
    }
    Some((
        kind,
        bit,
        cx.clamp(CENTER_MIN, CENTER_MAX),
        cy.clamp(CENTER_MIN, CENTER_MAX),
        r.clamp(RADIUS_MIN, RADIUS_MAX),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cap {
    None,
    Pad,
    Btn(u32),
    Start,
}

/// The controller. Feed touches in, call [`TouchController::tick`] once per
/// frame (60 fps), read the emitted [`bits`](crate::bits).
///
/// Editing is non-destructive: [`edit_begin`] snapshots the layout so the UI
/// can offer a CANCEL that restores exactly what was there, while
/// [`save_layout`] persists the current state to disk.
#[derive(Debug, Default)]
pub struct TouchController {
    /// Pointer id → captured zone (captured on down; kept until up).
    pointers: HashMap<u64, Cap>,
    /// Physical direction bits currently held (from the pad).
    raw_dirs: u32,
    /// Physical button bits currently held (A/B/C/X/Y/Z/S).
    raw_btns: u32,
    /// Last frame's emitted bits (after chord synthesis).
    held: u32,
    /// Frame before last (for edge detection).
    prev: u32,
    /// The live, user-editable layout.
    layout: Layout,
    /// Layout snapshot taken at [`edit_begin`] (None outside an edit session).
    snapshot: Option<Layout>,
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

    /// Drop every pointer and clear every held signal (edit-mode entry or
    /// app focus loss — never leave a button stuck down).
    pub fn release_all(&mut self) {
        self.pointers.clear();
        self.raw_dirs = 0;
        self.raw_btns = 0;
    }

    /// Advance one frame: synthesize chords, update edges, return held bits.
    pub fn tick(&mut self) -> u32 {
        // Chords derive from the *physical* bits only, so a synthesized
        // button never feeds another chord (the X synthesized from Y+Z
        // cannot fire the A+X→D chord, for instance).
        let raw = self.raw_btns;
        let mut out = raw;
        // Any two of a row → the missing one joins the pair: X+Y→+Z,
        // Y+Z→+X, Z+X→+Y (and likewise for kicks), so every two-button
        // shortcut works whichever pair the fingers happen to land on.
        out |= synth_triple(raw, bits::X, bits::Y, bits::Z);
        out |= synth_triple(raw, bits::A, bits::B, bits::C);
        if raw & bits::A != 0 && raw & bits::X != 0 {
            out |= bits::D;
        }
        if raw & bits::B != 0 && raw & bits::Y != 0 {
            out |= bits::W;
        }
        if raw & (bits::A | bits::B | bits::X | bits::Y) == (bits::A | bits::B | bits::X | bits::Y)
        {
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
        let mut v = Vec::with_capacity(self.layout.items.len());
        for p in &self.layout.items {
            v.push(ZoneInfo {
                kind: p.kind as i32,
                param: match p.kind {
                    ZoneKind::Btn => (p.bit.trailing_zeros() as i32) - 4, // A=0..Z=5
                    _ => 0,
                },
                cx: p.cx * w,
                cy: p.cy * h,
                r: p.r * m,
                held: match p.kind {
                    ZoneKind::Pad => self.raw_dirs != 0,
                    ZoneKind::Btn => self.raw_btns & p.bit != 0,
                    ZoneKind::Start => self.raw_btns & p.bit != 0,
                },
                ex: if p.kind == ZoneKind::Pad {
                    dir_x(self.raw_dirs)
                } else {
                    0.0
                },
                ey: if p.kind == ZoneKind::Pad {
                    dir_y(self.raw_dirs)
                } else {
                    0.0
                },
            });
        }
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

    // ── layout editing ──────────────────────────────────────────────────────

    /// The live layout (copy).
    pub fn layout(&self) -> Layout {
        self.layout.clone()
    }

    /// Replace the live layout wholesale (e.g. restore from settings).
    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
        self.release_all();
    }

    /// Begin an edit session: snapshot the layout and drop all held inputs
    /// so a long-press that opens the editor can't leave a button stuck.
    pub fn edit_begin(&mut self) {
        self.snapshot = Some(self.layout.clone());
        self.release_all();
    }

    /// Move the control `(kind, bit)` so its center sits under pixel
    /// `(x, y)` of a `w`×`h` view (UI layers may pass finger + grab-offset).
    /// Unknown ids are ignored; centers are clamped on-screen.
    pub fn edit_move(&mut self, kind: ZoneKind, bit: u32, x: f32, y: f32, w: f32, h: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        if let Some(p) = self.layout.find_mut(kind, bit) {
            p.cx = (x / w).clamp(CENTER_MIN, CENTER_MAX);
            p.cy = (y / h).clamp(CENTER_MIN, CENTER_MAX);
        }
    }

    /// Scale the control `(kind, bit)`'s radius by `factor` (clamped to
    /// [`RADIUS_MIN`]..[`RADIUS_MAX`]). Unknown ids are ignored.
    pub fn edit_scale(&mut self, kind: ZoneKind, bit: u32, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        if let Some(p) = self.layout.find_mut(kind, bit) {
            p.r = (p.r * factor).clamp(RADIUS_MIN, RADIUS_MAX);
        }
    }

    /// Abandon the edit session: restore the [`edit_begin`] snapshot.
    /// A no-op when no session is open.
    pub fn edit_revert(&mut self) {
        if let Some(s) = self.snapshot.take() {
            self.layout = s;
        }
        self.release_all();
    }

    /// Reset the live layout to factory defaults (stays inside the edit
    /// session — the pre-edit snapshot still allows CANCEL to restore).
    pub fn reset_layout(&mut self) {
        self.layout = Layout::default_layout();
    }

    /// Persist the live layout to `path` and close the edit session.
    /// Returns false when the write fails (session stays open).
    pub fn save_layout(&mut self, path: &Path) -> bool {
        let text = self.layout.to_text();
        match std::fs::write(path, text) {
            Ok(()) => {
                self.snapshot = None;
                self.release_all();
                true
            }
            Err(_) => false,
        }
    }

    /// Load a persisted layout from `path`. On any problem (missing file,
    /// partial contents) the current layout is kept — callers typically run
    /// this once at startup, where "keep defaults" is the right fallback.
    /// Returns true when the file was read and parsed.
    pub fn load_layout(&mut self, path: &Path) -> bool {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.layout = Layout::from_text(&text);
                true
            }
            Err(_) => false,
        }
    }

    // ── internals ──────────────────────────────────────────────────────────

    /// Hit test. Buttons sit on top of everything (nearest center wins
    /// between overlapping hit circles — matters once the user drags
    /// buttons close together), then START, then the pad.
    fn hit(&self, x: f32, y: f32, w: f32, h: f32) -> Cap {
        let m = w.min(h);
        let mut best: Option<(f32, u32)> = None;
        for p in self.layout.items.iter().filter(|p| p.kind == ZoneKind::Btn) {
            let d2 = dist2(x, y, p.cx * w, p.cy * h);
            let hr = p.r * m * BTN_HIT_SCALE;
            if d2 < hr * hr && best.is_none_or(|(bd, _)| d2 < bd) {
                best = Some((d2, p.bit));
            }
        }
        if let Some((_, b)) = best {
            return Cap::Btn(b);
        }
        if let Some(p) = self.layout.find(ZoneKind::Start, bits::S) {
            let hr = p.r * m * BTN_HIT_SCALE;
            if dist2(x, y, p.cx * w, p.cy * h) < hr * hr {
                return Cap::Start;
            }
        }
        if let Some(p) = self.layout.find(ZoneKind::Pad, 0) {
            let hr = p.r * m * PAD_HIT_SCALE;
            if dist2(x, y, p.cx * w, p.cy * h) < hr * hr {
                return Cap::Pad;
            }
        }
        Cap::None
    }

    fn apply(&mut self, cap: Cap, x: f32, y: f32, w: f32, h: f32) {
        match cap {
            Cap::None => {}
            Cap::Pad => {
                let (cx, cy, r) = match self.layout.find(ZoneKind::Pad, 0) {
                    Some(p) => (p.cx * w, p.cy * h, p.r * w.min(h)),
                    None => return,
                };
                let (dx, dy) = (x - cx, y - cy);
                // Schmitt trigger per axis: engage beyond PAD_DEADZONE*r,
                // release only under PAD_RELEASE*r. Seeding from the
                // current bits keeps a direction alive through the hold
                // band — see PAD_RELEASE for why this matters so much.
                let engage = PAD_DEADZONE * r;
                let release = PAD_RELEASE * r;
                let mut d = self.raw_dirs;
                // x axis: PL ↔ PR
                match d & (bits::PL | bits::PR) {
                    0 => {
                        if dx > engage {
                            d |= bits::PR;
                        } else if dx < -engage {
                            d |= bits::PL;
                        }
                    }
                    b if b == bits::PR => {
                        if dx < -engage {
                            d = (d & !bits::PR) | bits::PL; // hard side switch
                        } else if dx < release {
                            d &= !bits::PR;
                        }
                    }
                    _ => {
                        // PL held
                        if dx > engage {
                            d = (d & !bits::PL) | bits::PR;
                        } else if dx > -release {
                            d &= !bits::PL;
                        }
                    }
                }
                // y axis: PU ↔ PD (screen y grows downward)
                match d & (bits::PU | bits::PD) {
                    0 => {
                        if dy < -engage {
                            d |= bits::PU;
                        } else if dy > engage {
                            d |= bits::PD;
                        }
                    }
                    b if b == bits::PU => {
                        if dy > engage {
                            d = (d & !bits::PU) | bits::PD;
                        } else if dy > -release {
                            d &= !bits::PU;
                        }
                    }
                    _ => {
                        // PD held
                        if dy < -engage {
                            d = (d & !bits::PD) | bits::PU;
                        } else if dy < release {
                            d &= !bits::PD;
                        }
                    }
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

/// Unique temp-file counter for layout round-trip tests (host-side only).
#[cfg(test)]
use std::sync::atomic::AtomicU32;
#[cfg(test)]
static TMP_N: AtomicU32 = AtomicU32::new(0);

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

/// Two of a triple held → the remaining one joins the emission (the pressed
/// pair stays on, so `X+Y`-style commands still fire while the chord also
/// yields the third signal: `X+Y`→+Z, `Y+Z`→+X, `Z+X`→+Y).
fn synth_triple(held: u32, a: u32, b: u32, c: u32) -> u32 {
    let n = (held & a != 0) as u32 + (held & b != 0) as u32 + (held & c != 0) as u32;
    if n >= 2 {
        a | b | c
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::*;
    use std::sync::atomic::Ordering;

    // Reference screen: 1600×900 landscape.
    const SW: f32 = 1600.0;
    const SH: f32 = 900.0;

    fn btn_xy(c: &TouchController, b: u32) -> (f32, f32) {
        let z = c
            .zones(SW, SH)
            .into_iter()
            .find(|z| z.kind == 1 && z.param == (b.trailing_zeros() as i32) - 4)
            .unwrap();
        (z.cx, z.cy)
    }

    fn hit_miss_screen() -> (f32, f32) {
        (SW / 2.0, SH / 2.0)
    }

    #[test]
    fn tap_x_emits_only_x() {
        let mut c = TouchController::new();
        let (x, y) = btn_xy(&c, X);
        c.touch(1, TouchPhase::Down, x, y, SW, SH);
        assert_eq!(c.tick(), X);
    }

    #[test]
    fn tap_c_and_z_direct() {
        let mut c = TouchController::new();
        let (cx, cy) = btn_xy(&c, C);
        c.touch(1, TouchPhase::Down, cx, cy, SW, SH);
        assert_eq!(c.tick(), C);
        let (zx, zy) = btn_xy(&c, Z);
        c.touch(2, TouchPhase::Down, zx, zy, SW, SH);
        // C alone adds nothing; Z alone adds nothing.
        assert_eq!(c.tick(), C | Z);
    }

    #[test]
    fn chord_xy_emits_z() {
        let mut c = TouchController::new();
        let (xx, xy) = btn_xy(&c, X);
        let (yx, yy) = btn_xy(&c, Y);
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
        let (ax, ay) = btn_xy(&c, A);
        let (bx, by) = btn_xy(&c, B);
        c.touch(1, TouchPhase::Down, ax, ay, SW, SH);
        c.touch(2, TouchPhase::Down, bx, by, SW, SH);
        assert_eq!(c.tick(), A | B | C);
    }

    #[test]
    fn chord_yz_emits_x() {
        let mut c = TouchController::new();
        let (yx, yy) = btn_xy(&c, Y);
        let (zx, zy) = btn_xy(&c, Z);
        c.touch(1, TouchPhase::Down, yx, yy, SW, SH);
        c.touch(2, TouchPhase::Down, zx, zy, SW, SH);
        // The missing punch of any held pair joins: Y+Z → +X.
        assert_eq!(c.tick(), X | Y | Z);
        c.touch(2, TouchPhase::Up, zx, zy, SW, SH);
        assert_eq!(c.tick(), Y);
    }

    #[test]
    fn chord_xz_emits_y_and_bc_emits_a() {
        let mut c = TouchController::new();
        let (xx, xy) = btn_xy(&c, X);
        let (zx, zy) = btn_xy(&c, Z);
        c.touch(1, TouchPhase::Down, xx, xy, SW, SH);
        c.touch(2, TouchPhase::Down, zx, zy, SW, SH);
        assert_eq!(c.tick(), X | Y | Z);
        c.touch(1, TouchPhase::Up, xx, xy, SW, SH);
        c.touch(2, TouchPhase::Up, zx, zy, SW, SH);
        let (bx, by) = btn_xy(&c, B);
        let (cx, cy) = btn_xy(&c, C);
        c.touch(3, TouchPhase::Down, bx, by, SW, SH);
        c.touch(4, TouchPhase::Down, cx, cy, SW, SH);
        assert_eq!(c.tick(), A | B | C);
    }

    #[test]
    fn synthesized_bit_does_not_cascade() {
        let mut c = TouchController::new();
        let (ax, ay) = btn_xy(&c, A);
        let (yx, yy) = btn_xy(&c, Y);
        let (zx, zy) = btn_xy(&c, Z);
        c.touch(1, TouchPhase::Down, ax, ay, SW, SH);
        c.touch(2, TouchPhase::Down, yx, yy, SW, SH);
        c.touch(3, TouchPhase::Down, zx, zy, SW, SH);
        // Y+Z synthesizes X, but chords read physical bits only — D needs
        // a real A+X press, so it must NOT fire here.
        assert_eq!(c.tick(), A | X | Y | Z);
    }

    #[test]
    fn chords_ax_d_and_by_w() {
        let mut c = TouchController::new();
        let (ax, ay) = btn_xy(&c, A);
        let (xx, xy) = btn_xy(&c, X);
        c.touch(1, TouchPhase::Down, ax, ay, SW, SH);
        c.touch(2, TouchPhase::Down, xx, xy, SW, SH);
        // A+X → D (and no Z: Y not held; no C: B not held).
        assert_eq!(c.tick(), A | X | D);
        let (bx, by) = btn_xy(&c, B);
        let (yx, yy) = btn_xy(&c, Y);
        c.touch(3, TouchPhase::Down, bx, by, SW, SH);
        c.touch(4, TouchPhase::Down, yx, yy, SW, SH);
        // All four: every chord fires.
        assert_eq!(c.tick(), A | B | X | Y | Z | C | D | W | M);
    }

    #[test]
    fn pad_right_and_diagonal() {
        let mut c = TouchController::new();
        let (px, py) = (DEFAULT_PAD.0 * SW, DEFAULT_PAD.1 * SH);
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
        let (px, py) = (DEFAULT_PAD.0 * SW, DEFAULT_PAD.1 * SH);
        c.touch(1, TouchPhase::Down, px + 10.0, py + 5.0, SW, SH);
        assert_eq!(c.tick(), 0);
    }

    // ── hysteresis + capture (the v0.9.3 device bugs) ──────────────────────

    /// Pad ring radius on the reference screen.
    fn pad_r() -> f32 {
        DEFAULT_PAD.2 * SW.min(SH)
    }

    /// Finger tremor around the engage boundary must NOT flip the direction:
    /// once engaged, it survives the whole hold band (0.20r..0.40r). Every
    /// flip would be a fresh press edge in the MUGEN command buffer — the
    /// accidental F,F runs / B,B hops / crouch churn on the device.
    #[test]
    fn pad_hysteresis_hold_band() {
        let mut c = TouchController::new();
        let (px, py) = (DEFAULT_PAD.0 * SW, DEFAULT_PAD.1 * SH);
        let r = pad_r();
        // engage right…
        c.touch(1, TouchPhase::Down, px + 0.60 * r, py, SW, SH);
        assert_eq!(c.tick(), PR);
        // …then tremble between 0.22r and 0.38r for a long while
        for i in 0..200 {
            let wobble = 0.30 + 0.08 * ((i % 5) as f32 - 2.0) / 2.0;
            c.touch(1, TouchPhase::Move, px + wobble * r, py, SW, SH);
            assert_eq!(c.tick(), PR, "PR dropped at wobble {wobble}r (i={i})");
        }
        // …and only releasing under 0.20r drops it
        c.touch(1, TouchPhase::Move, px + 0.15 * r, py, SW, SH);
        assert_eq!(c.tick(), 0);
    }

    /// A fresh press inside the hold band engages nothing (no stale state).
    #[test]
    fn pad_hysteresis_cold_press_in_band() {
        let mut c = TouchController::new();
        let (px, py) = (DEFAULT_PAD.0 * SW, DEFAULT_PAD.1 * SH);
        let r = pad_r();
        c.touch(1, TouchPhase::Down, px + 0.30 * r, py, SW, SH);
        assert_eq!(c.tick(), 0);
        // crossing the release line downward from nothing still nothing
        c.touch(1, TouchPhase::Move, px + 0.10 * r, py, SW, SH);
        assert_eq!(c.tick(), 0);
    }

    /// Fast side switches pass through in ONE event (no dead gap that would
    /// eat quarter-circle motions).
    #[test]
    fn pad_hard_side_switch() {
        let mut c = TouchController::new();
        let (px, py) = (DEFAULT_PAD.0 * SW, DEFAULT_PAD.1 * SH);
        let r = pad_r();
        c.touch(1, TouchPhase::Down, px + 0.80 * r, py, SW, SH);
        assert_eq!(c.tick(), PR);
        // sweep straight across to the far side
        c.touch(1, TouchPhase::Move, px - 0.80 * r, py, SW, SH);
        assert_eq!(c.tick(), PL);
        // and back
        c.touch(1, TouchPhase::Move, px + 0.80 * r, py, SW, SH);
        assert_eq!(c.tick(), PR);
    }

    /// Diagonals survive per-axis hysteresis: DF stays DF while the finger
    /// wobbles on one axis inside the hold band.
    #[test]
    fn pad_diagonal_hysteresis() {
        let mut c = TouchController::new();
        let (px, py) = (DEFAULT_PAD.0 * SW, DEFAULT_PAD.1 * SH);
        let r = pad_r();
        c.touch(1, TouchPhase::Down, px + 0.70 * r, py + 0.70 * r, SW, SH);
        assert_eq!(c.tick(), PR | PD);
        // x trembles in the hold band, y solid → still DF
        for i in 0..50 {
            let wobble = 0.30 + 0.06 * (i % 3) as f32 / 3.0;
            c.touch(1, TouchPhase::Move, px + wobble * r, py + 0.70 * r, SW, SH);
            assert_eq!(c.tick(), PR | PD, "DF lost at x wobble {wobble}r");
        }
        // x collapses under the release line → plain D
        c.touch(1, TouchPhase::Move, px + 0.10 * r, py + 0.70 * r, SW, SH);
        assert_eq!(c.tick(), PD);
    }

    /// A drag that STARTS on the visible ring's edge (or a touch past it)
    /// must capture the pad — the old 1.10× capture circle left such drags
    /// dead ("can't move by dragging" on the device).
    #[test]
    fn pad_edge_start_drag_captures() {
        let mut c = TouchController::new();
        let (px, py) = (DEFAULT_PAD.0 * SW, DEFAULT_PAD.1 * SH);
        let r = pad_r();
        // down at 1.25r right of center — outside the drawn ring
        c.touch(1, TouchPhase::Down, px + 1.25 * r, py, SW, SH);
        assert_eq!(c.tick(), PR, "edge-start drag is a dead touch");
        // keep dragging further out — still captured (pointer owns the pad)
        c.touch(1, TouchPhase::Move, px + 2.5 * r, py, SW, SH);
        assert_eq!(c.tick(), PR);
        // down at 1.39r (just inside the capture circle) — also captures
        let mut c2 = TouchController::new();
        c2.touch(1, TouchPhase::Down, px + 1.39 * r, py, SW, SH);
        assert_eq!(c2.tick(), PR);
        // a touch clearly beyond the capture generosity still misses
        let mut c3 = TouchController::new();
        c3.touch(1, TouchPhase::Down, px + 2.0 * r, py, SW, SH);
        assert_eq!(c3.tick(), 0);
    }

    /// Finger up clears every direction instantly (no hysteresis lag on
    /// release — MUGEN hold commands must drop the moment the pad lets go).
    #[test]
    fn pad_up_clears_instantly() {
        let mut c = TouchController::new();
        let (px, py) = (DEFAULT_PAD.0 * SW, DEFAULT_PAD.1 * SH);
        let r = pad_r();
        c.touch(1, TouchPhase::Down, px + 0.70 * r, py + 0.70 * r, SW, SH);
        assert_eq!(c.tick(), PR | PD);
        // lift while still deep in the hold zone
        c.touch(1, TouchPhase::Up, px + 0.70 * r, py + 0.70 * r, SW, SH);
        assert_eq!(c.tick(), 0);
        // re-press in the hold band: nothing stale survives the lift
        c.touch(1, TouchPhase::Down, px + 0.30 * r, py + 0.30 * r, SW, SH);
        assert_eq!(c.tick(), 0);
    }

    #[test]
    fn start_button_and_multitouch() {
        let mut c = TouchController::new();
        let (px, py) = (DEFAULT_PAD.0 * SW, DEFAULT_PAD.1 * SH);
        let (xx, xy) = btn_xy(&c, X);
        c.touch(1, TouchPhase::Down, px + 80.0, py, SW, SH); // pad
        c.touch(2, TouchPhase::Down, xx, xy, SW, SH); // button
        c.touch(
            3,
            TouchPhase::Down,
            DEFAULT_START.0 * SW,
            DEFAULT_START.1 * SH,
            SW,
            SH,
        ); // start
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
        let (xx, xy) = btn_xy(&c, X);
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
            assert!(
                z.cx - z.r >= -1.0 && z.cx + z.r <= SW + 1.0,
                "zone out of X bounds"
            );
            assert!(
                z.cy - z.r >= -1.0 && z.cy + z.r <= SH + 1.0,
                "zone out of Y bounds"
            );
        }
        // pad + 6 buttons + start
        assert_eq!(c.zones(SW, SH).len(), 8);
    }

    #[test]
    fn default_layout_is_six_button_grid() {
        let c = TouchController::new();
        let btns: Vec<ZoneInfo> = c
            .zones(SW, SH)
            .into_iter()
            .filter(|z| z.kind == 1)
            .collect();
        let names: Vec<i32> = {
            let mut v: Vec<i32> = btns.iter().map(|z| z.param).collect();
            v.sort();
            v
        };
        // A=0 B=1 C=2 X=3 Y=4 Z=5 — all six present, punches above kicks.
        assert_eq!(names, vec![0, 1, 2, 3, 4, 5]);
        let row = |b: u32| {
            btns.iter()
                .find(|z| z.param == (b.trailing_zeros() as i32) - 4)
                .unwrap()
                .cy
        };
        assert!(row(X) < row(A), "punch row sits above kick row");
    }

    #[test]
    fn describe_lists_held() {
        let mut c = TouchController::new();
        let (xx, xy) = btn_xy(&c, X);
        let (yx, yy) = btn_xy(&c, Y);
        c.touch(1, TouchPhase::Down, xx, xy, SW, SH);
        c.touch(2, TouchPhase::Down, yx, yy, SW, SH);
        c.tick();
        assert_eq!(c.describe(), "X Y Z");
    }

    // ── editing ────────────────────────────────────────────────────────────

    #[test]
    fn edit_begin_releases_holds() {
        let mut c = TouchController::new();
        let (xx, xy) = btn_xy(&c, X);
        c.touch(1, TouchPhase::Down, xx, xy, SW, SH);
        assert_eq!(c.tick(), X);
        c.edit_begin();
        assert_eq!(c.tick(), 0, "edit entry must not leave buttons stuck");
    }

    #[test]
    fn edit_move_relocates_and_rehits() {
        let mut c = TouchController::new();
        c.edit_begin();
        // Move X to dead-center.
        c.edit_move(ZoneKind::Btn, X, SW / 2.0, SH / 2.0, SW, SH);
        let (xx, xy) = btn_xy(&c, X);
        assert!((xx - SW / 2.0).abs() < 0.5 && (xy - SH / 2.0).abs() < 0.5);
        c.touch(1, TouchPhase::Down, SW / 2.0, SH / 2.0, SW, SH);
        assert_eq!(c.tick(), X, "hit-testing must follow the edited layout");
    }

    #[test]
    fn edit_move_clamps_on_screen() {
        let mut c = TouchController::new();
        c.edit_begin();
        c.edit_move(ZoneKind::Btn, X, -9999.0, 9999.0, SW, SH);
        let (xx, xy) = btn_xy(&c, X);
        assert!(xx > 0.0 && xx < SW && xy > 0.0 && xy < SH);
    }

    #[test]
    fn edit_scale_clamps() {
        let mut c = TouchController::new();
        c.edit_begin();
        c.edit_scale(ZoneKind::Btn, X, 1000.0);
        let r = c
            .zones(SW, SH)
            .into_iter()
            .find(|z| z.kind == 1 && z.param == 3)
            .unwrap()
            .r;
        assert!((r - RADIUS_MAX * SH).abs() < 0.01);
        c.edit_scale(ZoneKind::Btn, X, 0.0001);
        let r2 = c
            .zones(SW, SH)
            .into_iter()
            .find(|z| z.kind == 1 && z.param == 3)
            .unwrap()
            .r;
        assert!((r2 - RADIUS_MIN * SH).abs() < 0.01);
        // Junk factors are ignored, not panic.
        c.edit_scale(ZoneKind::Btn, X, f32::NAN);
        c.edit_scale(ZoneKind::Btn, X, -1.0);
    }

    #[test]
    fn edit_revert_restores_snapshot() {
        let mut c = TouchController::new();
        let (x0, y0) = btn_xy(&c, X);
        c.edit_begin();
        c.edit_move(ZoneKind::Btn, X, 100.0, 100.0, SW, SH);
        c.edit_scale(ZoneKind::Btn, X, 1.5);
        c.edit_revert();
        let (x1, y1) = btn_xy(&c, X);
        assert_eq!((x0, y0), (x1, y1), "revert must restore pre-edit geometry");
        // Reverting with no session open is a no-op.
        c.edit_revert();
        let (x2, y2) = btn_xy(&c, X);
        assert_eq!((x1, y1), (x2, y2));
    }

    #[test]
    fn reset_layout_restores_defaults_inside_session() {
        let mut c = TouchController::new();
        let (x0, y0) = btn_xy(&c, X);
        c.edit_begin();
        c.edit_move(ZoneKind::Btn, X, 100.0, 100.0, SW, SH);
        c.reset_layout();
        let (x1, y1) = btn_xy(&c, X);
        assert_eq!((x0, y0), (x1, y1));
        // CANCEL still restores the true pre-edit state.
        c.edit_revert();
        let (x2, y2) = btn_xy(&c, X);
        assert_eq!((x0, y0), (x2, y2));
    }

    #[test]
    fn nearest_button_wins_overlap() {
        let mut c = TouchController::new();
        // Drop B right next to X (heavily overlapping hit circles).
        c.edit_begin();
        c.edit_move(ZoneKind::Btn, X, 1000.0, 450.0, SW, SH);
        c.edit_move(ZoneKind::Btn, B, 1110.0, 450.0, SW, SH);
        // A tap at 1050 is within both hit circles; nearer center (X@1000) wins.
        c.touch(1, TouchPhase::Down, 1050.0, 450.0, SW, SH);
        assert_eq!(c.tick(), X);
    }

    // ── persistence ─────────────────────────────────────────────────────────

    #[test]
    fn layout_text_roundtrip() {
        let mut c = TouchController::new();
        c.edit_begin();
        c.edit_move(ZoneKind::Btn, Z, 555.0, 444.0, SW, SH);
        c.edit_scale(ZoneKind::Pad, 0, 0.8);
        let before = c.layout();
        let text = before.to_text();
        let after = Layout::from_text(&text);
        assert_eq!(before.items.len(), after.items.len());
        for (a, b) in before.items.iter().zip(after.items.iter()) {
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.bit, b.bit);
            assert!((a.cx - b.cx).abs() < 1e-4);
            assert!((a.cy - b.cy).abs() < 1e-4);
            assert!((a.r - b.r).abs() < 1e-4);
        }
    }

    #[test]
    fn layout_text_partial_and_garbage() {
        let d = Layout::default_layout();
        // Only one line + junk around it: defaults fill the gaps.
        let parsed = Layout::from_text(
            "# comment\n; also a comment\n\nbtn Z 0.11 0.22 0.05\nwhat even is this\nbtn Q 1 2 3\nbtn A nope bad r\n",
        );
        assert_eq!(parsed.items.len(), d.items.len());
        let z = parsed.find(ZoneKind::Btn, Z).unwrap();
        assert!((z.cx - 0.11).abs() < 1e-5 && (z.cy - 0.22).abs() < 1e-5);
        let a = parsed.find(ZoneKind::Btn, A).unwrap();
        let da = d.find(ZoneKind::Btn, A).unwrap();
        assert_eq!((a.cx, a.cy, a.r), (da.cx, da.cy, da.r));
    }

    #[test]
    fn layout_text_clamps_hostile_values() {
        let parsed = Layout::from_text("pad -5.0 20.0 99.0\n");
        let p = parsed.find(ZoneKind::Pad, 0).unwrap();
        assert_eq!(p.cx, CENTER_MIN);
        assert_eq!(p.cy, CENTER_MAX);
        assert_eq!(p.r, RADIUS_MAX);
    }

    #[test]
    fn layout_save_load_file_roundtrip() {
        let mut c = TouchController::new();
        c.edit_begin();
        c.edit_move(ZoneKind::Btn, C, 300.0, 500.0, SW, SH);
        c.edit_scale(ZoneKind::Start, S, 1.25);
        let path = std::env::temp_dir().join(format!(
            "kurogane-layout-{}.ini",
            TMP_N.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(c.save_layout(&path));
        let mut c2 = TouchController::new();
        assert!(c2.load_layout(&path));
        let (cx, cy) = btn_xy(&c2, C);
        assert!((cx - 300.0).abs() < 0.5 && (cy - 500.0).abs() < 0.5);
        let s = c2.zones(SW, SH).into_iter().find(|z| z.kind == 2).unwrap();
        assert!((s.r - DEFAULT_START.2 * 1.25 * SH).abs() < 0.01);
        let _ = std::fs::remove_file(&path);
        // Missing file keeps the current layout.
        assert!(!c2.load_layout(&path));
        assert!(c2.layout().items.len() == 8);
    }
}
