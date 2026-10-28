//! Stage DEF parser — the subset of Ikemen GO `src/stage.go` needed to
//! compose the visible scene: `[Info]`/`[Camera]` (stored, used later),
//! `[PlayerInfo]` start positions, `[StageInfo]` ground + localcoord,
//! `[BGdef]` sprite file and the `[BG ...]` elements.
//!
//! Scope notes (v0.8, camera is static at (0,0), no zoom):
//! * `type = normal` is fully handled; `parallax` is kept with its
//!   `xscale`/`width` pair (the renderer approximates the trapezoid with the
//!   bottom scale); `anim` elements fall back to their first frame when the
//!   stage DEF embeds `[Begin Action]` data (parsed by `krg_format::anim`,
//!   passed in by the caller); `dummy`/`video` are skipped.
//! * `velocity`, `sin.x/sin.y`, bgctrl, zoomdelta and `window` clipping are
//!   parsed-but-ignored or rejected-with-no-error — the fight camera that
//!   drives them does not exist yet.
//! * Coordinate model (stage.go `drawTransformed`, `rcx = gameWidth/2`):
//!   element x is **relative to screen center** (160 at localcoord 320),
//!   y from the top; with the camera parked at (0,0) the screen position is
//!   simply `(160 + start.x, start.y)` — the formula the scene uses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use krg_core::{atof, atoi, section_name};

/// Background element type (`type =`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BgType {
    #[default]
    Normal,
    Parallax,
    Anim,
    Dummy,
    Video,
}

/// Element transparency (`trans =`), matching Ikemen's TT_ mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BgTrans {
    #[default]
    None,
    Add,
    Add1,
    AddAlpha,
    Sub,
    SubAdd,
}

/// One `[BG ...]` element. Defaults follow the MUGEN spec comments in the
/// kfm.def template (start 0,0; delta 1,1; trans none; tile 0,0).
#[derive(Debug, Clone)]
pub struct BgElement {
    /// Element name from `[BG <name>]` (diagnostics only).
    pub name: String,
    pub ty: BgType,
    pub group: u16,
    pub number: u16,
    /// `start = x, y` — x relative to screen center, y from top.
    pub start: [f32; 2],
    /// `delta = x, y` — parallax scroll factor per camera unit.
    pub delta: [f32; 2],
    pub trans: BgTrans,
    /// `alpha = src, dst` (0..=255, for add/sub/addalpha).
    pub alpha: [u8; 2],
    /// `tile = x, y` — 0 off, 1 infinite, n>1 that many copies.
    pub tile: [i32; 2],
    /// `tilespacing = x, y` — added to the sprite size (0,0 = adjacent).
    pub tilespacing: [i32; 2],
    /// `layerno` — 0 behind the fighters, 1 in front.
    pub layerno: i32,
    /// `actionno` (`anim =`) — 0 = static `spriteno`.
    pub actionno: i32,
    /// Parallax `xscale = top, bottom`.
    pub parallax_xscale: [f32; 2],
    /// Parallax `width = top, bottom` (pixel widths; overrides xscale).
    pub parallax_width: [i32; 2],
}

impl Default for BgElement {
    fn default() -> Self {
        BgElement {
            name: String::new(),
            ty: BgType::Normal,
            group: 0,
            number: 0,
            start: [0.0, 0.0],
            delta: [1.0, 1.0],
            trans: BgTrans::None,
            alpha: [255, 0],
            tile: [0, 0],
            tilespacing: [0, 0],
            layerno: 0,
            actionno: 0,
            parallax_xscale: [1.0, 1.0],
            parallax_width: [0, 0],
        }
    }
}

/// A parsed stage definition.
#[derive(Debug, Clone)]
pub struct StageDef {
    pub name: String,
    /// `[StageInfo] zoffset` — ground y in localcoord pixels.
    pub zoffset: i32,
    /// `[StageInfo] localcoord = w, h`.
    pub localcoord: [u32; 2],
    /// `[Camera]` scroll bounds (stored for the future fight camera).
    pub boundleft: i32,
    pub boundright: i32,
    pub boundhigh: i32,
    /// `[Camera] verticalfollow` — jump follow factor (kfm.def: 0.2)
    pub verticalfollow: f32,
    /// `[Camera] floortension` — follow after this height above ground
    pub floortension: i32,
    /// `[PlayerInfo]` — x relative to screen center, y from top.
    pub p1start: [f32; 2],
    pub p1facing: f32,
    pub p2start: [f32; 2],
    pub p2facing: f32,
    /// `[BGdef] spr` resolved against the DEF's directory.
    pub sff_path: PathBuf,
    /// Elements in file order (painter's order within a layer).
    pub bg: Vec<BgElement>,
}

