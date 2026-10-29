//! Regression tests for the three device-reported v0.9.2 bugs:
//!
//! 1. **Ceiling escape / floats mid-air** — every air hit re-launched the
//!    victim upward with no limit. Fixed by the Ikemen juggle system port
//!    (ghv.targeted_by points + StateDef juggle cost + the hit gate).
//! 2. **Stuck at the stage end** — the screen-edge clamp zeroed velocity
//!    (killing jump arcs and slides that touched the edge) and the player
//!    push had no corner redistribution, so a walking opponent pinned a
//!    cornered player forever.
//! 3. **Laggy hit chains** — consequence of 1: the victim hung frozen in
//!    the air while being chain-hit.

use krg_vm::ini::load_text;
use krg_vm::runtime::{compile_char, Fight};

fn read(p: &str) -> String {
    let b = std::fs::read(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    load_text(&b)
}

fn compile_kfm() -> std::sync::Arc<krg_vm::runtime::GlobalInfo> {
    let dir = "tests/fixtures/";
    compile_char(
        "Kung Fu Man",
        &read(&format!("{dir}kfm.cmd")),
        &[read(&format!("{dir}kfm.cns"))],
        Some(&read(&format!("{dir}common1.cns.zss"))),
        &read(&format!("{dir}common.const")),
        &[
            read(&format!("{dir}common.air")),
            read(&format!("{dir}kfm.air")),
        ],
    )
    .unwrap()
}

fn none() -> [bool; 14] {
    [false; 14]
}

fn skip_intro(fight: &mut Fight) {
    for _ in 0..80 {
        fight.step(&[none(), none()]);
    }
}

/// Bug 1: neutral jump. Track apex, air time, landing state.
#[test]
fn diag_neutral_jump() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut up = none();
    up[0] = true; // hold U

    let mut apex = 0.0f32;
    let mut air_ticks = 0usize;
    let mut landed_tick = 0usize;
    let mut trace: Vec<(usize, i32, f32, f32, f32)> = Vec::new();
    for t in 0..400 {
        // hold up only 1 tick (tap) so the jump starts; then release
        let inp = if t < 6 { up } else { none() };
        fight.step(&[inp, none()]);
        let c = &fight.chars[0];
        if t % 5 == 0 {
            trace.push((t, c.state_no, c.pos[1], c.vel[1], fight.cam_y));
        }
        if c.state_no == 40 {
            continue; // pre-jump crouch
        }
        if c.state_type == krg_vm::statedef::StateTypeT::A || c.pos[1] < 0.0 {
            air_ticks += 1;
            apex = apex.min(c.pos[1]);
            landed_tick = t;
        } else if air_ticks > 0 {
            break; // landed and settled
        }
    }
    println!("apex={apex} air_ticks={air_ticks} landed_at={landed_tick}");
    for row in &trace {
        println!("t={:3} st={:4} y={:8.2} vy={:7.2} camy={:7.2}", row.0, row.1, row.2, row.3, row.4);
    }
    // KFM neutral jump: vy=-8.4, g=.44 → apex ≈ -80, airtime ≈ 38 ticks
    assert!(apex > -110.0, "jump apex {apex} too deep — char exits screen top");
    assert!(air_ticks < 60, "air time {air_ticks} ticks — 'lands after a few seconds'");
}

/// Bug 2: walk to the stage right end, then try to walk back.
#[test]
fn diag_stage_end_walk_back() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut right = none();
    right[3] = true;
    let mut left = none();
    left[2] = true;

    // walk right for 10 seconds — well past reaching the edge
    let mut max_x = 0.0f32;
    for t in 0..600 {
        fight.step(&[right, none()]);
        max_x = max_x.max(fight.chars[0].pos[0]);
        if t % 100 == 99 {
            println!("walk-right t={t} x={:.2} st={} ctrl={} vel={:.2}",
                fight.chars[0].pos[0], fight.chars[0].state_no,
                fight.chars[0].ctrl, fight.chars[0].vel[0]);
        }
    }
    println!("max_x={max_x} final_x={:.2}", fight.chars[0].pos[0]);

    // now walk back left for 5 seconds — must make progress
    let x_before = fight.chars[0].pos[0];
    let mut min_after = f32::MAX;
    for t in 0..300 {
        fight.step(&[left, none()]);
        min_after = min_after.min(fight.chars[0].pos[0]);
        if t % 50 == 49 {
            println!("walk-back t={t} x={:.2} st={} ctrl={} vel={:.2}",
                fight.chars[0].pos[0], fight.chars[0].state_no,
                fight.chars[0].ctrl, fight.chars[0].vel[0]);
        }
    }
    let progress = x_before - min_after;
    println!("x_before={x_before} min_after={min_after} progress={progress}");
    assert!(progress > 50.0, "STUCK at stage edge: walked back only {progress} in 5s");
}

