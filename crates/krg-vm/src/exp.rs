//! Port origin: ikemen-src/src/compiler.go — the expression compiler:
//! `typedExp` (6127) / `argExpression` (6153) / `fullExpression` (6172),
//! the precedence chain `expBoolOr`→…→`expPostNot` (5673-6126),
//! `expOneOpSub` (5988) and `expValue` (1314).
//!
//! Emission model (proven against Go): runtime code (trigger reads) is
//! emitted *during* the LHS parse; constants ride in the returned `Value`
//! and are pushed at operator time:
//!   `1 + time`  → [Push(1)][time][add]
//!   `time + 1`  → [time][Push(1)][add]
//! (expOneOpSub: appendValue(bv); append(be); appendValue(bv2); append(opc))

use crate::lex::*;
use crate::op::*;
use crate::value::*;

/// Compile-time context: string pool + known commands + ZSS locals + funcs.
pub trait CompileCtx {
    fn add_string(&mut self, s: &str) -> usize;
    fn command_exists(&self, name: &str) -> bool;
    fn local_slot(&self, name: &str) -> Option<u8>;
    fn func_idx(&mut self, name: &str) -> usize;
}

type R<T> = Result<T, String>;

/// Ports CharCompiler's expression state (compiler.go:17).
pub struct Compiler<'a> {
    pub ctx: &'a mut dyn CompileCtx,
    token: String,
    previous_operator: String,
    reverse_order: bool,
    norange: bool,
    /// c.block != nil (always true for CNS/ZSS state compilation here)
    in_block: bool,
}

fn e<T>(msg: impl std::fmt::Display) -> R<T> {
    Err(msg.to_string())
}

impl<'a> Compiler<'a> {
    pub fn new(ctx: &'a mut dyn CompileCtx) -> Self {
        Compiler {
            ctx,
            token: String::new(),
            previous_operator: String::new(),
            reverse_order: false,
            norange: false,
            in_block: true,
        }
    }

    fn tok(&mut self, input: &mut &str) {
        self.token = next_token_lower(input);
    }

    // ── typed entry points ────────────────────────────────────────────────

    /// typedExp (6127): parse full boolean expression; returns (code, folded)
    pub fn typed_exp(&mut self, input: &mut &str, vt: ValueType) -> R<BytecodeExp> {
        self.tok(input);
        let mut out: BytecodeExp = Vec::new();
        let bv = self.exp_bool_or(&mut out, input)?;
        if !bv.is_none() {
            let mut b = bv;
            match vt {
                ValueType::Float => b.set_f(b.to_f()),
                ValueType::Int => b.set_i(b.to_i()),
                ValueType::Bool => b.set_b(b.to_b()),
                _ => {}
            }
            out.push(Op::Push(b));
        }
        Ok(out)
    }

    /// argExpression (6153): one function argument
    pub fn arg_expression(&mut self, input: &mut &str, vt: ValueType) -> R<BytecodeExp> {
        let be = self.typed_exp(input, vt)?;
        if !self.token.is_empty() {
            if self.token != "," {
                return e(format!("Invalid data: {}", self.token));
            }
            // peek: a following empty token means end of input — consume
            let old = *input;
            if next_token_lower(input).is_empty() {
                self.token.clear();
            } else {
                *input = old;
            }
        }
        Ok(be)
    }

    /// fullExpression (6172)
    pub fn full_expression(&mut self, input: &mut &str, vt: ValueType) -> R<BytecodeExp> {
        let be = self.typed_exp(input, vt)?;
        if !self.token.is_empty() {
            return e(format!("Invalid data: {}", self.token));
        }
        Ok(be)
    }

    // ── the precedence chain ──────────────────────────────────────────────

    fn exp_bool_or(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        let prev = std::mem::take(&mut self.previous_operator);
        let r = self.exp_one_op(out, input, "||", bool_or, Op::Blor, Self::exp_bool_xor);
        self.previous_operator = prev;
        r
    }

    fn exp_bool_xor(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        self.exp_one_op(out, input, "^^", bool_xor, Op::Blxor, Self::exp_bool_and)
    }

    fn exp_bool_and(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        self.exp_one_op(out, input, "&&", bool_and, Op::Bland, Self::exp_or)
    }

    fn exp_or(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        self.exp_one_op(out, input, "|", bw_or, Op::Bwor, Self::exp_xor)
    }

    fn exp_xor(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        self.exp_one_op(out, input, "^", bw_xor, Op::Bwxor, Self::exp_and)
    }

    fn exp_and(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        self.exp_one_op(out, input, "&", bw_and, Op::Bwand, Self::exp_eqne)
    }

