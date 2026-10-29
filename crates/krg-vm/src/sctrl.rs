//! Port origin: ikemen-src/src/compiler.go `scmap` (38-204) +
//! compiler_functions.go (per-sctrl param readers) + bytecode.go (per-sctrl
//! Run methods and param-id const blocks).
//!
//! Rust factoring: Go compiles each controller into a typed struct with a
//! virtual Run. We keep a table-driven form instead:
//! * [`Sctrl`] = `{ kind, params: Vec<(id, exps)> }`
//! * compile: one static param table per kind ([`PSpec`]) + a handful of
//!   hand-written special cases (var(n)=…, AssertSpecial flags, enums)
//! * run: [`run_sctrl`] evaluates the needed params against
//!   [`CharRuntime`] (implemented by runtime::Char)
//!
//! Enum-ish params (attr/guardflag/hitflag/animtype/postype/trans/…) are
//! parsed at compile time into constant ints — chars always use literals
//! and Go only tolerates expressions there in lenient mode. Our bit
//! mappings are internal to this engine (HitDef ↔ HitBy use the same one):
//! * attr stateType bits: S=1 C=2 A=4 L=8
//! * attr kind bits:      NA=1 SA=2 HA=4 NT=8 ST=16 HT=32 NP=64
//! * guardflag bits:      H=1 L=2 A=4
//! * hitflag bits:        H=1 L=2 A=4 D=8 F(fall)=16
//! * flag bit values for AssertSpecial mirror char.go:51 (ASF) and the
//!   GSF block (char.go:116 area) exactly.

use crate::eval::Ctx;
use crate::exp::Compiler;
use crate::ini::IniSection;
use crate::op::BytecodeExp;
use crate::statedef::{MoveTypeT, StateTypeT};
use crate::value::{v_int, Value, ValueType};

// ── kinds ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SctrlKind {
    ChangeState,
    SelfState,
    PosSet,
    PosAdd,
    PosFreeze,
    VelSet,
    VelAdd,
    VelMul,
    CtrlSet,
    ChangeAnim,
    ChangeAnim2,
    Turn,
    HitDef,
    AssertSpecial,
    PlaySnd,
    StopSnd,
    SndPan,
    Explod,
    ModifyExplod,
    RemoveExplod,
    NotHitBy,
    HitBy,
    HitOverride,
    TargetState,
    TargetBind,
    TargetLifeAdd,
    LifeAdd,
    LifeSet,
    PowerAdd,
    PowerSet,
    StateTypeSet,
    SprPriority,
    VarSet,
    VarAdd,
    VarRandom,
    VarRangeSet,
    EnvShake,
    ScreenBound,
    PlayerPush,
    Width,
    Pause,
    SuperPause,
    AfterImage,
    AfterImageTime,
    AngleDraw,
    AngleAdd,
    AngleMul,
    AngleSet,
    Trans,
    HitVelSet,
    HitFallSet,
    HitFallVel,
    HitFallDamage,
    DestroySelf,
    Helper,
    Projectile,
    DefenceMulSet,
    AttackMulSet,
    Gravity,
    MakeDust,
    GameMakeAnim,
    Offset,
    Null,
    MapSet,
    MapAdd,
    TargetFacing,
    TargetVelSet,
    TargetVelAdd,
    TargetVelMul,
    TargetDrop,
    TargetPowerAdd,
    HitAdd,
    PalFx,
    BgPalFx,
    AllPalFx,
    RemapPal,
    ModifyReversalDef,
    AttackDist,
    Zoom,
    Height,
    Depth,
    ForceFeedback,
    FallEnvShake,
    // ZSS-adjacent keeps
    ReversalDef,
    ModifyHitDef,
}

/// compiler.go:38 `scmap` — lowercased names → kinds (the KFM/common subset
/// plus forward-declared Ikemen ones).
pub fn kind_from_name(name: &str) -> Option<SctrlKind> {
    use SctrlKind::*;
    Some(match name.to_lowercase().as_str() {
        "changestate" => ChangeState,
        "selfstate" => SelfState,
        "posset" => PosSet,
        "posadd" => PosAdd,
        "posfreeze" => PosFreeze,
        "velset" => VelSet,
        "veladd" => VelAdd,
        "velmul" => VelMul,
        "ctrlset" => CtrlSet,
        "changeanim" => ChangeAnim,
        "changeanim2" => ChangeAnim2,
        "turn" => Turn,
        "hitdef" => HitDef,
        "assertspecial" => AssertSpecial,
        "playsnd" => PlaySnd,
        "stopsnd" => StopSnd,
        "sndpan" => SndPan,
        "explod" => Explod,
        "modifyexplod" => ModifyExplod,
        "removeexplod" => RemoveExplod,
        "nothitby" => NotHitBy,
        "hitby" => HitBy,
        "hitoverride" => HitOverride,
        "targetstate" => TargetState,
        "targetbind" => TargetBind,
        "targetlifeadd" => TargetLifeAdd,
        "lifeadd" => LifeAdd,
        "lifeset" => LifeSet,
        "poweradd" => PowerAdd,
        "powerset" => PowerSet,
        "statetypeset" => StateTypeSet,
        "movetypeset" => StateTypeSet,
        "sprpriority" => SprPriority,
        "varset" => VarSet,
        "varadd" => VarAdd,
        "varrandom" => VarRandom,
        "varrangeset" => VarRangeSet,
        "envshake" => EnvShake,
        "screenbound" => ScreenBound,
        "playerpush" => PlayerPush,
        "width" => Width,
        "pause" => Pause,
        "superpause" => SuperPause,
        "afterimage" => AfterImage,
        "afterimagetime" => AfterImageTime,
        "angledraw" => AngleDraw,
        "angleadd" => AngleAdd,
        "anglemul" => AngleMul,
        "angleset" => AngleSet,
        "trans" => Trans,
        "hitvelset" => HitVelSet,
        "hitfallset" => HitFallSet,
        "hitfallvel" => HitFallVel,
        "hitfalldamage" => HitFallDamage,
        "destroyself" => DestroySelf,
        "helper" => Helper,
        "projectile" => Projectile,
        "defencemulset" => DefenceMulSet,
        "attackmulset" => AttackMulSet,
        "gravity" => Gravity,
        "makedust" => MakeDust,
        "gamemakeanim" => GameMakeAnim,
        "offset" => Offset,
        "null" => Null,
        "mapset" => MapSet,
        "mapadd" => MapAdd,
        "reversaldef" => ReversalDef,
        "modifyhitdef" => ModifyHitDef,
        "targetfacing" => TargetFacing,
        "targetvelset" => TargetVelSet,
        "targetveladd" => TargetVelAdd,
        "targetvelmul" => TargetVelMul,
        "targetdrop" => TargetDrop,
        "targetpoweradd" => TargetPowerAdd,
        "hitadd" => HitAdd,
        "palfx" => PalFx,
        "bgpalfx" => BgPalFx,
        "allpalfx" => AllPalFx,
        "remappal" => RemapPal,
        "modifyreversaldef" => ModifyReversalDef,
        "attackdist" => AttackDist,
        "zoom" => Zoom,
        "height" => Height,
        "depth" => Depth,
        "forcefeedback" => ForceFeedback,
        "fallenvshake" => FallEnvShake,
        _ => return None,
    })
}

pub fn kind_name(kind: SctrlKind) -> &'static str {
    use SctrlKind::*;
    match kind {
        ChangeState => "ChangeState", SelfState => "SelfState", PosSet => "PosSet",
        PosAdd => "PosAdd", PosFreeze => "PosFreeze", VelSet => "VelSet",
        VelAdd => "VelAdd", VelMul => "VelMul", CtrlSet => "CtrlSet",
        ChangeAnim => "ChangeAnim", ChangeAnim2 => "ChangeAnim2", Turn => "Turn",
        HitDef => "HitDef", AssertSpecial => "AssertSpecial", PlaySnd => "PlaySnd",
        StopSnd => "StopSnd", SndPan => "SndPan", Explod => "Explod",
        ModifyExplod => "ModifyExplod", RemoveExplod => "RemoveExplod",
        NotHitBy => "NotHitBy", HitBy => "HitBy", HitOverride => "HitOverride",
        TargetState => "TargetState", TargetBind => "TargetBind",
        TargetLifeAdd => "TargetLifeAdd", LifeAdd => "LifeAdd", LifeSet => "LifeSet",
        PowerAdd => "PowerAdd", PowerSet => "PowerSet", StateTypeSet => "StateTypeSet",
        SprPriority => "SprPriority", VarSet => "VarSet", VarAdd => "VarAdd",
        VarRandom => "VarRandom", VarRangeSet => "VarRangeSet", EnvShake => "EnvShake",
        ScreenBound => "ScreenBound", PlayerPush => "PlayerPush", Width => "Width",
        Pause => "Pause", SuperPause => "SuperPause", AfterImage => "AfterImage",
        AfterImageTime => "AfterImageTime", AngleDraw => "AngleDraw",
        AngleAdd => "AngleAdd", AngleMul => "AngleMul", AngleSet => "AngleSet",
        Trans => "Trans", HitVelSet => "HitVelSet", HitFallSet => "HitFallSet",
        HitFallVel => "HitFallVel", HitFallDamage => "HitFallDamage",
        DestroySelf => "DestroySelf", Helper => "Helper", Projectile => "Projectile",
        DefenceMulSet => "DefenceMulSet", AttackMulSet => "AttackMulSet",
        Gravity => "Gravity", MakeDust => "MakeDust", GameMakeAnim => "GameMakeAnim",
        Offset => "Offset", Null => "Null", MapSet => "MapSet", MapAdd => "MapAdd",
        ReversalDef => "ReversalDef", ModifyHitDef => "ModifyHitDef",
        TargetFacing => "TargetFacing", TargetVelSet => "TargetVelSet",
        TargetVelAdd => "TargetVelAdd", TargetVelMul => "TargetVelMul",
        TargetDrop => "TargetDrop", TargetPowerAdd => "TargetPowerAdd",
        HitAdd => "HitAdd", PalFx => "PalFx", BgPalFx => "BgPalFx",
        AllPalFx => "AllPalFx", RemapPal => "RemapPal",
        ModifyReversalDef => "ModifyReversalDef", AttackDist => "AttackDist",
        Zoom => "Zoom", Height => "Height", Depth => "Depth",
        ForceFeedback => "ForceFeedback",
        FallEnvShake => "FallEnvShake",
    }
}

// ── param ids (per-kind namespaces; PREFIX_BIT marks F/S-prefixed forms) ─────

pub const PREFIX_BIT: u16 = 0x8000;

mod id {
    pub const VALUE: u16 = 0;
    pub const CTRL: u16 = 1;
    pub const ANIM: u16 = 2;
    pub const CONTINUE: u16 = 3;
    pub const READPLAYERID: u16 = 4;
    pub const REDIRECTID: u16 = 5;
    pub const X: u16 = 0;
    pub const Y: u16 = 1;
    pub const Z: u16 = 2;
    pub const ELEM: u16 = 6;
    pub const ELEMTIME: u16 = 7;
    pub const ANIMPLAYERNO: u16 = 8;
    pub const SPRITEPLAYERNO: u16 = 9;
    pub const ID: u16 = 10;
    pub const INDEX: u16 = 11;
    pub const TIME: u16 = 12;
    pub const ABSOLUTE: u16 = 13;
    pub const KILL: u16 = 14;
    pub const VARTYPE: u16 = 15;
    pub const RANGE: u16 = 16;
    pub const FIRST: u16 = 17;
    pub const LAST: u16 = 18;
    pub const FVALUE: u16 = 19;
    pub const AMPL: u16 = 20;
    pub const FREQ: u16 = 21;
    pub const PHASE: u16 = 22;
    pub const MUL: u16 = 23;
    pub const MOVECAMERA: u16 = 24;
    pub const PRIORITY: u16 = 25;
    pub const AFFECTTEAM: u16 = 26;
    pub const EDGE: u16 = 27;
    pub const HEIGHT: u16 = 28;
    pub const MOVETIME: u16 = 29;
    pub const PAUSEBG: u16 = 30;
    pub const ENDCMDBUFTIME: u16 = 31;
    pub const DARKEN: u16 = 32;
    pub const BRIGHTNESS: u16 = 33;
    pub const POS: u16 = 34;
    pub const P2DEFMUL: u16 = 35;
    pub const UNHITTABLE: u16 = 36;
    pub const SOUND: u16 = 37;
    pub const SCALE: u16 = 38;
    pub const CHANNEL: u16 = 39;
    pub const PAN: u16 = 40;
    pub const ABSPAN: u16 = 41;
    pub const VOLUME: u16 = 42;
    pub const VOLUMESCALE: u16 = 43;
    pub const FREQMUL: u16 = 44;
    pub const LOOP: u16 = 45;
    pub const LOWPRIORITY: u16 = 46;
    pub const STOPONGETHIT: u16 = 47;
    pub const STOPONCHANGESTATE: u16 = 48;
    pub const RECURSIVE: u16 = 49;
    pub const REMOVEEXPLODS: u16 = 50;
    pub const STATETYPE: u16 = 51;
    pub const MOVETYPE: u16 = 52;
    pub const PHYSICS: u16 = 53;
    pub const FORCEAIR: u16 = 54;
    pub const KEEPSTATE: u16 = 55;
    pub const FORCEGUARD: u16 = 56;
    pub const FORCESTAND: u16 = 57;
    pub const FORCECROUCH: u16 = 58;
    pub const VEL: u16 = 59;
    pub const ACCEL: u16 = 60;
    pub const FACING: u16 = 61;
    pub const REMOVETIME: u16 = 62;
    pub const SUPERMOVETIME: u16 = 63;
    pub const PAUSEMOVETIME: u16 = 64;
    pub const SPRPRIORITY: u16 = 65;
    pub const ONTOP: u16 = 66;
    pub const LAYERNO: u16 = 67;
    pub const SHADOW: u16 = 68;
    pub const OWNpal: u16 = 69;
    pub const WINDOW: u16 = 70;
    pub const IGNOREHITPAUSE: u16 = 71;
    pub const BINDTIME: u16 = 72;
    pub const VFACING: u16 = 73;
    pub const RANDOM: u16 = 74;
    pub const TRANS: u16 = 75;
    pub const ALPHA: u16 = 76;
    pub const NAME: u16 = 77;
    pub const HELPERTYPE: u16 = 78;
    pub const KEYCTRL: u16 = 79;
    pub const SIZEALL: u16 = 80;
    pub const INITCTRL: u16 = 81;
    // varSet old/new syntax
    pub const VARSLOT: u16 = 90;
    pub const VARVALUE: u16 = 91;
    // assertSpecial
    pub const FLAG: u16 = 100;
    pub const FLAG_G: u16 = 101;
    pub const ENABLED: u16 = 102;
    pub const NOKO: u16 = 103;
    pub const KEEPONE: u16 = 104;
    pub const DESTINATION: u16 = 105;
}

// ── compile side ─────────────────────────────────────────────────────────────

/// One param-table row.
pub struct PSpec {
    pub name: &'static str,
    pub id: u16,
    pub vt: ValueType,
    pub nargs: usize,
    pub mandatory: bool,
    pub prefixable: bool,
    /// getDataPrefix ffxDefault — hitdef sounds/sparks default to "f"
    /// (compiler_functions.go v1.0.0: hsnd/sprk use true; PlaySnd false)
    pub ffx_default: bool,
}

