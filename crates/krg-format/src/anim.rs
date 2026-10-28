//! AIR animation file compiler — faithful port of Ikemen GO `src/anim.go`
//! (`ReadAnimFrame`, `AnimCompiler`, `AnimationTable`, `resolveCopyAction`).
//!
//! Ported quirks (do not "fix"):
//! * Frame lines must start with a digit or `-` (anim.go:47).
//! * A frame with an invalid optional field is STILL appended with the
//!   fields parsed so far; only a warning is recorded (anim.go:371-378).
//! * Optional fields: blank means "keep default" (Ikemen behavior, not
//!   MUGEN 1.1's "zero") — anim.go:181-183.
//! * `clsn1default:`/`clsn2default:` set the carry-forward defaults;
//!   plain `clsn1:`/`clsn2:` apply to the next frame(s) until the next
//!   frame without a directive reverts to defaults (def1/def2, anim.go:380-393).
//! * Box `[n]` indices are honored when valid and unused, else the box goes
//!   into the first free slot (anim.go:515-537).
//! * Duplicate action numbers: first wins, even if it is a Copy Action
//!   (anim.go:1325-1333). Empty actions inherit the next action found in
//!   the file (recursion, anim.go:1334-1347).
//! * Copy Action chains resolve with a depth limit of 8; unresolved ones are
//!   purged from the table (anim.go:1380-1418).

use krg_core::{atoi, normalize_rect, section_name, TransType};
use std::collections::{HashMap, HashSet};

/// Port of `AnimFrame` (anim.go:12) + `newAnimFrame` defaults (anim.go:30).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimFrame {
    pub time: i32,
    pub group: i32,
    pub number: i32,
    pub xoffset: i32,
    pub yoffset: i32,
    pub trans_type: TransType,
    pub src_alpha: u8,
    pub dst_alpha: u8,
    /// Flip flags encoded as scale signs (Go: int8 1/-1).
    pub hscale: i8,
    pub vscale: i8,
    pub xscale: f32,
    pub yscale: f32,
    pub angle: f32,
    pub clsn1: Vec<[f32; 4]>,
    pub clsn2: Vec<[f32; 4]>,
}

impl Default for AnimFrame {
    fn default() -> Self {
        AnimFrame {
            time: -1,
            group: -1,
            number: -1,
            xoffset: 0,
            yoffset: 0,
            trans_type: TransType::None,
            src_alpha: 255,
            dst_alpha: 0,
            hscale: 1,
            vscale: 1,
            xscale: 1.0,
            yscale: 1.0,
            angle: 0.0,
            clsn1: Vec::new(),
            clsn2: Vec::new(),
        }
    }
}

/// Port of the compiled (static) fields of `Animation` (anim.go:212).
#[derive(Debug, Clone)]
pub struct Animation {
    pub frames: Vec<AnimFrame>,
    pub loopstart: i32,
    pub interpolate_offset: Vec<i32>,
    pub interpolate_scale: Vec<i32>,
    pub interpolate_angle: Vec<i32>,
    pub interpolate_blend: Vec<i32>,
    pub totaltime: i32,
    pub looptime: i32,
    pub prelooptime: i32,
    pub copy_action: i32,
    pub action_number: i32,
}

impl Default for Animation {
    fn default() -> Self {
        Animation {
            frames: Vec::new(),
            loopstart: 0,
            interpolate_offset: Vec::new(),
            interpolate_scale: Vec::new(),
            interpolate_angle: Vec::new(),
            interpolate_blend: Vec::new(),
            totaltime: 0,
            looptime: 0,
            prelooptime: 0,
            copy_action: -1,
            action_number: 0,
        }
    }
}

/// Port of `AnimationTable` (anim.go:1303).
#[derive(Debug, Clone, Default)]
pub struct AnimationTable {
    pub anims: HashMap<i32, Animation>,
    pub filename: String,
}

/// Warnings recorded while compiling (parity with Ikemen's log output).
#[derive(Debug, Clone, Default)]
pub struct AnimWarnings {
    pub messages: Vec<String>,
}

impl AnimWarnings {
    pub fn push(&mut self, msg: String) {
        self.messages.push(msg);
    }
}

