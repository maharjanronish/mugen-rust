# Kurogane (黒鉄)

**A Rust rewrite of the IKEMEN GO fighting game engine for Android 8+ (API 26)
— 100% content-compatible with the Go engine, with a fraction of the RAM,
and a clean minimal touch controller.**

Upstream reference: [ikemen-engine/Ikemen-GO](https://github.com/ikemen-engine/Ikemen-GO) (MIT).
Every ported function carries a `image.go:NNN`-style reference so behavior can
be audited line-by-line against the Go source.

---

## Controls — minimal buttons, every move

MUGEN content can require up to **14 input signals** (6 attack buttons, 4
directions, start, plus Ikemen's extra `d`/`w`/`m` buttons). Six on-screen
attack buttons is clutter — Kurogane ships **four** and synthesizes the rest
as simultaneous-press **chords**. Both signals are emitted at once, so
`QCF+Z`-style *and* `X+Y`-style commands all fire:

```
        ┌──────────────────────────────────────────────┐
        │  [≡] start                          HUD      │
        │                                              │
        │   ╭───╮                        Y   ○        │
        │   │ ● │  8-way pad        X  ○      ○  B    │
        │   ╰───╯                        A  ○         │
        └──────────────────────────────────────────────┘
```

| Physical | Emits |
|---|---|
| **X** light punch · **Y** heavy punch · **A** light kick · **B** heavy kick | X / Y / A / B |
| X+Y together | + **Z** (heavy punch) |
| A+B together | + **C** (heavy kick) |
| A+X together | + **D** · B+Y together | + **W** (Ikemen extras) |
| A+B+X+Y (all four) | + **M** |
| START | **S** |
| 8-way pad | PU/PD/PL/PR (physical; the engine maps F/B by facing) |

Quarter-circles, hold-back blocking, jumps and **double-tap dashes (`F,F`)**
all work natively — MUGEN command syntax reads them from the raw direction
stream, so no hidden controller magic is needed or wanted.

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

## Workspace layout

```
crates/
  krg-core/      lenient MUGEN text parsers (Atoi/Atof/SectionName — port of
                 common.go; every quirk preserved, incl. the Atof exponent bug)
  krg-format/    SFF v1/v2 + PCX + RLE8/RLE5/LZ5 + AIR compiler
                 (port of image.go & anim.go), lazy Sff + SpriteCache LRU
  krg-input/     minimal touch controller: 4 buttons + 8-way pad + START with
                 chord synthesis; Ikemen InputBits-compatible
  krg-sfftool/   CLI: inspect/verify/benchmark content (host-only CI tool)
  krg-android/   JNI entry point (libkurogane.so): content scan + controller
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

**CI**: every push to `main` runs the test suite and builds a debug APK —
see the *Actions* tab → `android-apk` → artifact `kurogane-debug-apk`.

## Getting MUGEN content onto the device

On first launch the app asks for storage access (classic permission dialog
on Android 8/9, the *All files access* toggle on Android 10+) and then scans,
in order:

1. `/storage/emulated/0/Kurogane` — **recommended**; any file manager works
2. `/storage/emulated/0/IkemenGO` — an existing Ikemen GO data folder is
   auto-detected
3. `Android/data/com.kurogane.engine/files/` — USB/ADB fallback

**Base data** — what the engine itself needs — is the `data/` and `font/`
folders from an [Ikemen GO release zip](https://github.com/ikemen-engine/Ikemen-GO/releases)
(or from a MUGEN 1.0/1.1 install). Drop them in so the folder looks like:

```
/storage/emulated/0/Kurogane/
  data/    system.def, fight.def, select.def, common1.cns …  (screenpack)
  font/    default fonts
  chars/   your characters — one folder each, containing a .def
  stages/  your stages (.def + .sff)
```

A screenpack (`data/system.def`) is the minimum for the scanner to accept a
folder as content. The launcher reports what it found (character/stage/font
counts, parsed SFF/AIR samples) on the HUD; the setup screen's **RESCAN**
button re-checks after you add files.

## Host-side checks (CI)

```bash
cargo test                # 34 tests: round-trips, quirks, Go-parity, controller
cargo run --release -p krg-sfftool --bin krg-bench
cargo run --release -p krg-sfftool -- path/to/char.sff --decode-all
cargo run --release -p krg-sfftool -- path/to/char.air
```

## Parity ledger (what is byte-exact vs the Go engine)

| Area | Status | Notes |
|---|---|---|
| Atoi / Atof / IsNumeric / SectionName | ✅ ported | incl. saturation (`IMax`/`IErr`) and the Atof signed-exponent bug |
| SFF v1 subheaders, linked sprites, ps flag | ✅ ported | first-wins duplicate keys, shareCopy semantics |
| SFF v1 PCX header, RLE w/ bpl padding strip | ✅ ported | incl. `k==bpl → n=1` quirk |
| SFF v1 palette scan (0x0C backward scan, char-first legacy path) | ✅ ported | incl. SetPxl silent-drop of length mismatches |
| SFF v2 subheaders (28-byte), ofs/tofs/lofs bases | ✅ ported | |
| RLE8 / RLE5 / LZ5 codecs | ✅ ported | byte-exact incl. index-freeze at `len-1` |
| SFF v2 palettes (pow2 clamp, 2.0.0.0 alpha forcing) | ✅ ported | |
| PNG sprites (10/11/12) + RGBA premultiply | ✅ ported | `SetRaw`'s `*0x101*a/0xff>>8` arithmetic |
| AIR frames/trans/clsn/copy-action/inherit | ✅ ported | incl. duplicate-first-wins & purge rules |
| InputBits (14 signals) + minimal controller | ✅ ported | bit-exact vs input.go:358; chords cover all signals |
| isActPal (0,0) palette reset | ✅ ported | load flag |
| preloadSff / ACT palette preselect | ⏳ next | engine-layer milestone |
| CNS/CMD bytecode, stages, sound, netplay | ⏳ roadmap | the big ones |

## Roadmap

1. ~~Format layer~~ ✅
2. ~~Minimal touch controller + APK pipeline~~ ✅
3. GLES 3.0 renderer + GameActivity (texture upload from `Pixels`)
4. DEF/INI config + font rendering (fnt/sff fonts)
5. CNS/CMD compiler + bytecode VM (the 48K-line core)
6. Stage engine, audio (AAudio/Oboe), input → engine wiring
7. Rollback netcode (strict-IEEE, frame-hash parity harness)
8. Lua scripting bridge (LuaJIT on arm64)

## License

MIT — see [LICENSE](LICENSE). Same as upstream Ikemen GO; the upstream engine
and its contributors are credited by reference.
