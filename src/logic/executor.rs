//! Logic executor: variables, instructions, and the per-tick loop.

use super::compiler::{Expr, Program};
use super::ops::{
    item_name_from_id, liquid_name_from_id, unit_name_from_id, Cond, LAccess, LookupKind, Op,
};
use super::view::{SensorValue, WorldView};
use std::sync::Arc;

/// Official LExecutor graphics/text bounds (v158.1).
///
/// `graphicsBuffer` is a per-executor command queue.  DrawI refuses to append
/// once it reaches 256 commands, while a LogicDisplay keeps at most 1024
/// pending commands after `drawflush`.
const MAX_GRAPHICS_BUFFER: usize = 256;
pub(crate) const MAX_DISPLAY_BUFFER: usize = 1024;
/// The print buffer is truncated at 400 characters; printchar refuses to
/// append past it.
const MAX_TEXT_BUFFER: usize = 400;

/// Object value carried by a variable. Phase 1 knows buildings and strings.
#[derive(Clone, Debug, PartialEq)]
pub enum LObject {
    Null,
    /// Building position (packed (x << 16) | y).
    Building(i32),
    /// Unit id (logic-bound units).
    Unit(i32),
    /// UnitType content reference (`SpawnUnitI.type.obj()` instanceof UnitType).
    UnitType(i16),
    /// Team content reference (`LVar.team()` instanceof Team).
    Team(u8),
    /// String value (print/printflush and set).
    Str(String),
}

/// A logic variable (name, numeric/object value, constant flag).
#[derive(Clone, Debug)]
pub struct LVar {
    #[allow(dead_code)]
    pub name: String,
    pub isobj: bool,
    pub numval: f64,
    pub objval: LObject,
    pub constant: bool,
}

impl LVar {
    /// Official `LVar.num()` (v159.7): non-null object -> 1, null object -> 0,
    /// numeric NaN/Inf -> 0, otherwise `numval`.
    pub fn num(&self) -> f64 {
        if self.isobj {
            if matches!(self.objval, LObject::Null) {
                0.0
            } else {
                1.0
            }
        } else if lvar_invalid(self.numval) {
            0.0
        } else {
            self.numval
        }
    }
    #[allow(dead_code)]
    pub fn obj(&self) -> &LObject {
        &self.objval
    }
    pub(crate) fn new_num(name: &str, value: f64) -> LVar {
        LVar {
            name: name.to_string(),
            isobj: false,
            numval: value,
            objval: LObject::Null,
            constant: true,
        }
    }
    pub(crate) fn new_obj(name: &str, value: LObject) -> LVar {
        LVar {
            name: name.to_string(),
            isobj: true,
            numval: 0.0,
            objval: value,
            constant: true,
        }
    }
    pub(crate) fn new_var(name: &str) -> LVar {
        LVar {
            name: name.to_string(),
            isobj: true,
            numval: 0.0,
            objval: LObject::Null,
            constant: false,
        }
    }
}

/// Graphics commands accepted by the v158.1 `draw` statement.  The numeric
/// discriminants are the wire/display command IDs from `LogicDisplay`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawType {
    Clear = 0,
    Color = 1,
    /// Virtual command: unpack a packed RGBA double into `Color`.
    ColorPack = 2,
    Stroke = 3,
    Line = 4,
    Rect = 5,
    LineRect = 6,
    Poly = 7,
    LinePoly = 8,
    Triangle = 9,
    Image = 10,
    /// The executor expands this to one command per character.
    Print = 11,
    Translate = 12,
    Scale = 13,
    Rotate = 14,
    Reset = 15,
}

impl DrawType {
    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "clear" => Self::Clear,
            "color" => Self::Color,
            "col" => Self::ColorPack,
            "stroke" => Self::Stroke,
            "line" => Self::Line,
            "rect" => Self::Rect,
            "lineRect" => Self::LineRect,
            "poly" => Self::Poly,
            "linePoly" => Self::LinePoly,
            "triangle" => Self::Triangle,
            "image" => Self::Image,
            "print" => Self::Print,
            "translate" => Self::Translate,
            "scale" => Self::Scale,
            "rotate" => Self::Rotate,
            "reset" => Self::Reset,
            _ => return None,
        })
    }
}

/// Values for a single `draw` instruction.  LStatements always allocates all
/// six operand slots (x, y, p1..p4), even for commands that use fewer; keeping
/// those slots here preserves jump/counter instruction alignment.
#[derive(Clone, Debug)]
pub struct DrawSpec {
    pub kind: DrawType,
    pub x: Expr,
    pub y: Expr,
    pub p1: Expr,
    pub p2: Expr,
    pub p3: Expr,
    pub p4: Expr,
}

/// Compiled instruction (LExecutor instruction subset).
#[derive(Clone, Debug)]
pub enum Instr {
    Set(usize, Expr),
    Op(usize, Op, Expr, Option<Expr>),
    Read(usize, Expr, Expr),
    Write(Expr, Expr, Expr),
    Jump(i32, Cond, Expr, Expr),
    Wait(Expr),
    GetLink(usize, Expr),
    Sensor(usize, Expr, LAccess),
    Print(Vec<Expr>),
    PrintChar(Expr),
    /// `draw <type> x y p1 p2 p3 p4`.
    Draw(DrawSpec),
    /// Flushes the executor graphics buffer to a same-team logic display.
    DrawFlush(Expr),
    PrintFlush(Expr),
    /// stop — halts execution at this instruction (counter back to self,
    /// yield). The official `exec.stop` flag only matters for LogicScript,
    /// which this port does not run.
    Stop,
    /// packcolor result r g b a — Color.toDoubleBits(clamp01(x) each channel).
    PackColor(usize, Expr, Expr, Expr, Expr),
    /// unpackcolor r g b a value — Color.fromDouble (low 32 bits, /255f).
    UnpackColor(usize, usize, usize, usize, Expr),
    ControlEnabled(Expr, Expr),
    SetRate(Expr),
    /// bind the executor to a unit of the given type (compile-time `@type`).
    Ubind(i16),
    /// `ubind <expr>`: the operand is evaluated at RUNTIME (official
    /// UnitBindI reads `type.obj()` when the instruction runs). A variable
    /// holding a Unit object binds that unit; anything else (numbers,
    /// strings, null) binds nothing, mirroring the official instanceof
    /// dispatch.
    UbindExpr(Expr),
    /// unit control subcommand (phase 2 subset).
    Ucontrol(UcOp),
    /// radar from t1 t2 t3 order sort output.
    Radar(RadarSpec),
    /// ulocate find group enemy ore building x y.
    Ulocate(UlocSpec),
    /// lookup kind index result — content name by id.
    Lookup(LookupKind, Expr, usize),
    /// fetch kind result team index extra — official FetchI (desktop
    /// 158.1): `fetch <type> <result> <team> <index> [extra]`, privileged
    /// only (FetchStatement.privileged() == true).
    Fetch(crate::logic::ops::FetchKind, usize, Expr, Expr, Expr),
    /// format value — official FormatI: substitute the LAST `{n}` in the
    /// runtime textBuffer with the formatted value.
    Format(Expr),
    /// getblock layer result x y — official GetBlockI (tile coordinates,
    /// privileged only).
    GetBlock(crate::logic::ops::TileLayer, usize, Expr, Expr),
    /// setblock layer block x y team rotation — official SetBlockI (tile
    /// coordinates, privileged only).
    SetBlock(crate::logic::ops::TileLayer, Expr, Expr, Expr, Expr, Expr),
    /// setflag flag value — global flag.
    SetFlag(Expr, Expr),
    /// getflag result flag.
    GetFlag(usize, Expr),
    /// spawn unit x y rotation result (hyper processors only).
    Spawn(SpawnSpec),
    /// status apply/clear — official ApplyEffectI, privileged.
    ApplyStatus(ApplyStatusSpec),
    /// spawnwave natural x y — official SpawnWaveI, privileged.
    SpawnWave(Expr, Expr, Expr),
    /// setrule rule value p1 p2 p3 p4 — official SetRuleI, privileged.
    SetRule(SetRuleSpec),
    /// explosion team x y radius damage air ground pierce — official
    /// ExplosionI damage path (no FX), privileged.
    Explosion(ExplosionSpec),
    /// setprop type of value — official SetPropI, privileged.
    SetProp(SetPropSpec),
    End,
    NoOp,
}