const fn ps(name: &'static str, i: u16, vt: ValueType, nargs: usize) -> PSpec {
    PSpec { name, id: i, vt, nargs, mandatory: false, prefixable: false, ffx_default: false }
}
const fn psm(name: &'static str, i: u16, vt: ValueType, nargs: usize) -> PSpec {
    PSpec { name, id: i, vt, nargs, mandatory: true, prefixable: false, ffx_default: false }
}
const fn psp(name: &'static str, i: u16, vt: ValueType, nargs: usize) -> PSpec {
    PSpec { name, id: i, vt, nargs, mandatory: false, prefixable: true, ffx_default: false }
}
const fn pspm(name: &'static str, i: u16, vt: ValueType, nargs: usize) -> PSpec {
    PSpec { name, id: i, vt, nargs, mandatory: true, prefixable: true, ffx_default: false }
}
/// prefixable + ffxDefault=true (HitDef sounds/sparks, GameMakeAnim)
const fn pspf(name: &'static str, i: u16, vt: ValueType, nargs: usize) -> PSpec {
    PSpec { name, id: i, vt, nargs, mandatory: false, prefixable: true, ffx_default: true }
}

/// The generic controller with plain expression params.
fn table(kind: SctrlKind) -> Vec<PSpec> {
    use SctrlKind::*;
    use ValueType::*;
    match kind {
        ChangeState | SelfState => vec![
            psm("value", id::VALUE, Int, 1),
            ps("ctrl", id::CTRL, Int, 1),
            psp("anim", id::ANIM, Int, 1),
            ps("continue", id::CONTINUE, Bool, 1),
            ps("readplayerid", id::READPLAYERID, Int, 1),
            ps("redirectid", id::REDIRECTID, Int, 1),
        ],
        PosSet | PosAdd => vec![
            ps("x", id::X, Float, 1),
            ps("y", id::Y, Float, 1),
            ps("z", id::Z, Float, 1),
            ps("redirectid", id::REDIRECTID, Int, 1),
        ],
        PosFreeze => vec![
            ps("value", id::VALUE, Bool, 1),
            ps("redirectid", id::REDIRECTID, Int, 1),
        ],
        VelSet | VelAdd | VelMul => vec![
            ps("x", id::X, Float, 1),
            ps("y", id::Y, Float, 1),
            ps("z", id::Z, Float, 1),
            ps("redirectid", id::REDIRECTID, Int, 1),
        ],
        CtrlSet => vec![
            psm("value", id::VALUE, Bool, 1),
            ps("redirectid", id::REDIRECTID, Int, 1),
        ],
        ChangeAnim => vec![
            pspm("value", id::VALUE, Int, 1),
            ps("elem", id::ELEM, Int, 1),
            ps("elemtime", id::ELEMTIME, Int, 1),
            ps("animplayerno", id::ANIMPLAYERNO, Int, 1),
            ps("spriteplayerno", id::SPRITEPLAYERNO, Int, 1),
            ps("readplayerid", id::READPLAYERID, Int, 1),
        ],
        ChangeAnim2 => vec![
            pspm("value", id::VALUE, Int, 1),
            ps("elem", id::ELEM, Int, 1),
            ps("elemtime", id::ELEMTIME, Int, 1),
            ps("readplayerid", id::READPLAYERID, Int, 1),
        ],
        Turn => vec![ps("redirectid", id::REDIRECTID, Int, 1)],
        PlaySnd => vec![
            pspm("value", id::VALUE, Int, 3),
            ps("channel", id::CHANNEL, Int, 1),
            ps("lowpriority", id::LOWPRIORITY, Bool, 1),
            ps("pan", id::PAN, Float, 1),
            ps("abspan", id::ABSPAN, Float, 1),
            ps("volume", id::VOLUME, Int, 1),
            ps("volumescale", id::VOLUMESCALE, Int, 1),
            ps("freqmul", id::FREQMUL, Float, 1),
            ps("loop", id::LOOP, Bool, 1),
            ps("priority", id::PRIORITY, Int, 1),
            ps("stopongethit", id::STOPONGETHIT, Bool, 1),
            ps("stoponchangestate", id::STOPONCHANGESTATE, Bool, 1),
        ],
        StopSnd => vec![ps("channel", id::CHANNEL, Int, 1)],
        SndPan => vec![
            ps("channel", id::CHANNEL, Int, 1),
            ps("pan", id::PAN, Float, 1),
            ps("abspan", id::ABSPAN, Float, 1),
        ],
        RemoveExplod => vec![ps("id", id::ID, Int, 1)],
        TargetState => vec![
            ps("id", id::ID, Int, 1),
            ps("index", id::INDEX, Int, 1),
            psm("value", id::VALUE, Int, 1),
        ],
        TargetBind => vec![
            ps("id", id::ID, Int, 1),
            ps("index", id::INDEX, Int, 1),
            ps("time", id::TIME, Int, 1),
            ps("pos", id::POS, Float, 3),
        ],
        TargetLifeAdd => vec![
            ps("id", id::ID, Int, 1),
            ps("index", id::INDEX, Int, 1),
            ps("absolute", id::ABSOLUTE, Bool, 1),
            ps("kill", id::KILL, Bool, 1),
            psm("value", id::VALUE, Int, 1),
        ],
        LifeAdd => vec![
            ps("absolute", id::ABSOLUTE, Bool, 1),
            ps("kill", id::KILL, Bool, 1),
            psm("value", id::VALUE, Int, 1),
        ],
        LifeSet | PowerAdd | PowerSet => vec![psm("value", id::VALUE, Int, 1)],
        SprPriority => vec![psm("value", id::VALUE, Int, 1)],
        VarRandom => vec![
            ps("v", id::VARSLOT, Int, 1),
            ps("range", id::RANGE, Int, 2),
        ],
        VarRangeSet => vec![
            ps("first", id::FIRST, Int, 1),
            ps("last", id::LAST, Int, 1),
            ps("value", id::VALUE, Int, 1),
            ps("fvalue", id::FVALUE, Float, 1),
        ],
        EnvShake | FallEnvShake => vec![
            ps("time", id::TIME, Int, 1),
            ps("ampl", id::AMPL, Int, 1),
            ps("freq", id::FREQ, Float, 1),
            ps("phase", id::PHASE, Float, 1),
            ps("mul", id::MUL, Float, 1),
        ],
        ScreenBound => vec![
            ps("value", id::VALUE, Bool, 1),
            ps("movecamera", id::MOVECAMERA, Bool, 2),
        ],
        PlayerPush => vec![
            ps("value", id::VALUE, Bool, 1),
            ps("priority", id::PRIORITY, Int, 1),
        ],
        Width => vec![
            ps("value", id::VALUE, Float, 2),
            ps("edge", id::EDGE, Float, 2),
            ps("height", id::HEIGHT, Float, 2),
        ],
        Pause => vec![
            psm("time", id::TIME, Int, 1),
            ps("movetime", id::MOVETIME, Int, 1),
            ps("pausebg", id::PAUSEBG, Bool, 1),
            ps("endcmdbuftime", id::ENDCMDBUFTIME, Int, 1),
        ],
        AfterImageTime => vec![psm("time", id::TIME, Int, 1)],
        AngleAdd | AngleMul | AngleSet => vec![
            psm("value", id::VALUE, Float, 1),
            ps("redirectid", id::REDIRECTID, Int, 1),
        ],
        AngleDraw => vec![
            ps("value", id::VALUE, Float, 1),
            ps("scale", id::SCALE, Float, 2),
        ],
        HitVelSet => vec![
            ps("x", id::X, Bool, 1),
            ps("y", id::Y, Bool, 1),
            ps("z", id::Z, Bool, 1),
        ],
        HitFallSet => vec![
            ps("value", id::VALUE, Int, 1),
            ps("xvel", id::X, Float, 1),
            ps("yvel", id::Y, Float, 1),
        ],
        DestroySelf => vec![
            ps("recursive", id::RECURSIVE, Bool, 1),
            ps("removeexplods", id::REMOVEEXPLODS, Bool, 1),
        ],
        DefenceMulSet | AttackMulSet => vec![psm("value", id::VALUE, Float, 1)],
        Offset => vec![
            ps("x", id::X, Float, 1),
            ps("y", id::Y, Float, 1),
        ],
        MakeDust => vec![
            ps("pos", id::POS, Float, 2),
            ps("spacing", id::TIME, Int, 1),
        ],
        Gravity | HitFallVel | HitFallDamage | Null => vec![],
        TargetFacing => vec![
            ps("id", id::ID, ValueType::Int, 1),
            ps("index", id::INDEX, ValueType::Int, 1),
            ps("value", id::VALUE, ValueType::Int, 1),
        ],
        TargetVelSet | TargetVelAdd | TargetVelMul => vec![
            ps("id", id::ID, ValueType::Int, 1),
            ps("index", id::INDEX, ValueType::Int, 1),
            ps("x", id::X, ValueType::Float, 1),
            ps("y", id::Y, ValueType::Float, 1),
            ps("z", id::Z, ValueType::Float, 1),
        ],
        TargetDrop => vec![
            ps("id", id::ID, ValueType::Int, 1),
            ps("excludeid", id::VALUE, ValueType::Int, 1),
            ps("keepone", id::KEEPONE, ValueType::Bool, 1),
        ],
        TargetPowerAdd => vec![
            ps("id", id::ID, ValueType::Int, 1),
            ps("index", id::INDEX, ValueType::Int, 1),
            psm("value", id::VALUE, ValueType::Int, 1),
        ],
        HitAdd => vec![psm("value", id::VALUE, ValueType::Int, 1)],
        AttackDist => vec![
            ps("value", id::X, ValueType::Float, 1),
            ps("air", id::Y, ValueType::Float, 1),
        ],
        Zoom => vec![psm("value", id::VALUE, ValueType::Float, 1)],
        Height => vec![
            ps("value", id::VALUE, ValueType::Float, 2),
            ps("top", id::VALUE, ValueType::Float, 2),
            ps("bottom", id::VALUE + 2, ValueType::Float, 2),
        ],
        Depth => vec![
            ps("value", id::VALUE, ValueType::Float, 2),
            ps("top", id::VALUE, ValueType::Float, 2),
            ps("bottom", id::VALUE + 2, ValueType::Float, 2),
        ],
        _ => vec![],
    }
}

/// Enum-string → constant-int bit mappings (internal, consistent across
/// HitDef / HitBy / NotHitBy / ReversalDef).

pub fn attr_state_bits(s: &str) -> i32 {
    let mut v = 0;
    for c in s.to_uppercase().chars() {
        v |= match c {
            'S' => 1,
            'C' => 2,
            'A' => 4,
            'L' => 8,
            _ => 0,
        };
    }
    v
}
pub fn attr_kind_bits(s: &str) -> i32 {
    let mut v = 0i32;
    for c in s.to_uppercase().chars() {
        v |= match c {
            'N' => 1,
            'S' => 2,
            'H' => 4,
            'T' => 8,
            'P' => 16,
            _ => 0,
        };
    }
    v
}
/// Guard flags (compiler.go:1446 flagSub): H=1 L=2 M=H|L A=4 D=8 F=16 P=32
pub fn guardflag_bits(s: &str) -> i32 {
    let mut v = 0;
    for c in s.to_uppercase().chars() {
        v |= match c {
            'H' => 1,
            'L' => 2,
            'M' => 1 | 2,
            'A' => 4,
            'D' => 8,
            'F' => 16,
            'P' => 32,
            _ => 0,
        };
    }
    v
}
/// Hit flags (same bit space as guardflag; '-'/'+' handled separately —
/// they are trailing sign markers in Go, not OR'd members)
pub fn hitflag_bits(s: &str) -> (i32, bool, bool) {
    let mut v = 0;
    let mut only_gethit = false;
    let mut not_gethit = false;
    for c in s.chars() {
        match c.to_ascii_uppercase() {
            'H' => v |= 1,
            'L' => v |= 2,
            'M' => v |= 1 | 2,
            'A' => v |= 4,
            'D' => v |= 8,
            'F' => v |= 16,
            'P' => v |= 32,
            '-' => only_gethit = true,
            '+' => not_gethit = true,
            _ => {}
        }
    }
    (v, only_gethit, not_gethit)
}
/// animtype enum (gethitvar.animtype / HitDef animtype)
pub fn animtype_val(s: &str) -> i32 {
    match s.trim().to_lowercase().as_str() {
        "light" => 0,
        "medium" | "med" => 1,
        "hard" | "heavy" => 2,
        "back" => 3,
        "up" => 4,
        "diagup" => 5,
        "none" => 6,
        _ => 0,
    }
}
pub fn hittype_val(s: &str) -> i32 {
    match s.trim().to_lowercase().as_str() {
        "high" => 1,
        "low" => 2,
        "trip" => 3,
        "none" => 0,
        _ => 4, // normal
    }
}
pub fn affectteam_val(s: &str) -> i32 {
    match s.trim().to_lowercase().as_str() {
        "e" => 0,
        "b" => 1,
        "f" => 2,
        _ => 0,
    }
}
pub fn priority_type_val(s: &str) -> i32 {
    match s.trim().to_lowercase().as_str() {
        "hit" => 0,
        "miss" => 1,
        "dodge" => 2,
        _ => 0,
    }
}
pub fn postype_val(s: &str) -> i32 {
    match s.trim().to_lowercase().as_str() {
        "p1" => 0,
        "p2" => 1,
        "front" | "f" => 2,
        "back" | "b" => 3,
        "left" | "l" => 4,
        "right" | "r" => 5,
        "none" => 6,
        _ => 0,
    }
}
pub fn trans_val(s: &str) -> i32 {
    match s.trim().to_lowercase().as_str() {
        "none" => 0,
        "add" => 1,
        "add1" => 2,
        "sub" => 3,
        "add2" => 4,
        "" => 0,
        _ => 0,
    }
}

/// ASF bit values (char.go:51 — exact iota order).
pub mod asf {
    pub const INVISIBLE: i64 = 1 << 0;
    pub const NOAIRGUARD: i64 = 1 << 1;
    pub const NOAUTOTURN: i64 = 1 << 2;
    pub const NOCROUCHGUARD: i64 = 1 << 3;
    pub const NOJUGGLECHECK: i64 = 1 << 4;
    pub const NOKO: i64 = 1 << 5;
    pub const NOSHADOW: i64 = 1 << 6;
    pub const NOSTANDGUARD: i64 = 1 << 7;
    pub const NOWALK: i64 = 1 << 8;
    pub const UNGUARDABLE: i64 = 1 << 9;
    pub const ANIMATEHITPAUSE: i64 = 1 << 10;
    pub const ANIMFREEZE: i64 = 1 << 11;
    pub const AUTOGUARD: i64 = 1 << 12;
    pub const DRAWUNDER: i64 = 1 << 13;
    pub const NOAIBUTTONJAM: i64 = 1 << 14;
    pub const NOAICHEAT: i64 = 1 << 15;
    pub const NOAILEVEL: i64 = 1 << 16;
    pub const NOAIRJUMP: i64 = 1 << 17;
    pub const NOBRAKE: i64 = 1 << 18;
    pub const NOCOMBODISPLAY: i64 = 1 << 19;
    pub const NOCORNERPUSH: i64 = 1 << 20;
    pub const NOCROUCH: i64 = 1 << 21;
    pub const NOFACEDISPLAY: i64 = 1 << 23;
    pub const NOFACEP2: i64 = 1 << 24;
    pub const NOFALLCOUNT: i64 = 1 << 25;
    pub const NOFALLDEFENCEUP: i64 = 1 << 26;
    pub const NOFALLHITFLAG: i64 = 1 << 27;
    pub const NOFASTRECOVERFROMLIEDOWN: i64 = 1 << 28;
    pub const NOGETUPFROMLIEDOWN: i64 = 1 << 29;
    pub const NOGUARDBARDISPLAY: i64 = 1 << 30;
    pub const NOHARDCODEDKEYS: i64 = 1 << 34;
    pub const NOINPUT: i64 = 1 << 36;
    pub const NOINTRORESET: i64 = 1 << 37;
    pub const NOJUMP: i64 = 1 << 38;
    pub const NOKOFALL: i64 = 1 << 39;
    pub const NOKOVELOCITY: i64 = 1 << 40;
    pub const NOLIFEBARDISPLAY: i64 = 1 << 42;
    pub const NOMAKEDUST: i64 = 1 << 43;
    pub const NONAMEDISPLAY: i64 = 1 << 44;
    pub const NOPOWERBARDISPLAY: i64 = 1 << 45;
    pub const NOREDLIFEDAMAGE: i64 = 1 << 46;
    pub const NOSCORE: i64 = 1 << 47;
    pub const NOSTAND: i64 = 1 << 48;
    pub const POSTROUNDINPUT: i64 = 1 << 52;
    pub const RUNFIRST: i64 = 1 << 54;
    pub const RUNLAST: i64 = 1 << 55;
    pub const SIZEPUSHONLY: i64 = 1 << 56;
    pub const NODESTROYSELF: i64 = 1 << 57;
}

