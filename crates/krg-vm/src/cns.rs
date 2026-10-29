//! Port origin: ikemen-src/src/compiler.go — CNS path:
//! `Compile` (8558-8809), `stateCompile`/`stateCompileCNS` (7166-7534),
//! `stateDef` (7020-7122), trigger loop (7311-7502), and the .cns constants
//! sections ([Data]/[Size]/[Velocity]/[Movement]/[Constants]).
//!
//! Faithful behaviors (compiler.go citations inline):
//! * one physical line = one param; `;` comments cut, no quote awareness
//! * `[statedef N]` requires exactly "statedef " (one space); number via
//!   scanStateDef: `const(name)`/`+1`→−10/`-N`/Atoi
//! * `[State N, label]` — the number and label are *ignored* (any content)
//! * persistent: Atoi, >128→1, ≤0 (≠1)→run-once (i32::MAX)
//! * triggerall AND · triggerN rows OR · same-number rows AND
//! * constant-0 trigger rows → never; constant-true → always (no expr)
//! * trigger1 mandatory; sctrl discarded when nothing can fire
//! * fast path: no trigger + persistent==1 + !ignorehitpause → direct node
//! * duplicate Statedef in one file skipped; positive states first-wins;
//!   negative states merge (old ctrls first, statedef params ignored)

use crate::cmd::CommandList;
use crate::exp::{CompileCtx, Compiler};
use crate::ini::{atoi, load_text, read_ini_section, section_name, split_and_trim, IniSection};
use crate::op::Op;
use crate::sctrl::{compile_sctrl, Sctrl, SctrlKind};
use crate::statedef::{
    MoveTypeT, Node, StateBlock, StateBytecode, StateTypeT, Triggers,
};
use crate::value::{v_bool, v_int, Value, ValueType};
use std::collections::HashMap;

// ── constants ([Data]/[Size]/[Velocity]/[Movement]/[Constants]) ───────────────

/// Typed char constants — every field mirrors compiler.go/char.go defaults.
#[derive(Debug, Clone)]
pub struct CharData {
    pub life: i32,
    pub attack: i32,
    pub defence: i32,
    pub fall_defence_up: i32,
    pub liedown_time: i32,
    pub airjuggle: i32,
    pub sparkno: i32,
    pub guard_sparkno: i32,
    pub ko_echo: i32,
    pub volume: i32,
    pub int_persist_index: i32,
    pub float_persist_index: i32,
}

