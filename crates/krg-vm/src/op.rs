//! Port origin: ikemen-src/src/bytecode.go — `OpCode` (bytecode.go:285-…) and
//! `BytecodeExp.run` (bytecode.go:2237), plus compiler.go's emission sites.
//!
//! Go stores bytecode as a flat `[]byte` with inline operands (jump offsets,
//! i32/f32 immediates, string pool indices, sub-program lengths). Rust keeps
//! the same postfix execution model (stack machine, LHS pushed before RHS,
//! operator pops v2 from the top — bytecode.go:2446) but hoists inline
//! operands into a typed enum so `eval` never does byte arithmetic.
//!
//! Sub-programs (redirects `p2(...)`, function calls, ZSS `:=` assignments)
//! become boxed slices — same order and semantics as Go's length-prefixed
//! inlining (compiler.go:1630 `out.appendI32Op(opc, len(be2))`).

use crate::value::Value;

/// Compiled postfix expression. Faithful to Go's `BytecodeExp`: an ordered
/// op stream evaluated over a value stack with short-circuit jumps.
pub type BytecodeExp = Vec<Op>;

/// LHS push-then-RHS then operator ordering (see module docs).
#[derive(Debug, Clone)]
pub enum Op {
    // ── push immediates (OC_int8/OC_int/OC_float/OC_string) ──────────────
    Push(Value),
    // ── stack control (OC_pop/OC_dup/OC_swap) ───────────────────────────
    Pop,
    // ── arithmetic / logic (bytecode.go:1747-1964) ──────────────────────
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,
    Pow,
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Bland,
    Blor,
    Blxor,
    Blnot,
    Bwand,
    Bwor,
    Bwxor,
    /// `OC_range_*` — inclusive/exclusive endpoints decided at compile time
    Range { incl_min: bool, incl_max: bool },
    // ── jumps (OC_jz8/OC_jnz8/OC_jmp8 + i32 forms) ─────────────────────
    /// absolute index; Go uses relative offsets, we store resolved targets
    Jmp(usize),
    Jz(usize),
    Jnz(usize),
    /// OC_jud8 — jump if Top is Undefined
    Jud(usize),

    // ── vars & locals ───────────────────────────────────────────────────
    /// OC_var — push int var(n); arg expr pushed below
    VarGet,
    SysvarGet,
    FvarGet,
    SysfvarGet,
    /// OC_assign_var — pop val, pop n, set var → push new value
    AssignVar,
    AssignFvar,
    AssignSysvar,
    AssignSysfvar,
    /// OC_localvar — push ZSS local ($x)
    LocalVar(u8),
    /// ZSS map read: `map(name)`
    MapGet,
    /// ZSS map write: `map(name) := value` (pop val, pop name)
    MapSet,

    // ── redirect / function machinery ───────────────────────────────────
    /// OC_p2 — evaluate sub-program against the p2 enemy
    P2(Box<BytecodeExp>),
    /// ZSS function call: args evaluated in caller context, body runs with
    /// its own local frame (Go OC_run + OC_localvar frame)
    Call { name_idx: usize, args: Vec<BytecodeExp> },
    /// OC_run_caller — sub-expression runs against the calling char
    RunCaller(Box<BytecodeExp>),
    /// OC_run_redir helper — evaluate sub-program against an id from stack
    RunRedirId(Box<BytecodeExp>),

    // ── trigger reads (the OC_* trigger opcodes we ship) ────────────────
    /// One-arg trigger read with known sub-kind
    Trig(Trig),
    /// Multi-arg / keyword triggers that read further stack args
    /// `animelem = n` family: [push n] then this → bool
    AnimElemTime,
    AnimElemNo,
    AnimExist,
    /// `command = "name"`: pop string idx
    Command,
    /// bare statetype/movetype push string ids? No — Go pushes dedicated
    /// compare ops. We compile `statetype = S` to Trig(StateType)+compare
    /// handled by the compiler emitting Eq with a string.
    /// gethitvar(key)
    GetHitVar(GetHitVarKind),
    /// const(...) named constants
    Const(ConstKind),
    /// const240p/480p/720p(x)
    Const240p,
    Const480p,
    Const720p,
    /// floor()/abs()/… one-arg math
    Floor,
    Abs,
    /// `ifelse(c, a, b)` / ZSS `cond(c, a, b)` — only the taken branch runs
    IfElse { c: Box<BytecodeExp>, a: Box<BytecodeExp>, b: Box<BytecodeExp> },
}

