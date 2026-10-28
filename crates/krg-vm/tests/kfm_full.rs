//! The "KFM actually loads" milestone: compile the real Ikemen GO v1.0.0
//! KFM character — cmd commands + cmd-file negative states + kfm.cns
//! constants & states + common1.cns.zss common states — and verify the
//! shapes the runtime depends on.

use krg_vm::cmd::{CmdFileCommands, CommandList};
use krg_vm::cns::{
    read_constants, CharData, CharMovement, CharSize, CharVelocity, StateCompiler,
};
use krg_vm::ini::load_text;
use krg_vm::statedef::{MoveTypeT, Node, StateTypeT};
use krg_vm::zss::compile_zss_text;
use std::collections::HashMap;

fn read(p: &str) -> String {
    let b = std::fs::read(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    load_text(&b)
}

#[test]
fn kfm_full_character_compiles() {
    let kfm_cns = read("tests/fixtures/kfm.cns");
    let kfm_cmd = read("tests/fixtures/kfm.cmd");
    let common_zss = read("tests/fixtures/common1.cns.zss");
    let common_const = read("tests/fixtures/common.const");

    // ── commands (compiler.go:8636-8769) ────────────────────────────────
    let cmds = CmdFileCommands::parse(&kfm_cmd).unwrap();
    assert_eq!(cmds.commands.len(), 37);
    let mut cl = CommandList::new();
    cmds.build(&mut cl).unwrap();

    // ── compiler ctx + named constants (common.const [Constants]) ──────
    let cl_box: &'static CommandList = Box::leak(Box::new(cl));
    let mut sc = StateCompiler::new(cl_box, 0);
    let mut data = CharData::default();
    let mut size = CharSize::default();
    let mut velocity = CharVelocity::default();
    let mut movement = CharMovement::default();
    read_constants(&common_const, &mut sc, &mut data, &mut size, &mut velocity, &mut movement);
    // spot-check a named constant exists (StateStand from common.const)
    assert!(
        sc.named_map.contains_key("statestand"),
        "common.const named constants missing; have {:?}",
        sc.named_map.keys().take(10).collect::<Vec<_>>()
    );

    // ── kfm.cns constants + states (compiler.go:8775: st files first) ───
    read_constants(&kfm_cns, &mut sc, &mut data, &mut size, &mut velocity, &mut movement);
    assert_eq!(data.life, 1000);
    assert_eq!(data.attack, 100);
    assert_eq!(size.ground_back, 15.0);
    assert!((velocity.walk_fwd - 2.4).abs() < 1e-5, "walk fwd {}", velocity.walk_fwd);
    assert!((movement.yaccel - 0.44).abs() < 1e-5);

    let mut states: HashMap<_, _> = HashMap::new();
    // KFM is a Mugen-version char → negoverride = true (its -1 states win)
    sc.compile_cns_text(&kfm_cns, "kfm.cns", true, &mut states)
        .unwrap_or_else(|e| panic!("kfm.cns failed: {e}"));

    // ── cmd-file states ([Statedef -1] area of kfm.cmd) ─────────────────
    sc.compile_cns_text(&kfm_cmd, "kfm.cmd", true, &mut states)
        .unwrap_or_else(|e| panic!("kfm.cmd states failed: {e}"));

    // ── common states (stcommon → common1.cns fallback → .zss) ─────────
    compile_zss_text(&mut sc, &common_zss, "common1.cns.zss", false, &mut states)
        .unwrap_or_else(|e| panic!("common1.cns.zss failed: {e}"));

    // ── structural assertions ────────────────────────────────────────────
    // KFM's own states (actual list from kfm.cns)
    for sn in [-3, 170, 180, 181, 191, 195, 200, 210, 230, 240, 400, 410,
               430, 440, 600, 610, 630, 640, 800, 810, 820, 821, 1000, 1010,
               1020, 1025, 1026, 1027, 1028, 1050, 1051, 1052, 1055, 1056,
               1060, 1061, 1070, 1071, 1075, 1100, 1110, 1120, 1200, 1210,
               1220, 1300, 1310, 1320, 1330, 1340, 1350, 1351, 1400, 1410,
               1420, 3000, 3050, 3051] {
        assert!(states.contains_key(&sn), "KFM state {sn} missing");
    }
    // common1 states
    for sn in [20, 40, 45, 50, 51, 52, 100, 105, 106, 110, 115, 120, 130, 131,
               132, 140, 150, 152, 154, 170, 175, 190, 191, 5000, 5001, 5010,
               5020, 5030, 5050, 5070, 5080, 5100, 5110, 5120, 5150, 5200,
               5500, 5900] {
        assert!(states.contains_key(&sn), "common state {sn} missing");
    }
    // negative states: -1 from kfm.cmd (Mugen-version char wins), -3 from kfm.cns
    assert!(states.contains_key(&-1), "cmd -1 states missing");
    assert!(states.contains_key(&-3), "kfm.cns -3 states missing");

    // ── verify a few compiled shapes ────────────────────────────────────
    let s200 = &states[&200];
    assert_eq!(s200.state_type, StateTypeT::S);
    assert_eq!(s200.physics, StateTypeT::S);
    assert_eq!(s200.move_type, MoveTypeT::A);
    // 200 has HitDef + PlaySnd + ChangeState at least
    assert!(s200.block.ctrls.len() >= 3, "state 200 ctrls: {}", s200.block.ctrls.len());

    // state 20 (common walk) — from ZSS, verify ZSS shapes exist
    let s20 = &states[&20];
    assert_eq!(s20.state_type, StateTypeT::S);
    assert!(s20.block.ctrls.len() >= 2, "state 20 ctrls: {}", s20.block.ctrls.len());
    // one of its nodes is an if-block (ZSS if)
    let mut has_block = false;
    for n in &s20.block.ctrls {
        if let Node::Block(_) = n {
            has_block = true;
            break;
        }
    }
    assert!(has_block, "ZSS if-blocks missing in state 20");

    // ── count states compiled for the report ─────────────────────────────
    println!("KFM compiled: {} states, {} commands, {} named consts, {} warnings",
        states.len(), cl_box.len(), sc.named.len(), sc.warnings.len());
    println!("first 10 warnings: {:#?}", sc.warnings.iter().take(10).collect::<Vec<_>>());
}
