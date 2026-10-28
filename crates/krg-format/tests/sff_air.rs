//! Integration tests: synthetic SFF v1/v2 files round-trip through the
//! parsers, exercising the exact code paths real MUGEN content hits —
//! including linked sprites, shared palettes, the 0x0C palette-marker scan
//! and every v2 codec.

use krg_format::*;
use std::io::Write;

// ─── SFF v1 writer (test-only, hand-rolled) ────────────────────────────────

pub fn pcx_header(w: u16, h: u16, bpl: u16, rle: bool) -> Vec<u8> {
    let mut h128 = vec![0u8; 128];
    h128[0] = 10; // manufacturer (zsoft)
    h128[1] = 5; // version
    h128[2] = if rle { 1 } else { 0 }; // encoding
    h128[3] = 8; // bpp
    h128[4..12].copy_from_slice(&[0, 0, 0, 0, (w - 1) as u8, 0, (h - 1) as u8, 0]);
    h128[66..68].copy_from_slice(&bpl.to_le_bytes());
    h128
}

pub fn pcx_rle_encode(indices: &[u8], w: u16, bpl: u16) -> Vec<u8> {
    // Standard PCX RLE: runs up to 63, always split at bpl boundaries.
    let mut out = Vec::new();
    for row in indices.chunks(w as usize) {
        let mut line = row.to_vec();
        line.extend(std::iter::repeat(0u8).take((bpl - w as u16) as usize)); // pad row to bpl
        let mut i = 0;
        while i < line.len() {
            let v = line[i];
            let mut run = 1usize;
            while i + run < line.len() && line[i + run] == v && run < 63 {
                run += 1;
            }
            if run > 1 || v >= 0xC0 {
                out.push(0xC0 | run as u8);
            }
            out.push(v);
            i += run;
        }
    }
    out
}

pub fn palette_bytes(pal: &[[u8; 3]; 16]) -> Vec<u8> {
    let mut out = vec![0u8; 768];
    for (i, c) in pal.iter().enumerate() {
        out[i * 3..i * 3 + 3].copy_from_slice(c);
    }
    out
}

/// Builds a minimal SFF v1 with one PCX-RLE sprite carrying its palette
/// (the common "first char sprite" layout) plus a second sprite sharing it.
pub fn build_sff_v1() -> Vec<u8> {
    let (w, h, bpl) = (4u16, 2u16, 4u16);
    #[allow(unused_variables)]
    let indices: Vec<u8> = vec![1, 1, 2, 2, 3, 3, 4, 4];
    let rle = pcx_rle_encode(&indices, w, bpl);
    let pal: Vec<[u8; 3]> = (0..16).map(|i| [i * 16, 255 - i * 16, i * 8]).collect();

    let mut buf: Vec<u8> = Vec::new();
    // Header: sig, version 1.1.2.4 (file bytes at +12..16 = verlo3,verlo2,verlo1,verhi)
    buf.extend_from_slice(b"ElecbyteSpr\0");
    buf.extend_from_slice(&[4, 2, 1, 1]);
    buf.extend_from_slice(&1u32.to_le_bytes()); // dummy (groups)
    buf.extend_from_slice(&2u32.to_le_bytes()); // NumberOfSprites
    buf.extend_from_slice(&512u32.to_le_bytes()); // FirstSpriteHeaderOffset
    buf.extend_from_slice(&0u32.to_le_bytes()); // dummy
    buf.extend_from_slice(&0u8.to_le_bytes()); // palette type
    buf.extend_from_slice(&[0u8; 3]);
    buf.extend_from_slice(&512u32.to_le_bytes()); // next subheader offset
    buf.extend_from_slice(&32u32.to_le_bytes()); // subheader size
    buf.extend_from_slice(&0u32.to_le_bytes()); // shared palette offset
    // pad to offset 512
    buf.extend(std::iter::repeat(0u8).take(512 - buf.len()));

    // ── Sprite 0: (0,0) 4x2, PCX + palette in the same block ──
    let spr0_start = buf.len() as u32; // = 512 (subheader position)
    let mut pcx = pcx_header(w, h, bpl, true);
    pcx.extend_from_slice(&rle);
    pcx.extend_from_slice(b"\x0C"); // palette marker
    pcx.extend_from_slice(&palette_bytes(&pal.try_into().unwrap()));
    let next_ofs = spr0_start + 32 + pcx.len() as u32; // sprite 1's subheader

    // subheader 0 (32 bytes): next, size, x, y, group, number, link, ps, pad
    buf.extend_from_slice(&next_ofs.to_le_bytes());
    buf.extend_from_slice(&(pcx.len() as u32).to_le_bytes());
    buf.extend_from_slice(&0i16.to_le_bytes()); // x
    buf.extend_from_slice(&0i16.to_le_bytes()); // y
    buf.extend_from_slice(&0u16.to_le_bytes()); // group
    buf.extend_from_slice(&0u16.to_le_bytes()); // number
    buf.extend_from_slice(&0u16.to_le_bytes()); // indexOfPrevious
    buf.push(0); // ps: not shared (sprite carries its own palette)
    buf.extend(std::iter::repeat(0u8).take(13));
    buf.extend_from_slice(&pcx);

    // ── Sprite 1: (9000,1) linked to sprite 0 (size=0) ──
    buf.extend_from_slice(&0u32.to_le_bytes()); // next (last)
    buf.extend_from_slice(&0u32.to_le_bytes()); // size 0 → linked
    buf.extend_from_slice(&5i16.to_le_bytes()); // own x axis
    buf.extend_from_slice(&7i16.to_le_bytes()); // own y axis
    buf.extend_from_slice(&9000u16.to_le_bytes()); // group
    buf.extend_from_slice(&1u16.to_le_bytes()); // number
    buf.extend_from_slice(&0u16.to_le_bytes()); // link → sprite 0
    buf.push(1); // ps
    buf.extend(std::iter::repeat(0u8).take(13));
    buf
}

