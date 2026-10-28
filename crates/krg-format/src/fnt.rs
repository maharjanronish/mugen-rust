//! Bitmap FNT v2 fonts — port of the SFF branch of Ikemen GO `src/font.go`.
//!
//! A bitmap font is a pair: `*.def` (metrics) + `*.sff` (glyph sprites,
//! SFF v2). Glyph addressing (font.go `LoadFntSff`): **sprite number =
//! character code**, group = bank (0 for the usual `BankType = palette`
//! fonts; with `BankType = sprite` every group is a bank and only bank 0 is
//! consumed here).
//!
//! Layout rules (font.go `CharWidth`/`TextWidth`/`drawChar`):
//! * `' '` advances `Size[0]` and draws nothing.
//! * A present glyph advances `glyph.w + Spacing[0]`.
//! * A missing glyph draws nothing and advances `Spacing[0]` only
//!   (CharWidth 0 — the same "invisible" behavior as the Go engine).
//! * Case fallback: bitmap fonts ship one case only (f-6x9 is uppercase
//!   only), so lookup tries the char, then the other ASCII case.
//!
//! Drawing itself happens in the renderer: a glyph is just one more sprite
//! pushed through the SFF sprite path, so text inherits streaming, batching
//! and per-quad tint (white glyphs recolor exactly like MUGEN's palette
//! swap).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use krg_core::{atoi, section_name};

use crate::sff::{Sff, SffType};

/// One glyph: where it lives in the font SFF plus its metrics.
#[derive(Debug, Clone, Copy)]
pub struct Glyph {
    /// Bank (SFF group) the glyph lives in.
    pub group: u16,
    /// Character code (SFF sprite number).
    pub number: u16,
    /// Sprite X offset — the glyph's horizontal draw offset.
    pub ofs: i16,
    /// Glyph advance width in pixels.
    pub w: u16,
}

/// Everything needed to lay out and draw text except the pixel source:
/// metrics + the bank-0 glyph table. Split out of [`BitmapFont`] so the
/// scene can own the SFF inside a `SpriteCache` (streaming) while the
/// metrics stay plain data.
#[derive(Debug, Clone, Default)]
pub struct FontMetrics {
    pub name: String,
    /// `Size = w, h` — w is the space advance.
    pub size: [u16; 2],
    /// `Spacing = x, y`.
    pub spacing: [i32; 2],
    /// `Offset = x, y` — draw offset applied once per string.
    pub offset: [i32; 2],
    /// bank-0 glyphs by character (ASCII: number < 256).
    pub glyphs: HashMap<u8, Glyph>,
}

impl FontMetrics {
    /// Look a character up, falling back to the other ASCII case (bitmap
    /// fonts ship one case only).
    pub fn glyph(&self, c: char) -> Option<Glyph> {
        lookup_glyph(&self.glyphs, c)
    }

    /// Advance of one character (font.go `CharWidth`): space → `Size[0]`,
    /// glyph → its width, missing → 0.
    pub fn char_width(&self, c: char) -> i32 {
        if c == ' ' {
            self.size[0] as i32
        } else {
            self.glyph(c).map(|g| g.w as i32).unwrap_or(0)
        }
    }

    /// Width of a string in pixels (font.go `TextWidth`, spacing included
    /// between glyphs, no trailing spacing).
    pub fn text_width(&self, text: &str) -> i32 {
        let mut w = 0i32;
        let mut any = false;
        for c in text.chars() {
            let cw = self.char_width(c);
            if cw + self.spacing[0] > 0 {
                w += cw + self.spacing[0];
                any = true;
            }
        }
        if any {
            w -= self.spacing[0];
        }
        w
    }
}

/// A loaded bitmap font.
#[derive(Debug, Clone)]
pub struct BitmapFont {
    pub metrics: FontMetrics,
    /// The font's own SFF, ready to be wrapped in a `SpriteCache`.
    pub sff: Sff,
    /// Where the def was loaded from (diagnostics).
    pub def_path: PathBuf,
}

impl BitmapFont {
    /// Split into the drawable metrics and the pixel source (the SFF moves
    /// into a `SpriteCache` owned by the renderer side).
    pub fn into_parts(self) -> (FontMetrics, Sff) {
        (self.metrics, self.sff)
    }
}

#[derive(Debug)]
pub enum FontError {
    Io(String),
    /// `[Def] Type = truetype` — TTF fonts come with the font raster milestone.
    Unsupported(String),
    NoSpriteFile,
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FontError::Io(e) => write!(f, "read: {e}"),
            FontError::Unsupported(t) => write!(f, "unsupported font type: {t}"),
            FontError::NoSpriteFile => write!(f, "[Def] has no File entry"),
        }
    }
}

