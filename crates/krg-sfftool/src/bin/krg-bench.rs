//! Scale benchmark — a synthetic screenpack-scale SFF v2 (5000 sprites)
//! exercised through the Kurogane pipeline.
//!
//! Run: `cargo run --release -p krg-sfftool --bin krg-bench`

use krg_format::{Sff, SffData, SffType, SpriteCache};
use std::time::Instant;

fn build_big_sff_v2(sprite_count: usize, w: u16, h: u16) -> Vec<u8> {
    // Each sprite: RLE8-encoded gradient block, one shared palette.
    let pixels: Vec<u8> = (0..w as usize * h as usize)
        .map(|i| ((i * 7 + i / w as usize) % 251 + 1) as u8)
        .collect();
    let mut rle8: Vec<u8> = Vec::with_capacity(pixels.len() * 2);
    let mut i = 0;
    while i < pixels.len() {
        let v = pixels[i];
        let mut run = 1usize;
        while i + run < pixels.len() && pixels[i + run] == v && run < 63 {
            run += 1;
        }
        rle8.push(0x40 | run as u8);
        rle8.push(v);
        i += run;
    }

    let first_sub = 4096u32;
    let pal_sub = 2048u32;
    let sprite_data_start = first_sub + 28 * sprite_count as u32;
    let lofs = sprite_data_start + (4 + rle8.len() as u32) * sprite_count as u32;

    let mut buf: Vec<u8> = Vec::with_capacity(lofs as usize + 1024 + sprite_count * rle8.len());
    buf.extend_from_slice(b"ElecbyteSpr\0");
    buf.extend_from_slice(&[0, 0, 0, 2]);
    buf.extend(std::iter::repeat(0u8).take(16));
    buf.extend_from_slice(&first_sub.to_le_bytes());
    buf.extend_from_slice(&(sprite_count as u32).to_le_bytes());
    buf.extend_from_slice(&pal_sub.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&lofs.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());

    buf.extend(std::iter::repeat(0u8).take(pal_sub as usize - buf.len()));
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&256u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&1024u32.to_le_bytes());

    buf.extend(std::iter::repeat(0u8).take(first_sub as usize - buf.len()));
    for s in 0..sprite_count {
        let data_ofs = sprite_data_start + (4 + rle8.len() as u32) * s as u32;
        buf.extend_from_slice(&(s as u16).to_le_bytes()); // group = s
        buf.extend_from_slice(&0u16.to_le_bytes()); // number = 0
        buf.extend_from_slice(&w.to_le_bytes());
        buf.extend_from_slice(&h.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes()); // x
        buf.extend_from_slice(&0i16.to_le_bytes()); // y
        buf.extend_from_slice(&0u16.to_le_bytes()); // link
        buf.push(2); // RLE8
        buf.push(8); // coldepth
        buf.extend_from_slice(&data_ofs.to_le_bytes());
        buf.extend_from_slice(&(4 + rle8.len() as u32).to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // palidx
        buf.extend_from_slice(&1u16.to_le_bytes()); // flags bit0 → += tofs(0)
    }
    for _ in 0..sprite_count {
        buf.extend_from_slice(&(rle8.len() as u32).to_le_bytes());
        buf.extend_from_slice(&rle8);
    }
    assert_eq!(buf.len(), lofs as usize);
    for c in 0..256u32 {
        buf.extend_from_slice(&[(c % 251) as u8, (c % 200) as u8, (c % 100) as u8, if c == 0 { 0 } else { 255 }]);
    }
    buf
}

fn main() {
    let sprite_count = 5000usize;
    let (w, h) = (120u16, 90u16); // fight-scale sprites
    println!("building synthetic SFF v2: {sprite_count} sprites of {w}x{h}…");
    let t0 = Instant::now();
    let bytes = build_big_sff_v2(sprite_count, w, h);
    println!(
        "  built: {} bytes ({:.1} MiB) in {:.0} ms",
        bytes.len(),
        bytes.len() as f64 / 1048576.0,
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // ---- Load (lazy: headers only) ----
    let t0 = Instant::now();
    let sff = Sff::load(SffData::Owned(bytes.into()), SffType::Other, false).unwrap();
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  load (headers + palettes only): {load_ms:.1} ms");
    println!(
        "  lazily held RAM: ≈ {} KiB of subheaders (pixels stay in the file mapping)",
        sprite_count * 28 / 1024
    );

    // ---- Decode everything once, unbounded (eager-equivalent throughput) ----
    let mut cache = SpriteCache::new(sff.clone(), usize::MAX);
    let t0 = Instant::now();
    let mut n = 0usize;
    for s in 0..sprite_count as u16 {
        if cache.get(s, 0).unwrap().is_some() {
            n += 1;
        }
    }
    let dec_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  decoded {n} sprites in {dec_ms:.0} ms ({:.2} ms/sprite)", dec_ms / n as f64);
    println!(
        "  unbounded cache: ≈ {:.1} MiB decoded pixels (what Ikemen holds eagerly)",
        cache.used_bytes() as f64 / 1048576.0
    );

    // ---- Decode all under a 64 MiB budget ----
    let mut cache = SpriteCache::new(sff.clone(), 64 * 1024 * 1024);
    let t0 = Instant::now();
    let mut decoded_bytes = 0usize;
    for s in 0..sprite_count as u16 {
        if let Some(d) = cache.get(s, 0).unwrap() {
            decoded_bytes += d.pixels.byte_len();
        }
    }
    let dec2_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let _ = decoded_bytes;
    println!(
        "  full sweep under 64 MiB budget: {dec2_ms:.0} ms, peak cache = {:.1} MiB",
        cache.used_bytes() as f64 / 1048576.0
    );

    // ---- Hot path: what a fight actually touches each frame ----
    let hot: Vec<u16> = (0..300).map(|i| (i * 7) % sprite_count as u16).collect();
    let mut cache = SpriteCache::new(sff.clone(), 16 * 1024 * 1024);
    for &s in &hot {
        let _ = cache.get(s, 0).unwrap(); // warm
    }
    const ITER: usize = 100;
    let t0 = Instant::now();
    for _ in 0..ITER {
        for &s in &hot {
            let _ = cache.get(s, 0).unwrap();
        }
    }
    let hot_ms = t0.elapsed().as_secs_f64() * 1000.0 / ITER as f64;
    println!(
        "  hot path (300 sprites/sweep, 16 MiB budget): {hot_ms:.3} ms per sweep, resident = {:.1} MiB",
        cache.used_bytes() as f64 / 1048576.0
    );
}
