//! Expression compiler + evaluator round-trip tests against a mock context.
//! These lock in the Go emission/evaluation semantics proven in research:
//! * `1 + time`  → [Push(1)][time][add]     (LHS const pushed at op time)
//! * `time + 1`  → [time][Push(1)][add]
//! * folding uses `value::` ops (Min-vtype float promotion)

use krg_vm::eval::{eval, Ctx};
use krg_vm::exp::{CompileCtx, Compiler};
use krg_vm::op::*;
use krg_vm::value::*;

struct MockCtx {
    pool: Vec<String>,
    // pretend runtime values
    time: i32,
    anim: i32,
    statetype: &'static str,
    velx: f32,
    vars: [i32; 8],
    cmd_x: bool,
}

impl MockCtx {
    fn new() -> Self {
        MockCtx {
            pool: vec!["x".into(), "S".into(), "A".into()],
            time: 5,
            anim: 200,
            statetype: "S",
            velx: 2.5,
            vars: [0; 8],
            cmd_x: true,
        }
    }
}

impl Ctx for MockCtx {
    fn pool(&self) -> &[String] {
        &self.pool
    }
    fn trig(&mut self, t: Trig) -> Value {
        match t {
            Trig::Time => v_int(self.time),
            Trig::Anim => v_int(self.anim),
            Trig::VelX => v_float(self.velx),
            Trig::StateType => v_string(1), // "S"
            Trig::PrevStateNo => v_int(0),
            _ => v_int(0),
        }
    }
    fn command(&mut self, name: &str) -> bool {
        name == "x"
    }
    fn gethitvar(&mut self, _k: GetHitVarKind) -> Value {
        v_int(0)
    }
    fn const_kind(&mut self, _k: ConstKind) -> Value {
        v_int(0)
    }
    fn const_scaled(&mut self, v: f32) -> f32 {
        v
    }
    fn animelemtime(&mut self, n: i32) -> i32 {
        n // mock: returns arg
    }
    fn animelemno(&mut self, t: f32) -> i32 {
        t as i32
    }
    fn animexist(&mut self, n: i32) -> bool {
        n < 300
    }
    fn random(&mut self) -> i32 {
        42
    }
    fn var_get(&mut self, n: i32) -> i32 {
        self.vars[n as usize % 8]
    }
    fn var_set(&mut self, n: i32, v: i32) -> i32 {
        self.vars[n as usize % 8] = v;
        v
    }
    fn fvar_get(&mut self, _n: i32) -> f32 {
        0.0
    }
    fn fvar_set(&mut self, _n: i32, v: f32) -> f32 {
        v
    }
    fn sysvar_get(&mut self, n: i32) -> i32 {
        self.vars[n as usize % 8]
    }
    fn sysvar_set(&mut self, n: i32, v: i32) -> i32 {
        self.vars[n as usize % 8] = v;
        v
    }
    fn sysfvar_get(&mut self, _n: i32) -> f32 {
        0.0
    }
    fn sysfvar_set(&mut self, _n: i32, v: f32) -> f32 {
        v
    }
    fn map_get(&mut self, _name: &str) -> Value {
        v_int(0)
    }
    fn map_set(&mut self, _name: &str, _v: Value) {}
    fn redirect_p2(&mut self, _sub: &[Op]) -> Value {
        v_undef()
    }
    fn redirect_id(&mut self, _id: i32, _sub: &[Op]) -> Value {
        v_undef()
    }
    fn call_func(&mut self, _name: &str, _args: Vec<Value>) -> Value {
        v_int(0)
    }
    fn local_get(&mut self, _i: u8) -> Value {
        v_int(0)
    }
}

struct MockCompile {
    pool: Vec<String>,
}

impl CompileCtx for MockCompile {
    fn add_string(&mut self, s: &str) -> usize {
        if let Some(i) = self.pool.iter().position(|p| p == s) {
            i
        } else {
            self.pool.push(s.to_string());
            self.pool.len() - 1
        }
    }
    fn command_exists(&self, name: &str) -> bool {
        name == "x" || name == "holddown"
    }
    fn local_slot(&self, _name: &str) -> Option<u8> {
        None
    }
    fn func_idx(&mut self, name: &str) -> usize {
        self.add_string(name)
    }
}

