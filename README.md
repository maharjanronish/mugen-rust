# Kurogane (黒鉄)

**A Rust rewrite of the IKEMEN GO fighting game engine for Android 8+ (API 26)
— 100% content-compatible with the Go engine, with a fraction of the RAM,
and a clean minimal touch controller.**

Upstream reference: [ikemen-engine/Ikemen-GO](https://github.com/ikemen-engine/Ikemen-GO) (MIT).
Every ported function carries a `image.go:NNN`-style reference so behavior can
be audited line-by-line against the Go source.

---

## Controls — six buttons, fully customizable

MUGEN content can require up to **14 input signals** (6 attack buttons, 4
directions, start, plus Ikemen's extra `d`/`w`/`m` buttons). Kurogane ships
the classic **six-button layout** (punch row **X Y Z**, kick row **A B C**) and
synthesizes the remaining signals as simultaneous-press **chords** — the
pressed buttons stay on, so `QCF+Z`-style *and* `X+Y`-style commands all
fire:

```
        ┌──────────────────────────────────────────────┐
        │  [≡] start  [EDIT]                  HUD      │
        │                                              │
        │   ╭───╮                      X ○  Y ○  Z ○   │
        │   │ ● │  8-way pad            A ○  B ○  C ○  │
        │   ╰───╯                                      │
        └──────────────────────────────────────────────┘
```

| Physical | Emits |
|---|---|
| **X/Y/Z** punches · **A/B/C** kicks | the six attack signals directly |
| any two of X, Y, Z | + the **missing punch** · any two of A, B, C | + the **missing kick** |
| A+X together | + **D** · B+Y together | + **W** (Ikemen extras) |
| A+B+X+Y (all four) | + **M** |
| START | **S** |
| 8-way pad | PU/PD/PL/PR (physical; the engine maps F/B by facing) |

Quarter-circles, hold-back blocking, jumps and **double-tap dashes (`F,F`)**
all work natively — MUGEN command syntax reads them from the raw direction
stream, so no hidden controller magic is needed or wanted.

### Editing the layout

Tap the small **EDIT** button (beside START) to open the editor:

* **drag** any control to move it (centers are stored normalized — the layout
  survives rotation and device changes)
* **pinch** with two fingers, or the **−/+** steppers, to resize
* **DEFAULTS** restores the factory layout, **CANCEL** undoes the whole
  session, **SAVE** persists
* the layout is stored in `files/controller.ini` (tolerant line format;
  missing/corrupt entries fall back to defaults)

Entering the editor releases all held buttons, so the hold can never
leave a button stuck down mid-fight.

Bit layout is **bit-exact with Ikemen GO's `InputBits`** (`src/input.go:358`):
`PU=1<<0 … PR=1<<3, A..Z=1<<4..1<<9, S=1<<10, D/W/M=1<<11..13`.

## Why

Ikemen GO is an excellent engine that pays two taxes on Android:

1. **Go runtime overhead** — the team ships with `GODEBUG=asyncpreemptoff=1,
   cgocheck=0,scavenge=off` just to keep the runtime cooperating with
   Android, and fights GC pressure with 10+ `sync.Pool` types.
2. **Eager sprite decode** — every sprite in every SFF is decoded to pixels
   and atlas-packed at load. Big screenpacks decode to gigabytes.

Kurogane keeps the *behavior* and fixes both: a small `unsafe`-free Rust core,
memory-mapped SFF files, and pixel decoding that happens **on demand** under
an LRU byte budget.

## Measured (host benchmark, 5000-sprite synthetic SFF v2)

| Metric | Ikemen-style eager | Kurogane lazy |
|---|---|---|
| RAM after load | ≈ 51.5 MiB decoded pixels | **≈ 136 KiB** subheaders (+ file mapping) |
| Load time | decode-everything | **0.3 ms** (headers only) |
| Decode cost | — | 0.02 ms/sprite |
| Hot fight sweep (300 sprites/frame) | — | 0.24 ms, 3.1 MiB resident under a 16 MiB budget |

## Renderer — GLES 3.0, budgeted everywhere

The renderer (crate **krg-gfx**, live in v0.7.0) is designed around three
hard budgets instead of "as fast as possible, sort it out later":

* **Fill rate**: the game renders at native MUGEN resolution (320×240) into
  an FBO and reaches the screen as **one** letterboxed upscale quad — the
  phone GPU's per-frame work at panel resolution is a single textured blit.
* **CPU**: one shader, one pre-allocated vertex ring (packed 20-byte
  vertices: 2×f32 pos, 2×f32 uv, 4×u8 color; 8192 quads ≈ 1 MB, reused every
  frame — zero per-frame allocations), one `glDrawArrays` per texture/blend
  **run**. Rotation is CPU-side per rotated quad only; the shader is a
  single matrix multiply.
* **RAM/VRAM**: two stacked LRUs — sprites decode on demand through the
  format layer's `SpriteCache` (CPU, byte-budgeted) and live on the GPU as
  texture objects under their own 48 MB budget with last-use eviction. A
  full MUGEN roster never needs to fit; the working set does.

Pixels are **premultiplied RGBA** end-to-end (decode → upload → blend), so
alpha and additive blending are each one `glBlendFunc` switch and bilinear
sprite edges have no dark halo. The v0.8 scene looks like a fighting game:
a **title menu** (bundled bitmap FNT v2 font, dpad+A navigation) over the
dimmed stage, then the **fight demo** — the stage's `[BG]` layers (normal
+ parallax + tiling, per-element `trans`), both kfm fighters at the stage's
start positions facing each other, and the foreground layer on top. The
one renderer LRU is namespaced per source SFF (stage / char / font), so
all three stream through the same budget without collisions.

## Workspace layout

```
crates/
  krg-core/      lenient MUGEN text parsers (Atoi/Atof/SectionName — port of
                 common.go; every quirk preserved, incl. the Atof exponent bug)
  krg-format/    SFF v1/v2 + PCX + RLE8/RLE5/LZ5 + AIR compiler + stage DEF
                 parser + bitmap FNT v2 fonts (ports of image.go, anim.go,
                 stage.go, font.go), lazy Sff + SpriteCache LRU
  krg-gfx/       GLES 3.0 renderer: batched 20-byte-vertex sprite quads,
                 LRU texture streaming, 320×240 scene FBO + single upscale
  krg-input/     touch controller: 6 buttons + 8-way pad + START with chord
                 synthesis and a drag/pinch-editable persistent layout;
                 Ikemen InputBits-compatible
  krg-sfftool/   CLI: inspect/verify/benchmark content (host-only CI tool)
  krg-android/   JNI entry point (libkurogane.so): content scan + controller
                 + GL renderer bridge + the scene (menu → stage + fighters)
android/         Docker image, build script, Gradle APK project
```

## Building the Android app

Requires Docker (recommended) or a host with Rust, `cargo-ndk`, Android SDK
(platform 34, build-tools 34.0.0, NDK 26.3) and JDK 17+.

```bash
# one-time
docker build -t kurogane-android android/

# build the APK
docker run --rm -v "$PWD:/work" kurogane-android ./android/build.sh
```

Output: `android/app/build/outputs/apk/debug/app-debug.apk`

**CI**: every push to `main` runs the test suite, a **real-content corpus
check** (every SFF/AIR from the official Ikemen GO release must parse and
fully decode), and builds a debug APK — see the *Actions* tab →
`android-apk` → artifact `kurogane-debug-apk`.

## Base data ships with the APK

The APK bundles the official Ikemen GO base content (screenpack `data/`,
`font/`, the KFM characters, `stages/`, `external/` scripts — the same set
the official Android APK carries, ≈50 MB). On first launch the app asks for
storage access, then **creates `/storage/emulated/0/Kurogane and installs
the base data there automatically** — the folder you would otherwise have
to make by hand appears ready to use.

After that it behaves like any MUGEN install: drop your own characters
into `Kurogane/chars/<name>/` and stages into `Kurogane/stages/`, then tap
**RESCAN**. Existing files are never overwritten, so your edits survive
reinstalls. An existing `/storage/emulated/0/IkemenGO` folder is
detected and used as-is instead.

The bundle is pulled from the [Ikemen GO release zip](https://github.com/ikemen-engine/Ikemen-GO/releases)
at build time (`./android/bundle-basedata.sh`; cached in CI), never committed
to git.

## Host-side checks (CI)

```bash
cargo test                # 37 tests: round-trips, quirks, Go-parity, real-file fixtures
cargo run --release -p krg-sfftool --bin krg-bench
cargo run --release -p krg-sfftool -- path/to/char.sff --decode-all
cargo run --release -p krg-sfftool -- path/to/char.air
```

## Parity ledger (what is byte-exact vs the Go engine)

| Area | Status | Notes |
|---|---|---|
| Atoi / Atof / IsNumeric / SectionName | ✅ ported | incl. saturation (`IMax`/`IErr`) and the Atof signed-exponent bug |
| SFF v2 header (64-byte: fsho@36, n@40, fpho@44, npal@48, lofs@52, tofs@60) | ✅ ported | verified byte-for-byte vs real FF3 files (fight/glyphs.sff) |
| SFF v1 subheaders, linked sprites, ps flag | ✅ ported | first-wins duplicate keys, shareCopy semantics |
| SFF v1 PCX header, RLE w/ bpl padding strip | ✅ ported | incl. `k==bpl → n=1` quirk |
| SFF v1 palette scan (0x0C backward scan, char-first legacy path) | ✅ ported | incl. SetPxl silent-drop of length mismatches |
| SFF v2 subheaders (28-byte), ofs/tofs/lofs bases | ✅ ported | |
| RLE8 / RLE5 / LZ5 codecs | ✅ ported | byte-exact incl. index-freeze at `len-1` |
| SFF v2 palettes (pow2 clamp, 2.0.0.0 alpha forcing) | ✅ ported | |
| PNG sprites (10/11/12) + RGBA premultiply | ✅ ported | `SetRaw`'s `*0x101*a/0xff>>8` arithmetic |
| AIR frames/trans/clsn/copy-action/inherit | ✅ ported | incl. duplicate-first-wins & purge rules |
| InputBits (14 signals) + minimal controller | ✅ ported | bit-exact vs input.go:358; chords cover all signals |
| GLES 3.0 batched sprite renderer + LRU texture streaming | ✅ ported | krg-gfx: 20-byte vertices, run batching, FBO 320×240 + single upscale |
| isActPal (0,0) palette reset | ✅ ported | load flag |
| preloadSff / ACT palette preselect | ⏳ next | engine-layer milestone |
| CNS/CMD state compiler + bytecode VM | ✅ v0.9 | kfm.cns/kfm.cmd/common1.cns.zss compile from the real v1.0.0 release |
| Command system (input.go port) | ✅ v0.9 | SOCD/assist/charge/>/\$/\* syntax, buffer timers, .cmd compiler |
| Char runtime: states, physics, hit detection, rounds, camera | ✅ v0.9 | KFM walks/crouches/jumps/punches; clsn hits, gethit, KO, next round, P2 CPU |
| Gethit chain + fall/tech/getup (char.go 12581 port) | ✅ v0.9.1 | hitshaketime/hittime tick down, fall → liedown → getup → 0; air hits tumble with yaccel 0.35 gravity |
| ZSS function calls (`call IkSys_*`) | ✅ v0.9.1 | were a silent no-op stub; ret slots fixed; same-number changeAnim keeps progress (Ikemen object semantics) |
| Guard / blocking (char.go 11429 + 11650 port) | ✅ v0.9.1 | guardflag gate, chip damage, guard states 150-155, guard.* fallback chain (guard.velocity ← ground.velocity) |
| Match restart (rematch) | ✅ v0.9.1 | Fight::restart_match + MATCH OVER → TAP A: REMATCH scene flow |
| Sound, netplay | ⏳ roadmap | audio bridge is the next big one |

## Roadmap

1. ~~Format layer~~ ✅
2. ~~Minimal touch controller + APK pipeline~~ ✅
3. ~~GLES 3.0 renderer core~~ ✅ (batching/LRU/FBO/flip) — next: fight
   camera (boundleft/right, zoom), true parallax trapezoids, GameActivity
4. ~~DEF/INI stage + font rendering~~ ✅ (stage DEF, bitmap FNT v2) —
   next: motif `system.def` menus, TTF fonts
5. ~~CNS/CMD compiler + bytecode VM~~ ✅ (the 48K-line core)
6. Stage engine completion (bgctrl/velocity/animated BGs), audio
   (AAudio/Oboe) ← next, fight.def lifebars
7. Rollback netcode (strict-IEEE, frame-hash parity harness)
8. Lua scripting bridge (LuaJIT on arm64)

## License

MIT — see [LICENSE](LICENSE). Same as upstream Ikemen GO; the upstream engine
and its contributors are credited by reference.
