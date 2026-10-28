//! Kurogane Android entry point.
//!
//! Loaded as `libkurogane.so` by the APK.
//!
//! * `MainActivity.nativeScan` — takes the newline-separated candidate content
//!   roots from the launcher (public `/storage/emulated/0/Kurogane` folder,
//!   an existing `IkemenGO` folder, app external/internal dirs), picks the
//!   first root that contains a MUGEN screenpack, scans it through the real
//!   Kurogane format layer and returns a report whose first line is `OK:`
//!   or `NO CONTENT`.
//! * `ControllerView.nativeInput*` — the touch controller bridge: touches are
//!   forwarded from the Java view, all zone geometry and chord logic live in
//!   `krg-input` (single source of truth; the view only renders what Rust
//!   computes). Layout editing (drag/pinch) arrives through the `edit_*`
//!   entry points and persists to a text file the app passes in.
//! * `GameSurfaceView.nativeGl*` — the GLES 3.0 renderer bridge. All GL calls
//!   happen on the GLSurfaceView render thread (that context owns them); the
//!   scene (kfm demo) streams its sprites through krg-gfx's LRU texture cache.
//! * `MainActivity.nativeSceneSetRoot` — hands the resolved content root to
//!   the GL thread; a generation counter makes it reload on rescan.

mod scene;

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jfloat, jfloatArray, jint, jlong, jstring};
use jni::JNIEnv;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use krg_gfx::Renderer;
use krg_input::{TouchController, TouchPhase, ZoneKind};

static CONTROLLER: Mutex<Option<TouchController>> = Mutex::new(None);

fn with_controller<R>(f: impl FnOnce(&mut TouchController) -> R) -> R {
    let mut guard = CONTROLLER.lock().unwrap();
    f(guard.get_or_insert_with(TouchController::new))
}

// ── GLES renderer (GameSurfaceView — GL thread only) ────────────────────────

/// GL-thread state: the renderer plus the demo scene. thread_local keeps the
/// glow context (not Send) off the statics and documents the thread contract.
struct Gls {
    renderer: Renderer,
    scene: Option<scene::Scene>,
    scene_gen: u64,
}

thread_local! {
    static GLS: RefCell<Option<Gls>> = const { RefCell::new(None) };
}

/// Content root for the scene (set from the scan thread, read on the GL
/// thread). `SCENE_GEN` bumps on every change → the GL thread reloads.
static SCENE_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);
static SCENE_GEN: AtomicU64 = AtomicU64::new(0);

/// EGL entry-point resolver for glow (Android only; never called on host).
#[cfg(target_os = "android")]
mod egl {
    /// libEGL is a system library on Android — the linker resolves this at
    /// load; no runtime lookup needed.
    #[link(name = "EGL")]
    extern "C" {
        fn eglGetProcAddress(name: *const std::os::raw::c_char)
            -> *const std::os::raw::c_void;
    }

    pub fn get_proc(name: &str) -> *const std::os::raw::c_void {
        match std::ffi::CString::new(name) {
            Ok(c) => unsafe { eglGetProcAddress(c.as_ptr()) },
            Err(_) => std::ptr::null(),
        }
    }
}

#[cfg(not(target_os = "android"))]
mod egl {
    pub fn get_proc(_name: &str) -> *const std::os::raw::c_void {
        std::ptr::null()
    }
}

#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_GameSurfaceView_nativeGlInit(
    _env: JNIEnv,
    _class: JClass,
) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    GLS.with(|g| {
        let mut slot = g.borrow_mut();
        if let Some(mut old) = slot.take() {
            old.renderer.destroy(); // context recreation: names are stale anyway
        }
        match unsafe { Renderer::new(egl::get_proc) } {
            Ok(renderer) => {
                log::info!("krg-gfx renderer online (GLES 3.0, batched quads + LRU textures)");
                *slot = Some(Gls {
                    renderer,
                    scene: None,
                    scene_gen: 0,
                });
            }
            Err(e) => log::error!("renderer init failed: {e}"),
        }
    });
}

#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_GameSurfaceView_nativeGlResize(
    _env: JNIEnv,
    _class: JClass,
    w: jint,
    h: jint,
) {
    GLS.with(|g| {
        if let Some(gls) = g.borrow_mut().as_mut() {
            gls.renderer.resize(w, h);
        }
    });
}

#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_GameSurfaceView_nativeGlDraw(
    _env: JNIEnv,
    _class: JClass,
) {
    GLS.with(|g| {
        let mut slot = g.borrow_mut();
        let Some(gls) = slot.as_mut() else {
            return;
        };
        // Root changed since the scene was loaded → reload it.
        let gen = SCENE_GEN.load(Ordering::Relaxed);
        if gls.scene_gen != gen {
            gls.scene_gen = gen;
            gls.scene = None;
        }
        if gls.scene.is_none() {
            let root = SCENE_ROOT.lock().unwrap().clone();
            if let Some(root) = root {
                gls.scene = scene::Scene::load(&root);
            }
        }
        if let Some(scene) = gls.scene.as_mut() {
            gls.renderer.begin_scene();
            scene.advance_and_draw(&mut gls.renderer);
            gls.renderer.present();
            let s = gls.renderer.stats();
            if s.frames % 300 == 0 {
                log::info!(
                    "gfx: frame {} — {} quads, {} runs, {} textures ({} KB), {} evictions",
                    s.frames,
                    s.quads,
                    s.runs,
                    s.textures,
                    s.tex_bytes / 1024,
                    s.evictions
                );
            }
        } else {
            gls.renderer.clear_default();
        }
    });
}

