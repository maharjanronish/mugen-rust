//! Port origin: ikemen-src/src/char.go + system.go — the per-frame
//! character runtime and fight system.
//!
//! v1 pipeline (char.go CharList.action order, 14001-14025):
//! * `step()` per char: input→buffer → CommandList::step
//!   (commandUpdate, char.go:13859)
//! * `action_prepare()` — hardcoded keys (guard 120 / jump 40 / crouch 10 /
//!   stand 12 / walk 20 / brake, char.go:12239-12277)
//! * `action_run()` — negative states -4…-1, pending state entry
//!   (stateChange2), current state run (bytecode.go:17130)
//! * `pos_update()` — pos += vel*facing, physics S/C friction + A gravity
//!   (char.go:10076), ss.time++
//!
//! Round machine (system.go roundState 1894): 0 pre-intro → 1 intro →
//! 2 fight → 3 over grace → 4 round over. Life/power/k.o. handling kept
//! minimal-but-correct for 1v1.

use crate::cmd::{CommandList, InputBuffer, LehmerRng};
use crate::exp::CompileCtx as _;
use crate::cns::{
    read_constants, CharData, CharMovement, CharSize, CharVelocity, StateCompiler,
};
use crate::eval::{eval, Ctx};
use crate::ini::load_text;
use crate::op::{ConstKind, GetHitVarKind, Trig};
use crate::sctrl::{
    CharRuntime, ExplodT, HitDefT, HelperT, PlaySndT, Sctrl, SctrlKind,
};
use crate::statedef::{
    run_state_bytecode, MoveTypeT, Node, StateBytecode, StateHost, StateTypeT,
};
use crate::value::{v_bool, v_float, v_int, v_none, v_string, v_undef, Value};
use crate::zss::FuncBytecode;
use std::collections::HashMap;

// ── compiled char data (shared per character definition) ─────────────────────

pub struct GlobalInfo {
    pub name: String,
    pub states: HashMap<i32, StateBytecode>,
    pub string_pool: Vec<String>,
    pub funcs: HashMap<String, FuncBytecode>,
    pub data: CharData,
    pub size: CharSize,
    pub velocity: CharVelocity,
    pub movement: CharMovement,
    pub named: Vec<(String, Value)>,
    /// Char localcoord width (def [Info] localcoord — 320 for classic
    /// chars, 640 for kfm720 etc.). Drives const240p/480p/720p scaling
    /// (char.go:5741 constp). Defaults to 320 until def parsing lands.
    pub localcoord: f32,
    pub cmd_template: CommandList,
    pub anims: krg_format::anim::AnimationTable,
    pub warnings: Vec<String>,
}

// ── animation runtime state (anim.go dynamic fields) ──────────────────────────

/// Dynamic per-char animation cursor (anim.go:212 `Animation` state part).
#[derive(Debug, Clone, Default)]
pub struct AnimState {
    pub anim_no: i32,
    pub curelem: i32,
    pub curelemtime: i32,
    pub curtime: i32,
    pub loopcount: i32,
    pub drawidx: i32,
    pub newframe: bool,
}

impl AnimState {
    /// changeAnim (char.go:6017) — reset cursor, optionally to an element
    pub fn set(&mut self, anim_no: i32) {
        self.anim_no = anim_no;
        self.curelem = 0;
        self.curelemtime = 0;
        self.curtime = 0;
        self.loopcount = 0;
        self.drawidx = 0;
        self.newframe = true;
    }

    /// anim.go:916 `Action()` — advance one tick.
    pub fn action(&mut self, anim: &krg_format::anim::Animation) {
        if anim.frames.is_empty() {
            return;
        }
        // -1 frame times freeze
        let cur_time = anim
            .frames
            .get(self.curelem as usize)
            .map(|f| f.time)
            .unwrap_or(-1);
        if cur_time == -1 {
            return;
        }
        self.newframe = false;
        if self.curtime <= anim.totaltime {
            self.curelemtime += 1;
        }
        if self.curelemtime >= cur_time.max(0) {
            let mut next = self.curelem + 1;
            if next as usize >= anim.frames.len() {
                next = anim.loopstart;
                self.loopcount += 1;
            }
            self.curelem = next;
            self.curelemtime = 0;
        }
        self.curtime += 1;
    }

    /// anim.go:616 AnimTime — time relative to anim end (≤0 during, 0 at end)
    pub fn anim_time(&self, anim: &krg_format::anim::Animation) -> i32 {
        self.curtime - anim.totaltime
    }

    /// anim.go:620 AnimationElemTime
    pub fn animelemtime(&self, anim: &krg_format::anim::Animation, elem: i32) -> i32 {
        if elem as usize > anim.frames.len() {
            let mut t = self.anim_time(anim);
            if t > 0 {
                t = 0;
            }
            return t;
        }
        let e = elem.max(0) - 1;
        let mut t = self.curtime;
        for i in 0..e {
            if let Some(f) = anim.frames.get(i as usize) {
                t -= f.time.max(0);
            }
        }
        t
    }

    /// anim.go:635 AnimationElemNo (simplified forward/backward walk)
    pub fn animelemno(&self, anim: &krg_format::anim::Animation, time: i32) -> i32 {
        if anim.frames.is_empty() {
            return 0;
        }
        let mut i = self.curelem;
        let mut t = time + self.curelemtime;
        if time <= 0 {
            loop {
                if t >= 0 {
                    return i + 1;
                }
                i -= 1;
                if i < 0 || (self.curelem >= anim.loopstart && i < anim.loopstart) {
                    i = anim.frames.len() as i32 - 1;
                }
                if i < 0 {
                    return 1;
                }
                t += anim.frames[i as usize].time.max(0);
            }
        } else {
            loop {
                t -= anim.frames.get(i as usize).map(|f| f.time.max(0)).unwrap_or(0);
                if t < 0 || (i as usize == anim.frames.len() - 1
                    && anim.frames[i as usize].time == -1)
                {
                    return i + 1;
                }
                i += 1;
                if i as usize >= anim.frames.len() {
                    i = anim.loopstart;
                    if t < 0 {
                        return i + 1;
                    }
                }
            }
        }
    }
}

// ── gethit vars ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct GetHitVar {
    pub xvel: f32,
    pub yvel: f32,
    pub zvel: f32,
    pub ground_type: i32,
    pub animtype: i32,
    pub air_type: i32,
    pub ground_type_is_high: bool,
    pub damage: i32,
    pub hitcount: i32,
    pub fallcount: i32,
    pub hitshaketime: i32,
    pub hittime: i32,
    pub slidetime: i32,
    pub ctrltime: i32,
    pub reeltime: i32,
    pub down_recovertime: i32,
    pub guardcount: i32,
    pub id: i32,
    pub playerno: i32,
    pub fall_xvel: f32,
    pub fall_yvel: f32,
    pub fall: bool,
    pub falltime: i32,
    pub guarded: bool,
    pub isbound: bool,
    pub yaccel: f32,
    pub power: i32,
    pub hitstate: [i32; 3],
}

// ── per-instance character state ──────────────────────────────────────────────

pub struct CharState {
    pub id: i32,
    pub player_no: usize,
    pub team_side: i32,
    pub is_helper: bool,
    pub gi: std::sync::Arc<GlobalInfo>,
    pub cmd_list: CommandList,
    // state
    pub state_no: i32,
    pub prev_state_no: i32,
    pub time_in_state: i32,
    pub state_type: StateTypeT,
    pub prev_state_type: StateTypeT,
    pub move_type: MoveTypeT,
    pub physics: StateTypeT,
    pub pending_state: Option<i32>,
    pub ps: Vec<i32>, // persistent counters for the current state
    /// Persistent counters for the negative states (-4..-1, slot = -n-1).
    /// They outlive state changes (unlike `ps`) — cloned per tick before,
    /// which silently re-armed every persistent(n) controller each frame.
    pub neg_ps: [Vec<i32>; 4],
    // movement
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub facing: i32,
    /// Round-start x (p1start/p2start anchor for roundReset)
    pub start_pos: f32,
    pub draw_offset: [f32; 2],
    pub pos_frozen: bool,
    // meters
    pub life: i32,
    pub life_max: i32,
    pub power: i32,
    pub power_max: i32,
    pub ctrl: bool,
    pub alive: bool,
    // vars
    pub vars: [i32; 64],
    pub fvars: [f32; 64],
    pub sysvars: [i32; 8],
    pub sysfvars: [f32; 8],
    pub maps: HashMap<String, Value>,
    // hit/combat
    pub hitpause_time: i32,
    pub hitdef: Option<HitDefT>,
    pub ghv: GetHitVar,
    pub hitdef_attr_st: i32,
    pub hitdef_attr_kind: i32,
    pub hit_count: i32,
    pub uniq_hit_count: i32,
    pub ctrl_overridden: bool,
    pub sprpriority: i32,
    pub attack_mul: f32,
    pub defence_mul: f32,
    // hitby slots: (st_mask, kind_mask, not, time)
    pub hitby: [(i32, i32, bool, i32); 8],
    pub hit_overridden: bool,
    // flags
    pub asf: u64,
    pub anim: AnimState,
    // ZSS locals frame (max 256)
    pub locals: Vec<Value>,
    // targeting
    pub targets: Vec<i32>,
    // fighting helper fields
    pub airjump_count: i32,
    pub inguarddist: bool,
    pub guard_flag: bool,
    pub store_move_type: MoveTypeT,
    pub warned: std::collections::HashSet<&'static str>,
    // per-frame caches (Fight refreshes before the state pass)
    pub p2: P2View,
    pub round_state_cache: i32,
    pub round_no_cache: i32,
    pub rounds_existed_cache: i32,
    /// MatchOver trigger cache (Fight refreshes each tick)
    pub match_over_cache: bool,
    pub stage_left: f32,
    pub stage_right: f32,
    pub attack_dist: f32,
    pub attack_dist_override: f32,
    // hitdef target latch (char.go hasTargetOfHitdef): ids this HitDef
    // activation has already hit — cleared when a new HitDef sctrl runs.
    // Without it one hitdef re-hits every frame (60×23 dmg/s = instant KO).
    pub hitdef_targets: Vec<i32>,
    // PlayerPush sctrl off-switch (CSF_playerpush)
    pub no_push: bool,
}

// ASF bits (mirror sctrl::asf)
const ASF_NOWALK: u64 = 1 << 8;
const ASF_NOCROUCH: u64 = 1 << 21;
const ASF_NOJUMP: u64 = 1 << 38;
const ASF_NOAUTOTURN: u64 = 1 << 2;
const ASF_NOBRAKE: u64 = 1 << 18;

