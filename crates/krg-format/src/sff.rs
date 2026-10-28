//! SFF (Spr image file) loading — faithful port of Ikemen GO `src/image.go`
//! (`SffHeader.Read`, `Sprite.read/readHeader/readV2`, `loadSff`, `loadPalettes`).
//!
//! # RAM strategy (the Kurogane optimization)
//! Ikemen GO decodes *every* sprite to pixels at load time and uploads it to a
//! texture atlas. Large screenpacks (thousands of sprites) therefore pay the
//! full decoded size in RAM/VRAM up front.
//!
//! Kurogane parses **only the subheader table** at load time, keeps the file
//! bytes in a shared read-only mapping ([`SffData`]), and decodes sprite pixels
//! **on demand** via [`crate::cache::SpriteCache`] with an LRU byte budget.
//! Content, keys, palettes and quirks are identical to the Go engine; only
//! *when* pixels materialize differs.
//!
//! # Ported quirks (do not "fix" these)
//! * Duplicate sprite keys: **first wins**, later ones ignored (loadSff:1776).
//! * Linked sprites (size==0) share pixels/palette of `indexOfPrevious` but
//!   keep **their own** axis offsets (shareCopy, image.go:797).
//! * SFFv1 palette marker scan: backwards from `blockEnd-769` for `0x0C`.
//! * Palette entry 0 is forced transparent (`alpha=0`), others opaque.
//! * v2 palettes: color count rounded up to a power of two, clamped to
//!   [16,256]; alpha forced only for version `2.0.0.0` (`version[2] == 0`).
//! * `SetPxl` mismatch: sprites whose decoded length differs from `w*h` are
//!   silently dropped to blank (image.go:853-859).

use crate::decode;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Backing storage for an SFF file: read-only mapping when possible, else an
/// in-memory buffer. Cheap to clone (Arc).
#[derive(Clone)]
pub enum SffData {
    Mapped(std::sync::Arc<memmap2::Mmap>),
    Owned(std::sync::Arc<Vec<u8>>),
}

impl std::ops::Deref for SffData {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            SffData::Mapped(m) => m,
            SffData::Owned(v) => v,
        }
    }
}

/// Errors for format parsing.
#[derive(Debug)]
pub enum SffError {
    InvalidHeader,
    UnrecognizedVersion,
    UnknownColorDepth,
    UnknownFormat,
    Truncated { need: u64, have: u64 },
    Png(String),
    Msg(String),
}

