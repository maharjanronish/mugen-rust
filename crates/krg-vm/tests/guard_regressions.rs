//! Regression tests for the second wave of device-reported bugs:
//!
//! 1. **Ceiling escape on crouch-guard vs the trip kick** — the guarding
//!    player flies out through the top of the screen. Root cause: the
//!    guard-start hardcoded key re-entered state 120 from the whole guard
//!    family (only {120,130,140} were excluded instead of Ikemen's
//!    {120,130-132,140,150-155}), clobbering the pending guard-hit state
//!    and churning state 120's `physics: U` (no gravity, no friction).
//! 2. **Stuck/freeze while guarding** — same root cause: the guard-hit
//!    chain (152 → 153 → 131) never got to run, so ctrl/hitOver
//!    bookkeeping never completed and the guard visibly jittered.
//! 3. **Gethit state routing** — ground.type=Trip now routes to 5070
//!    (char.go:13112), downed+grounded to 5080, and forcestand retags
//!    crouch victims of launchers (char.go:13105-13107).

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

/// Ceiling escape repro: P1 crouches next to P2 and mashes the trip kick
/// (state 440: ground.velocity = -1.5,-2, fall = 1, guardflag = L).
/// P2 crouch-guards the whole time (down + back). The guarding player must
/// stay on the floor AND the guard chain must actually run (guard-hit
/// states 152/153 observed, no state-120 churn).
#[test]
fn diag_crouch_guard_trip_kick() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    // P1: walk right until adjacent to P2
    let mut walk = none();
    walk[3] = true; // R
    for _ in 0..55 {
        fight.step(&[walk, none()]);
    }

    // P1: crouch + tap b (crouching strong kick / trip). P2: crouch-guard
    // (down + back — P2 faces left, so back is raw R).
    let mut p1 = none();
    p1[1] = true; // D
    let mut p2 = none();
    p2[1] = true; // D
    p2[3] = true; // R (back for left-facing P2)

    let mut worst_y = 0.0f32;
    let mut guard_hit_states = 0usize; // ticks in 150-155
    let mut state120_ticks = 0usize; // guard-start churn counter
    let mut trace: Vec<(usize, i32, i32, f32, f32)> = Vec::new();
    for t in 0..600 {
        // tap b every 40 ticks (a held button is not a new command edge)
        let mut p1t = p1;
        p1t[5] = (t % 40) < 2;
        fight.step(&[p1t, p2]);
        let d = &fight.chars[1];
        if (150..=155).contains(&d.state_no) {
            guard_hit_states += 1;
        }
        if d.state_no == 120 {
            state120_ticks += 1;
        }
        if t % 20 == 0 || d.pos[1] < -20.0 {
            trace.push((t, d.state_no, d.move_type as i32, d.pos[1], d.vel[1]));
        }
        worst_y = worst_y.min(d.pos[1]);
    }
    for (t, st, mt, y, vy) in &trace {
        println!("t={:3} p2st={:4} mt={:2} y={:8.2} vy={:7.2}", t, st, mt, y, vy);
    }
    println!(
        "worst_y={worst_y} guard_hit_ticks={guard_hit_states} state120_ticks={state120_ticks}"
    );
    // ceiling escape: the guarding player never leaves the ground
    assert!(
        worst_y > -20.0,
        "crouch-guarding player launched to y={worst_y} (ceiling escape)"
    );
    // the guard-hit chain actually plays (was clobbered by state-120 churn)
    assert!(
        guard_hit_states > 5,
        "guard-hit states (150-155) ran only {guard_hit_states} ticks — guard chain clobbered"
    );
    // no 120 churn: guard start should be entered rarely, not every 2nd tick
    assert!(
        state120_ticks < 60,
        "state 120 entered {state120_ticks}/600 ticks — guard-start loop"
    );
}

/// Standing P2 (holding back, but guardflag = L means the trip kick is only
/// crouch-guardable) eats a clean TRIP hit: routed to 5070/5071, small hop,
/// lands, gets up. Must never leave the screen and must recover.
#[test]
fn diag_standing_hit_by_trip_kick() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut walk = none();
    walk[3] = true;
    for _ in 0..55 {
        fight.step(&[walk, none()]);
    }

    let mut p1_walk = none();
    p1_walk[3] = true; // R — pursue
    let mut p1_atk = none();
    p1_atk[1] = true; // D
    // P2 stands still (no guard attempt): a clean ground Trip hit must
    // route to the 5070/5071 chain (char.go:13112).
    let p2 = none();

    let mut worst_y = 0.0f32;
    let mut tripped = false;
    let mut trace: Vec<(usize, i32, i32, f32, f32)> = Vec::new();
    for t in 0..600 {
        // cycle: walk forward, crouch early, tap b once crouched, recover
        let phase = t % 40;
        let mut inp = p1_walk;
        if (18..40).contains(&phase) {
            inp = p1_atk; // D held — crouch settles by phase 20
            if (20..22).contains(&phase) {
                inp[5] = true; // b tap
            }
        }
        fight.step(&[inp, p2]);
        let d = &fight.chars[1];
        if d.state_no == 5070 || d.state_no == 5071 {
            tripped = true;
        }
        if t % 10 == 0 || d.pos[1] < -60.0 {
            trace.push((t, d.state_no, d.move_type as i32, d.pos[1], d.vel[1]));
        }
        worst_y = worst_y.min(d.pos[1]);
    }
    for (t, st, mt, y, vy) in &trace {
        println!("t={:3} p2st={:4} mt={:2} y={:8.2} vy={:7.2}", t, st, mt, y, vy);
    }
    println!("worst_y={worst_y} tripped={tripped}");
    // the trip chain routed correctly (char.go:13112 — HT_Trip → 5070)
    assert!(tripped, "clean trip hit never entered states 5070/5071");
    assert!(worst_y > -110.0, "tripped player launched to y={worst_y}");
    // and must return to the ground eventually
    let mut grounded_again = false;
    for _ in 0..240 {
        fight.step(&[none(), none()]);
        if fight.chars[1].pos[1] >= -0.01 && fight.chars[1].ctrl {
            grounded_again = true;
            break;
        }
    }
    assert!(grounded_again, "tripped player never recovered to the ground");
}
