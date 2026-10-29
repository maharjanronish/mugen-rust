//! The v0.8 scene: **menu → fight demo**, rendered end-to-end through the
//! real stack — SFF parsing, AIR timing, stage DEF composition, FNT v2 text,
//! LRU texture streaming, batched GLES 3.0 drawing.
//!
//! This is the first screen that *looks like* a fighting game:
//! * **Menu** — stage dimmed behind, title + four items in the bundled
//!   bitmap font, navigated with the touch controller (dpad up/down, A or
//!   START to select). A placeholder until the motif-driven menus arrive
//!   with the DEF/INI + script milestones: every item currently enters the
//!   same fight demo (the CNS/CMD VM that separates the modes is next).
//! * **Fight** — the stage's layer-0 background elements, both fighters at
//!   the stage's start positions facing each other, layer-1 foreground, and
//!   a corner hint. B returns to the menu.
//!
//! When the char folder is missing the app stays on the setup panel; a
//! missing stage just means a black background; a missing font skips the
//! menu (fight starts directly).

use std::path::{Path, PathBuf};
use std::time::Instant;

use krg_core::TransType;
use krg_format::{
    AnimCompiler, Animation, BgTrans, BgType, BitmapFont, FontMetrics, SpriteCache, Sff, SffType,
    StageDef,
};
use krg_gfx::{Quad, Renderer, BLENDS_ADD, BLENDS_ALPHA};
use krg_input::bits;
use krg_vm::runtime::{compile_char, Fight, GlobalInfo};

/// Texture-LRU source slots (one renderer LRU, several SFFs).
const SRC_STAGE: u32 = 0;
const SRC_CHAR: u32 = 1;
const SRC_FONT: u32 = 2;

/// AIR time is 60 Hz ticks (anim.go: `TickTime` = 60/s).
const TICK_MS: f64 = 1000.0 / 60.0;
/// CPU-side decoded-pixel budgets (stage / chars / font).
const CPU_STAGE_BUDGET: usize = 16 * 1024 * 1024;
const CPU_CHAR_BUDGET: usize = 12 * 1024 * 1024;
const CPU_FONT_BUDGET: usize = 2 * 1024 * 1024;

const GAME_W: f32 = 320.0;
const GAME_H: f32 = 240.0;

/// Placeholder menu — the mode logic behind each item lands with the CNS/CMD
/// VM milestone.
const MENU_ITEMS: [&str; 4] = ["ARCADE", "VS MODE", "TRAINING", "OPTIONS"];

/// Version shown in the menu corner — keep in sync with
/// android/app/build.gradle.kts `versionName`.
const VERSION: &str = "0.9.0";