#[derive(Clone, Debug)]
pub struct SpawnSpec {
    /// Official SpawnUnitI.type: resolved at runtime via `obj() instanceof UnitType`.
    pub unit_type: Expr,
    pub x: Expr,
    pub y: Expr,
    pub rotation: Expr,
    /// Official SpawnUnitI.team (v159.7): runtime team operand.
    pub team: Expr,
    pub result: usize,
    /// Official SpawnUnitI.effect (v159.7): runtime LVar; `effect.bool()` gates spawn statuses.
    pub effect: Expr,
}

/// Official `ApplyEffectI`: `status <clear> <effect> <unit> <duration>`.
#[derive(Clone, Debug)]
pub struct ApplyStatusSpec {
    pub clear: bool,
    pub effect: String,
    pub unit: Expr,
    pub duration: Expr,
}

/// Official `LogicRule` (desktop 158.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicRule {
    CurrentWaveTime,
    WaveTimer,
    Waves,
    Wave,
    WaveSpacing,
    WaveSending,
    AttackMode,
    EnemyCoreBuildRadius,
    DropZoneRadius,
    UnitCap,
    MapArea,
    Lighting,
    CanGameOver,
    AmbientLight,
    SolarMultiplier,
    DragMultiplier,
    Ban,
    Unban,
    PauseDisabled,
    MusicVolume,
    BuildSpeed,
    UnitHealth,
    UnitBuildSpeed,
    UnitMineSpeed,
    UnitCost,
    UnitDamage,
    BlockHealth,
    BlockDamage,
    RtsMinWeight,
    RtsMinSquad,
}

impl LogicRule {
    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "currentWaveTime" => Self::CurrentWaveTime,
            "waveTimer" => Self::WaveTimer,
            "waves" => Self::Waves,
            "wave" => Self::Wave,
            "waveSpacing" => Self::WaveSpacing,
            "waveSending" => Self::WaveSending,
            "attackMode" => Self::AttackMode,
            "enemyCoreBuildRadius" => Self::EnemyCoreBuildRadius,
            "dropZoneRadius" => Self::DropZoneRadius,
            "unitCap" => Self::UnitCap,
            "mapArea" => Self::MapArea,
            "lighting" => Self::Lighting,
            "canGameOver" => Self::CanGameOver,
            "ambientLight" => Self::AmbientLight,
            "solarMultiplier" => Self::SolarMultiplier,
            "dragMultiplier" => Self::DragMultiplier,
            "ban" => Self::Ban,
            "unban" => Self::Unban,
            "pauseDisabled" => Self::PauseDisabled,
            "musicVolume" => Self::MusicVolume,
            "buildSpeed" => Self::BuildSpeed,
            "unitHealth" => Self::UnitHealth,
            "unitBuildSpeed" => Self::UnitBuildSpeed,
            "unitMineSpeed" => Self::UnitMineSpeed,
            "unitCost" => Self::UnitCost,
            "unitDamage" => Self::UnitDamage,
            "blockHealth" => Self::BlockHealth,
            "blockDamage" => Self::BlockDamage,
            "rtsMinWeight" => Self::RtsMinWeight,
            "rtsMinSquad" => Self::RtsMinSquad,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SetRuleSpec {
    pub rule: LogicRule,
    pub value: Expr,
    pub p1: Expr,
    pub p2: Expr,
    pub p3: Expr,
    pub p4: Expr,
}

#[derive(Clone, Debug)]
pub struct ExplosionSpec {
    pub team: Expr,
    pub x: Expr,
    pub y: Expr,
    pub radius: Expr,
    pub damage: Expr,
    pub air: Expr,
    pub ground: Expr,
    pub pierce: Expr,
}

/// Official SetPropI `type` operand: an `LAccess.settable` property or an
/// item/liquid content key.
#[derive(Clone, Debug)]
pub enum SetPropKey {
    Access(LAccess),
    Item(i16),
    Liquid(i16),
}

#[derive(Clone, Debug)]
pub struct SetPropSpec {
    pub key: SetPropKey,
    pub of: Expr,
    pub value: Expr,
}

#[derive(Clone, Debug)]
pub struct RadarSpec {
    pub from: Expr,
    pub targets: [RadarTarget; 3],
    pub sort: RadarSort,
    pub order: Expr,
    pub output: usize,
}

#[derive(Clone, Debug)]
pub struct UlocSpec {
    pub kind: UlocKind,
    pub group: UlocGroup,
    pub enemy: Expr,
    pub ore: Option<String>,
    pub building: usize,
    pub x: usize,
    pub y: usize,
}

/// `ucontrol` subcommands (unsupported ones compile to NoOp).
#[derive(Clone, Debug)]
pub enum UcOp {
    Move(Expr, Expr),
    Stop,
    /// within x y radius result — pure distance check.
    Within(Expr, Expr, Expr, usize),
    /// getBlock x y building floor — read the tile at (x, y).
    GetBlock(Expr, Expr, usize, usize),
    /// flag value — set the bound unit's flag.
    Flag(Expr),
    /// boost enabled — toggle the boosting stance.
    Boost(Expr),
    /// mine x y — set the mining target for the bound unit.
    Mine(Expr, Expr),
    /// itemDrop to amount — transfer items from the unit to a building.
    ItemDrop(Expr, Expr),
    /// itemTake from item amount — take items from a building into the unit.
    ItemTake(Expr, i16, Expr),
    /// shoot x y shoot — force the bound unit to aim and fire at a point.
    Shoot(Expr, Expr, Expr),
    /// target x y shoot — aim at a point (fires when shoot is non-zero).
    Target(Expr, Expr, Expr),
    /// build x y block rotation — construct a block (progress-based).
    Build(Expr, Expr, i16, Expr),
    /// pathfind x y — ControlPathfinder PathfindResult (v159.7 LogicAI).
    Pathfind(Expr, Expr),
    /// unbind — reset the unit's controller (`unit.resetController()`); the
    /// executor's `@unit` binding itself is NOT cleared (P0-03).
    Unbind,
}

/// Radar target filter (RadarTarget).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadarTarget {
    Any,
    Enemy,
    Ally,
    Player,
    Attacker,
    Flying,
    Ground,
    Boss,
}

impl RadarTarget {
    pub fn parse(token: &str) -> Option<RadarTarget> {
        Some(match token {
            "any" => RadarTarget::Any,
            "enemy" => RadarTarget::Enemy,
            "ally" => RadarTarget::Ally,
            "player" => RadarTarget::Player,
            "attacker" => RadarTarget::Attacker,
            "flying" => RadarTarget::Flying,
            "ground" => RadarTarget::Ground,
            "boss" => RadarTarget::Boss,
            _ => return None,
        })
    }
}

/// Radar sort key (RadarSort).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadarSort {
    Distance,
    Health,
    Shield,
    Armor,
    MaxHealth,
    X,
    Y,
}

impl RadarSort {
    pub fn parse(token: &str) -> Option<RadarSort> {
        Some(match token {
            "distance" => RadarSort::Distance,
            "health" => RadarSort::Health,
            "shield" => RadarSort::Shield,
            "armor" => RadarSort::Armor,
            "maxHealth" => RadarSort::MaxHealth,
            "x" => RadarSort::X,
            "y" => RadarSort::Y,
            _ => return None,
        })
    }
}

/// `ulocate` find kinds (first token).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UlocKind {
    Building,
    Ore,
    Spawn,
    Damaged,
    Core,
}

impl UlocKind {
    pub fn parse(token: &str) -> Option<UlocKind> {
        Some(match token {
            "building" => UlocKind::Building,
            "ore" => UlocKind::Ore,
            "spawn" => UlocKind::Spawn,
            "damaged" => UlocKind::Damaged,
            "core" => UlocKind::Core,
            _ => return None,
        })
    }
}

/// `ulocate` building groups (second token for building/damaged finds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UlocGroup {
    Core,
    Storage,
    Generator,
    Turret,
    Factory,
    Repair,
    Battery,
    Resupply,
    Reactor,
    Drill,
    Shield,
    Unit,
    All,
}

impl UlocGroup {
    pub fn parse(token: &str) -> Option<UlocGroup> {
        Some(match token {
            "core" => UlocGroup::Core,
            "storage" => UlocGroup::Storage,
            "generator" => UlocGroup::Generator,
            "turret" => UlocGroup::Turret,
            "factory" => UlocGroup::Factory,
            "repair" => UlocGroup::Repair,
            "battery" => UlocGroup::Battery,
            "resupply" => UlocGroup::Resupply,
            "reactor" => UlocGroup::Reactor,
            "drill" => UlocGroup::Drill,
            "shield" => UlocGroup::Shield,
            "unit" => UlocGroup::Unit,
            "all" => UlocGroup::All,
            _ => return None,
        })
    }
}

