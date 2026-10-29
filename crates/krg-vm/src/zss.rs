//! Port origin: ikemen-src/src/compiler.go — ZSS path: `stateCompileZSS`
//! (8395-8552), `readSentence`/`scan` (7561-7651), `stateBlock` grammar
//! (8246-8393), `[Function]` (8480-8549).
//!
//! ZSS (Zantei State Script) is Ikemen's own language; the v1.0.0 data set
//! ships `common1.cns.zss` as the common states, so this compiler is
//! required for KFM to *do* anything (stcommon = common1.cns → falls back
//! to the .zss via compiler.go:7181-7195).
//!
//! Grammar implemented (corpus-verified against data/*.zss):
//! * `[StateDef N; key: value; …]` — `;`-separated params inside brackets
//! * `[Function name(args) rets]` — first-loaded definition wins (8497)
//! * statements: `if/else if/else`, `switch…case/default`, `for`, `while`,
//!   `break;`, `continue;`, `let a[, b] = e1[, e2];`, `call f(args);`,
//!   `var/fvar/sysvar/sysfvar/map(…) := expr;`, `sctrlName{ key: value; … }`
//! * block attrs: `ignoreHitPause` and `persistent(n)` prefix the next
//!   block or if-chain (7795-7848)
//! * `#` comments (never `;` — that's the statement separator)
//! * locals: `[a-z_][0-9a-z_]*`, referenced as `$name`, ≤256 per state
//!
//! Compile products are the same [`StateBytecode`] trees the CNS path
//! emits, so both share the runtime.

use crate::cns::StateCompiler;
use crate::exp::CompileCtx as _;
use crate::ini::split_and_trim;
use crate::op::BytecodeExp;
use crate::sctrl::{compile_sctrl, Sctrl};
use crate::statedef::{Node, StateBlock, StateBytecode, Triggers};
use crate::value::ValueType;
use std::collections::HashMap;

/// bytecode.go:4993 `BytecodeFunction`
#[derive(Debug, Clone)]
pub struct FuncBytecode {
    pub num_vars: i32,
    pub num_args: usize,
    pub num_rets: usize,
    /// persistent-counter slots the body's gated blocks need (the parser's
    /// ordinal counter) — without them every `if`-block in a function body
    /// fails the ps bounds check and silently skips.
    pub num_ctrlsps: usize,
    pub body: Vec<Node>,
}

// ── sentence scanner ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sk {
    Text,
    Colon,
    Semi,
    LBrace,
    RBrace,
    Eof,
}

/// readSentenceLine (compiler.go:7598) — accumulate chars until a closure
/// token; `#` starts a comment outside quotes; `:=` stays inside text.
struct ZssReader {
    buf: Vec<char>,
    pos: usize,
}