    /// expEqne (5945) — = / != with `[a,b]` range support
    fn exp_eqne(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        let mut bv = self.exp_grls(out, input)?;
        loop {
            self.operator(input)?;
            let is_eq = match self.token.as_str() {
                "=" => true,
                "!=" => false,
                _ => return Ok(bv),
            };
            self.tok(input);
            // range form: x = [min, max]  (expRange, 5855)
            if !self.norange && (self.token == "[" || self.token == "(") {
                let open = self.token.clone();
                self.tok(input);
                let mut be2: BytecodeExp = Vec::new();
                let bv2 = self.exp_bool_or(&mut be2, input)?;
                if self.token == "," {
                    self.tok(input);
                    let mut be3: BytecodeExp = Vec::new();
                    let bv3 = self.exp_bool_or(&mut be3, input)?;
                    let close = self.token.clone();
                    if close != "]" && close != ")" {
                        return e("Missing ']' or ')'");
                    }
                    self.tok(input);
                    let (im, ix) = match (open.as_str(), close.as_str()) {
                        ("[", "]") => (true, true),
                        ("[", ")") => (true, false),
                        ("(", "]") => (false, true),
                        _ => (false, false),
                    };
                    // emission: [bv][be2][bv2][be3][bv3][range] (5886-5903)
                    push_val(out, &bv);
                    out.extend(be2.drain(..));
                    push_val(out, &bv2);
                    out.extend(be3.drain(..));
                    push_val(out, &bv3);
                    out.push(Op::Range { incl_min: im, incl_max: ix });
                    if !is_eq {
                        out.push(Op::Blnot);
                    }
                    self.reverse_order = true;
                    return Ok(v_none());
                }
                // "(" without "," — not a range; Go pushes back (5965-5974)
                if open != "(" {
                    return e("Missing ','");
                }
                // "(" with no ",": parenthesized operand, not a range —
                // Go re-injects the raw text (5965-5974). Deferred: error.
                return e("Missing ','");
            }
            // plain = / !=  (expOneOpSub)
            let mut be2: BytecodeExp = Vec::new();
            let bv2 = self.exp_grls(&mut be2, input)?;
            if bv.is_none() || bv2.is_none() {
                push_val(out, &bv);
                out.extend(be2.drain(..));
                push_val(out, &bv2);
                out.push(if is_eq { Op::Eq } else { Op::Ne });
                bv = v_none();
            } else {
                bv = if is_eq {
                    eq(&bv, &bv2, &[], &[])
                } else {
                    ne(&bv, &bv2, &[], &[])
                };
            }
        }
    }

    /// expGrls (5824) — > >= < <=
    fn exp_grls(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        let mut bv = self.exp_adsb(out, input)?;
        loop {
            self.operator(input)?;
            let f: fn(&Value, &Value) -> Value;
            let opc: Op;
            match self.token.as_str() {
                ">" => { f = gt; opc = Op::Gt; }
                ">=" => { f = ge; opc = Op::Ge; }
                "<" => { f = lt; opc = Op::Lt; }
                "<=" => { f = le; opc = Op::Le; }
                _ => return Ok(bv),
            }
            self.tok(input);
            let mut be2: BytecodeExp = Vec::new();
            let bv2 = self.exp_adsb(&mut be2, input)?;
            if bv.is_none() || bv2.is_none() {
                push_val(out, &bv);
                out.extend(be2.drain(..));
                push_val(out, &bv2);
                out.push(opc);
                bv = v_none();
            } else {
                bv = f(&bv, &bv2);
            }
        }
    }

    /// expAdsb (5799) — + -
    fn exp_adsb(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        let mut bv = self.exp_mldv(out, input)?;
        loop {
            self.operator(input)?;
            let f: fn(&Value, &Value) -> Value;
            let opc: Op;
            match self.token.as_str() {
                "+" => { f = add; opc = Op::Add; }
                "-" => { f = sub; opc = Op::Sub; }
                _ => return Ok(bv),
            }
            self.tok(input);
            let mut be2: BytecodeExp = Vec::new();
            let bv2 = self.exp_mldv(&mut be2, input)?;
            if bv.is_none() || bv2.is_none() {
                push_val(out, &bv);
                out.extend(be2.drain(..));
                push_val(out, &bv2);
                out.push(opc);
                bv = v_none();
            } else {
                bv = f(&bv, &bv2);
            }
        }
    }

    /// expMldv (5771) — * / %
    fn exp_mldv(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        let mut bv = self.exp_pow(out, input)?;
        loop {
            self.operator(input)?;
            let f: fn(&Value, &Value) -> Value;
            let opc: Op;
            match self.token.as_str() {
                "*" => { f = mul; opc = Op::Mul; }
                "/" => { f = div; opc = Op::Div; }
                "%" => { f = modulo; opc = Op::Mod; }
                _ => return Ok(bv),
            }
            self.tok(input);
            let mut be2: BytecodeExp = Vec::new();
            let bv2 = self.exp_pow(&mut be2, input)?;
            if bv.is_none() || bv2.is_none() {
                push_val(out, &bv);
                out.extend(be2.drain(..));
                push_val(out, &bv2);
                out.push(opc);
                bv = v_none();
            } else {
                bv = f(&bv, &bv2);
            }
        }
    }

