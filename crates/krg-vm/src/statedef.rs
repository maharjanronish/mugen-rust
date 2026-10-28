//! Port origin: ikemen-src/src/bytecode.go — `StateBytecode` (17048),
//! `StateBlock` (5083), `StateController` (4981) and the block-runner
//! `StateBlock.run` (5107-5264); trigger codegen semantics from
//! compiler.go:7443-7496.
//!
//! Rust factoring: Go compiles trigger rows into flat jz/jnz bytecode
//! (rows OR, same-number AND, triggerall AND — all short-circuiting).
//! We keep the same *observable* semantics structurally:
//! * triggerall: AND, short-circuit at first false
//! * triggerN rows: OR, short-circuit at first true row
//! * same-number triggerN expressions: AND, short-circuit at first false
//! * constant-false rows are folded away at compile time (trexist=-1);
//!   constant-true rows become empty expression lists (always pass)
//!
//! Persistent counters: every CNS sctrl gets a slot (`persistentIndex`);
//! the counter is decremented while >0 and reset to `persistent` after a
//! successful run (run-once = i32::MAX per Go's math.MaxInt32).

use crate::eval::{eval, Ctx};
use crate::op::BytecodeExp;
use crate::sctrl::{self, Sctrl, SctrlKind};
use crate::value::{v_none, Value};

/// StateType (bytecode.go StateType: S C A L U N).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateTypeT {
    #[default]
    S,
    C,
    A,
    L,
    U, // undefined
}

impl StateTypeT {
    pub fn from_str(s: &str) -> Option<StateTypeT> {
        Some(match s.trim().to_uppercase().as_str() {
            "S" => StateTypeT::S,
            "C" => StateTypeT::C,
            "A" => StateTypeT::A,
            "L" => StateTypeT::L,
            "U" => StateTypeT::U,
            _ => return None,
        })
    }
    /// MUGEN string id used by `statetype = S` comparisons.
    pub fn as_char(self) -> &'static str {
        match self {
            StateTypeT::S => "S",
            StateTypeT::C => "C",
            StateTypeT::A => "A",
            StateTypeT::L => "L",
            StateTypeT::U => "U",
        }
    }
}

/// MoveType (I A H U).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MoveTypeT {
    #[default]
    I,
    A,
    H,
    U,
}

impl MoveTypeT {
    pub fn from_str(s: &str) -> Option<MoveTypeT> {
        Some(match s.trim().to_uppercase().as_str() {
            "I" => MoveTypeT::I,
            "A" => MoveTypeT::A,
            "H" => MoveTypeT::H,
            "U" => MoveTypeT::U,
            _ => return None,
        })
    }
    pub fn as_char(self) -> &'static str {
        match self {
            MoveTypeT::I => "I",
            MoveTypeT::A => "A",
            MoveTypeT::H => "H",
            MoveTypeT::U => "U",
        }
    }
}

/// Compiled trigger set for one state block.
/// Rows are 1-indexed (trigger1, trigger2, …) stored 0-based in order.
#[derive(Debug, Clone, Default)]
pub struct Triggers {
    pub triggerall: Vec<BytecodeExp>,
    /// None = constant-false row (trexist=-1, never fires);
    /// empty vec = constant-true row (always passes).
    pub rows: Vec<Option<Vec<BytecodeExp>>>,
    /// true if any row can be true (drives sctrl discard rule)
    pub any_possible: bool,
}

impl Triggers {
    /// Is the whole block unconditional? (no triggerall, no rows)
    pub fn is_unconditional(&self) -> bool {
        self.triggerall.is_empty() && self.rows.is_empty()
    }

    /// Structural equivalent of the Go jz/jnz trigger bytecode.
    pub fn eval(&self, ctx: &mut dyn Ctx) -> bool {
        for e in &self.triggerall {
            if !eval(e, ctx).to_b() {
                return false;
            }
        }
        if self.rows.is_empty() {
            return true; // only triggeralls — all passed
        }
        for row in &self.rows {
            match row {
                None => continue, // constant-false row
                Some(exprs) => {
                    let mut all = true;
                    for e in exprs {
                        if !eval(e, ctx).to_b() {
                            all = false;
                            break;
                        }
                    }
                    if all {
                        return true; // short-circuit OR
                    }
                }
            }
        }
        false
    }
}

