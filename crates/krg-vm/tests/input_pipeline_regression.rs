//! Full-pipeline simulation: TouchController (event-driven, like Android
//! MotionEvent) → held sampling (like the GL thread reads controller_held)
//! → Fight::step_bits — replicates the device input path exactly.

use krg_input::{TouchController, TouchPhase};
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

// Reference screen: 1600×900 landscape (pad center 0.135w, 0.60h, r 0.115m).
const SW: f32 = 1600.0;
const SH: f32 = 900.0;
const PAD_CX: f32 = 0.135 * SW;
const PAD_CY: f32 = 0.60 * SH;
const PAD_R: f32 = 0.115 * SH.min(SW);

/// One "device frame": touch events first (UI thread), then Choreographer
/// tick, then the GL thread samples held. Returns the sampled held bits.
fn device_frame(tc: &mut TouchController, events: &[(u64, TouchPhase, f32, f32)]) -> u32 {
    for (id, phase, x, y) in events {
        tc.touch(*id, *phase, *x, *y, SW, SH);
    }
    tc.tick()
}

fn none_input() -> [bool; 14] {
    [false; 14]
}

fn bits_to_arr(b: u32) -> [bool; 14] {
    [
        b & 1 << 0 != 0, b & 1 << 1 != 0, b & 1 << 2 != 0, b & 1 << 3 != 0,
        b & 1 << 4 != 0, b & 1 << 5 != 0, b & 1 << 6 != 0, b & 1 << 7 != 0,
        b & 1 << 8 != 0, b & 1 << 9 != 0, b & 1 << 10 != 0, b & 1 << 11 != 0,
        b & 1 << 12 != 0, b & 1 << 13 != 0,
    ]
}

#[test]
fn pipeline_drag_forward() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    // skip intro
    let mut tc = TouchController::new();
    for _ in 0..80 {
        let held = device_frame(&mut tc, &[]);
        fight.step(&[bits_to_arr(held), none_input()]);
    }
    let x0 = fight.chars[0].pos[0];

    // A drag: DOWN at pad center, then MOVE right in ~8px steps for 240
    // frames (4 seconds of holding the stick right).
    let mut pid = 0u64;
    let mut x = PAD_CX;
    let y = PAD_CY;
    let mut events: Vec<(u64, TouchPhase, f32, f32)> =
        vec![(pid, TouchPhase::Down, x, y)];
    for _ in 0..240 {
        // move right 6px per frame (a steady drag)
        x += 6.0;
        if x > PAD_CX + PAD_R * 3.0 {
            x = PAD_CX + PAD_R * 3.0; // hold it there
        }
        events.push((pid, TouchPhase::Move, x, y));
        let held = device_frame(&mut tc, &events);
        events.clear();
        fight.step(&[bits_to_arr(held), none_input()]);
    }
    let x1 = fight.chars[0].pos[0];
    println!(
        "drag: x0={x0} x1={x1} progress={} state={}",
        x1 - x0,
        fight.chars[0].state_no
    );
    assert!(x1 - x0 > 100.0, "drag failed: only moved {}", x1 - x0);
}

#[test]
fn pipeline_drag_started_outside_capture_circle() {
    // Users pushing a "joystick" often land the first touch on the far edge
    // of the visible ring (or just outside it). The pad capture circle is
    // 1.40r now, so such drags must MOVE the character (they were dead at
    // 1.10r — the device "can't move by dragging" bug).
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    let mut tc = TouchController::new();
    for _ in 0..80 {
        let held = device_frame(&mut tc, &[]);
        fight.step(&[bits_to_arr(held), none_input()]);
    }
    let x0 = fight.chars[0].pos[0];

    // DOWN just outside the drawn ring (right side + 25%)
    let mut pid = 0u64;
    let mut x = PAD_CX + PAD_R * 1.25;
    let y = PAD_CY;
    let mut events: Vec<(u64, TouchPhase, f32, f32)> =
        vec![(pid, TouchPhase::Down, x, y)];
    for _ in 0..240 {
        x += 2.0;
        events.push((pid, TouchPhase::Move, x, y));
        let held = device_frame(&mut tc, &events);
        events.clear();
        fight.step(&[bits_to_arr(held), none_input()]);
    }
    let x1 = fight.chars[0].pos[0];
    println!(
        "edge-drag: x0={x0} x1={x1} progress={} state={}",
        x1 - x0,
        fight.chars[0].state_no
    );
    assert!(
        x1 - x0 > 100.0,
        "edge-start drag is dead: only moved {}",
        x1 - x0
    );
}

