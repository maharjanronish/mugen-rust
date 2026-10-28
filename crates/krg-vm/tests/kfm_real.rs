//! Real-world conformance: compile every command in the actual KFM cmd
//! file (Ikemen GO v1.0.0 release) and drive a handful of them.

use krg_vm::cmd::*;
use krg_vm::ini::load_text;

fn kfm_cmd() -> String {
    let bytes = std::fs::read("tests/fixtures/kfm.cmd").expect("kfm.cmd fixture");
    load_text(&bytes)
}

fn feed(ib: &mut InputBuffer, u: bool, d: bool, l: bool, r: bool, btn: Btn) {
    let (a, b, c, x, y, z) = match btn {
        Btn::A => (true, false, false, false, false, false),
        Btn::B => (false, true, false, false, false, false),
        Btn::C => (false, false, true, false, false, false),
        Btn::X => (false, false, false, true, false, false),
        Btn::Y => (false, false, false, false, true, false),
        Btn::Z => (false, false, false, false, false, true),
        Btn::None => (false, false, false, false, false, false),
    };
    ib.update_input_time(u, d, l, r, l, r, a, b, c, x, y, z, false, false, false, false);
}

#[derive(Clone, Copy)]
enum Btn {
    A,
    B,
    C,
    X,
    Y,
    Z,
    None,
}

#[test]
fn compile_all_37_kfm_commands() {
    let text = kfm_cmd();
    let cmds = CmdFileCommands::parse(&text).expect("kfm.cmd parses");
    assert_eq!(cmds.commands.len(), 37);
    let mut list = CommandList::new();
    cmds.build(&mut list).expect("all commands compile");
    // spot-check the names KFM's states reference
    for name in ["holdfwd", "holdback", "holdup", "holddown", "x", "y", "z",
                 "a", "b", "c", "start", "QCF_x", "QCF_y", "QCF_a", "QCF_b",
                 "QCF_xy", "QCB_xy", "QCF_ab", "FF", "BB", "down_b", "recovery", "TripleKFPalm",
                 "SmashKFUpper", "blocking", "upper_xy", "FF_ab", "down_a"] {
        assert!(
            list.get(name).is_some(),
            "KFM command missing: {name}"
        );
    }
}

#[test]
fn kfm_qcf_x_fires() {
    let mut list = CommandList::new();
    let cmds = CmdFileCommands::parse(&kfm_cmd()).unwrap();
    cmds.build(&mut list).unwrap();
    let frames: [(bool, bool, bool, bool, Btn); 8] = [
        (false, true, false, false, Btn::None),  // D
        (false, true, false, false, Btn::None),
        (false, true, false, true, Btn::None),   // DF
        (false, true, false, true, Btn::None),
        (false, false, false, true, Btn::None),  // F
        (false, false, false, true, Btn::None),
        (false, false, false, true, Btn::X),     // x → fires
        (false, false, false, false, Btn::None), // (buffer decrement frame)
    ];
    for (u, d, l, r, btn) in frames {
        {
            let b = &mut list.buffer;
            feed(b, u, d, l, r, btn);
        }
        let fired_this_frame = list.get_state("QCF_x");
        list.step(false, false, false, false, 0);
        if fired_this_frame {
            assert!(true, "QCF_x fired");
            return;
        }
    }
    panic!("QCF_x never fired with a real quarter-circle input");
}

#[test]
fn kfm_holdfwd_via_slash() {
    let mut list = CommandList::new();
    let cmds = CmdFileCommands::parse(&kfm_cmd()).unwrap();
    cmds.build(&mut list).unwrap();
    for held in [true, true, true] {
        {
            let b = &mut list.buffer;
            feed(b, false, false, false, held, Btn::None);
        }
        list.step(false, false, false, false, 0);
    }
    assert!(list.get_state("holdfwd"), "/F must be buffered while F held");
}

#[test]
fn kfm_double_tap_dash() {
    let mut list = CommandList::new();
    let cmds = CmdFileCommands::parse(&kfm_cmd()).unwrap();
    cmds.build(&mut list).unwrap();
    // F, F (autogreater expands) — press, hold, release gap, press again
    let seq = [true, true, false, true, true];
    let mut fired = false;
    for held in seq {
        {
            let b = &mut list.buffer;
            feed(b, false, false, false, held, Btn::None);
        }
        list.step(false, false, false, false, 0);
        fired |= list.get_state("FF");
    }
    assert!(fired, "F,F dash command must fire (KFM's FF)");
}