/// Trigger read opcodes (subset ported for v0.9.0 — grown per milestone;
/// every entry names its Go origin in the doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trig {
    // state (compiler.go expValue cases)
    StateNo,        // OC_stateno
    PrevStateNo,    // OC_prevstateno
    Time,           // OC_time
    Anim,           // OC_anim
    AnimTime,       // OC_animtime
    SelfAnimExist,  // OC_selfanimexist
    Ctrl,           // OC_ctrl
    CtrlTime,       // OC_ctrltime
    Facing,         // OC_facing
    // statetype/movetype — string-typed for = comparisons
    StateType,      // OC_statetype
    MoveType,       // OC_movetype
    Physics,        // OC_physics
    // position / velocity
    PosX,           // OC_pos_x
    PosY,           // OC_pos_y
    VelX,           // OC_vel_x
    VelY,           // OC_vel_y
    VelZ,           // OC_vel_z (Ikemen z-axis)
    PosZ,            // OC_pos_z (Ikemen z-axis)
    BackEdge,       // OC_backedge
    BackEdgeDist,   // OC_backedgedist
    BackEdgeBodyDist,
    FrontEdge,
    FrontEdgeDist,
    FrontEdgeBodyDist,
    ScreenPosX,     // OC_screenpos_x
    ScreenPosY,     // OC_screenpos_y
    CameraPosX,     // OC_camerapos_x (Ikemen)
    CameraPosY,
    // meters
    Life,           // OC_life
    LifeMax,        // OC_lifemax
    Power,          // OC_power
    PowerMax,       // OC_powermax
    Alive,          // OC_alive
    Id,             // OC_id
    IsHelper,       // one-arg variant reads stack
    NumEnemy,
    NumExplod,
    // hits
    HitCount,       // OC_hitcount
    UniqHitCount,
    HitPauseTime,   // OC_hitpausetime
    HitShakeOver,   // OC_hitshakeover
    HitOver,        // OC_hitover
    HitFall,        // OC_hitfall
    HitDefAttr,     // OC_hitdefattr bare form (attr as string)
    HitDefAttrCmp,  // pops kind,stat masks → bool (compile-time masks)
    HitVelX,        // OC_hitvel_x
    HitVelY,        // OC_hitvel_y
    MoveContact,    // OC_movecontact
    MoveHit,        // OC_movehit
    MoveGuarded,    // OC_moveguarded
    MoveReversed,
    // distances (own perspective)
    P2BodyDistX,    // OC_p2bodydist_x
    P2DistX,        // OC_p2dist_x
    P2DistY,        // OC_p2dist_y
    P2Life,
    P2StateNo,
    P2StateType,    // OC_p2statetype — string compare form
    P2MoveType,
    // input
    HoldBack,       // OC_holdback? — no: these are commands; kept for cmd bridge
    // world
    Random,         // OC_random (0..999)
    GameTime,       // OC_gametime
    RoundState,     // OC_roundstate
    RoundNo,        // OC_roundno
    TeamSide,       // OC_teamside
    PalNo,          // OC_palno
    // versions
    MugenVersion,   // major part (default)
    MugenVersionMinor,
    IkemenVersion,
    IkemenVersionMinor,
    InGuardDist,    // OC_inguarddist
    CanRecover,     // OC_canrecover
    // Ikemen system triggers
    RoundsExisted,
    ConsecutiveWins,
    MatchNo,
    IsHomeTeam,
    AiLevel,
    Win,
    WinClutch,
    WinHyper,
    WinPerfect,
    WinSpecial,
    AnimLoopCount,
}

/// `gethitvar(...)` fields (compiler.go case "gethitvar" → OC_ex_gethitvar_*)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetHitVarKind {
    Xvel,
    Yvel,
    Zvel,
    Type,
    AnimType,
    AirType,
    GroundType,
    Damage,
    HitCount,
    FallCount,
    HitShakeTime,
    HitTime,
    SlideTime,
    CtrlTime,
    ReelTime,
    DownRecoverTime,
    GuardCount,
    Id,
    Playerno,
    FallXvel,
    FallYvel,
    FallZvel,
    FallEnvShakeTime,
    FallEnvShakeFreq,
    FallEnvShakeAmpl,
    FallEnvShakePhase,
    Fall,
    FallTime,
    Guarded,
    IsBound,
    // extended fields (gethitvar: compiler.go current set)
    AnimType2,          // air.animtype alias
    XOff,
    YOff,
    ZOff,
    XAccel,
    YAccel,
    ZAccel,
    RecoverTime,        // down.recovertime alias / recovertime
    Power,
    HitPower,
    Score,
    Attr,
}