/// One node inside a state block (CNS: sctrl; ZSS: also sub-blocks,
/// expression statements, calls, loop control).
#[derive(Debug, Clone)]
pub enum Node {
    Sctrl(Sctrl),
    Block(Box<StateBlock>),
    /// ZSS `expr := value;` assignment statement (result discarded)
    ExprStmt(BytecodeExp),
    /// ZSS `call name(args);`
    Call { name_idx: usize, args: Vec<BytecodeExp>, ret: Vec<u8> },
    /// ZSS `break;` / `continue;`
    Break,
    Continue,
    /// ZSS loop head (`for`/`while`)
    Loop {
        /// evaluated before each iteration (bool)
        cond: Option<BytecodeExp>,
        /// `for i = begin; end; incr` fields (for-style)
        init: Option<(u8, Box<BytecodeExp>)>, // local slot := init
        incr: Option<BytecodeExp>,
        body: Vec<Node>,
    },
    /// ZSS `let` binding — evaluates exp, writes to local slot
    Let { slot: u8, exp: BytecodeExp },
}

/// bytecode.go:5083 `StateBlock`
#[derive(Debug, Clone)]
pub struct StateBlock {
    pub persistent: i32,
    pub persistent_index: i32, // -1 = none; CNS: ordinal within state
    pub ignorehitpause: bool,
    pub ctrls_ignorehitpause: bool,
    pub trigger: Option<Triggers>, // None = unconditional
    pub else_block: Option<Box<StateBlock>>,
    pub ctrls: Vec<Node>,
}

impl Default for StateBlock {
    /// newStateBlock (bytecode.go:5103)
    fn default() -> Self {
        StateBlock {
            persistent: 1,
            persistent_index: -1,
            ignorehitpause: false,
            ctrls_ignorehitpause: false,
            trigger: None,
            else_block: None,
            ctrls: Vec::new(),
        }
    }
}

/// bytecode.go:17048 `StateBytecode`
#[derive(Debug, Clone, Default)]
pub struct StateBytecode {
    pub state_type: StateTypeT,          // default S
    pub state_type_exp: Option<BytecodeExp>,
    pub move_type: MoveTypeT,            // default I
    pub move_type_exp: Option<BytecodeExp>,
    pub physics: StateTypeT,            // default N (no physics)
    pub physics_exp: Option<BytecodeExp>,
    pub player_no: usize,
    /// Statedef itself runs as a controller each state entry
    pub state_def: Option<Sctrl>,
    pub block: StateBlock,
    pub ctrlsps: Vec<i32>,
    pub num_vars: i32,
}

impl StateBytecode {
    /// newStateBytecode (bytecode.go:17063)
    pub fn new(player_no: usize) -> Self {
        StateBytecode {
            state_type: StateTypeT::S,
            move_type: MoveTypeT::I,
            physics: StateTypeT::U, // ST_N in Go = "none" is our U? No:
            // Go ST_N is a distinct "no physics" variant; we model the
            // default Statedef physics=ST_N as U (unset) and map N → U.
            player_no,
            ..Default::default()
        }
    }
}

/// Runtime view the state runner needs (implemented by runtime::Char).
pub trait StateHost: Ctx {
    fn hit_paused(&self) -> bool;
    /// Persistent countdown: true = skip this block this frame
    /// (counter decremented while >0; run-once counters never reset).
    fn persistent_skip(&mut self, persistent_index: i32) -> bool;
    /// Reset the counter after a successful run.
    fn persistent_reset(&mut self, persistent_index: i32, value: i32);
    /// Ensure counter storage for `n` slots (grows, zero-filled).
    fn persistent_ensure(&mut self, n: usize);
    /// The state-owner number (states run from that player's compiled set).
    fn state_owner(&self) -> usize;
    fn on_state_change(&mut self) {}
    /// Allocate/reset ZSS locals frame (numVars)
    fn frame_reset(&mut self, num_vars: i32);
    fn set_local(&mut self, slot: u8, v: Value);
    fn local_slot(&mut self, slot: u8) -> Value;
    /// Run a compiled sctrl against this char. Returns true if it
    /// changeStated (aborts the rest of the block).
    fn run_sctrl(&mut self, sctrl: &Sctrl, persistent_active: bool) -> bool;
    /// ZSS function call bridge
    fn zss_call(&mut self, name_idx: usize, args: Vec<Value>, rets: &mut Vec<Value>);
    fn break_signal(&mut self) -> bool;
    fn continue_signal(&mut self) -> bool;
}