impl ZssReader {
    fn new(text: &str) -> Self {
        // NOTE: '#' comments were already stripped line-wise by the caller
        ZssReader { buf: text.chars().collect(), pos: 0 }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.buf.len() && self.buf[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.buf.get(self.pos).copied()
    }

    /// Next sentence: (kind, text) — text empty for brace/semi/colon kinds.
    fn next(&mut self) -> (Sk, String) {
        self.skip_ws();
        let Some(c) = self.peek() else { return (Sk::Eof, String::new()) };
        match c {
            '{' => {
                self.pos += 1;
                (Sk::LBrace, String::new())
            }
            '}' => {
                self.pos += 1;
                (Sk::RBrace, String::new())
            }
            ';' => {
                self.pos += 1;
                (Sk::Semi, String::new())
            }
            ':' => {
                // ':=' is part of text; lone ':' terminates
                if self.buf.get(self.pos + 1) == Some(&'=') {
                    // fallthrough to text accumulation
                } else {
                    self.pos += 1;
                    return (Sk::Colon, String::new());
                }
                self.accumulate()
            }
            _ => self.accumulate(),
        }
    }

    fn accumulate(&mut self) -> (Sk, String) {
        let mut out = String::new();
        let mut in_quote = false;
        while let Some(c) = self.peek() {
            if in_quote {
                out.push(c);
                if c == '"' {
                    in_quote = false;
                }
                self.pos += 1;
                continue;
            }
            match c {
                '"' => {
                    in_quote = true;
                    out.push(c);
                    self.pos += 1;
                }
                '{' => return (Sk::Text, out),
                '}' => return (Sk::Text, out),
                ';' => return (Sk::Text, out),
                ':' => {
                    if self.buf.get(self.pos + 1) == Some(&'=') {
                        out.push(c);
                        out.push('=');
                        self.pos += 2;
                    } else {
                        return (Sk::Text, out);
                    }
                }
                _ => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
        (Sk::Text, out)
    }
}

// ── compile entry ────────────────────────────────────────────────────────────

struct ZssParser<'a, 'sc> {
    r: ZssReader,
    sc: &'a mut StateCompiler<'sc>,
    state_no: i32,
    persistent_next: Option<i32>,
    ihp_next: bool,
    ordinal: i32,
}

impl<'a, 'sc> ZssParser<'a, 'sc> {
    fn first_word(text: &str) -> (&str, &str) {
        let t = text.trim_start();
        let end = t
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(t.len());
        (&t[..end], t[end..].trim_start())
    }

    fn add_local(sc: &mut StateCompiler, name: &str) -> Option<u8> {
        // ZSS local names are [a-z_][0-9a-z_]* — the tokenizer lowercases
        // `$refs`, so store lowercased for lookups to match.
        let name = name.trim().to_lowercase();
        if name == "_" {
            return None; // discard
        }
        if let Some(slot) = sc.local_slot(&name) {
            return Some(slot);
        }
        if sc.locals.len() >= 256 {
            return None;
        }
        sc.locals.push(name);
        Some((sc.locals.len() - 1) as u8)
    }

    /// Compile an expression fragment (shared entry; keyword-insensitive).
    fn expr(&mut self, text: &str, vt: ValueType) -> Result<BytecodeExp, String> {
        let mut comp = crate::exp::Compiler::new(self.sc as &mut dyn crate::exp::CompileCtx);
        let mut d = text.trim().to_string();
        let mut dr = d.as_str();
        let be = comp
            .full_expression(&mut dr, vt)
            .map_err(|e| format!("in ZSS expr '{text}': {e}"))?;
        Ok(be)
    }

    /// Parse a `{ … }` body into nodes.
    fn parse_brace_body(&mut self) -> Result<Vec<Node>, String> {
        let (k, t) = self.r.next();
        match k {
            Sk::LBrace => {}
            Sk::Text if t.trim().is_empty() => return self.parse_brace_body(),
            Sk::Eof => return Ok(Vec::new()),
            _ => return Err(format!("expected '{{' in state {}", self.state_no)),
        }
        self.parse_nodes(true)
    }

    /// Parse statements until matching `}` (brace=true) or EOF.
    fn parse_nodes(&mut self, brace: bool) -> Result<Vec<Node>, String> {
        let mut nodes: Vec<Node> = Vec::new();
        loop {
            let (k, text) = self.r.next();
            match k {
                Sk::Eof => {
                    if brace {
                        return Err(format!("missing '}}' in state {}", self.state_no));
                    }
                    return Ok(nodes);
                }
                Sk::RBrace => {
                    if brace {
                        return Ok(nodes);
                    }
                    return Err(format!("unexpected '}}' in state {}", self.state_no));
                }
                Sk::Semi | Sk::Colon => continue,
                Sk::LBrace => {
                    // bare block with pending attrs
                    let body = self.parse_nodes(true)?;
                    let mut blk = StateBlock::default();
                    blk.persistent_index = self.claim_persistent();
                    blk.persistent = self.persistent_next.take().unwrap_or(1);
                    if self.ihp_next {
                        blk.ignorehitpause = true;
                        blk.ctrls_ignorehitpause = true;
                        self.ihp_next = false;
                    }
                    blk.ctrls = body;
                    nodes.push(Node::Block(Box::new(blk)));
                }
                Sk::Text => {
                    let mut t = text.trim().to_string();
                    if t.is_empty() {
                        continue;
                    }
                    // ── strip leading block attributes ────────────────────
                    // `persistent(n)` / `ignorehitpause` may PREFIX a whole
                    // statement on the same sentence
                    // (`persistent(0) if … { … }`). They must set the
                    // pending flag and leave the REST of the sentence for
                    // normal dispatch — dropping it (the old bug) deleted
                    // the entire `if` condition, leaving an unconditional
                    // ghost block (the state-50 flicker).
                    loop {
                        let (word, rest) = Self::first_word(&t);
                        let wl = word.to_lowercase();
                        if wl != "persistent" && wl != "ignorehitpause" {
                            break;
                        }
                        // attribute argument: first '(…)' after the keyword
                        let (arg, after) = match rest.find('(') {
                            Some(p) => match rest[p..].find(')') {
                                Some(e) => (&rest[p + 1..p + e], rest[p + e + 1..].trim()),
                                None => ("", ""),
                            },
                            None => ("", rest),
                        };
                        if wl == "persistent" {
                            let n = crate::ini::atoi(arg.trim());
                            // 0 = run once per state entry (i32::MAX latch)
                            self.persistent_next =
                                Some(if n <= 0 { i32::MAX } else { n.min(128) });
                        } else {
                            self.ihp_next = true;
                        }
                        t = after.to_string();
                        if t.is_empty() {
                            break;
                        }
                    }
                    if t.is_empty() {
                        continue; // attribute alone — applies to the next `{`
                    }
                    let (word, rest) = Self::first_word(&t);
                    let wl = word.to_lowercase();
                    match wl.as_str() {
                        "if" => {
                            let node = self.parse_if(rest)?;
                            nodes.push(node);
                        }
                        "else" => {
                            return Err(format!(
                                "'else' without 'if' in state {}",
                                self.state_no
                            ));
                        }
                        "switch" => {
                            let node = self.parse_switch(rest)?;
                            nodes.push(node);
                        }
                        "for" => {
                            let node = self.parse_for(rest)?;
                            nodes.push(node);
                        }
                        "while" => {
                            let cond = self.expr(rest, ValueType::Bool)?;
                            let body = self.parse_brace_body()?;
                            nodes.push(Node::Loop {
                                cond: Some(cond),
                                init: None,
                                incr: None,
                                body,
                            });
                        }
                        "break" => {
                            self.expect_semi();
                            nodes.push(Node::Break);
                        }
                        "continue" => {
                            self.expect_semi();
                            nodes.push(Node::Continue);
                        }
                        "let" => {
                            let node = self.parse_let(rest)?;
                            nodes.push(node);
                        }
                        "call" => {
                            let node = self.parse_call(rest)?;
                            nodes.push(node);
                        }
                        // ("persistent" / "ignorehitpause" never reach here:
                        // leading attributes are stripped before dispatch.)
                        _ => {
                            // sctrl block or expression statement
                            let (k2, t2) = self.r.next();
                            match k2 {
                                Sk::LBrace => {
                                    let node = self.parse_sctrl(&wl, word)?;
                                    nodes.push(node);
                                }
                                Sk::Semi => {
                                    // expression statement — must contain ':='
                                    if !t.contains(":=") {
                                        return Err(format!(
                                            "Expression with unused value: {t} (state {})",
                                            self.state_no
                                        ));
                                    }
                                    let be = self.expr(&t, ValueType::Undefined)?;
                                    nodes.push(Node::ExprStmt(be));
                                }
                                _ => {
                                    return Err(format!(
                                        "Unexpected statement: {t} (state {})",
                                        self.state_no
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn expect_semi(&mut self) {
        let (k, _) = self.r.next();
        if k == Sk::Eof {
            // tolerate EOF
        }
    }

    fn claim_persistent(&mut self) -> i32 {
        let idx = self.ordinal;
        self.ordinal += 1;
        idx
    }

    fn parse_if(&mut self, cond_text: &str) -> Result<Node, String> {
        let cond = self.expr(cond_text, ValueType::Bool)?;
        let body = self.parse_brace_body()?;
        let mut blk = StateBlock::default();
        blk.trigger = Some(Triggers {
            triggerall: vec![cond],
            rows: vec![Some(Vec::new())], // one always-true row
            any_possible: true,
        });
        blk.persistent_index = self.claim_persistent();
        blk.persistent = self.persistent_next.take().unwrap_or(1);
        if self.ihp_next {
            blk.ignorehitpause = true;
            blk.ctrls_ignorehitpause = true;
            self.ihp_next = false;
        }
        blk.ctrls = body;

        // else-if / else chain: peek next sentence
        let mut root = blk;
        let mut chain_tail: Option<&mut StateBlock> = None;
        let _ = &mut chain_tail;
        loop {
            // peek: we must not consume non-else sentences
            let save = (self.r.pos, ());
            let (k, text) = self.r.next();
            if k == Sk::Text && !text.trim().is_empty() {
                let (w, rest) = Self::first_word(text.trim());
                if w.eq_ignore_ascii_case("else") {
                    let r = rest.trim();
                    if r.to_lowercase().starts_with("if") {
                        let cond2 =
                            self.expr(&r[2..].trim(), ValueType::Bool)?;
                        let body2 = self.parse_brace_body()?;
                        let mut blk2 = StateBlock::default();
                        blk2.trigger = Some(Triggers {
                            triggerall: vec![cond2],
                            rows: vec![Some(Vec::new())],
                            any_possible: true,
                        });
                        blk2.persistent_index = self.claim_persistent();
                        blk2.ctrls = body2;
                        let boxed = Node::Block(Box::new(blk2));
                        attach_else(&mut root, boxed);
                        continue;
                    } else {
                        // plain else
                        let body2 = self.parse_brace_body()?;
                        let mut blk2 = StateBlock::default();
                        blk2.ctrls = body2;
                        let boxed = Node::Block(Box::new(blk2));
                        attach_else(&mut root, boxed);
                        break;
                    }
                }
            }
            // not else → restore position
            self.r.pos = save.0;
            let _ = k;
            break;
        }
        Ok(Node::Block(Box::new(root)))
    }

    fn parse_switch(&mut self, hdr_text: &str) -> Result<Node, String> {
        // switch hdr { case v; v2: ... default: ... }
        let hdr = self.expr(hdr_text, ValueType::Undefined)?;
        let (k, _) = self.r.next();
        if k != Sk::LBrace {
            return Err("switch requires '{'".into());
        }
        // collect case groups: Vec<(Vec<BytecodeExp>, Vec<Node>)> + default
        let mut groups: Vec<(Vec<BytecodeExp>, Vec<Node>)> = Vec::new();
        let mut default: Option<Vec<Node>> = None;
        loop {
            let (k, text) = self.r.next();
            match k {
                Sk::Eof => break,
                Sk::RBrace => break,
                Sk::Text => {
                    let t = text.trim().to_lowercase();
                    if t == "case" {
                        // values until ':' (each ends with ';')
                        let mut vals: Vec<BytecodeExp> = Vec::new();
                        loop {
                            let (k2, vtext) = self.r.next();
                            match k2 {
                                Sk::Semi => {}
                                Sk::Colon => break,
                                Sk::Text => {
                                    if !vtext.trim().is_empty() {
                                        vals.push(self.expr(&vtext, ValueType::Undefined)?);
                                    }
                                    // wait for : or ;
                                }
                                _ => return Err("bad case syntax".into()),
                            }
                            if k2 == Sk::Eof {
                                return Err("case missing ':'".into());
                            }
                        }
                        // body until next case/default/}
                        let mut body: Vec<Node> = Vec::new();
                        loop {
                            let save = self.r.pos;
                            let (k3, t3) = self.r.next();
                            match k3 {
                                Sk::RBrace => {
                                    self.r.pos = save;
                                    break;
                                }
                                Sk::Eof => break,
                                Sk::Text => {
                                    let (w, _) = Self::first_word(t3.trim());
                                    if w.eq_ignore_ascii_case("case")
                                        || w.eq_ignore_ascii_case("default")
                                    {
                                        self.r.pos = save;
                                        break;
                                    }
                                    // statement text — reprocess through a
                                    // one-shot loop by rewinding and reusing
                                    self.r.pos = save;
                                    let more = self.parse_stmt_until_stop()?;
                                    body.push(more);
                                }
                                _ => continue,
                            }
                        }
                        groups.push((vals, body));
                    } else if t == "default" {
                        let (k4, _) = self.r.next();
                        if k4 != Sk::Colon {
                            return Err("default missing ':'".into());
                        }
                        let mut body: Vec<Node> = Vec::new();
                        loop {
                            let save = self.r.pos;
                            let (k5, t5) = self.r.next();
                            match k5 {
                                Sk::RBrace => {
                                    self.r.pos = save;
                                    break;
                                }
                                Sk::Eof => break,
                                Sk::Text => {
                                    let (w, _) = Self::first_word(t5.trim());
                                    if w.eq_ignore_ascii_case("case")
                                        || w.eq_ignore_ascii_case("default")
                                    {
                                        self.r.pos = save;
                                        break;
                                    }
                                    self.r.pos = save;
                                    let more = self.parse_stmt_until_stop()?;
                                    body.push(more);
                                }
                                _ => continue,
                            }
                        }
                        default = Some(body);
                    }
                }
                _ => continue,
            }
        }
        // desugar: nested if/else-if chain comparing hdr = v (7929-8028)
        let mut chain: Option<StateBlock> = None;
        for (vals, body) in groups {
            let mut rows: Vec<Option<Vec<BytecodeExp>>> = Vec::new();
            for v in vals {
                let mut cmp: BytecodeExp = Vec::new();
                cmp.extend(hdr.iter().cloned());
                cmp.extend(v.iter().cloned());
                cmp.push(crate::op::Op::Eq);
                rows.push(Some(vec![cmp]));
            }
            if rows.is_empty() {
                rows.push(Some(Vec::new())); // case with no values → always
            }
            let mut blk = StateBlock::default();
            blk.trigger = Some(Triggers {
                triggerall: Vec::new(),
                rows,
                any_possible: true,
            });
            blk.ctrls = body;
            if let Some(prev) = chain.take() {
                blk.else_block = Some(Box::new(prev));
            }
            chain = Some(blk);
        }
        if let Some(db) = default {
            let mut dblk = StateBlock::default();
            dblk.ctrls = db;
            if let Some(prev) = chain.take() {
                dblk.else_block = Some(Box::new(prev));
            }
            chain = Some(dblk);
        }
        match chain {
            Some(root) => Ok(Node::Block(Box::new(root))),
            None => Ok(Node::Block(Box::new(StateBlock::default()))),
        }
    }

    fn parse_for(&mut self, hdr: &str) -> Result<Node, String> {
        // for [i =] begin; end; incr { … }
        let h = hdr.trim();
        // for [i =] begin; end; incr { … }  — segments ';'-separated
        let mut segs = h.split(';').map(|x| x.trim().to_string());
        let seg0 = segs.next().unwrap_or_default();
        let seg1 = segs.next().unwrap_or_default();
        let (init, cond_text) = match seg0.find('=') {
            Some(eq) => {
                let var = seg0[..eq].trim().to_string();
                let begin = seg0[eq + 1..].trim().to_string();
                let slot = Self::add_local(self.sc, &var.to_lowercase());
                let init_exp = self.expr(&begin, ValueType::Int)?;
                (slot.map(|s| (s, init_exp)), seg1)
            }
            None => (None, seg1),
        };
        let cond = if cond_text.is_empty() {
            None
        } else {
            Some(self.expr(&cond_text, ValueType::Bool)?)
        };
        let body = self.parse_brace_body()?;
        Ok(Node::Loop { cond, init: init.map(|(s, e)| (s, Box::new(e))), incr: None, body })
    }

    fn parse_let(&mut self, rest: &str) -> Result<Node, String> {
        // let a, b = e1, e2;
        let mut rest = rest.trim().to_string();
        // strip trailing ';' (may have already been consumed as its own Semi)
        if rest.ends_with(';') {
            rest.pop();
        }
        let Some(eq) = rest.find('=') else {
            return Err(format!("let without '=': {rest}"));
        };
        let names_part = rest[..eq].to_string();
        let exprs_part = rest[eq + 1..].trim().to_string();
        // split at TOP-LEVEL commas only (parens aware) — `let x = cond(a, b)`
        let names: Vec<String> = split_top_level(&names_part)
            .into_iter()
            .map(|s| s.trim().to_string())
            .collect();
        let exprs: Vec<String> = split_top_level(&exprs_part)
            .into_iter()
            .map(|s| s.trim().to_string())
            .collect();
        // single node carrying the LAST let (others applied before via
        // nested Blocks — for v1 corpus uses single-let almost always)
        let mut node_out: Option<Node> = None;
        for (n, e) in names.iter().zip(exprs.iter()) {
            let slot = Self::add_local(self.sc, n);
            let e_trim = e.trim();
            // `let x = call f(args);` — call with return slots
            if e_trim.to_lowercase().starts_with("call ") {
                let mut call_node = self.parse_call(&e_trim[5..])?;
                if let Node::Call { ret, .. } = &mut call_node {
                    if let Some(slot) = slot {
                        ret.push(slot);
                    }
                }
                node_out = Some(match (node_out, call_node) {
                    (None, c) => c,
                    (Some(prev), c) => Node::Block(Box::new(StateBlock {
                        ctrls: vec![prev, c],
                        ..Default::default()
                    })),
                });
                continue;
            }
            let exp = self.expr(e, ValueType::Undefined)?;
            if let Some(slot) = slot {
                let node = Node::Let { slot, exp };
                node_out = Some(match node_out {
                    None => node,
                    Some(prev) => Node::Block(Box::new(StateBlock {
                        ctrls: vec![prev, node],
                        ..Default::default()
                    })),
                });
            }
        }
        Ok(node_out.unwrap_or(Node::Block(Box::new(StateBlock::default()))))
    }

    fn parse_call(&mut self, rest: &str) -> Result<Node, String> {
        // call f(a, b) [ret1, ret2];
        let mut rest = rest.trim().to_string();
        if rest.ends_with(';') {
            rest.pop();
        }
        let (name, args_part) = match rest.find('(') {
            Some(i) => {
                let a = &rest[i + 1..];
                // strip only the call's own final ')'
                let a = a.strip_suffix(')').unwrap_or(a);
                (rest[..i].trim().to_string(), a.to_string())
            }
            None => (rest.clone(), String::new()),
        };
        let ret_part = "";
        let _ = ret_part;
        let mut args: Vec<BytecodeExp> = Vec::new();
        if !args_part.trim().is_empty() {
            for a in split_top_level(&args_part) {
                if a.trim().is_empty() {
                    continue;
                }
                args.push(self.expr(a.trim(), ValueType::Undefined)?);
            }
        }
        let nidx = {
            let comp_ctx = self.sc as &mut dyn crate::exp::CompileCtx;
            comp_ctx.add_string(&name)
        };
        // rets: none in the corpus call sites
        Ok(Node::Call { name_idx: nidx, args, ret: Vec::new() })
    }

    fn parse_sctrl(&mut self, name_lc: &str, orig: &str) -> Result<Node, String> {
        let Some(kind) = crate::sctrl::kind_from_name(name_lc) else {
            // ZSS is strict, but keep lenient to let more content load
            return Err(format!("Invalid state controller: {orig}"));
        };
        // params until '}': key: value;  …
        let mut sec = crate::ini::IniSection::new();
        loop {
            let (k, text) = self.r.next();
            match k {
                Sk::RBrace | Sk::Eof => break,
                Sk::Semi | Sk::Colon => continue,
                Sk::LBrace => return Err("unexpected '{' in sctrl params".into()),
                Sk::Text => {
                    let key = text.trim().to_lowercase();
                    if key.is_empty() {
                        continue;
                    }
                    // value sentences until ';'
                    let mut val = String::new();
                    let mut terminated = false;
                    loop {
                        let (k2, t2) = self.r.next();
                        match k2 {
                            Sk::Semi => {
                                terminated = true;
                                break;
                            }
                            Sk::Colon => {
                                // key: value separator consumed; value next
                            }
                            Sk::RBrace | Sk::Eof => {
                                self.r.pos -= if k2 == Sk::RBrace { 1 } else { 0 };
                                terminated = true;
                                break;
                            }
                            Sk::LBrace => return Err("nested brace in sctrl value".into()),
                            Sk::Text => {
                                if !t2.trim().is_empty() {
                                    if !val.is_empty() {
                                        val.push(' ');
                                    }
                                    val.push_str(t2.trim());
                                }
                            }
                        }
                    }
                    let _ = terminated;
                    if !sec.contains(&key) {
                        sec.insert(&key, val);
                    }
                }
            }
        }
        let mut collected: Vec<String> = Vec::new();
        let sctrl: Sctrl = {
            let mut comp = crate::exp::Compiler::new(self.sc as &mut dyn crate::exp::CompileCtx);
            compile_sctrl(kind, &mut sec, &mut comp, &mut |m: String| {
                collected.push(m)
            })?
        };
        self.sc.warnings.extend(collected.drain(..));
        // persistent slot for ZSS sctrls (persistent(0) ⇒ run-once)
        let pi = self.claim_persistent();
        let mut blk = StateBlock::default();
        blk.persistent_index = pi;
        blk.persistent = self.persistent_next.take().unwrap_or(1);
        if self.ihp_next {
            blk.ignorehitpause = true;
            blk.ctrls_ignorehitpause = true;
            self.ihp_next = false;
        }
        if sctrl.ignorehitpause {
            blk.ignorehitpause = true;
            blk.ctrls_ignorehitpause = true;
        }
        blk.ctrls = vec![Node::Sctrl(sctrl)];
        Ok(Node::Block(Box::new(blk)))
    }

    fn parse_stmt_until_stop(&mut self) -> Result<Node, String> {
        // parse ONE statement (used inside switch-case bodies)
        let mut nodes = self.parse_one_stmt()?;
        if nodes.len() == 1 {
            Ok(nodes.remove(0))
        } else {
            let mut blk = StateBlock::default();
            blk.ctrls = nodes;
            Ok(Node::Block(Box::new(blk)))
        }
    }

    fn parse_one_stmt(&mut self) -> Result<Vec<Node>, String> {
        // like parse_nodes but stops after one Text statement group
        let mut nodes = Vec::new();
        loop {
            let (k, text) = self.r.next();
            match k {
                Sk::Semi | Sk::Colon => continue,
                Sk::Text if text.trim().is_empty() => continue,
                Sk::Eof | Sk::RBrace | Sk::LBrace => return Ok(nodes),
                Sk::Text => {
                    let t = text.trim().to_string();
                    let (word, rest) = Self::first_word(&t);
                    let wl = word.to_lowercase();
                    match wl.as_str() {
                        "break" => {
                            self.expect_semi();
                            nodes.push(Node::Break);
                        }
                        "continue" => {
                            self.expect_semi();
                            nodes.push(Node::Continue);
                        }
                        _ => {
                            let (k2, _) = self.r.next();
                            if k2 == Sk::LBrace {
                                let node = self.parse_sctrl(&wl, word)?;
                                nodes.push(node);
                            } else if k2 == Sk::Semi {
                                let be = self.expr(&t, ValueType::Undefined)?;
                                nodes.push(Node::ExprStmt(be));
                            }
                        }
                    }
                    return Ok(nodes);
                }
            }
        }
    }
}

fn extract_paren(s: &str) -> Option<&str> {
    let start = s.find('(')?;
    let end = s.rfind(')')?;
    Some(&s[start + 1..end])
}

fn attach_else(root: &mut StateBlock, else_node: Node) {
    // walk to the deepest else_block and attach
    if let Some(ref mut eb) = root.else_block {
        let mut b: &mut StateBlock = eb.as_mut();
        loop {
            if let Some(ref mut e2) = b.else_block {
                b = e2.as_mut();
            } else {
                b.else_block = Some(Box::new(match else_node {
                    Node::Block(b) => *b,
                    other => {
                        let mut sb = StateBlock::default();
                        sb.ctrls = vec![other];
                        sb
                    }
                }));
                return;
            }
        }
    } else {
        root.else_block = Some(Box::new(match else_node {
            Node::Block(b) => *b,
            other => {
                let mut sb = StateBlock::default();
                sb.ctrls = vec![other];
                sb
            }
        }));
    }
}


/// Split "a, b" at top-level commas (respecting parens/quotes).
fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_quote = false;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            '(' | '[' if !in_quote => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' if !in_quote => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 && !in_quote => {
                out.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// stateCompileZSS (compiler.go:8395) — one .zss text → states + funcs.
pub fn compile_zss_text(
    sc: &mut StateCompiler,
    text: &str,
    filename: &str,
    negoverride: bool,
    states: &mut HashMap<i32, StateBytecode>,
) -> Result<(), String> {
    // strip '#' comments outside quotes, line-based (then join)
    let lines: Vec<String> = split_and_trim(text, "\n")
        .into_iter()
        .map(|l| strip_hash(&l))
        .collect();
    let mut i = 0usize;
    while i < lines.len() {
        let l = lines[i].trim().to_lowercase();
        i += 1;
        if l.starts_with('[') {
            // section header — find the closing ']'
            if let Some(close) = l.find(']') {
                let header = l[1..close].trim().to_string();
                let rest_tail = l[close + 1..].to_string(); // (usually empty)
                if header.starts_with("statedef") {
                    // [StateDef N; key: value; …]
                    let mut segs = header[8..].split(';');
                    let num_txt = segs.next().unwrap_or("").trim();
                    let state_no = parse_statedef_no(sc, num_txt)?;
                    let mut sbc = StateBytecode::new(sc.player_no);
                    if state_no < 0 {
                        if let Some(old) = states.get(&state_no) {
                            sbc = old.clone();
                        }
                    }
                    // params inside the header
                    let mut is = crate::ini::IniSection::new();
                    for seg in segs {
                        let seg = seg.trim();
                        if seg.is_empty() {
                            continue;
                        }
                        if let Some(c) = seg.find(':') {
                            let (k, v) = (seg[..c].trim().to_lowercase(), seg[c + 1..].trim());
                            if !k.is_empty() && !is.contains(&k) {
                                is.insert(&k, v);
                            }
                        }
                    }
                    compile_zss_statedef(sc, &mut is, &mut sbc, state_no)?;
                    let _ = rest_tail;
                    // body statements until next '[' line
                    let mut body_lines: Vec<String> = Vec::new();
                    while i < lines.len() {
                        let t = lines[i].trim();
                        if t.starts_with('[') {
                            break;
                        }
                        body_lines.push(lines[i].clone());
                        i += 1;
                    }
                    let mut p = ZssParser {
                        r: ZssReader::new(&body_lines.join("\n")),
                        sc,
                        state_no,
                        persistent_next: None,
                        ihp_next: false,
                        ordinal: sbc.block.ctrls.len() as i32,
                    };
                    let nodes = p.parse_nodes(false)?;
                    let sbc = &mut states.entry(state_no).or_insert(sbc);
                    // sbc already contains merged ctrls (clone started old)
                    // append this file's nodes
                    for n in nodes {
                        sbc.block.ctrls.push(n);
                    }
                    sbc.ctrlsps.resize(p.ordinal.max(0) as usize, 0);
                    // insert/merge rules mirror cns (7538-7540)
                    // (entry().or_insert keeps first-wins for positives; for
                    // negatives with negoverride we must REPLACE not merge:)
                    continue;
                } else if header.starts_with("function") {
                    // [Function name(args) rets]
                    let decl = header[8..].trim().to_string();
                    let (fname, args_part, rets_part) = parse_func_decl(&decl);
                    let mut arg_names: Vec<String> = Vec::new();
                    for a in split_top_level(&args_part) {
                        let a = a.trim().to_lowercase();
                        if !a.is_empty() {
                            arg_names.push(a);
                        }
                    }
                    let mut ret_names: Vec<String> = Vec::new();
                    if let Some(rp) = rets_part {
                        for r in split_top_level(&rp) {
                            let r = r.trim().to_lowercase();
                            if !r.is_empty() {
                                ret_names.push(r);
                            }
                        }
                    }
                    // locals reset per function (8506)
                    let saved_locals = std::mem::take(&mut sc.locals);
                    for a in &arg_names {
                        ZssParser::add_local(sc, a);
                    }
                    for r in &ret_names {
                        ZssParser::add_local(sc, r);
                    }
                    let mut body_lines: Vec<String> = Vec::new();
                    while i < lines.len() {
                        let t = lines[i].trim();
                        if t.starts_with('[') {
                            break;
                        }
                        body_lines.push(lines[i].clone());
                        i += 1;
                    }
                    let mut p = ZssParser {
                        r: ZssReader::new(&body_lines.join("\n")),
                        sc,
                        state_no: 0,
                        persistent_next: None,
                        ihp_next: false,
                        ordinal: 0,
                    };
                    let body = p.parse_nodes(false)?;
                    let num_vars = p.sc.locals.len() as i32;
                    let fb = FuncBytecode {
                        num_vars,
                        num_args: arg_names.len(),
                        num_rets: ret_names.len(),
                        num_ctrlsps: p.ordinal.max(0) as usize,
                        body,
                    };
                    // first-loaded wins (8497); key is LOWERCASED — section
                    // headers pass through to_lowercase() above, and call
                    // sites normalize the same way (zss_call)
                    let fname = fname.to_lowercase();
                    if !sc.funcs.contains_key(&fname) {
                        sc.funcs.insert(fname.clone(), fb);
                        sc.func_order.push(fname.clone());
                    }
                    sc.locals = saved_locals;
                    continue;
                }
                // unknown section — skip its body
                while i < lines.len() && !lines[i].trim().to_lowercase().starts_with('[') {
                    i += 1;
                }
            }
        }
    }
    let _ = negoverride;
    let _ = filename;
    Ok(())
}

fn strip_hash(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_quote = false;
    for c in line.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                out.push(c);
            }
            '#' if !in_quote => break,
            _ => out.push(c),
        }
    }
    out
}

fn parse_statedef_no(sc: &mut StateCompiler, num_txt: &str) -> Result<i32, String> {
    let t = num_txt.trim().to_lowercase();
    if t == "+1" {
        return Ok(-10);
    }
    if let Some(rest) = t.strip_prefix("const(") {
        let name = rest.trim_end_matches(')').trim().to_lowercase();
        if let Some(&idx) = sc.named_map.get(&name) {
            if let Some((_, v)) = sc.named.get(idx) {
                return Ok(v.to_i());
            }
        }
        return Ok(0);
    }
    if t.starts_with('-') {
        return Ok(-crate::ini::atoi(&t[1..]));
    }
    Ok(crate::ini::atoi(&t))
}

fn parse_func_decl(decl: &str) -> (String, String, Option<String>) {
    // "name(arg1, arg2) ret1, ret2" — rets after the closing paren
    let open = decl.find('(').unwrap_or(decl.len());
    let name = decl[..open].trim().to_string();
    let after = &decl[open..];
    let close = after.find(')').map(|i| i + open).unwrap_or(decl.len());
    let args = decl[open + 1..close.min(decl.len())].to_string();
    if close + 1 < decl.len() {
        (name, args, Some(decl[close + 1..].to_string()))
    } else {
        (name, args, None)
    }
}

fn compile_zss_statedef(
    sc: &mut StateCompiler,
    is: &mut crate::ini::IniSection,
    sbc: &mut StateBytecode,
    state_no: i32,
) -> Result<(), String> {
    if state_no < 0 {
        return Ok(());
    }
    // type/movetype/physics: enum letters or string expression
    let mut comp = crate::exp::Compiler::new(sc as &mut dyn crate::exp::CompileCtx);
    for key in ["type", "movetype", "physics"] {
        let Some(v) = is.take(key) else { continue };
        let data = v.trim();
        if data.is_empty() {
            continue;
        }
        let ok = match key {
            "type" => data.chars().all(|c| "SCALUscalu".contains(c)),
            "movetype" => data.chars().all(|c| "IAHUiahu".contains(c)),
            _ => data.chars().all(|c| "SCANUscanu".contains(c)),
        };
        if ok {
            match key {
                "type" => sbc.state_type = crate::statedef::StateTypeT::from_str(data).unwrap_or_default(),
                "movetype" => sbc.move_type = crate::statedef::MoveTypeT::from_str(data).unwrap_or_default(),
                _ => sbc.physics = crate::statedef::StateTypeT::from_str(data).unwrap_or_default(),
            }
        } else {
            let mut d = data.to_string();
            let mut dr = d.as_str();
            let exp = comp.full_expression(&mut dr, ValueType::String)?;
            match key {
                "type" => sbc.state_type_exp = Some(exp),
                "movetype" => sbc.move_type_exp = Some(exp),
                _ => sbc.physics_exp = Some(exp),
            }
        }
    }
    // remaining params compiled like the CNS Statedef pseudo-sctrl
    let mut sec = crate::ini::IniSection::new();
    for (k, v) in is.iter() {
        sec.insert(k, v.clone());
    }
    let mut sdef = Sctrl {
        kind: crate::sctrl::SctrlKind::Null,
        ignorehitpause: true,
        params: Vec::new(),
    };
    const SD_SPRPRIORITY: u16 = 4;
    const SD_FACEP2: u16 = 5;
    const SD_JUGGLE: u16 = 6;
    const SD_VELSET: u16 = 7;
    const SD_ANIM: u16 = 10;
    const SD_CTRL: u16 = 11;
    const SD_POWERADD: u16 = 12;
    for (key, vt, nargs, pid) in [
        ("sprpriority", ValueType::Int, 1, SD_SPRPRIORITY),
        ("facep2", ValueType::Bool, 1, SD_FACEP2),
        ("juggle", ValueType::Int, 1, SD_JUGGLE),
        ("velset", ValueType::Float, 3, SD_VELSET),
        ("ctrl", ValueType::Bool, 1, SD_CTRL),
        ("poweradd", ValueType::Int, 1, SD_POWERADD),
    ] {
        let Some(v) = is.take(key) else { continue };
        if v.trim().is_empty() {
            continue;
        }
        let mut d = v;
        let mut dr = d.as_str();
        let mut n = 0usize;
        while n < nargs {
            let be = if n + 1 < nargs {
                comp.arg_expression(&mut dr, vt)?
            } else {
                comp.full_expression(&mut dr, vt)?
            };
            sdef.params.push((pid + n as u16, be));
            n += 1;
            if !comp.trailing_comma() {
                break;
            }
        }
    }
    if let Some(v) = is.take("anim") {
        if !v.trim().is_empty() {
            let mut d = v;
            let mut dr = d.as_str();
            let be = comp.full_expression(&mut dr, ValueType::Int)?;
            sdef.params.push((SD_ANIM, be));
        }
    }
    sbc.state_def = Some(sdef);
    let _ = sec;
    Ok(())
}