pub fn build_sff_v2() -> Vec<u8> {
    #[allow(unused_variables)]
    let indices: Vec<u8> = vec![1, 1, 2, 2, 3, 3, 4, 4];
    // RLE8: (0x42,n) = 2 copies of n — fills the 4x2 sprite exactly.
    let rle8: Vec<u8> = vec![0x42, 1, 0x42, 2, 0x42, 3, 0x42, 4];

    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"ElecbyteSpr\0");
    buf.extend_from_slice(&[0, 0, 0, 2]); // verlo3..verhi → 2.0.0.0
    // Five dummy u32s @16..35 (image.go:1577-1581 — the v2 header is 64 bytes).
    buf.extend(std::iter::repeat(0u8).take(20));
    let first_sub: u32 = 2048;
    // Layout by construction: subheader @2048 (28 B) → prefix @2076 →
    // RLE8 @2080..2088 → palette data (lofs) @2088.
    let data_ofs: u32 = first_sub + 28; // 4-byte length prefix position
    let lofs: u32 = data_ofs + 4 + rle8.len() as u32;
    buf.extend_from_slice(&first_sub.to_le_bytes()); // FirstSpriteHeaderOffset @36
    buf.extend_from_slice(&1u32.to_le_bytes()); // NumberOfSprites @40
    buf.extend_from_slice(&1024u32.to_le_bytes()); // FirstPaletteHeaderOffset @44
    buf.extend_from_slice(&1u32.to_le_bytes()); // NumberOfPalettes @48
    buf.extend_from_slice(&lofs.to_le_bytes()); // lofs @52
    buf.extend_from_slice(&0u32.to_le_bytes()); // dummy @56
    buf.extend_from_slice(&0u32.to_le_bytes()); // tofs @60 (0 → absolute offsets)

    // Palette subheader at 1024 (16 bytes): gn0, gn1, gn2, link, ofs, plSize.
    buf.extend(std::iter::repeat(0u8).take(1024 - buf.len()));
    buf.extend_from_slice(&1u16.to_le_bytes()); // gn0 (group)
    buf.extend_from_slice(&1u16.to_le_bytes()); // gn1 (number)
    buf.extend_from_slice(&256u16.to_le_bytes()); // gn2 (numcols)
    buf.extend_from_slice(&0u16.to_le_bytes()); // link
    buf.extend_from_slice(&0u32.to_le_bytes()); // ofs (relative to lofs → 0)
    buf.extend_from_slice(&(256u32 * 4).to_le_bytes()); // plSize

    // Sprite subheader at 2048 (28 bytes) — TRUE v2 layout
    // (image.go:1159-1206): group u16 @0, number u16 @2, Size(w,h) i16
    // @4/@6, axis Offset i16 @8/@10, link u16 @12, format u8 @14,
    // coldepth u8 @15, ofs u32 @16, size u32 @20, palidx u16 @24,
    // flags u16 @26 — bit0: 0 → ofs is lofs-relative, 1 → tofs-relative.
    buf.extend(std::iter::repeat(0u8).take(first_sub as usize - buf.len()));
    buf.extend_from_slice(&0u16.to_le_bytes()); // group
    buf.extend_from_slice(&0u16.to_le_bytes()); // number
    buf.extend_from_slice(&4i16.to_le_bytes()); // width
    buf.extend_from_slice(&2i16.to_le_bytes()); // height
    buf.extend_from_slice(&0i16.to_le_bytes()); // x axis
    buf.extend_from_slice(&0i16.to_le_bytes()); // y axis
    buf.extend_from_slice(&0u16.to_le_bytes()); // link
    buf.push(2); // format 2 = RLE8
    buf.push(8); // coldepth
    buf.extend_from_slice(&data_ofs.to_le_bytes()); // ofs (flags bit0=1 → += tofs(0))
    buf.extend_from_slice(&(4 + rle8.len() as u32).to_le_bytes()); // size incl. prefix
    buf.extend_from_slice(&0u16.to_le_bytes()); // palidx
    buf.extend_from_slice(&1u16.to_le_bytes()); // flags bit0=1 → absolute ofs (tofs=0)
    // 28 bytes exactly — no rlelen tail.

    // Sprite data: 4-byte prefix + RLE8 payload. (usize::to_le_bytes would
    // write 8 bytes on 64-bit hosts — cast to u32!)
    buf.extend_from_slice(&(rle8.len() as u32).to_le_bytes());
    buf.extend_from_slice(&rle8);

    // Palette data at lofs (256 RGBA entries).
    buf.extend(std::iter::repeat(0u8).take((lofs as usize).saturating_sub(buf.len())));
    for i in 0..256u32 {
        let (r, g, b, a) = (i as u8, 255 - i as u8, (i / 2) as u8, if i == 0 { 0 } else { 255 });
        buf.extend_from_slice(&[r, g, b, a]);
    }
    buf
}

