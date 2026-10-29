//! Render-pipeline regression: replays the EXACT scene.rs loop
//! (tick accumulator → step_bits → snapshot) with Ikemen GO's effective
//! mobile alpha semantics — every display frame draws the END-OF-TICK
//! state (system.go:2643 tickInterpolation: `if s.tickNextFrame() { return
//! 1 }`, and await() system.go:881 paces rendering at 60fps == logic rate,
//! so every drawn frame is a boundary frame).
//!
//! What these tests pin down:
//! * the char's WORLD position never moves backward while walking
//!   (the "onion skin" / previous-frame-visible family of bugs — the old
//!   alpha = tick_acc/MS sawtoothed at 90/120Hz),
//! * the tick cadence is exact at 60/90/120Hz display rates,
//! * the char stays GROUNDED (pos.y == 0) for the whole walk — the
//!   "floor is an incline, the character goes up" regression,
//! * the camera tracks the walk smoothly (monotone, bounded per-frame
//!   deltas — no teleport/judder while scrolling).

use krg_vm::cmd::{PD_BIT, PL_BIT, PR_BIT};
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

/// One display frame's rendered state (what the user SEES).
#[derive(Debug, Clone, Copy)]
struct Rendered {
    pos_x: f32,    // world position drawn this frame
    pos_y: f32,    // must stay 0 while grounded (the "incline" bug)
    cam_x: f32,    // camera position drawn this frame
    frame_idx: i32,
    state_no: i32,
    ticks: u32, // ticks executed this display frame
}

/// Replicates scene.rs advance_and_draw's fight branch exactly:
/// tick_acc += dt; clamp to 2*MS; drain up to 2 ticks; then snapshot —
/// Ikemen default: alpha = 1 (end-of-tick state).
fn run_display(fight: &mut Fight, dt_ms: f64, acc: &mut f64, p1: u32, p2: u32) -> Rendered {
    const MS: f64 = 1000.0 / 60.0;
    *acc += dt_ms.min(100.0);
    if *acc > MS * 2.0 {
        *acc = MS * 2.0;
    }
    let mut ticks = 0u32;
    while *acc >= MS && ticks < 2 {
        *acc -= MS;
        fight.step_bits(p1, p2);
        ticks += 1;
    }
    let snap = fight.snapshot(1.0);
    let c = &snap.chars[0];
    Rendered {
        pos_x: c.pos[0],
        pos_y: c.pos[1],
        cam_x: snap.cam_x,
        frame_idx: c.frame_idx,
        state_no: c.state_no,
        ticks,
    }
}

fn new_fight() -> Fight {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    while fight.round_state < 2 {
        fight.step_bits(0, 0);
    }
    fight
}

/// Walk BACKWARD (away from P2) so the pushbox never interrupts the walk:
/// P1 starts at -60 and can walk to the left camera bound (~-300).
fn walk_trace(hz: f64, frames: usize, label: &str) -> Vec<Rendered> {
    let mut fight = new_fight();
    let mut acc = 0.0f64;
    let dt = 1000.0 / hz;
    let mut out = Vec::with_capacity(frames);
    for _ in 0..frames {
        out.push(run_display(&mut fight, dt, &mut acc, PL_BIT, 0));
    }
    // ── diagnostics ────────────────────────────────────────────────────
    println!("== {label} ({hz:.0} Hz, {frames} display frames) ==");
    println!("  df | ticks | pos_x   | d(pos)  | cam_x  | y | frame | state");
    let mut last = out[0].pos_x;
    for (i, r) in out.iter().enumerate() {
        let d = r.pos_x - last;
        if i < 24 || i % 40 == 0 {
            println!(
                "  {:>3} |   {}   | {:>8.3} | {:>+7.3} | {:>7.3} | {:.1} |  {:>2}   | {}",
                i, r.ticks, r.pos_x, d, r.cam_x, r.pos_y, r.frame_idx, r.state_no
            );
        }
        last = r.pos_x;
    }
    out
}

/// Every rendered frame during a ground walk keeps pos.y == 0 — the
/// "character walks up an incline that isn't there" regression.
fn assert_grounded(out: &[Rendered]) {
    for (i, r) in out.iter().enumerate() {
        assert!(
            r.pos_y.abs() < 0.001,
            "char left the ground at display frame {i}: pos.y = {} (state {})",
            r.pos_y,
            r.state_no
        );
    }
}

