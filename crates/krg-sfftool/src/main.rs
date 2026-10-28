//! krg-sfftool — inspect SFF/AIR files, verify integrity, benchmark decode.
//!
//! Host-side tool for CI and content debugging (the shipping product is the
//! Android app; this binary never runs on the device).

use krg_format::{AnimCompiler, Sff, SffType, SpriteCache};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: {} <file.sff|file.air> [--bench N] [--cache-bytes N] [--decode-all]\n\
             \n\
             Commands:\n\
               info        parse & print header/subheader stats (default)\n\
               --bench N   decode every sprite N times, report timings\n\
               --decode-all  decode every sprite once, report pixel stats",
            args[0]
        );
        std::process::exit(1);
    }
    let path = &args[1];
    let bench = flag(&args, "--bench").map(|v| v.parse::<u32>().unwrap_or(1));
    let decode_all = args.iter().any(|a| a == "--decode-all");
    let cache_bytes = flag(&args, "--cache-bytes")
        .map(|v| v.parse::<usize>().unwrap_or(256 * 1024 * 1024));

    match path.rsplit('.').next().unwrap_or("").to_lowercase().as_str() {
        "sff" => {
            let t0 = Instant::now();
            let sff = Sff::load_path(path, SffType::Char, false).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(2);
            });
            let parse_ms = t0.elapsed().as_secs_f64() * 1000.0;

            println!("SFF file: {path}");
            println!(
                "  version: {}.{}.{}.{}",
                sff.header.version[0],
                sff.header.version[1],
                sff.header.version[2],
                sff.header.version[3]
            );
            println!("  sprites (subheaders): {}", sff.entries.len());
            println!("  unique keys: {}", sff.index.len());
            println!(
                "  linked: {}",
                sff.entries.iter().filter(|e| e.link.is_some()).count()
            );
            println!("  palettes: {}", sff.palettes.palettes.len());
            let (mut _w_sum, mut h_max) = (0u64, 0u64);
            for e in sff.entries.iter() {
                _w_sum += e.width as u64;
                h_max = h_max.max(e.height as u64);
            }
            println!("  parse time: {parse_ms:.2} ms (headers only — lazy)");

            if decode_all || bench.is_some() {
                let n = bench.unwrap_or(1);
                let budget = cache_bytes.unwrap_or(usize::MAX);
                let mut cache = SpriteCache::new(sff.clone(), budget);
                let keys: Vec<(u16, u16)> = sff.index.keys().copied().collect();
                let t0 = Instant::now();
                let mut total_px = 0usize;
                let mut decoded = 0usize;
                let mut failures = 0usize;
                for _ in 0..n {
                    for &(g, num) in &keys {
                        match cache.get(g, num) {
                            Ok(Some(d)) => {
                                total_px += d.pixels.byte_len();
                                decoded += 1;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                failures += 1;
                                if failures <= 5 {
                                    eprintln!("  decode error ({g},{num}): {e}");
                                }
                            }
                        }
                    }
                }
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                println!("  decoded {decoded} sprites x {n} pass(es) in {ms:.1} ms");
                println!("  pixel bytes per pass: {:.2} MiB", total_px as f64 / n as f64 / 1048576.0);
                if failures > 0 {
                    println!("  failures: {failures}");
                }
                println!(
                    "  cache used: {:.2} MiB / budget {}",
                    cache.used_bytes() as f64 / 1048576.0,
                    match budget {
                        usize::MAX => "unlimited".to_string(),
                        b => format!("{:.0} MiB", b as f64 / 1048576.0),
                    }
                );
            }
        }
        "air" => {
            let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(2);
            });
            let t0 = Instant::now();
            let mut ac = AnimCompiler::new(true);
            ac.compile_text(path, &text);
            let warnings_len = ac.warnings.messages.len();
            let warn_head: Vec<String> =
                ac.warnings.messages.iter().take(5).cloned().collect();
            let table = ac.finalize();
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            println!("AIR file: {path}");
            println!("  actions: {}", table.anims.len());
            let mut keys: Vec<&i32> = table.anims.keys().collect();
            keys.sort();
            for k in keys.iter().take(10) {
                let a = &table.anims[k];
                println!(
                    "    action {k}: {} frames, loopstart {}, totaltime {}",
                    a.frames.len(),
                    a.loopstart,
                    a.totaltime
                );
            }
            if keys.len() > 10 {
                println!("    … and {} more", keys.len() - 10);
            }
            if warnings_len > 0 {
                println!("  warnings: {warnings_len}");
                for w in warn_head.iter() {
                    println!("    {w}");
                }
            }
            println!("  parse time: {ms:.2} ms");
        }
        ext => eprintln!("unsupported extension: .{ext}"),
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}
