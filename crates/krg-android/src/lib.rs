//! Kurogane Android entry point.
//!
//! Loaded as `libkurogane.so` by the APK.
//!
//! * `MainActivity.nativeHello` — scans `<filesDir>/data` for SFF/AIR content
//!   through the Kurogane format layer and returns a report string.
//! * `ControllerView.nativeInput*` — the minimal touch controller bridge:
//!   touches are forwarded from the Java view, all zone geometry and chord
//!   logic live in `krg-input` (single source of truth; the view only
//!   renders what Rust computes).

use jni::objects::{JClass, JString};
use jni::sys::{jfloat, jfloatArray, jint, jlong, jstring};
use jni::JNIEnv;
use std::path::PathBuf;
use std::sync::Mutex;

use krg_input::{TouchController, TouchPhase};

static CONTROLLER: Mutex<Option<TouchController>> = Mutex::new(None);

fn with_controller<R>(f: impl FnOnce(&mut TouchController) -> R) -> R {
    let mut guard = CONTROLLER.lock().unwrap();
    f(guard.get_or_insert_with(TouchController::new))
}

// ── Content scan (MainActivity) ─────────────────────────────────────────────

#[no_mangle]
pub extern "system" fn Java_com_kurogane_engine_MainActivity_nativeHello(
    mut env: JNIEnv,
    _class: JClass,
    j_files_dir: JString,
) -> jstring {
    let files_dir: String = env
        .get_string(&j_files_dir)
        .map(|s| s.into())
        .unwrap_or_default();

    let mut report = format!(
        "Kurogane 0.2.0 — content scan\nfiles dir: {files_dir}\n",
    );

    // Scan for SFF/AIR files under <dir>/data — the same layout the
    // Ikemen GO Android APK uses for extracted content.
    let data_dir = PathBuf::from(&files_dir).join("data");
    let mut found = 0usize;
    if let Ok(rd) = std::fs::read_dir(&data_dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            match ext.as_str() {
                "sff" => {
                    found += 1;
                    match krg_format::Sff::load_path(&p, krg_format::SffType::Char, false) {
                        Ok(s) => {
                            report.push_str(&format!(
                                "SFF {}: v{}.{}.{}.{} — {} sprites, {} palettes\n",
                                p.display(),
                                s.header.version[0],
                                s.header.version[1],
                                s.header.version[2],
                                s.header.version[3],
                                s.entries.len(),
                                s.palettes.palettes.len()
                            ));
                        }
                        Err(e) => {
                            report.push_str(&format!("SFF {}: ERROR {e}\n", p.display()));
                        }
                    }
                }
                "air" => {
                    found += 1;
                    if let Ok(text) = std::fs::read_to_string(&p) {
                        let mut ac = krg_format::AnimCompiler::new(false);
                        ac.compile_text(&p.display().to_string(), &text);
                        let t = ac.finalize();
                        report.push_str(&format!(
                            "AIR {}: {} actions\n",
                            p.display(),
                            t.anims.len()
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    if found == 0 {
        report.push_str(
            "no content found — place MUGEN data under\nAndroid/data/com.kurogane.engine/files/data/\n",
        );
    }

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

/// Flat zone array, stride 8: [type, param, cx, cy, r, held, ex, ey] × 6.
/// type: 0 = pad, 1 = button, 2 = start. Pixel coordinates for a w×h view.
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
