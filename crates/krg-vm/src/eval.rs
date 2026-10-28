//! Port origin: ikemen-src/src/bytecode.go `BytecodeExp.run` (bytecode.go:2237)
//! and the trigger read cases (run / run_ex / …).
//!
//! Rust factoring: Go splices jump opcodes into a flat byte stream for
//! short-circuit logic. Here control flow lives in the *callers* that own
//! the structure (trigger sets, `if` blocks — see statedef.rs); expressions
//! stay pure postfix stack programs. `ifelse`/`cond` compile to `Op::IfElse`
//! with boxed sub-expressions (only the chosen branch evaluates — same
//! observable behavior as Go's jz/jmp chains).

use crate::op::*;
use crate::value::*;

/// Runtime context every expression evaluates against. One instance per
/// evaluating char (or redirect target / function frame). The fight runtime
/// (fight.rs) implements this; krg-vm stays engine-agnostic.
pub trait Ctx {
    /// String pool of the *state owner*
    /// (Go: `sys.stringPool[sys.workingState.playerNo]`)
    fn pool(&self) -> &[String];

    // ── trigger reads ────────────────────────────────────────────────────
    fn trig(&mut self, t: Trig) -> Value;
    fn command(&mut self, name: &str) -> bool;
    fn gethitvar(&mut self, k: GetHitVarKind) -> Value;
    fn const_kind(&mut self, k: ConstKind) -> Value;
    /// const240p/480p/720p — Go OC_ex_const240p: value scaled by
    /// localcoord ratio (char_height / base)
    fn const_scaled(&mut self, scaled_value: f32) -> f32;
    /// `animelemtime(n)` — 0-based; <0 before, >0 after (Go OC_animelemtime)
    fn animelemtime(&mut self, n: i32) -> i32;
    /// `animelemno(t)` — elem at time t (Go OC_animelemno)
    fn animelemno(&mut self, t: f32) -> i32;
    /// `animexist(n)` / `selfanimexist(n)`
    fn animexist(&mut self, n: i32) -> bool;
    /// `random` — Go Rand(0, 999) inclusive
    fn random(&mut self) -> i32;

    // ── variables ────────────────────────────────────────────────────────
    fn var_get(&mut self, n: i32) -> i32;
    fn var_set(&mut self, n: i32, v: i32) -> i32;
    fn fvar_get(&mut self, n: i32) -> f32;
    fn fvar_set(&mut self, n: i32, v: f32) -> f32;
    fn sysvar_get(&mut self, n: i32) -> i32;
    fn sysvar_set(&mut self, n: i32, v: i32) -> i32;
    fn sysfvar_get(&mut self, n: i32) -> f32;
    fn sysfvar_set(&mut self, n: i32, v: f32) -> f32;

    // ── maps (Ikemen feature, used by functions.zss) ─────────────────────
    fn map_get(&mut self, name: &str) -> Value;
    fn map_set(&mut self, name: &str, v: Value);

    // ── redirects / functions ────────────────────────────────────────────
    /// Evaluate sub-program with p2 as context; Undefined if no p2
    fn redirect_p2(&mut self, sub: &[Op]) -> Value;
    /// Evaluate sub-program with char by id; Undefined if not found
    fn redirect_id(&mut self, id: i32, sub: &[Op]) -> Value;
    /// ZSS function call: args evaluated in caller context, body runs with
    /// its own local frame. Returns first return value (Go OC_run).
    fn call_func(&mut self, name: &str, args: Vec<Value>) -> Value;

    // ── ZSS locals ────────────────────────────────────────────────────────
    fn local_get(&mut self, i: u8) -> Value;
}

/// Evaluate a compiled expression; returns its value.
pub fn eval(exp: &[Op], ctx: &mut dyn Ctx) -> Value {
    let mut stack: Vec<Value> = Vec::with_capacity(8);
    eval_into(exp, ctx, &mut stack);
    stack.pop().unwrap_or_else(v_none)
}

