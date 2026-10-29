//! Benchmark: cost of one Fight::step_bits (the full CNS VM tick) — the
//! number that decides whether the GL thread straddles a high-refresh
//! vsync deadline (the judder / "onion skin" mechanism).

use std::time::Instant;

use krg_vm::cmd::PR_BIT;
use krg_vm::ini::load_text;
use krg_vm::runtime::{compile_char, Fight};

fn read(p: &str) -> String {
    let b = std::fs::read(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    load_text(&b)
}

fn compile_kfm() -> std::sync::Arc<krg_vm::runtime::GlobalInfo> {
    let dir = "tests/fixtures/";
    let cmd = read(&format!("{dir}kfm.cmd"));
    let cns = read(&format!("{dir}kfm.cns"));
    let common_zss = read(&format!("{dir}common1.cns.zss"));
    let common_const = read(&format!("{dir}common.const"));
    let kfm_air = read(&format!("{dir}kfm.air"));
    let common_air = read(&format!("{dir}common.air"));
    compile_char(
        "Kung Fu Man",
        &cmd,
        &[cns],
        Some(&common_zss),
        &common_const,
        &[common_air, kfm_air],
    )
    .unwrap()
}

fn main() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    while fight.round_state < 2 {
        fight.step_bits(0, 0);
    }

    // warm up
    for _ in 0..120 {
        fight.step_bits(PR_BIT, PR_BIT);
    }

    // 1) idle-ish: both walk right (typical steady fight load)
    let n = 1800;
    let t0 = Instant::now();
    for _ in 0..n {
        fight.step_bits(PR_BIT, PR_BIT);
    }
    let walk_us = t0.elapsed().as_secs_f64() * 1e6 / n as f64;

    // 2) heavy: alternate walk / light punch (state machine churn + hitdefs)
    let t0 = Instant::now();
    for i in 0..n {
        let p = if i % 60 < 30 { PR_BIT } else { krg_vm::cmd::XBIT };
        fight.step_bits(p, krg_vm::cmd::ABIT);
    }
    let attack_us = t0.elapsed().as_secs_f64() * 1e6 / n as f64;

    // 3) snapshot cost (per display frame)
    let t0 = Instant::now();
    for i in 0..n * 2 {
        let _ = fight.snapshot((i % 60) as f32 / 60.0);
    }
    let snap_us = t0.elapsed().as_secs_f64() * 1e6 / (n * 2) as f64;

    println!("step_bits (walk):        {walk_us:8.1} µs/tick");
    println!("step_bits (attack mix):  {attack_us:8.1} µs/tick");
    println!("snapshot:                {snap_us:8.1} µs/frame");
    println!();
    println!("frame budget @ 60Hz: 16667 µs | @ 90Hz: 11111 µs | @ 120Hz: 8333 µs");
    println!("typical mid-range phone multiplies host numbers by ~2-4x (big.LITTLE core)");
}
