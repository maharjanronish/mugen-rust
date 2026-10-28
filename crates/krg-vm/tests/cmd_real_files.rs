//! Real-content integration: compile the actual Ikemen GO v1.0.0
//! `chars/kfm/kfm.cmd` + `data/common.cmd` and exercise command matching
//! against scripted input timelines.
//!
//! The fixtures are verbatim copies from the official release zip — any
//! parse regression against real MUGEN content fails here, not on a phone.

use krg_vm::cmd::{compile_cmd_text, CommandList, SOCD_ALLOW};

fn fixture(name: &str) -> String {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(p).expect("fixture present")
}

/// Go's loader appends common cmd text after the char's own (compiler.go:8628).
fn compile_real_kfm() -> CommandList {
    let text = format!("{}\n{}", fixture("kfm.cmd"), fixture("common.cmd"));
    let mut l = CommandList::new();
    let mut w = Vec::new();
    compile_cmd_text(&mut l, &text, "Kung Fu Man", false, &mut w)
        .expect("real kfm.cmd compiles");
    assert!(w.is_empty(), "no compat warnings for an Ikemen-version char: {w:?}");
    l
}

const NEUTRAL: [bool; 14] = [false; 14];

fn frame(l: &mut CommandList, b: [bool; 14]) {
    l.input_update(b, true, false, 0, false, true, false, SOCD_ALLOW);
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
        assert!(!l.get(name).is_empty(), "missing command {name}");
    }
    // TripleKFPalm has TWO same-name variants (kfm.cmd defines it twice)
    assert_eq!(l.get("TripleKFPalm").len(), 2);
}

/// Walk forward: /$F must report true every frame while R is held.
#[test]
fn real_kfm_holdfwd() {
    let mut l = compile_real_kfm();
    settle(&mut l, 12);
    let mut f = NEUTRAL;
    f[3] = true; // R = F (facing right)
    for _ in 0..5 {
        frame(&mut l, f);
        assert!(l.get_state("holdfwd"), "/$F held");
        assert!(!l.get_state("holdback"));
    }
}

/// kfm's actual QCF_a: "~D, DF, F, a" — release-then-roll motion.
#[test]
fn real_kfm_qcf_a_motion() {
    let mut l = compile_real_kfm();
    settle(&mut l, 12);

    // tap down and release
    let mut f = NEUTRAL;
    f[1] = true;
    frame(&mut l, f);
    let f = NEUTRAL;
    frame(&mut l, f);
    // D+R (DF)
    let mut f = NEUTRAL;
    f[1] = true;
    f[3] = true;
    frame(&mut l, f);
    // release D, keep R (F)
    let mut f = NEUTRAL;
    f[3] = true;
    frame(&mut l, f);
    // press a
    let mut f = NEUTRAL;
    f[3] = true;
    f[4] = true;
    frame(&mut l, f);
    assert!(l.get_state("QCF_a"), "kfm QCF_a matched");
}

/// TripleKFPalm: "~D, DF, F, D, DF, F, x" — the double quarter-circle super.
#[test]
fn real_kfm_triple_palm_super() {
    let mut l = compile_real_kfm();
    settle(&mut l, 12);

    for _round in 0..2 {
        // ~D (tap-release)
        let mut f = NEUTRAL;
        f[1] = true;
        frame(&mut l, f);
        frame(&mut l, NEUTRAL);
        // DF then F
        let mut f = NEUTRAL;
        f[1] = true;
        f[3] = true;
        frame(&mut l, f);
        let mut f = NEUTRAL;
        f[3] = true;
        frame(&mut l, f);
    }
    // finish with x
    let mut f = NEUTRAL;
    f[3] = true;
    f[7] = true;
    frame(&mut l, f);
    assert!(l.get_state("TripleKFPalm"), "super motion matched");
}

/// FF dash on the real definition (auto-greater expanded).
#[test]
fn real_kfm_ff_dash() {
    let mut l = compile_real_kfm();
    settle(&mut l, 12);
    let mut f = NEUTRAL;
    f[3] = true;
    frame(&mut l, f);
    frame(&mut l, NEUTRAL);
    let mut f = NEUTRAL;
    f[3] = true;
    frame(&mut l, f);
    assert!(l.get_state("FF"));
}

/// The blank commands from common.cmd stay inert but assertable.
#[test]
fn real_kfm_common_blank_commands_assert() {
    let mut l = compile_real_kfm();
    settle(&mut l, 12);
    assert!(!l.get_state("menu"));
    assert!(l.assert("menu", 2));
    assert!(l.get_state("menu"));
}