/// Same, reusing a stack buffer (the hot path used by sctrl param reads).
pub fn eval_into(exp: &[Op], ctx: &mut dyn Ctx, stack: &mut Vec<Value>) {
    stack.clear();
    let mut ip = 0usize;
    while ip < exp.len() {
        match &exp[ip] {
            Op::Push(v) => stack.push(*v),

            Op::Pop => {
                stack.pop();
            }
            Op::Add => bin(stack, add),
            Op::Sub => bin(stack, sub),
            Op::Mul => bin(stack, mul),
            Op::Div => bin(stack, div),
            Op::Mod => bin(stack, modulo),
            Op::Pow => bin(stack, pow),
            Op::Neg => {
                if let Some(t) = stack.last_mut() {
                    *t = neg(t);
                }
            }
            Op::Eq => {
                if stack.len() >= 2 {
                    let v2 = stack.pop().unwrap();
                    let pool = ctx_pool(ctx);
                    if let Some(t) = stack.last_mut() {
                        *t = eq(t, &v2, pool, pool);
                    }
                }
            }
            Op::Ne => {
                if stack.len() >= 2 {
                    let v2 = stack.pop().unwrap();
                    let pool = ctx_pool(ctx);
                    if let Some(t) = stack.last_mut() {
                        *t = ne(t, &v2, pool, pool);
                    }
                }
            }
            Op::Gt => bin(stack, gt),
            Op::Ge => bin(stack, ge),
            Op::Lt => bin(stack, lt),
            Op::Le => bin(stack, le),
            Op::Bland => bin(stack, bool_and),
            Op::Blor => bin(stack, bool_or),
            Op::Blxor => bin(stack, bool_xor),
            Op::Blnot => {
                if let Some(t) = stack.last_mut() {
                    *t = bool_not(t);
                }
            }
            Op::Bwand => bin(stack, bw_and),
            Op::Bwor => bin(stack, bw_or),
            Op::Bwxor => bin(stack, bw_xor),
            Op::Range { incl_min, incl_max } => {
                if stack.len() >= 3 {
                    let max = stack.pop().unwrap();
                    let min = stack.pop().unwrap();
                    if let Some(t) = stack.last_mut() {
                        *t = range_check(t, &min, &max, *incl_min, *incl_max);
                    }
                }
            }

            // jump forms (kept for ZSS trigger-chain compat; v0.9 state
            // compilers do not emit them)
            Op::Jmp(t) => {
                ip = *t;
                continue;
            }
            Op::Jz(t) => {
                if !stack.last().map(|v| v.to_b()).unwrap_or(false) {
                    ip = *t;
                    continue;
                }
            }
            Op::Jnz(t) => {
                if stack.last().map(|v| v.to_b()).unwrap_or(false) {
                    ip = *t;
                    continue;
                }
            }
            Op::Jud(t) => {
                if stack.last().map(|v| v.is_undefined()).unwrap_or(false) {
                    ip = *t;
                    continue;
                }
            }

            // ── variables ─────────────────────────────────────────────────
            Op::VarGet => {
                let n = stack.pop().map(|v| v.to_i()).unwrap_or(0);
                stack.push(v_int(ctx.var_get(n)));
            }
            Op::SysvarGet => {
                let n = stack.pop().map(|v| v.to_i()).unwrap_or(0);
                stack.push(v_int(ctx.sysvar_get(n)));
            }
            Op::FvarGet => {
                let n = stack.pop().map(|v| v.to_i()).unwrap_or(0);
                stack.push(v_float(ctx.fvar_get(n)));
            }
            Op::SysfvarGet => {
                let n = stack.pop().map(|v| v.to_i()).unwrap_or(0);
                stack.push(v_float(ctx.sysfvar_get(n)));
            }
            Op::AssignVar => assign(stack, |n, v, ctx| ctx.var_set(n, v), ctx),
            Op::AssignSysvar => assign(stack, |n, v, ctx| ctx.sysvar_set(n, v), ctx),
            Op::AssignFvar => assign_f(stack, |n, v, ctx| ctx.fvar_set(n, v), ctx),
            Op::AssignSysfvar => assign_f(stack, |n, v, ctx| ctx.sysfvar_set(n, v), ctx),
            Op::LocalVar(i) => stack.push(ctx.local_get(*i)),
            Op::MapGet => {
                if let Some(n) = stack.pop() {
                    let name = n.to_s(ctx_pool(ctx)).to_owned();
                    stack.push(ctx.map_get(&name));
                }
            }
            Op::MapSet => {
                if stack.len() >= 2 {
                    let v = stack.pop().unwrap();
                    let n = stack.pop().unwrap();
                    let name = n.to_s(ctx_pool(ctx)).to_owned();
                    ctx.map_set(&name, v);
                    stack.push(v);
                }
            }

            // ── redirects / calls ────────────────────────────────────────
            Op::P2(sub) => {
                let r = ctx.redirect_p2(sub);
                stack.push(r);
            }
            Op::RunCaller(sub) => {
                let r = eval(sub, ctx);
                stack.push(r);
            }
            Op::RunRedirId(sub) => {
                let id = stack.pop().map(|v| v.to_i()).unwrap_or(-1);
                let r = ctx.redirect_id(id, sub);
                stack.push(r);
            }
            Op::Call { name_idx, args } => {
                let name = ctx
                    .pool()
                    .get(*name_idx)
                    .cloned()
                    .unwrap_or_default();
                let mut vals = Vec::with_capacity(args.len());
                for a in args.iter() {
                    vals.push(eval(a, ctx));
                }
                let r = ctx.call_func(&name, vals);
                stack.push(r);
            }

            // ── trigger reads ────────────────────────────────────────────
            Op::Trig(t) => stack.push(ctx.trig(*t)),
            Op::AnimElemTime => {
                let n = stack.pop().map(|v| v.to_i()).unwrap_or(0);
                stack.push(v_int(ctx.animelemtime(n)));
            }
            Op::AnimElemNo => {
                let t = stack.pop().map(|v| v.to_f()).unwrap_or(0.0);
                stack.push(v_int(ctx.animelemno(t)));
            }
            Op::AnimExist => {
                let n = stack.pop().map(|v| v.to_i()).unwrap_or(0);
                stack.push(v_bool(ctx.animexist(n)));
            }
            Op::Command => {
                if let Some(n) = stack.pop() {
                    let name = n.to_s(ctx_pool(ctx)).to_owned();
                    stack.push(v_bool(ctx.command(&name)));
                }
            }
            Op::GetHitVar(k) => stack.push(ctx.gethitvar(*k)),
            Op::Const(k) => stack.push(ctx.const_kind(*k)),
            Op::Const240p => scale_const(stack, ctx, 240.0),
            Op::Const480p => scale_const(stack, ctx, 480.0),
            Op::Const720p => scale_const(stack, ctx, 720.0),
            Op::Floor => {
                if let Some(t) = stack.last_mut() {
                    *t = v_int(t.to_i());
                }
            }
            Op::Abs => {
                if let Some(t) = stack.last_mut() {
                    if t.vtype == ValueType::Float {
                        t.set_f(t.to_f().abs());
                    } else {
                        *t = v_int(t.to_i().wrapping_abs());
                    }
                }
            }
            Op::IfElse { c, a, b } => {
                let r = eval_ifelse(c, a, b, ctx);
                stack.push(r);
            }
        }
        ip += 1;
    }
}

