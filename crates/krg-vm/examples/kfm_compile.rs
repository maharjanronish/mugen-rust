use krg_vm::cmd::{CmdFileCommands, CommandList};
use krg_vm::cns::{read_constants, CharData, CharMovement, CharSize, CharVelocity, StateCompiler};
use krg_vm::ini::load_text;
use std::collections::HashMap;

fn main() {
    let dir = "crates/krg-vm/tests/fixtures/";
    let kfm_cns = load_text(&std::fs::read(dir.to_string() + "kfm.cns").unwrap());
    let kfm_cmd = load_text(&std::fs::read(dir.to_string() + "kfm.cmd").unwrap());
    let common_const = load_text(&std::fs::read(dir.to_string() + "common.const").unwrap());
    let cmds = CmdFileCommands::parse(&kfm_cmd).unwrap();
    let mut cl_owned = CommandList::new();
    cmds.build(&mut cl_owned).unwrap();
    let cl: &'static CommandList = Box::leak(Box::new(cl_owned));
    let mut sc = StateCompiler::new(cl, 0);
    let (mut d, mut sz, mut v, mut mv) = (CharData::default(), CharSize::default(), CharVelocity::default(), CharMovement::default());
    read_constants(&common_const, &mut sc, &mut d, &mut sz, &mut v, &mut mv);
    read_constants(&kfm_cns, &mut sc, &mut d, &mut sz, &mut v, &mut mv);

    // 1. kfm.cns whole
    let mut states = HashMap::new();
    if let Err(e) = sc.compile_cns_text(&kfm_cns, "kfm.cns", true, &mut states) {
        println!("kfm.cns FAIL: {e}");
    }

    // 2. kfm.cmd WHOLE FILE (same as the real test)
    if let Err(e) = sc.compile_cns_text(&kfm_cmd, "kfm.cmd", true, &mut states) {
        println!("kfm.cmd WHOLE FAIL: {e}");
        // bisect: find the failing [State] by compiling one at a time
        let mut blocks: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut hold: Vec<String> = Vec::new();
        for line in kfm_cmd.lines() {
            let l = line.trim().to_lowercase();
            if l.starts_with("[state") || l.starts_with("[statedef") {
                if !cur.trim().is_empty() {
                    hold.push(std::mem::take(&mut cur));
                }
            }
            cur.push_str(line); cur.push('\n');
        }
        hold.push(cur);
        for (bi, b) in hold.iter().enumerate() {
            // each block prefixed with its statedef? blocks need statedef context:
            if let Err(e2) = sc.compile_cns_text(b, "blk", true, &mut states) {
                if e2 == e {
                    println!("  culprit block {bi}: {e2}");
                    for l in b.lines().take(10) { println!("  | {l}"); }
                }
            }
        }
    }
    println!("done ({} states)", states.len());
}
// (zss check happens in the kfm_full test)
