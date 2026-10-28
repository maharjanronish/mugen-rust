//! Lenient MUGEN text parsers — faithful ports of Ikemen GO `src/common.go`.
//!
//! Behavior notes (these are NOT std-lib semantics, on purpose):
//! * `atoi`/`atof` parse the *longest numeric prefix*, ignore trailing garbage,
//!   return 0 when there is no number, and **saturate** to `I_MAX`/`I_ERR`
//!   instead of erroring on overflow.
//! * `atof` is a hand-rolled fixed/exponential parser whose exact rounding and
//!   exponent-sign quirks are preserved because CNS/trigger constants flow
//!   through it and must match the Go engine bit-for-bit.

use crate::{I_ERR, I_MAX};

/// Port of `IsNumeric` (common.go:166): Go `ParseFloat` semantics on trimmed input.
pub fn is_numeric(s: &str) -> bool {
    let t = s.trim();
    // Go's strconv.ParseFloat accepts inf/nan/hex-float; MUGEN data in the wild
    // relies on the common cases only, so we mirror the accepted grammar closely:
    // [+-]? ( digits [ '.' digits? ] | '.' digits ) ( [eE] [+-]? digits )?
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut saw_digit = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        saw_digit = true;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            saw_digit = true;
        }
    }
    if !saw_digit {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        let mut j = i;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let mut exp_digit = false;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
            exp_digit = true;
        }
        // Go requires digits after 'e'; "1e" fails there.
        if !exp_digit {
            return false;
        }
        i = j;
    }
    i == b.len()
}

/// Port of `Atoi` (common.go:191).
///
/// * Trim whitespace, optional leading `+`/`-`.
/// * Consume decimal digits until the first non-digit (rest is garbage).
/// * Saturate: positive overflow → `I_MAX`, negative overflow → `I_ERR`
///   (with a warning in Go; we return the same values silently).
/// * Non-numeric input yields 0.
pub fn atoi(s: &str) -> i32 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    let bytes = s.as_bytes();
    let (sign, a) = match bytes[0] {
        b'-' | b'+' => (-1i64 * if bytes[0] == b'-' { 1 } else { -1 }, &s[1..]),
        _ => (1i64, s),
    };
    // Go: limit = MaxInt32, or MaxInt32+1 when negative (so i32::MIN is reachable).
    let limit: i64 = if sign < 0 { I_MAX as i64 + 1 } else { I_MAX as i64 };
    let mut n: i64 = 0;
    for c in a.bytes() {
        if !c.is_ascii_digit() {
            break;
        }
        n = n * 10 + (c - b'0') as i64;
        if n > limit {
            return if sign < 0 { I_ERR } else { I_MAX };
        }
    }
    (sign * n) as i32
}

/// Port of `Atof` (common.go:227). Preserves the exact hand-rolled algorithm
/// including its exponent-sign quirk (see tests).
pub fn atof(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }
    let neg = s.starts_with('-');
    // Go: a = mantissa text without sign. Note: Go keeps only '-'/'+' as sign.
    let a: &str = if s.as_bytes()[0] == b'-' || s.as_bytes()[0] == b'+' {
        &s[1..]
    } else {
        s
    };
    let ab = a.as_bytes();
    let mut f = 0.0f64;
    let mut p = 0usize;
    let mut i = 0usize;
    while i < ab.len() {
        let c = ab[i];
        if c == b'.' {
            if p != 0 {
                break;
            }
            p = i + 1;
            i += 1;
            continue;
        }
        if !c.is_ascii_digit() {
            break;
        }
        f = f * 10.0 + (c - b'0') as f64;
        i += 1;
    }
    let mut e = 0.0f64;
    // Go: `if i+1 < len(a) && (a[i] == 'e' || a[i] == 'E')`
    if i + 1 < ab.len() && (ab[i] == b'e' || ab[i] == b'E') {
        let mut j = i + 1;
        if j < ab.len() && (ab[j] == b'-' || ab[j] == b'+') {
            j += 1;
        }
        while j < ab.len() && ab[j].is_ascii_digit() {
            e = e * 10.0 + (ab[j] - b'0') as f64;
            j += 1;
        }
        if e != 0.0 {
            // Go quirk preserved: the check is `str[i+1] == '-'` — indexing the
            // FULL string with an offset computed against the sign-stripped
            // mantissa. With a leading sign, i+1 lands on the 'e' itself, so
            // the exponent sign is ignored (Go bug, kept for parity).
            let sb = s.as_bytes();
            if i + 1 < sb.len() && sb[i + 1] == b'-' {
                e *= -1.0;
            }
            if p == 0 {
                p = i;
            }
        }
    }
    if p > 0 {
        f *= 10f64.powi((p as i64 - i as i64 + e as i64) as i32);
    }
    if neg {
        -f
    } else {
        f
    }
}