/// Port of `ReadAnimFrame` (anim.go:45-210).
///
/// Returns `(frame, err)`: the frame is `Some` for any frame line — even one
/// with invalid optional fields (Go appends the partially-parsed frame and
/// keeps the error for logging).
pub fn read_anim_frame(line: &str) -> (Option<AnimFrame>, Option<String>) {
    // Valid lines must start with a number or '-' (anim.go:47).
    let b = line.as_bytes();
    if b.is_empty() || (b[0] < b'0' || b'9' < b[0]) && b[0] != b'-' {
        return (None, None);
    }
    let mut ary: Vec<&str> = line.splitn(10, ',').map(|s| s.trim()).collect();
    if ary.len() < 5 {
        return (None, Some("Invalid animation element definition".into()));
    }
    let mut af = AnimFrame::default();
    af.group = atoi(ary[0]);
    af.number = atoi(ary[1]);
    af.xoffset = atoi(ary[2]);
    af.yoffset = atoi(ary[3]);
    af.time = atoi(ary[4]);

    let mut err: Option<String> = None;

    // Flip flags (anim.go:73-86).
    if ary.len() >= 6 && !ary[5].is_empty() {
        for ch in ary[5].chars() {
            match ch {
                'H' | 'h' => {
                    af.hscale = -1;
                    af.xoffset = af.xoffset.wrapping_mul(-1);
                }
                'V' | 'v' => {
                    af.vscale = -1;
                    af.yoffset = af.yoffset.wrapping_mul(-1);
                }
                _ => {
                    err = Some(format!("Invalid animation element flip flag: {ch}"));
                }
            }
        }
    }

    // Transparency (anim.go:89-178).
    if ary.len() >= 7 && !ary[6].is_empty() {
        let raw = ary[6];
        if let Some(i) = raw.char_indices().find(|(_, c)| "ASas".contains(*c)).map(|(i, _)| i) {
            ary[6] = &raw[i..];
        }
        let a = ary[6].to_lowercase();

        // parseAlphaNumbers (anim.go:91-119) — Go sets `src` unconditionally
        // after the digit loop, so "sas" with no digits yields src=0 (quirk).
        #[allow(unused_assignments)]
        let parse_alpha = |s: &str, start: usize, def_src: u8, def_dst: u8| -> (u8, u8) {
            let (mut src, mut dst) = (def_src, def_dst);
            let sb = s.as_bytes();
            let mut i = start;
            let mut alp: u32 = 0;
            while i < sb.len() && sb[i].is_ascii_digit() {
                alp = alp * 10 + (sb[i] - b'0') as u32;
                i += 1;
            }
            alp &= 0x3fff;
            if alp > 255 {
                alp = 255;
            }
            src = alp as u8;
            if i < sb.len() && sb[i] == b'd' {
                i += 1;
                if i < sb.len() && sb[i].is_ascii_digit() {
                    alp = 0;
                    while i < sb.len() && sb[i].is_ascii_digit() {
                        alp = alp * 10 + (sb[i] - b'0') as u32;
                        i += 1;
                    }
                    alp &= 0x3fff;
                    if alp > 255 {
                        alp = 255;
                    }
                    dst = alp as u8;
                }
            }
            (src, dst)
        };

        let a_str = a.as_str();
        if a_str == "a1" {
            af.trans_type = TransType::Add;
            af.src_alpha = 255;
            af.dst_alpha = 128;
        } else if a_str.len() >= 3 && a_str.starts_with("sas") {
            af.trans_type = TransType::SubAdd;
            (af.src_alpha, af.dst_alpha) = parse_alpha(a_str, 3, 255, 255);
        } else if a_str.len() >= 2 && a_str.starts_with("sa") {
            af.trans_type = TransType::SubAdd;
            af.src_alpha = 255;
            af.dst_alpha = 255;
            if a_str.len() > 2 {
                err = Some(format!("Invalid animation element transparency: {a_str}"));
            }
        } else if a_str.len() >= 2 && a_str.starts_with("as") {
            af.trans_type = TransType::Add;
            (af.src_alpha, af.dst_alpha) = parse_alpha(a_str, 2, 255, 0); // MUGEN defaults to 0
        } else if a_str.len() >= 2 && a_str.starts_with("ss") {
            af.trans_type = TransType::Sub;
            (af.src_alpha, af.dst_alpha) = parse_alpha(a_str, 2, 255, 255);
        } else if !a_str.is_empty() && a_str.starts_with('a') {
            af.trans_type = TransType::Add;
            af.src_alpha = 255;
            af.dst_alpha = 255;
            if a_str.len() > 1 {
                err = Some(format!("Invalid animation element transparency: {a_str}"));
            }
        } else if !a_str.is_empty() && a_str.starts_with('s') {
            af.trans_type = TransType::Sub;
            af.src_alpha = 255;
            af.dst_alpha = 255;
            if a_str.len() > 1 {
                err = Some(format!("Invalid animation element transparency: {a_str}"));
            }
        } else {
            err = Some(format!("Invalid animation element transparency: {a_str}"));
        }
    }

    // X/Y scale and angle (anim.go:180-207): blank = keep default.
    let scale_field = |v: Option<&&str>, name: &str, err: &mut Option<String>| -> f32 {
        match v {
            Some(&s) if !s.is_empty() => {
                if krg_core::text::is_numeric(s) {
                    krg_core::text::atof(s) as f32
                } else {
                    *err = Some(format!("Invalid animation element {name}: {s}"));
                    1.0
                }
            }
            _ => 1.0,
        }
    };
    af.xscale = scale_field(ary.get(7), "x-scale", &mut err);
    af.yscale = scale_field(ary.get(8), "y-scale", &mut err);
    af.angle = match ary.get(9) {
        Some(&s) if !s.is_empty() => {
            if krg_core::text::is_numeric(s) {
                krg_core::text::atof(s) as f32
            } else {
                err = Some(format!("Invalid animation element angle: {s}"));
                0.0
            }
        }
        _ => 0.0,
    };

    (Some(af), err)
}