/// Per-tile executor state (not persisted; recompiled from config on load).
pub struct ExecutorState {
    pub program: Arc<Program>,
    pub vars: Vec<LVar>,
    pub counter: usize,
    pub wait: Vec<f64>,
    pub yield_flag: bool,
    /// Resolved link building positions (from the config link list).
    pub links: Vec<i32>,
    /// Desired instructions/second (setrate), 0 = block default.
    pub rate: f64,
    /// print/printflush text buffer.
    pub text_buffer: String,
    /// Packed `DisplayCmd` values waiting for `drawflush`.
    pub graphics_buffer: Vec<u64>,
    /// Hash of the config bytes this executor was compiled from.
    pub config_hash: u64,
    /// Unit bound by `ubind` (id), if any.
    pub bound_unit: Option<i32>,
    /// Per-unit-type round-robin cursors (`LExecutor.binds`, indexed by
    /// unit content id and grown lazily). A fresh executor starts every
    /// cursor at 0; recompiling a processor rebuilds the ExecutorState and
    /// resets the cursors, matching the fresh LExecutor Java installs on
    /// code change.
    pub bind_cursors: Vec<usize>,
    /// World processors (block 442) execute privileged LExecutor code;
    /// ordinary micro/logic/hyper processors remain unprivileged.
    pub privileged: bool,
    /// P0-7: set when strict mode rejected this program (unsupported or
    /// malformed statements). A rejected executor never runs, so the server
    /// fails loudly instead of executing a silently-degraded program.
    pub rejected: bool,
}

impl ExecutorState {
    pub fn new(program: Arc<Program>, links: Vec<i32>) -> ExecutorState {
        let instruction_count = program.instructions.len();
        ExecutorState {
            vars: program.vars.clone(),
            program,
            counter: 0,
            wait: vec![0.0; instruction_count],
            yield_flag: false,
            links,
            rate: 0.0,
            text_buffer: String::new(),
            graphics_buffer: Vec::new(),
            config_hash: 0,
            bound_unit: None,
            bind_cursors: Vec::new(),
            privileged: false,
            rejected: false,
        }
    }

    /// Official `LExecutor.binds[type.id]` (desktop 158.1 UnitBindI): the
    /// per-unit-type round-robin cursor, 0 for a fresh executor. Out-of-range
    /// ids (never produced by the compiler for content names) read as 0.
    pub fn bind_cursor(&self, unit_type: i16) -> usize {
        usize::try_from(unit_type)
            .ok()
            .and_then(|index| self.bind_cursors.get(index).copied())
            .unwrap_or(0)
    }

    /// Advances the per-type cursor (`binds[type.id] ++` in UnitBindI).
    pub fn advance_bind_cursor(&mut self, unit_type: i16) {
        if let Ok(index) = usize::try_from(unit_type) {
            if index >= self.bind_cursors.len() {
                self.bind_cursors.resize(index + 1, 0);
            }
            self.bind_cursors[index] = self.bind_cursors[index].wrapping_add(1);
        }
    }

    fn var_num(&self, idx: usize) -> f64 {
        self.vars.get(idx).map(LVar::num).unwrap_or(0.0)
    }

    fn eval(&self, expr: &Expr) -> LVar {
        match expr {
            Expr::Num(v) => LVar::new_num("___const", *v),
            Expr::Str(s) => LVar::new_obj("___str", LObject::Str(s.clone())),
            Expr::UnitType(id) => LVar::new_obj("___unit_type", LObject::UnitType(*id)),
            Expr::Team(team) => LVar::new_obj("___team", LObject::Team(*team)),
            Expr::Var(idx) => self
                .vars
                .get(*idx)
                .cloned()
                .unwrap_or_else(|| LVar::new_num("0", 0.0)),
            Expr::Bin(op, a, b) => {
                let av = self.eval(a);
                let bv = self.eval(b);
                if *op == Op::StrictEqual {
                    return LVar::new_num(
                        "___const",
                        (av.isobj == bv.isobj
                            && ((av.isobj && av.objval == bv.objval)
                                || (!av.isobj && av.numval == bv.numval)))
                            as i64 as f64,
                    );
                }
                let value = if op.unary() {
                    op.apply_unary(av.num())
                } else {
                    op.apply(av.num(), bv.num())
                };
                LVar::new_num("___const", value)
            }
        }
    }
}

/// Packs one signed integer into the ten-bit sign-magnitude fields used by
/// `mindustry.gen.DisplayCmd`.  Java's `DisplayCmd.get` masks the argument;
/// retaining the mask here is important for negative and oversized values.
fn pack_sign(value: i64) -> i32 {
    let magnitude = value.unsigned_abs() & 0x1ff;
    (magnitude as i32) | if value < 0 { 0x200 } else { 0 }
}

fn pack_unsigned(value: i64) -> i32 {
    (value as i32) & 0x1ff
}

/// Equivalent to `DisplayCmd.get(type, x, y, p1, p2, p3, p4)` generated by
/// Mindustry's @Struct annotation (4 + six 10-bit fields).
fn display_cmd(kind: DrawType, x: i32, y: i32, p1: i32, p2: i32, p3: i32, p4: i32) -> u64 {
    (kind as u64 & 0xf)
        | ((x as u64 & 0x3ff) << 4)
        | ((y as u64 & 0x3ff) << 14)
        | ((p1 as u64 & 0x3ff) << 24)
        | ((p2 as u64 & 0x3ff) << 34)
        | ((p3 as u64 & 0x3ff) << 44)
        | ((p4 as u64 & 0x3ff) << 54)
}

/// Resolve the image operand to the `(logic-id << 5) | content-type` value
/// expected by LogicDisplay.  The Rust port keeps content names as object
/// variable names/strings; these registries use the official v158.1 IDs.
fn image_content_packed(value: &LVar) -> i32 {
    let name = match (&value.isobj, &value.objval) {
        (true, LObject::Str(name)) => name.as_str(),
        (true, LObject::Unit(id)) => {
            return unit_name_from_id(*id as i16)
                .and_then(crate::network::units::parse_unit_type)
                .map(|id| (i32::from(id) << 5) | 6)
                .unwrap_or(-1);
        }
        (true, LObject::Building(_)) | (true, LObject::Null) => value.name.as_str(),
        _ => return -1,
    };
    let name = name.trim_start_matches('@').trim_matches('"');
    if let Some(id) = crate::game::block_names::block_id_from_name(name) {
        return (i32::from(id) << 5) | 1; // ContentType.block.ordinal()
    }
    if let Some(id) = crate::network::units::parse_unit_type(name) {
        return (i32::from(id) << 5) | 6; // ContentType.unit.ordinal()
    }
    if crate::logic::ops::item_id_from_name(name) >= 0
        && crate::logic::ops::item_name_from_id(crate::logic::ops::item_id_from_name(name))
            == Some(name)
    {
        let id = crate::logic::ops::item_id_from_name(name);
        return i32::from(id) << 5; // ContentType.item.ordinal()
    }
    if let Some(id) = (0..=10).find(|id| liquid_name_from_id(*id) == Some(name)) {
        return (i32::from(id) << 5) | 4; // ContentType.liquid.ordinal()
    }
    -1
}

/// Run `draw print`.  Fonts are not available in the headless server, but the
/// official logic font is monospaced at 7px advance; retaining the same
/// alignment/line-height arithmetic produces the exact command layout for the
/// ASCII glyph set shipped by `Fonts.logic`.
fn draw_print(state: &mut ExecutorState, x: i32, y: i32, align: i32) {
    if state.text_buffer.is_empty() {
        return;
    }
    const ADVANCE: i32 = 7;
    const LINE_HEIGHT: i32 = 14;
    let utf16: Vec<u16> = state.text_buffer.encode_utf16().collect();
    let mut max_width = 0i32;
    let mut lines = 1i32;
    let mut line_width = 0i32;
    for ch in &utf16 {
        if *ch == b'\n' as u16 {
            max_width = max_width.max(line_width);
            line_width = 0;
            lines += 1;
        } else {
            line_width += 1;
        }
    }
    max_width = max_width.max(line_width);
    let width = max_width * ADVANCE;
    let height = lines * LINE_HEIGHT;
    let is_left = matches!(align, 8 | 10 | 12);
    let is_right = matches!(align, 16 | 18 | 20);
    let is_top = matches!(align, 2 | 10 | 18);
    let is_bottom = matches!(align, 4 | 12 | 20);
    let ha = ((if is_left { -1 } else { 0 }) + 1 + if is_right { 1 } else { 0 }) as f32 / 2.0;
    let va = ((if is_bottom { -1 } else { 0 }) + 1 + if is_top { 1 } else { 0 }) as f32 / 2.0;
    let x_offset = -(width as f32 * ha) as i32;
    let y_offset = -(height as f32 * va) as i32 + (lines - 1) * LINE_HEIGHT;
    let mut cur_x = x;
    let mut cur_y = y;
    for ch in utf16 {
        if state.graphics_buffer.len() >= MAX_GRAPHICS_BUFFER {
            break;
        }
        if ch == b'\n' as u16 {
            cur_y -= LINE_HEIGHT;
            cur_x = x;
            continue;
        }
        // Fonts.logic is pre-baked with ASCII only. Java still advances over
        // missing glyphs, but does not append a command for them.
        if ch <= 0x7f {
            state.graphics_buffer.push(display_cmd(
                DrawType::Print,
                pack_sign(i64::from(cur_x + x_offset)),
                pack_sign(i64::from(cur_y + y_offset)),
                i32::from(ch),
                0,
                0,
                0,
            ));
        }
        cur_x += ADVANCE;
    }
    state.text_buffer.clear();
}

