//! Port origin: ikemen-src/src/bytecode.go — `BytecodeValue`, `ValueType`,
//! arithmetic fold methods (`add`/`sub`/…, bytecode.go:1747-2010).
//!
//! `BytecodeValue` is a tagged f64: MUGEN's CNS has no separate int/float
//! storage — every number lives in one f64 and the *type tag* decides how
//! operations coerce. Type-int promotions follow Go's `Min(vtype)` rule:
//! float path iff neither operand is `VT_None` and at least one is `VT_Float`
//! (bytecode.go:1752 and friends). Enum order is load-bearing:
//! None=0, **Float=1**, Int=2, Bool=3, String=4, Undefined=5.

/// ikemen `ValueType` (bytecode.go:267). Order matters — see `min_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValueType {
    None = 0,
    Float = 1,
    Int = 2,
    Bool = 3,
    String = 4,
    Undefined = 5,
}

/// ikemen `BytecodeValue` (bytecode.go:1306) — `{vtype, value float64}`.
/// Strings are pool *indices* stored in `value` (bytecode.go:1445).
#[derive(Debug, Clone, Copy)]
pub struct Value {
    pub vtype: ValueType,
    pub value: f64,
}

pub fn v_none() -> Value {
    Value { vtype: ValueType::None, value: 0.0 }
}
pub fn v_undef() -> Value {
    Value { vtype: ValueType::Undefined, value: f64::NAN }
}
pub fn v_int(i: i32) -> Value {
    Value { vtype: ValueType::Int, value: i as f64 }
}
pub fn v_float(f: f32) -> Value {
    if f.is_nan() {
        v_undef()
    } else {
        Value { vtype: ValueType::Float, value: f as f64 }
    }
}
pub fn v_bool(b: bool) -> Value {
    Value { vtype: ValueType::Bool, value: if b { 1.0 } else { 0.0 } }
}
pub fn v_string(idx: usize) -> Value {
    Value { vtype: ValueType::String, value: idx as f64 }
}

impl Value {
    /// bytecode.go:1311
    pub fn is_none(&self) -> bool {
        self.vtype == ValueType::None
    }
    /// bytecode.go:1315 — Undefined vtype OR NaN payload
    pub fn is_undefined(&self) -> bool {
        self.vtype == ValueType::Undefined || self.value.is_nan()
    }

    /// bytecode.go:1319 — Undefined/String read as 0 (NOT NaN: math must survive)
    pub fn to_f(&self) -> f32 {
        if self.is_undefined() || self.vtype == ValueType::String {
            return 0.0;
        }
        self.value as f32
    }
    /// bytecode.go:1328 — Go `int32(f64)` truncation semantics
    pub fn to_i(&self) -> i32 {
        if self.is_undefined() || self.vtype == ValueType::String {
            return 0;
        }
        self.value as i32
    }
    /// bytecode.go:1342 — String is always false, Undefined is false
    pub fn to_b(&self) -> bool {
        if self.is_undefined() {
            return false;
        }
        if self.vtype == ValueType::String {
            return false;
        }
        self.value != 0.0
    }
    /// bytecode.go:1370 — resolve pool index into text
    pub fn to_s<'a>(&self, pool: &'a [String]) -> &'a str {
        if self.vtype != ValueType::String {
            return "";
        }
        let idx = self.value as i64;
        if idx < 0 || idx as usize >= pool.len() {
            return "";
        }
        &pool[idx as usize]
    }

    /// bytecode.go:1396 — NaN becomes Undefined
    pub fn set_f(&mut self, f: f32) {
        if f.is_nan() {
            *self = v_undef();
        } else {
            *self = Value { vtype: ValueType::Float, value: f as f64 };
        }
    }
    pub fn set_i(&mut self, i: i32) {
        *self = v_int(i);
    }
    pub fn set_b(&mut self, b: bool) {
        *self = v_bool(b);
    }
}

/// The `Min(int32(v1.vtype), int32(v2.vtype)) == VT_Float` promotion test.
/// True iff neither operand is None and at least one is Float.
#[inline]
fn float_path(v1: &Value, v2: &Value) -> bool {
    v1.vtype != ValueType::None
        && v2.vtype != ValueType::None
        && (v1.vtype == ValueType::Float || v2.vtype == ValueType::Float)
}

// ── Arithmetics (bytecode.go:1747-1964) — runtime & fold semantics ──────────
// Go quirks preserved:
//   * no None/Undefined guards (commented out upstream)
//   * div by zero (ToF) → Undefined; mod by zero (ToI) → Undefined
//   * `%` always yields int
//   * int ops wrap silently (Go int32 arithmetic)

