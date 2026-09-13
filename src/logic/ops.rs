//! Logic op/condition semantics and content registries (no world access).

use super::executor::LVar;

/// Arithmetic/logic operators (LogicOp.java).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Idiv,
    Mod,
    Emod,
    Pow,
    Equal,
    NotEqual,
    Land,
    LessThan,
    LessThanEq,
    GreaterThan,
    GreaterThanEq,
    StrictEqual,
    Shl,
    Shr,
    Ushr,
    Or,
    And,
    Xor,
    Not,
    Max,
    Min,
    Angle,
    AngleDiff,
    Len,
    Noise,
    Abs,
    Sign,
    Log,
    Logn,
    Log10,
    Floor,
    Ceil,
    Round,
    Sqrt,
    Rand,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
}

impl Op {
    pub fn unary(&self) -> bool {
        matches!(
            self,
            Op::Not
                | Op::Abs
                | Op::Sign
                | Op::Log
                | Op::Log10
                | Op::Floor
                | Op::Ceil
                | Op::Round
                | Op::Sqrt
                | Op::Rand
                | Op::Sin
                | Op::Cos
                | Op::Tan
                | Op::Asin
                | Op::Acos
                | Op::Atan
        )
    }

    pub fn apply(&self, a: f64, b: f64) -> f64 {
        use Op::*;
        match self {
            Add => a + b,
            Sub => a - b,
            Mul => a * b,
            Div => a / b,
            Idiv => (a / b).floor(),
            Mod => a % b,
            Emod => ((a % b) + b) % b,
            Pow => a.powf(b),
            Logn => a.ln() / b.ln(),
            Equal => ((a - b).abs() < 0.000001) as i64 as f64,
            NotEqual => ((a - b).abs() >= 0.000001) as i64 as f64,
            Land => (a != 0.0 && b != 0.0) as i64 as f64,
            LessThan => (a < b) as i64 as f64,
            LessThanEq => (a <= b) as i64 as f64,
            GreaterThan => (a > b) as i64 as f64,
            GreaterThanEq => (a >= b) as i64 as f64,
            StrictEqual => 0.0, // never used (OpI handles it specially)
            Shl => ((a as i64) << (b as i64)) as f64,
            Shr => ((a as i64) >> (b as i64)) as f64,
            Ushr => (((a as u64) >> (b as u64)) as i64) as f64,
            Or => ((a as i64) | (b as i64)) as f64,
            And => ((a as i64) & (b as i64)) as f64,
            Xor => ((a as i64) ^ (b as i64)) as f64,
            Not => !(a as i64) as f64,
            Max => a.max(b),
            Min => a.min(b),
            Angle => {
                let (x, y) = (a as f32, b as f32);
                let deg = y.atan2(x).to_degrees();
                (if deg < 0.0 { deg + 360.0 } else { deg }) as f64
            }
            AngleDiff => {
                let mut diff = (a - b).abs() % 360.0;
                if diff > 180.0 {
                    diff = 360.0 - diff;
                }
                diff
            }
            Len => ((a * a + b * b) as f32).sqrt() as f64,
            Noise => simplex_raw2d(a, b),
            Abs | Sign | Log | Log10 | Floor | Ceil | Round | Sqrt | Rand | Sin | Cos | Tan
            | Asin | Acos | Atan => {
                let _ = (a, b);
                0.0 // unreachable for binary call
            }
        }
    }

    pub fn apply_unary(&self, a: f64) -> f64 {
        use Op::*;
        match self {
            Abs => a.abs(),
            Sign => a.signum(),
            Log => a.ln(),
            Log10 => a.log10(),
            Floor => a.floor(),
            Ceil => a.ceil(),
            Round => a.round(),
            Sqrt => a.sqrt(),
            Rand => rand::random::<f64>() * a,
            Sin => (a * std::f64::consts::PI / 180.0).sin(),
            Cos => (a * std::f64::consts::PI / 180.0).cos(),
            Tan => (a * std::f64::consts::PI / 180.0).tan(),
            Asin => a.asin() * 180.0 / std::f64::consts::PI,
            Acos => a.acos() * 180.0 / std::f64::consts::PI,
            Atan => a.atan() * 180.0 / std::f64::consts::PI,
            _ => 0.0,
        }
    }
}

fn simplex_raw2d(x: f64, y: f64) -> f64 {
    simplex_raw2d_seeded(0, x, y)
}