fn compile_with_pool(src: &str, pool: &mut Vec<String>) -> BytecodeExp {
    let mut cc = MockCompile { pool: std::mem::take(pool) };
    let mut c = Compiler::new(&mut cc);
    let mut input = src;
    let r = c.full_expression(&mut input, ValueType::Bool);
    *pool = cc.pool;
    r.unwrap_or_else(|e| panic!("compile failed for {:?}: {}", src, e))
}

fn compile(src: &str) -> BytecodeExp {
    let mut pool = vec!["x".to_string(), "S".to_string(), "A".to_string()];
    compile_with_pool(src, &mut pool)
}

fn eval_with(src: &str) -> Value {
    // pool seeds mirror MockCtx's, so compile-time indices line up
    let mut pool = vec!["x".to_string(), "S".to_string(), "A".to_string()];
    let exp = compile_with_pool(src, &mut pool);
    let mut ctx = MockCtx::new();
    ctx.pool = pool;
    eval(&exp, &mut ctx)
}

#[test]
fn constants_fold() {
    // compile with VT_Int so folded constants keep their numeric type
    // (VT_Bool — the trigger type — coerces like Go typedExp does)
    let mut cc = MockCompile { pool: vec![] };
    let mut c = Compiler::new(&mut cc);
    let mut input = "1 + 2";
    let exp = c.full_expression(&mut input, ValueType::Int).unwrap();
    assert_eq!(exp.len(), 1);
    assert!(matches!(exp[0], Op::Push(_)));
    assert_eq!(eval_with("1 + 2").to_b(), true); // trigger view: truthy
    // numeric checks via VT_Int compiles
    let num = |src: &str| {
        let mut cc = MockCompile { pool: vec![] };
        let mut c = Compiler::new(&mut cc);
        let mut input = src;
        let exp = c.full_expression(&mut input, ValueType::Int).unwrap();
        let mut ctx = MockCtx::new();
        eval(&exp, &mut ctx).to_i()
    };
    assert_eq!(num("1 + 2"), 3);
    assert_eq!(num("2 * 3 + 4"), 10);
    assert_eq!(num("2 + 3 * 4"), 14); // precedence
    assert_eq!(num("7 / 2"), 3);
    assert_eq!(num("7 % 3"), 1);
    assert_eq!(num("2 ** 10"), 1024);
}

#[test]
fn runtime_operands_order() {
    // time + 1 → runtime on LHS: [time][Push(1)][add]
    let exp = compile("time + 1");
    assert_eq!(exp.len(), 3);
    assert!(matches!(exp[0], Op::Trig(Trig::Time)));
    assert!(matches!(exp[1], Op::Push(_)));
    assert!(matches!(exp[2], Op::Add));
    assert_eq!(eval_with("time + 1").to_i(), 6);

    // 1 + time → [Push(1)][time][add]
    let exp = compile("1 + time");
    assert_eq!(exp.len(), 3);
    assert!(matches!(exp[0], Op::Push(_)));
    assert!(matches!(exp[1], Op::Trig(Trig::Time)));
    assert_eq!(eval_with("1 + time").to_i(), 6);

    // float promotion at runtime
    assert_eq!(eval_with("vel x + 1").to_f(), 3.5);
    assert_eq!(eval_with("vel x + 1").vtype, ValueType::Float);
}