/// Runs one instruction; side effects touch the world through the provided
/// context. Returns false when the executor yielded (stop the tick budget).
pub fn run_instruction(
    state: &mut ExecutorState,
    world: Option<&WorldView>,
    instr: &Instr,
) -> bool {
    match instr {
        Instr::Set(dest, expr) => {
            let value = state.eval(expr);
            if let Some(v) = state.vars.get_mut(*dest) {
                if !v.constant {
                    v.isobj = value.isobj;
                    v.numval = value.numval;
                    v.objval = value.objval;
                }
            }
        }
        Instr::Op(dest, op, a, b) => {
            let av = state.eval(a);
            let value = if op.unary() {
                op.apply_unary(av.num())
            } else if let Some(b) = b {
                let bv = state.eval(b);
                if *op == Op::StrictEqual {
                    (av.isobj == bv.isobj
                        && ((av.isobj && av.objval == bv.objval)
                            || (!av.isobj && av.numval == bv.numval))) as i64
                        as f64
                } else {
                    op.apply(av.num(), bv.num())
                }
            } else {
                0.0
            };
            if let Some(v) = state.vars.get_mut(*dest) {
                if !v.constant {
                    v.isobj = false;
                    v.numval = value;
                    v.objval = LObject::Null;
                }
            }
        }
        Instr::Read(dest, cell, addr) => {
            let cell_obj = state.eval(cell);
            let addr = state.eval(addr).num() as i64;
            let value = world
                .map(|w| w.read_memory(&cell_obj, addr, state.privileged))
                .unwrap_or(None);
            if let Some(v) = state.vars.get_mut(*dest) {
                if !v.constant {
                    v.isobj = false;
                    v.numval = value.unwrap_or(0.0);
                    v.objval = LObject::Null;
                }
            }
        }
        Instr::Write(cell, addr, value) => {
            let cell_obj = state.eval(cell);
            let addr = state.eval(addr).num() as i64;
            let value = state.eval(value).num();
            if let Some(w) = world {
                w.write_memory(&cell_obj, addr, value, state.privileged);
            }
        }
        Instr::Jump(target, cond, a, b) => {
            let av = state.eval(a);
            let bv = state.eval(b);
            if cond.test(&av, &bv) {
                state.counter = *target as usize;
            }
        }
        Instr::Wait(value) => {
            let ticks = state.eval(value).num();
            let slot = (state.counter as i64 - 1).max(0) as usize;
            if ticks <= 0.0 {
                state.wait[slot] = 0.0;
                state.yield_flag = true;
            } else if state.wait[slot] >= ticks {
                state.wait[slot] = 0.0;
            } else {
                state.counter = slot;
                state.yield_flag = true;
                state.wait[slot] += 1.0 / 60.0; // one game tick per run
            }
        }
        Instr::GetLink(dest, index) => {
            let index = state.eval(index).num() as i64;
            if index >= 0 && (index as usize) < state.links.len() {
                if let Some(v) = state.vars.get_mut(*dest) {
                    if !v.constant {
                        v.isobj = true;
                        v.numval = 0.0;
                        v.objval = LObject::Building(state.links[index as usize]);
                    }
                }
            }
        }
        Instr::Sensor(dest, target, item) => {
            let target = state.eval(target);
            let value = match item {
                LAccess::Counter => SensorValue::Num(state.counter as f64),
                LAccess::Links => SensorValue::Num(state.links.len() as f64),
                LAccess::Ipt => SensorValue::Num(state.var_num(state.program.ipt_var)),
                _ => world
                    .map(|w| w.sensor(&target, *item))
                    .unwrap_or(SensorValue::Num(0.0)),
            };
            if let Some(v) = state.vars.get_mut(*dest) {
                if !v.constant {
                    match value {
                        SensorValue::Num(n) => {
                            v.isobj = false;
                            v.numval = n;
                            v.objval = LObject::Null;
                        }
                        SensorValue::Obj(o) => {
                            v.isobj = true;
                            v.numval = 0.0;
                            v.objval = o;
                        }
                    }
                }
            }
        }
        Instr::Print(exprs) => {
            let mut text = String::new();
            for e in exprs {
                let v = state.eval(e);
                if v.isobj {
                    if let LObject::Str(s) = &v.objval {
                        text.push_str(s);
                    } else {
                        text.push_str("null");
                    }
                } else {
                    text.push_str(&format_number(v.numval));
                }
            }
            state.text_buffer.push_str(&text);
        }
        Instr::Draw(spec) => {
            // Java's DrawI checks the command bound before doing any work,
            // including the virtual color-pack and print expansion.
            if state.graphics_buffer.len() >= MAX_GRAPHICS_BUFFER {
                return !state.yield_flag;
            }
            let x = state.eval(&spec.x);
            let y = state.eval(&spec.y);
            let p1 = state.eval(&spec.p1);
            let p2 = state.eval(&spec.p2);
            let p3 = state.eval(&spec.p3);
            let p4 = state.eval(&spec.p4);
            if spec.kind == DrawType::ColorPack {
                let packed = x.num().to_bits() as u32;
                let r = i64::from((packed >> 24) & 0xff);
                let g = i64::from((packed >> 16) & 0xff);
                let b = i64::from((packed >> 8) & 0xff);
                let a = i64::from(packed & 0xff);
                state.graphics_buffer.push(display_cmd(
                    DrawType::Color,
                    pack_unsigned(r),
                    pack_unsigned(g),
                    pack_unsigned(b),
                    pack_unsigned(a),
                    0,
                    0,
                ));
            } else if spec.kind == DrawType::Print {
                draw_print(state, x.num() as i32, y.num() as i32, p1.num() as i32);
            } else {
                let mut xval = pack_sign(x.num() as i64);
                let mut yval = pack_sign(y.num() as i64);
                let mut p1val = pack_sign(p1.num() as i64);
                let p2val = pack_sign(p2.num() as i64);
                let p3val = pack_sign(p3.num() as i64);
                let mut p4val = pack_sign(p4.num() as i64);
                if spec.kind == DrawType::Image {
                    // Image content is an object and is packed across p1/p4;
                    // p2 and p3 remain the signed size/rotation fields.
                    let packed = image_content_packed(&p1);
                    p1val = packed & 0x3ff;
                    p4val = packed >> 10;
                } else if spec.kind == DrawType::Scale {
                    // LogicDisplay applies scaleStep (0.05) before packing.
                    xval = pack_sign((x.num() / 0.05) as i64);
                    yval = pack_sign((y.num() / 0.05) as i64);
                }
                state.graphics_buffer.push(display_cmd(
                    spec.kind, xval, yval, p1val, p2val, p3val, p4val,
                ));
            }
        }
        Instr::DrawFlush(target) => {
            let target = state.eval(target);
            if let Some(w) = world {
                w.draw_flush(&target, &state.graphics_buffer, state.privileged);
            }
            // Java always clears the local queue, even when target is not a
            // display, invalid, or owned by another team.
            state.graphics_buffer.clear();
        }
        Instr::PrintChar(value) => {
            // Official PrintCharI: objects only append when they are
            // UnlockableContent (emoji); this port has no content objects, so
            // object values are ignored. Numbers append (char)floor(value).
            if state.text_buffer.len() >= MAX_TEXT_BUFFER {
                return !state.yield_flag;
            }
            let v = state.eval(value);
            if !v.isobj {
                // Java narrows double -> int (truncate) then int -> char
                // (mod 2^16); lone surrogates cannot be represented in a Rust
                // String and are dropped.
                let code = (v.num().floor() as i64) & 0xFFFF;
                if let Some(c) = char::from_u32(code as u32) {
                    state.text_buffer.push(c);
                }
            }
        }
        Instr::Stop => {
            // Official StopI: skip back to self and yield. run_tick already
            // advanced the counter, so decrement it back onto this
            // instruction; the processor re-runs stop every tick and halts
            // here permanently (jump/@counter writes could still move on).
            state.counter = (state.counter as i64 - 1).max(0) as usize;
            state.yield_flag = true;
        }
        Instr::PackColor(dest, r, g, b, a) => {
            // Official PackColorI: Color.toDoubleBits(clamp01(ch) * 255f).
            // Mathf.clamp is Math.max(min, Math.min(value, max)): NaN flows
            // through to the (int) cast, which truncates it to 0; f32::clamp
            // returns NaN for a NaN input and the `as i32` cast yields 0.
            let clamp01 = |v: f64| -> f32 { (v as f32).clamp(0.0, 1.0) };
            let channel = |v: f64| -> i32 { (clamp01(v) * 255.0) as i32 & 0xff };
            let rgba = ((channel(state.eval(r).num()) & 0xff) << 24)
                | ((channel(state.eval(g).num()) & 0xff) << 16)
                | ((channel(state.eval(b).num()) & 0xff) << 8)
                | (channel(state.eval(a).num()) & 0xff);
            let value = f64::from_bits((rgba as i64 & 0xFFFF_FFFF) as u64);
            if let Some(v) = state.vars.get_mut(*dest) {
                if !v.constant {
                    v.isobj = false;
                    v.numval = value;
                    v.objval = LObject::Null;
                }
            }
        }
        Instr::UnpackColor(r, g, b, a, value) => {
            // Official UnpackColorI: Color.fromDouble — take the low 32 bits
            // ((int) cast) and divide each byte by 255f (float math).
            let rgba = state.eval(value).num().to_bits() as u32;
            let channel = |shift: u32| -> f64 { (((rgba >> shift) & 0xff) as f32 / 255.0) as f64 };
            let channels = [channel(24), channel(16), channel(8), channel(0)];
            for (dest, channel_value) in [*r, *g, *b, *a].iter().zip(channels) {
                if let Some(v) = state.vars.get_mut(*dest) {
                    if !v.constant {
                        v.isobj = false;
                        v.numval = channel_value;
                        v.objval = LObject::Null;
                    }
                }
            }
        }
        Instr::PrintFlush(target) => {
            let target = state.eval(target);
            let text = std::mem::take(&mut state.text_buffer);
            if let Some(w) = world {
                w.write_message(&target, &text, state.privileged);
            }
        }
        Instr::ControlEnabled(target, value) => {
            let target = state.eval(target);
            let value = state.eval(value).num() != 0.0;
            if let Some(w) = world {
                w.set_enabled(&target, value, state.privileged);
            }
        }
        Instr::SetRate(value) => {
            state.rate = state.eval(value).num();
        }
        Instr::Ubind(unit_type) => {
            // Official UnitBindI (desktop 158.1 offsets 177-206): the
            // executor's team's unit cache for this type, round-robin.
            // Without a world view there is no team data at all — the bind
            // clears (a standalone executor has no candidates).
            if let Some(w) = world {
                w.bind_unit(state, *unit_type);
            } else {
                state.bound_unit = None;
            }
            sync_bound_unit_var(state);
        }
        Instr::UbindExpr(expr) => {
            let operand = state.eval(expr);
            if let (true, LObject::Unit(unit_id)) = (operand.isobj, &operand.objval) {
                // Unit-object branch of UnitBindI: bind that exact unit.
                state.bound_unit = match world {
                    Some(w) => w.bind_unit_object(state, *unit_id),
                    None => None,
                };
            } else {
                // Official: `type.obj()` is neither a UnitType nor a Unit
                // (numbers, strings, null) -> bind nothing.
                state.bound_unit = None;
            }
            sync_bound_unit_var(state);
        }
        Instr::Ucontrol(op) => {
            // P0-03: official UnitControlI.run (desktop 158.1
            // LExecutor.java:344-349) runs `checkLogicAI` BEFORE the switch
            // and skips the WHOLE instruction when it fails. Every op except
            // `unbind` therefore goes through the central acquisition +
            // 600-tick lease refresh (`WorldView::refresh_logic_control`):
            // ubind alone never controls a unit, the first valid ucontrol
            // takes it over (clearing the old mining/build state), and every
            // further valid one re-points/refreshes the lease.
            if matches!(op, UcOp::Unbind) {
                // `unbind`: checkLogicAI still runs first in Java (it may
                // install a fresh LogicAI with the takeover cleanup), then
                // `unit.resetController()` (LExecutor.java:366-369). @unit is
                // NOT cleared — the executor keeps pointing at the same unit
                // and the per-type bind cursor is untouched.
                if let Some(w) = world {
                    w.ucontrol_unbind(state);
                }
            } else if let Some(w) = world {
                if w.refresh_logic_control(state) {
                    match op {
                        UcOp::Move(x, y) => {
                            w.ucontrol_move(state, state.eval(x).num(), state.eval(y).num());
                        }
                        UcOp::Stop => {
                            w.ucontrol_stop(state);
                        }
                        UcOp::Within(x, y, radius, dest) => {
                            let x = state.eval(x).num();
                            let y = state.eval(y).num();
                            let radius = state.eval(radius).num();
                            let result = w.ucontrol_within(state, x, y, radius);
                            if let Some(v) = state.vars.get_mut(*dest) {
                                if !v.constant {
                                    v.isobj = false;
                                    v.numval = result as i64 as f64;
                                    v.objval = LObject::Null;
                                }
                            }
                        }
                        UcOp::GetBlock(x, y, building, floor) => {
                            let x = state.eval(x).num();
                            let y = state.eval(y).num();
                            let (building_obj, floor_id) = w.ucontrol_getblock(x, y);
                            if let Some(v) = state.vars.get_mut(*building) {
                                if !v.constant {
                                    v.isobj = building_obj != LObject::Null;
                                    v.numval = 0.0;
                                    v.objval = building_obj;
                                }
                            }
                            if let Some(v) = state.vars.get_mut(*floor) {
                                if !v.constant {
                                    v.isobj = false;
                                    v.numval = floor_id;
                                    v.objval = LObject::Null;
                                }
                            }
                        }
                        UcOp::Flag(value) => {
                            w.ucontrol_flag(state, state.eval(value).num());
                        }
                        UcOp::Boost(enabled) => {
                            w.ucontrol_boost(state, state.eval(enabled).num() != 0.0);
                        }
                        UcOp::Mine(x, y) => {
                            w.ucontrol_mine(state, state.eval(x).num(), state.eval(y).num());
                        }
                        UcOp::ItemDrop(to, amount) => {
                            let to = state.eval(to);
                            w.ucontrol_itemdrop(state, &to, state.eval(amount).num() as i32);
                        }
                        UcOp::ItemTake(from, item, amount) => {
                            let from = state.eval(from);
                            w.ucontrol_itemtake(
                                state,
                                &from,
                                *item,
                                state.eval(amount).num() as i32,
                            );
                        }
                        UcOp::Shoot(x, y, shoot) => {
                            w.ucontrol_shoot(
                                state,
                                state.eval(x).num(),
                                state.eval(y).num(),
                                state.eval(shoot).num() != 0.0,
                            );
                        }
                        UcOp::Target(x, y, shoot) => {
                            w.ucontrol_shoot(
                                state,
                                state.eval(x).num(),
                                state.eval(y).num(),
                                state.eval(shoot).num() != 0.0,
                            );
                        }
                        UcOp::Build(x, y, block, rotation) => {
                            w.ucontrol_build(
                                state,
                                state.eval(x).num(),
                                state.eval(y).num(),
                                *block,
                                state.eval(rotation).num(),
                            );
                        }
                        UcOp::Pathfind(x, y) => {
                            w.ucontrol_pathfind(state, state.eval(x).num(), state.eval(y).num());
                        }
                        // Handled by the unbind branch above; a gate-passing
                        // execution never reaches this arm.
                        UcOp::Unbind => {}
                    }
                }
            }
        }
        Instr::Radar(spec) => {
            let from = state.eval(&spec.from);
            let order = state.eval(&spec.order).num();
            let found = world.and_then(|w| w.radar_find(&from, &spec.targets, spec.sort, order));
            if let Some(v) = state.vars.get_mut(spec.output) {
                if !v.constant {
                    match found {
                        Some(unit_id) => {
                            v.isobj = true;
                            v.numval = 0.0;
                            v.objval = LObject::Unit(unit_id);
                        }
                        None => {
                            v.isobj = false;
                            v.numval = 0.0;
                            v.objval = LObject::Null;
                        }
                    }
                }
            }
        }
        Instr::Ulocate(spec) => {
            let enemy = state.eval(&spec.enemy).num() != 0.0;
            // Official UnitLocateI scans the bound unit's team; a processor
            // without a bound unit falls back to its own team.
            let team = world
                .and_then(|w| w.bound_unit_team(state))
                .unwrap_or_else(|| world.map(|w| w.processor_team()).unwrap_or(1));
            // P0-03: UnitLocateI runs checkLogicAI first
            // (LExecutor.java:235-238) — a valid bound unit refreshes the
            // Logic lease BEFORE the locate; when the gate fails Java only
            // writes outFound=false and never scans, which the port mirrors
            // by treating the instruction as "not found".
            let controlled = world.is_some_and(|w| w.refresh_logic_control(state));
            let found = if controlled {
                world.and_then(|w| w.ulocate_find(spec, enemy, team))
            } else {
                None
            };
            let (building_obj, x, y) = found.unwrap_or((LObject::Null, 0.0, 0.0));
            if let Some(v) = state.vars.get_mut(spec.building) {
                if !v.constant {
                    v.isobj = building_obj != LObject::Null;
                    v.numval = 0.0;
                    v.objval = building_obj;
                }
            }
            if let Some(v) = state.vars.get_mut(spec.x) {
                if !v.constant {
                    v.isobj = false;
                    v.numval = x;
                    v.objval = LObject::Null;
                }
            }
            if let Some(v) = state.vars.get_mut(spec.y) {
                if !v.constant {
                    v.isobj = false;
                    v.numval = y;
                    v.objval = LObject::Null;
                }
            }
        }
        Instr::Lookup(kind, index, result) => {
            let index = state.eval(index).num() as i16;
            let name = match kind {
                LookupKind::Block => crate::game::block_names::block_name_from_id(index),
                LookupKind::Unit => unit_name_from_id(index),
                LookupKind::Item => item_name_from_id(index),
                LookupKind::Liquid => liquid_name_from_id(index),
            };
            if let Some(v) = state.vars.get_mut(*result) {
                if !v.constant {
                    match name {
                        Some(name) => {
                            v.isobj = true;
                            v.numval = 0.0;
                            v.objval = LObject::Str(name.to_string());
                        }
                        None => {
                            v.isobj = false;
                            v.numval = 0.0;
                            v.objval = LObject::Null;
                        }
                    }
                }
            }
        }
        Instr::Fetch(kind, result, team, index, extra) => {
            // Official FetchI.run (desktop 158.1): `team.team()` null -> the
            // whole instruction no-ops; otherwise index/query the team's
            // data. The statement is privileged-only (FetchStatement.
            // privileged() == true): in micro/logic/hyper processors the
            // official LParser replaces it with an InvalidStatement (NoOp).
            if !state.privileged {
                return !state.yield_flag;
            }
            let Some(w) = world else {
                return !state.yield_flag;
            };
            let Some(team) = lvar_team(&state.eval(team)) else {
                return !state.yield_flag;
            };
            let index = state.eval(index).num() as i32;
            let filter = lvar_content_filter(&state.eval(extra));
            let mut value = LObject::Null;
            let mut numeric = 0.0f64;
            match kind {
                crate::logic::ops::FetchKind::Unit => {
                    // TeamData.units: AI units + player units, in iteration
                    // order; optional UnitType filter via `extra`.
                    let mut seen = 0i32;
                    let mut found = false;
                    for unit in w.world.enemies.iter() {
                        if unit.team == team && unit_matches(filter, unit.unit_type) {
                            if seen == index {
                                value = LObject::Unit(unit.id);
                                found = true;
                                break;
                            }
                            seen += 1;
                        }
                    }
                    if !found && filter.is_none() {
                        // Filtered queries never match the campaign alpha
                        // (see UnitCount below).
                        for player in w.world.players.iter() {
                            if player.team == team {
                                if seen == index {
                                    value = LObject::Unit(player.unit_id);
                                    break;
                                }
                                seen += 1;
                            }
                        }
                    }
                }
                crate::logic::ops::FetchKind::UnitCount => {
                    // Official `unitCount` with a filter uses
                    // `unitCache(type).size`; the port's player units are
                    // the campaign alpha (content id 35), which never
                    // matches a Serpulo unit-type filter (0..=68), so a
                    // filtered count excludes them like the official cache.
                    let mut count = 0i32;
                    for unit in w.world.enemies.iter() {
                        if unit.team == team && unit_matches(filter, unit.unit_type) {
                            count += 1;
                        }
                    }
                    if matches!(filter, None | Some(ContentFilter::Block(_))) {
                        for player in w.world.players.iter() {
                            if player.team == team {
                                count += 1;
                            }
                        }
                    }
                    numeric = f64::from(count);
                    value = LObject::Null;
                }
                crate::logic::ops::FetchKind::Player => {
                    // Official: teamData.players.get(index).unit() (BlockUnitc
                    // -> its tile building); the port returns the unit id.
                    let mut seen = 0i32;
                    for player in w.world.players.iter() {
                        if player.team == team {
                            if seen == index {
                                value = LObject::Unit(player.unit_id);
                                break;
                            }
                            seen += 1;
                        }
                    }
                }
                crate::logic::ops::FetchKind::PlayerCount => {
                    let mut count = 0i32;
                    for player in w.world.players.iter() {
                        if player.team == team {
                            count += 1;
                        }
                    }
                    numeric = f64::from(count);
                    value = LObject::Null;
                }
                crate::logic::ops::FetchKind::Core => {
                    // TeamData.cores: core buildings of the team.
                    let mut seen = 0i32;
                    for tile in w.world.tiles.iter() {
                        if tile.team == team && (339..=344).contains(&tile.block) {
                            if seen == index {
                                value = LObject::Building(tile.position);
                                break;
                            }
                            seen += 1;
                        }
                    }
                }
                crate::logic::ops::FetchKind::CoreCount => {
                    let mut count = 0i32;
                    for tile in w.world.tiles.iter() {
                        if tile.team == team && (339..=344).contains(&tile.block) {
                            count += 1;
                        }
                    }
                    numeric = f64::from(count);
                    value = LObject::Null;
                }
                crate::logic::ops::FetchKind::Build => {
                    // TeamData.buildings (all blocks of the team, cores
                    // included); optional Block filter via `extra`.
                    let mut seen = 0i32;
                    for tile in w.world.tiles.iter() {
                        if tile.team == team && tile.block != 0 && block_matches(filter, tile.block)
                        {
                            if seen == index {
                                value = LObject::Building(tile.position);
                                break;
                            }
                            seen += 1;
                        }
                    }
                }
                crate::logic::ops::FetchKind::BuildCount => {
                    let mut count = 0i32;
                    for tile in w.world.tiles.iter() {
                        if tile.team == team && tile.block != 0 && block_matches(filter, tile.block)
                        {
                            count += 1;
                        }
                    }
                    numeric = f64::from(count);
                    value = LObject::Null;
                }
            }
            if let Some(v) = state.vars.get_mut(*result) {
                if !v.constant {
                    v.isobj = value != LObject::Null;
                    v.numval = numeric;
                    v.objval = value;
                }
            }
        }
        Instr::Format(value) => {
            // Official FormatI.run (JAR offsets 0-292): scan the runtime
            // textBuffer for the LAST `{d}` (d a single digit), replace it
            // with the formatted value, then truncate to 400 chars.
            let value = state.eval(value);
            // Operate in UTF-16 code units exactly like the Java
            // StringBuilder (a `{d}` marker is ASCII, but everything before
            // it may not be).
            let buffer: Vec<u16> = state.text_buffer.encode_utf16().collect();
            let mut position: Option<usize> = None;
            for (i, ch) in buffer.iter().enumerate() {
                if *ch == '{' as u16
                    && buffer.len() - i > 2
                    && (b'0' as u16..=b'9' as u16).contains(&buffer[i + 1])
                    && buffer[i + 2] == '}' as u16
                {
                    position = Some(i);
                }
            }
            let Some(index) = position else {
                return !state.yield_flag;
            };
            let replacement = if value.isobj {
                printi_to_string(&value, world)
            } else if (value.numval - java_round(value.numval) as f64).abs() < 1e-5 {
                java_round(value.numval).to_string()
            } else {
                java_double_to_string(value.numval)
            };
            let mut units: Vec<u16> = buffer;
            let replacement: Vec<u16> = replacement.encode_utf16().collect();
            units.splice(index..index + 3, replacement);
            state.text_buffer = String::from_utf16_lossy(&units);
            // Official FormatI: `if (textBuffer.length() > 400)
            // textBuffer.setLength(400)` — a UTF-16 code-unit cut.
            if state.text_buffer.encode_utf16().count() > MAX_TEXT_BUFFER {
                let mut units = 0usize;
                let mut boundary = 0usize;
                for (index, character) in state.text_buffer.char_indices() {
                    units += character.len_utf16();
                    if units > MAX_TEXT_BUFFER {
                        break;
                    }
                    boundary = index + character.len_utf8();
                }
                state.text_buffer.truncate(boundary);
            }
        }
        Instr::GetBlock(layer, result, x, y) => {
            // Official GetBlockI.run (JAR): tile = world.tile(Mathf.round(x),
            // Mathf.round(y)); null tile -> null result; the layer selects
            // floor/ore/block/building. Privileged-only (GetBlockStatement.
            // privileged() == true).
            if !state.privileged {
                return !state.yield_flag;
            }
            let x = state.eval(x).num();
            let y = state.eval(y).num();
            let tile_x = (x as f32 + 0.5).floor() as i32;
            let tile_y = (y as f32 + 0.5).floor() as i32;
            let result_obj = if let Some(w) = world {
                if tile_x < 0 || tile_y < 0 || tile_x >= w.world.width || tile_y >= w.world.height {
                    LObject::Null
                } else {
                    let pos = (tile_x << 16) | tile_y;
                    let tile = w.world.tiles.get(&pos);
                    match layer {
                        crate::logic::ops::TileLayer::Floor => LObject::Null,
                        crate::logic::ops::TileLayer::Ore => LObject::Null,
                        crate::logic::ops::TileLayer::Block => match tile {
                            Some(tile) if tile.block != 0 => LObject::Building(pos),
                            _ => LObject::Null,
                        },
                        crate::logic::ops::TileLayer::Building => match tile {
                            Some(tile) if tile.block != 0 => LObject::Building(pos),
                            _ => LObject::Null,
                        },
                    }
                }
            } else {
                LObject::Null
            };
            if let Some(v) = state.vars.get_mut(*result) {
                if !v.constant {
                    v.isobj = result_obj != LObject::Null;
                    v.numval = 0.0;
                    v.objval = result_obj;
                }
            }
        }
        Instr::SetBlock(layer, block, x, y, team, rotation) => {
            // Official SetBlockI.run (JAR offsets 0-242): server only,
            // privileged only, tile = world.tile(x.numi(), y.numi()); the
            // layer switch covers floor/ore/block (settable; building is
            // never settable).
            if !state.privileged {
                log::debug!("setblock ignored: processor is not privileged");
                return !state.yield_flag;
            }
            if let Some(w) = world {
                let block = state.eval(block).num() as i16;
                let x = state.eval(x).num();
                let y = state.eval(y).num();
                let team = state.eval(team).num() as u8;
                let rotation = state.eval(rotation).num();
                w.setblock(*layer, block, x, y, team, rotation);
            }
        }
        Instr::SetFlag(flag, value) => {
            if let Some(w) = world {
                let key = match state.eval(flag) {
                    LVar {
                        objval: LObject::Str(s),
                        ..
                    } => s,
                    v => v.name.clone(),
                };
                let value = state.eval(value).num();
                w.set_flag(&key, value);
            }
        }
        Instr::GetFlag(dest, flag) => {
            let value = match world {
                Some(w) => {
                    let key = match state.eval(flag) {
                        LVar {
                            objval: LObject::Str(s),
                            ..
                        } => s,
                        v => v.name.clone(),
                    };
                    w.get_flag(&key)
                }
                None => 0.0,
            };
            if let Some(v) = state.vars.get_mut(*dest) {
                if !v.constant {
                    v.isobj = false;
                    v.numval = value;
                    v.objval = LObject::Null;
                }
            }
        }
        Instr::Spawn(spec) => {
            if !state.privileged {
                return !state.yield_flag;
            }
            let spawned = world.and_then(|w| {
                // SpawnUnitI uses `type.obj() instanceof UnitType`; unlike
                // fetch/content filters, numeric IDs and strings are not
                // coerced into UnitType objects here.
                let unit_type = lvar_unit_type_object(&state.eval(&spec.unit_type))?;
                let team = lvar_team(&state.eval(&spec.team))?;
                // Official SpawnUnitI: World.unconv(tile) + Mathf.range(0.01f).
                let tile_x = state.eval(&spec.x).num();
                let tile_y = state.eval(&spec.y).num();
                // Official `effect.bool()` — runtime LVar, not compile-time token.
                let effect = lvar_bool(&state.eval(&spec.effect));
                w.spawn_unit(
                    unit_type,
                    tile_x,
                    tile_y,
                    state.eval(&spec.rotation).num(),
                    team,
                    effect,
                )
            });
            if let Some(unit_id) = spawned {
                if let Some(v) = state.vars.get_mut(spec.result) {
                    if !v.constant {
                        v.isobj = true;
                        v.numval = 0.0;
                        v.objval = LObject::Unit(unit_id);
                    }
                }
            }
        }
        Instr::ApplyStatus(spec) => {
            if !state.privileged {
                return !state.yield_flag;
            }
            if let Some(w) = world {
                let unit = state.eval(&spec.unit);
                let duration = state.eval(&spec.duration).num();
                w.apply_effect(spec.clear, &spec.effect, &unit, duration);
            }
        }
        Instr::SpawnWave(natural, x, y) => {
            if !state.privileged {
                return !state.yield_flag;
            }
            if let Some(w) = world {
                w.spawn_wave(
                    lvar_bool(&state.eval(natural)),
                    state.eval(x).num(),
                    state.eval(y).num(),
                );
            }
        }
        Instr::SetRule(spec) => {
            if !state.privileged {
                return !state.yield_flag;
            }
            if let Some(w) = world {
                let value = state.eval(&spec.value);
                let p1 = state.eval(&spec.p1);
                let p2 = state.eval(&spec.p2);
                let p3 = state.eval(&spec.p3);
                let p4 = state.eval(&spec.p4);
                w.set_rule(spec.rule, &value, &p1, &p2, &p3, &p4);
            }
        }
        Instr::Explosion(spec) => {
            if !state.privileged {
                return !state.yield_flag;
            }
            if let Some(w) = world {
                let damage = state.eval(&spec.damage).num();
                if damage < 0.0 {
                    return !state.yield_flag;
                }
                let team = lvar_team(&state.eval(&spec.team)).unwrap_or(0);
                w.logic_explosion(
                    team,
                    state.eval(&spec.x).num(),
                    state.eval(&spec.y).num(),
                    state.eval(&spec.radius).num(),
                    damage,
                    lvar_bool(&state.eval(&spec.air)),
                    lvar_bool(&state.eval(&spec.ground)),
                    lvar_bool(&state.eval(&spec.pierce)),
                );
            }
        }
        Instr::SetProp(spec) => {
            if !state.privileged {
                return !state.yield_flag;
            }
            if let Some(w) = world {
                let of = state.eval(&spec.of);
                let value = state.eval(&spec.value);
                w.set_prop(&spec.key, &of, &value);
            }
        }
        Instr::End => {
            state.counter = state.program.instructions.len();
        }
        Instr::NoOp => {}
    }
    !state.yield_flag
}

