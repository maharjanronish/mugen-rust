//! The GLES 3.0 renderer: one shader, one VBO ring, direct native-res draws.
//!
//! Frame shape (all GL calls happen on the GL thread that owns the context):
//!
//! ```text
//! begin_scene()               bind default fb, viewport = full surface,
//!                             clear, mvp = ortho(game space)
//!   push_sprite(...) ×N       resolve texture (LRU), append quad to the ring
//!   [flush() if the ring fills]      mid-frame safety valve
//! present()                   flush → swap (the Java layer)
//! ```
//!
//! The game is authored in MUGEN pixel space (height locked to 240 units,
//! width follows the screen aspect) but RASTERIZED AT DEVICE RESOLUTION:
//! one scaling stage instead of the old 320×240 FBO + upscale blit, which
//! blurred every sprite twice. Flat-color HUD quads and (NEAREST-sampled)
//! sprites now land on exact device pixels — the "HD" path.

use glow::HasContext;
use krg_format::SpriteCache;

use crate::batch::{Batcher, Quad, BLENDS_ADD, BLENDS_ALPHA, MAX_QUADS};
use crate::tex::{TexKey, TexLru};
use crate::{nz, GfxError};

/// Game-space height — MUGEN's classic localcoord y. The width is
/// aspect-driven (see [`Renderer::resize`]) so widescreen phones see MORE
/// stage instead of a pillarboxed 4:3 slice.
pub const GAME_H: i32 = 240;
/// Game-space width bounds: 320 = classic 4:3, 640 covers 20:9 phones.
pub const GAME_W_MIN: f32 = 320.0;
pub const GAME_W_MAX: f32 = 640.0;

/// Default GPU budget for sprite textures (48 MB — plenty for a stage + both
/// fighters' visible frames; everything else streams).
pub const TEX_BUDGET_BYTES: usize = 48 * 1024 * 1024;

const VS_SRC: &str = r#"#version 300 es
precision highp float;
uniform mat4 u_mvp;
in vec2 a_pos;
in vec2 a_uv;
in vec4 a_color;
out vec2 v_uv;
out vec4 v_color;
void main() {
    gl_Position = u_mvp * vec4(a_pos, 0.0, 1.0);
    v_uv = a_uv;
    v_color = a_color;
}"#;

const FS_SRC: &str = r#"#version 300 es
precision mediump float;
uniform sampler2D u_tex;
in vec2 v_uv;
in vec4 v_color;
out vec4 o;
void main() {
    vec4 t = texture(u_tex, v_uv);
    // Source pixels are premultiplied; the tint is straight RGBA, so apply
    // it channel-wise without un-premultiplying (see crate docs).
    o = vec4(t.rgb * v_color.rgb, t.a * v_color.a);
}"#;

/// Per-frame renderer counters (exposed for HUD/log diagnostics).
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub quads: u32,
    pub runs: u32,
    pub frames: u64,
    pub textures: u32,
    pub tex_bytes: usize,
    pub evictions: u64,
}

pub struct Renderer {
    gl: glow::Context,
    prog: glow::Program,
    uni_mvp: Option<glow::UniformLocation>,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    white: u32,
    batch: Batcher,
    texlru: TexLru,
    surface: (i32, i32),
    /// Game-space extents (gw × 240): gw = 240·aspect clamped to
    /// [GAME_W_MIN, GAME_W_MAX]. The ortho maps this onto the full surface.
    game_size: (f32, f32),
    stats: Stats,
}

