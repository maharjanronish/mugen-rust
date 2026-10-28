use krg_vm::ini::load_text;
use krg_vm::runtime::{compile_char, Fight};

fn read(p: &str) -> String {
    let b = std::fs::read(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    load_text(&b)
}

fn main() {
    let dir = "crates/krg-vm/tests/fixtures/";
    let cmd = read(&format!("{dir}kfm.cmd"));
    let cns = read(&format!("{dir}kfm.cns"));
    let common_zss = read(&format!("{dir}common1.cns.zss"));
    let common_const = read(&format!("{dir}common.const"));
    let kfm_air = read(&format!("{dir}kfm.air"));
    let common_air = read(&format!("{dir}common.air"));
    let gi = compile_char("KFM", &cmd, &[cns], Some(&common_zss), &common_const, &[common_air, kfm_air]).unwrap();
    println!("states: {} | anims: {}", gi.states.len(), gi.anims.anims.len());
    let mut fight = Fight::new_1v1(gi, 42);
    for _ in 0..80 { fight.step(&[[false; 14]; 2]); }
    let mut input = [false; 14];
    input[3] = true; // R
    for _ in 0..26 { fight.step(&[input, [false; 14]]); }
    let mut x_in = [false; 14];
    x_in[8] = true; // X
    fight.step(&[x_in, [false; 14]]);
    println!("x pressed: cmd x buffered = {}", fight.chars[0].cmd_list.get_state("x"));
    for f in 0..12 {
        fight.step(&[[false; 14]; 2]);
        let c = &fight.chars[0];
        println!("f{f}: st={} t={} pos={:.1} anim={} animtime={} ctrl={} | p2 st={} life={}",
            c.state_no, c.time_in_state, c.pos[0], c.anim.anim_no, c.time_in_state, c.ctrl,
            fight.chars[1].state_no, fight.chars[1].life);
    }
    // print state 20's compiled block summary
    if let Some(sb) = fight.chars[0].gi.states.get(&20) {
        println!("state 20: ctrls={} typ={:?} phys={:?}", sb.block.ctrls.len(), sb.state_type, sb.physics);
    }
}
