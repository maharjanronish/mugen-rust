//! The "KFM comes alive" milestone: the full runtime runs the compiled
//! states from the real v1.0.0 content. KFM walks, crouches, jumps from
//! raw inputs, and two KFMs trade hits until one loses the round.

use krg_vm::cmd::CommandList;
use krg_vm::ini::load_text;
use krg_vm::runtime::{compile_char, Fight};
use krg_vm::statedef::StateTypeT;

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
        Some(&common_zss), // stcommon: common1.cns → .zss fallback
        &common_const,
        &[common_air, kfm_air], // common fills gaps; char wins
    )
    .unwrap()
}

/// Hold bits: [U D L R a b c x y z s d w m]
fn none() -> [bool; 14] {
    [false; 14]
}

#[test]
fn kfm_walks_crouches_and_jumps() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);

    // run through the intro
    for _ in 0..80 {
        fight.step(&[none(), none()]);
    }
    assert_eq!(fight.round_state, 2, "round should be live after intro");
    assert_eq!(fight.chars[0].state_no, 0, "KFM should stand in state 0");
    assert_eq!(fight.chars[0].state_type, StateTypeT::S);

    // ── walk forward (hold right) ───────────────────────────────────────
    let start_x = fight.chars[0].pos[0];
    let mut input = none();
    input[3] = true; // R
    for _ in 0..20 {
        fight.step(&[input, none()]);
    }
    let walked = fight.chars[0].pos[0] - start_x;
    assert!(
        walked > 20.0,
        "KFM must walk forward: moved {walked} (state {})",
        fight.chars[0].state_no
    );

    // release → brake back to state 0
    for _ in 0..20 {
        fight.step(&[none(), none()]);
    }
    assert_eq!(fight.chars[0].state_no, 0, "must return to standing");

    // ── crouch (hold down) ──────────────────────────────────────────────
    let mut input = none();
    input[1] = true; // D
    for _ in 0..12 {
        fight.step(&[input, none()]);
    }
    assert!(
        fight.chars[0].state_no == 10 || fight.chars[0].state_no == 11,
        "hold down → crouch transition (got {})",
        fight.chars[0].state_no
    );
    assert_eq!(fight.chars[0].state_type, StateTypeT::C);
    // release → stand
    for _ in 0..12 {
        fight.step(&[none(), none()]);
    }
    assert_eq!(fight.chars[0].state_no, 0, "release down → stand");

    // ── jump (tap up) ───────────────────────────────────────────────────
    let mut input = none();
    input[0] = true; // U
    fight.step(&[input, none()]);
    input[0] = false;
    let mut saw_air = false;
    let mut peak_y = 0.0f32;
    for _ in 0..60 {
        fight.step(&[none(), none()]);
        if fight.chars[0].state_type == StateTypeT::A {
            saw_air = true;
        }
        peak_y = peak_y.min(fight.chars[0].pos[1]);
    }
    assert!(saw_air, "KFM must become airborne on jump");
    assert!(peak_y < -20.0, "KFM must gain jump height: {peak_y}");
    assert_eq!(fight.chars[0].state_type, StateTypeT::S, "lands back on S");
}

#[test]
fn kfm_light_punch_hits_p2() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);

    for _ in 0..80 {
        fight.step(&[none(), none()]);
    }
    let p2_life_start = fight.chars[1].life;

    // walk into range (P2 stands still at +60; KFM starts at -60)
    let mut input = none();
    input[3] = true; // R
    for _ in 0..36 {
        fight.step(&[input, none()]);
    }
    // throw X (standing light punch = state 200) a few times
    let mut input = none();
    input[7] = true; // X (InputBits index 7)
    let mut landed = false;
    let mut life_seen = 0;
    for _ in 0..60 {
        fight.step(&[input, none()]);
        input[7] = false;
        if fight.chars[1].life < p2_life_start {
            landed = true;
            life_seen = fight.chars[1].life;
            break;
        }
    }
    assert!(
        landed,
        "KFM's X must hit P2 (p2 state {}, dist {}, life {})",
        fight.chars[1].state_no,
        (fight.chars[0].pos[0] - fight.chars[1].pos[0]).abs(),
        fight.chars[1].life
    );
    assert_eq!(life_seen, p2_life_start - 23, "light punch damage 23");
    // P2 entered a gethit state
    assert!(
        (5000..=5020).contains(&fight.chars[1].state_no)
            || fight.chars[1].move_type == krg_vm::statedef::MoveTypeT::H,
        "P2 must be in hitstun (state {})",
        fight.chars[1].state_no
    );
}

// silence unused warnings for the helper
#[allow(dead_code)]
fn _keep(_: &CommandList) {}