#[test]
fn pipeline_fast_crouch_flicker() {
    // Fast stick movement: finger sweeps center→down→near-deadzone→down…
    // The pad has hysteresis now, so a finger hovering around the engage
    // boundary must NOT flicker the direction: no state churn, and the
    // char stays grounded (no accidental jumps/hops → no "floor is an
    // incline, it goes up").
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    let mut tc = TouchController::new();
    for _ in 0..80 {
        let held = device_frame(&mut tc, &[]);
        fight.step(&[bits_to_arr(held), none_input()]);
    }

    let mut pid = 0u64;
    let y0 = PAD_CY;
    let mut events: Vec<(u64, TouchPhase, f32, f32)> =
        vec![(pid, TouchPhase::Down, PAD_CX, y0)];
    // push down past the engage line ONCE, then tremble in the hold band
    let mut flips = 0;
    let mut last_pd = false;
    let mut state_changes = 0;
    let mut last_state = fight.chars[0].state_no;
    let mut worst_y = 0.0f32;
    // first engage the down direction decisively
    events.push((pid, TouchPhase::Move, PAD_CX, y0 + PAD_R * 0.6));
    let held = device_frame(&mut tc, &events);
    events.clear();
    fight.step(&[bits_to_arr(held), none_input()]);
    assert!(held & (1 << 1) != 0, "down never engaged at 0.60r");
    // bookkeeping starts from the ALREADY-engaged state (the pre-loop step
    // above turned PD on and entered the crouch chain — those don't count)
    last_pd = true;
    last_state = fight.chars[0].state_no;
    for t in 0..600 {
        // finger trembles INSIDE the hold band (0.22r..0.38r around engage)
        let osc = 0.30 + 0.06 * ((t % 7) as f32 - 3.0).abs() / 3.0;
        let ev = (pid, TouchPhase::Move, PAD_CX, y0 + osc * PAD_R);
        events.push(ev);
        let held = device_frame(&mut tc, &events);
        events.clear();
        fight.step(&[bits_to_arr(held), none_input()]);
        let pd = held & (1 << 1) != 0;
        if pd != last_pd {
            flips += 1;
            last_pd = pd;
        }
        worst_y = worst_y.min(fight.chars[0].pos[1]);
        if fight.chars[0].state_no != last_state {
            state_changes += 1;
            last_state = fight.chars[0].state_no;
        }
    }
    println!("pd_flips={flips} state_changes={state_changes} worst_y={worst_y}");
    // The whole point of hysteresis: ZERO direction flips through the
    // tremble band, so the crouch settles (at most 10→11 after the anim
    // finishes) and the char stays on the floor.
    assert_eq!(flips, 0, "direction flickered {flips}× in the hold band");
    assert!(
        state_changes <= 2,
        "state churned {state_changes}× while holding crouch"
    );
    assert!(worst_y > -1.0, "char rose to y={worst_y} while crouch-holding");
}