/// Strip a UTF-8 BOM if present.
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn unquote(v: &str) -> String {
    let t = v.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// Glyph lookup with case fallback: bitmap fonts ship one case only
/// (f-6x9 is uppercase-only), so a missing case resolves to the other one.
fn lookup_glyph(glyphs: &HashMap<u8, Glyph>, c: char) -> Option<Glyph> {
    if !c.is_ascii() {
        return None;
    }
    let b = c as u8;
    glyphs
        .get(&b)
        .or_else(|| {
            let other = if b.is_ascii_uppercase() {
                b.to_ascii_lowercase()
            } else {
                b.to_ascii_uppercase()
            };
            glyphs.get(&other)
        })
        .copied()
}

impl BitmapFont {
    /// Load a bitmap FNT v2 from its `.def` path (the `File =` sff resolves
    /// against the def's directory, like Ikemen's font search).
    pub fn load(def_path: &Path) -> Result<BitmapFont, FontError> {
        let text = std::fs::read_to_string(def_path)
            .map_err(|e| FontError::Io(format!("{}: {e}", def_path.display())))?;
        BitmapFont::parse(&text, def_path)
    }

    /// Parse from def text (the def's own path is used for sff resolution).
    pub fn parse(text: &str, def_path: &Path) -> Result<BitmapFont, FontError> {
        let dir = def_path.parent().unwrap_or(Path::new(""));
        let mut name = String::new();
        let mut ty = String::new();
        let mut bank_type = String::new();
        let mut size = [0u16; 2];
        let mut spacing = [0i32; 2];
        let mut offset = [0i32; 2];
        let mut file: Option<String> = None;

        let mut section = String::new();
        let mut kv: HashMap<String, String> = HashMap::new();
        let mut flush = |section: &str, kv: &mut HashMap<String, String>| {
            match section.to_lowercase().as_str() {
                "fnt " => { // section_name splits at the first space: "FNT v2" → "fnt "
                    if let Some(v) = kv.get("name") {
                        name = unquote(v);
                    }
                }
                "def" => {
                    if let Some(v) = kv.get("type") {
                        ty = v.trim().to_lowercase();
                    }
                    if let Some(v) = kv.get("banktype") {
                        bank_type = v.trim().to_lowercase();
                    }
                    if let Some(v) = kv.get("size") {
                        let a: Vec<&str> = v.split(',').map(str::trim).collect();
                        if let Some(s) = a.first() {
                            if !s.is_empty() {
                                size[0] = atoi(s).max(0) as u16;
                            }
                        }
                        if let Some(s) = a.get(1) {
                            if !s.is_empty() {
                                size[1] = atoi(s).max(0) as u16;
                            }
                        }
                    }
                    if let Some(v) = kv.get("spacing") {
                        let p: Vec<i32> = v.split(',').map(|s| atoi(s.trim())).collect();
                        if let Some(x) = p.first() {
                            spacing[0] = *x;
                        }
                        if let Some(y) = p.get(1) {
                            spacing[1] = *y;
                        }
                    }
                    if let Some(v) = kv.get("offset") {
                        let p: Vec<i32> = v.split(',').map(|s| atoi(s.trim())).collect();
                        if let Some(x) = p.first() {
                            offset[0] = *x;
                        }
                        if let Some(y) = p.get(1) {
                            offset[1] = *y;
                        }
                    }
                    if let Some(v) = kv.get("file") {
                        file = Some(unquote(v));
                    }
                }
                _ => {}
            }
        };
        for line in strip_bom(text).lines() {
            if let Some((sec, _sub)) = section_name(line) {
                flush(&section, &mut kv);
                section = sec;
                kv.clear();
                continue;
            }
            let no_comment = line.split(';').next().unwrap_or("");
            let t = no_comment.trim();
            if t.is_empty() {
                continue;
            }
            if let Some(eq) = t.find('=') {
                let k = t[..eq].trim().to_lowercase();
                let v = t[eq + 1..].trim().to_string();
                if !k.is_empty() {
                    kv.insert(k, v);
                }
            }
        }
        flush(&section, &mut kv);

        if ty == "truetype" {
            return Err(FontError::Unsupported("truetype".into()));
        }
        let Some(file) = file else {
            return Err(FontError::NoSpriteFile);
        };
        let sff_path = dir.join(&file);
        let sff = Sff::load_path(&sff_path, SffType::Other, false)
            .map_err(|e| FontError::Io(format!("{}: {e}", sff_path.display())))?;

        // Glyph collection (font.go LoadFntSff): bank 0 for palette banks;
        // with BankType sprite every group is a bank — only bank 0 is used.
        let sprite_bank = bank_type == "sprite";
        let mut glyphs = HashMap::new();
        for e in sff.entries.iter() {
            if (e.group == 0 || sprite_bank) && e.number < 256 && e.width > 0 {
                glyphs.insert(
                    e.number as u8,
                    Glyph {
                        group: e.group,
                        number: e.number,
                        ofs: e.x,
                        w: e.width,
                    },
                );
            }
        }

        Ok(BitmapFont {
            metrics: FontMetrics {
                name,
                size,
                spacing,
                offset,
                glyphs,
            },
            sff,
            def_path: def_path.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_def_metrics_strips_bom_and_loads_sff() {
        // The def text is synthetic (BOM included) but `File` points at the
        // real fixture sff in the same directory, exercising the full path.
        let def_text = "\u{feff}; test font
[FNT v2]
fntversion = 2,00
name = \"test font\"

[Def]
Type = bitmap
BankType = palette
Size = 5,8
Spacing = 1,2
Offset = 0,0
File = default-3x5.sff
";
        let f = BitmapFont::parse(def_text, Path::new("tests/fixtures/test.def")).unwrap();
        assert_eq!(f.metrics.name, "test font");
        assert_eq!(f.metrics.size, [5, 8]);
        assert_eq!(f.metrics.spacing, [1, 2]);
        assert_eq!(f.metrics.offset, [0, 0]);
        assert!(!f.metrics.glyphs.is_empty());
    }

    #[test]
    fn truetype_font_is_unsupported_for_now() {
        let t = "[FNT v2]\nname = \"x\"\n[Def]\nType = truetype\nFile = a.ttf\n";
        assert!(matches!(
            BitmapFont::parse(t, Path::new("a.def")),
            Err(FontError::Unsupported(_))
        ));
    }

    #[test]
    fn missing_file_is_an_error() {
        let t = "[FNT v2]\nname = \"x\"\n[Def]\nType = bitmap\nSize = 5,8\n";
        assert!(matches!(
            BitmapFont::parse(t, Path::new("a.def")),
            Err(FontError::NoSpriteFile)
        ));
    }

    #[test]
    fn loads_real_fixture_font_with_glyphs() {
        let def = Path::new("tests/fixtures/default-3x5.def");
        let fnt = match BitmapFont::load(def) {
            Ok(f) => f,
            Err(e) => panic!("fixture font must load: {e}"),
        };
        let (f, sff) = fnt.into_parts();
        assert!(!sff.entries.is_empty());
        assert_eq!(f.size, [3, 6]);
        assert_eq!(f.spacing, [1, 0]);
        assert_eq!(f.offset, [0, -1]);
        assert!(!f.glyphs.is_empty(), "font has no glyphs");
        // A-Z must be present for a usable menu font.
        for c in b'A'..=b'Z' {
            assert!(f.glyphs.contains_key(&c), "missing glyph {c}");
        }
        // Space is not a sprite — it is synthesized from Size[0].
        assert!(!f.glyphs.contains_key(&(b' ')));
        // Width math: 'A' glyph width + spacing model must be self-consistent.
        let wa = f.char_width('A');
        assert!(wa > 0 && wa <= 10, "A width {wa}");
        // Text width: 3 chars + 2 spacings.
        let w = f.text_width("ABC");
        let expect = f.char_width('A') + f.spacing[0] + f.char_width('B') + f.spacing[0]
            + f.char_width('C');
        assert_eq!(w, expect);
        assert!(f.text_width("A B") > f.text_width("AB"));
    }

    #[test]
    fn case_fallback_resolves_missing_case() {
        // An uppercase-only font (like f-6x9): lowercase must resolve up.
        let mut glyphs = HashMap::new();
        for c in b'A'..=b'Z' {
            glyphs.insert(
                c,
                Glyph {
                    group: 0,
                    number: c as u16,
                    ofs: 0,
                    w: 5,
                },
            );
        }
        assert_eq!(lookup_glyph(&glyphs, 'a').unwrap().number, b'A' as u16);
        assert_eq!(lookup_glyph(&glyphs, 'Q').unwrap().number, b'Q' as u16);
        assert_eq!(lookup_glyph(&glyphs, 'q').unwrap().number, b'Q' as u16);
        assert!(lookup_glyph(&glyphs, '#').is_none());
        assert!(lookup_glyph(&glyphs, 'é').is_none());
    }
}