/// Hand the resolved content root to the GL thread (called from the scan
/// thread; the generation bump makes the next draw reload the scene).
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_MainActivity_nativeSceneSetRoot(
    mut env: JNIEnv,
    _class: JClass,
    j_path: JString,
) {
    let path: String = env
        .get_string(&j_path)
        .map(|s| s.into())
        .unwrap_or_default();
    if path.is_empty() {
        return;
    }
    *SCENE_ROOT.lock().unwrap() = Some(PathBuf::from(&path));
    SCENE_GEN.fetch_add(1, Ordering::Relaxed);
    log::info!("scene root set: {path}");
}

// ── Content scan (MainActivity) ─────────────────────────────────────────────

/// True when `root` contains a MUGEN screenpack: a `data/` directory holding
/// at least one `.def` / `.cns` / `mugen.cfg` (case-insensitive — content
/// arrives with arbitrary casing from Windows tools).
fn root_has_basedata(root: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(root.join("data")) else {
        return false;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.ends_with(".def") || name.ends_with(".cns") || name == "mugen.cfg" {
            return true;
        }
    }
    false
}

fn has_def(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten().any(|e| {
        e.path()
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x.eq_ignore_ascii_case("def"))
    })
}

fn count_def_files(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("def"))
        })
        .count()
}

/// Characters are folders under `chars/` that contain a `.def`.
fn count_char_dirs(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .filter(|e| e.path().is_dir() && has_def(&e.path()))
        .count()
}

fn first_char_dir(dir: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && has_def(&p) {
            return Some(p);
        }
    }
    None
}

/// Parse up to `cap_sff` / `cap_air` samples from `dir` through the real
/// format layer — proves on-device that the content actually parses.
fn parse_samples(dir: &Path, report: &mut String, cap_sff: usize, cap_air: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let (mut ns, mut na) = (0usize, 0usize);
    for e in rd.flatten() {
        let p = e.path();
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "sff" if ns < cap_sff => {
                ns += 1;
                match krg_format::Sff::load_path(&p, krg_format::SffType::Char, false) {
                    Ok(s) => report.push_str(&format!(
                        "SFF {}: v{}.{}.{}.{} — {} sprites, {} palettes\n",
                        p.display(),
                        s.header.version[0],
                        s.header.version[1],
                        s.header.version[2],
                        s.header.version[3],
                        s.entries.len(),
                        s.palettes.palettes.len()
                    )),
                    Err(e) => report.push_str(&format!("SFF {}: ERROR {e}\n", p.display())),
                }
            }
            "air" if na < cap_air => {
                na += 1;
                if let Ok(text) = std::fs::read_to_string(&p) {
                    let mut ac = krg_format::AnimCompiler::new(false);
                    ac.compile_text(&p.display().to_string(), &text);
                    let t = ac.finalize();
                    report.push_str(&format!("AIR {}: {} actions\n", p.display(), t.anims.len()));
                }
            }
            _ => {}
        }
    }
}

/// Scan every candidate root (in order); first one with base data wins.
fn scan_report(roots: &str) -> String {
    let mut chosen: Option<PathBuf> = None;
    let mut checked: Vec<String> = Vec::new();
    for line in roots.lines().map(str::trim).filter(|l| !l.is_empty()) {
        checked.push(line.to_string());
        let p = PathBuf::from(line);
        if chosen.is_none() && root_has_basedata(&p) {
            chosen = Some(p);
        }
    }

    let Some(root) = chosen else {
        let mut r = String::from("NO CONTENT\nchecked roots:\n");
        for c in &checked {
            r.push_str(&format!("  {c}\n"));
        }
        if let Some(first) = checked.first() {
            r.push_str(&format!(
                "put MUGEN data here:\n{first}\n\
                 needs a screenpack: copy 'data' + 'font' from an Ikemen GO\n\
                 release (or MUGEN 1.x), then add chars/ and stages/\n"
            ));
        }
        return r;
    };

    let mut r = format!("OK: {}\n", root.display());
    let chars = count_char_dirs(&root.join("chars"));
    let stages = count_def_files(&root.join("stages"));
    let fonts = count_def_files(&root.join("font"));
    r.push_str(&format!(
        "chars: {chars} | stages: {stages} | fonts: {fonts}\n"
    ));
    parse_samples(&root.join("data"), &mut r, 3, 3);
    if let Some(cd) = first_char_dir(&root.join("chars")) {
        parse_samples(&cd, &mut r, 1, 1);
    }
    r
}