#[test]
fn comparisons() {
    assert_eq!(eval_with("time = 5").to_b(), true);
    assert_eq!(eval_with("time != 5").to_b(), false);
    assert_eq!(eval_with("time > 4").to_b(), true);
    assert_eq!(eval_with("time >= 5").to_b(), true);
    assert_eq!(eval_with("time < 5").to_b(), false);
    assert_eq!(eval_with("time = [1, 5]").to_b(), true);
    assert_eq!(eval_with("time = [5, 9]").to_b(), true); // inclusive
    assert_eq!(eval_with("time = (5, 9]").to_b(), false); // exclusive min
    assert_eq!(eval_with("time != [1, 4]").to_b(), true);
    assert_eq!(eval_with("time = 5 && anim = 200").to_b(), true);
    assert_eq!(eval_with("time = 5 || anim = 0").to_b(), true);
    assert_eq!(eval_with("!(time = 5)").to_b(), false);
    // bitwise results are Int under VT_Bool coercion (true) — check via VT_Int
    let num = |src: &str| {
        let mut pool = vec!["x".to_string(), "S".to_string(), "A".to_string()];
        let mut cc = MockCompile { pool: std::mem::take(&mut pool) };
        let mut c = Compiler::new(&mut cc);
        let mut input = src;
        let exp = c.full_expression(&mut input, ValueType::Int).unwrap();
        let mut ctx = MockCtx::new();
        ctx.pool = cc.pool;
        eval(&exp, &mut ctx).to_i()
    };
    assert_eq!(num("3 & 1"), 1);
    assert_eq!(num("3 | 4"), 7);
    assert_eq!(num("3 ^ 1"), 2);
}

#[test]
fn string_and_flag_triggers() {
    assert_eq!(eval_with("statetype = S").to_b(), true);
    assert_eq!(eval_with("statetype != A").to_b(), true);
    assert_eq!(eval_with("command = \"x\"").to_b(), true);
    assert_eq!(eval_with("command != \"x\"").to_b(), false);
    // unknown command name = compile error in Go (compiler.go:2209)
}

#[test]
fn vars_and_assigns() {
    // var(3) := 7 — then reads
    let exp = compile("var(3) := 7");
    let mut ctx = MockCtx::new();
    let r = eval(&exp, &mut ctx);
    assert_eq!(r.to_i(), 7);
    assert_eq!(ctx.vars[3], 7);
    // assignment result value = new var value
    let exp = compile("var(0) = 0");
    assert_eq!(eval(&exp, &mut MockCtx::new()).to_b(), true);
}

#[test]
fn ifelse_lazy_branch() {
    assert_eq!(eval_with("ifelse(time = 5, 10, 20)").to_i(), 10);
    assert_eq!(eval_with("ifelse(time = 4, 10, 20)").to_i(), 20);
    assert_eq!(eval_with("cond(time > 0, 1, 2)").to_i(), 1);
}

#[test]
fn animelem_old_style() {
    // animelem = 3 with mock animelemtime(n)=n: elemtime(3)=3 ≠ 0 → false
    assert_eq!(eval_with("animelem = 3").to_b(), false);
}

#[test]
fn redirects() {
    // p2 redirect with mock → Undefined; eq(undef,0)=true (undef reads 0)
    assert_eq!(eval_with("p2, stateno = 0").to_b(), true);
    assert_eq!(eval_with("p2, stateno != 0").to_b(), false);
    assert_eq!(eval_with("p2, stateno = 999").to_b(), false);
}

#[test]
fn go_quirks_preserved() {
    // division by zero → Undefined (survives only under VT_Undefined;
    // VT_Bool coerces like Go's typedExp — bool(false))
    let mut pool = vec![];
    let mut cc = MockCompile { pool: std::mem::take(&mut pool) };
    let mut c = Compiler::new(&mut cc);
    let mut input = "1 / 0";
    let exp = c.full_expression(&mut input, ValueType::Undefined).unwrap();
    pool = cc.pool;
    let mut ctx = MockCtx::new();
    ctx.pool = pool;
    assert_eq!(eval(&exp, &mut ctx).is_undefined(), true);
    assert_eq!(eval_with("1 / 0").to_b(), false);
    // int overflow wraps silently (via VT_Int compile)
    let mut cc = MockCompile { pool: vec![] };
    let mut c = Compiler::new(&mut cc);
    let mut input = "2147483647 + 1";
    let exp = c.full_expression(&mut input, ValueType::Int).unwrap();
    let mut ctx = MockCtx::new();
    assert_eq!(eval(&exp, &mut ctx).to_i(), i32::MIN);
    // scientific notation is rejected (Go quirk)
    let mut cc = MockCompile { pool: vec![] };
    let mut c = Compiler::new(&mut cc);
    let mut input = "1e3";
    assert!(c.full_expression(&mut input, ValueType::Int).is_err());
}