/// Java `Math.round(double)` — floor(x + 0.5), long result.
pub fn java_round(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

/// Java `Double.toString(double)` for the FormatI substitution path
/// (JAR FormatI.run offsets 160-268): integer-valued doubles take the
/// `String.valueOf(round(num))` long path, everything else goes through
/// `String.valueOf(double)`. This implements the official shortest-digit
/// decimal/scientific switching: plain notation for 1e-3 <= |v| < 1e7,
/// otherwise `d.dddE<sign><exp>` with at least one fraction digit.
pub fn java_double_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let negative = value < 0.0 || (value == 0.0 && value.is_sign_negative());
    let abs = value.abs();
    // Rust's `{:e}` emits the shortest round-trip mantissa (Ryū), the same
    // digit sequence Java picks; reformat to Java's exponent rules.
    let (mantissa, exponent) = {
        let formatted = format!("{abs:e}");
        let (mantissa, exponent) = formatted.split_once('e').unwrap();
        let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
        let digits = digits.trim_end_matches('0').to_string();
        let digits = if digits.is_empty() {
            "0".to_string()
        } else {
            digits
        };
        (digits, exponent.parse::<i32>().unwrap_or(0))
    };
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if (-3..7).contains(&exponent) {
        // Plain decimal notation.
        if exponent >= 0 {
            let point = exponent as usize + 1;
            if point >= mantissa.len() {
                out.push_str(&mantissa);
                out.push_str(&"0".repeat(point - mantissa.len()));
                out.push_str(".0");
            } else {
                out.push_str(&mantissa[..point]);
                out.push('.');
                out.push_str(&mantissa[point..]);
            }
        } else {
            out.push_str("0.");
            out.push_str(&"0".repeat((-exponent - 1) as usize));
            out.push_str(&mantissa);
        }
    } else {
        // Scientific notation: d.dddE<sign>exp.
        out.push_str(&mantissa[..1]);
        if mantissa.len() > 1 {
            out.push('.');
            out.push_str(&mantissa[1..]);
        } else {
            out.push_str(".0");
        }
        out.push('E');
        if exponent >= 0 {
            out.push('+');
        } else {
            out.push('-');
        }
        out.push_str(&exponent.abs().to_string());
    }
    out
}