/// Port of the AIR compiler state machine (`AnimCompiler`, anim.go:278).
pub struct AnimCompiler {
    pub table: AnimationTable,
    pub log: bool,
    pub warnings: AnimWarnings,
    seen: HashSet<i32>,
    lines: Vec<String>,
    i: usize,
    filename: String,
    main_file: String,
    anim_no: i32,
    finalized: bool,
}

impl AnimCompiler {
    pub fn new(log: bool) -> Self {
        AnimCompiler {
            table: AnimationTable::default(),
            log,
            warnings: AnimWarnings::default(),
            seen: HashSet::new(),
            lines: Vec::new(),
            i: 0,
            filename: String::new(),
            main_file: String::new(),
            anim_no: 0,
            finalized: false,
        }
    }

    /// Port of `compileText` (anim.go:315) — Go's `SplitAndTrim(text, "\n")`.
    pub fn compile_text(&mut self, filename: &str, text: &str) {
        if self.finalized {
            if self.log {
                self.warnings.push(format!(
                    "WARNING: {filename} compiled after the table was resolved (ignored)"
                ));
            }
            return;
        }
        self.filename = filename.to_string();
        if self.main_file.is_empty() {
            self.main_file = filename.to_string();
        }
        self.seen.clear();
        self.lines = text.split('\n').map(|l| l.trim().to_string()).collect();
        self.i = 0;
        while self.compile_action().is_some() {}
    }

    /// Port of `finalize` (anim.go:333).
    pub fn finalize(mut self) -> AnimationTable {
        resolve_copy_action(&mut self.table);
        self.table.filename = self.main_file.clone();
        self.finalized = true;
        self.table
    }

    fn warnf(&mut self, msg: String) {
        if self.log {
            self.warnings.push(msg);
        }
    }