pub fn add(v1: &Value, v2: &Value) -> Value {
    if float_path(v1, v2) {
        let mut r = *v1;
        r.set_f(v1.to_f() + v2.to_f());
        r
    } else {
        v_int(v1.to_i().wrapping_add(v2.to_i()))
    }
}
pub fn sub(v1: &Value, v2: &Value) -> Value {
    if float_path(v1, v2) {
        let mut r = *v1;
        r.set_f(v1.to_f() - v2.to_f());
        r
    } else {
        v_int(v1.to_i().wrapping_sub(v2.to_i()))
    }
}
pub fn mul(v1: &Value, v2: &Value) -> Value {
    if float_path(v1, v2) {
        let mut r = *v1;
        r.set_f(v1.to_f() * v2.to_f());
        r
    } else {
        v_int(v1.to_i().wrapping_mul(v2.to_i()))
    }
}
pub fn div(v1: &Value, v2: &Value) -> Value {
    if v2.to_f() == 0.0 {
        v_undef()
    } else if float_path(v1, v2) {
        let mut r = *v1;
        r.set_f(v1.to_f() / v2.to_f());
        r
    } else {
        v_int(v1.to_i().wrapping_div(v2.to_i()))
    }
}
pub fn modulo(v1: &Value, v2: &Value) -> Value {
    if v2.to_i() == 0 {
        v_undef()
    } else {
        v_int(v1.to_i().wrapping_rem(v2.to_i()))
    }
}
pub fn neg(v1: &Value) -> Value {
    if v1.is_none() {
        return v_none();
    }
    if v1.vtype == ValueType::Int {
        v_int(-(v1.value as i32))
    } else {
        v_float(-(v1.to_f()))
    }
}
/// bytecode.go:1709 `pow`. Int fast path (plain square&multiply; the
/// WinMugen bit-order quirk for pre-1.0 chars is deferred, see PARITY notes).
pub fn pow(v1: &Value, v2: &Value) -> Value {
    if float_path(v1, v2) || v2.to_f() < 0.0 {
        let mut r = *v1;
        r.set_f(v1.to_f().powf(v2.to_f()));
        r
    } else {
        let (i1, i2) = (v1.to_i() as i64, v2.to_i() as i64);
        v_int((i1.wrapping_pow(i2.clamp(0, u32::MAX as i64) as u32)) as i32)
    }
}

/// bytecode.go:1909 `eq` — strings compare case-insensitively; mixed
/// string/non-string is false; float path iff both non-None with a float.
pub fn eq(v1: &Value, v2: &Value, p1: &[String], p2: &[String]) -> Value {
    if v1.vtype == ValueType::String || v2.vtype == ValueType::String {
        if v1.vtype != ValueType::String || v2.vtype != ValueType::String {
            return v_bool(false);
        }
        let a = v1.to_s(p1).to_lowercase();
        let b = v2.to_s(p2).to_lowercase();
        return v_bool(a == b);
    }
    if float_path(v1, v2) {
        v_bool(v1.to_f() == v2.to_f())
    } else {
        v_bool(v1.to_i() == v2.to_i())
    }
}
/// bytecode.go:1933 `ne`
pub fn ne(v1: &Value, v2: &Value, p1: &[String], p2: &[String]) -> Value {
    if v1.vtype == ValueType::String || v2.vtype == ValueType::String {
        if v1.vtype != ValueType::String || v2.vtype != ValueType::String {
            return v_bool(true);
        }
        let a = v1.to_s(p1).to_lowercase();
        let b = v2.to_s(p2).to_lowercase();
        return v_bool(a != b);
    }
    if float_path(v1, v2) {
        v_bool(v1.to_f() != v2.to_f())
    } else {
        v_bool(v1.to_i() != v2.to_i())
    }
}

pub fn gt(v1: &Value, v2: &Value) -> Value {
    if float_path(v1, v2) {
        v_bool(v1.to_f() > v2.to_f())
    } else {
        v_bool(v1.to_i() > v2.to_i())
    }
}
pub fn ge(v1: &Value, v2: &Value) -> Value {
    if float_path(v1, v2) {
        v_bool(v1.to_f() >= v2.to_f())
    } else {
        v_bool(v1.to_i() >= v2.to_i())
    }
}
pub fn lt(v1: &Value, v2: &Value) -> Value {
    if float_path(v1, v2) {
        v_bool(v1.to_f() < v2.to_f())
    } else {
        v_bool(v1.to_i() < v2.to_i())
    }
}
pub fn le(v1: &Value, v2: &Value) -> Value {
    if float_path(v1, v2) {
        v_bool(v1.to_f() <= v2.to_f())
    } else {
        v_bool(v1.to_i() <= v2.to_i())
    }
}