/// Arc `Simplex.raw2d(seed, x, y)` (audit M22).
pub(crate) fn simplex_raw2d_seeded(seed: i32, x: f64, y: f64) -> f64 {
    const F2: f64 = 0.5 * (1.7320508075688772 - 1.0);
    const G2: f64 = (3.0 - 1.7320508075688772) / 6.0;
    const GRAD3: [[i32; 3]; 12] = [
        [1, 1, 0],
        [-1, 1, 0],
        [1, -1, 0],
        [-1, -1, 0],
        [1, 0, 1],
        [-1, 0, 1],
        [1, 0, -1],
        [-1, 0, -1],
        [0, 1, 1],
        [0, -1, 1],
        [0, 1, -1],
        [0, -1, -1],
    ];
    let s = (x + y) * F2;
    let i = simplex_fastfloor(x + s);
    let j = simplex_fastfloor(y + s);
    let t = (i + j) as f64 * G2;
    let x0 = x - (i as f64 - t);
    let y0 = y - (j as f64 - t);
    let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
    let x1 = x0 - i1 as f64 + G2;
    let y1 = y0 - j1 as f64 + G2;
    let x2 = x0 - 1.0 + 2.0 * G2;
    let y2 = y0 - 1.0 + 2.0 * G2;
    let ii = i & 255;
    let jj = j & 255;
    let gi0 = (simplex_perm(seed, ii + simplex_perm(seed, jj)) % 12) as usize;
    let gi1 = (simplex_perm(seed, ii + i1 + simplex_perm(seed, jj + j1)) % 12) as usize;
    let gi2 = (simplex_perm(seed, ii + 1 + simplex_perm(seed, jj + 1)) % 12) as usize;
    let n0 = simplex_corner(GRAD3[gi0], x0, y0);
    let n1 = simplex_corner(GRAD3[gi1], x1, y1);
    let n2 = simplex_corner(GRAD3[gi2], x2, y2);
    70.0 * (n0 + n1 + n2)
}

fn simplex_corner(g: [i32; 3], x: f64, y: f64) -> f64 {
    let mut t = 0.5 - x * x - y * y;
    if t < 0.0 {
        0.0
    } else {
        t *= t;
        t * t * (f64::from(g[0]) * x + f64::from(g[1]) * y)
    }
}

fn simplex_perm(seed: i32, x: i32) -> i32 {
    let mut x = (x & 255).wrapping_mul(0x45d9f3b);
    x = (simplex_ushr(x, 16) ^ x).wrapping_mul(0x45d9f3bi32.wrapping_add(seed));
    x = simplex_ushr(x, 16) ^ x;
    x & 0xff
}

fn simplex_ushr(x: i32, n: u32) -> i32 {
    ((x as u32) >> n) as i32
}

fn simplex_fastfloor(x: f64) -> i32 {
    if x > 0.0 {
        x as i32
    } else {
        x as i32 - 1
    }
}

#[cfg(test)]
mod simplex_tests {
    use super::simplex_raw2d_seeded;

    #[test]
    fn raw2d_is_deterministic_and_bounded() {
        let a = simplex_raw2d_seeded(0, 12.0, 34.0);
        let b = simplex_raw2d_seeded(0, 12.0, 34.0);
        assert_eq!(a, b);
        assert!(a.abs() <= 1.0001);
        assert_ne!(
            simplex_raw2d_seeded(0, 0.5, 0.25),
            simplex_raw2d_seeded(0, 1.5, 0.25)
        );
    }
}

/// Jump conditions (ConditionOp.java).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    Equal,
    NotEqual,
    LessThan,
    LessThanEq,
    GreaterThan,
    GreaterThanEq,
    StrictEqual,
    Always,
}

impl Cond {
    pub fn test(&self, a: &LVar, b: &LVar) -> bool {
        use Cond::*;
        match self {
            StrictEqual => {
                a.isobj == b.isobj
                    && ((a.isobj && a.objval == b.objval) || (!a.isobj && a.numval == b.numval))
            }
            _ => {
                let (an, bn) = (a.num(), b.num());
                match self {
                    Equal => (an - bn).abs() < 0.000001,
                    NotEqual => (an - bn).abs() >= 0.000001,
                    LessThan => an < bn,
                    LessThanEq => an <= bn,
                    GreaterThan => an > bn,
                    GreaterThanEq => an >= bn,
                    Always => true,
                    StrictEqual => false,
                }
            }
        }
    }
}