impl fmt::Display for SffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SffError::InvalidHeader => write!(f, "Unrecognized SFF file, invalid header"),
            SffError::UnrecognizedVersion => write!(f, "Unrecognized SFF version"),
            SffError::UnknownColorDepth => write!(f, "Unknown color depth"),
            SffError::UnknownFormat => write!(f, "Unknown format"),
            SffError::Truncated { need, have } => {
                write!(f, "unexpected end of file (need {need}, have {have})")
            }
            SffError::Png(e) => write!(f, "png: {e}"),
            SffError::Msg(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for SffError {}

// Little-endian readers with truncation errors ──────────────────────────────
fn rd_u8(d: &[u8], o: usize) -> Result<u8, SffError> {
    d.get(o).copied().ok_or(trunc(o + 1, d.len()))
}
fn rd_u16(d: &[u8], o: usize) -> Result<u16, SffError> {
    let b = d.get(o..o + 2).ok_or(trunc(o + 2, d.len()))?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}
fn rd_i16(d: &[u8], o: usize) -> Result<i16, SffError> {
    Ok(rd_u16(d, o)? as i16)
}
fn rd_u32(d: &[u8], o: usize) -> Result<u32, SffError> {
    let b = d.get(o..o + 4).ok_or(trunc(o + 4, d.len()))?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn trunc(need: usize, have: usize) -> SffError {
    SffError::Truncated { need: need as u64, have: have as u64 }
}
fn slice_at(d: &[u8], o: usize, len: usize) -> Result<&[u8], SffError> {
    d.get(o..o + len).ok_or(trunc(o + len, d.len()))
}

/// Port of `SffHeader` (image.go:1519).
#[derive(Debug, Clone, Copy)]
pub struct SffHeader {
    /// `[verhi, verlo1, verlo2, verlo3]`.
    pub version: [u8; 4],
    pub first_sprite_header_offset: u32,
    pub first_palette_header_offset: u32,
    pub number_of_sprites: u32,
    pub number_of_palettes: u32,
    /// v2: palette data base offset (`lofs`).
    pub lofs: u32,
    /// v2: sprite data base offset (`tofs`).
    pub tofs: u32,
}

impl SffHeader {
    /// Port of `SffHeader.Read` (image.go:1527).
    ///
    /// Field offsets (verified byte-for-byte against real FF3/Elecbyte
    /// files — fight.sff, glyphs.sff, default-3x5.sff):
    ///
    /// * v1: NumberOfSprites u32 @20, FirstSpriteHeaderOffset u32 @24,
    ///   dummy u32 @28. (Header is 32 bytes + padding; subheaders are a
    ///   linked list — each one's u32 @0 points at the next subheader.)
    /// * v2: FIVE dummy u32s @16..35, then FirstSpriteHeaderOffset @36,
    ///   NumberOfSprites @40, FirstPaletteHeaderOffset @44,
    ///   NumberOfPalettes @48, lofs @52, dummy @56, tofs @60. (64-byte
    ///   header; subheader table is a fixed-stride-28 array at fsho.)
    pub fn read(d: &[u8]) -> Result<SffHeader, SffError> {
        if d.len() < 12 || &d[..12] != b"ElecbyteSpr\0" {
            return Err(SffError::InvalidHeader);
        }
        let verhi = d[15];
        let mut h = SffHeader {
            version: [verhi, d[14], d[13], d[12]],
            first_sprite_header_offset: 0,
            first_palette_header_offset: 0,
            number_of_sprites: 0,
            number_of_palettes: 0,
            lofs: 0,
            tofs: 0,
        };
        match verhi {
            1 => {
                h.number_of_sprites = rd_u32(d, 20)?;
                h.first_sprite_header_offset = rd_u32(d, 24)?;
            }
            2 => {
                h.first_sprite_header_offset = rd_u32(d, 36)?;
                h.number_of_sprites = rd_u32(d, 40)?;
                h.first_palette_header_offset = rd_u32(d, 44)?;
                h.number_of_palettes = rd_u32(d, 48)?;
                h.lofs = rd_u32(d, 52)?;
                h.tofs = rd_u32(d, 60)?;
            }
            _ => return Err(SffError::UnrecognizedVersion),
        }
        Ok(h)
    }
}

/// SFF usage context — port of `SffType`. Only `Char` changes behavior
/// (the legacy SFFv1 first-sprite palette path, loadSff:1758).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SffType {
    Char,
    Other,
}

/// One parsed subheader. Pixels are decoded on demand.
#[derive(Debug, Clone)]
pub struct SpriteEntry {
    pub group: u16,
    pub number: u16,
    /// Axis offsets (the sprite's own, even when linked).
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
    pub palidx: i32,
    pub coldepth: u8,
    /// Pixel data location inside the file.
    pub data_offset: u32,
    pub data_size: u32,
    /// v1: PCX bytes-per-line (0 = raw PCX, no RLE stripping).
    pub pcx_bpl: u32,
    /// v1: exclusive end of the RLE byte range (== palette location).
    pub v1_rle_end: u32,
    /// v2 format: 0 raw, 2 RLE8, 3 RLE5, 4 LZ5, 10 PNG-pal, 11/12 PNG-rgba.
    pub v2_format: u8,
    /// Linked sprite: index of the data-owning source entry.
    pub link: Option<usize>,
}

/// Decoded pixels for one sprite.
#[derive(Debug, Clone)]
pub enum Pixels {
    /// 8-bit palette indices.
    Indexed(Vec<u8>),
    /// RGBA (premultiplied — see [`crate::sprite::premultiply_rgba`]).
    Rgba(Vec<u8>),
}

impl Pixels {
    pub fn byte_len(&self) -> usize {
        match self {
            Pixels::Indexed(p) | Pixels::Rgba(p) => p.len(),
        }
    }
}

/// Palettes — port of the load-time parts of `PaletteList` (image.go:472).
#[derive(Debug, Clone, Default)]
pub struct PaletteList {
    /// 256-color palettes, packed `a<<24|b<<16|g<<8|r` (Go packing: RGBA in
    /// little-endian memory order).
    pub palettes: Vec<[u32; 256]>,
    /// v2 `(group, number)` → palette slot (PalTable).
    pub pal_table: HashMap<(u16, u16), i32>,
    /// v2 declared color counts (numcols).
    pub numcols: HashMap<(u16, u16), i32>,
}

impl PaletteList {
    fn new_pal(&mut self) -> i32 {
        let i = self.palettes.len() as i32;
        self.palettes.push([0; 256]);
        i
    }
    pub fn get(&self, i: i32) -> Option<&[u32; 256]> {
        self.palettes.get(i as usize)
    }
}

/// A parsed SFF — the cheap "handle"; decode pixels through [`Sff::decode`]
/// or (recommended) through [`crate::cache::SpriteCache`].
#[derive(Clone)]
pub struct Sff {
    data: SffData,
    pub header: SffHeader,
    pub sff_type: SffType,
    /// Subheaders in file order (index = Go's `spriteList` position).
    pub entries: Arc<Vec<SpriteEntry>>,
    /// `(group, number)` → entry index. First key wins (Go parity).
    pub index: Arc<HashMap<(u16, u16), usize>>,
    pub palettes: Arc<PaletteList>,
}

impl fmt::Debug for Sff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sff")
            .field("version", &self.header.version)
            .field("sprites", &self.entries.len())
            .field("palettes", &self.palettes.palettes.len())
            .finish()
    }
}