/// Port of `SectionName` (common.go:862).
///
/// Parses `"[Begin Action 0]"`-style section headers. Returns
/// `(name, subname)` where `name` is lowercased *including a trailing space*
/// when one exists (Go keeps it: `name = sec[:i+1]`), and `subname` is the
/// remainder, case preserved. Returns `None` for non-section lines.
pub fn section_name(sec: &str) -> Option<(String, String)> {
    if !sec.starts_with('[') {
        return None;
    }
    // Go: trim AFTER stripping the comment: sec = TrimSpace(SplitN(sec, ";", 2)[0])
    let sec_no_comment = sec.split(';').next().unwrap_or("");
    let sec_t = sec_no_comment.trim();
    // Go then checks sec[len-1] == ']' on the TRIMMED string that still starts with '['
    if !sec_t.ends_with(']') {
        return None;
    }
    // Go: sec = sec[1:Index(sec, "]")]  — first ']' terminates.
    let inner = &sec_t[1..];
    let end = inner.find(']')?;
    let inner = &inner[..end];
    match inner.find(' ') {
        Some(i) => {
            let name = format!("{} ", &inner[..i]); // Go: sec[:i+1] keeps the space
            let subname = inner[i + 1..].to_string();
            Some((name.to_lowercase(), subname))
        }
        None => Some((inner.to_lowercase(), String::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoi_lenient() {
        assert_eq!(atoi("123"), 123);
        assert_eq!(atoi(" 42abc"), 42); // prefix parse, garbage ignored
        assert_eq!(atoi("abc"), 0); // no digits → 0
        assert_eq!(atoi("-7"), -7);
        assert_eq!(atoi("+7"), 7);
        assert_eq!(atoi("12.9"), 12); // stops at '.'
        assert_eq!(atoi("2147483647"), i32::MAX);
        assert_eq!(atoi("2147483648"), I_MAX); // saturate up
        assert_eq!(atoi("-2147483648"), i32::MIN);
        assert_eq!(atoi("-2147483649"), I_ERR); // saturate down
        assert_eq!(atoi(""), 0);
        assert_eq!(atoi("  "), 0);
        assert_eq!(atoi("-"), 0);
    }

    #[test]
    fn atof_basic() {
        assert_eq!(atof("3.14"), 3.14);
        assert_eq!(atof("-2.5"), -2.5);
        assert_eq!(atof("42abc"), 42.0);
        assert_eq!(atof("abc"), 0.0);
        assert_eq!(atof("5e2"), 500.0);
        assert_eq!(atof("5e-1"), 0.5);
        assert_eq!(atof(".5"), 0.5);
        assert_eq!(atof("+1.25"), 1.25);
    }

    #[test]
    fn atof_go_quirk_signed_exponent() {
        // Go bug preserved: leading sign shifts str indices, so "-1e-5"
        // reads the exponent sign at the wrong index and stays positive.
        assert_eq!(atof("-1e-5"), -100000.0);
        // Without leading sign the exponent sign IS honored.
        assert_eq!(atof("1e-5"), 0.00001);
    }

    #[test]
    fn is_numeric_cases() {
        assert!(is_numeric("3.14"));
        assert!(is_numeric(" -2 "));
        assert!(is_numeric("1e5"));
        assert!(is_numeric("+0.5"));
        assert!(!is_numeric("abc"));
        assert!(!is_numeric("1e")); // Go ParseFloat rejects "1e"
        assert!(!is_numeric(""));
    }

    #[test]
    fn section_names() {
        assert_eq!(
            section_name("[Begin Action 5]"),
            Some(("begin ".to_string(), "Action 5".to_string()))
        );
        assert_eq!(section_name("[Data]"), Some(("data".to_string(), String::new())));
        assert_eq!(section_name("clsn2: 1"), None);
        assert_eq!(section_name("[unclosed"), None);
        // Comment inside section header: Go strips the comment first, then trims.
        assert_eq!(
            section_name("[Begin Action 2]; comment"),
            Some(("begin ".to_string(), "Action 2".to_string()))
        );
        // But trailing text after a space (not comment) makes it fail the ']' check.
        assert_eq!(section_name("[Begin Action 2] ; note"), {
            // split(';') → "[Begin Action 2] " → trim → "[Begin Action 2]" ✓
            Some(("begin ".to_string(), "Action 2".to_string()))
        });
    }
}
