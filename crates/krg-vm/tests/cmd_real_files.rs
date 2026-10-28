//! Real-content integration: compile the actual Ikemen GO v1.0.0
//! `chars/kfm/kfm.cmd` + `data/common.cmd` and exercise command matching
//! against scripted input timelines.
//!
//! The fixtures are verbatim copies from the official release zip — any
//! parse regression against real MUGEN content fails here, not on a phone.

use krg_vm::cmd::{CmdFileCommands, CommandList};

fn fixture(name: &str) -> String {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let b = std::fs::read(p).expect("fixture present");
    krg_vm::ini::load_text(&b)
}

/// Go's loader appends common cmd text after the char's own (compiler.go:8628).
fn compile_real_kfm() -> CommandList {
    let text = format!("{}\n{}", fixture("kfm.cmd"), fixture("common.cmd"));
    let cmds = CmdFileCommands::parse(&text).expect("real kfm.cmd compiles");
    let mut l = CommandList::new();
    cmds.build(&mut l).expect("real commands compile");
    l
}

const NEUTRAL: [bool; 14] = [false; 14];

fn frame(l: &mut CommandList, b: [bool; 14]) {
    let [u, d, lf, rt, a, b2, c, x, y, z, s, dd, w, m] = b;
    // facing right: back = L, forward = R
    l.buffer.update_input_time(u, d, lf, rt, lf, rt, a, b2, c, x, y, z, s, dd, w, m);
    l.step(false, false, false, false, 0);
}

fn settle(l: &mut CommandList, n: usize) {
    for _ in 0..n {
        frame(l, NEUTRAL);
    }
}

/// kfm defines these; common.cmd merges in the blank placeholder commands.
#[test]
fn real_kfm_compiles_with_expected_groups() {
    let l = compile_real_kfm();
    for name in [
        "FF", "BB", "QCF_a", "QCF_b", "QCF_x", "QCF_y", "QCF_xy", "QCB_x",
        "TripleKFPalm", "SmashKFUpper", "blocking", "recovery",
        "holdfwd", "holdback", "holdup", "holddown",
        // from common.cmd
        "x", "y", "z", "a", "b", "c", "start", "menu",
    ] {
        assert!(l.get(name).is_some(), "missing command {name}");
    }
    // TripleKFPalm has TWO same-name variants (kfm.cmd defines it twice)
    assert_eq!(l.get("TripleKFPalm").unwrap().len(), 2);
}

/// Walk forward: /$F must report true every frame while R is held.
#[test]
fn real_kfm_holdfwd() {
    let mut l = compile_real_kfm();
    settle(&mut l, 12);
    let mut held = NEUTRAL;
    held[3] = true; // R
    for _ in 0..6 {
        frame(&mut l, held);
        assert!(l.get_state("holdfwd"), "/F held must buffer");
    }
    frame(&mut l, NEUTRAL);
    assert!(!l.get_state("holdfwd"), "released → not buffered");
}

/// QCF + x from a clean quarter-circle on real KFM command defs.
#[test]
fn real_kfm_qcf_x() {
    let mut l = compile_real_kfm();
    settle(&mut l, 12);
    let qcf = [
        [false, true, false, false, false, false, false, false, false, false, false, false, false, false],
        [false, true, false, false, false, false, false, false, false, false, false, false, false, false],
        [false, true, false, true, false, false, false, false, false, false, false, false, false, false],
        [false, true, false, true, false, false, false, false, false, false, false, false, false, false],
        [false, false, false, true, false, false, false, false, false, false, false, false, false, false],
        [false, false, false, true, false, false, false, false, false, false, false, false, false, false],
    ];
    for f in qcf {
        frame(&mut l, f);
    }
    let mut press = NEUTRAL;
    press[7] = true; // X
    frame(&mut l, press);
    assert!(l.get_state("QCF_x"), "real QCF+x must fire (23, 5 in kfm.cmd)");
}

/// Double-tap forward dash: F, F with autogreater expansion.
#[test]
fn real_kfm_dash() {
    let mut l = compile_real_kfm();
    settle(&mut l, 12);
    for held in [true, true, false, true, true] {
        let mut f = NEUTRAL;
        f[3] = held; // R
        frame(&mut l, f);
        if held && l.get_state("FF") {
            return;
        }
    }
    panic!("F,F dash (kfm FF command) never fired");
}

/// blank command definitions from common.cmd (holdback etc.) compile
/// with the first-empty step semantics and never fire spuriously.
#[test]
fn real_kfm_blank_common_commands_inert() {
    let mut l = compile_real_kfm();
    settle(&mut l, 30);
    for name in ["x", "y", "z", "a", "b", "c"] {
        assert!(
            !l.get_state(name),
            "{name} must not fire on neutral (blank common defs)"
        );
    }
}