#[test]
fn pipeline_double_tap_then_hold() {
    // double-tap right (each tap = 3 frames down + 3 frames up), then hold
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    let mut tc = TouchController::new();
    for _ in 0..80 {
        let held = device_frame(&mut tc, &[]);
        fight.step(&[bits_to_arr(held), none_input()]);
    }

    let mut pid = 0u64;
    let y = PAD_CY;
    let right = PAD_CX + PAD_R * 0.8;
    let mut events: Vec<(u64, TouchPhase, f32, f32)> = Vec::new();
    let mut saw_run = false;
    let x0 = fight.chars[0].pos[0];
    for t in 0..300 {
        // tap 1: 0-2 down, 3-5 up; tap 2: 6-8 down, then hold
        let (phase, x) = match t {
            0 => (TouchPhase::Down, right),
            1..=2 => (TouchPhase::Move, right),
            3 => (TouchPhase::Up, right),
            6 => (TouchPhase::Down, right),
            _ => (TouchPhase::Move, right),
        };
        events.push((pid, phase, x, y));
        let held = device_frame(&mut tc, &events);
        events.clear();
        fight.step(&[bits_to_arr(held), none_input()]);
        if fight.chars[0].state_no == 100 {
            saw_run = true;
        }
    }
    let x1 = fight.chars[0].pos[0];
    println!(
        "double-tap-hold: saw_run={saw_run} progress={} state={}",
        x1 - x0,
        fight.chars[0].state_no
    );
    assert!(saw_run, "double-tap run never fired through the pipeline");
}

/// Double-tap-BACK fires the hop (BB → state 105, the character briefly
/// goes airborne — correct MUGEN). It must LAND and return to standing
/// within a bounded time: no freeze mid-air, no stuck hop chain.
#[test]
fn pipeline_double_tap_back_hops_and_lands() {
    let gi = compile_kfm();
    let mut fight = Fight::new_1v1(gi, 42);
    let mut tc = TouchController::new();
    for _ in 0..80 {
        let held = device_frame(&mut tc, &[]);
        fight.step(&[bits_to_arr(held), none_input()]);
    }

    let mut pid = 0u64;
    let y = PAD_CY;
    let left = PAD_CX - PAD_R * 0.8;
    let mut events: Vec<(u64, TouchPhase, f32, f32)> = Vec::new();
    let x0 = fight.chars[0].pos[0];
    let mut saw_hop = false;
    let mut airborne_after_hop = 0usize;
    let mut hop_done = false;
    for t in 0..120 {
        // tap-tap-hold left: 3 down, 3 up, 3 down, then hold
        let (phase, x) = match t {
            0 => (TouchPhase::Down, left),
            1..=2 => (TouchPhase::Move, left),
            3 => (TouchPhase::Up, left),
            6 => (TouchPhase::Down, left),
            _ => (TouchPhase::Move, left),
        };
        events.push((pid, phase, x, y));
        let held = device_frame(&mut tc, &events);
        events.clear();
        fight.step(&[bits_to_arr(held), none_input()]);
        let c = &fight.chars[0];
        if c.state_no == 105 {
            saw_hop = true;
        }
        if saw_hop && !hop_done {
            if c.state_type == krg_vm::statedef::StateTypeT::A || c.pos[1] < 0.0 {
                airborne_after_hop += 1;
            } else if airborne_after_hop > 0 {
                hop_done = true; // back on the ground after the hop
            }
        }
    }
    // after the gesture: release and settle
    events.push((pid, TouchPhase::Up, left, y));
    let held = device_frame(&mut tc, &events);
    events.clear();
    fight.step(&[bits_to_arr(held), none_input()]);
    for _ in 0..60 {
        let held = device_frame(&mut tc, &[]);
        fight.step(&[bits_to_arr(held), none_input()]);
    }
    let c = &fight.chars[0];
    let x1 = c.pos[0];
    println!(
        "back-hop: saw_hop={saw_hop} airborne_ticks={airborne_after_hop} landed={} final_state={} x0={x0} x1={x1} y={}",
        hop_done, c.state_no, c.pos[1]
    );
    assert!(saw_hop, "BB never fired through the pipeline");
    assert!(hop_done, "hop never landed (frozen mid-air?)");
    assert!(airborne_after_hop < 60, "airborne {airborne_after_hop} ticks — float/freeze");
    assert_eq!(c.state_no, 0, "stuck in state {} after the hop", c.state_no);
    assert!(c.pos[1] >= -0.01, "char left the floor (y={})", c.pos[1]);
    assert!(x1 < x0 - 20.0, "hop made no backward progress");
}
