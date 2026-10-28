//! Port origin: ikemen-src/src/common.go — `IniSection` (common.go:981),
//! `ReadIniSection` (common.go:1054), `Parse` (common.go:1071) and the
//! `ReadI32`/`ReadF32`/`ReadBool`/`getText` readers (common.go:1102-1316).
//!
//! This is the universal MUGEN `key = value` section reader used by every
//! content file (DEF, CMD, CNS, stage, fight). Quirks preserved:
//! * section lines end the section; `key` is everything before the first
//!   `=`, space or tab, lowercased; the value is everything after the first
//!   `=``, trimmed;
//! * **the first occurrence of a key wins** (later duplicates are dropped);
//! * comment stripping happens per-line (`;` to end of line) *before* the
//!   key scan, so `key = 1 ; comment` keeps the whole tail — see Parse.

use krg_core::{atof, atoi, section_name};
use std::collections::HashMap;

/// Port of `IniSection` (common.go:981) — a lowercased key → value map.
/// Keys were lowercased at parse time; lookups must be lowercase.
#[derive(Debug, Clone, Default)]
pub struct IniSection {
    map: HashMap<String, String>,
}

impl IniSection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Raw get (key must already be lowercase).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(|s| s.as_str())
    }

    /// First-wins insert (Parse behavior, common.go:1085-1088).
    pub fn insert_first_wins(&mut self, key: &str, value: &str) {
        if !self.map.contains_key(key) {
            self.map.insert(key.to_string(), value.to_string());
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Iterate (key, value) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.map.iter()
    }

    /// Sorted keys — Go's `SortedKeys` helper, used for deterministic
    /// iteration (e.g. the `st`, `st1`… state file lists in char defs).
    pub fn sorted_keys(&self) -> Vec<String> {
        let mut k: Vec<String> = self.map.keys().cloned().collect();
        k.sort();
        k
    }

    /// Port of `Parse` (common.go:1071). `lines[*i..]` are consumed until a
    /// new `[section]` header starts (which is *not* consumed).
    pub fn parse(lines: &[&str], i: &mut usize) -> IniSection {
        let mut is = IniSection::new();
        while *i < lines.len() {
            let raw = lines[*i];
            if !raw.is_empty() && raw.as_bytes()[0] == b'[' {
                break;
            }
            let line = raw.split(';').next().unwrap_or("").trim();
            // Go: IndexAny(line, "= \t") — first '=' OR whitespace splits the key.
            if let Some(ia) = line.find(|c: char| c == '=' || c == ' ' || c == '\t') {
                if ia > 0 {
                    let name = line[..ia].to_lowercase();
                    let data = match line.find('=') {
                        Some(eq) => line[eq + 1..].trim(),
                        None => "",
                    };
                    is.insert_first_wins(&name, data);
                }
            }
            *i += 1;
        }
        is
    }

    /// Port of `ReadI32` (common.go:1102). Comma-separated ints; a missing
    /// or empty field leaves `out[i]` untouched; returns whether the key existed.
    pub fn read_i32(&self, name: &str, out: &mut [&mut i32]) -> bool {
        let Some(s) = self.get(name) else { return false };
        if s.is_empty() {
            return false;
        }
        for (i, part) in s.split(',').enumerate() {
            if i >= out.len() {
                break;
            }
            let p = part.trim();
            if !p.is_empty() {
                *out[i] = atoi(p);
            }
        }
        true
    }

    /// `ReadI32` for one value — the common single-output case.
    pub fn read_i32_1(&self, name: &str) -> Option<i32> {
        let s = self.get(name)?;
        if s.is_empty() {
            return None;
        }
        Some(atoi(s.split(',').next().unwrap_or(s)))
    }

    /// Port of `ReadF32` (common.go:1118).
    pub fn read_f32(&self, name: &str, out: &mut [&mut f32]) -> bool {
        let Some(s) = self.get(name) else { return false };
        if s.is_empty() {
            return false;
        }
        for (i, part) in s.split(',').enumerate() {
            if i >= out.len() {
                break;
            }
            let p = part.trim();
            if !p.is_empty() {
                *out[i] = atof(p) as f32;
            }
        }
        true
    }

    /// `ReadF32` for one value.
    pub fn read_f32_1(&self, name: &str) -> Option<f32> {
        let s = self.get(name)?;
        if s.is_empty() {
            return None;
        }
        Some(atof(s.split(',').next().unwrap_or(s)) as f32)
    }

    /// Port of `ReadBool` (common.go:1134) — any nonzero int is true.
    pub fn read_bool(&self, name: &str, out: &mut [&mut bool]) -> bool {
        let Some(s) = self.get(name) else { return false };
        if s.is_empty() {
            return false;
        }
        for (i, part) in s.split(',').enumerate() {
            if i >= out.len() {
                break;
            }
            let p = part.trim();
            if !p.is_empty() {
                *out[i] = atoi(p) != 0;
            }
        }
        true
    }

    /// `ReadBool` for one value.
    pub fn read_bool_1(&self, name: &str) -> Option<bool> {
        let s = self.get(name)?;
        if s.is_empty() {
            return None;
        }
        Some(atoi(s.split(',').next().unwrap_or(s)) != 0)
    }

    /// Port of `getText` (common.go:1305) — strips mandatory enclosing quotes.
    /// Errors when the value exists but is not fully quoted.
    pub fn get_text(&self, name: &str) -> Result<Option<String>, String> {
        let Some(s) = self.get(name) else { return Ok(None) };
        let b = s.as_bytes();
        if b.len() >= 2 && b[0] == b'"' && b[b.len() - 1] == b'"' {
            Ok(Some(s[1..s.len() - 1].to_string()))
        } else {
            Err("String not enclosed in \"".to_string())
        }
    }
}

