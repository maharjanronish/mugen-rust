//! Port origin: ikemen-src/src/compiler.go — `tokenizer`/`tokenizerCS`
//! (compiler.go:494-636), `isOperator` (compiler.go:638), `number`
//! (compiler.go:705), `specialSymbols` (compiler.go:11).
//!
//! The tokenizer is *stateful over a string cursor*: it advances `in` and
//! returns one token at a time. Lowercasing (`tokenizer` vs `tokenizerCS`)
//! matters because CNS is case-insensitive for identifiers but strings
//! keep their case.
//!
//! Go quirk preserved: `~` is NOT in specialSymbols — it only tokenizes when
//! leading; inside a word it glues to the word. `#` (ZSS comment) is special
//! but not in the switch: Go returns "" without advancing; callers (`scan`)
//! skip to the next line. We consume it as a single-char token instead —
//! equivalent for every caller that strips comments first (see zss.rs).

/// compiler.go:11 `specialSymbols`
pub const SPECIAL_SYMBOLS: &str = " !=<>()|&+-*/%,[]^:;{}#\"\t\r\n";

/// compiler.go:494 `tokenizer` — lowercases the token
pub fn next_token_lower(input: &mut &str) -> String {
    next_token_cs(input).to_lowercase()
}

/// compiler.go:499 `tokenizerCS` — case-sensitive
pub fn next_token_cs(input: &mut &str) -> String {
    *input = input.trim_start();
    let bytes = input.as_bytes();
    let b = match bytes.first() {
        None => return String::new(),
        Some(&b) => b,
    };
    let two = bytes.get(1).copied();
    match b {
        b'=' => {
            *input = &input[1..];
            return "=".into();
        }
        b':' if two == Some(b'=') => {
            *input = &input[2..];
            return ":=".into();
        }
        b':' => {
            *input = &input[1..];
            return ":".into();
        }
        b';' => {
            *input = &input[1..];
            return ";".into();
        }
        b'!' if two == Some(b'=') => {
            *input = &input[2..];
            return "!=".into();
        }
        b'!' => {
            *input = &input[1..];
            return "!".into();
        }
        b'>' if two == Some(b'=') => {
            *input = &input[2..];
            return ">=".into();
        }
        b'>' => {
            *input = &input[1..];
            return ">".into();
        }
        b'<' if two == Some(b'=') => {
            *input = &input[2..];
            return "<=".into();
        }
        b'<' => {
            *input = &input[1..];
            return "<".into();
        }
        b'~' => {
            *input = &input[1..];
            return "~".into();
        }
        b'&' if two == Some(b'&') => {
            *input = &input[2..];
            return "&&".into();
        }
        b'&' => {
            *input = &input[1..];
            return "&".into();
        }
        b'^' if two == Some(b'^') => {
            *input = &input[2..];
            return "^^".into();
        }
        b'^' => {
            *input = &input[1..];
            return "^".into();
        }
        b'|' if two == Some(b'|') => {
            *input = &input[2..];
            return "||".into();
        }
        b'|' => {
            *input = &input[1..];
            return "|".into();
        }
        b'+' => {
            *input = &input[1..];
            return "+".into();
        }
        b'-' => {
            *input = &input[1..];
            return "-".into();
        }
        b'*' if two == Some(b'*') => {
            *input = &input[2..];
            return "**".into();
        }
        b'*' => {
            *input = &input[1..];
            return "*".into();
        }
        b'/' => {
            *input = &input[1..];
            return "/".into();
        }
        b'%' => {
            *input = &input[1..];
            return "%".into();
        }
        b',' => {
            *input = &input[1..];
            return ",".into();
        }
        b'(' => {
            *input = &input[1..];
            return "(".into();
        }
        b')' => {
            *input = &input[1..];
            return ")".into();
        }
        b'[' => {
            *input = &input[1..];
            return "[".into();
        }
        b']' => {
            *input = &input[1..];
            return "]".into();
        }
        b'"' => {
            *input = &input[1..];
            return "\"".into();
        }
        b'{' => {
            *input = &input[1..];
            return "{".into();
        }
        b'}' => {
            *input = &input[1..];
            return "}".into();
        }
        _ => {}
    }

    // Number-ish run: digits with at most one '.', optional exponent
    // (compiler.go:607-626). The exponent scan preserves Go's acceptance of
    // a single sign directly after e/E.
    let mut i = 0usize;
    let mut ten = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'.' {
            if ten {
                break;
            }
            ten = true;
        } else if !c.is_ascii_digit() {
            break;
        }
        i += 1;
    }
    if i > 0 && i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let j = i + 1;
        let mut k = i + 1;
        while k < bytes.len() {
            let c = bytes[k];
            if !c.is_ascii_digit() && (k != j || (c != b'-' && c != b'+')) {
                break;
            }
            k += 1;
        }
        i = k;
    }
    if i == 0 {
        // Word: up to next special symbol (compiler.go:627-632).
        // If a "special" char not covered by the switch leads (e.g. '#'),
        // consume exactly one char to keep making progress (Go's scan() at
        // the ZSS layer skips such tokens by advancing lines instead).
        i = input
            .find(|c: char| SPECIAL_SYMBOLS.contains(c))
            .unwrap_or(input.len());
        if i == 0 {
            i = input.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
        }
    }
    let tok = &input[..i];
    *input = &input[i..];
    tok.to_string()
}