/// World position never moves in the wrong direction while holding back.
fn assert_no_backstep(out: &[Rendered]) {
    for w in out.windows(2) {
        assert!(
            w[1].pos_x <= w[0].pos_x + 0.0001,
            "world pos stepped forward while walking back: {} -> {} (state {})",
            w[0].pos_x,
            w[1].pos_x,
            w[1].state_no
        );
    }
}

#[test]
fn walk_60hz_one_tick_per_frame() {
    let out = walk_trace(60.0, 120, "walk-back @60Hz");
    assert_grounded(&out);
    assert_no_backstep(&out);
    // exactly one tick per display frame, every frame (once walking)
    for (i, r) in out.iter().enumerate().skip(10) {
        assert_eq!(r.ticks, 1, "display frame {i} ran {} ticks", r.ticks);
    }
    // walk speed is constant per tick → uniform per-frame deltas, measured
    // inside the FREE-walk window only (df 12..70): past ~-237 the screen-edge
    // clamp (cam_x − 150, char.go:10323 xScreenBound) legitimately shrinks the
    // per-tick delta — that's engine behavior, not jitter.
    let ds: Vec<f32> = out
        .windows(2)
        .skip(12)
        .take(58)
        .map(|w| (w[0].pos_x - w[1].pos_x).abs())
        .collect();
    let (mn, mx) = ds.iter().cloned().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
    assert!(
        (mx - mn) < 0.02,
        "60Hz walk deltas not uniform: min {mn:.3} max {mx:.3}"
    );
    assert!(mn > 2.0, "walk delta collapsed inside the free window: {mn:.3}");
}

#[test]
fn walk_90hz_two_ticks_per_three_frames() {
    let out = walk_trace(90.0, 120, "walk-back @90Hz");
    assert_grounded(&out);
    assert_no_backstep(&out);
    // steady state: 2 ticks per 3 display frames (60/90)
    for chunk in out.chunks(3).skip(6) {
        let t: u32 = chunk.iter().map(|r| r.ticks).sum();
        assert_eq!(t, 2, "tick cadence broke: {:?}", chunk.iter().map(|r| r.ticks).collect::<Vec<_>>());
    }
    // and exactly 3 rendered positions per 2 ticks (each tick state held),
    // counted inside the free-walk window (df 6..106, before the screen-edge
    // clamp freezes further position change): 100 windows, 2 moves per 3.
    let moved: usize = out
        .windows(2)
        .skip(6)
        .take(100)
        .filter(|w| (w[0].pos_x - w[1].pos_x).abs() > 0.001)
        .count();
    let expected = 100 - 100 / 3;
    assert!(
        (moved as i32 - expected as i32).abs() <= 1,
        "position updates {moved} != expected ~{expected} (each tick held 1-2 frames)"
    );
}

#[test]
fn walk_120hz_alternating_tick_frames() {
    let out = walk_trace(120.0, 240, "walk-back @120Hz");
    assert_grounded(&out);
    assert_no_backstep(&out);
    // steady state: tick, no-tick, tick, no-tick…
    for chunk in out.chunks(2).skip(8) {
        let t: u32 = chunk.iter().map(|r| r.ticks).sum();
        assert_eq!(t, 1, "tick cadence broke: {:?}", chunk.iter().map(|r| r.ticks).collect::<Vec<_>>());
    }
}

#[test]
fn walk_jittered_vsync_never_backsteps() {
    let mut fight = new_fight();
    let mut acc = 0.0f64;
    let mut rng: u64 = 0x9E3779B97F4A7C15;
    let mut prev = None;
    let mut max_y = 0.0f32;
    for i in 0..240 {
        // xorshift jitter in [-1.5, +1.5] ms around the 60Hz nominal period
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let j = ((rng % 3000) as f64 / 1000.0) - 1.5;
        let dt = 1000.0 / 60.0 + j;
        let r = run_display(&mut fight, dt, &mut acc, PL_BIT, 0);
        max_y = max_y.max(r.pos_y.abs());
        if let Some(px) = prev {
            assert!(
                r.pos_x <= px + 0.0001,
                "df {i}: world pos backstepped {px:.3} -> {:.3}",
                r.pos_x
            );
        }
        prev = Some(r.pos_x);
    }
    assert!(max_y < 0.001, "char left the ground under vsync jitter (y={max_y})");
}