/// LAccess sensor items (phase 1 subset).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LAccess {
    Health,
    Team,
    Block,
    Enabled,
    TotalItems,
    TotalLiquids,
    X,
    Y,
    Size,
    Range,
    Dead,
    Time,
    Tick,
    WaveNumber,
    Second,
    Minute,
    Links,
    Ipt,
    This,
    Unit,
    Counter,
    Flag,
    Shield,
    Rotation,
    Flying,
    // ---- Remaining senseable LAccess constants (LAccess.java v159.7).
    // Physics/client-only fields resolve to documented defaults instead of
    // degrading the whole sensor instruction to NoOp (audit H16).
    FirstItem,
    TotalPower,
    ItemCapacity,
    LiquidCapacity,
    PowerCapacity,
    PowerNetStored,
    PowerNetCapacity,
    PowerNetIn,
    PowerNetOut,
    Ammo,
    AmmoCapacity,
    CurrentAmmoType,
    MemoryCapacity,
    MaxHealth,
    Heat,
    Armor,
    Efficiency,
    Progress,
    Timescale,
    VelocityX,
    VelocityY,
    ShootX,
    ShootY,
    CameraX,
    CameraY,
    CameraWidth,
    CameraHeight,
    DisplayWidth,
    DisplayHeight,
    BufferSize,
    Operations,
    Solid,
    Shooting,
    Boosting,
    MineX,
    MineY,
    Mining,
    BuildX,
    BuildY,
    PingX,
    PingY,
    PingText,
    Building,
    Breaking,
    Speed,
    Type,
    Controlled,
    Controller,
    Name,
    PayloadCount,
    PayloadType,
    TotalPayload,
    PayloadCapacity,
    MaxUnits,
    Id,
    SelectedBlock,
    SelectedRotation,
    BulletLifetime,
    BulletTime,
}

impl LAccess {
    pub fn parse(token: &str) -> Option<LAccess> {
        Some(match token.trim_start_matches('@') {
            "health" => LAccess::Health,
            "team" => LAccess::Team,
            "block" => LAccess::Block,
            "enabled" => LAccess::Enabled,
            "totalItems" => LAccess::TotalItems,
            "totalLiquids" => LAccess::TotalLiquids,
            "x" => LAccess::X,
            "y" => LAccess::Y,
            "size" => LAccess::Size,
            "range" => LAccess::Range,
            "dead" => LAccess::Dead,
            "time" => LAccess::Time,
            "tick" => LAccess::Tick,
            "waveNumber" => LAccess::WaveNumber,
            "second" => LAccess::Second,
            "minute" => LAccess::Minute,
            "links" => LAccess::Links,
            "ipt" => LAccess::Ipt,
            "this" => LAccess::This,
            "unit" => LAccess::Unit,
            "counter" => LAccess::Counter,
            "flag" => LAccess::Flag,
            "shield" => LAccess::Shield,
            "rotation" => LAccess::Rotation,
            "flying" => LAccess::Flying,
            "firstItem" => LAccess::FirstItem,
            "totalPower" => LAccess::TotalPower,
            "itemCapacity" => LAccess::ItemCapacity,
            "liquidCapacity" => LAccess::LiquidCapacity,
            "powerCapacity" => LAccess::PowerCapacity,
            "powerNetStored" => LAccess::PowerNetStored,
            "powerNetCapacity" => LAccess::PowerNetCapacity,
            "powerNetIn" => LAccess::PowerNetIn,
            "powerNetOut" => LAccess::PowerNetOut,
            "ammo" => LAccess::Ammo,
            "ammoCapacity" => LAccess::AmmoCapacity,
            "currentAmmoType" => LAccess::CurrentAmmoType,
            "memoryCapacity" => LAccess::MemoryCapacity,
            "maxHealth" => LAccess::MaxHealth,
            "heat" => LAccess::Heat,
            "armor" => LAccess::Armor,
            "efficiency" => LAccess::Efficiency,
            "progress" => LAccess::Progress,
            "timescale" => LAccess::Timescale,
            "velocityX" => LAccess::VelocityX,
            "velocityY" => LAccess::VelocityY,
            "shootX" => LAccess::ShootX,
            "shootY" => LAccess::ShootY,
            "cameraX" => LAccess::CameraX,
            "cameraY" => LAccess::CameraY,
            "cameraWidth" => LAccess::CameraWidth,
            "cameraHeight" => LAccess::CameraHeight,
            "displayWidth" => LAccess::DisplayWidth,
            "displayHeight" => LAccess::DisplayHeight,
            "bufferSize" => LAccess::BufferSize,
            "operations" => LAccess::Operations,
            "solid" => LAccess::Solid,
            "shooting" => LAccess::Shooting,
            "boosting" => LAccess::Boosting,
            "mineX" => LAccess::MineX,
            "mineY" => LAccess::MineY,
            "mining" => LAccess::Mining,
            "buildX" => LAccess::BuildX,
            "buildY" => LAccess::BuildY,
            "pingX" => LAccess::PingX,
            "pingY" => LAccess::PingY,
            "pingText" => LAccess::PingText,
            "building" => LAccess::Building,
            "breaking" => LAccess::Breaking,
            "speed" => LAccess::Speed,
            "type" => LAccess::Type,
            "controlled" => LAccess::Controlled,
            "controller" => LAccess::Controller,
            "name" => LAccess::Name,
            "payloadCount" => LAccess::PayloadCount,
            "payloadType" => LAccess::PayloadType,
            "totalPayload" => LAccess::TotalPayload,
            "payloadCapacity" => LAccess::PayloadCapacity,
            "maxUnits" => LAccess::MaxUnits,
            "id" => LAccess::Id,
            "selectedBlock" => LAccess::SelectedBlock,
            "selectedRotation" => LAccess::SelectedRotation,
            "bulletLifetime" => LAccess::BulletLifetime,
            "bulletTime" => LAccess::BulletTime,
            _ => return None,
        })
    }
}