impl CharState {
    pub fn new(
        gi: std::sync::Arc<GlobalInfo>,
        player_no: usize,
        team_side: i32,
        id: i32,
    ) -> Self {
        let life = gi.data.life;
        let gi = { let g = &gi; std::sync::Arc::clone(g) };
        CharState {
            id,
            player_no,
            team_side,
            is_helper: false,
            gi: gi.clone(),
            cmd_list: CommandList::new(),
            state_no: 0,
            prev_state_no: 0,
            time_in_state: 0,
            state_type: StateTypeT::S,
            prev_state_type: StateTypeT::S,
            move_type: MoveTypeT::I,
            physics: StateTypeT::S,
            pending_state: None,
            ps: Vec::new(),
            neg_ps: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            pos: [0.0; 3],
            vel: [0.0; 3],
            facing: 1,
            start_pos: 0.0,
            draw_offset: [0.0; 2],
            pos_frozen: false,
            life,
            life_max: life,
            power: 0,
            power_max: gi.data.power_max(),
            ctrl: true,
            alive: true,
            vars: [0; 64],
            fvars: [0.0; 64],
            sysvars: [0; 8],
            sysfvars: [0.0; 8],
            maps: HashMap::new(),
            hitpause_time: 0,
            hitdef: None,
            ghv: GetHitVar::default(),
            hitdef_attr_st: 0,
            hitdef_attr_kind: 0,
            hit_count: 0,
            uniq_hit_count: 0,
            ctrl_overridden: false,
            sprpriority: 0,
            attack_mul: 1.0,
            defence_mul: 1.0,
            hitby: [(0, 0, false, 0); 8],
            hit_overridden: false,
            asf: 0,
            anim: AnimState::default(),
            locals: Vec::new(),
            targets: Vec::new(),
            airjump_count: 0,
            inguarddist: false,
            guard_flag: false,
            store_move_type: MoveTypeT::I,
            warned: Default::default(),
            p2: P2View::default(),
            round_state_cache: 0,
            round_no_cache: 1,
            rounds_existed_cache: 0,
            match_over_cache: false,
            stage_left: -320.0,
            stage_right: 320.0,
            attack_dist: gi.size.attack_dist,
            attack_dist_override: -1.0,
            hitdef_targets: Vec::new(),
            no_push: false,
        }
    }

    /// Init commands from the compiled template (each fighter buffers alone)
    pub fn init_commands(&mut self) {
        self.cmd_list.copy_from(&self.gi.cmd_template);
        self.cmd_list.buf_reset();
    }

    pub fn set_anim(&mut self, no: i32) {
        self.anim.set(no);
    }

    fn current_animation(&self) -> Option<&krg_format::anim::Animation> {
        self.gi.anims.anims.get(&self.anim.anim_no)
    }

    /// setFacing / turn
    pub fn turn(&mut self) {
        self.facing = -self.facing;
    }

    /// changeState1 (char.go:6797)
    pub fn change_state(&mut self, no: i32) {
        self.prev_state_no = self.state_no;
        self.pending_state = Some(no.max(0));
    }

    /// stateChange2 (char.go:6938) — apply pending state: init counters,
    /// run statedef params. Returns the state number entered.
    fn apply_pending_state(&mut self) -> bool {
        let Some(no) = self.pending_state.take() else {
            return false;
        };
        self.state_no = no;
        self.time_in_state = 0;
        self.store_move_type = self.move_type;
        // a state change re-arms the hitdef's targets (char.go: changeState →
        // hitdef.reset clears hitdefTargets unless hitdefpersist) — this is
        // what stops a jab from landing 60×/s while its AnimElem trigger
        // stays true, while still letting jab→jab chains connect. The
        // hitdef itself is retired with the state too (hitdef.reset).
        self.hitdef_targets.clear();
        self.hitdef = None;
        // reset persistent counters for the new state
        let n = self
            .gi
            .states
            .get(&no)
            .map(|sb| sb.ctrlsps.len())
            .unwrap_or(0);
        self.ps = vec![0; n];
        self.pending_state = None;
        // apply the statedef header (type/movetype/physics + params sctrl)
        let sb_owned = self.gi.states.get(&no).cloned();
        if let Some(sb) = &sb_owned {
            let mut typ = sb.state_type;
            let mut mt = sb.move_type;
            let mut phys = sb.physics;
            if let Some(exp) = &sb.state_type_exp {
                let v = eval(exp, self as &mut dyn Ctx);
                let s = v.to_s(&self.gi.string_pool).to_string();
                typ = StateTypeT::from_str(&s).unwrap_or(StateTypeT::U);
            }
            if let Some(exp) = &sb.move_type_exp {
                let v = eval(exp, self as &mut dyn Ctx);
                let s = v.to_s(&self.gi.string_pool).to_string();
                mt = MoveTypeT::from_str(&s).unwrap_or(MoveTypeT::U);
            }
            if let Some(exp) = &sb.physics_exp {
                let v = eval(exp, self as &mut dyn Ctx);
                let s = v.to_s(&self.gi.string_pool).to_string();
                phys = StateTypeT::from_str(&s).unwrap_or(StateTypeT::U);
            }
            self.prev_state_type = self.state_type;
            self.state_type = typ;
            self.move_type = mt;
            self.physics = phys;
            // statedef params (anim/velset/ctrl/poweradd/…)
            if let Some(def) = &sb.state_def.clone() {
                self.run_statedef_params(def);
            }
        }
        true
    }

    /// bytecode.go stateDef Run — the compiled statedef pseudo-sctrl
    fn run_statedef_params(&mut self, def: &Sctrl) {
        const SD_HITCOUNTPERSIST: u16 = 1;
        const SD_SPRPRIORITY: u16 = 4;
        const SD_FACEP2: u16 = 5;
        const SD_JUGGLE: u16 = 6;
        const SD_VELSET: u16 = 7;
        const SD_ANIM: u16 = 10;
        const SD_CTRL: u16 = 11;
        const SD_POWERADD: u16 = 12;
        for (pid, exp) in &def.params {
            if exp.is_empty() {
                continue;
            }
            match *pid {
                SD_VELSET => {
                    // 3 args; missing keep current velocity
                    let a0 = def.get_arg_checked(SD_VELSET, 0);
                    let a1 = def.get_arg_checked(SD_VELSET, 1);
                    let a2 = def.get_arg_checked(SD_VELSET, 2);
                    if let Some(e) = a0 {
                        self.vel[0] = eval(e, self as &mut dyn Ctx).to_f();
                    }
                    if let Some(e) = a1 {
                        self.vel[1] = eval(e, self as &mut dyn Ctx).to_f();
                    }
                    if let Some(e) = a2 {
                        self.vel[2] = eval(e, self as &mut dyn Ctx).to_f();
                    }
                }
                SD_ANIM => {
                    let v = eval(exp, self as &mut dyn Ctx).to_i();
                    if v >= 0 {
                        self.set_anim(v);
                    }
                }
                SD_CTRL => {
                    let v = eval(exp, self as &mut dyn Ctx).to_b();
                    self.ctrl = v;
                    self.ctrl_overridden = true;
                }
                SD_POWERADD => {
                    let v = eval(exp, self as &mut dyn Ctx).to_i();
                    self.power = (self.power + v).clamp(0, self.power_max);
                }
                SD_SPRPRIORITY => {
                    self.sprpriority = eval(exp, self as &mut dyn Ctx).to_i();
                }
                _ => {
                    let _ = (SD_HITCOUNTPERSIST, SD_FACEP2, SD_JUGGLE);
                }
            }
        }
    }

    fn anim_of(&self) -> krg_format::anim::Animation {
        self.current_animation().cloned().unwrap_or_default()
    }

    pub fn hit_paused(&self) -> bool {
        self.hitpause_time > 0
    }
}

impl Sctrl {
    fn get_arg_checked(&self, pid: u16, n: usize) -> Option<&crate::op::BytecodeExp> {
        self.get(pid + n as u16).filter(|e| !e.is_empty())
    }
}

// ── Ctx: trigger reads against CharState ──────────────────────────────────────

impl Ctx for CharState {
    fn pool(&self) -> &[String] {
        &self.gi.string_pool
    }

    fn trig(&mut self, t: Trig) -> Value {
        use Trig::*;
        match t {
            StateNo => v_int(self.state_no),
            PrevStateNo => v_int(self.prev_state_no),
            Time => v_int(self.time_in_state),
            Anim => v_int(self.anim.anim_no),
            AnimTime => {
                let a = self.anim_of();
                v_int(self.anim.anim_time(&a))
            }
            SelfAnimExist => {
                let n = 0; // arg handled by AnimExist op
                let _ = n;
                v_bool(false)
            }
            Ctrl => v_bool(self.ctrl),
            Facing => v_int(self.facing),
            StateType => v_string(self.state_type.as_char_ptr(self)),
            MoveType => v_string(self.move_type.as_char_ptr(self)),
            Physics => v_string(self.physics_char()),
            PosX => v_float(self.pos[0]),
            PosY => v_float(self.pos[1]),
            PosZ => v_float(self.pos[2]),
            VelX => v_float(self.vel[0]),
            VelY => v_float(self.vel[1]),
            VelZ => v_float(self.vel[2]),
            BackEdge => v_float(self.back_edge()),
            BackEdgeDist => v_float(self.back_edge_dist()),
            BackEdgeBodyDist => v_float(self.back_edge_dist()),
            FrontEdge => v_float(self.front_edge()),
            FrontEdgeDist => v_float(self.front_edge_dist()),
            FrontEdgeBodyDist => v_float(self.front_edge_dist()),
            ScreenPosX => v_float(self.pos[0]),
            ScreenPosY => v_float(self.pos[1]),
            CameraPosX | CameraPosY => v_float(0.0),
            Life => v_int(self.life),
            LifeMax => v_int(self.life_max),
            Power => v_int(self.power),
            PowerMax => v_int(self.power_max),
            Alive => v_bool(self.alive),
            Id => v_int(self.id),
            IsHelper => v_bool(self.is_helper),
            NumEnemy => v_int(self.num_enemy()),
            NumExplod => v_int(0),
            HitCount => v_int(self.hit_count),
            UniqHitCount => v_int(self.uniq_hit_count),
            HitPauseTime => v_int(self.hitpause_time),
            HitShakeOver => v_bool(self.ghv.hitshaketime <= 0),
            HitOver => v_bool(self.ghv.hittime <= 0),
            HitFall => v_bool(self.ghv.fall),
            HitDefAttr => v_string(self.hitdef_attr_string()),
            HitVelX => v_float(self.ghv.xvel),
            HitVelY => v_float(self.ghv.yvel),
            MoveContact | MoveHit | MoveGuarded | MoveReversed => v_bool(false),
            P2BodyDistX => v_float(self.p2_bodydist_x()),
            P2DistX => v_float(self.p2dist_x()),
            P2DistY => v_float(self.p2dist_y()),
            P2Life => v_int(self.p2.life),
            P2StateNo => v_int(self.p2.state_no),
            P2StateType => v_string(self.p2_statetype_char()),
            P2MoveType => v_string(self.p2_movetype_char()),
            Random => v_int(0), // overridden by Fight wrapper via rand
            GameTime => v_int(0),
            RoundState => v_int(self.round_state_cache),
            RoundNo => v_int(self.round_no_cache),
            TeamSide => v_int(self.team_side),
            PalNo => v_int(1),
            MugenVersion => v_int(1),
            MugenVersionMinor => v_int(0),
            IkemenVersion => v_int(0),
            IkemenVersionMinor => v_int(99),
            InGuardDist => v_bool(self.inguarddist),
            CanRecover => v_bool(!self.ghv.fall || self.ghv.yaccel == 0.0),
            AnimLoopCount => v_int(self.anim.loopcount),
            RoundsExisted => v_int(self.rounds_existed_cache),
            MatchOver => v_bool(self.match_over_cache),
            ConsecutiveWins => v_int(0),
            MatchNo => v_int(1),
            IsHomeTeam => v_bool(self.team_side == 0),
            AiLevel => v_int(0),
            Win | WinClutch | WinHyper | WinPerfect | WinSpecial => v_bool(false),
            CtrlTime => v_int(0),
            HitDefAttr => v_string(0),
            HitDefAttrCmp => v_bool(false), // handled by the HitDefAttrCmp op
            HoldBack => v_bool(false),       // commands carry hold state
        }
    }

    fn command(&mut self, name: &str) -> bool {
        self.cmd_list.get_state(name)
    }