impl Sff {
    /// Port of `loadSff` (image.go:1686) — headers, subheader table and
    /// palettes; pixel data stays undecoded in the mapping.
    pub fn load(data: SffData, sff_type: SffType, is_act_pal: bool) -> Result<Sff, SffError> {
        let bytes: &[u8] = &data;
        let header = SffHeader::read(bytes)?;

        let mut palettes = PaletteList::default();
        if header.version[0] == 2 {
            load_palettes_v2(bytes, &header, &mut palettes)?;
        }

        let n = header.number_of_sprites as usize;
        let mut entries: Vec<SpriteEntry> = Vec::with_capacity(n);
        let mut index: HashMap<(u16, u16), usize> = HashMap::with_capacity(n);
        // Go `prev`: the last non-linked successfully-read sprite, and its palidx.
        let mut prev: Option<(usize, i32)> = None;

        let mut shofs: u64 = header.first_sprite_header_offset as u64;
        for i in 0..n {
            let is_v1 = header.version[0] == 1;
            let (group, number, x, y, size, next, link_u16) = if is_v1 {
                // Port of readHeader (image.go:944):
                // next u32 @0, size u32 @4, x i16 @8, y i16 @10, group u16 @12,
                // number u16 @14, indexOfPrevious u16 @16, ps u8 @18, pad → 32.
                (
                    rd_u16(bytes, shofs as usize + 12)?,
                    rd_u16(bytes, shofs as usize + 14)?,
                    rd_i16(bytes, shofs as usize + 8)?,
                    rd_i16(bytes, shofs as usize + 10)?,
                    rd_u32(bytes, shofs as usize + 4)?,
                    rd_u32(bytes, shofs as usize)?,
                    rd_u16(bytes, shofs as usize + 16)?,
                )
            } else {
                // Port of readHeaderV2 (image.go:1159) — TRUE field order:
                // group u16 @0, number u16 @2, Size(w,h) i16 @4/@6, axis
                // Offset i16 @8/@10, link u16 @12, format u8 @14, coldepth
                // u8 @15, ofs u32 @16, size u32 @20, palidx u16 @22, flags
                // u16 @24, rlelen u32 @26 (skipped; stride is 28).
                (
                    rd_u16(bytes, shofs as usize)?,        // group
                    rd_u16(bytes, shofs as usize + 2)?,    // number
                    rd_i16(bytes, shofs as usize + 8)?,    // x axis (Offset[0])
                    rd_i16(bytes, shofs as usize + 10)?,   // y axis (Offset[1])
                    rd_u32(bytes, shofs as usize + 20)?,   // size (0 = linked)
                    rd_u32(bytes, shofs as usize + 16)?,    // ofs (raw; base added in meta)
                    rd_u16(bytes, shofs as usize + 12)?,    // link (indexOfPrevious)
                )
            };

            let mut e = SpriteEntry {
                group,
                number,
                x,
                y,
                width: 0,
                height: 0,
                palidx: -1,
                coldepth: 0,
                data_offset: 0,
                data_size: 0,
                pcx_bpl: 0,
                v1_rle_end: 0,
                v2_format: 0,
                link: None,
            };

            if size == 0 {
                // Linked sprite — port of shareCopy (image.go:797).
                if (link_u16 as usize) < i {
                    let src = &entries[link_u16 as usize];
                    e.link = Some(link_u16 as usize);
                    e.width = src.width;
                    e.height = src.height;
                    e.coldepth = src.coldepth;
                    e.pcx_bpl = src.pcx_bpl;
                    e.v1_rle_end = src.v1_rle_end;
                    e.v2_format = src.v2_format;
                    e.data_offset = src.data_offset;
                    e.data_size = src.data_size;
                    // shareCopy: palidx only when still negative (v1 case; v2
                    // subheaders always provide palidx via meta below — but
                    // linked v2 sprites skip meta, so copy it here like Go).
                    if e.palidx < 0 {
                        e.palidx = src.palidx;
                    }
                } else {
                    // Go: "index out of range" fallback (loadSff:1753).
                    e.palidx = 0;
                }
            } else if is_v1 {
                read_v1_meta(bytes, &mut e, &header, shofs, size, next, prev, &mut palettes, sff_type)?;
                prev = Some((i, e.palidx));
            } else {
                read_v2_meta(bytes, &mut e, shofs, header.lofs, header.tofs)?;
                prev = Some((i, e.palidx));
            }

            // loadSff:1763 — isActPal resets the (0,0) sprite's palette.
            if is_act_pal && e.group == 0 && e.number == 0 {
                e.palidx = 0;
            }

            // Duplicate keys: first wins (loadSff:1776).
            index.entry((e.group, e.number)).or_insert(i);

            entries.push(e);
            shofs = if is_v1 {
                next as u64 // v1: follow the next-subheader chain
            } else {
                shofs + 28 // v2: fixed stride
            };
        }

        Ok(Sff {
            data,
            header,
            sff_type,
            entries: Arc::new(entries),
            index: Arc::new(index),
            palettes: Arc::new(palettes),
        })
    }