/// GSF bit values (char.go GSF block — exact iota order).
pub mod gsf {
    pub const GLOBALNOKO: i64 = 1 << 0;
    pub const GLOBALNOSHADOW: i64 = 1 << 1;
    pub const INTRO: i64 = 1 << 2;
    pub const NOBARDISPLAY: i64 = 1 << 3;
    pub const NOBG: i64 = 1 << 4;
    pub const NOFG: i64 = 1 << 5;
    pub const NOKOSLOW: i64 = 1 << 6;
    pub const NOKOSND: i64 = 1 << 7;
    pub const NOMUSIC: i64 = 1 << 8;
    pub const ROUNDNOTOVER: i64 = 1 << 9;
    pub const TIMERFREEZE: i64 = 1 << 10;
    pub const CAMERAFREEZE: i64 = 1 << 11;
    pub const NOTIMEDISPLAY: i64 = 1 << 12;
}

fn assert_special_flag(s: &str) -> Option<(i64, bool, bool)> {
    // (bit, is_global, is_noko_special)
    use asf::*;
    use gsf::*;
    Some(match s.trim().to_lowercase().as_str() {
        "intro" => (INTRO, true, false),
        "invisible" => (INVISIBLE, false, false),
        "noairguard" => (NOAIRGUARD, false, false),
        "noautoturn" => (NOAUTOTURN, false, false),
        "nocrouchguard" => (NOCROUCHGUARD, false, false),
        "nojugglecheck" => (NOJUGGLECHECK, false, false),
        "noko" => (0, false, true),
        "noshadow" => (NOSHADOW, false, false),
        "nostandguard" => (NOSTANDGUARD, false, false),
        "nowalk" => (NOWALK, false, false),
        "unguardable" => (UNGUARDABLE, false, false),
        "globalnoko" => (GLOBALNOKO, true, false),
        "globalnoshadow" => (GLOBALNOSHADOW, true, false),
        "nobardisplay" => (NOBARDISPLAY, true, false),
        "nobg" => (NOBG, true, false),
        "nofg" => (NOFG, true, false),
        "nokoslow" => (NOKOSLOW, true, false),
        "nokosnd" => (NOKOSND, true, false),
        "nomusic" => (NOMUSIC, true, false),
        "roundnotover" => (ROUNDNOTOVER, true, false),
        "timerfreeze" => (TIMERFREEZE, true, false),
        "animatehitpause" => (ANIMATEHITPAUSE, false, false),
        "animfreeze" => (ANIMFREEZE, false, false),
        "autoguard" => (AUTOGUARD, false, false),
        "drawunder" => (DRAWUNDER, false, false),
        "noaibuttonjam" => (NOAIBUTTONJAM, false, false),
        "noaicheat" => (NOAICHEAT, false, false),
        "noailevel" => (NOAILEVEL, false, false),
        "noairjump" => (NOAIRJUMP, false, false),
        "nobrake" => (NOBRAKE, false, false),
        "nocombodisplay" => (NOCOMBODISPLAY, false, false),
        "nocornerpush" => (NOCORNERPUSH, false, false),
        "nocrouch" => (NOCROUCH, false, false),
        "nofacedisplay" => (NOFACEDISPLAY, false, false),
        "nofacep2" => (NOFACEP2, false, false),
        "nofallcount" => (NOFALLCOUNT, false, false),
        "nofalldefenceup" => (NOFALLDEFENCEUP, false, false),
        "nofallhitflag" => (NOFALLHITFLAG, false, false),
        "nofastrecoverfromliedown" => (NOFASTRECOVERFROMLIEDOWN, false, false),
        "nogetupfromliedown" => (NOGETUPFROMLIEDOWN, false, false),
        "noguardbardisplay" => (NOGUARDBARDISPLAY, false, false),
        "nohardcodedkeys" => (NOHARDCODEDKEYS, false, false),
        "noinput" => (NOINPUT, false, false),
        "nointroreset" => (NOINTRORESET, false, false),
        "nojump" => (NOJUMP, false, false),
        "nokofall" => (NOKOFALL, false, false),
        "nokovelocity" => (NOKOVELOCITY, false, false),
        "nolifebardisplay" => (NOLIFEBARDISPLAY, false, false),
        "nomakedust" => (NOMAKEDUST, false, false),
        "nonamedisplay" => (NONAMEDISPLAY, false, false),
        "nopowerbardisplay" => (NOPOWERBARDISPLAY, false, false),
        "noredlifedamage" => (NOREDLIFEDAMAGE, false, false),
        "noscore" => (NOSCORE, false, false),
        "nostand" => (NOSTAND, false, false),
        "postroundinput" => (POSTROUNDINPUT, false, false),
        "runfirst" => (RUNFIRST, false, false),
        "runlast" => (RUNLAST, false, false),
        "sizepushonly" => (SIZEPUSHONLY, false, false),
        _ => return None,
    })
}

// ── Sctrl struct ─────────────────────────────────────────────────────────────

/// One compiled state controller.
#[derive(Debug, Clone)]
pub struct Sctrl {
    pub kind: SctrlKind,
    pub ignorehitpause: bool,
    pub params: Vec<(u16, BytecodeExp)>,
}

impl Sctrl {
    pub fn get(&self, pid: u16) -> Option<&BytecodeExp> {
        self.params.iter().find(|(i, _)| *i == pid).map(|(_, e)| e)
    }
    /// multi-arg params are stored as separate (id+n, exp) rows
    pub fn get_arg(&self, pid: u16, n: usize) -> Option<&BytecodeExp> {
        self.params
            .iter()
            .find(|(i, _)| *i == pid + n as u16)
            .map(|(_, e)| e)
    }
    pub fn has(&self, pid: u16) -> bool {
        self.params.iter().any(|(i, _)| *i == pid)
    }
}

/// Multi-arg values are flattened: `value = 5, 0` → ids VALUE+0, VALUE+1.
/// This mirrors Go's append(topbe, bes...) per-arg layout.
#[derive(Debug, Clone)]
pub struct ParamBuilder {
    out: Vec<(u16, BytecodeExp)>,
}

impl ParamBuilder {
    pub fn new() -> Self {
        ParamBuilder { out: Vec::new() }
    }
    pub fn push(&mut self, pid: u16, exp: BytecodeExp) {
        self.out.push((pid, exp));
    }
    pub fn push_const(&mut self, pid: u16, v: Value) {
        self.out.push((pid, vec![crate::op::Op::Push(v)]));
    }
    pub fn push_const_i(&mut self, pid: u16, v: i32) {
        self.push_const(pid, v_int(v));
    }
    pub fn finish(self, kind: SctrlKind, ignorehitpause: bool) -> Sctrl {
        Sctrl { kind, ignorehitpause, params: self.out }
    }
}

/// exprs (compiler.go:6527) — comma-separated sub-expressions, up to
/// `num_arg`; missing args break out (runtime defaults apply).
fn exprs(
    comp: &mut Compiler,
    data: &str,
    vt: ValueType,
    num_arg: usize,
) -> Result<Vec<BytecodeExp>, String> {
    let mut input = data;
    let mut bes: Vec<BytecodeExp> = Vec::new();
    for n in 1..=num_arg {
        let be = if n < num_arg {
            comp.arg_expression(&mut input, vt)?
        } else {
            comp.full_expression(&mut input, vt)?
        };
        bes.push(be);
        if comp.trailing_comma() {
            // more args available
        } else {
            break;
        }
    }
    Ok(bes)
}

// ── HitDef param ids ─────────────────────────────────────────────────────────

pub mod hd {
    pub const ATTR_ST: u16 = 0;      // stateType part (S/C/A/L bits)
    pub const ATTR_KIND: u16 = 1;    // attack attr part (NA/SA/HA/… bits)
    pub const GUARDFLAG: u16 = 2;
    pub const HITFLAG: u16 = 3;
    pub const HITFLAG_MINUS: u16 = 4;  // '-' present
    pub const HITFLAG_PLUS: u16 = 5;   // '+' present
    pub const AFFECTTEAM: u16 = 6;
    pub const ID: u16 = 7;
    pub const CHAINID: u16 = 8;
    pub const KILL: u16 = 9;
    pub const GUARD_KILL: u16 = 10;
    pub const FALL_KILL: u16 = 11;
    pub const HITONCE: u16 = 12;
    pub const AIR_JUGGLE: u16 = 13;
    pub const GETPOWER: u16 = 14;    // 2 args
    pub const GIVEPOWER: u16 = 16;   // 2 args
    pub const DAMAGE: u16 = 18;      // 2 args (hit, guard)
    pub const NUMHITS: u16 = 20;
    pub const HITSOUND: u16 = 21;    // 2 args, prefixable
    pub const GUARDSOUND: u16 = 23;  // 2 args, prefixable
    pub const PRIORITY: u16 = 25;    // int
    pub const PRIORITY_TYPE: u16 = 26; // hit/miss/dodge
    pub const GROUND_TYPE: u16 = 27;
    pub const AIR_TYPE: u16 = 28;
    pub const FALL_TYPE: u16 = 29;
    pub const ANIMTYPE: u16 = 30;
    pub const AIR_ANIMTYPE: u16 = 31;
    pub const FALL_ANIMTYPE: u16 = 32;
    pub const P1STATENO: u16 = 33;
    pub const P2STATENO: u16 = 34;
    pub const P2GETP1STATE: u16 = 35;
    pub const P1SPRPRIORITY: u16 = 36;
    pub const P2SPRPRIORITY: u16 = 37;
    pub const FORCESTAND: u16 = 38;
    pub const FORCECROUCH: u16 = 39;
    pub const FALL_DAMAGE: u16 = 40;
    pub const FALL_XVELOCITY: u16 = 41;
    pub const FALL_YVELOCITY: u16 = 42;
    pub const FALL_RECOVER: u16 = 43;
    pub const FALL_RECOVERTIME: u16 = 44;
    pub const SPARKNO: u16 = 45;      // prefixable
    pub const GUARD_SPARKNO: u16 = 46; // prefixable
    pub const SPARKXY: u16 = 47;      // 2 args
    pub const DOWN_HITTIME: u16 = 49;
    pub const P1FACING: u16 = 50;
    pub const P1GETP2FACING: u16 = 51;
    pub const P2FACING: u16 = 52;
    pub const MINDIST: u16 = 53;     // 3 args
    pub const MAXDIST: u16 = 56;     // 3 args
    pub const SNAP: u16 = 59;        // 4 args
    pub const AIR_HITTIME: u16 = 63;
    pub const FALL: u16 = 64;
    pub const AIR_FALL: u16 = 65;
    pub const AIR_VELOCITY: u16 = 66;  // 3 args
    pub const AIRGUARD_VELOCITY: u16 = 69;
    pub const GUARD_VELOCITY: u16 = 72;
    pub const GROUND_VELOCITY: u16 = 75;  // 2 args (x[,y])
    pub const GROUND_HITTIME: u16 = 77;
    pub const GUARD_HITTIME: u16 = 78;
    pub const GUARD_CTRLTIME: u16 = 79;
    pub const AIRGUARD_CTRLTIME: u16 = 80;
    pub const GROUND_SLIDETIME: u16 = 81;
    pub const GUARD_SLIDETIME: u16 = 82;
    pub const DOWN_BOUNCE: u16 = 83;
    pub const DOWN_VELOCITY: u16 = 84;   // 3 args
    pub const DOWN_RECOVER: u16 = 87;
    pub const DOWN_RECOVERTIME: u16 = 88;
    pub const XACCEL: u16 = 89;
    pub const YACCEL: u16 = 90;
    pub const ENVSHAKE_TIME: u16 = 91;
    pub const ENVSHAKE_FREQ: u16 = 92;
    pub const ENVSHAKE_AMPL: u16 = 93;
    pub const ENVSHAKE_PHASE: u16 = 94;
    pub const REDLIFE: u16 = 95;     // 2 args
    pub const PAUSETIME: u16 = 96;   // 2 args (attacker, defender)
    pub const GUARD_PAUSETIME: u16 = 98; // 2 args (attacker, defender)
    pub const REDIRECTID: u16 = 97;
}

// ── special-case compile helpers ─────────────────────────────────────────────

/// paramValue (compiler.go:6560): read + delete key, compile, append.
/// In CNS an entirely-empty value is skipped (Mugen compat).
fn param_value(
    is: &mut IniSection,
    comp: &mut Compiler,
    pb: &mut ParamBuilder,
    spec: &PSpec,
    warn: &mut dyn FnMut(String),
) -> Result<bool, String> {
    let Some(data) = is.take(spec.name) else {
        return Ok(false);
    };
    if data.trim().is_empty() {
        return Ok(false); // Mugen ignores empty parameters
    }
    let mut id = spec.id;
    let mut data_owned;
    let mut data_ref: &str = data.as_str();
    if spec.prefixable {
        // paramPrefixValue + getDataPrefix (compiler.go:6071): strip a
        // leading F/S/M when the remainder starts with a non-letter token
        // or a known trigger name ("F5,2" → prefix f, args "5,2")
        let mut d = data.clone();
        let length = d.len();
        let prefix = get_data_prefix(&mut d, spec.ffx_default);
        if d.len() < length {
            if prefix == "f" {
                id |= PREFIX_BIT;
            } else if prefix == "s" {
                id |= SYS_PREFIX_BIT;
            }
            data_owned = d;
            data_ref = data_owned.as_str();
        } else if prefix == "f" {
            // no explicit prefix but ffxDefault → fightfx default
            id |= PREFIX_BIT;
        }
    }
    let bes = exprs(comp, data_ref, spec.vt, spec.nargs)?;
    for (n, be) in bes.into_iter().enumerate() {
        pb.push(id + n as u16, be);
    }
    Ok(true)
}

pub const SYS_PREFIX_BIT: u16 = 0x4000;

/// getDataPrefix (compiler.go:6071) — strip leading f/s/m when the next
/// "token" (leading [a-z] run of the remainder) is empty or a trigger
/// name. Returns the prefix ("" if none; "f" when ffx_default applies).
fn get_data_prefix(data: &mut String, ffx_default: bool) -> String {
    let mut prefix = String::new();
    if data.len() > 1 {
        let lower = data.to_lowercase();
        let longest = if lower.starts_with('f') {
            "f"
        } else if lower.starts_with('s') {
            "s"
        } else if lower.starts_with('m') {
            "m"
        } else {
            ""
        };
        if !longest.is_empty() {
            let rest = &lower[longest.len()..];
            let first: String = rest
                .chars()
                .take_while(|c| c.is_ascii_lowercase())
                .collect();
            if first.is_empty() || crate::exp::is_trigger_name(&first) {
                prefix = longest.to_string();
                *data = data[longest.len()..].to_string();
            }
        }
    }
    if ffx_default && prefix.is_empty() {
        prefix = "f".into();
    }
    prefix
}