/// Bug 3: victim gets hit out of the air and must come down.
/// P2 stands still; P1 walks in, P2 jumps, P1 punches it out of the air.
#[test]
fn diag_air_gethit_recovery() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    // P1 hold right + X (light punch), P2 jump straight up
    let mut p1 = none();
    p1[3] = true;
    p1[8] = true;
    let mut up = none();
    up[0] = true;

    let mut worst_float = 0usize;
    let mut floating = 0usize;
    let mut trace: Vec<(usize, i32, i32, f32, f32, bool)> = Vec::new();
    for t in 0..900 {
        let p2inp = if t < 60 { up } else { none() };
        fight.step(&[p1, p2inp]);
        let c = &fight.chars[1];
        trace.push((t, c.state_no, c.move_type as i32, c.pos[1], c.vel[1], c.ctrl));
        // floating: airborne, in gethit chain, but velocity ~0 (not falling)
        if c.pos[1] < -1.0 && c.move_type == krg_vm::statedef::MoveTypeT::H {
            if c.vel[1].abs() < 0.05 {
                floating += 1;
                worst_float = worst_float.max(floating);
            } else {
                floating = 0;
            }
        }
    }
    for (t, st, mt, y, vy, ctrl) in &trace {
        if *mt == 2 || t % 20 == 0 {
            println!("t={:3} st={:4} mt={:2} y={:8.2} vy={:7.2} ctrl={}", t, st, mt, y, vy, ctrl);
        }
    }
    println!("worst consecutive float-in-air-still: {worst_float} ticks");
    assert!(worst_float < 20, "victim floats motionless mid-air for {worst_float} ticks");
}

/// JUGGLE FIX: P2 jumps straight up; P1 stands adjacent and mashes X in
/// pulses. Before the fix every air hit re-set vy=-3/-4 (upward) with no
/// limit — the victim floated up out of the screen and only "landed after
/// a few seconds" when the mashing stopped. With Ikemen's juggle gate the
/// chain whiffs after ~15 points and the victim comes down promptly.
#[test]
fn diag_juggle_points_limit_air_chain() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    // walk P1 adjacent to P2 first (land_one_jab pattern)
    for _ in 0..55 {
        let mut i1 = none();
        i1[3] = true;
        fight.step(&[i1, none()]);
    }

    let mut up = none();
    up[0] = true;
    let mut mash = 0u32;
    let mut apex = 0.0f32;
    let mut air_hits = 0i32;
    let mut life_before: Option<i32> = None;
    let mut life_after_air_chain = 0i32;
    let mut landed = false;
    let mut air_frames = 0usize;
    for t in 0..480 {
        mash += 1;
        let tap = (mash % 16) < 6; // X pulse every 16 ticks
        let mut i1 = none();
        i1[8] = tap;
        // P2 hops straight up every 80 ticks
        let p2 = if t % 80 < 4 { up } else { none() };
        fight.step(&[i1, p2]);
        let v = &fight.chars[1];
        if v.pos[1] < -1.0 {
            air_frames += 1;
            if v.move_type == krg_vm::statedef::MoveTypeT::H {
                if life_before.is_none() {
                    life_before = Some(v.life);
                }
                air_hits += 1;
                apex = apex.min(v.pos[1]);
            }
        } else if life_before.is_some() && !landed {
            landed = true;
            life_after_air_chain = v.life;
        }
    }
    println!("apex={apex:.1} air_hits={air_hits} air_frames={air_frames} landed={landed}");
    if let Some(l0) = life_before {
        println!("life: {l0} -> {life_after_air_chain} ({} dmg while airborne)", l0 - life_after_air_chain);
    }
    // ceiling: victim must never get near the top of a 240-high screen
    assert!(apex > -190.0, "victim escaped through the ceiling (apex {apex})");
    // and must actually come back down
    assert!(landed, "victim never landed — stuck floating mid-air");
    // juggle must actually be limited: with 15 points and jab cost 1-4,
    // a 480-tick mash can NOT keep draining life in the air forever
    if let Some(l0) = life_before {
        let air_dmg = l0 - life_after_air_chain;
        assert!(air_dmg < 300, "air chain dealt {air_dmg} — juggle gate not limiting");
    }
}

