//! Regression tests for the four device-test bugs reported against v0.9.0:
//!
//! 1. **Inverted jump Y** — chars "disappeared" / sank below the floor while
//!    airborne. The model is unchanged (pos.y ≤ 0 always); this pins the
//!    invariant the renderer relied on being wrong about.
//! 2. **One-hit KO** — a HitDef re-hit every frame (60×23 dmg/s). Now each
//!    HitDef activation latches its targets (char.go hasTargetOfHitdef).
//! 3. **No player push** — fighters merged into each other. Now the size-box
//!    push keeps them apart and they can never cross through one another.
//! 4. **Backward walking broken** — the logical B/F fed to the command
//!    matcher ignored facing; after any side switch "holdback" walked
//!    forward. Now facing-relative.

use krg_vm::cmd::CommandList;
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

/// [U D L R a b c x y z s d w m]
fn none() -> [bool; 14] {
    [false; 14]
}

fn skip_intro(fight: &mut Fight) {
    for _ in 0..80 {
        fight.step(&[none(), none()]);
    }
    assert_eq!(fight.round_state, 2, "round live after intro");
}

/// 1+3: a full brawl — walk P1 into P2, both mash attacks. Asserts:
/// nobody ever sinks below the floor, no single tick ever costs more than
/// a full-strength hit's worth of life, and the fighters never cross
/// through each other (the push keeps P1 left of P2).
#[test]
fn brawl_ground_damage_and_separation() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut p1 = none();
    p1[3] = true; // hold R (toward P2) + mash X
    let mut p2 = none();
    p2[2] = true; // hold L (toward P1) + mash A
    let mut prev_life = [fight.chars[0].life, fight.chars[1].life];
    let life_max = fight.chars[0].life_max;

    let mut mash = 0u32;
    for t in 0..600 {
        mash += 1;
        let atk = (mash % 24) < 8;
        let mut i1 = p1;
        i1[8] = atk; // X punch
        let mut i2 = p2;
        i2[4] = atk; // A kick
        fight.step(&[i1, i2]);

        for i in 0..2 {
            assert!(
                fight.chars[i].pos[1] <= 0.001,
                "t{t}: char{i} sank below ground (y={})",
                fight.chars[i].pos[1]
            );
            let drop = prev_life[i] - fight.chars[i].life;
            // biggest single hit in kfm.cns is ~72 dmg; one tick may resolve
            // at most one hitdef — 100 covers it, anything more is the re-hit
            // bug back again.
            assert!(
                drop <= 100,
                "t{t}: char{i} lost {drop} life in one tick — re-hit bug"
            );
            prev_life[i] = fight.chars[i].life;
        }
        // both alive ⇒ P1 must stay left of P2 (push prevents crossing)
        if fight.chars[0].alive && fight.chars[1].alive {
            assert!(
                fight.chars[0].pos[0] < fight.chars[1].pos[0],
                "t{t}: fighters crossed (P1 x={}, P2 x={})",
                fight.chars[0].pos[0],
                fight.chars[1].pos[0]
            );
        }
    }
    // the fight must have actually connected — some damage happened
    let total = life_max * 2 - fight.chars[0].life - fight.chars[1].life;
    assert!(total > 20, "brawl produced no damage — hit detection dead?");
}

/// 1: a full jump arc must rise (y < 0) and land back at exactly 0, then
/// return to a standing state — never below the floor, never stuck in air.
#[test]
fn jump_arc_stays_above_ground_and_lands() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut up = none();
    up[0] = true; // hold U briefly — through jump start (40→50)
    let mut max_height = 0.0f32;
    for t in 0..90 {
        // release after takeoff so the landing doesn't chain a second jump
        // (MUGEN re-jumps if you keep holding up)
        let inp = if t < 4 { up } else { none() };
        fight.step(&[inp, none()]);
        max_height = max_height.min(fight.chars[0].pos[1]);
        assert!(
            fight.chars[0].pos[1] <= 0.001,
            "t{t}: below ground during jump (y={})",
            fight.chars[0].pos[1]
        );
    }
    assert!(
        max_height < -50.0,
        "jump never rose (peak y={max_height}) — jump states broken"
    );
    assert_eq!(
        fight.chars[0].pos[1], 0.0,
        "jump must end grounded exactly at the floor"
    );
    assert_eq!(
        fight.chars[0].state_no, 0,
        "after landing + releasing U, KFM must be back in state 0 (got {})",
        fight.chars[0].state_no
    );
}

/// 4: holdback must move P1 away from P2 (left) while facing right — and,
/// after a forced turn (facing left), "back" flips with it: the same
/// physical key now walks toward +x… no: back relative to facing is now
/// right. We assert velocity semantics through positions.
#[test]
fn walk_back_follows_facing() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);
    assert!(fight.chars[0].facing > 0, "P1 starts facing right (at P2)");

    // facing right: back = L → x decreases
    let x0 = fight.chars[0].pos[0];
    let mut back = none();
    back[2] = true; // L
    for _ in 0..20 {
        fight.step(&[back, none()]);
    }
    assert!(
        fight.chars[0].pos[0] < x0 - 10.0,
        "facing right + hold L must walk back (x {} → {})",
        x0,
        fight.chars[0].pos[0]
    );

    // force a side switch: teleport P1 past P2; auto_turn flips facing to -1
    // within a few standing frames.
    fight.chars[0].pos[0] = fight.chars[1].pos[0] + 40.0;
    let mut neutral = none();
    for _ in 0..10 {
        fight.step(&[neutral, none()]);
    }
    assert!(
        fight.chars[0].facing < 0,
        "after crossing, auto_turn must flip facing left"
    );

    // facing left: back = R → x increases (away from P2)
    let x1 = fight.chars[0].pos[0];
    let mut back2 = none();
    back2[3] = true; // R
    for _ in 0..20 {
        fight.step(&[back2, none()]);
    }
    assert!(
        fight.chars[0].pos[0] > x1 + 10.0,
        "facing left + hold R must walk back (x {} → {}) — B/F was not facing-relative",
        x1,
        fight.chars[0].pos[0]
    );
}

/// 2 (direct): one jab (state 200) may cost the defender at most its listed
/// damage (23 × attack 100 / defence 100), even though the HitDef stays
/// active for the whole anim — the target latch must hold until a *new*
/// HitDef executes.
#[test]
fn single_jab_costs_single_hit() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    // walk P1 into punch range
    let mut fwd = none();
    fwd[3] = true;
    for _ in 0..30 {
        fight.step(&[fwd, none()]);
    }
    // point-blank X jab: state 200, hitdef triggers on AnimElem 3.
    // ([bool;14] order is U D L R a b c x y z s d w m — X is index 7.)
    let life0 = fight.chars[1].life;
    let mut jab = none();
    jab[7] = true; // X = light punch (state 200, 23 dmg)
    let mut saw_drop = false;
    for t in 0..60 {
        fight.step(&[jab, none()]);
        let drop = life0 - fight.chars[1].life;
        if drop > 0 {
            saw_drop = true;
            assert!(
                drop <= 25,
                "t{t}: jab cost {drop} life so far — re-hitting every frame"
            );
        }
    }
    assert!(
        saw_drop,
        "point-blank jab never connected — hit detection broken"
    );
}