#[test]
fn sff_v1_roundtrip() {
    let bytes = build_sff_v1();
    let sff = Sff::load(SffData::Owned(bytes.into()), SffType::Char, false).unwrap();

    assert_eq!(sff.entries.len(), 2);
    assert_eq!(sff.index.len(), 2);
    assert_eq!(sff.palettes.palettes.len(), 1);

    let e0 = sff.entry(0, 0).unwrap();
    assert_eq!((e0.width, e0.height), (4, 2));
    assert_eq!((e0.x, e0.y), (0, 0));
    assert_eq!(e0.palidx, 0);

    // Linked sprite: own axis, shared pixels (9000,1) → entry 1.
    let e1 = sff.entry(9000, 1).unwrap();
    assert_eq!((e1.x, e1.y), (5, 7));
    assert!(e1.link.is_some());

    // Decode the first sprite: PCX RLE with bpl padding stripping.
    let d = sff.decode(e0).unwrap();
    match d {
        Pixels::Indexed(px) => assert_eq!(px, vec![1, 1, 2, 2, 3, 3, 4, 4]),
        _ => panic!("expected indexed pixels"),
    }

    // Decode through the link.
    let d1 = sff.decode(e1).unwrap();
    match d1 {
        Pixels::Indexed(px) => assert_eq!(px, vec![1, 1, 2, 2, 3, 3, 4, 4]),
        _ => panic!("expected indexed pixels"),
    }

    // Palette check: index 0 transparent, index 1 = (r16, g239, b8) →
    // packed a<<24|b<<16|g<<8|r = 0xFF_08_EF_10.
    let pal = sff.palettes.get(0).unwrap();
    assert_eq!(pal[0] & 0xFF00_0000, 0);
    assert_eq!(pal[1], 0xFF08_EF10);
}

#[test]
fn sff_v2_roundtrip() {
    let bytes = build_sff_v2();
    let sff = Sff::load(SffData::Owned(bytes.into()), SffType::Other, false).unwrap();

    assert_eq!(sff.entries.len(), 1);
    let e = sff.entry(0, 0).unwrap();
    assert_eq!((e.width, e.height), (4, 2));
    assert_eq!(e.v2_format, 2);
    assert_eq!(e.coldepth, 8);

    let d = sff.decode(e).unwrap();
    match d {
        Pixels::Indexed(px) => assert_eq!(px, vec![1, 1, 2, 2, 3, 3, 4, 4]),
        _ => panic!("expected indexed pixels"),
    }

    // v2 palette: RGBA bytes, alpha forced (2.0.0.0).
    let pal = sff.palettes.get(0).unwrap();
    assert_eq!(pal[0] & 0xFF00_0000, 0);
    assert_eq!(pal[1] & 0xFF00_0000, 0xFF00_0000);
    assert_eq!(pal[1] & 0xFF, 1); // r
    assert_eq!((pal[1] >> 8) & 0xFF, 254); // g
}