#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_MainActivity_nativeScan(
    mut env: JNIEnv,
    _class: JClass,
    j_roots: JString,
) -> jstring {
    let roots: String = env
        .get_string(&j_roots)
        .map(|s| s.into())
        .unwrap_or_default();
    let report = scan_report(&roots);
    env.new_string(report)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

// ── Touch controller (ControllerView) ───────────────────────────────────────

/// action codes: 0 = down, 1 = move, 2 = up.
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputTouch(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
    action: jint,
    x: jfloat,
    y: jfloat,
    w: jfloat,
    h: jfloat,
) {
    let phase = match action {
        0 => TouchPhase::Down,
        1 => TouchPhase::Move,
        _ => TouchPhase::Up,
    };
    with_controller(|c| c.touch(id as u64, phase, x, y, w, h));
}

#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputTick(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    with_controller(|c| c.tick()) as jint
}

#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputHeld(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    with_controller(|c| c.held()) as jint
}

/// Flat zone array, stride 8: [type, param, cx, cy, r, held, ex, ey] — 8
/// zones (pad + 6 buttons + START). type: 0 = pad, 1 = button, 2 = start.
/// Pixel coordinates for a w×h view.
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputZones(
    env: JNIEnv,
    _class: JClass,
    w: jfloat,
    h: jfloat,
) -> jfloatArray {
    let zones = with_controller(|c| c.zones(w, h));
    let mut flat: Vec<f32> = Vec::with_capacity(zones.len() * 8);
    for z in zones {
        flat.extend_from_slice(&[
            z.kind as f32,
            z.param as f32,
            z.cx,
            z.cy,
            z.r,
            z.held as i32 as f32,
            z.ex,
            z.ey,
        ]);
    }
    match env.new_float_array(flat.len() as i32) {
        Ok(arr) => {
            let _ = env.set_float_array_region(&arr, 0, &flat);
            arr.into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Human-readable held-state string (HUD).
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputDescribe(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    let s = with_controller(|c| c.describe());
    env.new_string(s)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

// ── Layout editing + persistence (ControllerView) ──────────────────────────

/// Wire identity resolver: (kind, param) → (ZoneKind, bit). `kind`/`param`
/// mirror the zone array the view already parses (kind: 0/1/2, param:
/// A=0..Z=5 for buttons). Returns None for garbage.
fn zone_id(kind: jint, param: jint) -> Option<(ZoneKind, u32)> {
    ZoneKind::from_wire(kind as i32, param as i32)
}

/// Drop every held signal + pointer (entering the layout editor, or focus
/// loss — never leave a button stuck down).
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputReleaseAll() {
    with_controller(|c| c.release_all());
}

/// Begin an edit session: snapshot the layout (for CANCEL) and clear holds.
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputEditBegin() {
    with_controller(|c| c.edit_begin());
}

/// Move the control identified by (kind, param) so its center sits at
/// pixel (x, y) of a w×h view (the view may add its grab offset first).
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputEditMove(
    _env: JNIEnv,
    _class: JClass,
    kind: jint,
    param: jint,
    x: jfloat,
    y: jfloat,
    w: jfloat,
    h: jfloat,
) {
    if let Some((k, bit)) = zone_id(kind, param) {
        with_controller(|c| c.edit_move(k, bit, x, y, w, h));
    }
}

/// Scale the control identified by (kind, param) by `factor` (clamped in
/// Rust). Used by the pinch gesture and the −/+ toolbar steppers.
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputEditScale(
    _env: JNIEnv,
    _class: JClass,
    kind: jint,
    param: jint,
    factor: jfloat,
) {
    if let Some((k, bit)) = zone_id(kind, param) {
        with_controller(|c| c.edit_scale(k, bit, factor));
    }
}

/// Abandon the edit session: restore the snapshot taken by EditBegin.
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputEditRevert() {
    with_controller(|c| c.edit_revert());
}

/// Reset the live layout to factory defaults (stays in the edit session,
/// so CANCEL still restores the pre-edit state).
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputResetLayout() {
    with_controller(|c| c.reset_layout());
}

/// Persist the live layout to `path` (UTF-8 text). True on success.
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputLayoutSave(
    mut env: JNIEnv,
    _class: JClass,
    j_path: JString,
) -> jboolean {
    let path: String = env
        .get_string(&j_path)
        .map(|s| s.into())
        .unwrap_or_default();
    if path.is_empty() {
        return 0;
    }
    with_controller(|c| c.save_layout(&PathBuf::from(&path))) as jboolean
}

/// Load a persisted layout from `path`. Missing/corrupt files keep the
/// current (default) layout. True when parsed. Called once at startup.
#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_ControllerView_nativeInputLayoutLoad(
    mut env: JNIEnv,
    _class: JClass,
    j_path: JString,
) -> jboolean {
    let path: String = env
        .get_string(&j_path)
        .map(|s| s.into())
        .unwrap_or_default();
    if path.is_empty() {
        return 0;
    }
    with_controller(|c| c.load_layout(Path::new(&path))) as jboolean
}