/// `ifelse(c, a, b)` structural evaluation (Go jz/jmp chain equivalent).
pub fn eval_ifelse(c: &[Op], a: &[Op], b: &[Op], ctx: &mut dyn Ctx) -> Value {
    if eval(c, ctx).to_b() {
        eval(a, ctx)
    } else {
        eval(b, ctx)
    }
}

fn ctx_pool<'a>(ctx: &'a dyn Ctx) -> &'a [String] {
    ctx.pool()
}

fn bin(stack: &mut Vec<Value>, f: impl Fn(&Value, &Value) -> Value) {
    if stack.len() >= 2 {
        let v2 = stack.pop().unwrap();
        if let Some(t) = stack.last_mut() {
            *t = f(t, &v2);
        }
    }
}

fn assign(
    stack: &mut Vec<Value>,
    f: impl FnOnce(i32, i32, &mut dyn Ctx) -> i32,
    ctx: &mut dyn Ctx,
) {
    if stack.len() >= 2 {
        let val = stack.pop().unwrap().to_i();
        if let Some(t) = stack.last_mut() {
            let n = t.to_i();
            *t = v_int(f(n, val, ctx));
        }
    }
}

fn assign_f(
    stack: &mut Vec<Value>,
    f: impl FnOnce(i32, f32, &mut dyn Ctx) -> f32,
    ctx: &mut dyn Ctx,
) {
    if stack.len() >= 2 {
        let val = stack.pop().unwrap().to_f();
        if let Some(t) = stack.last_mut() {
            let n = t.to_i();
            t.set_f(f(n, val, ctx));
        }
    }
}

fn scale_const(stack: &mut Vec<Value>, ctx: &mut dyn Ctx, base: f32) {
    if let Some(t) = stack.last_mut() {
        let v = t.to_f();
        t.set_f(ctx.const_scaled(v * base));
    }
}