/// bytecode.go:1863 `rangeCheck` — Undefined respected here (unlike gt/lt)
pub fn range_check(v1: &Value, min: &Value, max: &Value, incl_min: bool, incl_max: bool) -> Value {
    if v1.is_undefined() || min.is_undefined() || max.is_undefined() {
        return v_undef();
    }
    let all_floaty = [v1.vtype, min.vtype, max.vtype];
    let any_none = all_floaty.iter().any(|t| *t == ValueType::None);
    let any_float = all_floaty.iter().any(|t| *t == ValueType::Float);
    let (min_pass, max_pass) = if any_float && !any_none {
        let (v, lo, hi) = (v1.to_f(), min.to_f(), max.to_f());
        (
            if incl_min { v >= lo } else { v > lo },
            if incl_max { v <= hi } else { v < hi },
        )
    } else {
        let (v, lo, hi) = (v1.to_i(), min.to_i(), max.to_i());
        (
            if incl_min { v >= lo } else { v > lo },
            if incl_max { v <= hi } else { v < hi },
        )
    };
    v_bool(min_pass && max_pass)
}

pub fn bool_and(v1: &Value, v2: &Value) -> Value {
    v_bool(v1.to_b() && v2.to_b())
}
pub fn bool_or(v1: &Value, v2: &Value) -> Value {
    v_bool(v1.to_b() || v2.to_b())
}
pub fn bool_xor(v1: &Value, v2: &Value) -> Value {
    v_bool(v1.to_b() != v2.to_b())
}
/// bytecode.go:1695 `blnot` — Undefined stays Undefined (no !false flip)
pub fn bool_not(v1: &Value) -> Value {
    if v1.is_undefined() {
        return *v1;
    }
    v_bool(!v1.to_b())
}

pub fn bw_and(v1: &Value, v2: &Value) -> Value {
    v_int(v1.to_i() & v2.to_i())
}
pub fn bw_or(v1: &Value, v2: &Value) -> Value {
    v_int(v1.to_i() | v2.to_i())
}
pub fn bw_xor(v1: &Value, v2: &Value) -> Value {
    v_int(v1.to_i() ^ v2.to_i())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_basics() {
        assert_eq!(v_int(3).to_i(), 3);
        assert!(!v_int(0).to_b());
        assert!(v_int(-1).to_b());
        assert!(!v_string(0).to_b()); // strings are false
        assert_eq!(v_undef().to_i(), 0); // undefined reads as 0
        assert!(v_undef().is_undefined());
        assert!(!v_none().is_undefined());
    }

    #[test]
    fn promotion_min_rule() {
        // float beats int, None forces int path
        assert_eq!(add(&v_int(2), &v_int(3)).vtype, ValueType::Int);
        assert_eq!(add(&v_int(2), &v_float(3.0)).vtype, ValueType::Float);
        assert_eq!(add(&v_none(), &v_float(3.0)).vtype, ValueType::Int); // None → int path
        assert_eq!(add(&v_none(), &v_float(3.0)).to_i(), 3);
        assert_eq!(add(&v_int(2), &v_int(3)).to_i(), 5);
        assert_eq!(div(&v_int(7), &v_int(2)).to_i(), 3); // int div truncates
        assert!(div(&v_int(1), &v_int(0)).is_undefined()); // div by zero
        assert_eq!(modulo(&v_int(-7), &v_int(3)).to_i(), -1); // Go % semantics
        assert!(modulo(&v_int(1), &v_int(0)).is_undefined());
        // undefined operand in int path reads as 0 (not an error)
        assert_eq!(add(&v_undef(), &v_int(5)).to_i(), 5);
        // string operand reads as 0 via int path (Min(String,Int)=Int)
        assert_eq!(add(&v_string(0), &v_int(5)).to_i(), 5);
    }

    #[test]
    fn string_eq_is_case_insensitive() {
        let pool = vec!["Abc".into(), "aBC".into()];
        assert!(eq(&v_string(0), &v_string(1), &pool, &pool).to_b());
        let pool2 = vec!["xyz".into()];
        assert!(!eq(&v_string(0), &v_string(0), &pool, &pool2).to_b());
        // mixed string/number → false, never error
        assert!(!eq(&v_string(0), &v_int(0), &pool, &pool).to_b());
        assert!(ne(&v_string(0), &v_int(0), &pool, &pool).to_b());
    }

    #[test]
    fn bool_not_undefined_stays() {
        assert!(bool_not(&v_undef()).is_undefined());
        assert!(!bool_not(&v_bool(true)).to_b());
    }

    #[test]
    fn range_checks() {
        let t = v_int(5);
        assert!(range_check(&t, &v_int(1), &v_int(9), true, true).to_b());
        assert!(range_check(&t, &v_int(5), &v_int(9), true, true).to_b()); // [5,9]
        assert!(!range_check(&t, &v_int(5), &v_int(9), false, true).to_b()); // (5,9]
        assert!(!range_check(&v_undef(), &v_int(1), &v_int(9), true, true).to_b());
        assert!(range_check(&v_undef(), &v_int(1), &v_int(9), true, true).is_undefined());
    }
}