    /// Port of `compileAction` (anim.go:1315-1358).
    fn compile_action(&mut self) -> Option<Animation> {
        while self.i < self.lines.len() {
            let outer_no = self.anim_no;
            let read = self.read_action();
            self.anim_no = outer_no;
            match read {
                Some((no, a, aerr)) => {
                    if self.log {
                        if let Some(e) = aerr {
                            self.warnings.push(format!("WARNING: {e}"));
                        }
                    }
                    let existing = self.table.anims.get(&no).cloned();
                    if let Some(existing) = existing {
                        // First action for a duplicate number wins (anim.go:1327-1333).
                        if self.log && self.seen.contains(&no) {
                            self.warnf(format!(
                                "WARNING: Duplicate action key in {}: {} (ignored)",
                                self.filename.clone(), no
                            ));
                        }
                        self.seen.insert(no);
                        return Some(existing);
                    }
                    // Empty actions inherit the next action found
                    // (anim.go:1334-1347).
                    let mut a = a;
                    let mut logged = false;
                    while a.frames.is_empty() && self.i < self.lines.len() && a.copy_action < 0 {
                        if !logged && self.log {
                            let fname = self.filename.clone();
                            self.warnf(format!(
                                "WARNING: {}, action {}: no valid frames",
                                fname, no
                            ));
                            logged = true;
                        }
                        if let Some(a2) = self.compile_action() {
                            a = a2;
                            break;
                        }
                        self.i += 1;
                    }
                    self.table.anims.insert(no, a.clone());
                    self.seen.insert(no);
                    return Some(a);
                }
                None => {
                    self.i += 1;
                }
            }
        }
        None
    }

    /// Port of `readAction` (anim.go:567-592). Returns `(no, anim, err)`.
    fn read_action(&mut self) -> Option<(i32, Animation, Option<String>)> {
        let mut name = String::new();
        let mut subname = String::new();
        while self.i < self.lines.len() {
            if let Some((n, s)) = section_name(&self.lines[self.i]) {
                name = n;
                subname = s;
                break;
            }
            self.i += 1;
        }
        if name != "begin " {
            return None;
        }
        let spi = subname.find(' ')?;
        if subname[..spi + 1].to_lowercase() != "action " {
            return None;
        }
        self.i += 1;
        let no = atoi(&subname[spi + 1..]);
        self.anim_no = no;
        let (a, err) = self.read_animation();
        Some((no, a, err))
    }