impl Default for CharData {
    fn default() -> Self {
        CharData {
            life: 1000,
            attack: 100,
            defence: 100,
            fall_defence_up: 50,
            liedown_time: 60,
            airjuggle: 15,
            sparkno: 2,
            guard_sparkno: 40,
            ko_echo: 0,
            volume: 0,
            int_persist_index: 60,
            float_persist_index: 40,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CharSize {
    pub xscale: f32,
    pub yscale: f32,
    pub ground_back: f32,
    pub ground_front: f32,
    pub air_back: f32,
    pub air_front: f32,
    pub height: f32,
    pub attack_dist: f32,
    pub proj_attack_dist: f32,
    pub proj_doscale: i32,
    pub head_pos: (i32, i32),
    pub mid_pos: (i32, i32),
    pub shadowoffset: i32,
    pub draw_offset: (i32, i32),
}

impl Default for CharSize {
    fn default() -> Self {
        CharSize {
            xscale: 1.0,
            yscale: 1.0,
            ground_back: 15.0,
            ground_front: 16.0,
            air_back: 12.0,
            air_front: 12.0,
            height: 60.0,
            attack_dist: 160.0,
            proj_attack_dist: 90.0,
            proj_doscale: 0,
            head_pos: (0, 0),
            mid_pos: (0, 0),
            shadowoffset: 0,
            draw_offset: (0, 0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CharVelocity {
    pub walk_fwd: f32,
    pub walk_back: f32,
    pub run_fwd: (f32, f32),
    pub run_back: (f32, f32),
    pub jump_y: f32,
    /// jump.neu = x, y — the x component (neutral-jump drift, usually 0)
    pub jump_neu_x: f32,
    pub jump_back: f32,
    pub jump_fwd: f32,
    pub runjump_back: (f32, f32),
    pub runjump_fwd: (f32, f32),
    pub airjump_y: f32,
    pub airjump_neu_x: f32,
    pub airjump_back: f32,
    pub airjump_fwd: f32,
    pub air_gethit_ground_recover: (f32, f32),
    pub air_gethit_air_recover_mul: (f32, f32),
    pub air_gethit_air_recover_add: (f32, f32),
    pub air_gethit_air_recover_back: f32,
    pub air_gethit_air_recover_fwd: f32,
    pub air_gethit_air_recover_up: f32,
    pub air_gethit_air_recover_down: f32,
}

impl Default for CharVelocity {
    fn default() -> Self {
        CharVelocity {
            walk_fwd: 2.4,
            walk_back: -2.2,
            run_fwd: (4.0, 0.0),
            run_back: (-4.2, 0.0),
            jump_y: -8.0,
            jump_neu_x: 0.0,
            jump_back: -2.75,
            jump_fwd: 2.6,
            runjump_back: (-2.55, -8.0),
            runjump_fwd: (2.5, -8.0),
            airjump_y: -8.5,
            airjump_neu_x: 0.0,
            airjump_back: -2.55,
            airjump_fwd: 2.5,
            air_gethit_ground_recover: (-0.35, -5.0),
            air_gethit_air_recover_mul: (0.75, 0.75),
            air_gethit_air_recover_add: (-2.0, -4.5),
            air_gethit_air_recover_back: -2.0,
            air_gethit_air_recover_fwd: 2.0,
            air_gethit_air_recover_up: -4.0,
            air_gethit_air_recover_down: 4.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CharMovement {
    pub airjump_height: i32,
    pub yaccel: f32,
    pub stand_friction: f32,
    pub crouch_friction: f32,
    pub air_friction: f32,
    pub ground_friction: f32,
    pub gravity: f32,
    pub fall_defence_up: i32,
    pub jump_helddown_bonus: f32,
}

impl Default for CharMovement {
    fn default() -> Self {
        CharMovement {
            airjump_height: 35,
            yaccel: 0.44,
            stand_friction: 0.85,
            crouch_friction: 0.82,
            air_friction: 0.03,
            ground_friction: 0.85,
            gravity: 0.44,
            fall_defence_up: 50,
            jump_helddown_bonus: 0.0,
        }
    }
}

/// All char constants + named constants (common.const [Constants]) +
/// compiled states + commands + string pool. The result of compiling a
/// character.
pub struct CompiledChar {
    pub name: String,
    pub data: CharData,
    pub size: CharSize,
    pub velocity: CharVelocity,
    pub movement: CharMovement,
    /// named constants, ordered (index = ConstKind::Named payload)
    pub named: Vec<(String, Value)>,
    pub states: HashMap<i32, StateBytecode>,
    pub cmd_list: CommandList,
    /// the string pool — expressions reference it by index
    pub string_pool: Vec<String>,
    /// ZSS functions: name → (num_vars, StateBlock)
    pub funcs: HashMap<String, crate::zss::FuncBytecode>,
    /// compile warnings (single string keeps transfer cheap)
    pub warnings: Vec<String>,
}

// ── the CNS compiler ──────────────────────────────────────────────────────────

/// Compile context shared across one character's files (the string pool
/// persists; compiler.go:8773 clears it only at char start).
pub struct StateCompiler<'a> {
    pub string_pool: Vec<String>,
    pub commands: &'a CommandList,
    pub named: Vec<(String, Value)>,
    pub named_map: HashMap<String, usize>,
    pub locals: Vec<String>,
    pub funcs: HashMap<String, crate::zss::FuncBytecode>,
    pub func_order: Vec<String>,
    pub warnings: Vec<String>,
    pub player_no: usize,
}

impl<'a> CompileCtx for StateCompiler<'a> {
    fn add_string(&mut self, s: &str) -> usize {
        // pool dedup (Go StringPool.Add behavior, bytecode.go:1281)
        if let Some(i) = self.string_pool.iter().position(|p| p.eq_ignore_ascii_case(s)) {
            return i;
        }
        self.string_pool.push(s.to_string());
        self.string_pool.len() - 1
    }
    fn command_exists(&self, name: &str) -> bool {
        self.commands.get(name).is_some()
    }
    fn local_slot(&self, name: &str) -> Option<u8> {
        self.locals.iter().position(|l| l == name).map(|i| i as u8)
    }
    fn func_idx(&mut self, name: &str) -> usize {
        self.add_string(name)
    }
    fn const_idx(&mut self, name: &str) -> Option<usize> {
        let k = name.to_lowercase();
        if let Some(i) = self.named_map.get(&k) {
            return Some(*i);
        }
        // lazily register (value resolved at runtime via named vec)
        let idx = self.named.len();
        self.named.push((k.clone(), Value { vtype: ValueType::None, value: 0.0 }));
        self.named_map.insert(k, idx);
        Some(idx)
    }
}

impl<'a> StateCompiler<'a> {
    pub fn new(commands: &'a CommandList, player_no: usize) -> Self {
        StateCompiler {
            string_pool: Vec::new(),
            commands,
            named: Vec::new(),
            named_map: HashMap::new(),
            locals: Vec::new(),
            funcs: HashMap::new(),
            func_order: Vec::new(),
            warnings: Vec::new(),
            player_no,
        }
    }

    fn warn(&mut self, msg: String) {
        self.warnings.push(msg);
    }

    /// scanStateDef (compiler.go:7759): number after "[Statedef ".
    fn scan_state_def(&mut self, rest: &str) -> Result<i32, String> {
        let t = rest.trim().to_lowercase();
        if t == "+1" {
            return Ok(-10);
        }
        if let Some(stripped) = t.strip_prefix("const(") {
            let name = stripped.trim_end_matches(')').trim();
            if let Some(&idx) = self.named_map.get(&name.to_lowercase()) {
                if let Some((_, v)) = self.named.get(idx) {
                    return Ok(v.to_i());
                }
            }
            return Err(format!("StateDef const({name}) not defined"));
        }
        if t.starts_with('-') {
            let n = atoi(&t[1..]);
            if n == 0 && !t[1..].starts_with(|c: char| c.is_ascii_digit()) {
                return Err(format!("Invalid StateDef number: {t}"));
            }
            return Ok(-n);
        }
        Ok(atoi(&t))
    }

    /// stateDef (compiler.go:7020): compile Statedef params.
    /// Negative states: header params ignored (7023-7028).
    fn compile_state_def(
        &mut self,
        is: &mut IniSection,
        sbc: &mut StateBytecode,
        state_no: i32,
    ) -> Result<(), String> {
        if state_no < 0 {
            return Ok(());
        }
        let mut comp = Compiler::new(self as &mut dyn CompileCtx);
        // type / movetype / physics — letters-only or string expression
        for (key, target, exp_target) in [
            ("type", 0usize, 0usize),
            ("movetype", 1, 1),
            ("physics", 2, 2),
        ] {
            let Some(v) = is.take(key) else { continue };
            let data = v.trim();
            if data.is_empty() {
                continue;
            }
            let letters_ok = match key {
                "type" => data.chars().all(|c| "SCALUscalu".contains(c)),
                "movetype" => data.chars().all(|c| "IAHUiahu".contains(c)),
                _ => data.chars().all(|c| "SCANUscanu".contains(c)),
            };
            if letters_ok {
                match key {
                    "type" => {
                        sbc.state_type = StateTypeT::from_str(data)
                            .unwrap_or(StateTypeT::U);
                    }
                    "movetype" => {
                        sbc.move_type = MoveTypeT::from_str(data).unwrap_or(MoveTypeT::U);
                    }
                    _ => {
                        sbc.physics = StateTypeT::from_str(data).unwrap_or(StateTypeT::U);
                    }
                }
            } else {
                // dynamic string expression form
                let mut d = data.to_string();
                let mut dr = d.as_str();
                let exp = comp.full_expression(&mut dr, ValueType::String)?;
                match key {
                    "type" => sbc.state_type_exp = Some(exp),
                    "movetype" => sbc.move_type_exp = Some(exp),
                    _ => sbc.physics_exp = Some(exp),
                }
            }
            let _ = (target, exp_target);
        }
        // remaining statedef params → a StateDef pseudo-sctrl
        let mut sd_is = IniSection::new();
        for (k, v) in is.iter() {
            sd_is.insert(k, v.clone());
        }
        // build as a null-kind carrier with reserved ids
        let mut sdef = Sctrl {
            kind: SctrlKind::Null,
            ignorehitpause: true,
            params: Vec::new(),
        };
        // keys we handle
        const SD_HITCOUNTPERSIST: u16 = 1;
        const SD_MOVEHITPERSIST: u16 = 2;
        const SD_HITDEFPERSIST: u16 = 3;
        const SD_SPRPRIORITY: u16 = 4;
        const SD_FACEP2: u16 = 5;
        const SD_JUGGLE: u16 = 6;
        const SD_VELSET: u16 = 7; // 3 args
        const SD_ANIM: u16 = 10;
        const SD_CTRL: u16 = 11;
        const SD_POWERADD: u16 = 12;
        let mut expr_of = |key: &str, vt: ValueType, nargs: usize| -> Result<(), String> {
            let Some(v) = is.take(key) else { return Ok(()) };
            if v.trim().is_empty() {
                return Ok(());
            }
            let id = match key {
                "hitcountpersist" => SD_HITCOUNTPERSIST,
                "movehitpersist" => SD_MOVEHITPERSIST,
                "hitdefpersist" => SD_HITDEFPERSIST,
                "sprpriority" => SD_SPRPRIORITY,
                "facep2" => SD_FACEP2,
                "juggle" => SD_JUGGLE,
                "velset" => SD_VELSET,
                "anim" => SD_ANIM,
                "ctrl" => SD_CTRL,
                "poweradd" => SD_POWERADD,
                _ => return Ok(()),
            };
            let mut d = v;
            let mut dr = d.as_str();
            let mut n = 0usize;
            while n < nargs {
                let be = if n + 1 < nargs {
                    comp.arg_expression(&mut dr, vt)?
                } else {
                    comp.full_expression(&mut dr, vt)?
                };
                sdef.params.push((id + n as u16, be));
                n += 1;
                if !comp.trailing_comma() {
                    break;
                }
            }
            Ok(())
        };
        for (key, vt, nargs) in [
            ("hitcountpersist", ValueType::Bool, 1),
            ("movehitpersist", ValueType::Bool, 1),
            ("hitdefpersist", ValueType::Bool, 1),
            ("sprpriority", ValueType::Int, 1),
            ("facep2", ValueType::Bool, 1),
            ("juggle", ValueType::Int, 1),
            ("velset", ValueType::Float, 3),
            ("ctrl", ValueType::Bool, 1),
            ("poweradd", ValueType::Int, 1),
        ] {
            expr_of(key, vt, nargs)?;
        }
        // anim: prefixable
        if let Some(v) = is.take("anim") {
            if !v.trim().is_empty() {
                let (mut id, data) = if v.trim_start().len() >= 1 {
                    let t = v.trim_start();
                    let c0 = t.as_bytes()[0];
                    if (c0 == b'f' || c0 == b'F' || c0 == b's' || c0 == b'S')
                        && (t.len() == 1 || !(t.as_bytes()[1].is_ascii_alphanumeric()))
                    {
                        (SD_ANIM | crate::sctrl::PREFIX_BIT, t[1..].to_string())
                    } else {
                        (SD_ANIM, v.clone())
                    }
                } else {
                    (SD_ANIM, v.clone())
                };
                let mut d = data;
                let mut dr = d.as_str();
                let be = comp.full_expression(&mut dr, ValueType::Int)?;
                sdef.params.push((id, be));
                let _ = &mut id;
            }
        }
        let _ = sd_is;
        sbc.state_def = Some(sdef);
        Ok(())
    }

    /// parseTriggerNumber (compiler.go:6183): returns (n, is_all)
    fn parse_trigger_number(name: &str) -> Option<(Option<u32>, bool)> {
        if !name.starts_with("trigger") {
            return None;
        }
        let suffix = &name[7..];
        if suffix.eq_ignore_ascii_case("all") {
            return Some((None, true));
        }
        // readDigit (common.go:451): pure digits, no leading zero
        if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if suffix.len() > 1 && suffix.starts_with('0') {
            return None;
        }
        let n: u32 = suffix.parse().ok()?;
        if !(1..=65536).contains(&n) {
            return None;
        }
        Some((Some(n), false))
    }

    /// stateCompileCNS (compiler.go:7212-7534) — one .cns text → states.
    /// `negoverride`: char is Mugen-version → negative states of this file win.
    pub fn compile_cns_text(
        &mut self,
        text: &str,
        filename: &str,
        negoverride: bool,
        states: &mut HashMap<i32, StateBytecode>,
    ) -> Result<(), String> {
        let lines = split_and_trim(text, "\n");
        let mut exist_in_file: std::collections::HashSet<i32> = Default::default();
        let mut i = 0usize;

        while i < lines.len() {
            // compiler.go:7232: lowercase, trim, strip ';' comment
            let line = lines[i]
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            i += 1;
            if line.len() < 11
                || !line.starts_with('[')
                || !line.ends_with(']')
                || &line[1..10] != "statedef "
            {
                continue;
            }
            // [Statedef N] — strip the closing ']' (Go checks it then
            // scan() never consumes it)
            let header_rest = &line[10..line.len() - 1];
            let state_no = self.scan_state_def(header_rest)?;
            if exist_in_file.contains(&state_no) {
                let msg = format!(
                    "{}: Duplicate statedef {} in the same file — skipping second",
                    filename, state_no
                );
                self.warn(msg);
                // skip to next statedef; sctrls of the dup land nowhere —
                // emulate by continuing the scan (Go keeps the FIRST block)
                // (Go skips the *second* definition entirely.)
                continue;
            }
            exist_in_file.insert(state_no);

            // parse the Statedef header section (until next '[')
            let (mut is, _, _) = {
                let mut j = i;
                let sec = IniSection::parse(&lines, &mut j);
                i = j;
                let (_, n, s) = (String::new(), String::new(), String::new());
                (sec, n, s)
            };

            let mut sbc = StateBytecode::new(self.player_no);

            // negative-state merge: start from the existing definition,
            // append this file's sctrls after the old ones (7269-7271)
            let mut merging = false;
            if state_no < 0 {
                if let Some(old) = states.get(&state_no) {
                    sbc = old.clone();
                    merging = true;
                }
            }

            self.compile_state_def(&mut is, &mut sbc, state_no)?;

            // sctrl ordinal counter for persistent indices
            let mut ordinal = sbc.block.ctrls.len() as i32;

            // scan [State ...] sections
            loop {
                if i >= lines.len() {
                    break;
                }
                let raw = lines[i].split(';').next().unwrap_or("").trim().to_string();
                if raw.is_empty() || !raw.starts_with('[') {
                    i += 1;
                    continue;
                }
                if !raw.ends_with(']') {
                    i += 1;
                    continue; // "[State 200, 1" unclosed → skipped (7283-7285)
                }
                let lower = raw.to_lowercase();
                if lower.len() < 7 || &lower[1..7] != "state " {
                    break; // next non-[State] header (e.g. next Statedef)
                }
                i += 1;
                // [State N, label] — N and label are ignored entirely

                // parse this section's params with the trigger callback
                let mut block = StateBlock::default();
                let mut scf: Option<SctrlKind> = None;
                let mut triggerall: Vec<crate::op::BytecodeExp> = Vec::new();
                let mut rows: Vec<Option<Vec<crate::op::BytecodeExp>>> = Vec::new();
                // trexist: 0=missing, 1=exists, -1=const false
                let mut trexist: Vec<i32> = Vec::new();
                let mut sec = IniSection::new();

                let mut j = i;
                while j < lines.len() {
                    let l = lines[j].split(';').next().unwrap_or("").trim().to_string();
                    if !l.is_empty() && l.starts_with('[') {
                        break;
                    }
                    j += 1;
                    // find top-level '=' (parens aware, compiler.go:6205)
                    if let Some(eq) = find_top_level_equal(&l) {
                        if eq > 0 {
                            let lhs = l[..eq].trim();
                            let data = l[eq + 1..].trim();
                            let name = if let Some(sp) = parse_special_param_name(lhs) {
                                sp
                            } else {
                                lhs.to_lowercase()
                            };
                            if let Some((tn, is_all)) = Self::parse_trigger_number(&name) {
                                if data.is_empty() {
                                    return Err(format!("{name} cannot be empty"));
                                }
                                let mut comp = Compiler::new(self as &mut dyn CompileCtx);
                                let mut d = data.to_string();
                                let mut dr = d.as_str();
                                let be = comp.full_expression(&mut dr, ValueType::Bool)
                                    .map_err(|err| format!("in '{name} = {data}': {err}"))?;
                                // constant folding (compiler.go:7401-7413)
                                let const_val = be
                                    .first()
                                    .and_then(|op| match op {
                                        Op::Push(v) => Some(*v),
                                        _ => None,
                                    })
                                    .filter(|_| be.len() == 1);
                                if is_all {
                                    match const_val {
                                        Some(v) if !v.to_b() => {}
                                        _ => triggerall.push(be),
                                    }
                                } else if let Some(n) = tn {
                                    let idx = (n - 1) as usize;
                                    if rows.len() < idx + 1 {
                                        rows.resize(idx + 1, None);
                                        trexist.resize(idx + 1, 0);
                                    }
                                    match const_val {
                                        Some(v) if !v.to_b() => {
                                            trexist[idx] = -1;
                                        }
                                        Some(_) => {
                                            if trexist[idx] == 0 {
                                                trexist[idx] = 1;
                                                rows[idx] = Some(Vec::new());
                                            }
                                        }
                                        None => {
                                            if trexist[idx] >= 0 {
                                                if rows[idx].is_none() {
                                                    rows[idx] = Some(Vec::new());
                                                }
                                                rows[idx].as_mut().unwrap().push(be);
                                                trexist[idx] = 1;
                                            }
                                        }
                                        // (const-true handled above)
                                    }
                                }
                                continue;
                            }
                            match name.as_str() {
                                "type" => {
                                    scf = crate::sctrl::kind_from_name(data);
                                    if scf.is_none() {
                                        return Err(format!(
                                            "Invalid state controller: {data}"
                                        ));
                                    }
                                }
                                "persistent" => {
                                    if state_no >= 0 {
                                        let p = atoi(data);
                                        block.persistent = if p > 128 {
                                            1
                                        } else if p != 1 && p <= 0 {
                                            i32::MAX
                                        } else {
                                            p
                                        };
                                    }
                                }
                                "ignorehitpause" => {
                                    let ih = atoi(data) != 0;
                                    block.ignorehitpause = ih;
                                    block.ctrls_ignorehitpause = ih;
                                }
                                _ => {
                                    if !sec.contains(&name) {
                                        sec.insert(&name, data);
                                    }
                                }
                            }
                        }
                    }
                }
                i = j;

                // trigger1 mandatory (compiler.go:7438)
                if scf.is_none() {
                    return Err(format!(
                        "{filename} state {state_no}: State controller type not specified"
                    ));
                }
                if trexist.is_empty() || trexist[0] == 0 {
                    // missing trigger1 — Go errors; keep lenient with warn
                    self.warn(format!(
                        "{filename} state {state_no}: Missing trigger1"
                    ));
                }

                // assemble the trigger set (compiler.go:7443-7496 semantics)
                let mut any_possible = false;
                for (idx, te) in trexist.iter().enumerate() {
                    if *te == 0 {
                        break; // first missing row ends the chain
                    }
                    if *te > 0 {
                        any_possible = true;
                    }
                }
                let has_any_trigger = !triggerall.is_empty() || trexist.iter().any(|t| *t != 0);
                let trigger = if has_any_trigger {
                    Some(Triggers {
                        triggerall,
                        rows,
                        any_possible,
                    })
                } else {
                    None
                };

                // compile the sctrl params
                let kind = scf.unwrap();
                let mut collected: Vec<String> = Vec::new();
                {
                    let mut comp = Compiler::new(self as &mut dyn CompileCtx);
                    let sctrl = compile_sctrl(kind, &mut sec, &mut comp, &mut |m: String| {
                        collected.push(m)
                    })?;
                    // discard when no trigger can fire (compiler.go:7504-7516)
                    if let Some(t) = &trigger {
                        if !t.any_possible && !t.triggerall.is_empty() {
                            continue;
                        }
                    }
                    block.persistent_index = ordinal;
                    ordinal += 1;
                    sbc.ctrlsps.resize(ordinal.max(sbc.ctrlsps.len() as i32) as usize, 0);
                    if trigger.is_none() && block.persistent == 1 && !block.ignorehitpause {
                        sbc.block.ctrls.push(Node::Sctrl(sctrl));
                    } else {
                        block.trigger = trigger;
                        block.ctrls.push(Node::Sctrl(sctrl));
                        if block.ctrls_ignorehitpause {
                            sbc.block.ignorehitpause = true;
                        }
                        sbc.block.ctrls.push(Node::Block(Box::new(block)));
                    }
                }
                self.warnings.extend(collected.drain(..));
                let _ = merging;
                continue;

            }

            // insert (7538-7540): first-wins positive; negative override rules
            let should_insert = !states.contains_key(&state_no)
                || (state_no < 0 && !negoverride);
            if should_insert {
                states.insert(state_no, sbc);
            } else if state_no < 0 && negoverride {
                // Mugen-version char's own negative states win outright
                states.insert(state_no, sbc);
            }
        }
        Ok(())
    }
}

/// findTopLevelEqual (compiler.go:6205) — '=' outside parentheses.
fn find_top_level_equal(s: &str) -> Option<usize> {
    let mut uneven = 0i32;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'(' => uneven += 1,
            b')' => {
                if uneven > 0 {
                    uneven -= 1;
                }
            }
            b'=' if uneven == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// parseSpecialParamName (compiler.go:6226) — `var(…)`/`fvar(…)`/`sysvar(…)`/
/// `sysfvar(…)`/`map(…)` keys keep their inner text verbatim.
fn parse_special_param_name(lhs: &str) -> Option<String> {
    let lhs = lhs.trim();
    let i = lhs.find('(')?;
    if i == 0 {
        return None;
    }
    let fn_name = lhs[..i].trim().to_lowercase();
    if !matches!(fn_name.as_str(), "var" | "fvar" | "sysvar" | "sysfvar" | "map") {
        return None;
    }
    let mut uneven = 0i32;
    let mut end = None;
    for (j, b) in lhs.bytes().enumerate() {
        match b {
            b'(' => uneven += 1,
            b')' => {
                uneven -= 1;
                if uneven == 0 {
                    end = Some(j);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    if end != lhs.len() - 1 {
        return None;
    }
    Some(lhs[..end + 1].trim().to_lowercase())
}

// ── constants section reader (char.cns [Data]/[Size]/…) ──────────────────────

/// Read the leading constants sections of a .cns text (before any
/// [Statedef]) and the [Constants] named map.
pub fn read_constants(
    text: &str,
    comp: &mut StateCompiler,
    data: &mut CharData,
    size: &mut CharSize,
    velocity: &mut CharVelocity,
    movement: &mut CharMovement,
) {
    let lines = split_and_trim(text, "\n");
    let mut i = 0usize;
    loop {
        let (sec, name, _sub) = read_ini_section(&lines, &mut i);
        if name.is_empty() {
            break;
        }
        let n = name.trim_end_matches(']').trim().to_lowercase();
        match n.as_str() {
            "data" => {
                if let Some(v) = sec.read_i32_one("life") { data.life = v; }
                if let Some(v) = sec.read_i32_one("attack") { data.attack = v; }
                if let Some(v) = sec.read_i32_one("defence") { data.defence = v; }
                if let Some(v) = sec.read_i32_one("fall.defence_up") { data.fall_defence_up = v; }
                if let Some(v) = sec.read_i32_one("liedown.time") { data.liedown_time = v; }
                if let Some(v) = sec.read_i32_one("airjuggle") { data.airjuggle = v; }
                if let Some(v) = sec.read_i32_one("sparkno") { data.sparkno = v; }
                if let Some(v) = sec.read_i32_one("guard.sparkno") { data.guard_sparkno = v; }
                if let Some(v) = sec.read_i32_one("ko.echo") { data.ko_echo = v; }
                if let Some(v) = sec.read_i32_one("volume") { data.volume = v; }
                if let Some(v) = sec.read_i32_one("intpersistindex") { data.int_persist_index = v; }
                if let Some(v) = sec.read_i32_one("floatpersistindex") { data.float_persist_index = v; }
            }
            "size" => {
                let mut f = [0.0f32; 2];
                if sec.read_f32("xscale", &mut [&mut f[0]]) { size.xscale = f[0]; }
                if sec.read_f32("yscale", &mut [&mut f[0]]) { size.yscale = f[0]; }
                if sec.read_f32("ground.back", &mut [&mut f[0]]) { size.ground_back = f[0]; }
                if sec.read_f32("ground.front", &mut [&mut f[0]]) { size.ground_front = f[0]; }
                if sec.read_f32("air.back", &mut [&mut f[0]]) { size.air_back = f[0]; }
                if sec.read_f32("air.front", &mut [&mut f[0]]) { size.air_front = f[0]; }
                if sec.read_f32("height", &mut [&mut f[0]]) { size.height = f[0]; }
                if sec.read_f32("attack.dist", &mut [&mut f[0]]) { size.attack_dist = f[0]; }
                if sec.read_f32("proj.attack.dist", &mut [&mut f[0]]) { size.proj_attack_dist = f[0]; }
                if let Some(v) = sec.read_i32_one("proj.doscale") { size.proj_doscale = v; }
                let (mut hp0, mut hp1) = (0i32, 0i32);
                if sec.read_i32("head.pos", &mut [&mut hp0, &mut hp1]) {
                    size.head_pos = (hp0, hp1);
                }
                let (mut mp0, mut mp1) = (0i32, 0i32);
                if sec.read_i32("mid.pos", &mut [&mut mp0, &mut mp1]) {
                    size.mid_pos = (mp0, mp1);
                }
                if let Some(v) = sec.read_i32_one("shadowoffset") { size.shadowoffset = v; }
                let (mut do0, mut do1) = (0i32, 0i32);
                if sec.read_i32("draw.offset", &mut [&mut do0, &mut do1]) {
                    size.draw_offset = (do0, do1);
                }
            }
            "velocity" => {
                let mut f = [0.0f32; 3];
                if sec.read_f32("walk.fwd", &mut [&mut f[0]]) { velocity.walk_fwd = f[0]; }
                if sec.read_f32("walk.back", &mut [&mut f[0]]) { velocity.walk_back = f[0]; }
                let (mut a0, mut a1) = (0.0f32, 0.0f32);
                if sec.read_f32("run.fwd", &mut [&mut a0, &mut a1]) {
                    velocity.run_fwd = (a0, a1);
                }
                let (mut a0, mut a1) = (0.0f32, 0.0f32);
                if sec.read_f32("run.back", &mut [&mut a0, &mut a1]) {
                    velocity.run_back = (a0, a1);
                }
                let (mut jx, mut jy) = (0.0f32, 0.0f32);
                if sec.read_f32("jump.neu", &mut [&mut jx, &mut jy]) {
                    velocity.jump_neu_x = jx;
                    velocity.jump_y = jy;
                }
                if sec.read_f32("jump.back", &mut [&mut f[0]]) { velocity.jump_back = f[0]; }
                if sec.read_f32("jump.fwd", &mut [&mut f[0]]) { velocity.jump_fwd = f[0]; }
                let (mut a0, mut a1) = (0.0f32, 0.0f32);
                if sec.read_f32("runjump.back", &mut [&mut a0, &mut a1]) {
                    velocity.runjump_back = (a0, a1);
                }
                let (mut a0, mut a1) = (0.0f32, 0.0f32);
                if sec.read_f32("runjump.fwd", &mut [&mut a0, &mut a1]) {
                    velocity.runjump_fwd = (a0, a1);
                }
                let (mut jx, mut jy) = (0.0f32, 0.0f32);
                if sec.read_f32("airjump.neu", &mut [&mut jx, &mut jy]) {
                    velocity.airjump_neu_x = jx;
                    velocity.airjump_y = jy;
                }
                if sec.read_f32("airjump.back", &mut [&mut f[0]]) { velocity.airjump_back = f[0]; }
                if sec.read_f32("airjump.fwd", &mut [&mut f[0]]) { velocity.airjump_fwd = f[0]; }
                let (mut a0, mut a1) = (0.0f32, 0.0f32);
                if sec.read_f32("air.gethit.groundrecover", &mut [&mut a0, &mut a1]) {
                    velocity.air_gethit_ground_recover = (a0, a1);
                }
                let (mut a0, mut a1) = (0.0f32, 0.0f32);
                if sec.read_f32("air.gethit.airrecover.mul", &mut [&mut a0, &mut a1]) {
                    velocity.air_gethit_air_recover_mul = (a0, a1);
                }
                let (mut a0, mut a1) = (0.0f32, 0.0f32);
                if sec.read_f32("air.gethit.airrecover.add", &mut [&mut a0, &mut a1]) {
                    velocity.air_gethit_air_recover_add = (a0, a1);
                }
                if sec.read_f32("air.gethit.airrecover.back", &mut [&mut f[0]]) { velocity.air_gethit_air_recover_back = f[0]; }
                if sec.read_f32("air.gethit.airrecover.fwd", &mut [&mut f[0]]) { velocity.air_gethit_air_recover_fwd = f[0]; }
                if sec.read_f32("air.gethit.airrecover.up", &mut [&mut f[0]]) { velocity.air_gethit_air_recover_up = f[0]; }
                if sec.read_f32("air.gethit.airrecover.down", &mut [&mut f[0]]) { velocity.air_gethit_air_recover_down = f[0]; }
            }
            "movement" => {
                if let Some(v) = sec.read_i32_one("airjump.height") { movement.airjump_height = v; }
                if let Some(f) = sec.get("yaccel") { movement.yaccel = crate::ini::atof(f) as f32; }
                if let Some(f) = sec.get("stand.friction") { movement.stand_friction = crate::ini::atof(f) as f32; }
                if let Some(f) = sec.get("crouch.friction") { movement.crouch_friction = crate::ini::atof(f) as f32; }
                if let Some(f) = sec.get("air.friction") { movement.air_friction = crate::ini::atof(f) as f32; }
                if let Some(f) = sec.get("ground.friction") { movement.ground_friction = crate::ini::atof(f) as f32; }
                if let Some(v) = sec.read_i32_one("fall.defence_up") { movement.fall_defence_up = v; }
                if let Some(f) = sec.get("jump.helddownbonus") { movement.jump_helddown_bonus = crate::ini::atof(f) as f32; }
            }
            "constants" => {
                for (k, v) in sec.iter() {
                    let idx = comp.named.len();
                    let val = crate::ini::atof(v);
                    let has_dot = v.contains('.');
                    let value = if has_dot {
                        crate::value::v_float(val as f32)
                    } else {
                        v_int(val as i32)
                    };
                    comp.named.push((k.to_lowercase(), value));
                    comp.named_map.insert(k.to_lowercase(), idx);
                }
            }
            _ => {}
        }
    }
}

/// Load a text file (bytes) with BOM handling.
pub fn text_of(bytes: &[u8]) -> String {
    load_text(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_ok(text: &str) -> (StateCompiler<'static>, HashMap<i32, StateBytecode>) {
        // leak-free: use a static empty command list for tests
        static CL: std::sync::OnceLock<CommandList> = std::sync::OnceLock::new();
        let cl = CL.get_or_init(CommandList::new);
        let mut sc = StateCompiler::new(cl, 0);
        let mut states = HashMap::new();
        sc.compile_cns_text(text, "test.cns", false, &mut states).unwrap();
        (sc, states)
    }

    #[test]
    fn statedef_and_sctrl_compile() {
        let text = r#"
[Statedef 200]
type = S
physics = S
movetype = A
poweradd = 20
ctrl = 0
anim = 200
velset = 0,0

[State 200, hit]
type = HitDef
trigger1 = AnimElem = 3
attr = S, NA
damage = 23, 0
animtype = Light
guardflag = MA
hitflag = MAF
pausetime = 8, 8
ground.velocity = -4

[State 200, done]
type = ChangeState
trigger1 = AnimTime = 0
value = 0
ctrl = 1
"#;
        let (sc, states) = compile_ok(text);
        assert_eq!(sc.warnings.len(), 0, "warnings: {:?}", sc.warnings);
        let sb = states.get(&200).expect("state 200 compiled");
        assert_eq!(sb.state_type, StateTypeT::S);
        assert_eq!(sb.physics, StateTypeT::S);
        assert_eq!(sb.move_type, MoveTypeT::A);
        assert_eq!(sb.ctrlsps.len(), 2, "persistent slots for 2 sctrls");
        // fast path: both sctrls have triggers → wrapped blocks
        assert_eq!(sb.block.ctrls.len(), 2);
        assert!(matches!(sb.block.ctrls[0], Node::Block(_)));
    }

    #[test]
    fn trigger_semantics_rows() {
        // trigger1 OR trigger2; same-number AND
        let text = r#"
[Statedef 5]
type = S

[State 5, a]
type = Null
trigger1 = 1
trigger2 = 0

[State 5, b]
type = Null
trigger1 = 0
"#;
        let (_, states) = compile_ok(text);
        let sb = states.get(&5).unwrap();
        assert_eq!(sb.block.ctrls.len(), 2);
        if let Node::Block(b) = &sb.block.ctrls[0] {
            let t = b.trigger.as_ref().unwrap();
            assert_eq!(t.rows.len(), 2);
            assert!(matches!(t.rows[0], Some(_)), "const-true row keeps empty vec");
            assert!(matches!(t.rows[1], None), "const-false row = never");
        } else {
            panic!("expected wrapped block");
        }
    }

    #[test]
    fn persistent_and_ignorehitpause() {
        let text = r#"
[Statedef 130]
type = C

[State 130, runonce]
type = Null
trigger1 = 1
persistent = 0

[State 130, ihp]
type = Null
trigger1 = 1
ignorehitpause = 1
"#;
        let (_, states) = compile_ok(text);
        let sb = states.get(&130).unwrap();
        let mut saw_ihp = false;
        let mut saw_once = false;
        for n in &sb.block.ctrls {
            if let Node::Block(b) = n {
                if b.ignorehitpause {
                    saw_ihp = true;
                }
                if b.persistent == i32::MAX {
                    saw_once = true;
                }
            }
        }
        assert!(saw_ihp);
        assert!(saw_once);
    }

    #[test]
    fn negative_state_merge() {
        let cl: &'static CommandList = Box::leak(Box::new(CommandList::new()));
        let mut sc = StateCompiler::new(cl, 0);
        let mut states = HashMap::new();
        let first = "[Statedef -1]\ntype = S\n[State -1, a]\ntype = Null\ntrigger1 = 1\n";
        let second = "[Statedef -1]\ntype = S\n[State -1, b]\ntype = Null\ntrigger1 = 1\n";
        sc.compile_cns_text(first, "a.cns", false, &mut states).unwrap();
        sc.compile_cns_text(second, "b.cns", false, &mut states).unwrap();
        let sb = states.get(&-1).unwrap();
        assert_eq!(sb.block.ctrls.len(), 2, "negative states merge sctrls");
    }

    #[test]
    fn statedef_plus1_is_minus10() {
        let (_, s) = compile_ok("[Statedef +1]\ntype = S");
        assert!(s.contains_key(&-10), "[Statedef +1] must alias -10");
    }
}