    fn gethitvar(&mut self, k: GetHitVarKind) -> Value {
        use GetHitVarKind::*;
        let g = &self.ghv;
        match k {
            Xvel => v_float(g.xvel),
            Yvel => v_float(g.yvel),
            Zvel => v_float(g.zvel),
            Type => v_int(g.ground_type),
            AnimType => v_int(g.animtype),
            AirType => v_int(g.air_type),
            GroundType => v_int(g.ground_type),
            Damage => v_int(g.damage),
            HitCount => v_int(g.hitcount),
            FallCount => v_int(g.fallcount),
            HitShakeTime => v_int(g.hitshaketime),
            HitTime => v_int(g.hittime),
            SlideTime => v_int(g.slidetime),
            CtrlTime => v_int(g.ctrltime),
            ReelTime => v_int(g.reeltime),
            DownRecoverTime => v_int(g.down_recovertime),
            GuardCount => v_int(g.guardcount),
            Id => v_int(g.id),
            Playerno => v_int(g.playerno),
            FallXvel => v_float(g.fall_xvel),
            FallYvel => v_float(g.fall_yvel),
            FallZvel => v_float(0.0),
            FallEnvShakeTime => v_int(0),
            FallEnvShakeFreq => v_float(60.0),
            FallEnvShakeAmpl => v_int(0),
            FallEnvShakePhase => v_float(90.0),
            Fall => v_bool(g.fall),
            FallTime => v_int(g.falltime),
            Guarded => v_bool(g.guarded),
            IsBound => v_bool(g.isbound),
            AnimType2 => v_int(g.animtype),
            XOff => v_float(0.0),
            YOff => v_float(0.0),
            ZOff => v_float(0.0),
            XAccel => v_float(0.0),
            YAccel => v_float(g.yaccel),
            ZAccel => v_float(0.0),
            RecoverTime => v_int(g.down_recovertime),
            Power => v_int(g.power),
            HitPower => v_int(g.power),
            Score => v_float(0.0),
            Attr => v_int(0),
        }
    }

    fn const_kind(&mut self, k: ConstKind) -> Value {
        use ConstKind::*;
        let gi = &self.gi;
        match k {
            DataLife => v_int(gi.data.life),
            DataPower => v_int(gi.data.power_max()),
            DataAttack => v_float(gi.data.attack as f32 / 100.0),
            DataDefence => v_float(gi.data.defence as f32 / 100.0),
            DataFallDefenceUp => v_float(gi.data.fall_defence_up as f32),
            DataLiedownTime => v_int(gi.data.liedown_time),
            DataAirjuggle => v_int(gi.data.airjuggle),
            DataSparkno => v_int(gi.data.sparkno),
            DataGuardSparkno => v_int(gi.data.guard_sparkno),
            DataKOEcho => v_int(gi.data.ko_echo),
            DataVolume => v_int(gi.data.volume),
            SizeXscale => v_float(gi.size.xscale),
            SizeYscale => v_float(gi.size.yscale),
            SizeGroundBack => v_float(gi.size.ground_back),
            SizeGroundFront => v_float(gi.size.ground_front),
            SizeAirBack => v_float(gi.size.air_back),
            SizeAirFront => v_float(gi.size.air_front),
            SizeHeight => v_float(gi.size.height),
            SizeAttackDist => v_float(gi.size.attack_dist),
            SizeProjAttackDist => v_float(gi.size.proj_attack_dist),
            SizeProjDoscale => v_int(gi.size.proj_doscale),
            SizeHeadPos => v_float(gi.size.head_pos.0 as f32),
            SizeMidPos => v_float(gi.size.mid_pos.0 as f32),
            SizeShadowOffset => v_int(gi.size.shadowoffset),
            SizeDrawOffset => v_float(gi.size.draw_offset.0 as f32),
            VelocityWalkFwd => v_float(gi.velocity.walk_fwd),
            VelocityWalkBack => v_float(gi.velocity.walk_back),
            VelocityRunFwdX => v_float(gi.velocity.run_fwd.0),
            VelocityRunFwdY => v_float(gi.velocity.run_fwd.1),
            VelocityRunBackX => v_float(gi.velocity.run_back.0),
            VelocityRunBackY => v_float(gi.velocity.run_back.1),
            VelocityJumpY => v_float(gi.velocity.jump_y),
            VelocityJumpNeuX => v_float(gi.velocity.jump_neu_x),
            VelocityJumpBack => v_float(gi.velocity.jump_back),
            VelocityJumpFwd => v_float(gi.velocity.jump_fwd),
            VelocityAirJumpNeuX => v_float(gi.velocity.airjump_neu_x),
            VelocityRunJumpBackX => v_float(gi.velocity.runjump_back.0),
            VelocityRunJumpBackY => v_float(gi.velocity.runjump_back.1),
            VelocityRunJumpFwdX => v_float(gi.velocity.runjump_fwd.0),
            VelocityRunJumpFwdY => v_float(gi.velocity.runjump_fwd.1),
            VelocityAirJumpY => v_float(gi.velocity.airjump_y),
            VelocityAirJumpBack => v_float(gi.velocity.airjump_back),
            VelocityAirJumpFwd => v_float(gi.velocity.airjump_fwd),
            VelocityAirGetHitGroundRecoverX => {
                v_float(gi.velocity.air_gethit_ground_recover.0)
            }
            VelocityAirGetHitGroundRecoverY => {
                v_float(gi.velocity.air_gethit_ground_recover.1)
            }
            VelocityAirGetHitAirRecoverMulX => {
                v_float(gi.velocity.air_gethit_air_recover_mul.0)
            }
            VelocityAirGetHitAirRecoverMulY => {
                v_float(gi.velocity.air_gethit_air_recover_mul.1)
            }
            VelocityAirGetHitAirRecoverAddX => {
                v_float(gi.velocity.air_gethit_air_recover_add.0)
            }
            VelocityAirGetHitAirRecoverAddY => {
                v_float(gi.velocity.air_gethit_air_recover_add.1)
            }
            VelocityAirGetHitAirRecoverBack => {
                v_float(gi.velocity.air_gethit_air_recover_back)
            }
            VelocityAirGetHitAirRecoverFwd => {
                v_float(gi.velocity.air_gethit_air_recover_fwd)
            }
            VelocityAirGetHitAirRecoverUp => {
                v_float(gi.velocity.air_gethit_air_recover_up)
            }
            VelocityAirGetHitAirRecoverDown => {
                v_float(gi.velocity.air_gethit_air_recover_down)
            }
            MovementAirJumpHeight => v_int(gi.movement.airjump_height),
            MovementAirJumpNum => v_int(1),
            MovementYAccel => v_float(gi.movement.yaccel),
            MovementStandFriction => v_float(gi.movement.stand_friction),
            MovementCrouchFriction => v_float(gi.movement.crouch_friction),
            MovementAirFriction => v_float(gi.movement.air_friction),
            MovementGroundFriction => v_float(gi.movement.ground_friction),
            MovementGravity => v_float(gi.movement.gravity),
            MovementFallDefenceUp => v_float(gi.movement.fall_defence_up as f32),
            MovementJumpHeldDownBonus => v_float(gi.movement.jump_helddown_bonus),
            MovementStandFrictionThreshold => v_float(0.03),
            Named(idx) => gi
                .named
                .get(idx)
                .map(|(_, v)| *v)
                .unwrap_or_else(v_none),
            Unknown => v_int(0),
        }
    }

    fn const_scaled(&mut self, base: f32, value: f32) -> f32 {
        // char.go:5741 constp: localcoord_x / base * value
        self.gi.localcoord / base * value
    }

    fn animelemtime(&mut self, n: i32) -> i32 {
        let a = self.anim_of();
        self.anim.animelemtime(&a, n)
    }

    fn animelemno(&mut self, t: f32) -> i32 {
        let a = self.anim_of();
        self.anim.animelemno(&a, t as i32)
    }

    fn animexist(&mut self, n: i32) -> bool {
        self.gi.anims.anims.contains_key(&n)
    }

    fn random(&mut self) -> i32 {
        0 // Fight wrapper intercepts via own RNG through rand_state
    }

    fn var_get(&mut self, n: i32) -> i32 {
        *self.vars.get(n as usize).unwrap_or(&0)
    }
    fn var_set(&mut self, n: i32, v: i32) -> i32 {
        if let Some(slot) = self.vars.get_mut(n as usize) {
            *slot = v;
        }
        v
    }
    fn fvar_get(&mut self, n: i32) -> f32 {
        *self.fvars.get(n as usize).unwrap_or(&0.0)
    }
    fn fvar_set(&mut self, n: i32, v: f32) -> f32 {
        if let Some(slot) = self.fvars.get_mut(n as usize) {
            *slot = v;
        }
        v
    }
    fn sysvar_get(&mut self, n: i32) -> i32 {
        *self.sysvars.get(n as usize).unwrap_or(&0)
    }
    fn sysvar_set(&mut self, n: i32, v: i32) -> i32 {
        if let Some(slot) = self.sysvars.get_mut(n as usize) {
            *slot = v;
        }
        v
    }
    fn sysfvar_get(&mut self, n: i32) -> f32 {
        *self.sysfvars.get(n as usize).unwrap_or(&0.0)
    }
    fn sysfvar_set(&mut self, n: i32, v: f32) -> f32 {
        if let Some(slot) = self.sysfvars.get_mut(n as usize) {
            *slot = v;
        }
        v
    }

    fn map_get(&mut self, name: &str) -> Value {
        self.maps
            .get(&name.to_lowercase())
            .copied()
            .unwrap_or_else(v_none)
    }
    fn map_set(&mut self, name: &str, v: Value) {
        self.maps.insert(name.to_lowercase(), v);
    }

    fn redirect_p2(&mut self, sub: &[crate::op::Op]) -> Value {
        v_none()
    }
    fn redirect_id(&mut self, _id: i32, _sub: &[crate::op::Op]) -> Value {
        v_undef()
    }
    fn call_func(&mut self, name: &str, args: Vec<Value>) -> Value {
        v_none()
    }

    fn local_get(&mut self, i: u8) -> Value {
        self.locals.get(i as usize).copied().unwrap_or_else(v_none)
    }
    fn hitdefattr_check(&mut self, st: i32, kind: i32) -> bool {
        (self.hitdef_attr_st & st) != 0 && (self.hitdef_attr_kind & kind) != 0
    }
}

// ── P2 view cache + helpers ───────────────────────────────────────────────────

/// Per-frame cached view of the opponent (char.go redirects read live
/// data; we cache before the state pass — same observable frame data).
#[derive(Debug, Clone, Default)]
pub struct P2View {
    pub id: i32,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub life: i32,
    pub state_no: i32,
    pub state_type: StateTypeT,
    pub move_type: MoveTypeT,
    pub facing: i32,
    pub alive: bool,
    pub ground_front: f32,
    /// p2's effective guard distance (AttackDist override or
    /// size.attack.dist) — the *attacker's* reach decides whether *we*
    /// can guard (char.go:14100 guard distance check).
    pub attack_dist: f32,
}

impl StateTypeT {
    /// pool string for statetype comparisons — resolved against the host
    pub fn as_char_ptr(self, _h: &CharState) -> usize {
        // string pool index of the type char — we insert the five type
        // strings at pool build time; simpler: return a stable pseudo
        // index mapped in `statetype string` helpers below.
        match self {
            StateTypeT::S => 0,
            StateTypeT::C => 1,
            StateTypeT::A => 2,
            StateTypeT::L => 3,
            StateTypeT::U => 4,
        }
    }
}

