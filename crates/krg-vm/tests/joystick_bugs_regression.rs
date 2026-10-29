//! Reproduction tests for the v0.9.3 device bugs (joystick feel):
//!
//! 1. **Drag doesn't walk** — double-tap-forward runs (FF command), but
//!    holding the pad forward (drag) doesn't walk; quick taps do move.
//! 2. **Walking back freezes** — holding back gets stuck.
//! 3. **Rapid crouch/stand** — fast joystick toggles look janky; suspected
//!    floor-coordinate confusion (char treats floor as incline and rises).

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

/// Bug 1: hold forward (drag) — the char must walk continuously.
#[test]
fn diag_hold_forward_walks() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut fwd = none();
    fwd[3] = true; // R held the whole time (a drag)

    let mut last_x = fight.chars[0].pos[0];
    let mut stalled = 0usize;
    let mut trace: Vec<String> = Vec::new();
    for t in 0..240 {
        fight.step(&[fwd, none()]);
        let c = &fight.chars[0];
        if t % 20 == 0 {
            trace.push(format!(
                "t={:3} st={:4} x={:8.2} vx={:6.2} anim={:4} ctrl={} ty={:?}",
                t, c.state_no, c.pos[0], c.vel[0], c.anim.anim_no, c.ctrl, c.state_type
            ));
        }
        if c.pos[0] <= last_x + 0.01 {
            stalled += 1;
        }
        last_x = c.pos[0];
    }
    for row in &trace {
        println!("{row}");
    }
    let progress = fight.chars[0].pos[0] - (-60.0);
    println!("progress={progress} stalled_ticks={stalled}");
    // 240 ticks at ~2.3 px/tick walking ≈ 500px, minus camera clamp; a drag
    // must produce sustained progress, not just the initial tap steps.
    assert!(progress > 100.0, "drag-walk only moved {progress} in 4s");
}

/// Bug 2: hold back — must walk backwards continuously (no freeze).
#[test]
fn diag_hold_back_walks() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut back = none();
    back[2] = true; // L held (P1 faces right → back)

    let mut last_x = fight.chars[0].pos[0];
    let mut stalled = 0usize;
    let mut trace: Vec<String> = Vec::new();
    for t in 0..240 {
        fight.step(&[back, none()]);
        let c = &fight.chars[0];
        if t % 20 == 0 {
            trace.push(format!(
                "t={:3} st={:4} x={:8.2} vx={:6.2} anim={:4} ctrl={} ty={:?}",
                t, c.state_no, c.pos[0], c.vel[0], c.anim.anim_no, c.ctrl, c.state_type
            ));
        }
        if c.pos[0] >= last_x - 0.01 {
            stalled += 1;
        }
        last_x = c.pos[0];
    }
    for row in &trace {
        println!("{row}");
    }
    let regress = -60.0 - fight.chars[0].pos[0];
    println!("backward progress={regress} stalled_ticks={stalled}");
    assert!(regress > 100.0, "back-walk froze: only {regress} in 4s");
}

/// Bug 3: rapid crouch/stand toggling (fast joystick wiggling on the pad).
/// Track y: the char must NEVER leave the floor (y >= -epsilon) while
/// grounded, and the state machine must settle (no livelock).
#[test]
fn diag_rapid_crouch_toggle() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut down = none();
    down[1] = true; // D

    // wiggle: down 2 ticks, up 1 tick, down 3, up 2, ... (fast stick noise)
    let mut worst_y = 0.0f32;
    let mut state_changes = 0usize;
    let mut last_state = fight.chars[0].state_no;
    let mut trace: Vec<String> = Vec::new();
    for t in 0..600 {
        let held_down = matches!(t % 7, 0..=3);
        let inp = if held_down { down } else { none() };
        fight.step(&[inp, none()]);
        let c = &fight.chars[0];
        worst_y = worst_y.min(c.pos[1]);
        if c.state_no != last_state {
            state_changes += 1;
            last_state = c.state_no;
        }
        if t % 50 == 0 {
            trace.push(format!(
                "t={:3} st={:4} x={:8.2} y={:7.2} vx={:6.2} vy={:6.2} ty={:?}",
                t, c.state_no, c.pos[0], c.pos[1], c.vel[0], c.vel[1], c.state_type
            ));
        }
    }
    for row in &trace {
        println!("{row}");
    }
    println!("worst_y={worst_y} state_changes={state_changes}");
    // The char wiggling crouch on the ground must not rise off the floor.
    assert!(
        worst_y > -1.0,
        "floor confusion: char rose to y={worst_y} while crouch-toggling"
    );
}

/// Double-tap-forward then KEEP holding (the drag-after-run gesture):
/// state 100 (run) must run while F is held (until the stage edge —
/// running in place against the wall is correct MUGEN), and must EXIT
/// to 0 promptly once F is released (no stuck run).
#[test]
fn diag_run_then_hold_keeps_running() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    skip_intro(&mut fight);

    let mut fwd = none();
    fwd[3] = true;

    // tap (3 ticks), release (3 ticks), tap (3), then HOLD for 2 seconds
    let mut saw_run = false;
    let mut x_at_run_start = 0.0f32;
    let mut trace: Vec<String> = Vec::new();
    for t in 0..120 {
        let inp = match t {
            0..=2 => fwd,     // first tap
            3..=5 => none(),  // release
            6..=8 => fwd,     // second tap → FF fires
            _ => fwd,         // keep holding (the drag)
        };
        fight.step(&[inp, none()]);
        let c = &fight.chars[0];
        if c.state_no == 100 && !saw_run {
            saw_run = true;
            x_at_run_start = c.pos[0];
        }
        if t % 30 == 0 {
            trace.push(format!(
                "t={:3} st={:4} x={:8.2} vx={:6.2} ctrl={}",
                t, c.state_no, c.pos[0], c.vel[0], c.ctrl
            ));
        }
    }
    let run_progress = fight.chars[0].pos[0] - x_at_run_start;
    for row in &trace {
        println!("{row}");
    }
    // release F: the run must exit to 0 within a few ticks (state 100's
    // `command != "holdfwd" → changeState 0`)
    let mut exit_ticks = 0usize;
    for t in 0..30 {
        fight.step(&[none(), none()]);
        if fight.chars[0].state_no == 100 {
            exit_ticks = t + 1;
        }
    }
    let final_state = fight.chars[0].state_no;
    println!(
        "saw_run={saw_run} run_progress={run_progress} exit_ticks={exit_ticks} final_state={final_state}"
    );
    assert!(saw_run, "FF never fired — double-tap run broken");
    assert!(run_progress > 100.0, "run made no progress ({run_progress})");
    assert!(
        final_state != 100,
        "stuck in run state 30 ticks after releasing F"
    );
    assert!(exit_ticks <= 3, "run exited only after {exit_ticks} ticks");
}
