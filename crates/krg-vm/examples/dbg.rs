use krg_vm::exp::{Compiler, CompileCtx};
use krg_vm::eval::{eval, Ctx};
use krg_vm::op::*;
use krg_vm::value::*;

struct CC { pool: Vec<String> }
impl CompileCtx for CC {
    fn add_string(&mut self, s: &str) -> usize { self.pool.push(s.into()); self.pool.len()-1 }
    fn command_exists(&self, _n: &str) -> bool { true }
    fn local_slot(&self, _n: &str) -> Option<u8> { None }
    fn func_idx(&mut self, n: &str) -> usize { self.add_string(n) }
}
struct MC { pool: Vec<String> }
impl Ctx for MC {
    fn pool(&self) -> &[String] { &self.pool }
    fn trig(&mut self, _t: Trig) -> Value { v_int(0) }
    fn command(&mut self, _n: &str) -> bool { false }
    fn gethitvar(&mut self, _k: GetHitVarKind) -> Value { v_int(0) }
    fn const_kind(&mut self, _k: ConstKind) -> Value { v_int(0) }
    fn const_scaled(&mut self, v: f32) -> f32 { v }
    fn animelemtime(&mut self, n: i32) -> i32 { n }
    fn animelemno(&mut self, t: f32) -> i32 { t as i32 }
    fn animexist(&mut self, _n: i32) -> bool { true }
    fn random(&mut self) -> i32 { 0 }
    fn var_get(&mut self, _n: i32) -> i32 { 0 }
    fn var_set(&mut self, _n: i32, v: i32) -> i32 { v }
    fn fvar_get(&mut self, _n: i32) -> f32 { 0.0 }
    fn fvar_set(&mut self, _n: i32, v: f32) -> f32 { v }
    fn sysvar_get(&mut self, _n: i32) -> i32 { 0 }
    fn sysvar_set(&mut self, _n: i32, v: i32) -> i32 { v }
    fn sysfvar_get(&mut self, _n: i32) -> f32 { 0.0 }
    fn sysfvar_set(&mut self, _n: i32, v: f32) -> f32 { v }
    fn map_get(&mut self, _n: &str) -> Value { v_int(0) }
    fn map_set(&mut self, _n: &str, _v: Value) {}
    fn redirect_p2(&mut self, _s: &[Op]) -> Value { v_undef() }
    fn redirect_id(&mut self, _i: i32, _s: &[Op]) -> Value { v_undef() }
    fn call_func(&mut self, _n: &str, _a: Vec<Value>) -> Value { v_int(0) }
    fn local_get(&mut self, _i: u8) -> Value { v_int(0) }
}

fn main() {
    for src in ["1 + 2", "2 * 3 + 4", "time > 4", "time = 5"] {
        let mut cc = CC { pool: vec![] };
        let mut c = Compiler::new(&mut cc);
        let mut input = src;
        match c.full_expression(&mut input, ValueType::Bool) {
            Ok(exp) => {
                let mut mc = MC { pool: cc.pool.clone() };
                let v = eval(&exp, &mut mc);
                println!("{:?} => {} (ops: {:?})", src, v.to_i(),
                    exp.iter().map(|o| format!("{:?}", o)).collect::<Vec<_>>());
            }
            Err(e) => println!("{:?} => ERR {}", src, e),
        }
    }
}