/// compiler.go:638 `isOperator` — precedence classes; negative = terminator
pub fn is_operator(token: &str) -> i32 {
    match token {
        "" | "," | ")" | "]" => -1,
        "||" => 1,
        "^^" => 2,
        "&&" => 3,
        "|" => 4,
        "^" => 5,
        "&" => 6,
        "=" | "!=" => 7,
        ">" | ">=" | "<" | "<=" => 8,
        "+" | "-" => 9,
        "*" | "/" | "%" => 10,
        "**" => 11,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_basics() {
        let mut s = "a = 5";
        assert_eq!(next_token_lower(&mut s), "a");
        assert_eq!(next_token_lower(&mut s), "=");
        assert_eq!(next_token_lower(&mut s), "5");
        assert_eq!(next_token_lower(&mut s), "");

        let mut s = "x>=10.5e3,y";
        assert_eq!(next_token_lower(&mut s), "x");
        assert_eq!(next_token_lower(&mut s), ">=");
        assert_eq!(next_token_lower(&mut s), "10.5e3");
        assert_eq!(next_token_lower(&mut s), ",");
        assert_eq!(next_token_lower(&mut s), "y");

        let mut s = "Command = \"QCF_a\"";
        assert_eq!(next_token_lower(&mut s), "command"); // lowercased
        assert_eq!(next_token_lower(&mut s), "=");
        assert_eq!(next_token_lower(&mut s), "\"");
        assert_eq!(next_token_cs(&mut s), "QCF_a"); // string keeps case

        let mut s = "vel x != 2";
        assert_eq!(next_token_lower(&mut s), "vel");
        assert_eq!(next_token_lower(&mut s), "x");
        assert_eq!(next_token_lower(&mut s), "!=");

        let mut s = "3.4.5";
        assert_eq!(next_token_lower(&mut s), "3.4");
        assert_eq!(next_token_lower(&mut s), ".5");

        let mut s = "$B, a+b";
        assert_eq!(next_token_cs(&mut s), "$B");
        assert_eq!(next_token_cs(&mut s), ",");

        // ~ inside a word glues (not in specialSymbols); leading ~ tokenizes
        let mut s = "a~b";
        assert_eq!(next_token_lower(&mut s), "a~b");
        let mut s = "~5";
        assert_eq!(next_token_lower(&mut s), "~");
        assert_eq!(next_token_lower(&mut s), "5");
    }

    #[test]
    fn operator_precedence_classes() {
        assert!(is_operator("**") > is_operator("*"));
        assert!(is_operator("*") > is_operator("+"));
        assert!(is_operator("+") > is_operator("="));
        assert!(is_operator("=") > is_operator("&&"));
        assert!(is_operator("&&") > is_operator("||"));
        assert_eq!(is_operator(","), -1);
        assert_eq!(is_operator("foo"), 0);
    }
}
