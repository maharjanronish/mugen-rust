//! CPU side of the sprite batcher — pure data, no GL, fully unit-tested.
//!
//! Every sprite of a frame becomes 6 vertices (two triangles) of a packed
//! 20-byte [`Vertex`] layout, appended into one reusable [`Vec`]. A **run**
//! is a maximal stretch of consecutive vertices that shares texture + blend
//! mode — the GL layer turns each run into one `glDrawArrays` call with no
//! state changes in between. Texture handles are plain `u32` here (0 = the
//! shared white texture) so this module compiles and tests on the host
//! without any GL context.
//!
//! Rotation is applied on the CPU per quad: only sprites actually drawn at an
//! angle pay the sin/cos, and the shader stays a single matrix multiply.

use std::vec::Vec;

/// Premultiplied-alpha blend (`ONE, ONE_MINUS_SRC_ALPHA`).
pub const BLENDS_ALPHA: u8 = 0;
/// Additive blend (`ONE, ONE`) — MUGEN `trans = add`.
pub const BLENDS_ADD: u8 = 1;

/// Batch capacity in quads (8192 × 120 B ≈ 1 MB of vertex ring).
pub const MAX_QUADS: usize = 8192;

/// Packed vertex: position (2×f32), uv (2×f32), straight RGBA tint (4×u8).
///
/// Color is a *straight* (not premultiplied) multiplier — the shader does the
/// one correct premultiply: `out = vec4(t.rgb * c.rgb, t.a * c.a)`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

const _: () = assert!(std::mem::size_of::<Vertex>() == 20);

/// One sprite quad. Destination rect in game-space pixels; pivot in the same
/// space (rotation center — usually the rect center).
#[derive(Debug, Clone, Copy)]
pub struct Quad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub color: [u8; 4],
    /// Rotation in radians (0 = axis-aligned fast path).
    pub angle: f32,
    pub px: f32,
    pub py: f32,
}

impl Quad {
    /// The four dst corners (tl, tr, br, bl), rotated around the pivot.
    fn corners(&self) -> [[f32; 2]; 4] {
        let raw = [
            [self.x, self.y],
            [self.x + self.w, self.y],
            [self.x + self.w, self.y + self.h],
            [self.x, self.y + self.h],
        ];
        if self.angle == 0.0 {
            return raw;
        }
        let (s, c) = self.angle.sin_cos();
        raw.map(|[x, y]| {
            let (dx, dy) = (x - self.px, y - self.py);
            [self.px + dx * c - dy * s, self.py + dx * s + dy * c]
        })
    }
}

/// A maximal stretch of vertices sharing texture + blend mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    pub tex: u32,
    pub blend: u8,
    /// Index of the run's first vertex.
    pub first: u32,
    /// Vertex count (multiple of 6).
    pub count: u32,
}

/// The frame's vertex ring + run list. Clear and refill every frame — the
/// underlying allocations are kept across frames (no per-frame heap churn).
#[derive(Debug, Default)]
pub struct Batcher {
    verts: Vec<Vertex>,
    runs: Vec<Run>,
}

impl Batcher {
    pub fn new() -> Self {
        Batcher {
            verts: Vec::with_capacity(MAX_QUADS * 6),
            runs: Vec::with_capacity(32),
        }
    }

    pub fn clear(&mut self) {
        self.verts.clear();
        self.runs.clear();
    }

    pub fn quad_count(&self) -> usize {
        self.verts.len() / 6
    }

    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    pub fn is_full(&self) -> bool {
        self.quad_count() >= MAX_QUADS
    }

    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// Append one quad. `false` (and no append) when the ring is full — the
    /// caller flushes and re-pushes.
    pub fn push(&mut self, tex: u32, blend: u8, q: &Quad) -> bool {
        if self.is_full() {
            return false;
        }
        let first = self.verts.len() as u32;
        let [tl, tr, br, bl] = q.corners();
        let uvs = [
            [q.u0, q.v0],
            [q.u1, q.v0],
            [q.u1, q.v1],
            [q.u0, q.v1],
        ];
        // Two triangles: tl, tr, br + tl, br, bl.
        for &(pos, uv) in &[(tl, uvs[0]), (tr, uvs[1]), (br, uvs[2])] {
            self.verts.push(Vertex { pos, uv, color: q.color });
        }
        for &(pos, uv) in &[(tl, uvs[0]), (br, uvs[2]), (bl, uvs[3])] {
            self.verts.push(Vertex { pos, uv, color: q.color });
        }
        match self.runs.last_mut() {
            Some(r) if r.tex == tex && r.blend == blend => r.count += 6,
            _ => self.runs.push(Run {
                tex,
                blend,
                first,
                count: 6,
            }),
        }
        true
    }