/// enum-string param: parse to a constant int (attrs/flags/types).
fn enum_param(
    is: &mut IniSection,
    pb: &mut ParamBuilder,
    name: &str,
    pid: u16,
    f: impl Fn(&str) -> i32,
) -> Result<bool, String> {
    let Some(data) = is.take(name) else {
        return Ok(false);
    };
    if data.trim().is_empty() {
        return Ok(false);
    }
    pb.push_const_i(pid, f(&data));
    Ok(true)
}

/// `attr = S, NA` → two const params (state bits, kind bits)
fn attr_param(is: &mut IniSection, pb: &mut ParamBuilder) -> Result<bool, String> {
    let Some(data) = is.take("attr") else {
        return Ok(false);
    };
    let mut parts = data.splitn(2, ',');
    let st = parts.next().unwrap_or("");
    let kind = parts.next().unwrap_or("");
    pb.push_const_i(hd::ATTR_ST, attr_state_bits(st));
    pb.push_const_i(hd::ATTR_KIND, attr_kind_bits(kind));
    Ok(true)
}

/// Compile one state controller. `is` holds the (non-trigger) params;
/// consumed keys are removed; leftover keys are reported (warns in
/// lenient mode, matching stateSec).
pub fn compile_sctrl(
    kind: SctrlKind,
    is: &mut IniSection,
    comp: &mut Compiler,
    warn: &mut dyn FnMut(String),
) -> Result<Sctrl, String> {
    use SctrlKind::*;
    let mut pb = ParamBuilder::new();
    let mut ignorehitpause = false;

    // ignorehitpause param (CNS keyword, not stored in params)
    if let Some(v) = is.take("ignorehitpause") {
        ignorehitpause = crate::ini::atoi(&v) != 0;
    }

    match kind {
        Null => {
            // consume everything silently
            let keys: Vec<String> = is.iter().map(|(k, _)| k.clone()).collect();
            for k in keys {
                is.take(&k);
            }
            return Ok(pb.finish(Null, ignorehitpause));
        }
        AssertSpecial => {
            // redirectid / enabled / flag...
            param_value(is, comp, &mut pb, &ps("redirectid", id::REDIRECTID, ValueType::Int, 1), warn)?;
            param_value(is, comp, &mut pb, &ps("enabled", id::ENABLED, ValueType::Bool, 1), warn)?;
            // every other key is treated as a flag name (MUGEN style:
            // flag = intro / flag2 = … — and bare "flag = value" repeats)
            let keys: Vec<String> = is.iter().map(|(k, _)| k.clone()).collect();
            for k in keys {
                if let Some(data) = is.take(&k) {
                    if let Some((bit, global, is_noko)) = assert_special_flag(&data) {
                        if is_noko {
                            pb.push_const_i(id::NOKO, 1);
                        } else if global {
                            pb.push_const(id::FLAG_G, v_int(bit as i32));
                        } else {
                            pb.push_const_i(id::FLAG, bit as i32);
                        }
                    } else {
                        warn(format!("Unknown AssertSpecial flag: {data}"));
                    }
                }
            }
        }
        VarSet | VarAdd => {
            // old syntax: value + v/fv/sysv/sysfv
            // new syntax: var(n) = value (key form, CNS only)
            let mut did_new = false;
            let keys: Vec<String> = is.iter().map(|(k, _)| k.clone()).collect();
            for k in keys {
                let kl = k.to_lowercase();
                let vtype: Option<u8> = if kl.starts_with("var(") {
                    Some(0)
                } else if kl.starts_with("fvar(") {
                    Some(1)
                } else if kl.starts_with("sysvar(") {
                    Some(2)
                } else if kl.starts_with("sysfvar(") {
                    Some(3)
                } else if kl.starts_with("map(") {
                    Some(4)
                } else {
                    None
                };
                let Some(vt) = vtype else { continue };
                let Some(data) = is.take(&k) else { continue };
                did_new = true;
                if vt == 4 {
                    // map(name) = value — name goes to the string pool
                    let name = k.trim();
                    let name = &name[name.find('(').unwrap_or(0) + 1..];
                    let name = name.trim_end_matches(')').trim();
                    let nidx = comp.ctx.add_string(name);
                    pb.push_const_i(id::VARTYPE, 4);
                    pb.push_const_i(id::VARSLOT, nidx as i32);
                    let vt_exp = if kind == VarAdd { ValueType::Int } else { ValueType::Int };
                    let mut data_ref = data.as_str();
                    let be = comp.full_expression(&mut data_ref, vt_exp)?;
                    pb.push(id::VARVALUE, be);
                } else {
                    // compile the index expression inside the parens
                    let inner = k.trim();
                    let inner = &inner[inner.find('(').unwrap_or(0) + 1..];
                    let inner = inner.trim_end_matches(')').trim();
                    let mut idx_input = inner.to_string();
                    let mut idx_in = idx_input.as_str();
                    let idx_exp = comp.full_expression(&mut idx_in, ValueType::Int)?;
                    pb.push_const_i(id::VARTYPE, vt as i32);
                    pb.push(id::VARSLOT, idx_exp);
                    let vt_exp = match vt {
                        1 | 3 => ValueType::Float,
                        _ => ValueType::Int,
                    };
                    let mut data_ref = data.as_str();
                    let be = comp.full_expression(&mut data_ref, vt_exp)?;
                    pb.push(id::VARVALUE, be);
                }
            }
            if did_new {
                is.take("redirectid");
            } else {
                // old syntax
                let mut value_data = String::new();
                let mut has_value = false;
                if let Some(v) = is.take("value") {
                    has_value = true;
                    value_data = v;
                }
                let (var_type, idx) = if let Some(v) = is.take("v") {
                    (0i32, v)
                } else if let Some(v) = is.take("fv") {
                    (1, v)
                } else if let Some(v) = is.take("sysv") {
                    (2, v)
                } else if let Some(v) = is.take("sysfv") {
                    (3, v)
                } else {
                    if has_value {
                        warn("VarSet does not specify variable number".into());
                    }
                    (-1, String::new())
                };
                if var_type >= 0 && has_value {
                    pb.push_const_i(id::VARTYPE, var_type);
                    let mut idx_src = idx;
                    let mut idx_in = idx_src.as_str();
                    let idx_exp = comp.full_expression(&mut idx_in, ValueType::Int)?;
                    pb.push(id::VARSLOT, idx_exp);
                    let vt_exp = match var_type {
                        1 | 3 => ValueType::Float,
                        _ => ValueType::Int,
                    };
                    let mut val_src = value_data;
                    let mut val_in = val_src.as_str();
                    let val_exp = comp.full_expression(&mut val_in, vt_exp)?;
                    pb.push(id::VARVALUE, val_exp);
                }
                is.take("redirectid");
            }
        }
        HitDef => {
            // redirectid
            param_value(is, comp, &mut pb, &ps("redirectid", hd::REDIRECTID, ValueType::Int, 1), warn)?;
            // attr first (mandatory)
            attr_param(is, &mut pb)?;
            // guard/hit flags + enums
            enum_param(is, &mut pb, "guardflag", hd::GUARDFLAG, guardflag_bits)?;
            {
                if let Some(data) = is.take("hitflag") {
                    let (bits, minus, plus) = hitflag_bits(&data);
                    pb.push_const_i(hd::HITFLAG, bits);
                    if minus {
                        pb.push_const_i(hd::HITFLAG_MINUS, 1);
                    }
                    if plus {
                        pb.push_const_i(hd::HITFLAG_PLUS, 1);
                    }
                }
            }
            enum_param(is, &mut pb, "affectteam", hd::AFFECTTEAM, affectteam_val)?;
            let enum_specs: &[(&str, u16, fn(&str) -> i32)] = &[
                ("ground.type", hd::GROUND_TYPE, hittype_val),
                ("air.type", hd::AIR_TYPE, hittype_val),
                ("fall.type", hd::FALL_TYPE, hittype_val),
                ("animtype", hd::ANIMTYPE, animtype_val),
                ("air.animtype", hd::AIR_ANIMTYPE, animtype_val),
                ("fall.animtype", hd::FALL_ANIMTYPE, animtype_val),
            ];
            for (n, pid, f) in enum_specs {
                enum_param(is, &mut pb, n, *pid, *f)?;
            }
            // priority: "3, Hit" — int part + type suffix
            if let Some(pdata) = is.take("priority") {
                let (num_part, type_part) = match pdata.find(',') {
                    Some(i) => (&pdata[..i], &pdata[i+1..]),
                    None => (pdata.as_str(), ""),
                };
                let mut num_in = num_part.trim().to_string();
                let mut num_ref = num_in.as_str();
                let num_exp = comp.full_expression(&mut num_ref, ValueType::Int)?;
                pb.push(hd::PRIORITY, num_exp);
                if !type_part.trim().is_empty() {
                    pb.push_const_i(hd::PRIORITY_TYPE, priority_type_val(type_part));
                }
            }
            // numeric/expression params
            let specs = vec![
                ps("id", hd::ID, ValueType::Int, 1),
                ps("chainid", hd::CHAINID, ValueType::Int, 1),
                ps("kill", hd::KILL, ValueType::Bool, 1),
                ps("guard.kill", hd::GUARD_KILL, ValueType::Bool, 1),
                ps("fall.kill", hd::FALL_KILL, ValueType::Bool, 1),
                ps("hitonce", hd::HITONCE, ValueType::Bool, 1),
                ps("air.juggle", hd::AIR_JUGGLE, ValueType::Int, 1),
                ps("getpower", hd::GETPOWER, ValueType::Int, 2),
                ps("givepower", hd::GIVEPOWER, ValueType::Int, 2),
                ps("damage", hd::DAMAGE, ValueType::Int, 2),
                ps("numhits", hd::NUMHITS, ValueType::Int, 1),
                pspf("hitsound", hd::HITSOUND, ValueType::Int, 2),
                pspf("guardsound", hd::GUARDSOUND, ValueType::Int, 2),
                ps("p1stateno", hd::P1STATENO, ValueType::Int, 1),
                ps("p2stateno", hd::P2STATENO, ValueType::Int, 1),
                ps("p2getp1state", hd::P2GETP1STATE, ValueType::Bool, 1),
                ps("p1sprpriority", hd::P1SPRPRIORITY, ValueType::Int, 1),
                ps("sprpriority", hd::P1SPRPRIORITY, ValueType::Int, 1),
                ps("p2sprpriority", hd::P2SPRPRIORITY, ValueType::Int, 1),
                ps("forcestand", hd::FORCESTAND, ValueType::Int, 1),
                ps("forcecrouch", hd::FORCECROUCH, ValueType::Int, 1),
                ps("fall.damage", hd::FALL_DAMAGE, ValueType::Int, 1),
                ps("fall.xvelocity", hd::FALL_XVELOCITY, ValueType::Float, 1),
                ps("fall.yvelocity", hd::FALL_YVELOCITY, ValueType::Float, 1),
                ps("fall.recover", hd::FALL_RECOVER, ValueType::Bool, 1),
                ps("fall.recovertime", hd::FALL_RECOVERTIME, ValueType::Int, 1),
                pspf("sparkno", hd::SPARKNO, ValueType::Int, 1),
                pspf("guard.sparkno", hd::GUARD_SPARKNO, ValueType::Int, 1),
                ps("sparkxy", hd::SPARKXY, ValueType::Float, 2),
                ps("down.hittime", hd::DOWN_HITTIME, ValueType::Int, 1),
                ps("p1facing", hd::P1FACING, ValueType::Int, 1),
                ps("p1getp2facing", hd::P1GETP2FACING, ValueType::Int, 1),
                ps("p2facing", hd::P2FACING, ValueType::Int, 1),
                ps("mindist", hd::MINDIST, ValueType::Float, 3),
                ps("maxdist", hd::MAXDIST, ValueType::Float, 3),
                ps("snap", hd::SNAP, ValueType::Float, 4),
                ps("air.hittime", hd::AIR_HITTIME, ValueType::Int, 1),
                ps("fall", hd::FALL, ValueType::Bool, 1),
                ps("air.fall", hd::AIR_FALL, ValueType::Bool, 1),
                ps("air.velocity", hd::AIR_VELOCITY, ValueType::Float, 3),
                ps("airguard.velocity", hd::AIRGUARD_VELOCITY, ValueType::Float, 3),
                ps("guard.velocity", hd::GUARD_VELOCITY, ValueType::Float, 3),
                ps("ground.velocity", hd::GROUND_VELOCITY, ValueType::Float, 2),
                ps("ground.hittime", hd::GROUND_HITTIME, ValueType::Int, 1),
                ps("guard.hittime", hd::GUARD_HITTIME, ValueType::Int, 1),
                ps("guard.ctrltime", hd::GUARD_CTRLTIME, ValueType::Int, 1),
                ps("airguard.ctrltime", hd::AIRGUARD_CTRLTIME, ValueType::Int, 1),
                ps("guard.pausetime", hd::GUARD_PAUSETIME, ValueType::Int, 2),
                ps("ground.slidetime", hd::GROUND_SLIDETIME, ValueType::Int, 1),
                ps("guard.slidetime", hd::GUARD_SLIDETIME, ValueType::Int, 1),
                ps("down.bounce", hd::DOWN_BOUNCE, ValueType::Bool, 1),
                ps("down.velocity", hd::DOWN_VELOCITY, ValueType::Float, 3),
                ps("down.recover", hd::DOWN_RECOVER, ValueType::Bool, 1),
                ps("down.recovertime", hd::DOWN_RECOVERTIME, ValueType::Int, 1),
                ps("xaccel", hd::XACCEL, ValueType::Float, 1),
                ps("yaccel", hd::YACCEL, ValueType::Float, 1),
                ps("envshake.time", hd::ENVSHAKE_TIME, ValueType::Int, 1),
                ps("envshake.freq", hd::ENVSHAKE_FREQ, ValueType::Float, 1),
                ps("envshake.ampl", hd::ENVSHAKE_AMPL, ValueType::Int, 1),
                ps("envshake.phase", hd::ENVSHAKE_PHASE, ValueType::Float, 1),
                ps("redlife", hd::REDLIFE, ValueType::Int, 2),
                ps("pausetime", hd::PAUSETIME, ValueType::Int, 2),
            ];
            for spec in &specs {
                param_value(is, comp, &mut pb, spec, warn)?;
            }
            // priority type suffix: "3, Hit"
            if let Some(pdata) = pb.get_pub(hd::PRIORITY).map(|e| e.clone()) {
                let _ = pdata;
            }
            if let Some(data) = is.take("priority") {
                // re-read raw: value may have ", Hit" suffix — exprs consumed
                // the int already; parse the type by splitting on ','
                let parts: Vec<&str> = data.split(',').collect();
                if parts.len() > 1 {
                    pb.push_const_i(hd::PRIORITY_TYPE, priority_type_val(parts[1]));
                }
            }
        }
        _ => {
            for spec in table(kind) {
                param_value(is, comp, &mut pb, &spec, warn)?;
            }
            // kind-specific extras
            match kind {
                StateTypeSet => {
                    // statetype/movetype/physics as enum strings
                    if let Some(v) = is.take("statetype") {
                        if let Some(t) = StateTypeT::from_str(&v) {
                            pb.push_const_i(id::STATETYPE, t as i32);
                        }
                    }
                    if let Some(v) = is.take("value") {
                        if let Some(t) = StateTypeT::from_str(&v) {
                            pb.push_const_i(id::STATETYPE, t as i32);
                        }
                    }
                    if let Some(v) = is.take("movetype") {
                        if let Some(t) = MoveTypeT::from_str(&v) {
                            pb.push_const_i(id::MOVETYPE, t as i32);
                        }
                    }
                    if let Some(v) = is.take("physics") {
                        if let Some(t) = StateTypeT::from_str(&v) {
                            pb.push_const_i(id::PHYSICS, t as i32);
                        }
                    }
                }
                NotHitBy | HitBy => {
                    // legacy value / value2 → slots 0/1; new: attr/slot/time
                    let not = kind == NotHitBy;
                    let _ = not;
                    // new syntax
                    if let Some(a) = is.take("attr") {
                        let mut parts = a.splitn(2, ',');
                        let st = parts.next().unwrap_or("");
                        let kd = parts.next().unwrap_or("");
                        // NotHitBy stores what it BLOCKS; semantics resolved at run
                        pb.push_const_i(id::VALUE, attr_state_bits(st));
                        pb.push_const_i(id::VALUE + 1, attr_kind_bits(kd));
                    }
                    if let Some(a) = is.take("value") {
                        let mut parts = a.splitn(2, ',');
                        let st = parts.next().unwrap_or("");
                        let kd = parts.next().unwrap_or("");
                        pb.push_const_i(id::VALUE, attr_state_bits(st));
                        pb.push_const_i(id::VALUE + 1, attr_kind_bits(kd));
                    }
                    if let Some(a) = is.take("value2") {
                        let mut parts = a.splitn(2, ',');
                        let st = parts.next().unwrap_or("");
                        let kd = parts.next().unwrap_or("");
                        pb.push_const_i(id::VALUE + 2, attr_state_bits(st));
                        pb.push_const_i(id::VALUE + 3, attr_kind_bits(kd));
                    }
                    if let Some(v) = is.take("slot") {
                        pb.push_const_i(id::INDEX, crate::ini::atoi(&v));
                    }
                    if let Some(v) = is.take("time") {
                        let mut tin = v;
                        let mut tin_ref = tin.as_str();
                        let be = comp.full_expression(&mut tin_ref, ValueType::Int)?;
                        pb.push(id::TIME, be);
                    }
                }
                HitOverride => {
                    if let Some(a) = is.take("attr") {
                        let mut parts = a.splitn(2, ',');
                        let st = parts.next().unwrap_or("");
                        let kd = parts.next().unwrap_or("");
                        pb.push_const_i(id::VALUE, attr_state_bits(st));
                        pb.push_const_i(id::VALUE + 1, attr_kind_bits(kd));
                    }
                    if let Some(v) = is.take("slot") {
                        pb.push_const_i(id::INDEX, crate::ini::atoi(&v));
                    }
                    for spec in [
                        ps("stateno", id::TIME + 1, ValueType::Int, 1),
                        ps("time", id::TIME, ValueType::Int, 1),
                        ps("forceair", id::FORCEAIR, ValueType::Bool, 1),
                    ] {
                        param_value(is, comp, &mut pb, &spec, warn)?;
                    }
                }
                Helper => {
                    compile_helper(is, comp, &mut pb, warn)?;
                }
                Explod | ModifyExplod | GameMakeAnim => {
                    compile_explod(is, comp, &mut pb, warn, kind == GameMakeAnim)?;
                }
                SuperPause => {
                    for spec in [
                        psm("time", id::TIME, ValueType::Int, 1),
                        ps("movetime", id::MOVETIME, ValueType::Int, 1),
                        ps("pausebg", id::PAUSEBG, ValueType::Bool, 1),
                        ps("endcmdbuftime", id::ENDCMDBUFTIME, ValueType::Int, 1),
                        ps("darken", id::DARKEN, ValueType::Bool, 1),
                        ps("brightness", id::BRIGHTNESS, ValueType::Int, 1),
                        psp("anim", id::ANIM, ValueType::Int, 1),
                        ps("pos", id::POS, ValueType::Float, 3),
                        ps("poweradd", id::VALUE + 1, ValueType::Int, 1),
                        ps("unhittable", id::UNHITTABLE, ValueType::Bool, 1),
                    ] {
                        param_value(is, comp, &mut pb, &spec, warn)?;
                    }
                }
                Projectile => {
                    compile_projectile(is, comp, &mut pb, warn)?;
                }
                RemapPal => {
                    for spec in [
                        ps("source", id::VALUE, ValueType::Int, 2),
                        ps("destination", id::DESTINATION, ValueType::Int, 2),
                    ] {
                        param_value(is, comp, &mut pb, &spec, warn)?;
                    }
                }
                PalFx | BgPalFx | AllPalFx => {
                    // consume palfx params silently for v1 (color/add/mul/time)
                    let keys: Vec<String> = is.iter().map(|(k, _)| k.clone()).collect();
                    for k in keys {
                        is.take(&k);
                    }
                }
                _ => {}
            }
        }
    }

    // leftover params → warn (stateSec "Unknown state controller parameter(s)")
    let leftovers: Vec<String> = is.iter().map(|(k, _)| k.clone()).collect();
    if !leftovers.is_empty() {
        warn(format!(
            "Unknown state controller parameter(s) for {}: {}",
            kind_name(kind),
            leftovers.join(", ")
        ));
    }

    Ok(pb.finish(kind, ignorehitpause))
}