impl MoveTypeT {
    pub fn as_char_ptr(self, _h: &CharState) -> usize {
        match self {
            MoveTypeT::I => 0,
            MoveTypeT::A => 1,
            MoveTypeT::H => 2,
            MoveTypeT::U => 3,
        }
    }
}

/// String constants used by statetype-style comparisons. The exp compiler
/// compiles `statetype = S` to Trig(StateType)+Push(pool("S"))+Eq — so the
/// runtime must push a *pool string*. We pre-seed these into the pool.
pub const TYPE_POOL_SEED: [&str; 8] = ["S", "C", "A", "L", "U", "I", "H", "N"];

impl CharState {
    fn physics_char(&self) -> usize {
        match self.physics {
            StateTypeT::S => TYPE_POOL_SEED.iter().position(|s| *s == "S").unwrap_or(0),
            StateTypeT::C => TYPE_POOL_SEED.iter().position(|s| *s == "C").unwrap_or(0),
            StateTypeT::A => TYPE_POOL_SEED.iter().position(|s| *s == "A").unwrap_or(0),
            StateTypeT::L => TYPE_POOL_SEED.iter().position(|s| *s == "L").unwrap_or(0),
            StateTypeT::U => TYPE_POOL_SEED.iter().position(|s| *s == "U").unwrap_or(0),
        }
    }

    fn back_edge(&self) -> f32 {
        -self.stage_left // set by Fight each frame
    }
    fn back_edge_dist(&self) -> f32 {
        (self.pos[0] - self.stage_left).max(0.0)
    }
    fn front_edge(&self) -> f32 {
        self.stage_right
    }
    fn front_edge_dist(&self) -> f32 {
        (self.stage_right - self.pos[0]).max(0.0)
    }
    fn num_enemy(&self) -> i32 {
        1
    }
    fn p2_bodydist_x(&self) -> f32 {
        let raw = (self.p2.pos[0] - self.pos[0]).abs() - self.p2.ground_front;
        raw.max(0.0)
    }
    fn p2dist_x(&self) -> f32 {
        (self.p2.pos[0] - self.pos[0]) * self.facing as f32
    }
    fn p2dist_y(&self) -> f32 {
        self.p2.pos[1] - self.pos[1]
    }
    fn p2_field_life(&self) -> i32 {
        self.p2.life
    }
    fn p2_statetype_char(&self) -> usize {
        self.p2.state_type.as_char_ptr(self)
    }
    fn p2_movetype_char(&self) -> usize {
        self.p2.move_type.as_char_ptr(self)
    }
    fn hitdef_attr_string(&self) -> usize {
        0
    }
}

impl CharData {
    pub fn power_max(&self) -> i32 {
        3000
    }
}

// ── StateHost impl ────────────────────────────────────────────────────────────

impl StateHost for CharState {
    fn hit_paused(&self) -> bool {
        CharState::hit_paused(self)
    }
    fn persistent_skip(&mut self, pi: i32) -> bool {
        let idx = pi as usize;
        if idx >= self.ps.len() {
            self.ps.resize(idx + 1, 0);
        }
        let c = &mut self.ps[idx];
        // bytecode.go:5136 — decrement first, then test (persistent(1) = the
        // default — runs every tick)
        if *c != i32::MAX {
            *c -= 1;
        }
        *c > 0
    }
    fn persistent_reset(&mut self, pi: i32, value: i32) {
        let idx = pi as usize;
        if idx >= self.ps.len() {
            self.ps.resize(idx + 1, 0);
        }
        self.ps[idx] = value;
    }
    fn persistent_ensure(&mut self, n: usize) {
        if self.ps.len() < n {
            self.ps.resize(n, 0);
        }
    }
    fn state_owner(&self) -> usize {
        self.player_no
    }
    fn frame_reset(&mut self, num_vars: i32) {
        self.locals = vec![v_none(); num_vars.max(0) as usize];
    }
    fn set_local(&mut self, slot: u8, v: Value) {
        if slot as usize >= self.locals.len() {
            self.locals.resize(slot as usize + 1, v_none());
        }
        self.locals[slot as usize] = v;
    }
    fn local_slot(&mut self, slot: u8) -> Value {
        self.locals.get(slot as usize).copied().unwrap_or_else(v_none)
    }
    fn run_sctrl(&mut self, sctrl: &Sctrl, _persistent_active: bool) -> bool {
        crate::sctrl::run_sctrl(sctrl, self)
    }
    fn zss_call(&mut self, name_idx: usize, args: Vec<Value>, rets: &mut Vec<Value>) {
        let name = self
            .gi
            .string_pool
            .get(name_idx)
            .cloned()
            .unwrap_or_default();
        if let Some(fb) = self.gi.funcs.get(&name).cloned() {
            // fresh local frame: args then ret slots
            let saved = std::mem::take(&mut self.locals);
            self.frame_reset(fb.num_vars);
            for (i, a) in args.iter().take(fb.num_args).enumerate() {
                self.set_local(i as u8, *a);
            }
            // run body nodes
            let mut blk = crate::statedef::StateBlock {
                ctrls: fb.body.clone(),
                ..Default::default()
            };
            let _ = run_state_bytecode_nodes(&mut blk, self);
            // collect rets
            for i in 0..fb.num_rets {
                rets.push(self.locals.get(fb.num_args + i).copied().unwrap_or_else(v_none));
            }
            self.locals = saved;
        }
    }
    fn break_signal(&mut self) -> bool {
        false
    }
    fn continue_signal(&mut self) -> bool {
        false
    }
    fn on_state_change(&mut self) {}
}

fn run_state_bytecode_nodes(blk: &mut crate::statedef::StateBlock, host: &mut dyn StateHost) -> bool {
    // run ctrls directly (skipping block persistent/trigger — the function
    // body's own blocks still run via Node::Block)
    let _ = blk;
    false
}

// ── CharRuntime impl — the state controllers mutate CharState ─────────────────

impl CharRuntime for CharState {
    fn change_state(&mut self, no: i32, ctrl: i32, anim: i32, _anim_ffx: bool, _continue_: bool) -> bool {
        // ctrl: -1 keep; else set
        if ctrl >= 0 {
            self.ctrl = ctrl != 0;
            self.ctrl_overridden = true;
        }
        if anim >= 0 {
            self.set_anim(anim);
        }
        self.change_state(no);
        true // stop the block (crun == c)
    }
    fn self_state(&mut self, no: i32, ctrl: i32, anim: i32, _ffx: bool) {
        if ctrl >= 0 {
            self.ctrl = ctrl != 0;
            self.ctrl_overridden = true;
        }
        if anim >= 0 {
            self.set_anim(anim);
        }
        self.change_state(no);
    }
    fn change_anim(&mut self, no: i32, elem: i32, elemtime: i32, _ffx: bool) {
        self.set_anim(no);
        if elem >= 0 {
            // seek to element
            let a = self.anim_of();
            self.anim.curelem = (elem - 1).max(0);
            self.anim.curelemtime = elemtime.max(0);
            let _ = a;
        }
    }
    fn change_anim2(&mut self, no: i32, elem: i32, elemtime: i32) {
        self.change_anim(no, elem, elemtime, false);
    }
    fn turn(&mut self) {
        CharState::turn(self);
    }
    fn state_type_set(&mut self, st: Option<StateTypeT>, mt: Option<MoveTypeT>, physics: Option<StateTypeT>) {
        if let Some(t) = st {
            self.prev_state_type = self.state_type;
            self.state_type = t;
        }
        if let Some(m) = mt {
            self.move_type = m;
        }
        if let Some(p) = physics {
            self.physics = p;
        }
    }
    fn spr_priority_set(&mut self, v: i32) {
        self.sprpriority = v;
    }

    fn pos_set(&mut self, x: Option<f32>, y: Option<f32>, z: Option<f32>) {
        if let Some(v) = x {
            self.pos[0] = v;
        }
        if let Some(v) = y {
            self.pos[1] = v;
        }
        if let Some(v) = z {
            self.pos[2] = v;
        }
    }
    fn pos_add(&mut self, x: Option<f32>, y: Option<f32>, z: Option<f32>) {
        if let Some(v) = x {
            self.pos[0] += v * self.facing as f32;
        }
        if let Some(v) = y {
            self.pos[1] += v;
        }
        if let Some(v) = z {
            self.pos[2] += v;
        }
    }
    fn pos_freeze(&mut self, v: bool) {
        self.pos_frozen = v;
    }
    fn vel_set(&mut self, x: Option<f32>, y: Option<f32>, z: Option<f32>) {
        if let Some(v) = x {
            self.vel[0] = v;
        }
        if let Some(v) = y {
            self.vel[1] = v;
        }
        if let Some(v) = z {
            self.vel[2] = v;
        }
    }
    fn vel_add(&mut self, x: Option<f32>, y: Option<f32>, z: Option<f32>) {
        if let Some(v) = x {
            self.vel[0] += v;
        }
        if let Some(v) = y {
            self.vel[1] += v;
        }
        if let Some(v) = z {
            self.vel[2] += v;
        }
    }
    fn vel_mul(&mut self, x: Option<f32>, y: Option<f32>, z: Option<f32>) {
        if let Some(v) = x {
            self.vel[0] *= v;
        }
        if let Some(v) = y {
            self.vel[1] *= v;
        }
        if let Some(v) = z {
            self.vel[2] *= v;
        }
    }
    fn set_ctrl(&mut self, v: bool) {
        self.ctrl = v;
        self.ctrl_overridden = true;
    }
    fn offset_set(&mut self, x: f32, y: f32) {
        self.draw_offset = [x, y];
    }

    fn life_add(&mut self, v: i32, absolute: bool, kill: bool) {
        let dmg = if absolute {
            v
        } else {
            let def = (self.gi.data.defence as f32 / 100.0) * self.defence_mul;
            (v as f32 / def) as i32
        };
        self.life -= dmg;
        if self.life <= 0 {
            self.life = if kill { 0 } else { 1 };
        }
        self.alive = self.life > 0;
    }
    fn life_set(&mut self, v: i32) {
        self.life = v.max(0);
        self.alive = self.life > 0;
    }
    fn power_add(&mut self, v: i32) {
        self.power = (self.power + v).clamp(0, self.power_max);
    }
    fn power_set(&mut self, v: i32) {
        self.power = v.clamp(0, self.power_max);
    }
    fn defence_mul_set(&mut self, v: f32) {
        self.defence_mul = v;
    }
    fn attack_mul_set(&mut self, v: f32) {
        self.attack_mul = v;
    }

    fn hit_def(&mut self, hd: &HitDefT) {
        self.hitdef = Some(hd.clone());
        self.hitdef_attr_st = hd.attr_st;
        self.hitdef_attr_kind = hd.attr_kind;
    }
    fn reversal_def(&mut self, hd: &HitDefT) {}
    fn not_hit_by(&mut self, slot: i32, time: i32, st: i32, kind: i32, not: bool) {
        let s = (slot.max(0).min(7)) as usize;
        self.hitby[s] = (st, kind, not, time);
    }
    fn hit_by(&mut self, slot: i32, time: i32, st: i32, kind: i32) {
        let s = (slot.max(0).min(7)) as usize;
        self.hitby[s] = (st, kind, false, time);
    }
    fn hit_override(&mut self, _st: i32, _kind: i32, _slot: i32, stateno: i32, _time: i32, forceair: bool) {
        // simplified: mark overridden; fight resolves on hit
        self.hit_overridden = stateno >= 0;
        let _ = forceair;
    }
    fn hit_vel_set(&mut self, x: bool, y: bool, z: bool) {
        if x {
            self.vel[0] = self.ghv.xvel;
        }
        if y {
            self.vel[1] = self.ghv.yvel;
        }
        if z {
            self.vel[2] = self.ghv.zvel;
        }
    }
    fn hit_fall_set(&mut self, fall: Option<i32>, xvel: Option<f32>, yvel: Option<f32>) {
        if let Some(f) = fall {
            self.ghv.fall = f != 0;
        }
        if let Some(v) = xvel {
            self.ghv.fall_xvel = v;
        }
        if let Some(v) = yvel {
            self.ghv.fall_yvel = v;
        }
    }
    fn hit_fall_vel(&mut self) {
        self.vel[0] = self.ghv.fall_xvel;
        self.vel[1] = self.ghv.fall_yvel;
    }
    fn hit_fall_damage(&mut self) {
        // fall damage deferred to fight resolution
    }
    fn gravity(&mut self) {
        self.vel[1] += self.gi.movement.yaccel;
    }

