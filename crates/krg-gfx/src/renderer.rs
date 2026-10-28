//! The GLES 3.0 renderer: one shader, one VBO ring, one FBO, one blit.
//!
//! Frame shape (all GL calls happen on the GL thread that owns the context):
//!
//! ```text
//! begin_scene()               bind FBO, viewport 320×240, clear, mvp=ortho(game)
//!   push_sprite(...) ×N       resolve texture (LRU), append quad to the ring
//!   [flush() if the ring fills]      mid-frame safety valve
//! present()                   flush → bind default fb → one letterboxed
//!                             upscale quad → swap (the Java layer)
//! ```
//!
//! The game never renders at phone resolution: the scene is authored at
//! MUGEN's native pixel space in an FBO, and the only full-surface work is
//! the single upscale draw.

use glow::HasContext;
use krg_format::SpriteCache;

use crate::batch::{Batcher, Quad, BLENDS_ADD, BLENDS_ALPHA, MAX_QUADS};
use crate::tex::{TexKey, TexLru};
use crate::{nz, GfxError};

/// Native MUGEN scene resolution (the classic localcoord).
pub const GAME_W: i32 = 320;
pub const GAME_H: i32 = 240;

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
    fbo: glow::Framebuffer,
    scene_tex: u32,
    white: u32,
    batch: Batcher,
    texlru: TexLru,
    surface: (i32, i32),
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

        // ── scene FBO at native game resolution ────────────────────────────
        let scene_tex = create_scene_texture(&gl, GAME_W, GAME_H)?;
        let fbo = gl
            .create_framebuffer()
            .map_err(GfxError::Gl)?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            nz(scene_tex),
            0,
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        if status != glow::FRAMEBUFFER_COMPLETE {
            return Err(GfxError::Gl(format!(
                "scene framebuffer incomplete: 0x{status:04X}"
            )));
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);

        // ── shared 1×1 white texture (untextured quads / controller) ──────
        let white = gl
            .create_texture()
            .map_err(GfxError::Gl)?;
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
            fbo,
            scene_tex,
            white: white.0.get(),
            batch: Batcher::new(),
            texlru,
            surface: (GAME_W, GAME_H),
            stats,
        })
    }

    /// Surface size change (rotation, resize).
    pub fn resize(&mut self, w: i32, h: i32) {
        self.surface = (w.max(1), h.max(1));
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Begin the game scene: bind the FBO, clear, set the game-space MVP.
    pub fn begin_scene(&mut self) {
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.viewport(0, 0, GAME_W, GAME_H);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(self.prog));
            gl.uniform_matrix_4_f32_slice(
                self.uni_mvp.as_ref(),
                false,
                &ortho(GAME_W as f32, GAME_H as f32),
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
    /// `(x, y)` is the sprite's **axis position** in game space — the AIR
    /// `xoffset/yoffset` of the entry are subtracted like Ikemen's draw at
    /// `(pos - offset)` with `xs`/`ys` scale factors (negative flips).
    /// Returns `false` when the sprite is missing or failed to stream (the
    /// frame stays valid; the sprite is simply not drawn this frame).
    #[allow(clippy::too_many_arguments)]
    pub fn push_sprite(
        &mut self,
        cache: &mut SpriteCache,
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
        let key = TexKey { group, number };
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

    /// End the frame: flush the scene, then present it letterboxed to the
    /// default framebuffer.
    pub fn present(&mut self) {
        self.flush();
        let gl = &self.gl;
        let (sw, sh) = self.surface;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, sw, sh);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(self.prog));
            gl.uniform_matrix_4_f32_slice(
                self.uni_mvp.as_ref(),
                false,
                &ortho(sw as f32, sh as f32),
            );
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            let (dx, dy, dw, dh) = letterbox(sw, sh, GAME_W, GAME_H);
            let q = Quad {
                x: dx,
                y: dy,
                w: dw,
                h: dh,
                u0: 0.0,
                v0: 0.0,
                u1: 1.0,
                v1: 1.0,
                color: [255, 255, 255, 255],
                angle: 0.0,
                px: dx + dw / 2.0,
                py: dy + dh / 2.0,
            };
            // Scene texture is sampled with its own (linear) filter.
            if let Some(t) = nz(self.scene_tex) {
                gl.bind_texture(glow::TEXTURE_2D, Some(t));
            }
            gl.bind_vertex_array(Some(self.vao));
            let pushed = self.batch.push(self.scene_tex, BLENDS_ALPHA, &q);
            debug_assert!(pushed);
            self.flush();
        }
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
            if let Some(t) = nz(self.scene_tex) {
                gl.delete_texture(t);
            }
            if let Some(t) = nz(self.white) {
                gl.delete_texture(t);
            }
            gl.delete_program(self.prog);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            gl.delete_framebuffer(self.fbo);
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

/// Largest `gw:gh`-aspect rect that fits into `sw:sh`, centered.
pub fn letterbox(sw: i32, sh: i32, gw: i32, gh: i32) -> (f32, f32, f32, f32) {
    let (sw, sh, gw, gh) = (sw as f32, sh as f32, gw as f32, gh as f32);
    let scale = if sw / gw < sh / gh {
        sw / gw
    } else {
        sh / gh
    };
    let (w, h) = (gw * scale, gh * scale);
    ((sw - w) / 2.0, (sh - h) / 2.0, w, h)
}

/// Scene target texture — RGBA8, linear filter (the upscale itself), clamped.
fn create_scene_texture(gl: &glow::Context, w: i32, h: i32) -> Result<u32, GfxError> {
    unsafe {
        let tex = gl
            .create_texture()
            .map_err(GfxError::Gl)?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            w,
            h,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        Ok(tex.0.get())
    }
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

    #[test]
    fn letterbox_fits_and_centers() {
        // 4:3 game on a 16:9 landscape phone → pillarboxed, full height.
        let (x, y, w, h) = letterbox(2400, 1080, 320, 240);
        assert_eq!((h, 1080.0), (1080.0, 1080.0));
        assert!((w - 1440.0).abs() < 0.01); // 4:3 of 1080
        assert_eq!((y, x), (0.0, (2400.0 - 1440.0) / 2.0));
        // Portrait phone → letterboxed, full width.
        let (x, y, w, h) = letterbox(1080, 2400, 320, 240);
        assert_eq!((w, x), (1080.0, 0.0));
        assert!((h - 810.0).abs() < 0.01);
        assert_eq!(y, (2400.0 - 810.0) / 2.0);
    }
}