    /// Load from a file path (memory-mapped, read-only).
    pub fn load_path<P: AsRef<std::path::Path>>(
        path: P,
        sff_type: SffType,
        is_act_pal: bool,
    ) -> Result<Sff, SffError> {
        let f = std::fs::File::open(path.as_ref())
            .map_err(|e| SffError::Msg(format!("open: {e}")))?;
        let mmap = unsafe { memmap2::Mmap::map(&f) }
            .map_err(|e| SffError::Msg(format!("mmap: {e}")))?;
        Self::load(SffData::Mapped(Arc::new(mmap)), sff_type, is_act_pal)
    }

    /// `(group, number)` → entry — port of `GetSprite` (image.go:2312).
    pub fn entry(&self, group: u16, number: u16) -> Option<&SpriteEntry> {
        self.index.get(&(group, number)).map(|&i| &self.entries[i])
    }

    /// Decode one sprite's pixels on demand — port of `Sprite.read` (v1) and
    /// `Sprite.readV2` (v2) against the mapped bytes.
    pub fn decode(&self, e: &SpriteEntry) -> Result<Pixels, SffError> {
        // Follow links to the data-owning entry (shareCopy semantics).
        let mut src = e;
        let mut depth = 0;
        while let Some(l) = src.link {
            src = &self.entries[l];
            depth += 1;
            if depth > 16 {
                break; // corrupt chain guard (Go resolves at load; we resolve at use)
            }
        }
        let bytes: &[u8] = &self.data;
        if self.header.version[0] == 1 {
            self.decode_v1(bytes, src)
        } else {
            self.decode_v2(bytes, src)
        }
    }