/// Official `LExecutor$PrintI.toString(Object)` (JAR offsets 0-146) mapped
/// onto the port's LObject values: null -> "null", String -> itself,
/// MappableContent/Unit -> content name, Building -> block name (resolved
/// through the world when available), Enum/Team -> name, else "[object]".
pub fn printi_to_string(value: &LVar, world: Option<&WorldView>) -> String {
    match &value.objval {
        LObject::Str(s) => s.clone(),
        LObject::Unit(id) => crate::logic::ops::unit_name_from_id(*id as i16)
            .unwrap_or("[object]")
            .to_string(),
        LObject::Building(pos) => world
            .and_then(|w| w.world.tiles.get(pos))
            .and_then(|tile| crate::game::block_names::block_name_from_id(tile.block))
            .unwrap_or("[object]")
            .to_string(),
        LObject::UnitType(id) => crate::logic::ops::unit_name_from_id(*id)
            .unwrap_or("[object]")
            .to_string(),
        LObject::Team(id) => match *id {
            1 => "sharded",
            2 => "crux",
            3 => "malis",
            4 => "green",
            5 => "blue",
            6 => "neoplastic",
            other => return other.to_string(),
        }
        .to_string(),
        LObject::Null => "null".to_string(),
    }
}

/// Official `LVar.invalid(double)` (v159.7): NaN or infinite.
#[inline]
pub fn lvar_invalid(d: f64) -> bool {
    !d.is_finite()
}