const WHITE: [u8; 4] = [255, 255, 255, 255];
const GRAY: [u8; 4] = [160, 160, 160, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

/// One fighter's placement (both share the kfm anim: standing, standing…).
struct Fighter {
    /// Axis x in screen space (stage x + 160).
    x: f32,
    /// Axis y (stage zoffset, scaled to 320×240).
    y: f32,
    /// +1 faces right, −1 faces left (mirrors the sprite).
    facing: f32,
}

struct StageRun {
    def: StageDef,
    cache: SpriteCache,
    /// 320 / localcoord — HiRes stages (640×480 etc.) shrink to fit.
    scl: f32,
}

struct FontRun {
    metrics: FontMetrics,
    cache: SpriteCache,
}

enum Mode {
    Menu,
    Fight,
}

/// The live fight runtime (krg-vm) + its compiled character data. When the
/// VM files can't be assembled the scene falls back to the static demo.
struct FightRun {
    fight: Fight,
    /// 60Hz tick accumulator (ms).
    tick_acc: f64,
    /// P2 CPU (input.go:1941 AiInput) — jam-based button/dir generator
    ai: krg_vm::cmd::AiInput,
    ai_rng: krg_vm::cmd::LehmerRng,
    ai_level: f32,
}

impl FightRun {
    /// Assemble the compiled char from a content root: the char folder's
    /// def → cmd/st/air, plus the bundled common files (common.cmd merged
    /// into cmd text per compiler.go:8628, common.const, common.air and
    /// stcommon → common1.cns.zss via the fallback).
    fn load(root: &Path) -> Option<FightRun> {
        let char_dir = find_char_dir(root)?;
        let def_path = def_of(&char_dir)?;
        let def_text = read_text(&def_path)?;
        let files = krg_vm::ini::read_def_files(&def_text);

        let read = |rel: &str| -> Option<String> {
            // resolve relative to the char dir, then the content root
            let p = char_dir.join(rel);
            if p.is_file() {
                return read_text(&p);
            }
            let p = root.join(rel);
            if p.is_file() {
                return read_text(&p);
            }
            None
        };

        let cmd = files.get("cmd").and_then(|c| read(c))?;
        // common.cmd appended after the char's own (compiler.go:8628)
        let cmd = match read("data/common.cmd") {
            Some(cc) => format!("{cmd}\n{cc}"),
            None => cmd,
        };
        let mut cns_texts = Vec::new();
        for key in ["st", "st0", "st1", "st2", "st3", "st4", "st5", "st6"] {
            if let Some(t) = files.get(key).and_then(|v| read(v)) {
                cns_texts.push(t);
            }
        }
        if cns_texts.is_empty() {
            return None;
        }
        // stcommon: common1.cns → falls back to the .zss (7181-7195)
        let stcommon = files
            .get("stcommon")
            .and_then(|v| read(v))
            .or_else(|| read("data/common1.cns.zss"));
        let common_const = read("data/common.const")?;
        let kfm_air = files.get("anim").and_then(|v| read(v))?;
        let common_air = read("data/common.air").unwrap_or_default();

        let gi = compile_char(
            "kfm",
            &cmd,
            &cns_texts,
            stcommon.as_deref(),
            &common_const,
            &[common_air, kfm_air],
        )
        .map_err(|e| log::warn!("scene: char compile: {e}"))
        .ok()?;
        let fight = Fight::new_1v1(gi, 42);
        Some(FightRun {
            fight,
            tick_acc: 0.0,
            ai: krg_vm::cmd::AiInput::default(),
            ai_rng: krg_vm::cmd::LehmerRng::new(4242),
            ai_level: 3.0,
        })
    }
}

pub struct Scene {
    mode: Mode,
    cursor: usize,
    stage: Option<StageRun>,
    font: Option<FontRun>,

    // kfm standing animation (shared by both fighters for now).
    char_cache: SpriteCache,
    anim: Animation,
    /// Per-frame durations in ms (time clamped to ≥ 1 tick).
    durations: Vec<f64>,
    /// Cumulative start time of each frame (ms since anim start).
    starts: Vec<f64>,
    /// Frame index the loop returns to (AIR loopstart, clamped).
    loop_from: usize,
    /// Total length of the looping window (ms).
    loop_ms: f64,
    /// Master clock (ms, clamped advance).
    t_ms: f64,
    last: Instant,

    fighters: [Fighter; 2],
    prev_input: u32,
    /// Live krg-vm fight (None → static standing demo).
    fight: Option<FightRun>,
}

impl Scene {
    /// Load the scene from a content root: the bundled kfm (or first char
    /// folder), the kfm stage (or first stages/*.def) and the menu font.
    pub fn load(root: &Path) -> Option<Scene> {
        let sff_path = find_char_sff(root)?;
        let air_path = sff_path.with_extension("air");

        let sff = Sff::load_path(&sff_path, SffType::Char, false)
            .map_err(|e| log::warn!("scene: {}: {e}", sff_path.display()))
            .ok()?;
        let air_text = std::fs::read_to_string(&air_path)
            .map_err(|e| log::warn!("scene: {}: {e}", air_path.display()))
            .ok()?;
        let mut ac = AnimCompiler::new(false);
        ac.compile_text(&air_path.display().to_string(), &air_text);
        let anims = ac.finalize();
        let anim = anims
            .anims
            .get(&0)
            .or_else(|| anims.anims.values().next())?
            .clone();
        if anim.frames.is_empty() {
            return None;
        }

        let mut durations = Vec::with_capacity(anim.frames.len());
        for f in &anim.frames {
            let ticks = if f.time > 0 { f.time } else { 1 } as f64;
            durations.push(ticks * TICK_MS);
        }
        let mut starts = Vec::with_capacity(durations.len());
        let mut acc = 0.0;
        for d in &durations {
            starts.push(acc);
            acc += d;
        }
        let loop_from = (anim.loopstart.max(0) as usize).min(durations.len() - 1);
        let loop_ms: f64 = durations[loop_from..].iter().sum();

        let stage = find_stage(root);
        let font = find_font(root);
        let mut fight = FightRun::load(root);
        // stage camera bounds (StageDef [Camera], scaled to 320-space)
        if let (Some(fr), Some(st)) = (fight.as_mut(), stage.as_ref()) {
            fr.fight.set_stage_camera(
                st.def.boundleft as f32 * st.scl,
                st.def.boundright as f32 * st.scl,
                st.def.boundhigh as f32 * st.scl,
                st.def.verticalfollow,
                st.def.floortension as f32,
            );
        }

        // Fighter placement: stage data when present, template defaults.
        let (p1, p2) = match &stage {
            Some(s) => {
                let scl = s.scl;
                (
                    Fighter {
                        x: GAME_W / 2.0 + s.def.p1start[0] * scl,
                        y: s.def.zoffset as f32 * scl,
                        facing: if s.def.p1facing < 0.0 { -1.0 } else { 1.0 },
                    },
                    Fighter {
                        x: GAME_W / 2.0 + s.def.p2start[0] * scl,
                        y: s.def.zoffset as f32 * scl,
                        facing: if s.def.p2facing < 0.0 { -1.0 } else { 1.0 },
                    },
                )
            }
            None => (
                Fighter {
                    x: 90.0,
                    y: 200.0,
                    facing: 1.0,
                },
                Fighter {
                    x: 230.0,
                    y: 200.0,
                    facing: -1.0,
                },
            ),
        };

        log::info!(
            "scene: {} — action {}, {} frames | stage {} | font {}",
            sff_path.display(),
            anim.action_number,
            anim.frames.len(),
            stage
                .as_ref()
                .map(|s| s.def.name.clone())
                .unwrap_or_else(|| "<none>".into()),
            font.as_ref()
                .map(|f| f.metrics.name.clone())
                .unwrap_or_else(|| "<none>".into())
        );

        let mode = if font.is_some() {
            Mode::Menu
        } else {
            log::warn!("scene: no bitmap font found — starting in fight mode");
            Mode::Fight
        };

        Some(Scene {
            mode,
            cursor: 0,
            stage,
            font,
            char_cache: SpriteCache::new(sff, CPU_CHAR_BUDGET),
            anim,
            durations,
            starts,
            loop_from,
            loop_ms: loop_ms.max(1.0),
            t_ms: 0.0,
            last: Instant::now(),
            fighters: [p1, p2],
            prev_input: 0,
            fight,
        })
    }

    /// Index of the frame that should be on screen at time `t`.
    fn frame_at(&self, t: f64) -> usize {
        let n = self.durations.len();
        let pre: f64 = self.starts[..self.loop_from].iter().sum();
        let t = if t < pre || self.loop_ms <= 0.0 {
            t
        } else {
            pre + (t - pre) % self.loop_ms
        };
        let mut i = 0;
        while i + 1 < n && self.starts[i + 1] <= t {
            i += 1;
        }
        i
    }

    /// Advance the clock, consume input, and draw the current mode.
    pub fn advance_and_draw(&mut self, r: &mut Renderer) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f64() * 1000.0;
        self.last = now;
        // Clamp so returning from background doesn't fast-forward the anim.
        self.t_ms += dt.min(100.0);

        // Input edges: pressed = held ∧ ¬previously held.
        let held = crate::controller_held();
        let pressed = held & !self.prev_input;
        self.prev_input = held;

        match self.mode {
            Mode::Menu => {
                let n = MENU_ITEMS.len();
                if pressed & bits::PU != 0 {
                    self.cursor = (self.cursor + n - 1) % n;
                }
                if pressed & bits::PD != 0 {
                    self.cursor = (self.cursor + 1) % n;
                }
                if pressed & (bits::A | bits::S) != 0 {
                    self.mode = Mode::Fight;
                    log::info!("menu: {} selected", MENU_ITEMS[self.cursor]);
                }
                self.draw_menu(r);
            }
            Mode::Fight => {
                if pressed & bits::B != 0 {
                    self.mode = Mode::Menu;
                }
                // step the live runtime at 60Hz with the touch bits
                if let Some(fr) = self.fight.as_mut() {
                    fr.tick_acc += dt.min(100.0);
                    const MS: f64 = 1000.0 / 60.0;
                    let mut ticks = 0;
                    while fr.tick_acc >= MS && ticks < 4 {
                        fr.tick_acc -= MS;
                        // P2 CPU: AiInput jam feeds raw input bits
                        let intro = fr.fight.round_state < 2;
                        fr.ai.update(intro, fr.ai_level, &mut fr.ai_rng);
                        let b = fr.ai.buttons();
                        let mut p2 = 0u32;
                        for (i, on) in b.iter().enumerate() {
                            if *on {
                                p2 |= 1 << i;
                            }
                        }
                        fr.fight.step_bits(held, p2);
                        ticks += 1;
                    }
                }
                self.draw_fight(r);
            }
        }
    }

    // ── menu ────────────────────────────────────────────────────────────────

    fn draw_menu(&mut self, r: &mut Renderer) {
        self.draw_stage_layer(r, 0, 0.0, 0.0);
        self.draw_stage_layer(r, 1, 0.0, 0.0);
        // Dim the stage behind the menu.
        let q = full_screen_quad([0, 0, 0, 170]);
        r.push(0, &q);

        // Title block.
        self.draw_text_centered(r, "KUROGANE", 56.0, 2.0, WHITE);
        self.draw_text_centered(r, "MUGEN FOR ANDROID", 84.0, 1.0, GRAY);

        // Items (selected: yellow + a marker).
        for (i, item) in MENU_ITEMS.iter().enumerate() {
            let y = 118.0 + i as f32 * 16.0;
            let color = if i == self.cursor { YELLOW } else { WHITE };
            let w = self.draw_text_centered(r, item, y, 1.0, color);
            if i == self.cursor {
                let cx = GAME_W / 2.0 - w / 2.0;
                self.draw_text(r, ">", cx - 12.0, y, 1.0, YELLOW);
            }
        }

        // Footer hints + version.
        self.draw_text_centered(r, "UP/DOWN + A", 226.0, 1.0, GRAY);
        if let Some(fr) = self.font.as_ref() {
            let w = fr.metrics.text_width(VERSION) as f32;
            self.draw_text(r, VERSION, GAME_W - w - 4.0, 4.0, 1.0, GRAY);
        }
    }

    // ── fight ───────────────────────────────────────────────────────────────

    fn draw_fight(&mut self, r: &mut Renderer) {
        // Live runtime path: snapshot (borrows end here), then draw with it
        let live = self.fight.as_ref().map(|fr| {
            (fr.fight.snapshot(), fr.fight.chars[0].gi.clone())
        });
        if let Some((snap, gi)) = live {
            self.draw_stage_layer(r, 0, snap.cam_x, snap.cam_y);
            self.draw_fight_chars(r, &snap, &gi);
            self.draw_stage_layer(r, 1, snap.cam_x, snap.cam_y);
            self.draw_hud(r, &snap);
            return;
        }

        // ── static fallback demo (no compiled char) ─────────────────────
        let scl = self.stage.as_ref().map(|s| s.scl).unwrap_or(1.0);
        self.draw_stage_layer(r, 0, 0.0, 0.0);
        let i = self.frame_at(self.t_ms).min(self.anim.frames.len() - 1);
        let f = &self.anim.frames[i];
        if f.group >= 0 && f.number >= 0 {
            let xs = f.hscale as f32 * f.xscale;
            let ys = f.vscale as f32 * f.yscale;
            let blend = if f.trans_type == TransType::Add {
                BLENDS_ADD
            } else {
                BLENDS_ALPHA
            };
            for fighter in &self.fighters {
                let _ = r.push_sprite(
                    &mut self.char_cache,
                    SRC_CHAR,
                    f.group as u16,
                    f.number as u16,
                    fighter.x,
                    fighter.y,
                    xs * fighter.facing * scl,
                    ys * scl,
                    WHITE,
                    blend,
                );
            }
        }

        self.draw_stage_layer(r, 1, 0.0, 0.0);

        // Corner hint (back to menu).
        if let Some(fr) = self.font.as_ref() {
            let w = fr.metrics.text_width("B:MENU") as f32;
            self.draw_text(r, "B:MENU", GAME_W - w - 4.0, GAME_H - 12.0, 1.0, GRAY);
        }
    }

    /// Draw the live fighters from a fight snapshot, camera-scrolled.
    fn draw_fight_chars(
        &mut self,
        r: &mut Renderer,
        snap: &krg_vm::runtime::FightSnapshot,
        gi: &std::sync::Arc<krg_vm::runtime::GlobalInfo>,
    ) {
        let scl = self.stage.as_ref().map(|s| s.scl).unwrap_or(1.0);
        let ground_y = self
            .stage
            .as_ref()
            .map(|s| s.def.zoffset as f32 * s.scl)
            .unwrap_or(GAME_H - 32.0);
        for cd in &snap.chars {
            let Some(a) = gi.anims.anims.get(&cd.anim_no) else {
                continue;
            };
            let Some(f) = a.frames.get(cd.frame_idx.max(0) as usize) else {
                continue;
            };
            if f.group < 0 {
                continue;
            }
            let xs = f.hscale as f32 * f.xscale * cd.xscale * cd.facing as f32 * scl;
            let ys = f.vscale as f32 * f.yscale * cd.yscale * scl;
            let blend = if f.trans_type == TransType::Add {
                BLENDS_ADD
            } else {
                BLENDS_ALPHA
            };
            let _ = r.push_sprite(
                &mut self.char_cache,
                SRC_CHAR,
                f.group as u16,
                f.number as u16,
                GAME_W / 2.0 + (cd.pos[0] - snap.cam_x) * scl,
                ground_y - cd.pos[1] * scl - snap.cam_y * scl,
                xs,
                ys,
                WHITE,
                blend,
            );
        }
    }

    fn draw_hud(&mut self, r: &mut Renderer, snap: &krg_vm::runtime::FightSnapshot) {
        const BAR_Y: f32 = 8.0;
        const BAR_W: f32 = 110.0;
        const BAR_H: f32 = 8.0;
        const POW_Y: f32 = BAR_Y + BAR_H + 2.0;
        const POW_H: f32 = 4.0;
        let bg = [24, 24, 32, 255];
        let frame = [255, 255, 255, 255];
        let yellow = [255, 232, 72, 255];
        let blue = [64, 132, 255, 255];
        let red = [228, 44, 44, 255];

        for (i, cd) in snap.chars.iter().enumerate() {
            let (x0, right) = if i == 0 { (5.0, false) } else { (GAME_W - 5.0, true) };
            let life_frac = (cd.life as f32 / cd.life_max as f32).clamp(0.0, 1.0);
            let pow_frac = (cd.power as f32 / cd.power_max.max(1) as f32).clamp(0.0, 1.0);

            let quad = |x: f32, y: f32, w: f32, h: f32, c: [u8; 4]| Quad {
                x, y, w, h, u0: 0.0, v0: 0.0, u1: 1.0, v1: 1.0,
                color: c, angle: 0.0, px: 0.0, py: 0.0,
            };

            // life bar: bg → fill → frame
            let bar_x = |frac: f32| if right { x0 - BAR_W * frac } else { x0 };
            let fill_x = |frac: f32| if right { x0 - BAR_W * frac } else { x0 };
            let _ = bar_x;
            let bg_x = if right { x0 - BAR_W } else { x0 };
            r.push(0, &quad(bg_x, BAR_Y, BAR_W, BAR_H, bg));
            let fill = if right { quad(x0 - BAR_W * life_frac, BAR_Y, BAR_W * life_frac, BAR_H, yellow) }
                       else { quad(x0, BAR_Y, BAR_W * life_frac, BAR_H, yellow) };
            r.push(0, &fill);
            // 1px frame
            r.push(0, &quad(bg_x, BAR_Y, BAR_W, 1.0, frame));
            r.push(0, &quad(bg_x, BAR_Y + BAR_H - 1.0, BAR_W, 1.0, frame));
            r.push(0, &quad(bg_x, BAR_Y, 1.0, BAR_H, frame));
            r.push(0, &quad(bg_x + BAR_W - 1.0, BAR_Y, 1.0, BAR_H, frame));

            // power bar
            let pbg_x = if right { x0 - BAR_W } else { x0 };
            r.push(0, &quad(pbg_x, POW_Y, BAR_W, POW_H, bg));
            let pfill = if right { quad(x0 - BAR_W * pow_frac, POW_Y, BAR_W * pow_frac, POW_H, blue) }
                        else { quad(x0, POW_Y, BAR_W * pow_frac, POW_H, blue) };
            r.push(0, &pfill);

            // KO tint on the loser's bar
            if !cd.alive {
                r.push(0, &quad(bg_x, BAR_Y, BAR_W, BAR_H, [200, 32, 32, 120]));
            }
            let _ = fill_x;
        }

        // timer (MM:SS from ticks)
        let texts: Vec<(String, f32, f32, f32, [u8; 4])> = {
            let Some(fr) = self.font.as_ref() else {
                return;
            };
            let t = snap.time_left.max(0);
            let mut out = vec![];
            let txt = format!("{:02}:{:02}", t / 60, t % 60);
            let w = fr.metrics.text_width(&txt) as f32;
            out.push((txt, (GAME_W - w) / 2.0, BAR_Y - 2.0, 1.0, WHITE));
            if snap.round_state < 2 {
                let msg = if snap.round_state == 0 { "GET READY" } else { "ROUND 1" };
                let w = fr.metrics.text_width(msg) as f32;
                out.push((msg.into(), (GAME_W - w) / 2.0, GAME_H / 2.0 - 20.0, 1.0, YELLOW));
            }
            if snap.round_state >= 3 {
                let msg = "K.O.";
                let w = fr.metrics.text_width(msg) as f32;
                out.push((msg.into(), (GAME_W - w) / 2.0, GAME_H / 2.0 - 20.0, 2.0, red));
            }
            out
        };
        for (t, x, y, s, c) in texts {
            self.draw_text(r, &t, x, y, s, c);
        }
    }

    // ── stage composition ────────────────────────────────────────────────────

    /// Draw one stage layer (0 = behind fighters, 1 = in front).
    ///
    /// Camera is parked at (0,0) for now, so element position is simply
    /// `(160 + start.x, start.y)` (stage.go anchors stage BGs at screen
    /// center) scaled by `scl` for HiRes stages. Parallax is approximated
    /// with its bottom xscale (the true trapezoid needs a vertex-format
    /// extension; tracked for the next renderer milestone).
    fn draw_stage_layer(&mut self, r: &mut Renderer, layer: i32, cam_x: f32, cam_y: f32) {
        let Some(st) = self.stage.as_mut() else {
            return;
        };
        for bi in 0..st.def.bg.len() {
            let bg = &st.def.bg[bi];
            if bg.layerno != layer {
                continue;
            }
            let Some(entry) = st.cache.entry(bg.group, bg.number) else {
                continue;
            };
            let (w, h) = (entry.width as f32, entry.height as f32);
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            let scl = st.scl;
            let x = GAME_W / 2.0 + bg.start[0] * scl - cam_x * scl;
            let y = bg.start[1] * scl - cam_y * scl;
            let xs = if bg.ty == BgType::Parallax {
                parallax_scale(bg, w)
            } else {
                1.0
            } * scl;
            let (blend, color) = trans_blend(bg);
            let step = (w * xs + bg.tilespacing[0] as f32 * scl).max(1.0);
            for k in tile_copies(bg.tile[0], x, w * xs) {
                let _ = r.push_sprite(
                    &mut st.cache,
                    SRC_STAGE,
                    bg.group,
                    bg.number,
                    x + k as f32 * step,
                    y,
                    xs,
                    scl,
                    color,
                    blend,
                );
            }
        }
    }

    // ── text ────────────────────────────────────────────────────────────────

    /// Draw one string, left-aligned at `x`. Returns the drawn width.
    fn draw_text(
        &mut self,
        r: &mut Renderer,
        text: &str,
        x: f32,
        y: f32,
        scale: f32,
        color: [u8; 4],
    ) -> f32 {
        let Some(fr) = self.font.as_mut() else {
            return 0.0;
        };
        let m = &fr.metrics;
        let mut cx = x + m.offset[0] as f32 * scale;
        let cy = y + m.offset[1] as f32 * scale;
        for c in text.chars() {
            if c == ' ' {
                cx += m.size[0] as f32 * scale;
            } else if let Some(g) = m.glyph(c) {
                let _ = r.push_sprite(
                    &mut fr.cache,
                    SRC_FONT,
                    g.group,
                    g.number,
                    cx,
                    cy,
                    scale,
                    scale,
                    color,
                    BLENDS_ALPHA,
                );
                cx += (g.w as f32 + m.spacing[0] as f32) * scale;
            } else {
                cx += m.spacing[0] as f32 * scale;
            }
        }
        cx - x
    }

    /// Draw centered; returns the drawn width.
    fn draw_text_centered(
        &mut self,
        r: &mut Renderer,
        text: &str,
        y: f32,
        scale: f32,
        color: [u8; 4],
    ) -> f32 {
        let w = self
            .font
            .as_ref()
            .map(|f| f.metrics.text_width(text) as f32 * scale)
            .unwrap_or(0.0);
        self.draw_text(r, text, GAME_W / 2.0 - w / 2.0, y, scale, color)
    }
}