fn compile_helper(
    is: &mut IniSection,
    comp: &mut Compiler,
    pb: &mut ParamBuilder,
    warn: &mut dyn FnMut(String),
) -> Result<(), String> {
    for spec in [
        ps("helpertype", id::HELPERTYPE, ValueType::Int, 1),
        ps("name", id::NAME, ValueType::String, 1),
        ps("stateno", id::VALUE, ValueType::Int, 1),
        ps("id", id::ID, ValueType::Int, 1),
        ps("pos", id::POS, ValueType::Float, 3),
        ps("postype", id::VARSLOT + 2, ValueType::Int, 1),
        ps("facing", id::FACING, ValueType::Int, 1),
        ps("size.xscale", id::SIZEALL, ValueType::Float, 1),
        ps("size.yscale", id::SIZEALL + 1, ValueType::Float, 1),
        ps("size.ground.back", id::SIZEALL + 2, ValueType::Int, 1),
        ps("size.ground.front", id::SIZEALL + 3, ValueType::Int, 1),
        ps("size.air.back", id::SIZEALL + 4, ValueType::Int, 1),
        ps("size.air.front", id::SIZEALL + 5, ValueType::Int, 1),
        ps("size.height", id::SIZEALL + 6, ValueType::Int, 1),
        ps("ownpal", id::OWNpal, ValueType::Bool, 1),
        ps("keyctrl", id::KEYCTRL, ValueType::Bool, 4),
        ps("supermovetime", id::SUPERMOVETIME, ValueType::Int, 1),
        ps("pausemovetime", id::PAUSEMOVETIME, ValueType::Int, 1),
        ps("removetime", id::REMOVETIME, ValueType::Int, 1),
    ] {
        param_value(is, comp, pb, &spec, warn)?;
    }
    Ok(())
}

fn compile_explod(
    is: &mut IniSection,
    comp: &mut Compiler,
    pb: &mut ParamBuilder,
    warn: &mut dyn FnMut(String),
    gamemakeanim: bool,
) -> Result<(), String> {
    // postype enum
    if let Some(v) = is.take("postype") {
        pb.push_const_i(id::VARSLOT + 2, postype_val(&v));
    }
    let anim_spec = if gamemakeanim {
        pspf("anim", id::ANIM, ValueType::Int, 1)
    } else {
        psp("anim", id::ANIM, ValueType::Int, 1)
    };
    for spec in [
        anim_spec,
        ps("id", id::ID, ValueType::Int, 1),
        ps("pos", id::POS, ValueType::Float, 3),
        ps("facing", id::FACING, ValueType::Int, 1),
        ps("vfacing", id::VFACING, ValueType::Int, 1),
        ps("vel", id::VEL, ValueType::Float, 3),
        ps("velocity", id::VEL, ValueType::Float, 3),
        ps("accel", id::ACCEL, ValueType::Float, 3),
        ps("scale", id::SCALE, ValueType::Float, 2),
        ps("random", id::RANDOM, ValueType::Float, 3),
        ps("removetime", id::REMOVETIME, ValueType::Int, 1),
        ps("supermovetime", id::SUPERMOVETIME, ValueType::Int, 1),
        ps("pausemovetime", id::PAUSEMOVETIME, ValueType::Int, 1),
        ps("sprpriority", id::SPRPRIORITY, ValueType::Int, 1),
        ps("ontop", id::ONTOP, ValueType::Bool, 1),
        ps("layerno", id::LAYERNO, ValueType::Int, 1),
        ps("shadow", id::SHADOW, ValueType::Int, 3),
        ps("ownpal", id::OWNpal, ValueType::Bool, 1),
        ps("bindtime", id::BINDTIME, ValueType::Int, 1),
        ps("removeongethit", id::STOPONGETHIT, ValueType::Bool, 1),
    ] {
        param_value(is, comp, pb, &spec, warn)?;
    }
    // trans + alpha
    if let Some(v) = is.take("trans") {
        pb.push_const_i(id::TRANS, trans_val(&v));
    }
    if let Some(v) = is.take("alpha") {
        let mut vin = v;
        let bes = exprs(comp, &mut vin, ValueType::Int, 2)?;
        for (n, be) in bes.into_iter().enumerate() {
            pb.push(id::ALPHA + n as u16, be);
        }
    }
    Ok(())
}

fn compile_projectile(
    is: &mut IniSection,
    comp: &mut Compiler,
    pb: &mut ParamBuilder,
    warn: &mut dyn FnMut(String),
) -> Result<(), String> {
    if let Some(v) = is.take("postype") {
        pb.push_const_i(id::VARSLOT + 2, postype_val(&v));
    }
    for spec in [
        psp("projid", id::ID, ValueType::Int, 1),
        ps("velocity", id::VEL, ValueType::Float, 3),
        ps("velmul", id::MUL, ValueType::Float, 3),
        ps("projremove", id::REMOVETIME, ValueType::Int, 1),
        ps("projremovetime", id::REMOVETIME + 1, ValueType::Int, 1),
        ps("removetime", id::REMOVETIME + 2, ValueType::Int, 1),
        ps("pos", id::POS, ValueType::Float, 3),
        ps("facing", id::FACING, ValueType::Int, 1),
        ps("shadow", id::SHADOW, ValueType::Int, 3),
        psp("aistream", id::PRIORITY, ValueType::Int, 1),
        ps("sprpriority", id::SPRPRIORITY, ValueType::Int, 1),
        ps("supermovetime", id::SUPERMOVETIME, ValueType::Int, 1),
        ps("pausemovetime", id::PAUSEMOVETIME, ValueType::Int, 1),
        ps("ownpal", id::OWNpal, ValueType::Bool, 1),
    ] {
        param_value(is, comp, pb, &spec, warn)?;
    }
    Ok(())
}

impl ParamBuilder {
    /// debug helper used transiently during development
    pub fn get_pub(&self, _pid: u16) -> Option<&BytecodeExp> {
        None
    }
}

// ── run side ─────────────────────────────────────────────────────────────────

use crate::eval::eval;

/// Runtime data for one hitdef activation (subset consumed by the fight
/// system; fields mirror Go's `HitDef` struct, char.go).
#[derive(Debug, Clone, Default)]
pub struct HitDefT {
    pub attr_st: i32,
    pub attr_kind: i32,
    pub guardflag: i32,
    pub hitflag: i32,
    pub hitflag_minus: bool,
    pub hitflag_plus: bool,
    pub affectteam: i32,
    pub id: i32,
    pub chainid: i32,
    pub kill: Option<bool>,
    pub guard_kill: Option<bool>,
    pub fall_kill: Option<bool>,
    pub hitonce: bool,
    pub air_juggle: i32,
    pub getpower: [i32; 2],
    pub givepower: [i32; 2],
    pub damage: [i32; 2],
    pub numhits: i32,
    pub hitsound: [i32; 2],
    pub hitsound_ffx: bool,
    pub guardsound: [i32; 2],
    pub guardsound_ffx: bool,
    pub priority: i32,
    pub priority_type: i32,
    pub ground_type: i32,
    pub air_type: i32,
    pub fall_type: i32,
    pub animtype: i32,
    pub air_animtype: i32,
    pub fall_animtype: i32,
    pub p1stateno: i32,
    pub p2stateno: i32,
    pub p2getp1state: bool,
    pub p1sprpriority: Option<i32>,
    pub p2sprpriority: Option<i32>,
    pub forcestand: bool,
    pub forcecrouch: bool,
    pub fall_damage: i32,
    pub fall_xvelocity: f32,
    pub fall_yvelocity: f32,
    pub fall_recover: i32, // -1 = default
    pub fall_recovertime: i32,
    pub sparkno: i32,
    pub sparkno_ffx: bool,
    pub guard_sparkno: i32,
    pub guard_sparkno_ffx: bool,
    pub sparkxy: [f32; 2],
    pub down_hittime: i32,
    pub p1facing: i32,
    pub p1getp2facing: i32,
    pub p2facing: i32,
    pub mindist: [f32; 3],
    pub maxdist: [f32; 3],
    pub snap: [f32; 4],
    pub air_hittime: i32,
    pub fall: bool,
    pub air_fall: bool,
    pub air_velocity: [f32; 3],
    pub airguard_velocity: [f32; 3],
    pub guard_velocity: [f32; 3],
    pub ground_velocity: [f32; 3],
    pub ground_hittime: i32,
    pub guard_hittime: i32,
    pub guard_ctrltime: i32,
    pub airguard_ctrltime: i32,
    /// guard.pausetime (char.go:931 falls back to pausetime)
    pub guard_pausetime: [i32; 2],
    pub ground_slidetime: i32,
    pub guard_slidetime: i32,
    pub down_bounce: bool,
    pub down_velocity: [f32; 3],
    pub down_recover: bool,
    pub down_recovertime: i32,
    pub xaccel: f32,
    pub yaccel: f32,
    pub envshake_time: i32,
    pub envshake_freq: f32,
    pub envshake_ampl: i32,
    pub envshake_phase: f32,
    pub redlife: [i32; 2],
    pub pausetime: [i32; 2],
}

/// Explod / GameMakeAnim activation data.
#[derive(Debug, Clone, Default)]
pub struct ExplodT {
    pub anim: i32,
    pub anim_ffx: bool,
    pub id: i32,
    pub postype: i32,
    pub pos: [f32; 3],
    pub facing: i32,
    pub vfacing: i32,
    pub vel: [f32; 3],
    pub accel: [f32; 3],
    pub scale: [f32; 2],
    pub random: [f32; 3],
    pub removetime: i32,
    pub supermovetime: i32,
    pub pausemovetime: i32,
    pub sprpriority: i32,
    pub ontop: bool,
    pub layerno: i32,
    pub shadow: [i32; 3],
    pub ownpal: bool,
    pub bindtime: i32,
    pub removeongethit: bool,
    pub trans: i32,
    pub alpha: [i32; 2],
}

#[derive(Debug, Clone, Default)]
pub struct PlaySndT {
    pub group: i32,
    pub number: i32,
    pub ffx: bool,
    pub channel: i32,
    pub lowpriority: bool,
    pub pan: f32,
    pub abspan: bool,
    pub volume: i32,
    pub volumescale: i32,
    pub freqmul: f32,
    pub loop_: bool,
    pub priority: i32,
}