#[test]
fn walk_with_hitches_never_backsteps_or_bursts() {
    let mut fight = new_fight();
    let mut acc = 0.0f64;
    let mut prev = None;
    for i in 0..300 {
        // every 60th frame a 45ms hitch (GC / texture upload)
        let dt = if i % 60 == 0 && i > 0 { 45.0 } else { 1000.0 / 60.0 };
        let r = run_display(&mut fight, dt, &mut acc, PL_BIT, 0);
        assert!(r.ticks <= 2, "df {i}: {} ticks in one display frame", r.ticks);
        if let Some(px) = prev {
            assert!(
                r.pos_x <= px + 0.0001,
                "df {i}: world pos backstepped after hitch {px:.3} -> {:.3}",
                r.pos_x
            );
        }
        prev = Some(r.pos_x);
    }
}

/// Crouch-walk: holding down+back must stay grounded too (the reported
/// "crouch stand is being quickly rendered not smooth" family — the
/// crouch entry/exit must never lift the char).
#[test]
fn crouch_walk_stays_grounded() {
    let mut fight = new_fight();
    let mut acc = 0.0f64;
    let dt = 1000.0 / 60.0;
    let mut states = std::collections::HashSet::new();
    for _ in 0..240 {
        let r = run_display(&mut fight, dt, &mut acc, PD_BIT | PL_BIT, 0);
        states.insert(r.state_no);
        assert!(
            r.pos_y.abs() < 0.001,
            "crouch-walk lifted the char: pos.y = {} (state {})",
            r.pos_y,
            r.state_no
        );
    }
    // and it actually reached the crouch states (10/11), not just stood
    assert!(states.contains(&11), "never crouched: {states:?}");
}

/// Camera tracks the walking char smoothly: monotone follow with small
/// bounded per-display-frame deltas (camera.go action / camera_update —
/// x eases toward the clamped players' midpoint each tick; no quantized
/// grid, no teleports, no back-jitter while scrolling).
#[test]
fn camera_tracks_walk_smoothly() {
    let out = walk_trace(60.0, 180, "walk-back cam @60Hz");
    let mut moved = 0;
    for w in out.windows(2).skip(8) {
        let d = (w[1].cam_x - w[0].cam_x).abs();
        if d > 0.001 {
            moved += 1;
        }
        // one eased step of camera_update per tick: 0.45 · (target − cam),
        // target moves ≤ walkspeed per tick → the per-frame jump stays tiny
        assert!(
            d < 3.0,
            "camera jumped {d:.3} units in one display frame ({} -> {})",
            w[0].cam_x,
            w[1].cam_x
        );
    }
    assert!(moved > 60, "camera never tracked the walk ({moved} moving frames)");
    // monotone follow while the char walks left (cam_x only decreases)
    for w in out.windows(2).skip(8) {
        assert!(
            w[1].cam_x <= w[0].cam_x + 0.0001,
            "camera backstepped {} -> {}",
            w[0].cam_x,
            w[1].cam_x
        );
    }
}

/// The lerp machinery stays functional for a future TickInterpolation
/// toggle (Ikemen keeps it behind the config too): alpha 0/0.5/1 land on
/// start/mid/end tick positions.
#[test]
fn snapshot_lerp_machinery_intact() {
    let mut fight = new_fight();
    // one walk tick
    fight.step_bits(PR_BIT, 0);
    let end = fight.chars[0].pos[0];
    let start = fight.prev_pos[0][0];
    assert!(end > start, "walk tick did not move the char ({start} -> {end})");
    let a0 = fight.snapshot(0.0).chars[0].pos[0];
    let a5 = fight.snapshot(0.5).chars[0].pos[0];
    let a1 = fight.snapshot(1.0).chars[0].pos[0];
    assert!((a0 - start).abs() < 0.001, "alpha 0 = {a0}, want {start}");
    assert!((a5 - (start + end) / 2.0).abs() < 0.01, "alpha 0.5 = {a5}");
    assert!((a1 - end).abs() < 0.001, "alpha 1 = {a1}, want {end}");
}