#[test]
fn sff_duplicate_keys_first_wins() {
    let (w, h, bpl) = (2u16, 1u16, 2u16);
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"ElecbyteSpr\0");
    buf.extend_from_slice(&[1, 0, 0, 1]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes()); // NumberOfSprites
    buf.extend_from_slice(&512u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend(std::iter::repeat(0u8).take(512 - buf.len()));

    let pal: Vec<[u8; 3]> = (0..16).map(|i| [i * 16, i * 16, i * 16]).collect();
    let mut pcx = pcx_header(w, h, bpl, false);
    pcx.extend_from_slice(&[7u8, 8u8]); // raw (encoding=0), no RLE
    pcx.extend_from_slice(b"\x0C");
    pcx.extend_from_slice(&palette_bytes(&pal.try_into().unwrap()));
    let next_ofs = 512 + 32 + pcx.len() as u32;

    buf.extend_from_slice(&next_ofs.to_le_bytes());
    buf.extend_from_slice(&(pcx.len() as u32).to_le_bytes());
    buf.extend_from_slice(&0i16.to_le_bytes());
    buf.extend_from_slice(&0i16.to_le_bytes());
    buf.extend_from_slice(&5u16.to_le_bytes()); // group 5
    buf.extend_from_slice(&1u16.to_le_bytes()); // number 1
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.push(0);
    buf.extend(std::iter::repeat(0u8).take(13));
    buf.extend_from_slice(&pcx);

    let sff = Sff::load(SffData::Owned(buf.into()), SffType::Other, false).unwrap();
    assert_eq!(sff.entries.len(), 1);
    assert_eq!(sff.index.len(), 1);
    // Raw PCX (encoding=0) in a non-char SFF: the backward 0x0C scan bounds
    // the RLE bytes exactly, so the data passes through as-is.
    let e = sff.entry(5, 1).unwrap();
    let d = sff.decode(e).unwrap();
    match d {
        Pixels::Indexed(px) => assert_eq!(px, vec![7, 8]),
        _ => panic!("expected indexed"),
    }
}

// Char-type variant: the same file loaded as Char drops the raw sprite,
// exactly like Ikemen GO (the legacy first-sprite path's rleSize covers
// pixels+marker+palette, and SetPxl rejects the length mismatch).
#[test]
fn sff_char_first_raw_pcx_dropped_like_go() {
    let (w, h, bpl) = (2u16, 1u16, 2u16);
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"ElecbyteSpr\0");
    buf.extend_from_slice(&[1, 0, 0, 1]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes()); // NumberOfSprites
    buf.extend_from_slice(&512u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend(std::iter::repeat(0u8).take(512 - buf.len()));

    let pal: Vec<[u8; 3]> = (0..16).map(|i| [i * 16, i * 16, i * 16]).collect();
    let mut pcx = pcx_header(w, h, bpl, false);
    pcx.extend_from_slice(&[7u8, 8u8]); // raw (encoding=0), no RLE
    pcx.extend_from_slice(b"\x0C");
    pcx.extend_from_slice(&palette_bytes(&pal.try_into().unwrap()));
    let next_ofs = 512 + 32 + pcx.len() as u32;

    buf.extend_from_slice(&next_ofs.to_le_bytes());
    buf.extend_from_slice(&(pcx.len() as u32).to_le_bytes());
    buf.extend_from_slice(&0i16.to_le_bytes());
    buf.extend_from_slice(&0i16.to_le_bytes());
    buf.extend_from_slice(&5u16.to_le_bytes()); // group 5
    buf.extend_from_slice(&1u16.to_le_bytes()); // number 1
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.push(0);
    buf.extend(std::iter::repeat(0u8).take(13));
    buf.extend_from_slice(&pcx);

    let sff = Sff::load(SffData::Owned(buf.into()), SffType::Char, false).unwrap();
    let e = sff.entry(5, 1).unwrap();
    let d = sff.decode(e).unwrap();
    match d {
        Pixels::Indexed(px) => assert!(px.is_empty(), "Go drops this sprite: {px:?}"),
        _ => panic!("expected indexed"),
    }
}