#[derive(Debug, Clone, Default)]
pub struct HelperT {
    pub helpertype_player: bool,
    pub name_idx: usize,
    pub stateno: i32,
    pub id: i32,
    pub pos: [f32; 3],
    pub postype: i32,
    pub facing: i32,
    pub size_xscale: f32,
    pub size_yscale: f32,
    pub size_ground_back: i32,
    pub size_ground_front: i32,
    pub size_air_back: i32,
    pub size_air_front: i32,
    pub size_height: i32,
    pub ownpal: bool,
    pub keyctrl: [bool; 4],
}

/// The host-side runtime contract implemented by runtime::Char.
/// Defaults are no-ops so the runtime can grow controller-by-controller;
/// unimplemented ones hit `unimpl()` which logs once per kind.
pub trait CharRuntime: Ctx {
    // ── state ops ────────────────────────────────────────────────────────
    /// Returns true when the call came from the char itself (crun == c in Go)
    fn change_state(&mut self, no: i32, ctrl: i32, anim: i32, anim_ffx: bool, continue_: bool) -> bool;
    fn self_state(&mut self, no: i32, ctrl: i32, anim: i32, anim_ffx: bool);
    fn change_anim(&mut self, no: i32, elem: i32, elemtime: i32, ffx: bool);
    fn change_anim2(&mut self, no: i32, elem: i32, elemtime: i32);
    fn turn(&mut self);
    fn state_type_set(&mut self, st: Option<StateTypeT>, mt: Option<MoveTypeT>, physics: Option<StateTypeT>);
    fn spr_priority_set(&mut self, v: i32);

    // ── position/velocity ────────────────────────────────────────────────
    fn pos_set(&mut self, x: Option<f32>, y: Option<f32>, z: Option<f32>);
    fn pos_add(&mut self, x: Option<f32>, y: Option<f32>, z: Option<f32>);
    fn pos_freeze(&mut self, v: bool);
    fn vel_set(&mut self, x: Option<f32>, y: Option<f32>, z: Option<f32>);
    fn vel_add(&mut self, x: Option<f32>, y: Option<f32>, z: Option<f32>);
    fn vel_mul(&mut self, x: Option<f32>, y: Option<f32>, z: Option<f32>);
    fn set_ctrl(&mut self, v: bool);
    fn offset_set(&mut self, x: f32, y: f32);

    // ── meters ───────────────────────────────────────────────────────────
    fn life_add(&mut self, v: i32, absolute: bool, kill: bool);
    fn life_set(&mut self, v: i32);
    fn power_add(&mut self, v: i32);
    fn power_set(&mut self, v: i32);
    fn defence_mul_set(&mut self, v: f32);
    fn attack_mul_set(&mut self, v: f32);

    // ── combat ───────────────────────────────────────────────────────────
    fn hit_def(&mut self, hd: &HitDefT);
    fn reversal_def(&mut self, hd: &HitDefT);
    fn not_hit_by(&mut self, slot: i32, time: i32, st: i32, kind: i32, not: bool);
    fn hit_by(&mut self, slot: i32, time: i32, st: i32, kind: i32);
    fn hit_override(&mut self, st: i32, kind: i32, slot: i32, stateno: i32, time: i32, forceair: bool);
    fn hit_vel_set(&mut self, x: bool, y: bool, z: bool);
    fn hit_fall_set(&mut self, fall: Option<i32>, xvel: Option<f32>, yvel: Option<f32>);
    fn hit_fall_vel(&mut self);
    fn hit_fall_damage(&mut self);
    fn gravity(&mut self);

    // ── targets ──────────────────────────────────────────────────────────
    fn target_state(&mut self, id: Option<i32>, index: Option<i32>, value: i32);
    fn target_bind(&mut self, id: Option<i32>, index: Option<i32>, time: i32, pos: [f32; 3]);
    fn target_life_add(&mut self, id: Option<i32>, index: Option<i32>, absolute: bool, kill: bool, value: i32);

    // ── presentation ─────────────────────────────────────────────────────
    fn assert_special(&mut self, flag: i64, global: bool, enabled: bool);
    fn assert_special_noko(&mut self, enabled: bool);
    fn play_snd(&mut self, snd: &PlaySndT);
    fn stop_snd(&mut self, channel: i32);
    fn snd_pan(&mut self, channel: i32, pan: f32, abs: bool);
    fn explod(&mut self, ex: &ExplodT);
    fn modify_explod(&mut self, ex: &ExplodT, id: Option<i32>);
    fn remove_explod(&mut self, id: Option<i32>);
    fn make_dust(&mut self, pos: [Option<f32>; 2], spacing: i32);
    fn after_image(&mut self, time: i32);
    fn after_image_time(&mut self, time: i32);
    fn angle_draw(&mut self, v: Option<f32>, scale: [Option<f32>; 2]);
    fn angle_set(&mut self, v: f32);
    fn angle_add(&mut self, v: f32);
    fn angle_mul(&mut self, v: f32);
    fn trans(&mut self, mode: i32, alpha: [i32; 2]);
    fn env_shake(&mut self, time: i32, ampl: i32, freq: f32, phase: f32, mul: f32);
    fn screen_bound(&mut self, v: bool, movecam_x: bool, movecam_y: bool);
    fn player_push(&mut self, v: bool, priority: i32, affectteam: i32);
    fn width(&mut self, player: Option<[f32; 2]>, edge: Option<[f32; 2]>, height: Option<[f32; 2]>);
    fn pause(&mut self, time: i32, movetime: i32, pausebg: bool, endcmdbuftime: i32, superpause: bool, darken: bool);

    // ── spawn / helpers ──────────────────────────────────────────────────
    fn helper_spawn(&mut self, helper: &HelperT);
    fn projectile_spawn(&mut self, hd: &HitDefT, ex: &ExplodT, vel: [f32; 3], removetime: i32, projid: i32);
    fn destroy_self(&mut self, recursive: bool, remove_explods: bool) -> bool;

    // ── vars / maps ──────────────────────────────────────────────────────
    /// varType: 0=var 1=fvar 2=sysvar 3=sysfvar 4=map(name pool idx)
    fn var_write(&mut self, var_type: u8, slot: i32, v: Value, add: bool);
    fn var_random(&mut self, slot: i32, range: [i32; 2]);
    fn var_range_set(&mut self, first: i32, last: i32, value: Option<i32>, fvalue: Option<f32>);

    fn target_facing(&mut self, id: Option<i32>, index: Option<i32>, value: i32);
    fn target_vel(&mut self, id: Option<i32>, index: Option<i32>, x: Option<f32>, y: Option<f32>, z: Option<f32>, kind: SctrlKind);
    fn target_drop(&mut self, id: Option<i32>, exclude_id: Option<i32>, keep_one: bool);
    fn target_power_add(&mut self, id: Option<i32>, index: Option<i32>, value: i32);
    fn hit_add(&mut self, value: i32);
    fn pal_fx(&mut self, char_fx: bool, bg_fx: bool);
    fn remap_pal(&mut self, source: [i32; 2], destination: [i32; 2]);
    fn attack_dist(&mut self, x: f32, y: f32);
    fn zoom(&mut self, value: f32);
    fn height(&mut self, value: [f32; 2], bottom: [f32; 2]);

    fn unimpl(&mut self, what: &str);
}

/// Evaluate exp[0] as i32 (missing/empty → default)
fn ev_i(sc: &Sctrl, pid: u16, host: &mut dyn CharRuntime, def: i32) -> i32 {
    match sc.get(pid) {
        Some(e) if !e.is_empty() => eval(e, host).to_i(),
        _ => def,
    }
}
fn ev_f(sc: &Sctrl, pid: u16, host: &mut dyn CharRuntime, def: f32) -> f32 {
    match sc.get(pid) {
        Some(e) if !e.is_empty() => eval(e, host).to_f(),
        _ => def,
    }
}
fn ev_b(sc: &Sctrl, pid: u16, host: &mut dyn CharRuntime, def: bool) -> bool {
    match sc.get(pid) {
        Some(e) if !e.is_empty() => eval(e, host).to_b(),
        _ => def,
    }
}
fn ev_opt_i(sc: &Sctrl, pid: u16, host: &mut dyn CharRuntime) -> Option<i32> {
    match sc.get(pid) {
        Some(e) if !e.is_empty() => Some(eval(e, host).to_i()),
        _ => None,
    }
}
fn ev_opt_f(sc: &Sctrl, pid: u16, host: &mut dyn CharRuntime) -> Option<f32> {
    match sc.get(pid) {
        Some(e) if !e.is_empty() => Some(eval(e, host).to_f()),
        _ => None,
    }
}
fn ev_opt_b(sc: &Sctrl, pid: u16, host: &mut dyn CharRuntime) -> Option<bool> {
    match sc.get(pid) {
        Some(e) if !e.is_empty() => Some(eval(e, host).to_b()),
        _ => None,
    }
}