/// Full-screen flat color quad (drawn against the shared white texture).
fn full_screen_quad(color: [u8; 4]) -> Quad {
    Quad {
        x: 0.0,
        y: 0.0,
        w: GAME_W,
        h: GAME_H,
        u0: 0.0,
        v0: 0.0,
        u1: 1.0,
        v1: 1.0,
        color,
        angle: 0.0,
        px: GAME_W / 2.0,
        py: GAME_H / 2.0,
    }
}

/// Parallax uniform-scale approximation: bottom `xscale`, or `width[1]`
/// converted through the sprite width when the def uses `width =` instead.
fn parallax_scale(bg: &krg_format::BgElement, sprite_w: f32) -> f32 {
    if bg.parallax_width[0] > 0 && sprite_w > 0.0 {
        bg.parallax_width[1] as f32 / sprite_w
    } else {
        bg.parallax_xscale[1]
    }
    .max(0.05)
}

/// Map a BG element's `trans` to the renderer's two blend modes + tint.
///
/// * `none/default` → alpha blend, full opacity.
/// * `add`/`add1` → additive at full strength.
/// * `addalpha s,d` → when `s + d ≈ 255` it is exactly an alpha blend at
///   tint `s` (the common 128,128 = 50/50 case); otherwise additive with
///   the source scaled by `s` (approximation until the blend-run model
///   grows per-run src/dst factors).
/// * `sub`/`subadd` → drawn opaque (rare; proper reverse-subtract pending).
fn trans_blend(bg: &krg_format::BgElement) -> (u8, [u8; 4]) {
    match bg.trans {
        BgTrans::None | BgTrans::Sub | BgTrans::SubAdd => (BLENDS_ALPHA, WHITE),
        BgTrans::Add | BgTrans::Add1 => (BLENDS_ADD, WHITE),
        BgTrans::AddAlpha => {
            let (s, d) = (bg.alpha[0], bg.alpha[1]);
            if s as u32 + d as u32 >= 255 {
                (BLENDS_ALPHA, [255, 255, 255, s])
            } else {
                (BLENDS_ADD, [s, s, s, 255])
            }
        }
    }
}