impl Renderer {
    /// Build the renderer against the **current** GL context. `loader` must
    /// resolve GL entry points (eglGetProcAddress on Android).
    ///
    /// # Safety
    /// A GLES 3.0 context must be current on the calling thread, and all
    /// later method calls must come from that same thread.
    pub unsafe fn new(
        loader: impl FnMut(&str) -> *const std::os::raw::c_void,
    ) -> Result<Renderer, GfxError> {
        let gl = glow::Context::from_loader_function(loader);

        // ── program ────────────────────────────────────────────────────────
        let vs = gl
            .create_shader(glow::VERTEX_SHADER)
            .map_err(GfxError::Gl)?;
        gl.shader_source(vs, VS_SRC);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            let log = gl.get_shader_info_log(vs);
            gl.delete_shader(vs);
            return Err(GfxError::Shader(format!("vertex: {log}")));
        }
        let fs = gl
            .create_shader(glow::FRAGMENT_SHADER)
            .map_err(GfxError::Gl)?;
        gl.shader_source(fs, FS_SRC);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            let log = gl.get_shader_info_log(fs);
            gl.delete_shader(vs);
            gl.delete_shader(fs);
            return Err(GfxError::Shader(format!("fragment: {log}")));
        }
        let prog = gl
            .create_program()
            .map_err(GfxError::Gl)?;
        gl.attach_shader(prog, vs);
        gl.attach_shader(prog, fs);
        gl.bind_attrib_location(prog, 0, "a_pos");
        gl.bind_attrib_location(prog, 1, "a_uv");
        gl.bind_attrib_location(prog, 2, "a_color");
        gl.link_program(prog);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        if !gl.get_program_link_status(prog) {
            return Err(GfxError::Shader(gl.get_program_info_log(prog)));
        }
        gl.use_program(Some(prog));
        let uni_mvp = gl.get_uniform_location(prog, "u_mvp");
        if let Some(loc) = gl.get_uniform_location(prog, "u_tex") {
            gl.uniform_1_i32(Some(&loc), 0); // texture unit 0
        }

        // ── vertex ring ────────────────────────────────────────────────────
        let vao = gl
            .create_vertex_array()
            .map_err(GfxError::Gl)?;
        gl.bind_vertex_array(Some(vao));
        let vbo = gl
            .create_buffer()
            .map_err(GfxError::Gl)?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            &vec![0u8; MAX_QUADS * 6 * 20],
            glow::DYNAMIC_DRAW,
        );
        let stride = 20;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 8);
        gl.enable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_f32(2, 4, glow::UNSIGNED_BYTE, true, stride, 16);

        // ── shared 1×1 white texture (untextured quads / controller) ──────
        let white = gl
            .create_texture()
            .map_err(GfxError::Gl)?;;
        gl.bind_texture(glow::TEXTURE_2D, Some(white));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            1,
            1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&[255, 255, 255, 255])),
        );

        gl.enable(glow::BLEND);

        let texlru = TexLru::new(TEX_BUDGET_BYTES);
        let stats = Stats {
            tex_bytes: texlru.used_bytes(),
            ..Stats::default()
        };

        Ok(Renderer {
            gl,
            prog,
            uni_mvp,
            vao,
            vbo,
            white: white.0.get(),
            batch: Batcher::new(),
            texlru,
            surface: (GAME_H, GAME_H),
            game_size: (320.0, GAME_H as f32),
            stats,
        })
    }

    /// Surface size change (rotation, resize). Recomputes the game-space
    /// width: **height stays locked at 240 MUGEN units** and the width
    /// follows the physical aspect (clamped), so a 16:9/20:9 phone shows a
    /// WIDER slice of the stage with zero letterboxing and zero stretching.
    pub fn resize(&mut self, w: i32, h: i32) {
        self.surface = (w.max(1), h.max(1));
        let aspect = self.surface.0 as f32 / self.surface.1 as f32;
        let gw = (GAME_H as f32 * aspect).clamp(GAME_W_MIN, GAME_W_MAX);
        self.game_size = (gw, GAME_H as f32);
    }

    /// Current game-space extents (width, height) — the scene lays out
    /// against this (centering, HUD anchors, stage anchoring).
    pub fn game_extents(&self) -> (f32, f32) {
        self.game_size
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Begin the game scene: clear the default framebuffer and set the
    /// game-space MVP (game units → full surface, rasterized at native res).
    pub fn begin_scene(&mut self) {
        let gl = &self.gl;
        let (sw, sh) = self.surface;
        let (gw, gh) = self.game_size;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, sw, sh);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(self.prog));
            gl.uniform_matrix_4_f32_slice(
                self.uni_mvp.as_ref(),
                false,
                &ortho(gw, gh),
            );
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }
        self.batch.clear();
        self.texlru.advance_frame();
    }

    /// Append an already-resolved quad (white texture = flat color quad).
    pub fn push(&mut self, tex: u32, q: &Quad) -> bool {
        let tex = if tex == 0 { self.white } else { tex };
        self.batch.push(tex, BLENDS_ALPHA, q)
    }

    /// Resolve (streaming on miss) and draw one sprite.
    ///
    /// `src` namespaces the texture LRU: several SFFs (stage / chars / font)
    /// can share one renderer without their (group, number) colliding.
    /// `(x, y)` is the sprite's **axis position** in game space — the AIR
    /// `xoffset/yoffset` of the entry are subtracted like Ikemen's draw at
    /// `(pos - offset)` with `xs`/`ys` scale factors (negative flips).
    /// Returns `false` when the sprite is missing or failed to stream (the
    /// frame stays valid; the sprite is simply not drawn this frame).
    #[allow(clippy::too_many_arguments)]
    pub fn push_sprite(
        &mut self,
        cache: &mut SpriteCache,
        src: u32,
        group: u16,
        number: u16,
        x: f32,
        y: f32,
        xs: f32,
        ys: f32,
        color: [u8; 4],
        blend: u8,
    ) -> bool {
        let Some(entry) = cache.entry(group, number) else {
            return false;
        };
        let (w, h, ox, oy) = (
            entry.width as f32,
            entry.height as f32,
            entry.x as f32,
            entry.y as f32,
        );
        if w == 0.0 || h == 0.0 {
            return false;
        }
        let key = TexKey { src, group, number };
        let info = match self.texlru.get(key) {
            Some(info) => info,
            None => {
                // Cache miss: decode + upload. `Ok(None)` = absent sprite;
                // errors are logged-and-skipped, never fatal to the frame.
                let dec = match cache.get(group, number) {
                    Ok(Some(d)) => d,
                    Ok(None) => return false,
                    Err(e) => {
                        log::debug!("sprite {group},{number} failed to decode: {e}");
                        return false;
                    }
                };
                match self.texlru.put(
                    &self.gl,
                    key,
                    w as i32,
                    h as i32,
                    &dec.pixels,
                    dec.palette.as_ref(),
                ) {
                    Ok(info) => info,
                    Err(e) => {
                        log::debug!("sprite {group},{number} failed to upload: {e}");
                        return false;
                    }
                }
            }
        };
        let q = Quad {
            x: x - ox * xs,
            y: y - oy * ys,
            w: w * xs,
            h: h * ys,
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
            color,
            angle: 0.0,
            px: x,
            py: y,
        };
        let blend = if blend == BLENDS_ADD { BLENDS_ADD } else { BLENDS_ALPHA };
        self.batch.push(info.tex, blend, &q)
    }

    /// Upload the vertex ring and issue one draw per run (into whichever
    /// target is currently bound).
    pub fn flush(&mut self) {
        let bytes = self.batch.bytes();
        if bytes.is_empty() {
            return;
        }
        self.stats.quads = self.batch.quad_count() as u32;
        self.stats.runs = self.batch.run_count() as u32;
        let gl = &self.gl;
        unsafe {
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);
            for run in self.batch.runs().iter() {
                if let Some(t) = nz(run.tex) {
                    gl.bind_texture(glow::TEXTURE_2D, Some(t));
                }
                gl.blend_func(
                    glow::ONE,
                    if run.blend == BLENDS_ADD {
                        glow::ONE
                    } else {
                        glow::ONE_MINUS_SRC_ALPHA
                    },
                );
                gl.draw_arrays(glow::TRIANGLES, run.first as i32, run.count as i32);
            }
            self.batch.clear();
        }
    }

    /// End the frame: flush the batch (already drawn at native resolution).
    pub fn present(&mut self) {
        self.flush();
        // stats
        self.stats.frames += 1;
        self.stats.textures = self.texlru.len() as u32;
        self.stats.tex_bytes = self.texlru.used_bytes();
        self.stats.evictions = self.texlru.evictions();
    }

    /// Clear the default framebuffer only (no scene yet — setup mode).
    pub fn clear_default(&mut self) {
        let gl = &self.gl;
        let (sw, sh) = self.surface;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, sw, sh);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    /// Free every GPU resource this renderer owns (context teardown).
    pub fn destroy(&mut self) {
        let gl = &self.gl;
        self.texlru.destroy_all(gl);
        unsafe {
            if let Some(t) = nz(self.white) {
                gl.delete_texture(t);
            }
            gl.delete_program(self.prog);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
        }
    }
}