    /// Port of the pixel half of `Sprite.read` (image.go:1045-1157) — SFFv1.
    fn decode_v1(&self, bytes: &[u8], e: &SpriteEntry) -> Result<Pixels, SffError> {
        let rle_start = e.data_offset as usize + 128; // PCX header is 128 bytes
        let rle_end = e.v1_rle_end as usize;
        let rle = if rle_end > rle_start {
            slice_at(bytes, rle_start, rle_end - rle_start)?
        } else {
            &[][..]
        };
        let px = decode::rle_pcx_decode(rle, e.width as u32, e.height as u32, e.pcx_bpl);
        // SetPxl (image.go:853): drop length mismatches silently → blank sprite.
        if px.len() != (e.width as usize) * (e.height as usize) {
            return Ok(Pixels::Indexed(Vec::new()));
        }
        Ok(Pixels::Indexed(px))
    }

    /// Port of `Sprite.readV2` (image.go:1358).
    fn decode_v2(&self, bytes: &[u8], e: &SpriteEntry) -> Result<Pixels, SffError> {
        let off = e.data_offset as usize;
        let size = e.data_size as usize;
        let w = e.width as u32;
        let h = e.height as u32;
        match e.v2_format {
            0 => {
                // Raw (image.go:1365-1378).
                let raw = slice_at(bytes, off, size)?;
                match e.coldepth {
                    8 => Ok(Pixels::Indexed(raw.to_vec())),
                    24 => {
                        // SetRaw depth 24 → RGBA with opaque alpha.
                        let mut rgba = Vec::with_capacity(raw.len() / 3 * 4);
                        for c in raw.chunks_exact(3) {
                            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
                        }
                        Ok(Pixels::Rgba(rgba))
                    }
                    32 => Ok(Pixels::Rgba(crate::sprite::premultiply_rgba(raw.to_vec()))),
                    _ => Err(SffError::UnknownColorDepth),
                }
            }
            2 | 3 | 4 => {
                // Encoded: data starts with a 4-byte length prefix
                // (image.go:1381-1394). Go clamps datasize to ≥ 4.
                let len = if size < 4 { 0 } else { size - 4 };
                let enc = slice_at(bytes, off + 4, len)?;
                let px = match e.v2_format {
                    2 => decode::rle8_decode(enc, w, h),
                    3 => decode::rle5_decode(enc, w, h),
                    _ => decode::lz5_decode(enc, w, h),
                };
                Ok(Pixels::Indexed(px))
            }
            10 => {
                // PNG, paletted → keep indices (image.go:1404-1412). Go reads
                // the stream unbounded from offset+4; the decoder stops at IEND.
                let stream = bytes.get(off + 4..).ok_or(trunc(off + 4, bytes.len()))?;
                let mut reader = png::Decoder::new(std::io::Cursor::new(stream))
                    .read_info()
                    .map_err(|e| SffError::Png(e.to_string()))?;
                let info = reader.info().clone();
                let mut buf = vec![0u8; reader.output_buffer_size()];
                reader.next_frame(&mut buf).map_err(|e| SffError::Png(e.to_string()))?;
                if info.color_type == png::ColorType::Indexed {
                    buf.truncate((w as usize) * (h as usize));
                    Ok(Pixels::Indexed(buf))
                } else {
                    // Go: non-paletted in format 10 yields no pixels.
                    Ok(Pixels::Indexed(Vec::new()))
                }
            }
            11 | 12 => {
                // PNG → RGBA, premultiplied like Go's NRGBA→RGBA conversion
                // through SetRaw (image.go:1413-1425).
                let stream = bytes.get(off + 4..).ok_or(trunc(off + 4, bytes.len()))?;
                let mut reader = png::Decoder::new(std::io::Cursor::new(stream))
                    .read_info()
                    .map_err(|e| SffError::Png(e.to_string()))?;
                let mut buf = vec![0u8; reader.output_buffer_size()];
                reader.next_frame(&mut buf).map_err(|e| SffError::Png(e.to_string()))?;
                Ok(Pixels::Rgba(crate::sprite::premultiply_rgba(buf)))
            }
            _ => Err(SffError::UnknownFormat),
        }
    }
}

