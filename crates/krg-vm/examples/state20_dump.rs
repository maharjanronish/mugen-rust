use krg_vm::ini::load_text;
use krg_vm::runtime::{compile_char, CharState, Fight};
use krg_vm::statedef::{run_state_bytecode, Node};

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
    let common_air = read(&format!("{dir}common.air"));
    let kfm_air = read(&format!("{dir}kfm.air"));
    let gi = compile_char("KFM", &cmd, &[cns], Some(&common_zss), &common_const, &[common_air, kfm_air]).unwrap();
    let sb = gi.states.get(&20).unwrap();
    println!("state 20: type={:?} phys={:?} ctrls={} ctrlsps={}",
        sb.state_type, sb.physics, sb.block.ctrls.len(), sb.ctrlsps.len());
    for (i, n) in sb.block.ctrls.iter().enumerate() {
        if let Node::Block(b) = n {
            println!("ctrl[{i}] BLOCK persistent={} pi={} ihp={} has_trigger={}",
                b.persistent, b.persistent_index, b.ignorehitpause, b.trigger.is_some());
            if let Some(t) = &b.trigger {
                println!("  triggerall exprs={} rows={:?}", t.triggerall.len(),
                    t.rows.iter().map(|r| r.as_ref().map(|v| v.len())).collect::<Vec<_>>());
                for ta in &t.triggerall {
                    println!("  triggerall ops: {:?}", ta);
                }
            }
            for (j, c) in b.ctrls.iter().enumerate() {
                match c {
                    Node::Sctrl(sc) => println!("  inner[{j}] sctrl {:?} params={:?}", sc.kind,
                        sc.params.iter().map(|(id, e)| (id, e.len())).collect::<Vec<_>>()),
                    _ => println!("  inner[{j}] other"),
                }
            }
        } else {
            println!("ctrl[{i}] direct");
        }
    }
    // manual run: fresh char with time_in_state=0
    let gi2 = std::sync::Arc::clone(&gi);
    let mut c = CharState::new(gi2, 0, 0, 99);
    c.init_commands();
    c.state_no = 20;
    c.time_in_state = 0;
    run_state_bytecode(sb, &mut c, true);
    println!("after run: vel.x={} (expect 2.4)", c.vel[0]);
}