/// Column-major orthographic matrix mapping screen pixel space (y down) to
/// clip space.
fn ortho(w: f32, h: f32) -> [f32; 16] {
    [
        2.0 / w, 0.0, 0.0, 0.0,
        0.0, -2.0 / h, 0.0, 0.0,
        0.0, 0.0, -1.0, 0.0,
        -1.0, 1.0, 0.0, 1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ortho_maps_corners_to_clip_space() {
        let m = ortho(320.0, 240.0);
        // x=0,y=0 (top-left) → (-1, 1); x=320,y=240 (bottom-right) → (1, -1)
        let tl = (m[0] * 0.0 + m[4] * 0.0 + m[12], m[1] * 0.0 + m[5] * 0.0 + m[13]);
        let br = (m[0] * 320.0 + m[4] * 240.0 + m[12], m[1] * 320.0 + m[5] * 240.0 + m[13]);
        assert!((tl.0 - (-1.0)).abs() < 1e-6 && (tl.1 - 1.0).abs() < 1e-6);
        assert!((br.0 - 1.0).abs() < 1e-6 && (br.1 - (-1.0)).abs() < 1e-6);
    }

    /// Game width follows the surface aspect (height locked at 240 units),
    /// clamped to [320, 640]: a 16:9 phone sees a 426-wide stage slice, a
    /// 20:9 phone 533 — never a pillarboxed 320, never unbounded.
    #[test]
    fn game_width_follows_aspect_and_clamps() {
        // pure function of (w, h) — replicate resize()'s math
        let gw = |w: i32, h: i32| {
            (GAME_H as f32 * (w as f32 / h as f32)).clamp(GAME_W_MIN, GAME_W_MAX)
        };
        // 4:3 → classic 320
        assert!((gw(320, 240) - 320.0).abs() < 0.01);
        // 16:9 landscape phone → 426.67
        assert!((gw(1920, 1080) - 426.67).abs() < 0.1, "{}", gw(1920, 1080));
        // 20:9 phone → 533.3
        assert!((gw(2400, 1080) - 533.33).abs() < 0.1, "{}", gw(2400, 1080));
        // ultrawide → clamped at 640
        assert!((gw(3840, 1080) - 640.0).abs() < 0.01);
    }
}