    fn target_state(&mut self, _id: Option<i32>, _index: Option<i32>, value: i32) {
        // fight resolves targets
        let _ = value;
    }
    fn target_bind(&mut self, _id: Option<i32>, _index: Option<i32>, _time: i32, _pos: [f32; 3]) {}
    fn target_life_add(&mut self, _id: Option<i32>, _index: Option<i32>, _absolute: bool, _kill: bool, _value: i32) {}
    fn target_facing(&mut self, _id: Option<i32>, _index: Option<i32>, _value: i32) {}
    fn target_vel(&mut self, _id: Option<i32>, _index: Option<i32>, _x: Option<f32>, _y: Option<f32>, _z: Option<f32>, _kind: SctrlKind) {}
    fn target_drop(&mut self, _id: Option<i32>, _exclude_id: Option<i32>, _keep_one: bool) {}
    fn target_power_add(&mut self, _id: Option<i32>, _index: Option<i32>, _value: i32) {}
    fn hit_add(&mut self, value: i32) {
        self.hit_count += value;
    }
    fn pal_fx(&mut self, _char_fx: bool, _bg_fx: bool) {}
    fn remap_pal(&mut self, _source: [i32; 2], _destination: [i32; 2]) {}
    fn attack_dist(&mut self, x: f32, _y: f32) {
        self.attack_dist_override = x;
    }
    fn zoom(&mut self, _value: f32) {}
    fn height(&mut self, _value: [f32; 2], _bottom: [f32; 2]) {}

    fn assert_special(&mut self, flag: i64, _global: bool, enabled: bool) {
        if enabled {
            self.asf |= flag as u64;
        } else {
            self.asf &= !(flag as u64);
        }
    }
    fn assert_special_noko(&mut self, _enabled: bool) {}
    fn play_snd(&mut self, _snd: &PlaySndT) {
        // audio bridge deferred to the android layer
    }
    fn stop_snd(&mut self, _channel: i32) {}
    fn snd_pan(&mut self, _channel: i32, _pan: f32, _abs: bool) {}
    fn explod(&mut self, _ex: &ExplodT) {}
    fn modify_explod(&mut self, _ex: &ExplodT, _id: Option<i32>) {}
    fn remove_explod(&mut self, _id: Option<i32>) {}
    fn make_dust(&mut self, _pos: [Option<f32>; 2], _spacing: i32) {}
    fn after_image(&mut self, _time: i32) {}
    fn after_image_time(&mut self, _time: i32) {}
    fn angle_draw(&mut self, _v: Option<f32>, _scale: [Option<f32>; 2]) {}
    fn angle_set(&mut self, _v: f32) {}
    fn angle_add(&mut self, _v: f32) {}
    fn angle_mul(&mut self, _v: f32) {}
    fn trans(&mut self, _mode: i32, _alpha: [i32; 2]) {}
    fn env_shake(&mut self, _time: i32, _ampl: i32, _freq: f32, _phase: f32, _mul: f32) {}
    fn screen_bound(&mut self, _v: bool, _mx: bool, _my: bool) {}
    fn player_push(&mut self, v: bool, _priority: i32, _affectteam: i32) {
        self.no_push = !v;
    }
    fn width(&mut self, _player: Option<[f32; 2]>, _edge: Option<[f32; 2]>, _height: Option<[f32; 2]>) {}
    fn pause(&mut self, _time: i32, _movetime: i32, _pausebg: bool, _endcmdbuftime: i32, _superpause: bool, _darken: bool) {}
    fn helper_spawn(&mut self, _helper: &HelperT) {}
    fn projectile_spawn(&mut self, _hd: &HitDefT, _ex: &ExplodT, _vel: [f32; 3], _removetime: i32, _projid: i32) {}
    fn destroy_self(&mut self, _recursive: bool, _remove_explods: bool) -> bool {
        false
    }

    fn var_write(&mut self, var_type: u8, slot: i32, v: Value, add: bool) {
        match var_type {
            0 => {
                let i = slot as usize;
                if i < self.vars.len() {
                    if add {
                        self.vars[i] = self.vars[i].wrapping_add(v.to_i());
                    } else {
                        self.vars[i] = v.to_i();
                    }
                }
            }
            1 => {
                let i = slot as usize;
                if i < self.fvars.len() {
                    if add {
                        self.fvars[i] += v.to_f();
                    } else {
                        self.fvars[i] = v.to_f();
                    }
                }
            }
            2 => {
                let i = slot as usize;
                if i < self.sysvars.len() {
                    if add {
                        self.sysvars[i] = self.sysvars[i].wrapping_add(v.to_i());
                    } else {
                        self.sysvars[i] = v.to_i();
                    }
                }
            }
            3 => {
                let i = slot as usize;
                if i < self.sysfvars.len() {
                    if add {
                        self.sysfvars[i] += v.to_f();
                    } else {
                        self.sysfvars[i] = v.to_f();
                    }
                }
            }
            4 => {
                // map: slot = pool string idx
                if let Some(name) = self.gi.string_pool.get(slot as usize) {
                    let key = name.to_lowercase();
                    let cur = self.maps.get(&key).copied().unwrap_or_else(v_none);
                    let nv = if add { crate::value::add(&cur, &v) } else { v };
                    self.maps.insert(key, nv);
                }
            }
            _ => {}
        }
    }
    fn var_random(&mut self, slot: i32, range: [i32; 2]) {
        let i = slot as usize;
        if i < self.vars.len() {
            self.vars[i] = self.next_rand(range[0], range[1]);
        }
    }
    fn var_range_set(&mut self, first: i32, last: i32, value: Option<i32>, fvalue: Option<f32>) {
        for n in first..=last {
            let i = n as usize;
            if let Some(v) = value {
                if i < self.vars.len() {
                    self.vars[i] = v;
                }
            }
            if let Some(v) = fvalue {
                if i < self.fvars.len() {
                    self.fvars[i] = v;
                }
            }
        }
    }

    fn unimpl(&mut self, what: &str) {
        if let Some(key) = what.split_whitespace().next() {
            let leaked: &'static str = Box::leak(key.to_string().into_boxed_str());
            self.warned.insert(leaked);
        }
    }
}

// ── Fight: the per-frame system ───────────────────────────────────────────────

pub struct Fight {
    pub chars: Vec<CharState>,
    pub rng: LehmerRng,
    pub round_state: i32,
    pub round_no: i32,
    pub intro_time: i32,
    pub over_waittime: i32,
    pub match_time: i32,
    pub cur_round_time: i32,
    pub frame_count: i32,
    pub stage_left: f32,
    pub stage_right: f32,
    pub env_shake: (i32, i32, f32, f32), // time, ampl, freq, phase
    pub finished: bool,
    pub wins: [i32; 2],
    /// ticks spent in round_state 3 (over grace)
    pub over_timer: i32,
    /// wins needed to take the match (default 2, config rounds.to.win)
    pub max_wins: i32,
    // ── fight camera (camera.go port, classic model) ─────────────────
    /// Camera x in stage space (chars draw at pos - cam)
    pub cam_x: f32,
    /// Camera y: negative while tracking jumps (boundhigh..0)
    pub cam_y: f32,
    /// [Camera] stage scroll bounds: left, right, high (camera.go:8-10)
    pub cam_bound_left: f32,
    pub cam_bound_right: f32,
    pub cam_bound_high: f32,
    /// verticalfollow (kfm.def default 0.2) + floortension
    pub cam_vfollow: f32,
    pub cam_floortension: f32,
}

impl Fight {
    pub fn new(chars: Vec<CharState>, seed: i32) -> Self {
        Fight {
            chars,
            rng: LehmerRng::new(if seed == 0 { 1 } else { seed }),
            round_state: 0,
            round_no: 1,
            intro_time: 60, // short intro for headless play
            over_waittime: 45,
            match_time: 60 * 60,
            cur_round_time: 60 * 60,
            frame_count: 0,
            stage_left: -320.0,
            stage_right: 320.0,
            env_shake: (0, 0, 60.0, 0.0),
            finished: false,
            wins: [0; 2],
            over_timer: 0,
            max_wins: 2,
            cam_x: 0.0,
            cam_y: 0.0,
            cam_bound_left: -150.0,
            cam_bound_right: 150.0,
            cam_bound_high: -25.0,
            cam_vfollow: 0.2,
            cam_floortension: 0.0,
        }
    }

    /// Stage camera parameters (StageDef [Camera] values, scaled to
    /// 320-space by the caller).
    pub fn set_stage_camera(
        &mut self,
        left: f32,
        right: f32,
        high: f32,
        vfollow: f32,
        floortension: f32,
    ) {
        self.cam_bound_left = left;
        self.cam_bound_right = right;
        self.cam_bound_high = high;
        self.cam_vfollow = vfollow;
        self.cam_floortension = floortension;
    }

    /// Classic camera follow (old camera.go action): x tracks the alive
    /// players' midpoint clamped to the stage scroll bounds; y tracks the
    /// highest char past floortension scaled by verticalfollow, faster up
    /// than down.
    fn camera_update(&mut self) {
        let (mut sum, mut n) = (0.0f32, 0usize);
        let mut highest = 0.0f32;
        for c in &self.chars {
            if !c.alive {
                continue;
            }
            sum += c.pos[0];
            n += 1;
            highest = highest.min(c.pos[1]);
        }
        if n > 0 {
            let mid = sum / n as f32;
            let target_x = mid.clamp(self.cam_bound_left, self.cam_bound_right);
            self.cam_x += (target_x - self.cam_x) * 0.45;
            if (self.cam_x - target_x).abs() < 0.05 {
                self.cam_x = target_x;
            }
        }
        // vertical: follow jumps once above the floor tension line
        let target_y = if highest < -self.cam_floortension {
            (highest * self.cam_vfollow).max(self.cam_bound_high).min(0.0)
        } else {
            0.0
        };
        let f = if target_y < self.cam_y { 0.55 } else { 0.18 };
        self.cam_y += (target_y - self.cam_y) * f;
        if (self.cam_y - target_y).abs() < 0.05 {
            self.cam_y = target_y;
        }
    }