/// SFFv1 metadata: PCX header + palette bookkeeping — port of the header half
/// of `Sprite.read` (image.go:1045-1157).
fn read_v1_meta(
    bytes: &[u8],
    e: &mut SpriteEntry,
    _header: &SffHeader,
    shofs: u64,
    size: u32,
    next_subheader: u32,
    prev: Option<(usize, i32)>,
    palettes: &mut PaletteList,
    sff_type: SffType,
) -> Result<(), SffError> {
    // `ps` byte (image.go:1051) sits at subheader+18.
    let ps = rd_u8(bytes, shofs as usize + 18)?;
    let palette_same = ps != 0 && prev.is_some();

    // readPcxHeader (image.go:969) at the sprite data start = shofs+32.
    let off = (shofs + 32) as usize;
    let encoding = rd_u8(bytes, off + 2)?;
    let bpp = rd_u8(bytes, off + 3)?;
    if bpp != 8 {
        return Err(SffError::Msg(format!(
            "Invalid PCX color depth: expected 8-bit, got {bpp}"
        )));
    }
    // readPcxHeader (image.go:969): rect is [4]uint16 at +4 (xmin, ymin, xmax,
    // ymax) — four consecutive little-endian u16 fields, NOT byte offsets.
    let xmin = rd_u16(bytes, off + 4)?;
    let ymin = rd_u16(bytes, off + 6)?;
    let xmax = rd_u16(bytes, off + 8)?;
    let ymax = rd_u16(bytes, off + 10)?;
    let bpl = rd_u16(bytes, off + 66)?;

    e.width = xmax.wrapping_sub(xmin).wrapping_add(1);
    e.height = ymax.wrapping_sub(ymin).wrapping_add(1);
    e.pcx_bpl = if encoding == 1 { bpl as u32 } else { 0 };
    e.data_offset = off as u32;
    e.data_size = size;
    e.coldepth = 8;

    let pcx_data_start = off + 128;
    let next_sub = next_subheader as usize;

    // isCharFirstSprite (loadSff:1758).
    let is_char_first =
        sff_type == SffType::Char && (prev.is_none() || (e.group == 0 && e.number == 0));

    // Palette location logic (image.go:1062-1127).
    let (palette_offset, rle_end) = if is_char_first {
        let mut datasize = size as usize;
        if next_sub > off {
            datasize = next_sub - off;
        }
        let pal = off + datasize.saturating_sub(768);
        (pal, pal)
    } else {
        let block_end = if next_sub > off { next_sub } else { off + size as usize };
        if palette_same {
            (block_end, block_end)
        } else {
            // Backward scan for the 0x0C marker from blockEnd-769 (image.go:1098).
            let scan_start = block_end.saturating_sub(769);
            let mut found: Option<usize> = None;
            if scan_start >= pcx_data_start {
                let region = bytes.get(pcx_data_start..=scan_start).ok_or(trunc(scan_start + 1, bytes.len()))?;
                for (idx, &b) in region.iter().enumerate().rev() {
                    if b == 0x0C {
                        found = Some(pcx_data_start + idx);
                        break;
                    }
                }
            }
            let pal = found.unwrap_or_else(|| block_end.saturating_sub(769));
            (pal, pal)
        }
    };
    e.v1_rle_end = rle_end as u32;

    // Palette bookkeeping (image.go:1129-1154).
    if palette_same {
        e.palidx = prev.map(|(_, p)| p).unwrap_or(-1);
        if e.palidx < 0 {
            e.palidx = palettes.new_pal();
        }
    } else {
        let pi = palettes.new_pal();
        e.palidx = pi;
        let pal = palettes.palettes.last_mut().unwrap();
        let mut pos = palette_offset;
        if !is_char_first {
            pos += 1; // skip the 0x0C marker byte (image.go:1141)
        }
        for i in 0..256usize {
            let rgb = slice_at(bytes, pos + i * 3, 3)?;
            let alpha: u32 = if i == 0 { 0 } else { 255 };
            pal[i] = alpha << 24 | (rgb[2] as u32) << 16 | (rgb[1] as u32) << 8 | (rgb[0] as u32);
        }
    }
    Ok(())
}