/// Tile copy offsets along x: `0` = single copy, `1` = cover the screen
/// (both directions), `n > 1` = n copies right of the base.
fn tile_copies(tile: i32, x: f32, w: f32) -> Vec<i32> {
    match tile {
        0 => vec![0],
        n if n > 1 => (0..n).collect(),
        _ => {
            // Infinite: copies around the base until off-screen both ways.
            let mut ks = vec![0i32];
            let step = w.max(1.0);
            let right = ((GAME_W - x) / step).ceil() as i32;
            let left = (x / step).ceil() as i32;
            for k in 1..=right.min(64) {
                ks.push(k);
            }
            for k in 1..=left.min(64) {
                ks.push(-k);
            }
            ks
        }
    }
}

// ── content discovery ───────────────────────────────────────────────────────

/// Pick the character for the demo: bundled kfm first, then any chars/*
/// folder that has an .sff with a sibling .air (the AIR is required for
/// the animation).
fn find_char_sff(root: &Path) -> Option<PathBuf> {
    let candidates = ["chars/kfm/kfm.sff", "chars/kfm720/kfm.sff", "data/kfm/kfm.sff"];
    for c in candidates {
        let p = root.join(c);
        if p.is_file() {
            return Some(p);
        }
    }
    let chars = root.join("chars");
    let rd = std::fs::read_dir(chars).ok()?;
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .ok()
            .map(|rd| {
                rd.flatten()
                    .map(|f| f.file_name().to_string_lossy().to_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        for name in names {
            if name.ends_with(".sff") {
                let sff = dir.join(&name);
                if sff.with_extension("air").is_file() {
                    return Some(sff);
                }
            }
        }
    }
    None
}

/// Pick the stage: bundled kfm.def first, then the first stages/*.def whose
/// `spr` sff actually loads. HiRes stages get their scale factor here.
fn find_stage(root: &Path) -> Option<StageRun> {
    let mut candidates: Vec<PathBuf> = vec![root.join("stages/kfm.def")];
    if let Ok(rd) = std::fs::read_dir(root.join("stages")) {
        let mut more: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| x.eq_ignore_ascii_case("def"))
            })
            .collect();
        more.sort();
        candidates.extend(more);
    }
    for p in candidates {
        let Ok(def) = StageDef::load_path(&p) else {
            continue;
        };
        let Ok(sff) = Sff::load_path(&def.sff_path, SffType::Other, false) else {
            log::warn!("scene: stage {} sprite file failed to load", p.display());
            continue;
        };
        let lw = def.localcoord[0].max(1) as f32;
        let scl = (GAME_W / lw).min(4.0);
        log::info!("scene: stage {} ({} elements)", def.name, def.bg.len());
        return Some(StageRun {
            def,
            cache: SpriteCache::new(sff, CPU_STAGE_BUDGET),
            scl,
        });
    }
    None
}