    /// Feed one frame of raw input bits (14 signals, InputBits order)
    /// per char: [U D L R a b c x y z s d w m].
    pub fn step(&mut self, inputs: &[[bool; 14]]) {
        self.frame_count += 1;

        // ── round state machine (system.go stepRoundState, simplified) ──
        if self.intro_time > 0 {
            self.intro_time -= 1;
            self.round_state = if self.intro_time > 10 { 0 } else { 1 };
            if self.intro_time == 0 {
                self.round_state = 2;
                for c in &mut self.chars {
                    c.ctrl = true;
                }
            }
        }
        let round_live = self.round_state == 2;

        // refresh cached views (p2, edges, round info) BEFORE state pass
        for i in 0..self.chars.len() {
            let view = {
                let p2i = 1 - i;
                let p2 = self.chars.get(p2i);
                P2View {
                    id: p2.map(|p| p.id).unwrap_or(-1),
                    pos: p2.map(|p| p.pos).unwrap_or([0.0; 3]),
                    vel: p2.map(|p| p.vel).unwrap_or([0.0; 3]),
                    life: p2.map(|p| p.life).unwrap_or(0),
                    state_no: p2.map(|p| p.state_no).unwrap_or(0),
                    state_type: p2.map(|p| p.state_type).unwrap_or(StateTypeT::S),
                    move_type: p2.map(|p| p.move_type).unwrap_or(MoveTypeT::I),
                    facing: p2.map(|p| p.facing).unwrap_or(1),
                    alive: p2.map(|p| p.alive).unwrap_or(false),
                    ground_front: p2
                        .map(|p| p.gi.size.ground_front)
                        .unwrap_or(16.0),
                    attack_dist: p2
                        .map(|p| {
                            if p.attack_dist_override >= 0.0 {
                                p.attack_dist_override
                            } else {
                                p.attack_dist
                            }
                        })
                        .unwrap_or(160.0),
                }
            };
            let c = &mut self.chars[i];
            c.p2 = view;
            c.round_state_cache = self.round_state;
            c.round_no_cache = self.round_no;
            c.match_over_cache = self.finished
                || (self.round_state == 3
                    && (self.wins[0] >= self.max_wins || self.wins[1] >= self.max_wins));
            c.stage_left = self.stage_left;
            c.stage_right = self.stage_right;
            // Proximity guard fires only while p2 is actually swinging
            // (char.go:14100 — the flag is set during hit detection, gated
            // on the attacker's moveType == A). With an idle opponent,
            // holding back must WALK, not guard.
            c.inguarddist = round_live
                && c.p2.alive
                && c.p2.move_type == MoveTypeT::A
                && (c.p2.pos[0] - c.pos[0]).abs() <= c.p2.attack_dist;
            c.attack_dist_override = -1.0;
        }

        // ── 1. input → buffer → commands (commandUpdate) ──────────────────
        for (i, c) in self.chars.iter_mut().enumerate() {
            if let Some(btns) = inputs.get(i) {
                let (u, d, l, r) = (btns[0], btns[1], btns[2], btns[3]);
                let (a, b, b2, c2) = (btns[4], btns[5], btns[6], btns[7]);
                let (x, y, z, s) = (btns[8], btns[9], btns[10], btns[11]);
                let (d2, w, m) = (btns[12], btns[13], false);
                // engine B/F are facing-relative (input.go: raw L/R → logical).
                // Facing left swaps them, or "holdback" walks forward after a
                // side switch and the CPU fights while moonwalking away.
                let (bf, ff) = if c.facing >= 0 { (l, r) } else { (r, l) };
                c.cmd_list.buffer.update_input_time(
                    u, d, l, r, bf, ff, a, b, b2, c2, x, y, z, s, d2, w,
                );
            }
            let hpbuf = c.hitpause_time > 0;
            c.cmd_list.step(false, false, hpbuf, false, 0);
        }

        // ── 2/3/4. per-char action (prepare → run → physics) ─────────────
        for i in 0..self.chars.len() {
            // hitpause gating (actionPrepare/actionRun skip when paused)
            if self.chars[i].hitpause_time > 0 {
                self.chars[i].hitpause_time -= 1;
                // frozen this frame (Go keeps anim frozen too)
                continue;
            }
            self.action_prepare(i);
            self.auto_turn(i);
            self.action_run(i);
            self.pos_update(i);
            // anim advance + timers
            let anim_no = self.chars[i].anim.anim_no;
            let anim = self.chars[i].gi.anims.anims.get(&anim_no).cloned().unwrap_or_default();
            self.chars[i].anim.action(&anim);
            self.chars[i].time_in_state += 1;
        }

        // hit detection (v1: clsn overlap + hitdef resolution)
        if round_live {
            self.hit_detection();
        }

        // player push (char.go:14522 pushDetection) — after physics so
        // walk/jump positions have settled; keeps bodies from overlapping.
        self.push_detection();

        // camera follow (after physics + hits settle positions)
        self.camera_update();

        // round timers / KO
        if round_live {
            self.cur_round_time -= 1;
            for i in 0..self.chars.len() {
                if self.chars[i].life <= 0 && self.chars[i].alive {
                    self.chars[i].alive = false;
                }
            }
            let any_dead = self.chars.iter().any(|c| !c.alive);
            let timeout = self.cur_round_time <= 0;
            if any_dead || timeout {
                self.round_state = 3;
                self.over_timer = 0;
                // winner bookkeeping: KO first, then time-out by life
                // fraction (draw round: both sides score, system.go 1894)
                if self.chars[0].alive && !self.chars[1].alive {
                    self.wins[0] += 1;
                } else if self.chars[1].alive && !self.chars[0].alive {
                    self.wins[1] += 1;
                } else if timeout && self.chars[0].alive && self.chars[1].alive {
                    let f0 = self.chars[0].life as f32 / self.chars[0].life_max as f32;
                    let f1 = self.chars[1].life as f32 / self.chars[1].life_max as f32;
                    if f0 > f1 {
                        self.wins[0] += 1;
                    } else if f1 > f0 {
                        self.wins[1] += 1;
                    } else {
                        self.wins[0] += 1;
                        self.wins[1] += 1;
                    }
                }
            }
        } else if self.round_state == 3 {
            // over grace (system.go: 3 over → round over), then next round
            // or match end when someone reaches max_wins
            self.over_timer += 1;
            if self.over_timer > self.over_waittime {
                if self.wins[0] >= self.max_wins || self.wins[1] >= self.max_wins {
                    self.finished = true;
                } else {
                    self.start_round();
                }
            }
        }
    }

    /// Begin the next round (system.go roundReset): fighters back to their
    /// start positions, life refilled, power kept, short intro, timer reset.
    fn start_round(&mut self) {
        self.round_no += 1;
        self.intro_time = 30;
        self.round_state = 0;
        self.over_timer = 0;
        self.cur_round_time = self.match_time;
        self.cam_x = 0.0;
        self.cam_y = 0.0;
        let starts = [(self.chars[0].start_pos, 1), (self.chars[1].start_pos, -1)];
        for (i, (x, face)) in starts.iter().enumerate() {
            let c = &mut self.chars[i];
            c.pos = [*x, 0.0, 0.0];
            c.vel = [0.0, 0.0, 0.0];
            c.facing = *face;
            c.life = c.life_max;
            c.alive = true;
            c.ctrl = false;
            c.hitpause_time = 0;
            c.hitdef = None;
            c.hitdef_targets.clear();
            c.move_type = MoveTypeT::I;
            c.pending_state = None;
            c.time_in_state = 0;
            c.cmd_list.buf_reset();
            c.set_anim(0);
            c.change_state(0);
        }
    }