    /// expPow (5739) — **
    fn exp_pow(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        let mut bv = self.exp_post_not(out, input)?;
        loop {
            self.operator(input)?;
            if self.token != "**" {
                return Ok(bv);
            }
            self.tok(input);
            let mut be2: BytecodeExp = Vec::new();
            let bv2 = self.exp_post_not(&mut be2, input)?;
            if bv.is_none() || bv2.is_none() {
                push_val(out, &bv);
                out.extend(be2.drain(..));
                push_val(out, &bv2);
                out.push(Op::Pow);
                bv = v_none();
            } else {
                bv = pow(&bv, &bv2);
            }
        }
    }

    /// expPostNot (5689) — postfix `!` + operator sanity
    fn exp_post_not(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        let bv = self.exp_value(out, input)?;
        let mut b = bv;
        if self.token == "!" {
            b = bool_not(&b);
            self.tok(input);
        }
        if self.previous_operator.is_empty() {
            let opp = is_operator(&self.token);
            if opp == 0 && !(self.reverse_order && self.token == "(") {
                return e(format!(
                    "No comparison operator\nToken = '{}' String = '{}'\n[ECID 3]\n",
                    self.token, input
                ));
            }
        }
        Ok(b)
    }

    /// `operator()` (668) — token push-back when precedence regresses.
    /// Simplified vs Go: fires only for the malformed-input recovery path.
    fn operator(&mut self, input: &mut &str) -> R<()> {
        if !self.previous_operator.is_empty() {
            let prev_prec = is_operator(&self.previous_operator);
            let cur_prec = is_operator(&self.token);
            if cur_prec <= prev_prec {
                let c0 = self.token.chars().next().unwrap_or('\0');
                if cur_prec < 0 || ((!self.reverse_order || c0 != '(') && !c0.is_ascii_alphabetic())
                {
                    return e(format!("Invalid data: {}", self.previous_operator));
                }
                // Malformed input (Go tolerates under ignoreMostErrors by
                // pushing the token back and re-parsing; deferred — see
                // PARITY notes). Error visibly instead.
                return e(format!("Invalid data: {}", self.token));
            }
        }
        Ok(())
    }

    // ── expOneOpSub (5988) shared binary level ────────────────────────────

    fn exp_one_op(
        &mut self,
        out: &mut BytecodeExp,
        input: &mut &str,
        op_str: &str,
        fold: fn(&Value, &Value) -> Value,
        opc: Op,
        sub: fn(&mut Self, &mut BytecodeExp, &mut &str) -> R<Value>,
    ) -> R<Value> {
        let mut bv = sub(self, out, input)?;
        loop {
            self.operator(input)?;
            if self.token != op_str {
                return Ok(bv);
            }
            self.tok(input);
            let mut be2: BytecodeExp = Vec::new();
            let bv2 = sub(self, &mut be2, input)?;
            if bv.is_none() || bv2.is_none() {
                push_val(out, &bv);
                out.extend(be2.drain(..));
                push_val(out, &bv2);
                out.push(opc.clone());
                bv = v_none();
            } else {
                bv = fold(&bv, &bv2);
            }
        }
    }
}

/// Go `appendValue` — None emits nothing (bytecode.go:1569)
fn push_val(out: &mut BytecodeExp, bv: &Value) {
    if !bv.is_none() {
        out.push(Op::Push(*bv));
    }
}