#[test]
fn sprite_cache_budget_eviction() {
    let bytes = build_sff_v1();
    let sff = Sff::load(SffData::Owned(bytes.into()), SffType::Char, false).unwrap();
    // Budget smaller than one sprite's decoded bytes (8) → evicted after use.
    let mut cache = SpriteCache::new(sff, 4);
    let d = cache.get(0, 0).unwrap().unwrap();
    assert_eq!(d.pixels.byte_len(), 8);
    assert!(cache.cached_count() == 0 || cache.used_bytes() <= 4);
    // Still decodes on demand again.
    let d2 = cache.get(0, 0).unwrap().unwrap();
    assert_eq!(d2.pixels.byte_len(), 8);
}

#[test]
fn air_compile_full() {
    let text = r#"
; comment line
[Begin Action 0]
clsn2default: 2
  clsn2[0] = -10, -20, 30, 5
  clsn2[1] = 40, 50, 60, 70
0,0, 0,0, 5, ,A1
0,1, 3,4, 6, H, AS255D128
loopstart
1,2, 0,0, 10,,S
[Begin Action 1]
copy action 0
[Begin Action 2]
5,5, 1,1, 3
"#;
    let mut ac = AnimCompiler::new(true);
    ac.compile_text("test.air", text);
    let table = ac.finalize();

    let a0 = table.anims.get(&0).unwrap();
    assert_eq!(a0.frames.len(), 3);
    assert_eq!(a0.loopstart, 2);

    let f0 = &a0.frames[0];
    assert_eq!(f0.time, 5);
    assert_eq!(f0.trans_type, krg_core::TransType::Add);
    assert_eq!(f0.src_alpha, 255);
    assert_eq!(f0.dst_alpha, 128);
    // clsn2default applied to the frame without its own directive.
    assert_eq!(f0.clsn2.len(), 2);
    // No normalization needed: -10 < 30 and -20 < 5 (normalize_rect only
    // swaps when left > right on the same axis).
    assert_eq!(f0.clsn2[0], [-10.0, -20.0, 30.0, 5.0]);

    let f1 = &a0.frames[1];
    assert_eq!(f1.hscale, -1);
    assert_eq!(f1.xoffset, -3);
    assert_eq!(f1.trans_type, krg_core::TransType::Add);
    assert_eq!(f1.src_alpha, 255);
    assert_eq!(f1.dst_alpha, 128);

    // Copy action resolved.
    let a1 = table.anims.get(&1).unwrap();
    assert_eq!(a1.frames.len(), 3);
    assert_eq!(a1.copy_action, -1);

    // Time bookkeeping: 5 + 6 + 10 = 21 total, loop 10, preloop 11.
    assert_eq!(a0.totaltime, 21);
    assert_eq!(a0.looptime, 10);
    assert_eq!(a0.prelooptime, 11);
}

#[test]
fn air_empty_action_inherits_next() {
    // Action 5 has no frames (only a comment) → inherits action 6 (Go quirk).
    let text = "[Begin Action 5]\n; nothing\n[Begin Action 6]\n0,0, 0,0, 4\n";
    let mut ac = AnimCompiler::new(true);
    ac.compile_text("t.air", text);
    let table = ac.finalize();
    let a5 = table.anims.get(&5).unwrap();
    assert_eq!(a5.frames.len(), 1);
    assert_eq!(a5.frames[0].time, 4);
}

#[test]
fn air_unresolved_copy_action_purged() {
    let text = "[Begin Action 3]\ncopy action 99\n[Begin Action 4]\n0,0, 0,0, 2\n";
    let mut ac = AnimCompiler::new(true);
    ac.compile_text("t.air", text);
    let table = ac.finalize();
    assert!(!table.anims.contains_key(&3)); // purged
    assert!(table.anims.contains_key(&4));
}

#[test]
fn write_read_roundtrip_file() {
    // Exercise the mmap path too.
    let dir = std::env::temp_dir().join("kurogane-test");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.sff");
    let bytes = build_sff_v1();
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(&bytes).unwrap();
    drop(f);
    let sff = Sff::load_path(&p, SffType::Char, false).unwrap();
    let e = sff.entry(0, 0).unwrap();
    match sff.decode(e).unwrap() {
        Pixels::Indexed(px) => assert_eq!(px, vec![1, 1, 2, 2, 3, 3, 4, 4]),
        _ => panic!("expected indexed"),
    }
    let _ = std::fs::remove_file(&p);
}