    /// Engine auto-turn (char.go): with ctrl on the ground, the char faces
    /// the nearest opponent — common1's turn states (5/6) refine the anim.
    fn auto_turn(&mut self, i: usize) {
        let me_x = self.chars[i].pos[0];
        let mut best: Option<(f32, usize)> = None; // (dist, idx)
        for (j, c) in self.chars.iter().enumerate() {
            if j == i || !c.alive || c.is_helper {
                continue;
            }
            let d = (c.pos[0] - me_x).abs();
            if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                best = Some((d, j));
            }
        }
        let Some((dist, j)) = best else { return };
        if dist < 0.5 {
            return;
        }
        let toward = (self.chars[j].pos[0] - me_x).signum() as i32;
        let c = &mut self.chars[i];
        if c.ctrl
            && (c.state_type == StateTypeT::S || c.state_type == StateTypeT::C)
            && c.facing != toward
        {
            c.facing = toward;
        }
    }

    /// actionPrepare hardcoded keys (char.go:12239-12277)
    fn action_prepare(&mut self, i: usize) {
        let buf = self.chars[i].cmd_list.buffer.cur; // copy (17 i32s)
        let c = &mut self.chars[i];
        let has_ctrl = c.ctrl && c.round_state_cache == 2;
        let (ub, db, bb, fb) = (
            buf[crate::cmd::KU] > 0,
            buf[crate::cmd::KD] > 0,
            buf[crate::cmd::KB] > 0,
            buf[crate::cmd::KF] > 0,
        );
        if has_ctrl {
            let st = c.state_type;
            let no = c.state_no;
            if c.inguarddist && bb && st != StateTypeT::L {
                if no != 120 && no != 130 && no != 140 {
                    c.change_state(120);
                }
            } else if !(c.asf & ASF_NOJUMP != 0) && st == StateTypeT::S && ub {
                if no != 40 {
                    c.change_state(40);
                }
            } else if !(c.asf & ASF_NOCROUCH != 0) && st == StateTypeT::S && db {
                if no != 10 {
                    if no != 100 {
                        c.vel[0] = 0.0;
                    }
                    c.change_state(10);
                }
            } else if st == StateTypeT::C && db == false {
                if no != 12 {
                    c.change_state(12);
                }
            } else if !(c.asf & ASF_NOWALK != 0) && st == StateTypeT::S && (fb != bb) {
                if no != 20 {
                    c.change_state(20);
                }
            }
        }
        // braking: state 20 with no direction
        if !(c.asf & ASF_NOBRAKE != 0)
            && c.state_no == 20
            && (bb == fb)
        {
            c.change_state(0);
        }
    }

    /// actionRun (char.go:12432): -1 states, state entry, current state
    fn action_run(&mut self, i: usize) {
        let c = &mut self.chars[i];

        // negative states (-4…-1) run first when present. Their persistent
        // counters live per-char and OUTLIVE state changes (bytecode.go
        // shares sb.ctrlsps across ticks — the old per-tick clone re-armed
        // every persistent(n) controller each frame).
        for neg in [-4, -3, -2, -1] {
            let sb_owned = c.gi.states.get(&neg).cloned();
            if let Some(sb) = sb_owned {
                let slot = (-neg - 1) as usize; // -4→0 … -1→3
                let mut ps = std::mem::take(&mut c.neg_ps[slot]);
                if ps.len() < sb.ctrlsps.len() {
                    ps.resize(sb.ctrlsps.len(), 0);
                }
                run_state_bytecode_with_ps(&sb, c, &mut ps);
                c.neg_ps[slot] = ps;
            }
        }

        // pending state entry (stateChange2)
        if c.apply_pending_state() {
            // states may changeState again during run below
        }

        // current state
        let no = c.state_no;
        let sb_owned = c.gi.states.get(&no).cloned();
        if let Some(sb) = sb_owned {
            let c2 = &mut self.chars[i];
            let mut ps = std::mem::take(&mut c2.ps);
            run_state_bytecode_with_ps(&sb, c2, &mut ps);
            self.chars[i].ps = ps;
        }

        // a state run may have queued another change — apply next frame
    }

    /// posUpdate (char.go:10076) + landing (12521)
    fn pos_update(&mut self, i: usize) {
        let c = &mut self.chars[i];
        if !c.pos_frozen {
            c.pos[0] += c.vel[0] * c.facing as f32;
            c.pos[1] += c.vel[1];
            c.pos[2] += c.vel[2];
        }
        // physics friction/gravity
        match c.physics {
            StateTypeT::S => {
                c.vel[0] *= c.gi.movement.stand_friction;
                if c.vel[0].abs() < 1.0 {
                    c.vel[0] = 0.0;
                }
            }
            StateTypeT::C => {
                c.vel[0] *= c.gi.movement.crouch_friction;
            }
            StateTypeT::A => {
                c.vel[1] += c.gi.movement.yaccel;
            }
            _ => {}
        }
        // ground clamp + landing (state 52)
        if c.pos[1] >= 0.0 {
            c.pos[1] = 0.0;
            if c.physics == StateTypeT::A && c.vel[1] > 0.0 {
                // landing: fall back to state 52 (jump landing)
                if c.state_no < 5000 {
                    c.vel[1] = 0.0;
                    c.change_state(52);
                }
            }
        }
        // screen-edge push (char.go screenbound): chars stay within the
        // camera view — 10px margin inside the 160px half-screen
        let left = self.cam_x - 150.0;
        let right = self.cam_x + 150.0;
        if c.pos[0] < left {
            c.pos[0] = left;
            if c.vel[0] < 0.0 {
                c.vel[0] = 0.0;
            }
        } else if c.pos[0] > right {
            c.pos[0] = right;
            if c.vel[0] > 0.0 {
                c.vel[0] = 0.0;
            }
        }
    }

    /// Player push (char.go:14522 pushDetection, 2D x-axis subset).
    ///
    /// Both fighters' size boxes ([Size] ground_back/ground_front around the
    /// axis, mirrored by facing) must overlap on x *and* their current clsn2
    /// boxes must overlap — the MUGEN quirk that lets a jumper pass over a
    /// stander without pushing. The overlap splits 50/50 (equal
    /// weight/pushfactor), which resolves the walk-into-each-other case in
    /// about a frame and stops the "merged fighters" glitch.
    fn push_detection(&mut self) {
        for ai in 0..self.chars.len() {
            for di in (ai + 1)..self.chars.len() {
                let (a, d) = (&self.chars[ai], &self.chars[di]);
                if a.is_helper || d.is_helper || !a.alive || !d.alive {
                    continue;
                }
                if a.no_push || d.no_push {
                    continue;
                }
                // size boxes in stage space, facing-mirrored
                let afront = a.gi.size.ground_front;
                let aback = a.gi.size.ground_back;
                let dfront = d.gi.size.ground_front;
                let dback = d.gi.size.ground_back;
                let (al, ar) = if a.facing >= 0 {
                    (a.pos[0] - aback, a.pos[0] + afront)
                } else {
                    (a.pos[0] - afront, a.pos[0] + aback)
                };
                let (dl, dr) = if d.facing >= 0 {
                    (d.pos[0] - dback, d.pos[0] + dfront)
                } else {
                    (d.pos[0] - dfront, d.pos[0] + dback)
                };
                let ov = ar.min(dr) - al.max(dl);
                if ov <= 0.0 {
                    continue;
                }
                // clsn2 overlap gate (char.go: "only Mugen does this").
                // Mirror-normalized extents — see hit_detection for why the
                // naive `min*facing` produces an inverted box when facing -1.
                let clsn_ov = |c1: &CharState, c2: &CharState| -> bool {
                    let Some(a1) = c1.gi.anims.anims.get(&c1.anim.anim_no) else {
                        return false;
                    };
                    let Some(f1) = a1.frames.get(c1.anim.curelem.max(0) as usize) else {
                        return false;
                    };
                    let Some(a2) = c2.gi.anims.anims.get(&c2.anim.anim_no) else {
                        return false;
                    };
                    let Some(f2) = a2.frames.get(c2.anim.curelem.max(0) as usize) else {
                        return false;
                    };
                    if f1.clsn2.is_empty() || f2.clsn2.is_empty() {
                        return false;
                    }
                    let bx = |x: f32, f: i32, b: &[f32; 4]| -> (f32, f32) {
                        let (mn, mx) = (b[0].min(b[2]), b[0].max(b[2]));
                        if f >= 0 {
                            (x + mn, x + mx)
                        } else {
                            (x - mx, x - mn)
                        }
                    };
                    for b1 in &f1.clsn2 {
                        let (l1, r1) = bx(c1.pos[0], c1.facing, b1);
                        let (t1, bt1) = (
                            c1.pos[1] + b1[1].min(b1[3]),
                            c1.pos[1] + b1[1].max(b1[3]),
                        );
                        for b2 in &f2.clsn2 {
                            let (l2, r2) = bx(c2.pos[0], c2.facing, b2);
                            let (t2, bt2) = (
                                c2.pos[1] + b2[1].min(b2[3]),
                                c2.pos[1] + b2[1].max(b2[3]),
                            );
                            if l1 < r2 && r1 > l2 && t1 < bt2 && bt1 > t2 {
                                return true;
                            }
                        }
                    }
                    false
                };
                if !clsn_ov(a, d) {
                    continue;
                }
                // split the overlap: each side gives way by half
                let a_left = a.pos[0] <= d.pos[0];
                let (da, dd) = (ov * 0.5, ov * 0.5);
                if a_left {
                    self.chars[ai].pos[0] -= da;
                    self.chars[di].pos[0] += dd;
                } else {
                    self.chars[ai].pos[0] += da;
                    self.chars[di].pos[0] -= dd;
                }
            }
        }
    }

    /// v1 hit detection: attacker clsn1 (attack box) vs defender clsn2
    /// (defense box), attributes, damage, hitpause, gethit state entry.
    fn hit_detection(&mut self) {
        // snapshot attacker hitdefs
        let mut hits: Vec<(usize, usize, HitDefT)> = Vec::new();
        for (ai, a) in self.chars.iter().enumerate() {
            let Some(hd) = &a.hitdef else { continue };
            if a.move_type != MoveTypeT::A {
                continue;
            }
            // hitonce latch
            if hd.hitonce && a.hit_count > 0 {
                continue;
            }
            let Some(anim) = a.gi.anims.anims.get(&a.anim.anim_no) else {
                continue;
            };
            let Some(frame) = anim.frames.get(a.anim.curelem as usize) else {
                continue;
            };
            if frame.clsn1.is_empty() {
                continue;
            }
            for (di, d) in self.chars.iter().enumerate() {
                if di == ai || !d.alive {
                    continue;
                }
                // one hit per target per HitDef activation (char.go
                // hasTargetOfHitdef) — without this the same hitdef lands
                // every frame, which reads as a one-hit KO.
                if a.hitdef_targets.contains(&d.id) {
                    continue;
                }
                let Some(danim) = d.gi.anims.anims.get(&d.anim.anim_no) else {
                    continue;
                };
                let Some(dframe) = danim.frames.get(d.anim.curelem as usize) else {
                    continue;
                };
                if dframe.clsn2.is_empty() {
                    continue;
                }
                // boxes: [left, top, right, bottom] in char space.
                // Facing-left mirroring must re-normalize min/max AFTER the
                // flip — multiplying extents by −1 leaves (l,r) inverted and
                // every overlap test against a left-facing char fails.
                let scale = a.gi.size.xscale;
                let (ax, af) = (a.pos[0], a.facing as f32);
                let (dx, df) = (d.pos[0], d.facing as f32);
                let ext = |b: &[f32; 4]| (b[0].min(b[2]), b[0].max(b[2]));
                let bx = |x: f32, f: f32, b: &[f32; 4], s: f32| -> (f32, f32) {
                    let (mn, mx) = ext(b);
                    if f >= 0.0 {
                        (x + mn * s, x + mx * s)
                    } else {
                        (x - mx * s, x - mn * s)
                    }
                };
                let mut overlap = false;
                for b1 in &frame.clsn1 {
                    for b2 in &dframe.clsn2 {
                        let (l1, r1) = bx(ax, af, b1, scale);
                        let t1 = a.pos[1] + b1[1].min(b1[3]);
                        let bt1 = a.pos[1] + b1[1].max(b1[3]);
                        let (l2, r2) = bx(dx, df, b2, 1.0);
                        let t2 = d.pos[1] + b2[1].min(b2[3]);
                        let bt2 = d.pos[1] + b2[1].max(b2[3]);
                        if l1 < r2 && r1 > l2 && t1 < bt2 && bt1 > t2 {
                            overlap = true;
                            break;
                        }
                    }
                    if overlap {
                        break;
                    }
                }
                if overlap {
                    hits.push((ai, di, hd.clone()));
                }
            }
        }
        // resolve
        for (ai, di, hd) in hits {
            // latch this target for the current hitdef activation
            let d_id = self.chars[di].id;
            self.chars[ai].hitdef_targets.push(d_id);
            let attacker_attack = self.chars[ai].attack_mul * self.chars[ai].gi.data.attack as f32 / 100.0;
            let defender_def = self.chars[di].defence_mul * self.chars[di].gi.data.defence as f32 / 100.0;
            let attacker_facing = self.chars[ai].facing;
            let attacker_id = self.chars[ai].id;
            let dmg = (hd.damage[0] as f32 * attacker_attack / defender_def) as i32;
            let d = &mut self.chars[di];
            // hitflag vs statetype: H/S, L/C, A/A — simplified map
            let st_bit = match d.state_type {
                StateTypeT::S => 1,
                StateTypeT::C => 2,
                StateTypeT::A => 4,
                _ => 8,
            };
            if hd.hitflag & st_bit == 0 {
                continue;
            }
            d.life -= dmg;
            if d.life < 0 {
                d.life = if hd.kill.unwrap_or(true) { 0 } else { 1 };
            }
            d.alive = d.life > 0;
            // gethit vars
            d.ghv.damage = dmg;
            d.ghv.xvel = hd.ground_velocity[0];
            d.ghv.yvel = hd.ground_velocity[1];
            d.ghv.hitshaketime = hd.pausetime[1].max(0);
            d.ghv.hittime = hd.ground_hittime.max(0);
            d.ghv.fall = hd.fall;
            d.ghv.id = attacker_id;
            d.hitpause_time = hd.pausetime[0].max(0) + 1;
            d.hit_count += 1;
            d.hitdef_attr_st = 0; // clear own hitdef attr on gethit
            // velocity + facing + state change
            d.vel[0] = hd.ground_velocity[0];
            if d.state_type == StateTypeT::A {
                d.vel[0] = hd.air_velocity[0];
                d.vel[1] = hd.air_velocity[1];
            }
            if hd.p2facing != 0 {
                d.facing = if hd.p2facing < 0 {
                    -attacker_facing
                } else {
                    attacker_facing
                };
            }
            // gethit state (char.go tick 13069): 5000/5010/5020
            let gs = match d.state_type {
                StateTypeT::S => 5000,
                StateTypeT::C => 5010,
                StateTypeT::A => 5020,
                _ => 5000,
            };
            d.move_type = MoveTypeT::H;
            d.change_state(gs);
            // attacker pause + counters
            let a2 = &mut self.chars[ai];
            a2.hitpause_time = hd.pausetime[0].max(0) + 1;
            a2.uniq_hit_count += 1;
        }
    }
}

/// run_state_bytecode with an explicit persistent slice (negative states
/// share the compiled sb.ctrlsps; current states use char-local ps).
pub fn run_state_bytecode_with_ps(
    sb: &StateBytecode,
    host: &mut CharState,
    ps: &mut Vec<i32>,
) -> bool {
    if ps.len() < sb.ctrlsps.len() {
        ps.resize(sb.ctrlsps.len(), 0);
    }
    // state_def params are applied on ENTRY (apply_pending_state), not here.
    // `ps` passes through UNCLONED: persistent counters must survive the
    // call (the old ps.to_vec() threw every countdown away each tick, so
    // persistent(0) latches never latched and persistent(n) never skipped).
    run_block_with_ps(&sb.block, host, ps.as_mut_slice(), false)
}