    /// Port of `readAnimation` (anim.go:340-466).
    fn read_animation(&mut self) -> (Animation, Option<String>) {
        let mut a = Animation::default();
        a.action_number = self.anim_no;
        let mut err: Option<String> = None;
        let mut clsn1: Vec<[f32; 4]> = Vec::new();
        let mut clsn1d: Vec<[f32; 4]> = Vec::new();
        let mut clsn2: Vec<[f32; 4]> = Vec::new();
        let mut clsn2d: Vec<[f32; 4]> = Vec::new();
        let mut def1 = true;
        let mut def2 = true;
        let mut ols = 0i32;

        while self.i < self.lines.len() {
            let line_raw = self.lines[self.i].clone();
            if line_raw.starts_with('[') {
                break;
            }
            // Go: lower + trim + strip after ';' (anim.go:355-356).
            let line = line_raw.split(';').next().unwrap_or("").trim().to_lowercase();

            // Copy Action (anim.go:358-369): strict ParseInt, -1 on failure.
            if line.len() >= 12 && line.starts_with("copy action ") {
                let num_str = line[12..].trim();
                a.copy_action = match num_str.parse::<i32>() {
                    Ok(id) if !num_str.is_empty() => id,
                    _ => -1,
                };
                return (a, None);
            }

            let (frame, ferr) = read_anim_frame(&line);
            if let Some(lerr) = ferr {
                // anim.go:374-378: keep going; tag with file/action/line.
                err = Some(format!(
                    "{}, action {}, line {}: {}",
                    self.filename,
                    self.anim_no,
                    self.i + 1,
                    lerr
                ));
            }

            if let Some(mut af) = frame {
                // Clsn carry logic (anim.go:380-393).
                ols = a.loopstart;
                if def1 {
                    clsn1 = clsn1d.clone();
                }
                if def2 {
                    clsn2 = clsn2d.clone();
                }
                af.clsn1 = clsn1.clone();
                af.clsn2 = clsn2.clone();
                a.frames.push(af);
                def1 = true;
                def2 = true;
            } else {
                // Directives (anim.go:394-436). Go prefix-matches with length
                // guards exactly as written.
                let lb = line.as_bytes();
                if line.len() >= 9 && lb[..9] == *b"loopstart" {
                    a.loopstart = a.frames.len() as i32;
                } else if line.len() >= 18 && lb[..18] == *b"interpolate offset" {
                    a.interpolate_offset.push(a.frames.len() as i32);
                } else if line.len() >= 17 && lb[..17] == *b"interpolate scale" {
                    a.interpolate_scale.push(a.frames.len() as i32);
                } else if line.len() >= 17 && lb[..17] == *b"interpolate angle" {
                    a.interpolate_angle.push(a.frames.len() as i32);
                } else if line.len() >= 17 && lb[..17] == *b"interpolate blend" {
                    a.interpolate_blend.push(a.frames.len() as i32);
                } else if line.len() >= 5 && lb[..4] == *b"clsn" {
                    // Go (anim.go:404-435): guard checks `break` the SWITCH —
                    // the for-loop's `ac.i++` still runs, so the line is
                    // skipped. `readClsn` consumes lines itself (and its final
                    // `ac.i--` + the outer `ac.i++` balance out).
                    let handled = match line.find(':') {
                        Some(ii) if atoi(&line[ii + 1..]) >= 0 => {
                            let size = atoi(&line[ii + 1..]);
                            self.read_clsn_directive(&line, size, &mut err,
                                (&mut clsn1, &mut clsn1d, &mut def1),
                                (&mut clsn2, &mut clsn2d, &mut def2));
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        continue; // read_clsn advanced self.i past consumed lines
                    }
                }
            }
            self.i += 1;
        }

        // Tail (anim.go:439-464).
        if a.loopstart as usize >= a.frames.len() {
            a.loopstart = ols;
        }
        compute_times(&mut a);
        (a, err)
    }

    /// The `clsn` directive dispatch (anim.go:414-435). `readClsn` (below)
    /// consumes box lines and advances `self.i`; the caller must NOT run its
    /// own increment afterwards.
    fn read_clsn_directive(
        &mut self,
        line: &str,
        size: i32,
        err: &mut Option<String>,
        c1: (&mut Vec<[f32; 4]>, &mut Vec<[f32; 4]>, &mut bool),
        c2: (&mut Vec<[f32; 4]>, &mut Vec<[f32; 4]>, &mut bool),
    ) {
        let clsn_type = line.as_bytes()[4];
        // isDefault: `len(line) >= 12 && line[5:12] == "default"`
        // (matches "clsn1default: N" — the real MUGEN spelling).
        let is_default =
            line.len() >= 12 && line.as_bytes()[5..12] == *b"default";
        match clsn_type {
            b'1' => {
                match self.read_clsn(size) {
                    Ok(boxes) => {
                        *c1.0 = boxes.clone();
                        if is_default {
                            *c1.1 = boxes;
                        }
                        *c1.2 = false;
                    }
                    Err(e) => *err = Some(e),
                }
            }
            b'2' => {
                match self.read_clsn(size) {
                    Ok(boxes) => {
                        *c2.0 = boxes.clone();
                        if is_default {
                            *c2.1 = boxes;
                        }
                        *c2.2 = false;
                    }
                    Err(e) => *err = Some(e),
                }
            }
            _ => {}
        }
    }

    /// Port of `readClsn` (anim.go:468-565). Advances `self.i` past consumed
    /// lines and leaves it one before the first non-box line (Go's `ac.i--`).
    fn read_clsn(&mut self, size: i32) -> Result<Vec<[f32; 4]>, String> {
        let size = size as usize;
        let mut clsn = vec![[0f32; 4]; size];
        if size == 0 {
            self.i += 1; // move past the "clsn:" header line
            return Ok(clsn);
        }
        let mut filled = vec![false; size];
        let line_no = self.i + 1;
        self.i += 1; // past the header line (anim.go:479)
        let mut count = 0usize;
        while count < size && self.i < self.lines.len() {
            let rline = self.lines[self.i]
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if rline.is_empty() {
                self.i += 1;
                continue;
            }
            let rb = rline.as_bytes();
            if rb.len() < 4 || rb[..4] != *b"clsn" {
                break;
            }
            let Some(eq_idx) = rline.find('=') else {
                break;
            };
            // Optional [n] between the type and "=" (anim.go:500-514).
            let mut box_idx: i64 = -1;
            if let Some(br_start) = rline.find('[') {
                if br_start < eq_idx {
                    if let Some(rel_end) = rline[br_start..].find(']') {
                        if rel_end > 0 {
                            let idx_str = rline[br_start + 1..br_start + rel_end].trim();
                            let valid = !idx_str.is_empty()
                                && idx_str.bytes().all(|c| c.is_ascii_digit());
                            if valid {
                                box_idx = atoi(idx_str) as i64;
                            }
                        }
                    }
                }
            }
            // Invalid/duplicate index → first free slot (anim.go:515-537).
            if box_idx < 0 || box_idx as usize >= size || filled[box_idx as usize] {
                let requested = box_idx;
                box_idx = -1;
                for (k, f) in filled.iter().enumerate() {
                    if !f {
                        box_idx = k as i64;
                        break;
                    }
                }
                if box_idx < 0 {
                    if self.log {
                        self.warnings.push(format!(
                            "WARNING: line {line_no}: Clsn no free slot (size {size}); box dropped"
                        ));
                    }
                    self.i += 1;
                    continue;
                }
                if requested >= 0 && self.log {
                    self.warnings.push(format!(
                        "WARNING: line {line_no}: Clsn[{}] index is duplicate or out of range; placed in slot [{}]",
                        requested, box_idx
                    ));
                }
            }
            // Coordinates after "=" (anim.go:539-548).
            let coords: Vec<&str> = rline[eq_idx + 1..].split(',').collect();
            if coords.len() < 4 {
                break;
            }
            clsn[box_idx as usize] = normalize_rect([
                atoi(coords[0]) as f32,
                atoi(coords[1]) as f32,
                atoi(coords[2]) as f32,
                atoi(coords[3]) as f32,
            ]);
            filled[box_idx as usize] = true;
            count += 1;
            self.i += 1;
        }
        self.i -= 1; // back up one step (anim.go:554)

        if count != size {
            return Err(format!(
                "{}, action {}, line {}: Clsn: declared {} boxes but only {} found",
                self.filename, self.anim_no, line_no, size, count
            ));
        }
        Ok(clsn)
    }
}

/// Port of the time bookkeeping tail of `readAnimation` (anim.go:439-464).
fn compute_times(a: &mut Animation) {
    if a.frames.is_empty() {
        return;
    }
    if a.frames.last().map(|f| f.time) == Some(-1) {
        a.totaltime = -1;
        return;
    }
    let mut tmp = 0i32;
    let mut total = 0i32;
    let mut looptime = 0i32;
    let mut prelooptime = 0i32;
    for (i, f) in a.frames.iter().enumerate() {
        if f.time == -1 {
            total = 0;
            looptime = -tmp;
            prelooptime = 0;
        }
        total = total.wrapping_add(f.time);
        if i < a.loopstart as usize {
            prelooptime = prelooptime.wrapping_add(f.time);
            tmp = tmp.wrapping_add(f.time);
        } else {
            looptime = looptime.wrapping_add(f.time);
        }
    }
    a.totaltime = total;
    a.looptime = looptime;
    a.prelooptime = prelooptime;
    if a.totaltime == -1 {
        a.prelooptime = 0;
    }
}

/// Port of `resolveCopyAction` (anim.go:1380-1418): chains of Copy Action are
/// followed up to 8 deep; unresolved actions are purged.
fn resolve_copy_action(table: &mut AnimationTable) {
    let keys: Vec<i32> = table.anims.keys().copied().collect();
    let mut to_delete: Vec<i32> = Vec::new();
    for no in keys {
        let mut loops = 0u32;
        while table.anims[&no].copy_action >= 0 && loops < 8 {
            let ca = table.anims[&no].copy_action;
            let Some(target) = table.anims.get(&ca) else { break };
            if ca == no {
                break; // self-reference
            }
            if target.copy_action >= 0 {
                // Jump to the next link in the chain (anim.go:1395-1398).
                let next = target.copy_action;
                table.anims.get_mut(&no).unwrap().copy_action = next;
            } else {
                // Shallow copy of the resolved target (anim.go:1400-1404).
                let mut t = target.clone();
                t.copy_action = -1;
                table.anims.insert(no, t);
            }
            loops += 1;
        }
        // If Copy Action could not be resolved, drop the action (anim.go:1408-1411).
        if table.anims[&no].copy_action >= 0 {
            to_delete.push(no);
        }
    }
    for no in to_delete {
        table.anims.remove(&no);
    }
}