/// The run dispatcher (Go: each controller's Run method).
pub fn run_sctrl(sc: &Sctrl, host: &mut dyn CharRuntime) -> bool {
    use SctrlKind::*;
    match sc.kind {
        Null => false,
        ChangeState => {
            let v = ev_i(sc, id::VALUE, host, 0);
            let ctrl = ev_i(sc, id::CTRL, host, -1);
            let (anim, ffx) = (ev_i(sc, id::ANIM, host, -1), sc.has(id::ANIM | PREFIX_BIT));
            let cont = ev_b(sc, id::CONTINUE, host, false);
            host.change_state(v, ctrl, anim, ffx, cont)
        }
        SelfState => {
            let v = ev_i(sc, id::VALUE, host, 0);
            let ctrl = ev_i(sc, id::CTRL, host, -1);
            let (anim, ffx) = (ev_i(sc, id::ANIM, host, -1), sc.has(id::ANIM | PREFIX_BIT));
            host.self_state(v, ctrl, anim, ffx);
            false
        }
        PosSet => {
            let (x, y, z) = (
                ev_opt_f(sc, id::X, host),
                ev_opt_f(sc, id::Y, host),
                ev_opt_f(sc, id::Z, host),
            );
            host.pos_set(x, y, z);
            false
        }
        PosAdd => {
            let (x, y, z) = (
                ev_opt_f(sc, id::X, host),
                ev_opt_f(sc, id::Y, host),
                ev_opt_f(sc, id::Z, host),
            );
            host.pos_add(x, y, z);
            false
        }
        PosFreeze => {
            let v = ev_b(sc, id::VALUE, host, false);
            host.pos_freeze(v);
            false
        }
        VelSet => {
            let (x, y, z) = (
                ev_opt_f(sc, id::X, host),
                ev_opt_f(sc, id::Y, host),
                ev_opt_f(sc, id::Z, host),
            );
            host.vel_set(x, y, z);
            false
        }
        VelAdd => {
            let (x, y, z) = (
                ev_opt_f(sc, id::X, host),
                ev_opt_f(sc, id::Y, host),
                ev_opt_f(sc, id::Z, host),
            );
            host.vel_add(x, y, z);
            false
        }
        VelMul => {
            let (x, y, z) = (
                ev_opt_f(sc, id::X, host),
                ev_opt_f(sc, id::Y, host),
                ev_opt_f(sc, id::Z, host),
            );
            host.vel_mul(x, y, z);
            false
        }
        CtrlSet => {
            let v = ev_b(sc, id::VALUE, host, false);
            host.set_ctrl(v);
            false
        }
        ChangeAnim => {
            let v = ev_i(sc, id::VALUE, host, 0);
            let ffx = sc.has(id::VALUE | PREFIX_BIT);
            let elem = ev_i(sc, id::ELEM, host, -1);
            let elemtime = ev_i(sc, id::ELEMTIME, host, -1);
            host.change_anim(v, elem, elemtime, ffx);
            false
        }
        ChangeAnim2 => {
            let v = ev_i(sc, id::VALUE, host, 0);
            let elem = ev_i(sc, id::ELEM, host, -1);
            let elemtime = ev_i(sc, id::ELEMTIME, host, -1);
            host.change_anim2(v, elem, elemtime);
            false
        }
        Turn => {
            host.turn();
            false
        }
        HitDef | ReversalDef => {
            let hd = build_hitdef(sc, host);
            if sc.kind == HitDef {
                host.hit_def(&hd);
            } else {
                host.reversal_def(&hd);
            }
            false
        }
        AssertSpecial => {
            let enabled = ev_b(sc, id::ENABLED, host, true);
            let mut flags: Vec<(u16, i64)> = Vec::with_capacity(4);
            for (pid, flag) in &sc.params {
                match *pid {
                    id::FLAG | id::FLAG_G => flags.push((*pid, eval(flag, host).to_i() as i64)),
                    _ => {}
                }
            }
            for (pid, bit) in flags {
                match pid {
                    id::FLAG => host.assert_special(bit, false, enabled),
                    id::FLAG_G => host.assert_special(bit, true, enabled),
                    _ => {}
                }
            }
            if sc.has(id::NOKO) {
                host.assert_special_noko(enabled);
            }
            false
        }
        PlaySnd => {
            let mut snd = PlaySndT::default();
            snd.group = ev_i(sc, id::VALUE, host, -1);
            snd.number = ev_i(sc, id::VALUE + 1, host, 0);
            snd.ffx = sc.has(id::VALUE | PREFIX_BIT);
            snd.channel = ev_i(sc, id::CHANNEL, host, -1);
            snd.lowpriority = ev_b(sc, id::LOWPRIORITY, host, false);
            snd.pan = ev_f(sc, id::PAN, host, 0.0);
            snd.abspan = sc.has(id::ABSPAN);
            snd.volume = ev_i(sc, id::VOLUME, host, 0);
            snd.volumescale = ev_i(sc, id::VOLUMESCALE, host, 100);
            snd.freqmul = ev_f(sc, id::FREQMUL, host, 1.0).clamp(0.01, 5.0);
            snd.loop_ = ev_b(sc, id::LOOP, host, false);
            snd.priority = ev_i(sc, id::PRIORITY, host, 0);
            host.play_snd(&snd);
            false
        }
        StopSnd => {
            let ch = ev_i(sc, id::CHANNEL, host, 0);
            host.stop_snd(ch);
            false
        }
        SndPan => {
            let (ch, pan) = (ev_i(sc, id::CHANNEL, host, 0), ev_f(sc, id::PAN, host, 0.0));
            let abs = sc.has(id::ABSPAN);
            host.snd_pan(ch, pan, abs);
            false
        }
        Explod | GameMakeAnim => {
            let ex = build_explod(sc, host);
            host.explod(&ex);
            false
        }
        ModifyExplod => {
            let ex = build_explod(sc, host);
            let mid = ev_opt_i(sc, id::ID, host);
            host.modify_explod(&ex, mid);
            false
        }
        RemoveExplod => {
            let rid = ev_opt_i(sc, id::ID, host);
            host.remove_explod(rid);
            false
        }
        NotHitBy | HitBy => {
            let not = sc.kind == NotHitBy;
            let slot = ev_i(sc, id::INDEX, host, 0);
            let time = ev_i(sc, id::TIME, host, 1).max(1);
            let st = ev_i(sc, id::VALUE, host, 15);
            let kind = ev_i(sc, id::VALUE + 1, host, 63);
            let st2 = ev_i(sc, id::VALUE + 2, host, 15);
            let kind2 = ev_i(sc, id::VALUE + 3, host, 63);
            let has2 = sc.has(id::VALUE + 2);
            let slot = ev_i(sc, id::INDEX, host, 0);
            let time = ev_i(sc, id::TIME, host, 1).max(1);
            let st = ev_i(sc, id::VALUE, host, 15);
            let kind = ev_i(sc, id::VALUE + 1, host, 63);
            if not {
                host.not_hit_by(slot, time, st, kind, true);
            } else {
                host.hit_by(slot, time, st, kind);
            }
            // legacy value2 slot
            if has2 {
                if not {
                    host.not_hit_by(1, time, st2, kind2, true);
                } else {
                    host.hit_by(1, time, st2, kind2);
                }
            }
            false
        }
        HitOverride => {
            let st = ev_i(sc, id::VALUE, host, 0);
            let kind = ev_i(sc, id::VALUE + 1, host, 0);
            let slot = ev_i(sc, id::INDEX, host, 0);
            let stateno = ev_i(sc, id::TIME + 1, host, -1);
            let time = ev_i(sc, id::TIME, host, 1);
            let forceair = ev_b(sc, id::FORCEAIR, host, false);
            host.hit_override(st, kind, slot, stateno, time, forceair);
            false
        }
        TargetState => {
            let (tid, tix, v) = (
                ev_opt_i(sc, id::ID, host),
                ev_opt_i(sc, id::INDEX, host),
                ev_i(sc, id::VALUE, host, 0),
            );
            host.target_state(tid, tix, v);
            false
        }
        TargetBind => {
            let mut pos = [0.0f32; 3];
            for i in 0..3 {
                pos[i] = ev_f(sc, id::POS + i as u16, host, 0.0);
            }
            let (tid, tix, time) = (
                ev_opt_i(sc, id::ID, host),
                ev_opt_i(sc, id::INDEX, host),
                ev_i(sc, id::TIME, host, 1),
            );
            host.target_bind(tid, tix, time, pos);
            false
        }
        TargetLifeAdd => {
            let (tid, tix, abs, kill, v) = (
                ev_opt_i(sc, id::ID, host),
                ev_opt_i(sc, id::INDEX, host),
                ev_b(sc, id::ABSOLUTE, host, false),
                ev_b(sc, id::KILL, host, true),
                ev_i(sc, id::VALUE, host, 0),
            );
            host.target_life_add(tid, tix, abs, kill, v);
            false
        }
        LifeAdd => {
            let (v, abs, kill) = (
                ev_i(sc, id::VALUE, host, 0),
                ev_b(sc, id::ABSOLUTE, host, false),
                ev_b(sc, id::KILL, host, true),
            );
            host.life_add(v, abs, kill);
            false
        }
        LifeSet => {
            let v = ev_i(sc, id::VALUE, host, 0);
            host.life_set(v);
            false
        }
        PowerAdd => {
            let v = ev_i(sc, id::VALUE, host, 0);
            host.power_add(v);
            false
        }
        PowerSet => {
            let v = ev_i(sc, id::VALUE, host, 0);
            host.power_set(v);
            false
        }
        StateTypeSet => {
            let st = sc.get(id::STATETYPE).map(|_| ev_i(sc, id::STATETYPE, host, 0)).and_then(|v| {
                match v {
                    0 => Some(StateTypeT::S),
                    1 => Some(StateTypeT::C),
                    2 => Some(StateTypeT::A),
                    3 => Some(StateTypeT::L),
                    4 => Some(StateTypeT::U),
                    _ => None,
                }
            });
            let mt = sc.get(id::MOVETYPE).map(|_| ev_i(sc, id::MOVETYPE, host, 0)).and_then(|v| {
                match v {
                    0 => Some(MoveTypeT::I),
                    1 => Some(MoveTypeT::A),
                    2 => Some(MoveTypeT::H),
                    _ => Some(MoveTypeT::U),
                }
            });
            let phys = sc.get(id::PHYSICS).map(|_| ev_i(sc, id::PHYSICS, host, 0)).and_then(|v| {
                match v {
                    0 => Some(StateTypeT::S),
                    1 => Some(StateTypeT::C),
                    2 => Some(StateTypeT::A),
                    3 => Some(StateTypeT::L),
                    4 => Some(StateTypeT::U),
                    _ => None,
                }
            });
            host.state_type_set(st, mt, phys);
            false
        }
        SprPriority => {
            let v = ev_i(sc, id::VALUE, host, 0);
            host.spr_priority_set(v);
            false
        }
        VarSet | VarAdd => {
            let vt = ev_i(sc, id::VARTYPE, host, 0) as u8;
            let slot = ev_i(sc, id::VARSLOT, host, 0);
            let v = match sc.get(id::VARVALUE) {
                Some(e) if !e.is_empty() => eval(e, host),
                _ => Value { vtype: ValueType::None, value: 0.0 },
            };
            host.var_write(vt, slot, v, sc.kind == VarAdd);
            false
        }
        VarRandom => {
            let slot = ev_i(sc, id::VARSLOT, host, 0);
            let lo = ev_i(sc, id::RANGE, host, 0);
            let hi = ev_i(sc, id::RANGE + 1, host, if lo == 0 { 1000 } else { lo });
            host.var_random(slot, [lo, hi]);
            false
        }
        VarRangeSet => {
            let first = ev_i(sc, id::FIRST, host, 0);
            let last = ev_i(sc, id::LAST, host, 0);
            let (v, fv) = (
                ev_opt_i(sc, id::VALUE, host),
                ev_opt_f(sc, id::FVALUE, host),
            );
            host.var_range_set(first, last, v, fv);
            false
        }
        EnvShake => {
            let (t, ampl, freq, phase, mul) = (
                ev_i(sc, id::TIME, host, 10),
                ev_i(sc, id::AMPL, host, -4),
                ev_f(sc, id::FREQ, host, 60.0),
                ev_f(sc, id::PHASE, host, 90.0),
                ev_f(sc, id::MUL, host, 1.0),
            );
            host.env_shake(t, ampl, freq, phase, mul);
            false
        }
        ScreenBound => {
            let (v, mx, my) = (
                ev_b(sc, id::VALUE, host, true),
                ev_b(sc, id::MOVECAMERA, host, true),
                ev_b(sc, id::MOVECAMERA + 1, host, true),
            );
            host.screen_bound(v, mx, my);
            false
        }
        PlayerPush => {
            let (v, pri) = (ev_b(sc, id::VALUE, host, true), ev_i(sc, id::PRIORITY, host, 0));
            host.player_push(v, pri, 0);
            false
        }
        Width => {
            let player = if sc.has(id::VALUE) {
                Some([
                    ev_f(sc, id::VALUE, host, 0.0),
                    ev_f(sc, id::VALUE + 1, host, 0.0),
                ])
            } else {
                None
            };
            let edge = if sc.has(id::EDGE) {
                Some([
                    ev_f(sc, id::EDGE, host, 0.0),
                    ev_f(sc, id::EDGE + 1, host, 0.0),
                ])
            } else {
                None
            };
            let height = if sc.has(id::HEIGHT) {
                Some([
                    ev_f(sc, id::HEIGHT, host, 0.0),
                    ev_f(sc, id::HEIGHT + 1, host, 0.0),
                ])
            } else {
                None
            };
            host.width(player, edge, height);
            false
        }
        Pause | SuperPause => {
            let superpause = sc.kind == SuperPause;
            let (t, mt, pb, ecb, dark) = (
                ev_i(sc, id::TIME, host, 0),
                ev_i(sc, id::MOVETIME, host, 0),
                ev_b(sc, id::PAUSEBG, host, true),
                ev_i(sc, id::ENDCMDBUFTIME, host, 0),
                ev_b(sc, id::DARKEN, host, true),
            );
            host.pause(t, mt, pb, ecb, superpause, dark);
            false
        }
        AfterImage => {
            let v = ev_i(sc, id::TIME, host, 20);
            host.after_image(v);
            false
        }
        AfterImageTime => {
            let v = ev_i(sc, id::TIME, host, 0);
            host.after_image_time(v);
            false
        }
        AngleDraw => {
            let (v, sx, sy) = (
                ev_opt_f(sc, id::VALUE, host),
                ev_opt_f(sc, id::SCALE, host),
                ev_opt_f(sc, id::SCALE + 1, host),
            );
            host.angle_draw(v, [sx, sy]);
            false
        }
        AngleAdd => {
            let v = ev_f(sc, id::VALUE, host, 0.0);
            host.angle_add(v);
            false
        }
        AngleMul => {
            let v = ev_f(sc, id::VALUE, host, 1.0);
            host.angle_mul(v);
            false
        }
        AngleSet => {
            let v = ev_f(sc, id::VALUE, host, 0.0);
            host.angle_set(v);
            false
        }
        Trans => {
            let mode = ev_i(sc, id::TRANS, host, 0);
            let a1 = ev_i(sc, id::ALPHA, host, 255);
            let a2 = ev_i(sc, id::ALPHA + 1, host, if mode == 2 { 128 } else { 255 });
            host.trans(mode, [a1, a2]);
            false
        }
        HitVelSet => {
            let (x, y, z) = (
                ev_b(sc, id::X, host, false),
                ev_b(sc, id::Y, host, false),
                ev_b(sc, id::Z, host, false),
            );
            host.hit_vel_set(x, y, z);
            false
        }
        HitFallSet => {
            let (fall, xv, yv) = (
                ev_opt_i(sc, id::VALUE, host),
                ev_opt_f(sc, id::X, host),
                ev_opt_f(sc, id::Y, host),
            );
            host.hit_fall_set(fall, xv, yv);
            false
        }
        HitFallVel => {
            host.hit_fall_vel();
            false
        }
        HitFallDamage => {
            host.hit_fall_damage();
            false
        }
        DestroySelf => {
            let recursive = ev_b(sc, id::RECURSIVE, host, false);
            let remove_explods = ev_b(sc, id::REMOVEEXPLODS, host, false);
            host.destroy_self(recursive, remove_explods)
        }
        Helper => {
            let mut h = HelperT::default();
            h.stateno = ev_i(sc, id::VALUE, host, 0);
            h.id = ev_i(sc, id::ID, host, 0);
            for i in 0..3 {
                h.pos[i] = ev_f(sc, id::POS + i as u16, host, 0.0);
            }
            h.postype = ev_i(sc, id::VARSLOT + 2, host, 0);
            h.facing = ev_i(sc, id::FACING, host, 0);
            h.size_xscale = ev_f(sc, id::SIZEALL, host, 1.0);
            h.size_yscale = ev_f(sc, id::SIZEALL + 1, host, 1.0);
            h.size_ground_back = ev_i(sc, id::SIZEALL + 2, host, 0);
            h.size_ground_front = ev_i(sc, id::SIZEALL + 3, host, 0);
            h.size_air_back = ev_i(sc, id::SIZEALL + 4, host, 0);
            h.size_air_front = ev_i(sc, id::SIZEALL + 5, host, 0);
            h.size_height = ev_i(sc, id::SIZEALL + 6, host, 0);
            h.ownpal = ev_b(sc, id::OWNpal, host, false);
            for i in 0..4 {
                h.keyctrl[i] = ev_b(sc, id::KEYCTRL + i as u16, host, false);
            }
            host.helper_spawn(&h);
            false
        }
        Projectile => {
            let hd = build_hitdef(sc, host);
            let mut ex = build_explod(sc, host);
            let ffx_anim = ev_i(sc, id::ANIM | PREFIX_BIT, host, 0);
            ex.anim = if sc.has(id::ANIM | PREFIX_BIT) { ffx_anim } else { ex.anim };
            let mut vel = [0.0f32; 3];
            for i in 0..3 {
                vel[i] = ev_f(sc, id::VEL + i as u16, host, 0.0);
            }
            let removetime = ev_i(sc, id::REMOVETIME + 2, host, -2);
            let projid = ev_i(sc, id::ID, host, 0);
            host.projectile_spawn(&hd, &ex, vel, removetime, projid);
            false
        }
        DefenceMulSet => {
            let v = ev_f(sc, id::VALUE, host, 1.0);
            host.defence_mul_set(v);
            false
        }
        AttackMulSet => {
            let v = ev_f(sc, id::VALUE, host, 1.0);
            host.attack_mul_set(v);
            false
        }
        Gravity => {
            host.gravity();
            false
        }
        MakeDust => {
            let (px, py, spacing) = (
                ev_opt_f(sc, id::POS, host),
                ev_opt_f(sc, id::POS + 1, host),
                ev_i(sc, id::TIME, host, 4),
            );
            host.make_dust([px, py], spacing);
            false
        }
        Offset => {
            let (x, y) = (ev_f(sc, id::X, host, 0.0), ev_f(sc, id::Y, host, 0.0));
            host.offset_set(x, y);
            false
        }
        MapSet | MapAdd => {
            // VARSLOT holds a pool string idx (compiled from map: "name")
            let nidx = ev_i(sc, id::VARSLOT, host, -1);
            if nidx >= 0 {
                let v = ev_i(sc, id::VARVALUE, host, 0);
                host.var_write(4, nidx, v_int(v), sc.kind == MapAdd);
            }
            false
        }
        ModifyHitDef => {
            host.unimpl("ModifyHitDef");
            false
        }
        TargetFacing => {
            let (tid, tix, v) = (
                ev_opt_i(sc, id::ID, host),
                ev_opt_i(sc, id::INDEX, host),
                ev_i(sc, id::VALUE, host, 0),
            );
            host.target_facing(tid, tix, v);
            false
        }
        TargetVelSet | TargetVelAdd | TargetVelMul => {
            let (tid, tix) = (ev_opt_i(sc, id::ID, host), ev_opt_i(sc, id::INDEX, host));
            let (x, y, z) = (
                ev_opt_f(sc, id::X, host),
                ev_opt_f(sc, id::Y, host),
                ev_opt_f(sc, id::Z, host),
            );
            host.target_vel(tid, tix, x, y, z, sc.kind);
            false
        }
        TargetDrop => {
            let (tid, ex, keep) = (
                ev_opt_i(sc, id::ID, host),
                ev_opt_i(sc, id::VALUE, host),
                ev_b(sc, id::KEEPONE, host, true),
            );
            host.target_drop(tid, ex, keep);
            false
        }
        TargetPowerAdd => {
            let (tid, tix, v) = (
                ev_opt_i(sc, id::ID, host),
                ev_opt_i(sc, id::INDEX, host),
                ev_i(sc, id::VALUE, host, 0),
            );
            host.target_power_add(tid, tix, v);
            false
        }
        HitAdd => {
            let v = ev_i(sc, id::VALUE, host, 0);
            host.hit_add(v);
            false
        }
        PalFx | BgPalFx | AllPalFx => {
            host.pal_fx(sc.kind == PalFx, sc.kind == BgPalFx);
            false
        }
        RemapPal => {
            let (s1, s2) = (
                ev_i(sc, id::VALUE, host, -1),
                ev_i(sc, id::VALUE + 1, host, 0),
            );
            let (d1, d2) = (
                ev_i(sc, id::DESTINATION, host, -1),
                ev_i(sc, id::DESTINATION + 1, host, 0),
            );
            host.remap_pal([s1, s2], [d1, d2]);
            false
        }
        ModifyReversalDef => {
            host.unimpl("ModifyReversalDef");
            false
        }
        AttackDist => {
            let (x, y) = (ev_f(sc, id::X, host, 0.0), ev_f(sc, id::Y, host, 0.0));
            host.attack_dist(x, y);
            false
        }
        Zoom => {
            let v = ev_f(sc, id::VALUE, host, 1.0);
            host.zoom(v);
            false
        }
        Height => {
            let (t0, b0) = (ev_f(sc, id::VALUE, host, 0.0), ev_f(sc, id::VALUE + 1, host, 0.0));
            let (bt, bb) = (ev_f(sc, id::VALUE + 2, host, t0), ev_f(sc, id::VALUE + 3, host, b0));
            host.height([t0, b0], [bt, bb]);
            false
        }
        Depth => {
            host.unimpl("Depth");
            false
        }
        ForceFeedback => {
            // PC rumble — meaningless on Android touch; consume params
            false
        }
        FallEnvShake => {
            // envshake triggered from fall gethit vars
            let (t, ampl, freq, phase, mul) = (
                ev_i(sc, id::TIME, host, 0),
                ev_i(sc, id::AMPL, host, -4),
                ev_f(sc, id::FREQ, host, 60.0),
                ev_f(sc, id::PHASE, host, 90.0),
                ev_f(sc, id::MUL, host, 1.0),
            );
            host.env_shake(t, ampl, freq, phase, mul);
            false
        }
    }
}

