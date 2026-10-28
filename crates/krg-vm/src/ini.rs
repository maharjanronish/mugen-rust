//! Port origin: ikemen-src/src/common.go — text/INI plumbing shared by the
//! CMD reader (compiler.go:8636), the CNS compiler (compiler.go:7219) and
//! the .def parser.
//!
//! * `load_text`      — common.go:503 `LoadText` (BOM strip; CRLF tolerated)
//! * `split_and_trim` — common.go:799 `SplitAndTrim`
//! * `section_name`   — common.go:862 `SectionName`
//! * `read_ini_section` — common.go:1054 `ReadIniSection` + `IniSection`
//! * `atoi` / `atof`  — common.go:191 / :231 (leading-digits semantics:
//!   `Atoi("2.5")==2`, `Atoi("1e3")==1`, saturation instead of error)

/// LoadText (common.go:503): strip UTF-8 BOM. CRLF handled by trim later.
pub fn load_text(bytes: &[u8]) -> String {
    let s = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    String::from_utf8_lossy(s).into_owned()
}

/// SplitAndTrim (common.go:799) — split + `str::trim` each line (`\r` dies).
pub fn split_and_trim(text: &str, sep: &str) -> Vec<String> {
    text.split(sep).map(|l| l.trim().to_string()).collect()
}

/// SectionName (common.go:862). Returns `(name_lowercase_with_space,
/// subname)`. The name keeps its trailing space ("statedef "), exactly like
/// Go, so callers can prefix-match reliably.
pub fn section_name(sec: &str) -> (String, String) {
    if sec.is_empty() || !sec.starts_with('[') {
        return (String::new(), String::new());
    }
    let sec = sec.split(';').next().unwrap_or("").trim();
    if !sec.ends_with(']') {
        return (String::new(), String::new());
    }
    let sec = &sec[1..sec.find(']').unwrap_or(sec.len())];
    let (name, sub) = match sec.find(' ') {
        Some(i) => (&sec[..i + 1], &sec[i + 1..]),
        None => (sec, ""),
    };
    (name.to_lowercase(), sub.to_string())
}

/// IniSection (common.go:981): lowercased key → raw value map, first-wins.
#[derive(Debug, Clone, Default)]
pub struct IniSection {
    map: std::collections::HashMap<String, String>,
}

impl IniSection {
    pub fn new() -> Self {
        Self::default()
    }

    /// IniSection.Parse (common.go:1071): stop at next `[`-line; keys split
    /// at first of `=`/space/tab, value after first `=`; `;` comments are
    /// stripped *without* quote awareness (MUGEN semantics).
    pub fn parse(lines: &[String], i: &mut usize) -> Self {
        let mut is = Self::new();
        while *i < lines.len() {
            if !lines[*i].is_empty() && lines[*i].starts_with('[') {
                break;
            }
            let line = lines[*i].split(';').next().unwrap_or("").trim();
            if let Some(ia) = line.find(['=', '\t', ' ']) {
                if ia > 0 {
                    let name = line[..ia].to_lowercase();
                    let data = match line.find('=') {
                        Some(eq) => line[eq + 1..].trim().to_string(),
                        None => String::new(),
                    };
                    is.map.entry(name).or_insert(data);
                }
            }
            *i += 1;
        }
        is
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(|s| s.as_str())
    }

    pub fn get_lc(&self, name: &str) -> Option<&str> {
        // keys are stored lowercased; callers may pass any case
        self.map.get(&name.to_lowercase()).map(|s| s.as_str())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.map.contains_key(&name.to_lowercase())
    }

    /// Take + remove (used by the state-controller param reader, which
    /// consumes keys to detect leftovers).
    pub fn take(&mut self, name: &str) -> Option<String> {
        self.map.remove(&name.to_lowercase())
    }

    pub fn insert(&mut self, name: &str, value: impl Into<String>) {
        self.map.insert(name.to_lowercase(), value.into());
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.map.iter()
    }