/// `lookup` content kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupKind {
    Block,
    Unit,
    Item,
    Liquid,
}

/// `fetch <type> <result> <team> <index> [extra]` — official FetchType
/// (mindustry.logic.FetchType, desktop 158.1): unit, unitCount, player,
/// playerCount, core, coreCount, build, buildCount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchKind {
    Unit,
    UnitCount,
    Player,
    PlayerCount,
    Core,
    CoreCount,
    Build,
    BuildCount,
}

impl FetchKind {
    /// JAR 158.1 `FetchType.valueOf(String)` — the wire tokens written by
    /// LogicIO.write (`fetch unit result @sharded 0`).
    pub fn parse(token: &str) -> Option<FetchKind> {
        match token.trim().to_ascii_lowercase().as_str() {
            "unit" => Some(FetchKind::Unit),
            "unitcount" => Some(FetchKind::UnitCount),
            "player" => Some(FetchKind::Player),
            "playercount" => Some(FetchKind::PlayerCount),
            "core" => Some(FetchKind::Core),
            "corecount" => Some(FetchKind::CoreCount),
            "build" => Some(FetchKind::Build),
            "buildcount" => Some(FetchKind::BuildCount),
            _ => None,
        }
    }
}

/// `getblock`/`setblock` layer operand — official `mindustry.logic.TileLayer`
/// (desktop 158.1): `all` = [floor, ore, block, building], `settable` =
/// [floor, ore, block] (building is never settable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileLayer {
    Floor,
    Ore,
    Block,
    Building,
}

impl TileLayer {
    pub fn parse(token: &str) -> Option<TileLayer> {
        match token.trim().to_ascii_lowercase().as_str() {
            "floor" => Some(TileLayer::Floor),
            "ore" => Some(TileLayer::Ore),
            "block" => Some(TileLayer::Block),
            "building" => Some(TileLayer::Building),
            _ => None,
        }
    }
}

/// Official team content ids (Team.all, desktop 158.1): derelict 0, sharded
/// 1, crux 2, malis 3, green 4, blue 5, neoplastic 6.
pub fn team_id_from_token(token: &str) -> Option<f64> {
    match token.trim().to_ascii_lowercase().as_str() {
        "@derelict" | "derelict" => Some(0.0),
        "@sharded" | "sharded" => Some(1.0),
        "@crux" | "crux" => Some(2.0),
        "@malis" | "malis" => Some(3.0),
        "@green" | "green" => Some(4.0),
        "@blue" | "blue" => Some(5.0),
        "@neoplastic" | "neoplastic" => Some(6.0),
        _ => None,
    }
}