impl Default for StageDef {
    fn default() -> Self {
        StageDef {
            name: String::new(),
            zoffset: 200,
            localcoord: [320, 240],
            boundleft: -150,
            boundright: 150,
            boundhigh: -25,
            verticalfollow: 0.2,
            floortension: 0,
            p1start: [-70.0, 0.0],
            p1facing: 1.0,
            p2start: [70.0, 0.0],
            p2facing: -1.0,
            sff_path: PathBuf::new(),
            bg: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum StageError {
    Io(String),
    NoSpriteFile,
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StageError::Io(e) => write!(f, "read: {e}"),
            StageError::NoSpriteFile => write!(f, "[BGdef] has no spr entry"),
        }
    }
}

/// Split a `key = value` line into trimmed lowercase key and trimmed value.
/// Comment lines (`;`) and malformed lines yield None.
fn key_value(line: &str) -> Option<(String, String)> {
    let no_comment = line.split(';').next().unwrap_or("");
    let t = no_comment.trim();
    if t.is_empty() {
        return None;
    }
    let eq = t.find('=')?;
    let k = t[..eq].trim().to_lowercase();
    let v = t[eq + 1..].trim().to_string();
    if k.is_empty() {
        return None;
    }
    Some((k, v))
}

/// Strip a UTF-8 BOM if present (DEF files from Windows tools carry one).
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// `value.split(',')` pieces trimmed, padded with "" to `n` (IniSection
/// SplitAndTrim semantics: missing values are simply absent).
fn csv<'a>(v: &'a str, n: usize) -> Vec<&'a str> {
    let mut out: Vec<&str> = v.split(',').map(str::trim).collect();
    out.resize(n, "");
    out
}

fn f32_at(v: &str, i: usize) -> f32 {
    atof(csv(v, i + 1)[i]) as f32
}

fn i32_at(v: &str, i: usize) -> i32 {
    atoi(csv(v, i + 1)[i])
}