/// Pick the menu font: f-6x9 first, then default-3x5, then the first
/// font/*.def that parses as a bitmap font.
fn find_font(root: &Path) -> Option<FontRun> {
    let mut candidates: Vec<PathBuf> = vec![
        root.join("font/f-6x9.def"),
        root.join("font/default-3x5.def"),
    ];
    if let Ok(rd) = std::fs::read_dir(root.join("font")) {
        let mut more: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| x.eq_ignore_ascii_case("def"))
            })
            .collect();
        more.sort();
        candidates.extend(more);
    }
    for p in candidates {
        match BitmapFont::load(&p) {
            Ok(f) => {
                let (metrics, sff) = f.into_parts();
                log::info!("scene: font {} ({} glyphs)", metrics.name, metrics.glyphs.len());
                return Some(FontRun {
                    metrics,
                    cache: SpriteCache::new(sff, CPU_FONT_BUDGET),
                });
            }
            Err(krg_format::FontError::Unsupported(t)) => {
                log::debug!("scene: skip {}: {t}", p.display());
            }
            Err(e) => log::debug!("scene: skip {}: {e}", p.display()),
        }
    }
    None
}

// ── FightRun helpers ──────────────────────────────────────────────────────────

/// UTF-8 text read with BOM strip (krg-vm ini::load_text semantics).
fn read_text(p: &Path) -> Option<String> {
    let b = std::fs::read(p)
        .map_err(|e| log::warn!("scene: {}: {e}", p.display()))
        .ok()?;
    Some(krg_vm::ini::load_text(&b))
}

/// The first `<dir>/*.def` that has a [Files] section with a cmd.
fn def_of(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<PathBuf> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("def")).unwrap_or(false) {
            // Prefer `<name>.def` matching the folder name (kfm/kfm.def)
            if best.is_none() {
                best = Some(p.clone());
            }
            let stem_matches = p.file_stem() == dir.file_name();
            if stem_matches {
                return Some(p);
            }
        }
    }
    best
}

/// The char folder with an SFF (kfm first, like find_char_sff).
fn find_char_dir(root: &Path) -> Option<PathBuf> {
    let candidates = ["chars/kfm", "chars/kfm720", "data/kfm"];
    for c in candidates {
        let p = root.join(c);
        if p.is_dir() {
            return Some(p);
        }
    }
    let chars = root.join("chars");
    let rd = std::fs::read_dir(chars).ok()?;
    for e in rd.flatten() {
        let dir = e.path();
        if dir.is_dir() {
            let mut has_sff = false;
            if let Ok(files) = std::fs::read_dir(&dir) {
                has_sff = files.flatten().any(|f| {
                    f.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.eq_ignore_ascii_case("sff"))
                        .unwrap_or(false)
                });
            }
            if has_sff {
                return Some(dir);
            }
        }
    }
    None
}