    /// Vertex ring as raw bytes for `glBufferSubData`. The layout is
    /// `repr(C)` with no padding (checked above), so the cast is sound.
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: Vertex is repr(C), 20 bytes, no padding — a Vertex slice is
        // a valid byte slice of len*20.
        unsafe {
            std::slice::from_raw_parts(
                self.verts.as_ptr() as *const u8,
                self.verts.len() * std::mem::size_of::<Vertex>(),
            )
        }
    }

    #[cfg(test)]
    fn vertex(&self, i: usize) -> &Vertex {
        &self.verts[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(x: f32) -> Quad {
        Quad {
            x,
            y: 0.0,
            w: 10.0,
            h: 20.0,
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
            color: [10, 20, 30, 255],
            angle: 0.0,
            px: x + 5.0,
            py: 10.0,
        }
    }

    #[test]
    fn vertex_layout_is_20_bytes() {
        assert_eq!(std::mem::size_of::<Vertex>(), 20);
        let mut b = Batcher::new();
        b.push(1, BLENDS_ALPHA, &quad(0.0));
        assert_eq!(b.bytes().len(), 6 * 20);
    }

    #[test]
    fn six_vertices_per_quad_in_order() {
        let mut b = Batcher::new();
        b.push(1, BLENDS_ALPHA, &quad(0.0));
        assert_eq!(b.quad_count(), 1);
        let tl = b.vertex(0).pos;
        let br = b.vertex(2).pos;
        assert_eq!(tl, [0.0, 0.0]);
        assert_eq!(br, [10.0, 20.0]);
        // uv of tr is (u1, v0)
        assert_eq!(b.vertex(1).uv, [1.0, 0.0]);
        // second triangle restarts at tl
        assert_eq!(b.vertex(3).pos, [0.0, 0.0]);
        assert_eq!(b.vertex(5).pos, [0.0, 20.0]);
    }

    #[test]
    fn texture_change_splits_runs_blend_change_too() {
        let mut b = Batcher::new();
        b.push(1, BLENDS_ALPHA, &quad(0.0));
        b.push(1, BLENDS_ALPHA, &quad(5.0)); // same tex+blend → same run
        b.push(2, BLENDS_ALPHA, &quad(9.0)); // tex change → new run
        b.push(2, BLENDS_ADD, &quad(9.0)); // blend change → new run
        assert_eq!(b.run_count(), 3);
        assert_eq!(b.runs()[0].count, 12);
        assert_eq!(b.runs()[1].first, 12);
        assert_eq!(b.runs()[2].blend, BLENDS_ADD);
        // runs are contiguous
        assert_eq!(b.runs()[1].first, b.runs()[0].first + b.runs()[0].count);
        assert_eq!(b.runs()[2].first, b.runs()[1].first + b.runs()[1].count);
    }

    #[test]
    fn rotation_moves_corners_around_pivot() {
        let mut q = quad(0.0);
        q.angle = std::f32::consts::FRAC_PI_2; // 90°
        let mut b = Batcher::new();
        b.push(1, BLENDS_ALPHA, &q);
        // tl (0,0) rotated 90° around pivot (5,10): dx=-5,dy=-10 → (5+10, 10-5)
        assert!((b.vertex(0).pos[0] - 15.0).abs() < 1e-5);
        assert!((b.vertex(0).pos[1] - 5.0).abs() < 1e-5);
    }

    #[test]
    fn full_batch_refuses_push() {
        let mut b = Batcher::new();
        for i in 0..MAX_QUADS {
            assert!(b.push(1, BLENDS_ALPHA, &quad(i as f32)));
        }
        assert!(b.is_full());
        assert!(!b.push(1, BLENDS_ALPHA, &quad(0.0)));
        assert_eq!(b.quad_count(), MAX_QUADS);
    }
}
