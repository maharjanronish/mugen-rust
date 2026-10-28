//! Sprite helpers: premultiplication and the lazy LRU decode cache.

use crate::sff::{Pixels, Sff, SpriteEntry};
use std::collections::HashMap;

/// Port of the 32-bit branch of `Sprite.SetRaw` (image.go:886-905):
/// straight RGBA → premultiplied RGBA, with fully-transparent pixels zeroed so
/// their RGB cannot bleed through bilinear filtering. Reproduces Go's
/// NRGBA→RGBA conversion arithmetic (`c * 0x101 * a / 0xff >> 8`).
pub fn premultiply_rgba(mut data: Vec<u8>) -> Vec<u8> {
    let mut i = 0;
    while i + 3 < data.len() {
        let a = data[i + 3];
        if a == 255 {
            i += 4;
            continue;
        }
        if a == 0 {
            data[i] = 0;
            data[i + 1] = 0;
            data[i + 2] = 0;
            i += 4;
            continue;
        }
        data[i] = ((data[i] as u32 * 0x101 * a as u32 / 0xff) >> 8) as u8;
        data[i + 1] = ((data[i + 1] as u32 * 0x101 * a as u32 / 0xff) >> 8) as u8;
        data[i + 2] = ((data[i + 2] as u32 * 0x101 * a as u32 / 0xff) >> 8) as u8;
        i += 4;
    }
    data
}

/// A decoded sprite ready for texture upload.
#[derive(Debug, Clone)]
pub struct DecodedSprite {
    pub entry_index: usize,
    pub pixels: Pixels,
    /// Palette snapshot for the sprite's palidx at decode time (Indexed only).
    pub palette: Option<[u32; 256]>,
}

/// LRU cache over an [`Sff`] that decodes sprites on demand and enforces a
/// **byte budget** — the core RAM optimization. Sprites absent from the cache
/// are re-decoded from the (still mmapped, shared) file when next requested.
///
/// Eviction policy: when the budget is exceeded, the oldest ~10% of entries
/// by last-use generation are dropped (deterministic, cheap; good enough for
/// fighting-game access patterns where hot frames cluster).
pub struct SpriteCache {
    sff: Sff,
    budget_bytes: usize,
    used_bytes: usize,
    generation: u64,
    cache: HashMap<(u16, u16), Cached>,
    /// Default per-sprite palette snapshot policy: snapshot at decode.
    snapshot_palette: bool,
}

#[derive(Debug)]
struct Cached {
    pixels: Pixels,
    palette: Option<[u32; 256]>,
    gen: u64,
}

impl SpriteCache {
    pub fn new(sff: Sff, budget_bytes: usize) -> Self {
        SpriteCache {
            sff,
            budget_bytes,
            used_bytes: 0,
            generation: 0,
            cache: HashMap::new(),
            snapshot_palette: true,
        }
    }

    pub fn sff(&self) -> &Sff {
        &self.sff
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn cached_count(&self) -> usize {
        self.cache.len()
    }

    /// Look up (group, number) in the owning SFF (Go `GetSprite`).
    pub fn entry(&self, group: u16, number: u16) -> Option<&SpriteEntry> {
        self.sff.entry(group, number)
    }

    /// Get decoded pixels, decoding (and caching) on demand.
    pub fn get(&mut self, group: u16, number: u16) -> Result<Option<DecodedSprite>, crate::sff::SffError> {
        let Some(entry) = self.sff.entry(group, number) else {
            return Ok(None);
        };
        self.generation += 1;
        let gen = self.generation;

        if let Some(c) = self.cache.get_mut(&(group, number)) {
            c.gen = gen;
            return Ok(Some(DecodedSprite {
                entry_index: self.sff.index[&(group, number)],
                pixels: c.pixels.clone(),
                palette: c.palette,
            }));
        }

        let pixels = self.sff.decode(entry)?;
        let palette = if self.snapshot_palette {
            self.sff.palettes.get(entry.palidx).copied()
        } else {
            None
        };
        let cost = pixels.byte_len();
        self.cache.insert(
            (group, number),
            Cached { pixels: pixels.clone(), palette, gen },
        );
        self.used_bytes += cost;
        if self.used_bytes > self.budget_bytes {
            self.evict();
        }
        Ok(Some(DecodedSprite {
            entry_index: self.sff.index[&(group, number)],
            pixels,
            palette,
        }))
    }

    /// Decode without caching (one-shot use).
    pub fn peek(&self, group: u16, number: u16) -> Result<Option<DecodedSprite>, crate::sff::SffError> {
        match self.sff.entry(group, number) {
            Some(e) => {
                let pixels = self.sff.decode(e)?;
                let palette = if self.snapshot_palette {
                    self.sff.palettes.get(e.palidx).copied()
                } else {
                    None
                };
                Ok(Some(DecodedSprite {
                    entry_index: self.sff.index[&(group, number)],
                    pixels,
                    palette,
                }))
            }
            None => Ok(None),
        }
    }

    /// Drop the oldest ~10% of cached sprites (by last-use generation).
    fn evict(&mut self) {
        let mut gens: Vec<u64> = self.cache.values().map(|c| c.gen).collect();
        if gens.is_empty() {
            return;
        }
        gens.sort_unstable();
        let cut = gens[(gens.len() as f64 * 0.1).ceil().max(1.0) as usize - 1];
        let before = self.cache.len();
        self.cache.retain(|_, c| c.gen > cut);
        let removed = before - self.cache.len();
        if removed == 0 {
            // All same generation (single bulk load): drop half.
            let mut it: Vec<_> = self.cache.drain().collect();
            let keep = it.len() / 2;
            it.truncate(keep);
            self.cache = it.into_iter().collect();
        }
        self.used_bytes = self.cache.values().map(|c| c.pixels.byte_len()).sum();
    }

    /// Evict everything.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.used_bytes = 0;
    }
}