/// Official `LVar.bool()` (v159.7):
/// `isobj ? objval != null : Math.abs(numval) >= 0.00001`.
pub fn lvar_bool(value: &LVar) -> bool {
    if value.isobj {
        !matches!(value.objval, LObject::Null)
    } else {
        value.numval.abs() >= 0.00001
    }
}

/// Official `LVar.team()` (v159.7): Team object -> id; numeric ->
/// `Team.all[(int)numval]` when `0 <= t < 256`; String and other objects are
/// null (never reparsed by name at runtime).
pub fn lvar_team(value: &LVar) -> Option<u8> {
    if value.isobj {
        match &value.objval {
            LObject::Team(id) => Some(*id),
            _ => None,
        }
    } else {
        let id = value.numval as i32;
        if (0..256).contains(&id) {
            Some(id as u8)
        } else {
            None
        }
    }
}

/// A `fetch` extra operand classified like the official `LVar.obj()`
/// instanceof checks (FetchI.run): UnitType filters unit queries, Block
/// filters build queries, anything else means "no filter". Numeric values
/// never filter (`obj()` is null for numeric vars in the JAR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentFilter {
    Unit(i16),
    Block(i16),
}

/// Unit query filter semantics (FetchI: only a UnitType extra filters).
fn unit_matches(filter: Option<ContentFilter>, unit_type: i16) -> bool {
    match filter {
        None | Some(ContentFilter::Block(_)) => true,
        Some(ContentFilter::Unit(id)) => id == unit_type,
    }
}