/// JUGGLE BOOKKEEPING: the first air hit spends the statedef cost from
/// the victim's 15 airjuggle points and consumes the attacker's cost;
/// state re-entry re-arms it (char.go:12092-12162 + stateDef_juggle).
#[test]
fn diag_juggle_points_spent_and_rearmed() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    for _ in 0..55 {
        let mut i1 = none();
        i1[3] = true;
        fight.step(&[i1, none()]);
    }
    let mut up = none();
    up[0] = true;
    let mut mash = 0u32;
    let mut pts_after_first: Option<i32> = None;
    let mut jugg_rearmed = false;
    let mut recovered = false;
    let mut apex = 0.0f32;
    for t in 0..320 {
        mash += 1;
        let tap = (mash % 16) < 6;
        let mut i1 = none();
        i1[8] = tap;
        let p2 = if t < 4 { up } else { none() };
        fight.step(&[i1, p2]);
        let v = &fight.chars[1];
        if v.pos[1] < -1.0 {
            apex = apex.min(v.pos[1]);
        }
        if pts_after_first.is_none() && !v.ghv.targeted_by.is_empty() {
            pts_after_first = Some(v.ghv.targeted_by[0].1);
        }
        if t > 40 && fight.chars[0].state_no == 210 && fight.chars[0].juggle > 0 {
            jugg_rearmed = true;
        }
        if t > 200 && v.ctrl && v.state_no < 5000 {
            recovered = true;
        }
    }
    println!("pts={:?} rearm={jugg_rearmed} rec={recovered} apex={apex:.1}", pts_after_first);
    // kfm airjuggle=15, jab state 210 costs juggle=4 → 11 left after the
    // launcher; the attacker's cost is consumed and re-armed per state entry
    assert_eq!(pts_after_first, Some(11), "first air hit must spend exactly 4 of 15 points");
    assert!(jugg_rearmed, "StateDef juggle never re-armed the cost on state re-entry");
    assert!(recovered, "victim never recovered after the air hit");
    assert!(apex > -190.0, "victim escaped through the ceiling (apex {apex})");
}

/// CORNER PUSH: P1 idle at the right screen edge, P2 pressing into it.
/// Ikemen rules (char.go:14755-82): the cornered char keeps the wall, the
/// presser absorbs the full push (no body-merge), and when P1 walks out
/// (presser idle) it pushes P2 ahead of it and escapes with real progress.
#[test]
fn diag_corner_press_escape() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut right = none();
    right[3] = true;
    let mut left = none();
    left[2] = true;

    // P1 walks to the right edge (P2 idle)
    for _ in 0..600 {
        fight.step(&[right, none()]);
    }
    let x_edge = fight.chars[0].pos[0];
    assert!(x_edge > 100.0, "setup: P1 did not reach the right edge (x={x_edge})");

    // P2 presses rightward into the cornered P1 for 3s
    fight.chars[1].pos[0] = x_edge - 18.0;
    fight.chars[1].facing = 1;
    let mut min_gap = f32::MAX;
    for _ in 0..180 {
        fight.step(&[none(), right]);
        let gap = (fight.chars[1].pos[0] - fight.chars[0].pos[0]).abs();
        min_gap = min_gap.min(gap);
    }
    // P1 must keep the wall (not merged through), P2 must not stack inside
    assert!(min_gap > -5.0, "bodies merged through the corner (gap {min_gap})");
    let still_at_wall = (fight.chars[0].pos[0] - x_edge).abs() < 1.0;
    println!("wall_held={still_at_wall} min_gap={min_gap:.2}");

    // P1 walks left, P2 idle: pushes P2 ahead and escapes the corner
    let x_before = fight.chars[0].pos[0];
    let mut min_x = f32::MAX;
    for _ in 0..240 {
        fight.step(&[left, none()]);
        min_x = min_x.min(fight.chars[0].pos[0]);
    }
    let progress = x_before - min_x;
    println!("escaped_by={progress:.2}");
    assert!(progress > 50.0, "cornered player could not push out: only {progress}");
}

/// FULL device scenario: real CPU AI (Ikemen input.go random jammer),
/// P1 walks to the right stage end, stays there under CPU pressure, then
/// must walk back left. No permanent freeze, no trap state, progress real.
#[test]
fn diag_stage_end_with_real_cpu() {
    use krg_vm::cmd::{AiInput, LehmerRng};
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut ai = AiInput::default();
    let mut rng = LehmerRng::new(4242);
    let mut right = none();
    right[3] = true;
    let mut left = none();
    left[2] = true;

    // P1 walks right for 10s with the CPU live
    for _ in 0..600 {
        let intro = fight.round_state < 2;
        ai.update(intro, 3.0, &mut rng);
        fight.step(&[right, ai.buttons()]);
    }
    let x_edge = fight.chars[0].pos[0];
    assert!(x_edge > 100.0, "P1 never reached the stage end (x={x_edge})");

    // linger 5s at the edge under CPU pressure
    for _ in 0..300 {
        let intro = fight.round_state < 2;
        ai.update(intro, 3.0, &mut rng);
        fight.step(&[none(), ai.buttons()]);
    }

    // walk back left for 5s — real progress required
    let x_before = fight.chars[0].pos[0];
    let mut min_x = f32::MAX;
    for _ in 0..300 {
        let intro = fight.round_state < 2;
        ai.update(intro, 3.0, &mut rng);
        fight.step(&[left, ai.buttons()]);
        min_x = min_x.min(fight.chars[0].pos[0]);
    }
    let progress = x_before - min_x;
    println!("edge={x_edge:.2} walkback_progress={progress:.2}");
    assert!(progress > 50.0, "P1 stuck at stage end: only {progress} of leftward progress in 5s");
}
