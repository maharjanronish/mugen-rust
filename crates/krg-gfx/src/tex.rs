//! GPU texture store: LRU-cached GL textures over the format layer.
//!
//! A sprite reaches the GPU exactly once per cache lifetime: the first time
//! it is drawn it is decoded via [`krg_format::SpriteCache`] (CPU-side, also
//! budgeted), converted to premultiplied RGBA, and uploaded; afterwards it is
//! a hash lookup + `note_use`. Eviction frees the GL texture objects so VRAM
//! stays under a fixed ceiling — a full MUGEN roster never needs to fit.

use std::collections::HashMap;

use glow::HasContext;

use crate::lru::FrameLru;
use crate::GfxError;
use krg_format::Pixels;

/// Identity of a sprite texture inside one SFF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TexKey {
    pub group: u16,
    pub number: u16,
}

/// A resident texture (handle + geometry). `tex` is the raw glow handle so
/// the struct stays plain data.
#[derive(Debug, Clone, Copy)]
pub struct TexInfo {
    /// Non-zero GL texture name.
    pub tex: u32,
    pub w: i32,
    pub h: i32,
}

/// The GL half of sprite streaming: (group, number) → texture, with a byte
/// budget and last-use eviction driven by [`FrameLru`].
pub struct TexLru {
    budget_bytes: usize,
    lru: FrameLru<TexKey>,
    map: HashMap<TexKey, TexInfo>,
    evictions: u64,
}

impl TexLru {
    pub fn new(budget_bytes: usize) -> Self {
        TexLru {
            budget_bytes,
            lru: FrameLru::new(budget_bytes),
            map: HashMap::new(),
            evictions: 0,
        }
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub fn used_bytes(&self) -> usize {
        self.lru.used()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    pub fn frame(&self) -> u64 {
        self.lru.frame()
    }

    pub fn advance_frame(&mut self) {
        self.lru.advance_frame();
    }

    /// Resident texture lookup (marks a use). `None` = cache miss: the caller
    /// decodes and re-inserts through [`TexLru::put`].
    pub fn get(&mut self, key: TexKey) -> Option<TexInfo> {
        self.lru.note_use(&key).map(|_| self.map[&key])
    }

    /// Upload `pixels` as the texture for `key`, insert, and evict
    /// over-budget entries (deleting their GL textures).
    pub fn put(
        &mut self,
        gl: &glow::Context,
        key: TexKey,
        w: i32,
        h: i32,
        pixels: &Pixels,
        palette: Option<&[u32; 256]>,
    ) -> Result<TexInfo, GfxError> {
        let rgba = pixels_to_rgba(pixels, palette)?;
        let tex = upload_rgba(gl, w, h, &rgba)?;
        let info = TexInfo { tex: tex.0.get(), w, h };
        let cost = (w as usize) * (h as usize) * 4;
        self.lru.insert(key, cost);
        self.map.insert(key, info);
        for (k, _) in self.lru.evict_to_budget() {
            if let Some(old) = self.map.remove(&k) {
                delete_texture(gl, old.tex);
                self.evictions += 1;
            }
        }
        Ok(info)
    }

    /// Delete every resident texture (context loss / scene teardown).
    pub fn destroy_all(&mut self, gl: &glow::Context) {
        for info in self.map.values() {
            delete_texture(gl, info.tex);
        }
        self.map.clear();
        self.lru.clear();
    }
}

/// Convert decode-layer pixels to premultiplied RGBA bytes for upload.
///
/// `Pixels::Rgba` is already premultiplied by the format layer (parity with
/// Ikemen's NRGBA→RGBA); indexed pixels are expanded through their palette
/// snapshot and then run through the same premultiply — one formula, one
/// look, everywhere.
pub fn pixels_to_rgba(
    pixels: &Pixels,
    palette: Option<&[u32; 256]>,
) -> Result<Vec<u8>, GfxError> {
    match pixels {
        Pixels::Rgba(rgba) => Ok(rgba.clone()),
        Pixels::Indexed(idx) => {
            let pal = palette.ok_or_else(|| {
                GfxError::Gl("indexed sprite without palette snapshot".into())
            })?;
            // Palette packing (sff.rs): a<<24 | b<<16 | g<<8 | r.
            let mut straight: Vec<u8> = Vec::with_capacity(idx.len() * 4);
            for &i in idx {
                let p = pal[i as usize];
                straight.extend_from_slice(&[
                    p as u8,
                    (p >> 8) as u8,
                    (p >> 16) as u8,
                    (p >> 24) as u8,
                ]);
            }
            Ok(krg_format::premultiply_rgba(straight))
        }
    }
}

/// Upload one RGBA8 texture with pixel-art sampling (nearest, clamped).
fn upload_rgba(gl: &glow::Context, w: i32, h: i32, bytes: &[u8]) -> Result<glow::Texture, GfxError> {
    let expected = (w as usize) * (h as usize) * 4;
    if bytes.len() != expected {
        return Err(GfxError::Gl(format!(
            "sprite upload size mismatch: have {} bytes, need {expected}",
            bytes.len()
        )));
    }
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
            glow::PixelUnpackData::Slice(Some(bytes)),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        Ok(tex)
    }
}

fn delete_texture(gl: &glow::Context, raw: u32) {
    if let Some(t) = crate::nz(raw) {
        unsafe { gl.delete_texture(t) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_expands_through_palette_and_premultiplies() {
        // Palette packing (sff.rs): a<<24 | b<<16 | g<<8 | r.
        // idx0 must be truly transparent: a byte = 0 → the whole word is 0.
        let mut pal = [0u32; 256];
        pal[1] = 0xFF_00_00_FF; // a=255, r=255
        pal[2] = 0x80_FF_FF_FF; // a=128, rgb=255
        let px = Pixels::Indexed(vec![0, 1, 2]);
        let rgba = pixels_to_rgba(&px, Some(&pal)).unwrap();
        assert_eq!(rgba.len(), 12);
        // idx0: transparent → all zero
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
        // idx1: opaque red stays red
        assert_eq!(&rgba[4..8], &[255, 0, 0, 255]);
        // idx2: half-alpha white premultiplies to ~127
        let v = rgba[8] as u32;
        assert!((120..=135).contains(&v), "premul value {v}");
        assert_eq!(rgba[11], 128);
    }

    #[test]
    fn indexed_without_palette_is_an_error() {
        let px = Pixels::Indexed(vec![0]);
        assert!(pixels_to_rgba(&px, None).is_err());
    }

    #[test]
    fn rgba_passes_through() {
        let px = Pixels::Rgba(vec![1, 2, 3, 4]);
        assert_eq!(pixels_to_rgba(&px, None).unwrap(), vec![1, 2, 3, 4]);
    }
}