/// Port of `ReadIniSection` (common.go:1054): finds the next `[section]`
/// header, then parses its body. Returns `(section, name, subname)`;
/// `name` is empty when no section remains (EOF).
pub fn read_ini_section(lines: &[&str], i: &mut usize) -> (IniSection, String, String) {
    let mut name = String::new();
    let mut subname = String::new();
    while *i < lines.len() {
        if let Some((n, s)) = section_name(lines[*i]) {
            name = n;
            subname = s;
            *i += 1;
            break;
        }
        *i += 1;
    }
    if name.is_empty() {
        return (IniSection::new(), name, subname);
    }
    let is = IniSection::parse(lines, i);
    (is, name, subname)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_first_wins_and_trim() {
        let text = "[Command]\nname = \"QCF_a\"\ncommand = D, DF, F, a\ntime = 15\nname = dup";
        let lines: Vec<&str> = text.split('\n').collect();
        let mut i = 0;
        let (is, name, sub) = read_ini_section(&lines, &mut i);
        assert_eq!(name, "command");
        assert_eq!(sub, "");
        assert_eq!(is.get_text("name").unwrap().unwrap(), "QCF_a"); // first wins
        assert_eq!(is.get("command"), Some("D, DF, F, a"));
        assert_eq!(is.read_i32_1("time"), Some(15));
    }

    #[test]
    fn comment_only_lines_and_section_split() {
        let text = "; a comment\n[Defaults]\ntime=20 ; trailing comment\n\n[Command]\nx=1";
        let lines: Vec<&str> = text.split('\n').collect();
        let mut i = 0;
        let (is1, n1, _) = read_ini_section(&lines, &mut i);
        assert_eq!(n1, "defaults");
        assert_eq!(is1.read_i32_1("time"), Some(20));
        let (is2, n2, _) = read_ini_section(&lines, &mut i);
        assert_eq!(n2, "command");
        assert_eq!(is2.read_i32_1("x"), Some(1));
    }

    #[test]
    fn quoted_text_requires_quotes() {
        let mut is = IniSection::new();
        is.insert_first_wins("a", "no-quotes");
        assert!(is.get_text("a").is_err());
        is.insert_first_wins("b", "\"quoted\"");
        assert_eq!(is.get_text("b").unwrap().unwrap(), "quoted");
        assert_eq!(is.get_text("zz").unwrap(), None);
    }

    #[test]
    fn multi_value_reads() {
        let mut is = IniSection::new();
        is.insert_first_wins("v", "1, 2 ,3");
        let mut a = 0i32;
        let mut b = 0i32;
        let mut c = 0i32;
        assert!(is.read_i32("v", &mut [&mut a, &mut b, &mut c]));
        assert_eq!((a, b, c), (1, 2, 3));
        let (mut f1, mut f2) = (0.0f32, 0.0f32);
        is.insert_first_wins("f", "0.5, -2");
        assert!(is.read_f32("f", &mut [&mut f1, &mut f2]));
        assert_eq!((f1, f2), (0.5, -2.0));
        // empty field leaves output untouched
        is.insert_first_wins("g", "5, ");
        let mut g1 = 0i32;
        let mut g2 = 9i32;
        assert!(is.read_i32("g", &mut [&mut g1, &mut g2]));
        assert_eq!((g1, g2), (5, 9));
    }
}