/// One `in`-place helper used by exp_value: read a quoted string token
/// (compiler.go:1375 `text`).
fn read_quoted(input: &mut &str) -> Option<String> {
    if let Some(i) = input.find('"') {
        let s = input[..i].to_string();
        *input = &input[i + 1..];
        Some(s)
    } else {
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// expValue (compiler.go:1314-5666) — trigger keyword dispatch.
// Ports the subset needed by KFM + common1.cns.zss + functions.zss;
// unknown keywords error with "Invalid data" exactly like Go (compiler.go:5666).
// ═══════════════════════════════════════════════════════════════════════════

impl<'a> Compiler<'a> {
    pub fn exp_value(&mut self, out: &mut BytecodeExp, input: &mut &str) -> R<Value> {
        self.exp_value_rd(out, input, false)
    }

    /// `rd` = inside a redirect (Go param; affects OC_run_caller placement —
    /// our structural P2 op keeps the caller context naturally)
    pub fn exp_value_rd(&mut self, out: &mut BytecodeExp, input: &mut &str, _rd: bool) -> R<Value> {
        self.reverse_order = true;
        self.norange = false;

        // number literal (compiler.go:705 `number`) — no emission
        if let Some(bv) = self.number() {
            self.tok(input);
            return Ok(bv);
        }

        let tok = self.token.clone();
        match tok.as_str() {
            // ── string literal — only valid in comparisons (command = "x")
            "\"" => {
                return e("Not enclosed in \"");
            }

            // ── unary ! (Go 1811) and parentheses (1829) ──────────────────
            "!" => {
                self.tok(input);
                let mut be1: BytecodeExp = Vec::new();
                let bv1 = self.exp_value(&mut be1, input)?;
                if bv1.is_none() {
                    out.extend(be1);
                    out.push(Op::Blnot);
                    return Ok(v_none());
                } else {
                    return Ok(bool_not(&bv1));
                }
            }
            "(" => {
                self.tok(input);
                let mut be1: BytecodeExp = Vec::new();
                let bv1 = self.exp_bool_or(&mut be1, input)?;
                if bv1.is_none() {
                    out.extend(be1);
                }
                if self.token != ")" {
                    return e("Missing ')'");
                }
                self.tok(input); // consume ")" (Go: falls to shared tail)
                return Ok(bv1);
            }

            // ── state (Go cases ~1983-2050) ────────────────────────────────
            "anim" => out.push(Op::Trig(Trig::Anim)),
            "animtime" => out.push(Op::Trig(Trig::AnimTime)),
            "stateno" => out.push(Op::Trig(Trig::StateNo)),
            "prevstateno" => out.push(Op::Trig(Trig::PrevStateNo)),
            "time" => out.push(Op::Trig(Trig::Time)),
            "ctrl" => out.push(Op::Trig(Trig::Ctrl)),
            "ctrltime" => out.push(Op::Trig(Trig::CtrlTime)),
            "facing" => out.push(Op::Trig(Trig::Facing)),
            "alive" => out.push(Op::Trig(Trig::Alive)),
            "life" => out.push(Op::Trig(Trig::Life)),
            "lifemax" => out.push(Op::Trig(Trig::LifeMax)),
            "power" => out.push(Op::Trig(Trig::Power)),
            "powermax" => out.push(Op::Trig(Trig::PowerMax)),
            "id" => out.push(Op::Trig(Trig::Id)),
            "ishelper" => {
                // one optional arg (compiler.go case "ishelper")
                self.tok(input);
                if self.token == "(" {
                    self.tok(input);
                    let mut be: BytecodeExp = Vec::new();
                    let bv = self.exp_bool_or(&mut be, input)?;
                    push_val(&mut be, &bv);
                    if self.token != ")" {
                        return e("Missing ')'");
                    }
                    out.extend(be);
                    self.tok(input);
                    out.push(Op::Trig(Trig::IsHelper));
                    return Ok(v_none());
                }
                let rest = format!("{} {}", self.token, input);
                let leaked: &'static str = Box::leak(rest.into_boxed_str());
                *input = leaked;
                self.token = "ishelper".into();
                out.push(Op::Trig(Trig::IsHelper));
            }
            "numenemy" => out.push(Op::Trig(Trig::NumEnemy)),
            "numexplod" => {
                self.one_arg_trigger(input, out, Trig::NumExplod)?;
                return Ok(v_none());
            }
            "gametime" => out.push(Op::Trig(Trig::GameTime)),
            "roundstate" => out.push(Op::Trig(Trig::RoundState)),
            "roundno" => out.push(Op::Trig(Trig::RoundNo)),
            "teamside" => out.push(Op::Trig(Trig::TeamSide)),
            "palno" => out.push(Op::Trig(Trig::PalNo)),
            "inguarddist" => out.push(Op::Trig(Trig::InGuardDist)),
            "canrecover" => out.push(Op::Trig(Trig::CanRecover)),
            "random" => out.push(Op::Trig(Trig::Random)),
            "animloopcount" => out.push(Op::Trig(Trig::AnimLoopCount)),

            // ── hit vars (bare forms) ─────────────────────────────────────
            "hitcount" => out.push(Op::Trig(Trig::HitCount)),
            "uniqhitcount" => out.push(Op::Trig(Trig::UniqHitCount)),
            "hitpausetime" => out.push(Op::Trig(Trig::HitPauseTime)),
            "hitshakeover" => out.push(Op::Trig(Trig::HitShakeOver)),
            "hitover" => out.push(Op::Trig(Trig::HitOver)),
            "hitfall" => out.push(Op::Trig(Trig::HitFall)),
            "movecontact" => out.push(Op::Trig(Trig::MoveContact)),
            "movehit" => out.push(Op::Trig(Trig::MoveHit)),
            "moveguarded" => out.push(Op::Trig(Trig::MoveGuarded)),
            "movereversed" => out.push(Op::Trig(Trig::MoveReversed)),

            // ── position / velocity keyword.sub ───────────────────────────
            "pos" => {
                self.tok(input);
                match self.token.as_str() {
                    "x" => out.push(Op::Trig(Trig::PosX)),
                    "y" => out.push(Op::Trig(Trig::PosY)),
                    _ => return e(format!("Invalid Pos argument: {}", self.token)),
                }
            }
            "vel" => {
                self.tok(input);
                match self.token.as_str() {
                    "x" => out.push(Op::Trig(Trig::VelX)),
                    "y" => out.push(Op::Trig(Trig::VelY)),
                    _ => return e(format!("Invalid Vel argument: {}", self.token)),
                }
            }
            "screenpos" => {
                self.tok(input);
                match self.token.as_str() {
                    "x" => out.push(Op::Trig(Trig::ScreenPosX)),
                    "y" => out.push(Op::Trig(Trig::ScreenPosY)),
                    _ => return e(format!("Invalid ScreenPos argument: {}", self.token)),
                }
            }
            "hitvel" => {
                self.tok(input);
                match self.token.as_str() {
                    "x" => out.push(Op::Trig(Trig::HitVelX)),
                    "y" => out.push(Op::Trig(Trig::HitVelY)),
                    _ => return e(format!("Invalid HitVel argument: {}", self.token)),
                }
            }
            "backedge" => out.push(Op::Trig(Trig::BackEdge)),
            "backedgedist" => out.push(Op::Trig(Trig::BackEdgeDist)),
            "backedgebodydist" => out.push(Op::Trig(Trig::BackEdgeBodyDist)),
            "frontedge" => out.push(Op::Trig(Trig::FrontEdge)),
            "frontedgedist" => out.push(Op::Trig(Trig::FrontEdgeDist)),
            "frontedgebodydist" => out.push(Op::Trig(Trig::FrontEdgeBodyDist)),
            "p2bodydist" => {
                self.tok(input);
                match self.token.as_str() {
                    "x" => out.push(Op::Trig(Trig::P2BodyDistX)),
                    "y" => out.push(Op::Trig(Trig::P2DistY)),
                    _ => return e(format!("Invalid P2BodyDist argument: {}", self.token)),
                }
            }
            "p2dist" => {
                self.tok(input);
                match self.token.as_str() {
                    "x" => out.push(Op::Trig(Trig::P2DistX)),
                    "y" => out.push(Op::Trig(Trig::P2DistY)),
                    _ => return e(format!("Invalid P2Dist argument: {}", self.token)),
                }
            }
            "p2life" => out.push(Op::Trig(Trig::P2Life)),
            "p2stateno" => out.push(Op::Trig(Trig::P2StateNo)),
            "p2statetype" => out.push(Op::Trig(Trig::P2StateType)),
            "p2movetype" => out.push(Op::Trig(Trig::P2MoveType)),

            // ── statetype / movetype — string comparisons ──────────────────
            // (Go: flagTrigger + pushSelf → OC_statetype etc. push strings)
            "statetype" => {
                self.flag_trigger(out, input, Trig::StateType)?;
                return Ok(v_none());
            }
            "movetype" => {
                self.flag_trigger(out, input, Trig::MoveType)?;
                return Ok(v_none());
            }
            "physics" => {
                self.flag_trigger(out, input, Trig::Physics)?;
                return Ok(v_none());
            }

            // ── selfanimexist / animexist (one arg) ────────────────────────
            "selfanimexist" => {
                self.one_arg_trigger(input, out, Trig::SelfAnimExist)?;
                // SelfAnimExist is its own op that reads the arg
                if let Some(Op::Trig(Trig::SelfAnimExist)) = out.last() {
                    let last = out.len() - 1;
                    out[last] = Op::AnimExist;
                }
                return Ok(v_none());
            }
            "animexist" => {
                self.one_arg_trigger(input, out, Trig::SelfAnimExist)?;
                if let Some(Op::Trig(Trig::SelfAnimExist)) = out.last() {
                    let last = out.len() - 1;
                    out[last] = Op::AnimExist;
                }
                return Ok(v_none());
            }
            "animelemno" => {
                self.one_arg_trigger(input, out, Trig::AnimLoopCount)?;
                if let Some(Op::Trig(Trig::AnimLoopCount)) = out.last() {
                    let last = out.len() - 1;
                    out[last] = Op::AnimElemNo;
                }
                return Ok(v_none());
            }
            "animelemtime" => {
                self.one_arg_trigger(input, out, Trig::AnimLoopCount)?;
                if let Some(Op::Trig(Trig::AnimLoopCount)) = out.last() {
                    let last = out.len() - 1;
                    out[last] = Op::AnimElemTime;
                }
                return Ok(v_none());
            }

            // ── old-style `animelem = N` (compiler.go:4297) ────────────────
            "animelem" => {
                // parseOldAnimElemStyle: animelem = N  →  animelemtime(N) = 0
                self.tok(input);
                let is_eq = match self.token.as_str() {
                    "=" => Some(true),
                    "!=" => Some(false),
                    _ => None,
                };
                if is_eq.is_none() {
                    return e("AnimElem must be followed by = or !=");
                }
                self.tok(input);
                let n = match self.token.parse::<i32>() {
                    Ok(n) if n > 0 => n,
                    _ => return e("AnimElem must be greater than 0"),
                };
                // emit [Push(n)][AnimElemTime][Jud(skip)][Push(0)][Eq/Ne]
                let mut seq: BytecodeExp = vec![Op::Push(v_int(n)), Op::AnimElemTime];
                let jud_at = out.len() + seq.len();
                seq.push(Op::Jud(0)); // target patched below
                let mut tail: BytecodeExp = vec![Op::Push(v_int(0))];
                tail.push(if is_eq == Some(true) { Op::Eq } else { Op::Ne });
                let target = jud_at + 1 + tail.len();
                seq[2] = Op::Jud(target);
                out.extend(seq);
                out.extend(tail);
                self.tok(input);
                return Ok(v_none());
            }

            // ── var family ────────────────────────────────────────────────
            "var" | "sysvar" | "fvar" | "sysfvar" => {
                let which = tok.as_str();
                // oneArg (1141): read (expr)
                self.tok(input);
                if self.token != "(" {
                    return e(format!("Missing '(' after {}", which));
                }
                self.tok(input);
                let mut be: BytecodeExp = Vec::new();
                let bv = self.exp_bool_or(&mut be, input)?;
                push_val(&mut be, &bv);
                if self.token != ")" {
                    return e("Missing ')'");
                }
                self.tok(input);
                // `:=` assignment support (ZSS)
                if self.token == ":=" {
                    self.tok(input);
                    let mut be2: BytecodeExp = Vec::new();
                    let bv2 = self.exp_eqne(&mut be2, input)?;
                    push_val(&mut be2, &bv2);
                    out.extend(be);
                    out.extend(be2);
                    out.push(match which {
                        "var" => Op::AssignVar,
                        "sysvar" => Op::AssignSysvar,
                        "fvar" => Op::AssignFvar,
                        _ => Op::AssignSysfvar,
                    });
                    return Ok(v_none());
                }
                out.extend(be);
                out.push(match which {
                    "var" => Op::VarGet,
                    "sysvar" => Op::SysvarGet,
                    "fvar" => Op::FvarGet,
                    _ => Op::SysfvarGet,
                });
                return Ok(v_none());
            }

            // ── gethitvar(...) ────────────────────────────────────────────
            "gethitvar" => {
                self.tok(input);
                if self.token != "(" {
                    return e("Missing '(' after GetHitVar");
                }
                self.tok(input);
                let kind = match self.token.as_str() {
                    "xvel" => GetHitVarKind::Xvel,
                    "yvel" => GetHitVarKind::Yvel,
                    "zvel" => GetHitVarKind::Zvel,
                    "type" => GetHitVarKind::Type,
                    "animtype" => GetHitVarKind::AnimType,
                    "airtype" => GetHitVarKind::AirType,
                    "groundtype" => GetHitVarKind::GroundType,
                    "damage" => GetHitVarKind::Damage,
                    "hitcount" => GetHitVarKind::HitCount,
                    "fallcount" => GetHitVarKind::FallCount,
                    "hitshaketime" => GetHitVarKind::HitShakeTime,
                    "hittime" => GetHitVarKind::HitTime,
                    "slidetime" => GetHitVarKind::SlideTime,
                    "ctrltime" => GetHitVarKind::CtrlTime,
                    "reeltime" => GetHitVarKind::ReelTime,
                    "down.recovertime" => GetHitVarKind::DownRecoverTime,
                    "guardcount" => GetHitVarKind::GuardCount,
                    "id" => GetHitVarKind::Id,
                    "playerno" => GetHitVarKind::Playerno,
                    "fall.xvel" => GetHitVarKind::FallXvel,
                    "fall.yvel" => GetHitVarKind::FallYvel,
                    "fall.zvel" => GetHitVarKind::FallZvel,
                    "fall.envshake.time" => GetHitVarKind::FallEnvShakeTime,
                    "fall.envshake.freq" => GetHitVarKind::FallEnvShakeFreq,
                    "fall.envshake.ampl" => GetHitVarKind::FallEnvShakeAmpl,
                    "fall.envshake.phase" => GetHitVarKind::FallEnvShakePhase,
                    "fall" => GetHitVarKind::Fall,
                    "falltime" => GetHitVarKind::FallTime,
                    "guarded" => GetHitVarKind::Guarded,
                    "isbound" => GetHitVarKind::IsBound,
                    _ => return e(format!("Invalid GetHitVar argument: {}", self.token)),
                };
                self.tok(input);
                if self.token != ")" {
                    return e("Missing ')'");
                }
                out.push(Op::GetHitVar(kind));
            }

            // ── const(...) ────────────────────────────────────────────────
            "const" => {
                self.tok(input);
                if self.token != "(" {
                    return e("Missing '(' after Const");
                }
                self.tok(input);
                let kind = self.parse_const_kind()?;
                self.tok(input);
                if self.token != ")" {
                    return e("Missing ')'");
                }
                out.push(Op::Const(kind));
            }
            "const240p" | "const480p" | "const720p" => {
                let op = match tok.as_str() {
                    "const240p" => Op::Const240p,
                    "const480p" => Op::Const480p,
                    _ => Op::Const720p,
                };
                self.tok(input);
                if self.token != "(" {
                    return e(format!("Missing '(' after {}", tok));
                }
                self.tok(input);
                let mut be: BytecodeExp = Vec::new();
                let bv = self.exp_bool_or(&mut be, input)?;
                push_val(&mut be, &bv);
                if self.token != ")" {
                    return e("Missing ')'");
                }
                out.extend(be);
                out.push(op);
                self.tok(input);
                return Ok(v_none());
            }

            // ── math / misc one-arg functions ─────────────────────────────
            "abs" => {
                self.one_arg_trigger(input, out, Trig::AnimLoopCount)?;
                if let Some(Op::Trig(Trig::AnimLoopCount)) = out.last() {
                    let last = out.len() - 1;
                    out[last] = Op::Abs;
                }
                return Ok(v_none());
            }
            "floor" => {
                self.one_arg_trigger(input, out, Trig::AnimLoopCount)?;
                if let Some(Op::Trig(Trig::AnimLoopCount)) = out.last() {
                    let last = out.len() - 1;
                    out[last] = Op::Floor;
                }
                return Ok(v_none());
            }
            "ifelse" | "cond" => {
                self.tok(input);
                if self.token != "(" {
                    return e(format!("Missing '(' after {}", tok));
                }
                self.tok(input);
                let mut cexp: BytecodeExp = Vec::new();
                let bv = self.exp_bool_or(&mut cexp, input)?;
                push_val(&mut cexp, &bv);
                if self.token != "," {
                    return e("Missing ','");
                }
                self.tok(input);
                let mut aexp: BytecodeExp = Vec::new();
                let bv2 = self.exp_bool_or(&mut aexp, input)?;
                push_val(&mut aexp, &bv2);
                if self.token != "," {
                    return e("Missing ','");
                }
                self.tok(input);
                let mut bexp: BytecodeExp = Vec::new();
                let bv3 = self.exp_bool_or(&mut bexp, input)?;
                push_val(&mut bexp, &bv3);
                if self.token != ")" {
                    return e("Missing ')'");
                }
                out.push(Op::IfElse {
                    c: cexp.into(),
                    a: aexp.into(),
                    b: bexp.into(),
                });
                self.tok(input);
                return Ok(v_none());
            }

            // ── map(name) / map(name) := value ────────────────────────────
            "map" => {
                self.tok(input);
                if self.token != "(" {
                    return e("Missing '(' after Map");
                }
                self.tok(input);
                let mut be: BytecodeExp = Vec::new();
                let bv = self.exp_bool_or(&mut be, input)?;
                push_val(&mut be, &bv);
                if self.token != ")" {
                    return e("Missing ')'");
                }
                self.tok(input);
                if self.token == ":=" {
                    self.tok(input);
                    let mut be2: BytecodeExp = Vec::new();
                    let bv2 = self.exp_eqne(&mut be2, input)?;
                    push_val(&mut be2, &bv2);
                    out.extend(be);
                    out.extend(be2);
                    out.push(Op::MapSet);
                    return Ok(v_none());
                }
                out.extend(be);
                out.push(Op::MapGet);
                return Ok(v_none());
            }

            // ── mugenVersion / ikemenversion (ZSS functions.zss) ─────────
            "mugenversion" | "ikemenversion" => {
                out.push(Op::Trig(if tok == "mugenversion" {
                    Trig::MugenVersion
                } else {
                    Trig::IkemenVersion
                }));
            }

            // ── redirects: p2(expr) ────────────────────────────────────────
            "p2" => {
                self.tok(input);
                if self.token != "," {
                    return e("Missing ',' after P2");
                }
                self.tok(input);
                let mut be2: BytecodeExp = Vec::new();
                let bv2 = self.exp_value_rd(&mut be2, input, true)?;
                push_val(&mut be2, &bv2);
                out.push(Op::P2(be2.into()));
                return Ok(v_none());
            }

            // ── ZSS local var $name ───────────────────────────────────────
            t if t.len() >= 2 && t.starts_with('$') && t != "$_" => {
                match self.ctx.local_slot(&t[1..]) {
                    Some(vi) => out.push(Op::LocalVar(vi)),
                    None => return e(format!("{} is not defined", t)),
                }
            }

            // ── command = "name" (compiler.go:2197) ────────────────────────
            "command" | "selfcommand" => {
                let is_eq = match self.peek_eqne(input)? {
                    Some(ne) => ne,
                    None => return e(format!("No comparison operator after {}", tok)),
                };
                self.tok(input); // consume = / !=
                self.tok(input);
                if self.token != "\"" {
                    return e("Not enclosed in \"");
                }
                let name = match read_quoted(input) {
                    Some(n) => n,
                    None => return e("Not enclosed in \""),
                };
                if !self.ctx.command_exists(&name) {
                    return e(format!("Command doesn't exist: {}", name));
                }
                let idx = self.ctx.add_string(&name);
                out.push(Op::Push(v_string(idx)));
                out.push(Op::Command);
                if is_eq == Some(false) {
                    out.push(Op::Blnot);
                }
                self.tok(input);
                return Ok(v_none());
            }

            // ── hitdefattr = attr (compiler.go trgAttr) ───────────────────
            "hitdefattr" => {
                // read `=` then attr text like SCA,NA,SA
                let is_eq = match self.peek_eqne(input)? {
                    Some(ne) => ne,
                    None => return e("No comparison operator after hitdefattr"),
                };
                self.tok(input);
                self.tok(input);
                let attr_text = self.token.clone();
                // Go parses attr from raw text here (trgAttr)
                let _ = attr_text;
                // compile as: Trig(HitDefAttr) with attr string on stack
                let idx = self.ctx.add_string(&attr_text);
                out.push(Op::Push(v_string(idx)));
                out.push(Op::Trig(Trig::HitDefAttr));
                if is_eq == Some(false) {
                    out.push(Op::Blnot);
                }
                self.tok(input);
                return Ok(v_none());
            }

            _ => {
                return e(format!("Invalid data: {}", tok));
            }
        }

        // bare keyword consumed exactly one token (Go: fallthrough to 5667)
        self.tok(input);
        Ok(v_none())
    }

    /// `number()` (compiler.go:705): parse current token as numeric constant.
    /// Scientific-notation tokens return None (Go quirk → later error).
    fn number(&self) -> Option<Value> {
        let token = &self.token;
        let f: f64 = match token.parse() {
            Ok(f) => f,
            Err(_) => return None,
        };
        if token.contains('.') {
            Some(v_float(f as f32))
        } else if token.contains('e') || token.contains('E') {
            None
        } else if f > i32::MAX as f64 {
            Some(v_int(i32::MAX))
        } else if f < i32::MIN as f64 {
            Some(v_int(i32::MIN))
        } else {
            Some(v_int(f as i32))
        }
    }

    /// checkEquality (951) — peek `=` / `!=` without consuming input
    fn peek_eqne(&mut self, input: &mut &str) -> R<Option<Option<bool>>> {
        let saved = *input;
        let saved_tok = self.token.clone();
        let t = next_token_lower(input);
        let r = match t.as_str() {
            "=" => Some(Some(true)),
            "!=" => Some(Some(false)),
            _ => None,
        };
        *input = saved;
        self.token = saved_tok;
        Ok(r)
    }

    /// flagTrigger pattern (1401) for statetype/movetype/physics:
    /// bare → push string; `= S` → compare with string; `= expr` → runtime eq
    fn flag_trigger(&mut self, out: &mut BytecodeExp, input: &mut &str, t: Trig) -> R<()> {
        // peek for = / !=
        match self.peek_eqne(input)? {
            None => {
                // bare use: push the string value
                out.push(Op::Trig(t));
                self.tok(input);
                Ok(())
            }
            Some(is_eq) => {
                self.tok(input); // consume = / !=
                self.tok(input);
                // RHS: keyword letter or quoted or expression
                let rhs_str: Option<String> = if self.token == "\"" {
                    read_quoted(input)
                } else if self.token.chars().all(|c| c.is_ascii_alphabetic()) && !self.token.is_empty()
                {
                    Some(self.token.clone())
                } else {
                    None
                };
                if let Some(s) = rhs_str {
                    let idx = self.ctx.add_string(&s);
                    out.push(Op::Trig(t));
                    out.push(Op::Push(v_string(idx)));
                    if is_eq == Some(true) {
                        out.push(Op::Eq);
                    } else {
                        out.push(Op::Ne);
                    }
                    self.tok(input);
                } else {
                    // expression RHS
                    let mut be2: BytecodeExp = Vec::new();
                    let bv2 = self.exp_grls(&mut be2, input)?;
                    push_val(&mut be2, &bv2);
                    out.push(Op::Trig(t));
                    out.extend(be2.drain(..));
                    if is_eq == Some(true) {
                        out.push(Op::Eq);
                    } else {
                        out.push(Op::Ne);
                    }
                }
                Ok(())
            }
        }
    }

    /// oneArg (1141) helper: `(expr)` — emits arg then caller pushes the op
    fn one_arg_trigger(&mut self, input: &mut &str, out: &mut BytecodeExp, t: Trig) -> R<()> {
        self.tok(input);
        if self.token != "(" {
            return e(format!("Missing '(' after trigger"));
        }
        self.tok(input);
        let mut be: BytecodeExp = Vec::new();
        let bv = self.exp_bool_or(&mut be, input)?;
        push_val(&mut be, &bv);
        if self.token != ")" {
            return e("Missing ')'");
        }
        out.extend(be);
        out.push(Op::Trig(t));
        self.tok(input);
        Ok(())
    }

    /// const(...) sub-keyword parsing (compiler.go:2244+)
    fn parse_const_kind(&mut self) -> R<ConstKind> {
        let k = ConstKind::from_name(&self.token);
        Ok(k)
    }
}