    /// getText (common.go:1305): value must be `"-quoted` (outer pair
    /// stripped); error otherwise.
    pub fn get_text(&self, name: &str) -> Result<Option<String>, String> {
        let Some(s) = self.map.get(&name.to_lowercase()) else {
            return Ok(None);
        };
        if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
            Ok(Some(s[1..s.len() - 1].to_string()))
        } else {
            Err(format!("String not enclosed in \" ({name} = {s:?})"))
        }
    }

    /// ReadI32 (common.go:1102) — comma multi-assign, Atoi semantics.
    pub fn read_i32(&self, name: &str, out: &mut [&mut i32]) -> bool {
        let Some(s) = self.map.get(&name.to_lowercase()) else {
            return false;
        };
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

    pub fn read_i32_one(&self, name: &str) -> Option<i32> {
        let mut v = 0i32;
        if self.read_i32(name, &mut [&mut v]) {
            Some(v)
        } else {
            None
        }
    }

    /// ReadF32 (common.go:1117) — Atof semantics.
    pub fn read_f32(&self, name: &str, out: &mut [&mut f32]) -> bool {
        let Some(s) = self.map.get(&name.to_lowercase()) else {
            return false;
        };
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

    /// ReadBool (common.go:1132) — `Atoi(x) != 0`.
    pub fn read_bool(&self, name: &str, out: &mut [&mut bool]) -> bool {
        let Some(s) = self.map.get(&name.to_lowercase()) else {
            return false;
        };
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

    pub fn read_bool_one(&self, name: &str) -> Option<bool> {
        let mut v = false;
        if self.read_bool(name, &mut [&mut v]) {
            Some(v)
        } else {
            None
        }
    }
}

/// ReadIniSection (common.go:1054): scan to next `[section]` header, parse
/// its body. Returns (section, name, subname); empty name = EOF.
pub fn read_ini_section(lines: &[String], i: &mut usize) -> (IniSection, String, String) {
    let mut name = String::new();
    let mut subname = String::new();
    while *i < lines.len() {
        let (n, s) = section_name(&lines[*i]);
        if !n.is_empty() {
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

/// Atoi (common.go:191): sign + leading decimal digits only; saturates to
/// `IErr`/`IMax` instead of erroring (Go parity: `Atoi("2.5")==2`).
pub fn atoi(s: &str) -> i32 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    let neg = s.starts_with('-');
    let a = if neg || s.starts_with('+') { &s[1..] } else { s };
    // negative side allows one more (i32::MIN)
    let mut limit = i32::MAX as i64;
    if neg {
        limit += 1;
    }
    let mut n: i64 = 0;
    for (i, b) in a.bytes().enumerate() {
        if !b.is_ascii_digit() {
            break;
        }
        n = n * 10 + (b - b'0') as i64;
        if n > limit {
            return if neg { i32::MIN } else { i32::MAX };
        }
        let _ = i;
    }
    if neg {
        n = -n;
    }
    n as i32
}

/// Atof (common.go:231): sign, digits, one `.`, optional exponent.
pub fn atof(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }
    let (neg, a) = if s.starts_with('-') {
        (true, &s[1..])
    } else if s.starts_with('+') {
        (false, &s[1..])
    } else {
        (false, s)
    };
    let mut f = 0.0f64;
    let mut i = 0usize;
    let mut p = 0usize;
    let bytes = a.as_bytes();
    while i < bytes.len() {
        let c = bytes[i];
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
    let mut eneg = false;
    if i + 1 < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut j = i + 1;
        if bytes[j] == b'-' || bytes[j] == b'+' {
            eneg = bytes[j] == b'-';
            j += 1;
        }
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            e = e * 10.0 + (bytes[j] - b'0') as f64;
            j += 1;
        }
        if e != 0.0 {
            if eneg {
                f /= 10f64.powf(e);
            } else {
                f *= 10f64.powf(e);
            }
        }
    }
    if p != 0 {
        let div = 10f64.powf((i - p) as f64);
        if neg {
            -f / div
        } else {
            f / div
        }
    } else if neg {
        -f
    } else {
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoi_atof_quirks() {
        assert_eq!(atoi("2.5"), 2);
        assert_eq!(atoi("1e3"), 1);
        assert_eq!(atoi("-abc"), 0);
        assert_eq!(atoi("+7"), 7);
        assert_eq!(atoi("-2147483648"), i32::MIN);
        assert_eq!(atoi("99999999999"), i32::MAX);
        assert!((atof("-1.5") + 1.5).abs() < 1e-9);
        assert!((atof("2e3") - 2000.0).abs() < 1e-9);
        assert!((atof(".5") - 0.5).abs() < 1e-9);
    }

    #[test]
    fn section_splitting() {
        let text = "; comment\n[Statedef 200]\ntype = S\nphysics = S ; inline\n[State 200, hit]\ntype = HitDef\ntrigger1 = 1\n";
        let lines = split_and_trim(text, "\n");
        assert_eq!(lines[0], "; comment");
        let mut i = 0;
        let (_, name, sub) = read_ini_section(&lines, &mut i);
        assert_eq!(name, "statedef ");
        assert_eq!(sub, "200");
    }

    #[test]
    fn ini_parse_first_wins_and_lower() {
        let text = "Foo = 1\nfoo = 2\nBar = 3";
        let lines = split_and_trim(text, "\n");
        let mut i = 0;
        let is = IniSection::parse(&lines, &mut i);
        assert_eq!(is.get("foo"), Some("1"));
        assert_eq!(is.get("bar"), Some("3"));
        assert_eq!(is.read_i32_one("foo"), Some(1));
    }

    #[test]
    fn section_name_keeps_space() {
        let (n, s) = section_name("[Statedef 200]");
        assert_eq!(n, "statedef ");
        assert_eq!(s, "200");
        let (n, s) = section_name("[State 200, Walk Fwd]");
        assert_eq!(n, "state ");
        assert_eq!(s, "200, Walk Fwd");
        let (n, _) = section_name("[Command]");
        assert_eq!(n, "command");
    }
}