fn build_hitdef(sc: &Sctrl, host: &mut dyn CharRuntime) -> HitDefT {
    use hd::*;
    let mut h = HitDefT::default();
    h.attr_st = ev_i(sc, ATTR_ST, host, 0);
    h.attr_kind = ev_i(sc, ATTR_KIND, host, 0);
    h.guardflag = ev_i(sc, GUARDFLAG, host, 0);
    // default hitflag "MAF" = H|L|A (char.go HitDef default)
    h.hitflag = ev_i(sc, HITFLAG, host, 7);
    h.hitflag_minus = ev_b(sc, HITFLAG_MINUS, host, false);
    h.hitflag_plus = ev_b(sc, HITFLAG_PLUS, host, false);
    h.affectteam = ev_i(sc, AFFECTTEAM, host, 0);
    h.id = ev_i(sc, ID, host, 0);
    h.chainid = ev_i(sc, CHAINID, host, -1);
    h.kill = ev_opt_b(sc, KILL, host);
    h.guard_kill = ev_opt_b(sc, GUARD_KILL, host);
    h.fall_kill = ev_opt_b(sc, FALL_KILL, host);
    h.hitonce = ev_b(sc, HITONCE, host, false);
    h.air_juggle = ev_i(sc, AIR_JUGGLE, host, 0);
    h.getpower = [ev_i(sc, GETPOWER, host, 0), ev_i(sc, GETPOWER + 1, host, 0)];
    h.givepower = [ev_i(sc, GIVEPOWER, host, 0), ev_i(sc, GIVEPOWER + 1, host, 0)];
    h.damage = [ev_i(sc, DAMAGE, host, 0), ev_i(sc, DAMAGE + 1, host, 0)];
    h.numhits = ev_i(sc, NUMHITS, host, 1);
    h.hitsound = [ev_i(sc, HITSOUND, host, -1), ev_i(sc, HITSOUND + 1, host, 0)];
    h.hitsound_ffx = sc.has(HITSOUND | PREFIX_BIT);
    h.guardsound = [ev_i(sc, GUARDSOUND, host, -1), ev_i(sc, GUARDSOUND + 1, host, 0)];
    h.guardsound_ffx = sc.has(GUARDSOUND | PREFIX_BIT);
    h.priority = ev_i(sc, PRIORITY, host, 4);
    h.priority_type = ev_i(sc, PRIORITY_TYPE, host, 0);
    h.ground_type = ev_i(sc, GROUND_TYPE, host, 4);
    h.air_type = ev_i(sc, AIR_TYPE, host, h.ground_type);
    h.fall_type = ev_i(sc, FALL_TYPE, host, 7);
    h.animtype = ev_i(sc, ANIMTYPE, host, 0);
    h.air_animtype = ev_i(sc, AIR_ANIMTYPE, host, 6);
    h.fall_animtype = ev_i(sc, FALL_ANIMTYPE, host, 3);
    h.p1stateno = ev_i(sc, P1STATENO, host, -1);
    h.p2stateno = ev_i(sc, P2STATENO, host, -1);
    h.p2getp1state = ev_b(sc, P2GETP1STATE, host, true);
    h.p1sprpriority = ev_opt_i(sc, P1SPRPRIORITY, host);
    h.p2sprpriority = ev_opt_i(sc, P2SPRPRIORITY, host);
    h.forcestand = ev_i(sc, FORCESTAND, host, 0) > 0;
    h.forcecrouch = ev_i(sc, FORCECROUCH, host, 0) > 0;
    h.fall_damage = ev_i(sc, FALL_DAMAGE, host, 0);
    h.fall_xvelocity = ev_f(sc, FALL_XVELOCITY, host, f32::NAN);
    h.fall_yvelocity = ev_f(sc, FALL_YVELOCITY, host, -4.5);
    h.fall_recover = ev_i(sc, FALL_RECOVER, host, -1);
    // HitDef.reset (char.go:757): fall.recovertime default 4 — NOT 220;
    // gethitvar-based tech recovery must unlock after 4 ticks in the fall.
    h.fall_recovertime = ev_i(sc, FALL_RECOVERTIME, host, 4);
    h.sparkno = ev_i(sc, SPARKNO, host, -1);
    h.sparkno_ffx = sc.has(SPARKNO | PREFIX_BIT);
    h.guard_sparkno = ev_i(sc, GUARD_SPARKNO, host, -1);
    h.guard_sparkno_ffx = sc.has(GUARD_SPARKNO | PREFIX_BIT);
    h.sparkxy = [ev_f(sc, SPARKXY, host, 0.0), ev_f(sc, SPARKXY + 1, host, 0.0)];
    h.down_hittime = ev_i(sc, DOWN_HITTIME, host, 20);
    h.p1facing = ev_i(sc, P1FACING, host, 0);
    h.p1getp2facing = ev_i(sc, P1GETP2FACING, host, 0);
    h.p2facing = ev_i(sc, P2FACING, host, 0);
    h.mindist = [ev_f(sc, MINDIST, host, 0.0), ev_f(sc, MINDIST + 1, host, 0.0), ev_f(sc, MINDIST + 2, host, 0.0)];
    h.maxdist = [ev_f(sc, MAXDIST, host, 0.0), ev_f(sc, MAXDIST + 1, host, 0.0), ev_f(sc, MAXDIST + 2, host, 0.0)];
    for i in 0..4 {
        h.snap[i] = ev_f(sc, SNAP + i as u16, host, 0.0);
    }
    h.air_hittime = ev_i(sc, AIR_HITTIME, host, 20);
    h.fall = ev_b(sc, FALL, host, false);
    // air.fall unset (IErr in Go, char.go:11739 `hd.air_fall != 0` → true):
    // air hits put the victim into the fall chain by default — WinMUGEN
    // behavior, and what makes air-hit victims actually come DOWN.
    h.air_fall = ev_b(sc, AIR_FALL, host, true);
    h.air_velocity = [ev_f(sc, AIR_VELOCITY, host, 0.0), ev_f(sc, AIR_VELOCITY + 1, host, 0.0), ev_f(sc, AIR_VELOCITY + 2, host, 0.0)];
    h.ground_velocity = [ev_f(sc, GROUND_VELOCITY, host, 0.0), ev_f(sc, GROUND_VELOCITY + 1, host, 0.0), 0.0];
    // NaN = unset → the Go ifnanset fallbacks (char.go:946-951)
    h.airguard_velocity = [ev_f(sc, AIRGUARD_VELOCITY, host, f32::NAN), ev_f(sc, AIRGUARD_VELOCITY + 1, host, f32::NAN), ev_f(sc, AIRGUARD_VELOCITY + 2, host, f32::NAN)];
    h.guard_velocity = [ev_f(sc, GUARD_VELOCITY, host, f32::NAN), ev_f(sc, GUARD_VELOCITY + 1, host, 0.0), ev_f(sc, GUARD_VELOCITY + 2, host, f32::NAN)];
    if h.guard_velocity[0].is_nan() {
        h.guard_velocity[0] = h.ground_velocity[0];
    }
    if h.guard_velocity[2].is_nan() {
        h.guard_velocity[2] = h.ground_velocity[2];
    }
    if h.airguard_velocity[0].is_nan() {
        h.airguard_velocity[0] = h.air_velocity[0] * 1.5;
    }
    if h.airguard_velocity[1].is_nan() {
        h.airguard_velocity[1] = h.air_velocity[1] * 0.5;
    }
    if h.airguard_velocity[2].is_nan() {
        h.airguard_velocity[2] = h.air_velocity[2] * 1.5;
    }
    h.ground_hittime = ev_i(sc, GROUND_HITTIME, host, 13);
    h.ground_slidetime = ev_i(sc, GROUND_SLIDETIME, host, 12);
    // guard.* params use the -1 IErr sentinel, then the Go fallback chain
    // (char.go:931-948): guard.hittime ← ground.slidetime ← ground.hittime;
    // guard.slidetime ← guard.hittime; guard.ctrltime ← guard.slidetime;
    // airguard.ctrltime ← guard.ctrltime; guard.velocity ← ground.velocity;
    // airguard.velocity ← air.velocity × (1.5, 0.5, 1.5).
    h.guard_hittime = ev_i(sc, GUARD_HITTIME, host, -1);
    h.guard_ctrltime = ev_i(sc, GUARD_CTRLTIME, host, -1);
    h.airguard_ctrltime = ev_i(sc, AIRGUARD_CTRLTIME, host, -1);
    h.guard_slidetime = ev_i(sc, GUARD_SLIDETIME, host, -1);
    h.guard_pausetime = [
        ev_i(sc, GUARD_PAUSETIME, host, -1),
        ev_i(sc, GUARD_PAUSETIME + 1, host, -1),
    ];
    if h.guard_pausetime[0] < 0 {
        h.guard_pausetime[0] = h.pausetime[0];
    }
    if h.guard_pausetime[1] < 0 {
        h.guard_pausetime[1] = h.pausetime[1];
    }
    if h.guard_hittime < 0 {
        h.guard_hittime = if h.ground_slidetime >= 0 { h.ground_slidetime } else { h.ground_hittime };
    }
    if h.guard_slidetime < 0 {
        h.guard_slidetime = h.guard_hittime;
    }
    if h.guard_ctrltime < 0 {
        h.guard_ctrltime = h.guard_slidetime;
    }
    if h.airguard_ctrltime < 0 {
        h.airguard_ctrltime = h.guard_ctrltime;
    }
    h.down_bounce = ev_b(sc, DOWN_BOUNCE, host, false);
    h.down_velocity = [ev_f(sc, DOWN_VELOCITY, host, 0.0), ev_f(sc, DOWN_VELOCITY + 1, host, 0.0), ev_f(sc, DOWN_VELOCITY + 2, host, 0.0)];
    h.down_recover = ev_b(sc, DOWN_RECOVER, host, true);
    // down.recovertime < 0 → resolve to the victim's [Data] liedown.time
    // at hit time (char.go:14291). -1 is the Go IErr sentinel.
    h.down_recovertime = ev_i(sc, DOWN_RECOVERTIME, host, -1);
    h.xaccel = ev_f(sc, XACCEL, host, 0.0);
    // HitDef.reset (char.go:75): yaccel default 0.35 — the gravity the
    // common gethit states (5030/5035/5040/5050) apply via
    // velAdd{y: getHitVar(yAccel)}. 0.0 made air-hit victims fly out the
    // ceiling with no gravity. Classic 320-localcoord chars: unscaled.
    h.yaccel = ev_f(sc, YACCEL, host, 0.35);
    h.envshake_time = ev_i(sc, ENVSHAKE_TIME, host, 0);
    h.envshake_freq = ev_f(sc, ENVSHAKE_FREQ, host, 60.0);
    h.envshake_ampl = ev_i(sc, ENVSHAKE_AMPL, host, -4);
    h.envshake_phase = ev_f(sc, ENVSHAKE_PHASE, host, 90.0);
    h.redlife = [ev_i(sc, REDLIFE, host, 0), ev_i(sc, REDLIFE + 1, host, 0)];
    h.pausetime = [ev_i(sc, PAUSETIME, host, 0), ev_i(sc, PAUSETIME + 1, host, 0)];
    h
}

fn build_explod(sc: &Sctrl, host: &mut dyn CharRuntime) -> ExplodT {
    let mut e = ExplodT::default();
    e.anim = ev_i(sc, id::ANIM, host, -1);
    e.anim_ffx = sc.has(id::ANIM | PREFIX_BIT);
    e.id = ev_i(sc, id::ID, host, 0);
    e.postype = ev_i(sc, id::VARSLOT + 2, host, 0);
    for i in 0..3 {
        e.pos[i] = ev_f(sc, id::POS + i as u16, host, 0.0);
        e.vel[i] = ev_f(sc, id::VEL + i as u16, host, 0.0);
        e.accel[i] = ev_f(sc, id::ACCEL + i as u16, host, 0.0);
        e.random[i] = ev_f(sc, id::RANDOM + i as u16, host, 0.0);
    }
    e.scale = [ev_f(sc, id::SCALE, host, 1.0), ev_f(sc, id::SCALE + 1, host, 1.0)];
    e.facing = ev_i(sc, id::FACING, host, 0);
    e.vfacing = ev_i(sc, id::VFACING, host, 0);
    e.removetime = ev_i(sc, id::REMOVETIME, host, -2);
    e.supermovetime = ev_i(sc, id::SUPERMOVETIME, host, 0);
    e.pausemovetime = ev_i(sc, id::PAUSEMOVETIME, host, 0);
    e.sprpriority = ev_i(sc, id::SPRPRIORITY, host, 0);
    e.ontop = ev_b(sc, id::ONTOP, host, false);
    e.layerno = ev_i(sc, id::LAYERNO, host, if e.ontop { 1 } else { 0 });
    for i in 0..3 {
        e.shadow[i] = ev_i(sc, id::SHADOW + i as u16, host, 0);
    }
    e.ownpal = ev_b(sc, id::OWNpal, host, false);
    e.bindtime = ev_i(sc, id::BINDTIME, host, 0);
    e.removeongethit = ev_b(sc, id::STOPONGETHIT, host, false);
    e.trans = ev_i(sc, id::TRANS, host, 0);
    e.alpha = [ev_i(sc, id::ALPHA, host, 255), ev_i(sc, id::ALPHA + 1, host, 0)];
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use SctrlKind::{AssertSpecial, ChangeState, HitDef};

    /// flag bits must match char.go iota order exactly
    #[test]
    fn asf_bits_exact() {
        assert_eq!(asf::INVISIBLE, 1 << 0);
        assert_eq!(asf::NOWALK, 1 << 8);
        assert_eq!(asf::AUTOGUARD, 1 << 12);
        assert_eq!(asf::NOSTAND, 1 << 48);
        assert_eq!(gsf::INTRO, 1 << 2);
        assert_eq!(gsf::TIMERFREEZE, 1 << 10);
    }

    #[test]
    fn attr_bits() {
        assert_eq!(attr_state_bits("S"), 1);
        assert_eq!(attr_state_bits("SCA"), 7);
        assert_eq!(attr_kind_bits("NA"), 1);
        assert_eq!(attr_kind_bits("SA"), 2);
        assert_eq!(attr_kind_bits("HA"), 4);
        assert_eq!(guardflag_bits("MA"), 7); // M = H|L (compiler.go:1446)
        assert_eq!(guardflag_bits("HLA"), 7);
    }

    #[test]
    fn kind_registry_names() {
        assert_eq!(kind_from_name("changestate"), Some(ChangeState));
        assert_eq!(kind_from_name("HitDef"), Some(HitDef));
        assert_eq!(kind_from_name("AssertSpecial"), Some(AssertSpecial));
        assert_eq!(kind_from_name("bogus"), None);
    }
}
