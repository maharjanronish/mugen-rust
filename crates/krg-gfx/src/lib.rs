//! Kurogane GLES 3.0 renderer.
//!
//! # Design — the whole pipeline is built around three budgets
//!
//! **Fill rate**: the game renders at its native MUGEN resolution (typically
//! 320×240) into an offscreen FBO, then reaches the screen as ONE upscaled
//! quad. The phone's GPU spends its time on one big blit, not on a scene the
//! size of a 1440p panel.
//!
//! **CPU**: every sprite of a frame lands in a single pre-allocated vertex
//! ring (packed 20-byte vertices, no per-frame allocations, no per-sprite
//! draw calls — one draw per texture/blend run). Vertex math is done on the
//! CPU only when a quad is actually rotated (rare: helper sprites, sparks).
//!
//! **RAM/VRAM**: sprites stream in on demand — [`krg_format::SpriteCache`]
//! decodes from the mmapped SFF (CPU-side LRU, byte-budgeted) and the GPU
//! side keeps only a second LRU of texture objects, also byte-budgeted.
//! Nothing is resident "just in case"; eviction is by last-use frame.
//!
//! Pixels are **premultiplied RGBA** (see `krg_format::premultiply_rgba`),
//! so blending is `(ONE, ONE_MINUS_SRC_ALPHA)` for alpha and `(ONE, ONE)`
//! for additive — the same setup Ikemen GO uses, which also makes
//! bilinear-filtered sprite edges correct (no dark halo).
//!
//! The GL binding is [glow]; function pointers come from
//! `eglGetProcAddress` on Android (the EGL context itself is created by the
//! Java layer's GLSurfaceView, so no Java→Rust loader plumbing).

pub mod batch;
pub mod lru;
pub mod renderer;
pub mod tex;

pub use batch::{Batcher, Quad, BLENDS_ADD, BLENDS_ALPHA};
pub use lru::FrameLru;
pub use renderer::{Renderer, GAME_H, GAME_W};
pub use tex::{TexKey, TexLru};

/// Raw GL texture name → glow handle (0 stays `None`).
pub(crate) fn nz(raw: u32) -> Option<glow::NativeTexture> {
    std::num::NonZeroU32::new(raw).map(glow::NativeTexture)
}

#[derive(Debug)]
pub enum GfxError {
    /// A glow/OpenGL call failed (lost context, out of memory, …).
    Gl(String),
    /// Shader compilation or program link failed, with the info log.
    Shader(String),
}

impl std::fmt::Display for GfxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GfxError::Gl(s) => write!(f, "OpenGL error: {s}"),
            GfxError::Shader(s) => write!(f, "shader error: {s}"),
        }
    }
}

impl std::error::Error for GfxError {}
