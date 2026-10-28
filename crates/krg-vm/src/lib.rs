//! krg-vm — the MUGEN CNS/CMD/ZSS virtual machine.
//!
//! Port origin: ikemen-src/src/{compiler,bytecode,input,char,state}.go —
//! the 48K-line core that makes characters *behave*: expression compilation
//! and evaluation, command detection, state files, state controllers, and
//! the per-frame character runtime (physics, hit detection, rounds).
//!
//! Layering (bottom → top):
//! * [`value`]  — BytecodeValue + arithmetic fold semantics
//! * [`lex`]    — the CNS tokenizer
//! * [`op`]     — postfix opcodes (typed form of Go's flat []byte)
//! * [`eval`]   — the stack-machine evaluator + `Ctx` runtime trait
//! * [`exp`]    — the expression compiler (precedence chain + triggers)
//! * [`cmd`]    — [Command] parsing + input buffer + matcher
//! * [`cns`]    — CNS state-file compiler ([Statedef]/[State])
//! * [`zss`]    — ZSS state-file compiler (Ikemen's own script language)
//! * [`sctrl`]  — state controller compile+run registry
//! * [`statedef`] — StateBytecode/StateBlock runtime structures
//! * [`runtime`]/[`fight`] — the Char runtime + fight system (phases, physics, hit detection, camera, rounds)

pub mod cmd;
pub mod cns;
pub mod eval;
pub mod exp;
pub mod lex;
pub mod op;
pub mod runtime;
pub mod sctrl;
pub mod statedef;
pub mod value;
pub mod zss;

pub use eval::{eval, Ctx};
pub use op::BytecodeExp;
pub use value::{v_bool, v_float, v_int, v_none, v_string, v_undef, Value, ValueType};