impl LookupKind {
    pub fn parse(token: &str) -> Option<LookupKind> {
        Some(match token {
            "block" => LookupKind::Block,
            "unit" => LookupKind::Unit,
            "item" => LookupKind::Item,
            "liquid" => LookupKind::Liquid,
            _ => return None,
        })
    }
}

/// Item content id from a name token (official Items registry order:
/// copper 0, lead 1, metaglass 2, graphite 3, sand 4, coal 5, titanium 6,
/// thorium 7, scrap 8, silicon 9, plastanium 10, phase-fabric 11,
/// surge-alloy 12, spore-pod 13, blast-compound 14, pyratite 15,
/// beryllium 16, tungsten 17, oxide 18, carbide 19, fissile-matter 20,
/// dormant-cyst 21).
pub fn item_id_from_name(name: &str) -> i16 {
    match name {
        "copper" => 0,
        "lead" => 1,
        "metaglass" => 2,
        "graphite" => 3,
        "sand" => 4,
        "coal" => 5,
        "titanium" => 6,
        "thorium" => 7,
        "scrap" => 8,
        "silicon" => 9,
        "plastanium" => 10,
        "phase-fabric" => 11,
        "surge-alloy" => 12,
        "spore-pod" => 13,
        "blast-compound" => 14,
        "pyratite" => 15,
        "beryllium" => 16,
        "tungsten" => 17,
        "oxide" => 18,
        "carbide" => 19,
        "fissile-matter" => 20,
        "dormant-cyst" => 21,
        _ => 0,
    }
}

/// Official v158.1 unit content name by id. Delegates to the single unit
/// content registry (`src/game/unit_types.rs`) so the logic domain never
/// owns a second unit table (ARCHITECTURE.md: inversion of dependencies).
pub fn unit_name_from_id(id: i16) -> Option<&'static str> {
    crate::game::unit_types::unit_name_from_id(id)
}

/// Official v158.1 item content name by id (jar dump).
pub fn item_name_from_id(id: i16) -> Option<&'static str> {
    Some(match id {
        0 => "copper",
        1 => "lead",
        2 => "metaglass",
        3 => "graphite",
        4 => "sand",
        5 => "coal",
        6 => "titanium",
        7 => "thorium",
        8 => "scrap",
        9 => "silicon",
        10 => "plastanium",
        11 => "phase-fabric",
        12 => "surge-alloy",
        13 => "spore-pod",
        14 => "blast-compound",
        15 => "pyratite",
        16 => "beryllium",
        17 => "tungsten",
        18 => "oxide",
        19 => "carbide",
        20 => "fissile-matter",
        21 => "dormant-cyst",
        _ => return None,
    })
}

/// Reverse of `liquid_name_from_id`. Unknown names return `None` (unlike
/// `item_id_from_name`, which collapses misses to copper).
pub fn liquid_id_from_name(name: &str) -> Option<i16> {
    Some(match name {
        "water" => 0,
        "slag" => 1,
        "oil" => 2,
        "cryofluid" => 3,
        "neoplasm" => 4,
        "arkycite" => 5,
        "gallium" => 6,
        "ozone" => 7,
        "hydrogen" => 8,
        "nitrogen" => 9,
        "cyanogen" => 10,
        _ => return None,
    })
}

/// Reverse of `item_name_from_id`. Unknown names return `None`.
pub fn try_item_id_from_name(name: &str) -> Option<i16> {
    let id = item_id_from_name(name);
    (item_name_from_id(id) == Some(name)).then_some(id)
}

/// Official v158.1 liquid content name by id (jar dump).
pub fn liquid_name_from_id(id: i16) -> Option<&'static str> {
    Some(match id {
        0 => "water",
        1 => "slag",
        2 => "oil",
        3 => "cryofluid",
        4 => "neoplasm",
        5 => "arkycite",
        6 => "gallium",
        7 => "ozone",
        8 => "hydrogen",
        9 => "nitrogen",
        10 => "cyanogen",
        _ => return None,
    })
}

/// Ore overlay floor id -> item id (Serpulo ores).
pub fn ore_item_id(overlay: i16) -> Option<i16> {
    Some(match overlay {
        73 => 0,  // oreCopper
        74 => 1,  // oreLead
        75 => 8,  // oreScrap
        76 => 5,  // oreCoal
        77 => 6,  // oreTitanium
        78 => 7,  // oreThorium
        79 => 16, // oreBeryllium
        80 => 17, // oreTungsten
        _ => return None,
    })
}