/// SFFv2 metadata — port of `readHeaderV2` field decoding (image.go:1159).
fn read_v2_meta(
    bytes: &[u8],
    e: &mut SpriteEntry,
    shofs: u64,
    lofs: u32,
    tofs: u32,
) -> Result<(), SffError> {
    let o = shofs as usize;
    // readHeaderV2 (image.go:1159) — the reads total EXACTLY 28 bytes:
    // group u16 @0, number u16 @2, Size(w,h) i16 @4/@6, axis Offset i16
    // @8/@10, link u16 @12, format u8 @14, coldepth u8 @15, ofs u32 @16,
    // size u32 @20, palidx u16 @24, flags u16 @26. (No rlelen tail — the
    // stride and the read length coincide.)
    e.width = rd_u16(bytes, o + 4)?;
    e.height = rd_u16(bytes, o + 6)?;
    let format = rd_u8(bytes, o + 14)?;
    e.coldepth = rd_u8(bytes, o + 15)?;
    let mut ofs = rd_u32(bytes, o + 16)?;
    e.data_size = rd_u32(bytes, o + 20)?;
    let palidx = rd_u16(bytes, o + 24)?;
    let flags = rd_u16(bytes, o + 26)?;
    if flags & 1 == 0 {
        ofs += lofs;
    } else {
        ofs += tofs;
    }
    e.v2_format = format;
    e.data_offset = ofs;
    e.palidx = palidx as i32;
    Ok(())
}

/// Port of `loadPalettes` (image.go:2139) — SFFv2 palette table.
fn load_palettes_v2(
    bytes: &[u8],
    header: &SffHeader,
    palettes: &mut PaletteList,
) -> Result<(), SffError> {
    let mut unique: HashMap<(u16, u16), i32> = HashMap::new();
    for i in 0..header.number_of_palettes as usize {
        let o = header.first_palette_header_offset as usize + i * 16;
        let gn0 = rd_u16(bytes, o)?;
        let gn1 = rd_u16(bytes, o + 2)?;
        let gn2 = rd_u16(bytes, o + 4)?;
        let link = rd_u16(bytes, o + 6)?;
        let ofs = rd_u32(bytes, o + 8)?;
        let pl_size = rd_u32(bytes, o + 12)?;

        // Resolve the palette contents (image.go:2170-2189), and the index it
        // registers under: duplicate → old, linked → link, fresh → i.
        let is_dup = unique.contains_key(&(gn0, gn1));
        let resolved: Option<[u32; 256]> = if is_dup {
            palettes.get(unique[&(gn0, gn1)]).copied()
        } else if pl_size == 0 {
            palettes.get(link as i32).copied()
        } else {
            Some(read_palette_v2(bytes, header, ofs, pl_size)?)
        };

        // SetSource(i, pal) — slot i gets the resolved palette.
        if palettes.palettes.len() <= i {
            palettes.palettes.resize(i + 1, [0; 256]);
        }
        if let Some(p) = resolved {
            palettes.palettes[i] = p;
        }
        // uniquePals[key] = idx; PalTable[key] = i (image.go:2190-2192).
        let idx: i32 = if is_dup {
            unique[&(gn0, gn1)]
        } else if pl_size == 0 {
            link as i32
        } else {
            i as i32
        };
        unique.insert((gn0, gn1), idx);
        palettes.pal_table.insert((gn0, gn1), i as i32);
        palettes.numcols.insert((gn0, gn1), gn2 as i32);
    }
    Ok(())
}

/// Port of `ReadPalette` (image.go:2215) — one SFFv2 palette.
fn read_palette_v2(
    bytes: &[u8],
    header: &SffHeader,
    ofs: u32,
    pl_size: u32,
) -> Result<[u32; 256], SffError> {
    let raw_count = (pl_size / 4) as usize;
    // Depth: next power of two ≥ rawCount, clamped to [16, 256] (image.go:2224-2235).
    let mut depth = 1usize;
    while depth < raw_count {
        depth *= 2;
    }
    depth = depth.clamp(16, 256);
    let base = (header.lofs + ofs) as usize;
    let mut pal = [0u32; 256];
    for (i, c) in pal.iter_mut().enumerate().take(depth) {
        let mut rgba = [0u8; 4];
        if i < raw_count {
            rgba.copy_from_slice(slice_at(bytes, base + i * 4, 4)?);
        }
        if header.version[2] == 0 {
            // 2.0.0.0 only: force alpha — index 0 transparent, others opaque.
            rgba[3] = if i == 0 { 0 } else { 255 };
        }
        *c = (rgba[3] as u32) << 24
            | (rgba[2] as u32) << 16
            | (rgba[1] as u32) << 8
            | rgba[0] as u32;
    }
    Ok(pal)
}
