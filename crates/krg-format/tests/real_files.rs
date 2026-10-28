//! Regression tests against REAL content files (copied verbatim from the
//! official Ikemen GO v1.0.0 release). These caught the v2 header offset bug
//! (every field 4 bytes early → "unexpected end of file (need 1886680172)")
//! that synthetic round-trip tests could never catch — the builder shared
//! the parser's misunderstanding.
//!
//! * `default-3x5.sff` / `default-3x5-bold.sff` — FF3-written SFF **v2.0.1.0**
//!   with a 32-color palette, LZ5 sprites and the `coldepth=5` junk value
//!   that Go ignores for compressed formats (image.go:1370).
//! * `kfm-intro.sff` — real Elecbyte-era SFF **v1.01** (12 sprites, PCX,
//!   linked subheader list, shared-palette bookkeeping).

use krg_format::{Pixels, Sff, SffType};

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn decode_all(sff: &Sff) -> (usize, usize, usize) {
    // (decoded-with-pixels, decoded-empty, failures). Empty = SetPxl drop,
    // which Go performs silently — not an error.
    let mut with_px = 0;
    let mut empty = 0;
    let mut failures = 0;
    for (g, n) in sff.index.keys() {
        let e = sff.entry(*g, *n).unwrap();
        match sff.decode(e) {
            Ok(Pixels::Indexed(p)) if !p.is_empty() => with_px += 1,
            Ok(Pixels::Rgba(p)) if !p.is_empty() => with_px += 1,
            Ok(_) => empty += 1,
            Err(_) => failures += 1,
        }
    }
    (with_px, empty, failures)
}

#[test]
fn real_ff3_v2_font_parses_and_decodes() {
    let sff = Sff::load_path(fixture("default-3x5.sff"), SffType::Other, false).unwrap();

    // Header — verified against the raw bytes of the shipped file:
    // fsho=528, 94 sprites, 1 palette, lofs=3160, tofs=4319 (= file size).
    assert_eq!(sff.header.version, [2, 0, 1, 0]); // FF3 writes 2.0.1.0
    assert_eq!(sff.header.first_sprite_header_offset, 528);
    assert_eq!(sff.header.number_of_sprites, 94);
    assert_eq!(sff.header.number_of_palettes, 1);
    assert_eq!(sff.header.lofs, 3160);
    assert_eq!(sff.header.tofs, 4319);

    assert_eq!(sff.entries.len(), 94);
    assert_eq!(sff.palettes.palettes.len(), 1);

    // Glyph (0,33) is 1×5 pixels (read straight from the subheader table).
    let e = sff.entry(0, 33).unwrap();
    assert_eq!((e.width, e.height), (1, 5));

    // The whole font decodes: every sprite yields pixels, zero failures.
    let (with_px, empty, failures) = decode_all(&sff);
    assert_eq!(failures, 0);
    assert_eq!(empty, 0);
    assert_eq!(with_px, sff.index.len());

    // The palette is a real 32-color RGBA palette (pow2 clamp keeps 32).
    // FF3 writes v2.0.1.0 → NO alpha forcing (image.go:2255).
    let pal = sff.palettes.get(0).unwrap();
    let nonzero: usize = pal.iter().filter(|&&c| c != 0).count();
    assert!(nonzero > 10, "palette should have real colors, got {nonzero}");
}

#[test]
fn real_ff3_v2_bold_font_parses_and_decodes() {
    let sff = Sff::load_path(fixture("default-3x5-bold.sff"), SffType::Other, false).unwrap();
    assert_eq!(sff.header.version, [2, 0, 1, 0]);
    assert_eq!(sff.header.number_of_sprites, 94);
    let (with_px, empty, failures) = decode_all(&sff);
    assert_eq!(failures, 0);
    assert_eq!(empty, 0);
    assert_eq!(with_px, sff.index.len());
}

#[test]
fn real_v1_kfm_intro_parses_and_decodes() {
    let sff = Sff::load_path(fixture("kfm-intro.sff"), SffType::Char, false).unwrap();

    // Header read straight from the file: 12 sprites, first subheader @512.
    // Version bytes @12..16 = 00 01 00 01 → [verhi,verlo1,verlo2,verlo3] = 1.0.1.0.
    assert_eq!(sff.header.version, [1, 0, 1, 0]);
    assert_eq!(sff.header.number_of_sprites, 12);
    assert_eq!(sff.header.first_sprite_header_offset, 512);
    assert_eq!(sff.entries.len(), 12);

    let (with_px, _empty, failures) = decode_all(&sff);
    assert_eq!(failures, 0);
    // The legacy char-first-sprite path may legitimately drop mismatches.
    assert!(with_px > 0, "intro should have real sprites (with_px={with_px})");

    // v1 palettes were extracted from the PCX palette blocks.
    assert!(!sff.palettes.palettes.is_empty());
    let pal = sff.palettes.get(0).unwrap();
    assert_eq!(pal[0] & 0xFF00_0000, 0); // index 0 forced transparent
    assert_eq!(pal[1] & 0xFF00_0000, 0xFF00_0000);
}
