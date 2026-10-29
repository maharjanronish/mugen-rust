//! Reproduction tests for the two device-reported v0.9.x fight bugs:
//!
//! A. **Hit-stuck** — after being hit (especially repeatedly), the enemy
//!    stays in the gethit chain forever: never regains ctrl, never returns
//!    to state 0. Root cause candidate: ghv.hitshaketime / ghv.hittime are
//!    set on hit but never decremented (Ikemen GO decrements them every
//!    tick in actionRun, char.go:12581-12636), so hitShakeOver / hitOver
//!    never fire and the common1.cns gethit states never exit.
//! B. **Ceiling escape** — an airborne char that gets hit ends up in states
//!    with physics N and (if hitshaketime is stuck) no gravity ever applies;
//!    a char flying upward never comes back down and leaves the screen.

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

fn none() -> [bool; 14] {
    [false; 14]
}

fn skip_intro(fight: &mut Fight) {
    for _ in 0..80 {
        fight.step(&[none(), none()]);
    }
    assert_eq!(fight.round_state, 2, "round live after intro");
}

/// Walk P1 next to P2 so a jab connects, then jab once (X for a few frames).
fn land_one_jab(fight: &mut Fight) {
    // close the distance: P1 walks right toward P2 for ~40 ticks
    for _ in 0..40 {
        let mut i1 = none();
        i1[3] = true; // R
        fight.step(&[i1, none()]);
    }
    // jab: hold nothing but tap X
    for _ in 0..6 {
        let mut i1 = none();
        i1[8] = true; // X
        fight.step(&[i1, none()]);
    }
}

/// A: after a single clean hit lands, the victim must eventually recover:
/// back to a normal state (0/11/12 family), ctrl restored.
#[test]
fn victim_recovers_after_single_hit() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);
    land_one_jab(&mut fight);

    let life_before = fight.chars[1].life;
    let hit_landed = life_before < fight.chars[1].life_max;
    if !hit_landed {
        // walk closer and try once more (range jitter)
        for _ in 0..15 {
            let mut i1 = none();
            i1[3] = true;
            fight.step(&[i1, none()]);
        }
        for _ in 0..6 {
            let mut i1 = none();
            i1[8] = true;
            fight.step(&[i1, none()]);
        }
    }
    assert!(
        fight.chars[1].life < fight.chars[1].life_max,
        "test setup failed: jab never landed"
    );

    // wait far longer than any KFM gethit chain (shake 8 + hittime ~20)
    for _ in 0..180 {
        fight.step(&[none(), none()]);
    }

    let p2 = &fight.chars[1];
    assert!(
        p2.ctrl,
        "victim never regained ctrl (state {}, movetype {:?})",
        p2.state_no,
        p2.move_type
    );
    assert!(
        p2.state_no < 5000,
        "victim still in gethit chain after 3s (state {})",
        p2.state_no
    );
}

/// A2: the full reported scenario — P1 mashes punches continuously; whenever
/// P2 is hit it must keep cycling through gethit normally, and after the
/// mashing stops P2 must recover within a bounded window. It may never
/// freeze motionless forever.
#[test]
fn continuous_pressure_never_freezes_victim() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    // 8 seconds of continuous mashing while walking forward
    for t in 0..480 {
        let mut i1 = none();
        i1[3] = true; // R toward P2
        i1[8] = (t % 20) < 8; // X mashing
        fight.step(&[i1, none()]);
    }

    // release everything
    for t in 0..240 {
        fight.step(&[none(), none()]);
        if t > 180 {
            let p2 = &fight.chars[1];
            if p2.alive {
                assert!(
                    p2.ctrl || p2.state_no < 5000,
                    "P2 frozen after pressure stopped (state {}, ctrl {})",
                    p2.state_no,
                    p2.ctrl
                );
            }
        }
    }
    let p2 = &fight.chars[1];
    if p2.alive {
        assert!(
            p2.ctrl,
            "P2 never recovered ctrl after 4s of no pressure (state {})",
            p2.state_no
        );
    }
}

/// B: hit someone who is airborne — they must come back down to the floor
/// (gravity applies through the whole gethit chain) and never leave the
/// stage vertical space.
#[test]
fn air_hit_victim_returns_to_ground() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    // P2 jumps straight up
    for t in 0..20 {
        let mut i2 = none();
        i2[0] = true; // U
        fight.step(&[none(), i2]);
    }
    // P1 closes in and jabs upward-ish (KFM jab reaches airborne targets
    // near apex). Walk in first.
    for _ in 0..34 {
        let mut i1 = none();
        i1[3] = true;
        fight.step(&[i1, none()]);
    }
    for _ in 0..10 {
        let mut i1 = none();
        i1[8] = true; // X
        fight.step(&[i1, none()]);
    }

    // however the hit resolved, within 5 seconds the victim is back on the
    // ground or in a normal recovery state — never stuck floating/flying.
    let mut peak = 0.0f32;
    for _ in 0..300 {
        fight.step(&[none(), none()]);
        peak = peak.min(fight.chars[1].pos[1]);
    }
    let p2 = &fight.chars[1];
    assert!(
        p2.pos[1] > -400.0,
        "P2 flew away vertically (y={}, no gravity in gethit chain)",
        p2.pos[1]
    );
    if p2.alive && p2.pos[1] >= -0.001 {
        assert!(
            p2.ctrl || p2.state_no < 5000,
            "P2 on ground but stuck in gethit (state {})",
            p2.state_no
        );
    }
    // sanity: the peak must be bounded (jump height ~100, air-hit adds some)
    assert!(
        peak > -400.0,
        "P2 left the stage vertically during the exchange (peak y={peak})"
    );
}

/// B2: repeated air hits must not send the victim out the ceiling — the
/// classic "enemy jumps out of the ceiling" report.
#[test]
fn repeated_air_hits_stay_in_bounds() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut rng_t = 0u32;
    for _ in 0..720 {
        rng_t += 1;
        // P2 keeps hopping
        let mut i2 = none();
        i2[0] = (rng_t % 90) < 12;
        // P1 walks in and mashes
        let mut i1 = none();
        i1[3] = true;
        i1[8] = (rng_t % 22) < 9;
        fight.step(&[i1, i2]);
        let y = fight.chars[1].pos[1];
        assert!(
            y > -400.0,
            "P2 escaped through the ceiling (y={y}) — vertical bound missing"
        );
        let y0 = fight.chars[0].pos[1];
        assert!(y0 > -400.0, "P1 escaped through the ceiling (y={y0})");
    }
}