impl StageDef {
    /// Parse a stage DEF. `text` is the file's contents; `dir` is the DEF's
    /// directory (the `spr =` file resolves against it).
    pub fn parse(text: &str, dir: &Path) -> Result<StageDef, StageError> {
        let mut def = StageDef::default();
        let mut bgdef_spr: Option<String> = None;
        let mut section: String = String::new();
        let mut subname: String = String::new();
        let mut kv: HashMap<String, String> = HashMap::new();

        let finish_section = |section: &str,
                                  subname: &str,
                                  kv: &mut HashMap<String, String>,
                                  def: &mut StageDef,
                                  spr: &mut Option<String>| {
            let sec = section.to_lowercase();
            match sec.as_str() {
                "info" => {
                    if let Some(v) = kv.get("name") {
                        def.name = unquote(v);
                    }
                }
                "camera" => {
                    if let Some(v) = kv.get("boundleft") {
                        def.boundleft = atoi(v);
                    }
                    if let Some(v) = kv.get("boundright") {
                        def.boundright = atoi(v);
                    }
                    if let Some(v) = kv.get("boundhigh") {
                        def.boundhigh = atoi(v);
                    }
                    if let Some(v) = kv.get("verticalfollow") {
                        def.verticalfollow = f32_at(v, 0);
                    }
                    if let Some(v) = kv.get("floortension") {
                        def.floortension = atoi(v);
                    }
                }
                "playerinfo" => {
                    if let Some(v) = kv.get("p1startx") {
                        def.p1start[0] = f32_at(v, 0);
                    }
                    if let Some(v) = kv.get("p1starty") {
                        def.p1start[1] = f32_at(v, 0);
                    }
                    if let Some(v) = kv.get("p1facing") {
                        def.p1facing = f32_at(v, 0);
                    }
                    if let Some(v) = kv.get("p2startx") {
                        def.p2start[0] = f32_at(v, 0);
                    }
                    if let Some(v) = kv.get("p2starty") {
                        def.p2start[1] = f32_at(v, 0);
                    }
                    if let Some(v) = kv.get("p2facing") {
                        def.p2facing = f32_at(v, 0);
                    }
                }
                "stageinfo" => {
                    if let Some(v) = kv.get("zoffset") {
                        def.zoffset = atoi(v);
                    }
                    if let Some(v) = kv.get("localcoord") {
                        def.localcoord = [i32_at(v, 0).max(0) as u32, i32_at(v, 1).max(0) as u32];
                    }
                }
                "bgdef" => {
                    if let Some(v) = kv.get("spr") {
                        *spr = Some(unquote(v));
                    }
                }
                s if s == "bg " || s.starts_with("bg ") || s == "bg" => {
                    // "[BG name]" → section_name yields ("bg ", name).
                    let mut bg = BgElement::default();
                    bg.name = subname.trim().to_string();
                    if let Some(v) = kv.get("type") {
                        bg.ty = match v.to_lowercase().as_str() {
                            "normal" => BgType::Normal,
                            "parallax" => BgType::Parallax,
                            "anim" => BgType::Anim,
                            "dummy" => BgType::Dummy,
                            "video" => BgType::Video,
                            _ => BgType::Normal,
                        };
                    }
                    if let Some(v) = kv.get("spriteno") {
                        let g = i32_at(v, 0);
                        let n = i32_at(v, 1);
                        if g >= 0 && n >= 0 {
                            bg.group = g as u16;
                            bg.number = n as u16;
                        }
                    }
                    if let Some(v) = kv.get("start") {
                        bg.start = [f32_at(v, 0), f32_at(v, 1)];
                    }
                    if let Some(v) = kv.get("delta") {
                        bg.delta = [f32_at(v, 0), f32_at(v, 1)];
                    }
                    if let Some(v) = kv.get("trans") {
                        bg.trans = match v.to_lowercase().as_str() {
                            "add" => BgTrans::Add,
                            "add1" => BgTrans::Add1,
                            "addalpha" => BgTrans::AddAlpha,
                            "sub" => BgTrans::Sub,
                            "subadd" => BgTrans::SubAdd,
                            _ => BgTrans::None,
                        };
                    }
                    if let Some(v) = kv.get("alpha") {
                        bg.alpha = [
                            atoi(&csv(v, 1)[0]).clamp(0, 255) as u8,
                            atoi(&csv(v, 2)[1]).clamp(0, 255) as u8,
                        ];
                    }
                    if let Some(v) = kv.get("tile") {
                        bg.tile = [i32_at(v, 0), i32_at(v, 1)];
                        // Ikemen: negative tile = infinite.
                        for t in &mut bg.tile {
                            if *t < 0 {
                                *t = i32::MAX;
                            }
                        }
                    }
                    if let Some(v) = kv.get("tilespacing") {
                        bg.tilespacing = [i32_at(v, 0), i32_at(v, 1)];
                    }
                    if let Some(v) = kv.get("layerno") {
                        bg.layerno = atoi(v).clamp(0, 1);
                    }
                    if let Some(v) = kv.get("actionno") {
                        bg.actionno = atoi(v);
                    } else if let Some(v) = kv.get("anim") {
                        bg.actionno = atoi(v);
                        if bg.actionno > 0 {
                            bg.ty = BgType::Anim;
                        }
                    }
                    if let Some(v) = kv.get("xscale") {
                        bg.parallax_xscale = [f32_at(v, 0), f32_at(v, 1)];
                    }
                    if let Some(v) = kv.get("width") {
                        bg.parallax_width = [i32_at(v, 0), i32_at(v, 1)];
                    }
                    if bg.ty != BgType::Dummy && bg.ty != BgType::Video {
                        def.bg.push(bg);
                    }
                }
                _ => {}
            }
        };

        for line in strip_bom(text).lines() {
            if let Some((name, sub)) = section_name(line) {
                finish_section(&section, &subname, &mut kv, &mut def, &mut bgdef_spr);
                section = name;
                subname = sub;
                kv.clear();
                continue;
            }
            if let Some((k, v)) = key_value(line) {
                kv.insert(k, v);
            }
        }
        finish_section(&section, &subname, &mut kv, &mut def, &mut bgdef_spr);

        let Some(spr) = bgdef_spr else {
            return Err(StageError::NoSpriteFile);
        };
        def.sff_path = dir.join(spr);
        Ok(def)
    }

    /// Load and parse from a path.
    pub fn load_path(path: &Path) -> Result<StageDef, StageError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| StageError::Io(format!("{}: {e}", path.display())))?;
        StageDef::parse(&text, path.parent().unwrap_or(Path::new("")))
    }
}

