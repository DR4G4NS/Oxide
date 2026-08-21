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
    // Deterministic pseudo-noise matching Simplex.raw2d(0, x, y) closely
    // enough for automation (phase 1 approximation; exact gradient noise is
    // phase 2).
    let seed = 0u64;
    let h = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let fx = x as f32;
    let fy = y as f32;
    let xi = (fx * 31.0) as i64;
    let yi = (fy * 57.0) as i64;
    let mix = |ix: i64, iy: i64| -> f32 {
        let mut z = h
            .wrapping_add(ix as u64)
            .wrapping_add((iy as u64).wrapping_mul(0x1000_0000_01B3));
        z ^= z >> 33;
        z = z.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        z ^= z >> 33;
        ((z & 0xFFFF) as f32 / 0xFFFF as f32) * 2.0 - 1.0
    };
    let dx = fx * 31.0 - xi as f32;
    let dy = fy * 57.0 - yi as f32;
    let mut v = mix(xi, yi) * (1.0 - dx) * (1.0 - dy)
        + mix(xi + 1, yi) * dx * (1.0 - dy)
        + mix(xi, yi + 1) * (1.0 - dx) * dy
        + mix(xi + 1, yi + 1) * dx * dy;
    v = v.clamp(-1.0, 1.0);
    v as f64
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
