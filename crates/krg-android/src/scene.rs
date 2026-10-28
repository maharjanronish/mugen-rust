//! The v0.7 demo scene: one character playing its standing animation
//! through the full stack — real SFF parsing, real AIR timing, LRU texture
//! streaming, batched GLES 3.0 rendering.
//!
//! This is deliberately the *thinnest possible* thing that exercises every
//! layer end-to-end (the engine's scene graph, stage, camera and CNS VM come
//! later): it picks the bundled kfm (or the first char folder it recognizes),
//! loops action 0, and lets the renderer's cache do its job. When the char
//! folder is missing it simply doesn't draw — the app stays on the setup
//! panel.

use std::path::Path;
use std::time::Instant;

use krg_core::TransType;
use krg_format::{AnimCompiler, Animation, SpriteCache, Sff, SffType};
use krg_gfx::{BLENDS_ADD, BLENDS_ALPHA, Renderer};

/// AIR time is 60 Hz ticks (anim.go: `TickTime` = 60/s).
const TICK_MS: f64 = 1000.0 / 60.0;
/// CPU-side decoded-pixel budget for one character's sprites.
const CPU_SPRITE_BUDGET: usize = 12 * 1024 * 1024;

pub struct Scene {
    cache: SpriteCache,
    anim: Animation,
    /// Per-frame durations in ms (time clamped to ≥ 1 tick).
    durations: Vec<f64>,
    /// Cumulative start time of each frame (ms since anim start).
    starts: Vec<f64>,
    /// Frame index the loop returns to (AIR loopstart, clamped).
    loop_from: usize,
    /// Total length of the looping window (ms).
    loop_ms: f64,
    /// Master clock (ms, clamped advance — see below).
    t_ms: f64,
    last: Instant,
    /// Draw position (axis point, game space).
    pos: [f32; 2],
    scale: f32,
}

impl Scene {
    /// Load the demo scene from a content root: prefers the bundled kfm,
    /// falls back to the first kfm-like folder in chars/.
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

        log::info!(
            "scene: {} — action {}, {} frames, loop {:.0} ms",
            sff_path.display(),
            anim.action_number,
            anim.frames.len(),
            loop_ms
        );
        Some(Scene {
            cache: SpriteCache::new(sff, CPU_SPRITE_BUDGET),
            anim,
            durations,
            starts,
            loop_from,
            loop_ms: loop_ms.max(1.0),
            t_ms: 0.0,
            last: Instant::now(),
            pos: [160.0, 228.0],
            scale: 1.0,
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

    /// Advance the clock and draw the current frame.
    pub fn advance_and_draw(&mut self, r: &mut Renderer) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f64() * 1000.0;
        self.last = now;
        // Clamp so returning from background doesn't fast-forward the anim.
        self.t_ms += dt.min(100.0);

        let i = self.frame_at(self.t_ms).min(self.anim.frames.len() - 1);
        let f = &self.anim.frames[i];
        // AIR blank frames (group -1) draw nothing.
        if f.group < 0 || f.number < 0 {
            return;
        }
        let xs = f.hscale as f32 * f.xscale * self.scale;
        let ys = f.vscale as f32 * f.yscale * self.scale;
        let blend = if f.trans_type == TransType::Add {
            BLENDS_ADD
        } else {
            BLENDS_ALPHA
        };
        let _ = r.push_sprite(
            &mut self.cache,
            f.group as u16,
            f.number as u16,
            self.pos[0],
            self.pos[1],
            xs,
            ys,
            [255, 255, 255, 255],
            blend,
        );
    }
}

/// Pick the character for the demo: bundled kfm first, then any chars/*
/// folder that has an .sff with a sibling .air (the AIR is required for
/// the animation).
fn find_char_sff(root: &Path) -> Option<std::path::PathBuf> {
    let candidates = [
        "chars/kfm/kfm.sff",
        "chars/kfm720/kfm.sff",
        "data/kfm/kfm.sff",
    ];
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