fn run_block_with_ps(
    block: &crate::statedef::StateBlock,
    host: &mut CharState,
    ps: &mut [i32],
    in_loop: bool,
) -> bool {
    if host.hit_paused() && !block.ignorehitpause {
        return false;
    }
    let pi = block.persistent_index;
    if pi >= 0 {
        let idx = pi as usize;
        if idx >= ps.len() {
            return false; // shouldn't happen; callers pre-size
        }
        // bytecode.go:5136 — decrement FIRST, then test: persistent(1) (the
        // default) runs every tick, persistent(2) every 2nd, 0 = latch.
        if ps[idx] != i32::MAX {
            ps[idx] -= 1;
        }
        if ps[idx] > 0 {
            return false;
        }
    }
    let take = match &block.trigger {
        None => true,
        Some(t) => t.eval(host as &mut dyn Ctx),
    };
    if take {
        for node in &block.ctrls {
            if run_node_with_ps(node, host, ps, in_loop) {
                return true;
            }
        }
        if pi >= 0 {
            ps[pi as usize] = block.persistent;
        }
        false
    } else if let Some(eb) = &block.else_block {
        run_block_with_ps(eb, host, ps, in_loop)
    } else {
        false
    }
}

fn run_node_with_ps(
    node: &Node,
    host: &mut CharState,
    ps: &mut [i32],
    in_loop: bool,
) -> bool {
    match node {
        Node::Sctrl(sc) => {
            let ok = !host.hit_paused() || sc.ignorehitpause;
            if ok {
                crate::sctrl::run_sctrl(sc, host)
            } else {
                false
            }
        }
        Node::Block(b) => run_block_with_ps(b, host, ps, in_loop),
        Node::ExprStmt(exp) => {
            let _ = eval(exp, host as &mut dyn Ctx);
            false
        }
        Node::Call { name_idx, args, ret } => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval(a, host as &mut dyn Ctx));
            }
            let mut out = vec![v_none(); ret.len()];
            host.zss_call(*name_idx, vals, &mut out);
            for (i, slot) in ret.iter().enumerate() {
                if i < out.len() {
                    host.set_local(*slot, out[i]);
                }
            }
            false
        }
        Node::Let { slot, exp } => {
            let v = eval(exp, host as &mut dyn Ctx);
            host.set_local(*slot, v);
            false
        }
        Node::Break | Node::Continue => false,
        Node::Loop { cond, init, incr, body } => {
            if let Some((slot, exp)) = init {
                let v = eval(exp, host as &mut dyn Ctx);
                host.set_local(*slot, v);
            }
            let mut iters = 0usize;
            loop {
                if let Some(c) = cond {
                    if !eval(c, host as &mut dyn Ctx).to_b() {
                        break;
                    }
                }
                let mut aborted = false;
                for n in body {
                    if run_node_with_ps(n, host, ps, true) {
                        aborted = true;
                        break;
                    }
                    if matches!(n, Node::Break) {
                        aborted = true;
                        break;
                    }
                }
                if aborted {
                    break;
                }
                if let Some(i) = incr {
                    let _ = eval(i, host as &mut dyn Ctx);
                }
                iters += 1;
                if iters > 250_000 {
                    break;
                }
            }
            false
        }
    }
}

impl CharState {
    fn next_rand(&mut self, lo: i32, hi: i32) -> i32 {
        // deterministic per-char rand (Fight provides seeds via frame data;
        // v1: char-local LCG seeded from id)
        let r = (self.id as i64 * 1103515245 + self.time_in_state as i64) % 2147483647;
        lo + (r % (hi - lo + 1) as i64) as i32
    }
}

// ── char loading ───────────────────────────────────────────────────────────────

/// Compile a full character from its definition directory + the engine
/// common texts (cmd/cns/const are resolved like compiler.go Compile:
/// st files → cmd states → stcommon → engine common states).
pub fn compile_char(
    name: &str,
    cmd_text: &str,
    cns_texts: &[String],
    stcommon_text: Option<&str>,
    common_const_text: &str,
    air_texts: &[String],
) -> Result<std::sync::Arc<GlobalInfo>, String> {
    // commands
    let cmds = crate::cmd::CmdFileCommands::parse(cmd_text)?;
    let mut cl = CommandList::new();
    cmds.build(&mut cl)?;
    let cl: &'static mut CommandList = Box::leak(Box::new(cl));

    // compiler context (leaked static lifetime keeps StateCompiler simple;
    // data moves into GlobalInfo at the end)
    let sc: &'static mut StateCompiler<'static> =
        Box::leak(Box::new(StateCompiler::new(cl, 0)));

    // seed the pool with the type letters so statetype compares resolve
    for t in TYPE_POOL_SEED {
        sc.add_string(t);
    }

    let (mut data, mut size, mut velocity, mut movement) = (
        CharData::default(),
        CharSize::default(),
        CharVelocity::default(),
        CharMovement::default(),
    );
    // constants precedence matches Ikemen: common.const first, then the
    // char's own cns files ([Data]/[Size]/[Velocity]/[Movement] at the top
    // of kfm.cns) override. Skipping the char pass made every fighter run
    // on generic defaults — walk.back lost its sign and KFM walked his
    // "backward" vel forward.
    read_constants(common_const_text, sc, &mut data, &mut size, &mut velocity, &mut movement);
    for t in cns_texts {
        read_constants(t, sc, &mut data, &mut size, &mut velocity, &mut movement);
    }

    let mut states: HashMap<i32, StateBytecode> = HashMap::new();
    // KFM is a Mugen-version char: negoverride = true
    for t in cns_texts {
        sc.compile_cns_text(t, "char.cns", true, &mut states)?;
    }
    sc.compile_cns_text(cmd_text, "char.cmd", true, &mut states)?;
    if let Some(tc) = stcommon_text {
        // .cns → .cns.zss fallback handled by caller supplying zss text
        crate::zss::compile_zss_text(sc, tc, "common.cns.zss", false, &mut states)?;
    }

    // animations: common.air first (fills gaps), char's air second (wins)
    let mut ac = krg_format::anim::AnimCompiler::new(false);
    for (i, t) in air_texts.iter().enumerate() {
        ac.compile_text(if i == 0 { "common.air" } else { "char.air" }, t);
    }
    let anims = ac.finalize();

    let gi = GlobalInfo {
        name: name.to_string(),
        states,
        string_pool: std::mem::take(&mut sc.string_pool),
        funcs: std::mem::take(&mut sc.funcs),
        data,
        size,
        velocity,
        movement,
        named: std::mem::take(&mut sc.named),
        localcoord: 320.0, // def [Info] not parsed yet — classic chars
        cmd_template: {
            // rebuild a fresh template from the same compiled definitions
            let mut t = CommandList::new();
            cmds.build(&mut t)?;
            t
        },
        anims,
        warnings: std::mem::take(&mut sc.warnings),
    };
    Ok(std::sync::Arc::new(gi))
}

impl Fight {
    /// Two identical fighters, mirror positions, facing each other.
    pub fn new_1v1(gi: std::sync::Arc<GlobalInfo>, seed: i32) -> Self {
        let mut c1 = CharState::new(gi.clone(), 0, 0, 1);
        c1.pos = [-60.0, 0.0, 0.0];
        c1.start_pos = -60.0;
        c1.facing = 1;
        c1.init_commands();
        c1.set_anim(0);
        let mut c2 = CharState::new(gi, 1, 1, 2);
        c2.pos = [60.0, 0.0, 0.0];
        c2.start_pos = 60.0;
        c2.facing = -1;
        c2.init_commands();
        c2.set_anim(0);
        Fight::new(vec![c1, c2], seed)
    }
}

impl CharState {
    pub fn p2_life_x(&self) -> i32 {
        self.p2.life
    }
}

// safety: mark Send+Sync for GlobalInfo (all plain data)
unsafe impl Send for GlobalInfo {}
unsafe impl Sync for GlobalInfo {}

// ── android scene bridge ──────────────────────────────────────────────────────

/// One character's renderable state (drawn by krg-android from the scene).
#[derive(Debug, Clone, Default)]
pub struct CharDraw {
    pub pos: [f32; 3],
    pub facing: i32,
    pub anim_no: i32,
    /// current AIR element index (0-based)
    pub frame_idx: i32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub xscale: f32,
    pub yscale: f32,
    pub life: i32,
    pub life_max: i32,
    pub power: i32,
    pub power_max: i32,
    pub state_no: i32,
    pub state_type: StateTypeT,
    pub move_type: MoveTypeT,
    pub alive: bool,
}

/// Per-frame render state for the fight scene.
#[derive(Debug, Clone, Default)]
pub struct FightSnapshot {
    pub chars: Vec<CharDraw>,
    pub round_state: i32,
    pub round_no: i32,
    pub time_left: i32,
    /// Camera scroll (stage space): draw world at (pos - cam)
    pub cam_x: f32,
    pub cam_y: f32,
}

impl Fight {
    /// Step one 60Hz tick with raw InputBits (krg-input::bits layout) —
    /// the android touch controller feeds these directly.
    pub fn step_bits(&mut self, p1: u32, p2: u32) {
        let bits_to_arr = |b: u32| -> [bool; 14] {
            [
                b & crate::cmd::PU_BIT != 0,
                b & crate::cmd::PD_BIT != 0,
                b & crate::cmd::PL_BIT != 0,
                b & crate::cmd::PR_BIT != 0,
                b & crate::cmd::ABIT != 0,
                b & crate::cmd::BBIT != 0,
                b & crate::cmd::CBIT != 0,
                b & crate::cmd::XBIT != 0,
                b & crate::cmd::YBIT != 0,
                b & crate::cmd::ZBIT != 0,
                b & crate::cmd::SBIT != 0,
                b & crate::cmd::DBIT != 0,
                b & crate::cmd::WBIT != 0,
                b & crate::cmd::MBIT != 0,
            ]
        };
        self.step(&[bits_to_arr(p1), bits_to_arr(p2)]);
    }

    /// Render snapshot (cheap; clones only the draw-relevant fields).
    pub fn snapshot(&self) -> FightSnapshot {
        let chars = self
            .chars
            .iter()
            .map(|c| {
                let frame_idx = {
                    let anim = c.gi.anims.anims.get(&c.anim.anim_no);
                    match anim {
                        Some(a) if !a.frames.is_empty() => {
                            (c.anim.curelem.max(0) as usize).min(a.frames.len() - 1) as i32
                        }
                        _ => 0,
                    }
                };
                let (flip_h, flip_v, xs, ys) = {
                    let anim = c.gi.anims.anims.get(&c.anim.anim_no);
                    match anim.and_then(|a| a.frames.get(frame_idx.max(0) as usize)) {
                        Some(f) => (f.hscale < 0, f.vscale < 0, f.xscale, f.yscale),
                        None => (false, false, 1.0, 1.0),
                    }
                };
                CharDraw {
                    pos: c.pos,
                    facing: c.facing,
                    anim_no: c.anim.anim_no,
                    frame_idx,
                    flip_h,
                    flip_v,
                    xscale: c.gi.size.xscale,
                    yscale: c.gi.size.yscale,
                    life: c.life,
                    life_max: c.life_max,
                    power: c.power,
                    power_max: c.power_max,
                    state_no: c.state_no,
                    state_type: c.state_type,
                    move_type: c.move_type,
                    alive: c.alive,
                }
            })
            .collect();
        FightSnapshot {
            chars,
            round_state: self.round_state,
            round_no: self.round_no,
            time_left: self.cur_round_time / 60,
            cam_x: self.cam_x,
            cam_y: self.cam_y,
        }
    }
}