/// StateBlock.run (bytecode.go:5107) — returns true if a controller
/// changeStated (caller aborts remaining controllers).
pub fn run_block(block: &StateBlock, host: &mut dyn StateHost, in_loop: bool) -> bool {
    // Hitpause gating (bytecode.go:5109+)
    if host.hit_paused() && !block.ignorehitpause {
        return false;
    }

    // Persistent countdown (bytecode.go:5136-5143)
    let pi = block.persistent_index;
    if pi >= 0 && host.persistent_skip(pi) {
        return false;
    }

    // Trigger
    let take = match &block.trigger {
        None => true,
        Some(t) => t.eval(host),
    };

    if take {
        for node in &block.ctrls {
            if run_node(node, host, in_loop) {
                return true; // changeState aborts
            }
        }
        // Successful completion → reset persistent
        if pi >= 0 {
            host.persistent_reset(pi, block.persistent);
        }
        false
    } else if let Some(eb) = &block.else_block {
        run_block(eb, host, in_loop)
    } else {
        false
    }
}

fn run_node(node: &Node, host: &mut dyn StateHost, in_loop: bool) -> bool {
    match node {
        Node::Sctrl(sc) => {
            let ihp_ok = !host.hit_paused() || sc.ignorehitpause;
            if ihp_ok {
                host.run_sctrl(sc, true)
            } else {
                false
            }
        }
        Node::Block(b) => run_block(b, host, in_loop),
        Node::ExprStmt(exp) => {
            let _ = eval(exp, host);
            false
        }
        Node::Call { name_idx, args, ret } => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval(a, host));
            }
            let mut out = vec![v_none(); ret.len()];
            host.zss_call(*name_idx, vals, &mut out);
            for (i, slot) in ret.iter().enumerate() {
                if i < out.len() {
                    host.set_local(*slot, out[i]);
                }
            }
            false
        }
        Node::Let { slot, exp } => {
            let v = eval(exp, host);
            host.set_local(*slot, v);
            false
        }
        Node::Break => {
            if in_loop {
                // simplified: signaled through hit-pause-free flag in host
                false // handled by Loop via return value below
            } else {
                false
            }
        }
        Node::Continue => false,
        Node::Loop { cond, init, incr, body } => {
            if let Some((slot, exp)) = init {
                let v = eval(exp, host);
                host.set_local(*slot, v);
            }
            let mut iterations = 0usize;
            loop {
                if let Some(c) = cond {
                    if !eval(c, host).to_b() {
                        break;
                    }
                }
                let mut aborted = false;
                for n in body {
                    if run_node(n, host, true) {
                        aborted = true;
                        break;
                    }
                    if matches!(n, Node::Break) {
                        aborted = true;
                        break;
                    }
                }
                if aborted {
                    break;
                }
                if let Some(i) = incr {
                    let _ = eval(i, host);
                }
                // Go MaxLoop guard (bytecode.go:5234)
                iterations += 1;
                if iterations > 250_000 {
                    break;
                }
            }
            false
        }
    }
}

/// Run a whole StateBytecode for one frame (StateBytecode.run 17130).
/// `is_current` — this is the char's active state: persistent counters
/// come from the char (reset on entry); otherwise shared sb.ctrlsps.
pub fn run_state_bytecode(
    sb: &StateBytecode,
    host: &mut dyn StateHost,
    _is_current: bool,
) -> bool {
    // Persistent counters live on the host (char-local table; Go shares
    // sb.ctrlsps across chars for negative states — a known deviation,
    // revisit when helper sync matters).
    host.persistent_ensure(sb.ctrlsps.len());
    if let Some(def) = &sb.state_def {
        // Statedef params re-apply on every state entry
        host.run_sctrl(def, true);
    }
    run_block(&sb.block, host, false)
}

/// Kind check used by the compiler diagnostics
pub fn sctrl_kind_name(kind: SctrlKind) -> &'static str {
    sctrl::kind_name(kind)
}