/// Build query filter semantics (FetchI: only a Block extra filters).
fn block_matches(filter: Option<ContentFilter>, block: i16) -> bool {
    match filter {
        None | Some(ContentFilter::Unit(_)) => true,
        Some(ContentFilter::Block(id)) => id == block,
    }
}

#[allow(dead_code)]
pub fn lvar_unit_type(value: &LVar) -> Option<i16> {
    if let LObject::UnitType(id) = value.objval {
        return Some(id);
    }
    if let Some(ContentFilter::Unit(id)) = lvar_content_filter(value) {
        return Some(id);
    }
    if !value.isobj {
        let id = value.numval.round() as i64;
        if (0..=i16::MAX as i64).contains(&id) {
            return Some(id as i16);
        }
    }
    None
}

/// Exact v159.7 SpawnUnitI type gate: only an object-backed UnitType is
/// accepted. Numeric IDs, names, null, and other content objects fail the
/// `type.obj() instanceof UnitType` check in the Java implementation.
pub fn lvar_unit_type_object(value: &LVar) -> Option<i16> {
    if !value.isobj {
        return None;
    }
    match value.objval {
        LObject::UnitType(id) => Some(id),
        _ => None,
    }
}

pub fn lvar_content_filter(value: &LVar) -> Option<ContentFilter> {
    match &value.objval {
        LObject::Str(name) => {
            let name = name.trim().trim_start_matches('@').trim_matches('"');
            if let Some(unit_id) = crate::network::units::parse_unit_type(name) {
                Some(ContentFilter::Unit(unit_id))
            } else {
                crate::game::block_names::block_id_from_name(name).map(ContentFilter::Block)
            }
        }
        _ => None,
    }
}

pub fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let rounded = value.round();
    if (value - rounded).abs() < 1e-9 {
        format!("{:.0}", rounded)
    } else {
        format!("{:.6}", value)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

impl ExecutorState {
    /// Runs up to `budget` instructions (or until yield). Applies side
    /// effects via `view`. Returns whether anything may have changed.
    pub fn run_tick(&mut self, view: Option<&WorldView>, budget: usize) {
        // P0-7: rejected programs never execute.
        if self.rejected {
            return;
        }
        self.yield_flag = false;
        // Runtime constants.
        if let Some(v) = self.vars.get_mut(self.program.links_var) {
            v.isobj = false;
            v.numval = self.links.len() as f64;
            v.objval = LObject::Null;
        }
        let ipt = self
            .program
            .vars
            .get(self.program.ipt_var)
            .map(LVar::num)
            .unwrap_or(8.0);
        let effective_budget = if self.rate > 0.0 {
            (self.rate / 60.0).round().max(1.0).min(ipt) as usize
        } else {
            budget
        };
        // @this points at the processor (when a world view is available).
        if let Some(v) = self.vars.get_mut(self.program.this_var) {
            v.isobj = true;
            v.objval = view
                .map(|w| LObject::Building(w.processor_pos))
                .unwrap_or(LObject::Null);
        }
        // @unit = the ubind-bound unit, or null.
        if let Some(v) = self.vars.get_mut(self.program.unit_var) {
            v.isobj = self.bound_unit.is_some();
            v.objval = self.bound_unit.map(LObject::Unit).unwrap_or(LObject::Null);
        }
        let mut ran = 0;
        while ran < effective_budget && !self.yield_flag {
            let len = self.program.instructions.len();
            if len == 0 {
                break;
            }
            if self.counter >= len {
                self.counter = 0;
            }
            // Clone the instruction so run_instruction can borrow `self`
            // mutably without a conflict with the program borrow.
            let instr = self.program.instructions[self.counter].clone();
            // Advance the counter before running (official runOnce).
            self.counter += 1;
            let keep = run_instruction(self, view, &instr);
            if !keep {
                break;
            }
            ran += 1;
        }
    }
}

/// Mirrors the official `exec.unit.setconst(...)` used by UnitBindI: the
/// `@unit` variable updates IMMEDIATELY (mid-tick), so a
/// `sensor x @unit ...` following the bind in the same tick observes the
/// new unit. (run_tick also re-syncs @unit from `bound_unit` every tick.)
/// `ucontrol unbind` no longer touches @unit: 158.1's unbind case only
/// calls `unit.resetController()` (LExecutor.java:366-369), leaving the
/// executor bound to the same unit.
pub(crate) fn sync_bound_unit_var(state: &mut ExecutorState) {
    if let Some(v) = state.vars.get_mut(state.program.unit_var) {
        v.isobj = state.bound_unit.is_some();
        v.numval = 0.0;
        v.objval = state.bound_unit.map(LObject::Unit).unwrap_or(LObject::Null);
    }
}