/// Strip paired double quotes around a value.
fn unquote(v: &str) -> String {
    let t = v.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KFM_DEF: &str = r#"; KFM stage (test excerpt)
[Info]
name = "Mountainside Temple"
mugenversion = 1.1

[Camera]
boundleft = -150
boundright = 150
boundhigh = -22

[PlayerInfo]
p1startx = -70
p1starty = 0
p1facing = 1
p2startx = 70
p2starty = 0
p2facing = -1

[StageInfo]
zoffset = 200
localcoord = 320, 240

[Shadow]
intensity = 64

[BGdef]
spr = kfm.sff
debugbg = 0

[Begin Action 9000]
9000,1, 0,0,-1

[BG 0]
type  = normal
spriteno = 0, 0
layerno = 0
start = 0, 0
delta = .5,.5
trans = none
mask  = 0
tile  = 0, 0

[BG Floor]
type  = parallax
spriteno = 10,0
start = 0, 181
delta = .78, .75
xscale = 1, 1.75

[BG Wall reflection on floor]
type  = normal
spriteno = 1,1
trans = addalpha
alpha = 128,128
start = 0, 239
delta = .8, .75
mask = 1
tile = 1, 0

[BG Foreground]
type = dummy
spriteno = 99,99
layerno = 1
"#;

    #[test]
    fn parses_kfm_stage_sections() {
        let def = StageDef::parse(KFM_DEF, Path::new("stages")).unwrap();
        assert_eq!(def.name, "Mountainside Temple");
        assert_eq!(def.zoffset, 200);
        assert_eq!(def.localcoord, [320, 240]);
        assert_eq!((def.boundleft, def.boundright, def.boundhigh), (-150, 150, -22));
        assert_eq!(def.p1start, [-70.0, 0.0]);
        assert_eq!(def.p2start, [70.0, 0.0]);
        assert_eq!((def.p1facing, def.p2facing), (1.0, -1.0));
        assert_eq!(def.sff_path, Path::new("stages/kfm.sff"));

        // dummy is dropped, 5 real elements survive... (4 here + none)
        assert_eq!(def.bg.len(), 3);
        let bg0 = &def.bg[0];
        assert_eq!(bg0.name, "0");
        assert_eq!(bg0.ty, BgType::Normal);
        assert_eq!((bg0.group, bg0.number), (0, 0));
        assert_eq!(bg0.start, [0.0, 0.0]);
        assert_eq!(bg0.delta, [0.5, 0.5]);
        assert_eq!(bg0.trans, BgTrans::None);

        let floor = &def.bg[1];
        assert_eq!(floor.name, "Floor");
        assert_eq!(floor.ty, BgType::Parallax);
        assert_eq!(floor.start, [0.0, 181.0]);
        assert_eq!(floor.parallax_xscale, [1.0, 1.75]);

        let refl = &def.bg[2];
        assert_eq!(refl.trans, BgTrans::AddAlpha);
        assert_eq!(refl.alpha, [128, 128]);
        assert_eq!(refl.tile, [1, 0]);
    }

    #[test]
    fn begin_action_lines_are_not_kv() {
        // "9000,1, 0,0,-1" has no '=' — must not poison any section.
        let def = StageDef::parse(KFM_DEF, Path::new("stages")).unwrap();
        assert!(def.bg.iter().all(|b| b.group != 9000));
    }

    #[test]
    fn missing_spr_is_an_error() {
        let text = "[Info]\nname = \"x\"\n[BG 0]\nspriteno = 0,0\n";
        assert!(matches!(
            StageDef::parse(text, Path::new(".")),
            Err(StageError::NoSpriteFile)
        ));
    }

    #[test]
    fn defaults_follow_the_template() {
        let text = "[BGdef]\nspr = x.sff\n[BG a]\nspriteno = 1,2\n";
        let def = StageDef::parse(text, Path::new("s")).unwrap();
        assert_eq!(def.zoffset, 200); // StageInfo absent
        let bg = &def.bg[0];
        assert_eq!(bg.delta, [1.0, 1.0]);
        assert_eq!(bg.start, [0.0, 0.0]);
        assert_eq!(bg.tile, [0, 0]);
        assert_eq!(bg.trans, BgTrans::None);
        assert_eq!(bg.layerno, 0);
        assert_eq!((bg.group, bg.number), (1, 2));
    }

    #[test]
    fn bom_and_quoted_name_survive() {
        let text = "\u{feff}[Info]\nname = \"BOM Stage\"\n[BGdef]\nspr=b.sff\n";
        let def = StageDef::parse(text, Path::new(".")).unwrap();
        assert_eq!(def.name, "BOM Stage");
    }
}