/// `const(...)` fields — only the ones used by KFM/common1; grows later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstKind {
    DataLife,
    DataPower,
    DataAttack,
    DataDefence,
    DataFallDefenceUp,
    DataLiedownTime,
    DataAirjuggle,
    DataSparkno,
    DataGuardSparkno,
    DataKOEcho,
    DataVolume,
    SizeXscale,
    SizeYscale,
    SizeGroundBack,
    SizeGroundFront,
    SizeAirBack,
    SizeAirFront,
    SizeHeight,
    SizeAttackDist,
    SizeProjAttackDist,
    SizeProjDoscale,
    SizeHeadPos,
    SizeMidPos,
    SizeShadowOffset,
    SizeDrawOffset,
    VelocityWalkFwd,
    VelocityWalkBack,
    VelocityRunFwdX,
    VelocityRunFwdY,
    VelocityRunBackX,
    VelocityRunBackY,
    VelocityJumpY,
    VelocityJumpBack,
    VelocityJumpFwd,
    VelocityRunJumpBackX,
    VelocityRunJumpBackY,
    VelocityRunJumpFwdX,
    VelocityRunJumpFwdY,
    VelocityAirJumpY,
    VelocityAirJumpBack,
    VelocityAirJumpFwd,
    VelocityAirGetHitGroundRecoverX,
    VelocityAirGetHitGroundRecoverY,
    VelocityAirGetHitAirRecoverMulX,
    VelocityAirGetHitAirRecoverMulY,
    VelocityAirGetHitAirRecoverAddX,
    VelocityAirGetHitAirRecoverAddY,
    VelocityAirGetHitAirRecoverBack,
    VelocityAirGetHitAirRecoverFwd,
    VelocityAirGetHitAirRecoverUp,
    VelocityAirGetHitAirRecoverDown,
    MovementAirJumpHeight,
    MovementAirJumpNum,
    MovementYAccel,
    MovementStandFriction,
    MovementCrouchFriction,
    MovementAirFriction,
    MovementGroundFriction,
    MovementGravity,
    MovementFallDefenceUp,
    MovementJumpHeldDownBonus,
    /// named state constants from common.const (e.g. StateDownedGetHit_gettingUp)
    Named(usize),
    /// `const(data.foo)` not yet ported — reads 0, logged at compile
    Unknown,
}

impl ConstKind {
    /// Parse `const(name)` / ZSS `const(name)` argument text.
    /// Returns Unknown for anything not yet ported (compile continues,
    /// evaluates to 0 — matches Ikemen leniency mode for unknown consts).
    pub fn from_name(name: &str) -> ConstKind {
        use ConstKind::*;
        let k = name.trim().to_lowercase();
        match k.as_str() {
            "data.life" => DataLife,
            "data.power" => DataPower,
            "data.attack" => DataAttack,
            "data.defence" => DataDefence,
            "data.fall.defence_up" => DataFallDefenceUp,
            "data.liedown.time" => DataLiedownTime,
            "data.airjuggle" => DataAirjuggle,
            "data.sparkno" => DataSparkno,
            "data.guard.sparkno" => DataGuardSparkno,
            "data.ko.echo" => DataKOEcho,
            "data.volume" => DataVolume,
            "size.xscale" => SizeXscale,
            "size.yscale" => SizeYscale,
            "size.ground.back" => SizeGroundBack,
            "size.ground.front" => SizeGroundFront,
            "size.air.back" => SizeAirBack,
            "size.air.front" => SizeAirFront,
            "size.height" => SizeHeight,
            "size.attack.dist" => SizeAttackDist,
            "size.proj.attack.dist" => SizeProjAttackDist,
            "size.proj.doscale" => SizeProjDoscale,
            "size.head.pos" => SizeHeadPos,
            "size.mid.pos" => SizeMidPos,
            "size.shadowoffset" => SizeShadowOffset,
            "size.draw.offset" => SizeDrawOffset,
            "velocity.walk.fwd" => VelocityWalkFwd,
            "velocity.walk.back" => VelocityWalkBack,
            "velocity.run.fwd" => VelocityRunFwdX, // x; .y handled by caller suffix split
            "velocity.run.back" => VelocityRunBackX,
            "velocity.jump.neu" => VelocityJumpY,
            "velocity.jump.back" => VelocityJumpBack,
            "velocity.jump.fwd" => VelocityJumpFwd,
            "velocity.runjump.back" => VelocityRunJumpBackX,
            "velocity.runjump.fwd" => VelocityRunJumpFwdX,
            "velocity.airjump.neu" => VelocityAirJumpY,
            "velocity.airjump.back" => VelocityAirJumpBack,
            "velocity.airjump.fwd" => VelocityAirJumpFwd,
            "velocity.air.gethit.groundrecover" => VelocityAirGetHitGroundRecoverX,
            "velocity.air.gethit.airrecover.mul" => VelocityAirGetHitAirRecoverMulX,
            "velocity.air.gethit.airrecover.add" => VelocityAirGetHitAirRecoverAddX,
            "velocity.air.gethit.airrecover.back" => VelocityAirGetHitAirRecoverBack,
            "velocity.air.gethit.airrecover.fwd" => VelocityAirGetHitAirRecoverFwd,
            "velocity.air.gethit.airrecover.up" => VelocityAirGetHitAirRecoverUp,
            "velocity.air.gethit.airrecover.down" => VelocityAirGetHitAirRecoverDown,
            "movement.airjump.height" => MovementAirJumpHeight,
            "movement.airjump.num" => MovementAirJumpNum,
            "movement.yaccel" => MovementYAccel,
            "movement.stand.friction" => MovementStandFriction,
            "movement.crouch.friction" => MovementCrouchFriction,
            "movement.air.friction" => MovementAirFriction,
            "movement.ground.friction" => MovementGroundFriction,
            "movement.gravity" => MovementGravity,
            "movement.fall.defence_up" => MovementFallDefenceUp,
            "movement.jump.helddownbonus" => MovementJumpHeldDownBonus,
            _ => Unknown,
        }
    }
}
