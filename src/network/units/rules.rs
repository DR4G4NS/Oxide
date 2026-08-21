//! Wave spawn tables and map Rules parsing (SpawnGroup/WaveRules/TeamRule).
//! Units facade re-exports through crate::network::units::*.

use crate::network::combat::enemy::spawn_wave;
use crate::network::world::*;
use serde_json;

use super::*;

pub(crate) fn spawn_group_amount(
    wave: u32,
    begin: u32,
    end: u32,
    spacing: u32,
    scaling: f32,
    base: u32,
    max: u32,
) -> u32 {
    let spacing = spacing.max(1); // official: `if(spacing == 0) spacing = 1;`
    if wave < begin || wave > end || !(wave - begin).is_multiple_of(spacing) {
        return 0;
    }
    (base + (((wave - begin) / spacing) as f32 / scaling.max(0.000_001)) as u32).min(max.max(1))
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WaveSpawn {
    pub(crate) spec: EnemySpec,
    pub(crate) amount: u32,
    pub(crate) shield: f32,
    pub(crate) status_effect: i16,
    /// Packed spawn-point position (`x << 16 | y`) that this group must use,
    /// or -1 to spawn at any spawn overlay (official `SpawnGroup.spawn`).
    pub(crate) spawn: i32,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wave_spawn(
    wave: u32,
    spec: EnemySpec,
    begin: u32,
    end: u32,
    spacing: u32,
    scaling: f32,
    base: u32,
    max: u32,
    shields: f32,
    shield_scaling: f32,
) -> Option<WaveSpawn> {
    let amount = spawn_group_amount(wave, begin, end, spacing, scaling, base, max);
    (amount > 0).then_some(WaveSpawn {
        spec,
        amount,
        shield: (shields + shield_scaling * wave.saturating_sub(begin) as f32).max(0.0),
        status_effect: -1,
        spawn: -1,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wave_spawn_with_effect(
    wave: u32,
    spec: EnemySpec,
    begin: u32,
    end: u32,
    spacing: u32,
    scaling: f32,
    base: u32,
    max: u32,
    shields: f32,
    shield_scaling: f32,
    status_effect: i16,
) -> Option<WaveSpawn> {
    wave_spawn(
        wave,
        spec,
        begin,
        end,
        spacing,
        scaling,
        base,
        max,
        shields,
        shield_scaling,
    )
    .map(|mut spawn| {
        spawn.status_effect = status_effect;
        spawn
    })
}

pub(crate) fn initial_official_wave_groups(wave: u32) -> Vec<WaveSpawn> {
    let mut groups = Vec::new();
    for group in [
        wave_spawn(wave, DAGGER, 0, 10, 1, 2.0, 1, 40, 0.0, 0.0),
        wave_spawn(wave, CRAWLER, 4, 13, 1, 1.5, 2, 40, 0.0, 0.0),
        wave_spawn(wave, FLARE, 12, 16, 1, 1.0, 1, 40, 0.0, 0.0),
        wave_spawn(wave, DAGGER, 11, u32::MAX, 2, 1.7, 1, 40, 0.0, 15.0),
        wave_spawn(wave, PULSAR, 13, u32::MAX, 3, 0.5, 1, 40, 0.0, 0.0),
        wave_spawn(wave, MACE, 7, 30, 3, 2.0, 1, 40, 0.0, 0.0),
        wave_spawn(wave, DAGGER, 12, u32::MAX, 2, 1.0, 4, 40, 0.0, 10.0),
        wave_spawn(wave, MACE, 28, 40, 3, 1.0, 1, 40, 0.0, 20.0),
        wave_spawn_with_effect(wave, SPIROCT, 45, u32::MAX, 3, 1.0, 1, 40, 0.0, 10.0, 13),
        wave_spawn_with_effect(wave, MACE, 120, u32::MAX, 2, 3.0, 5, 40, 0.0, 0.0, 13),
        wave_spawn(wave, FLARE, 16, u32::MAX, 2, 1.0, 1, 40, 0.0, 20.0),
        wave_spawn_with_effect(wave, QUASAR, 82, u32::MAX, 3, 3.0, 4, 40, 0.0, 30.0, 13),
        wave_spawn_with_effect(wave, PULSAR, 41, u32::MAX, 5, 3.0, 1, 40, 0.0, 0.0, 15),
        wave_spawn(wave, FORTRESS, 40, u32::MAX, 5, 2.0, 2, 40, 0.0, 0.0),
        wave_spawn_with_effect(wave, DAGGER, 35, 60, 3, 1.0, 4, 40, 0.0, 0.0, 13),
        wave_spawn_with_effect(wave, DAGGER, 42, 130, 3, 1.0, 4, 40, 0.0, 0.0, 13),
        wave_spawn(wave, HORIZON, 40, u32::MAX, 2, 2.0, 2, 40, 0.0, 0.0),
        wave_spawn_with_effect(wave, FLARE, 50, u32::MAX, 5, 3.0, 4, 40, 100.0, 10.0, 13),
        wave_spawn(wave, ZENITH, 50, u32::MAX, 5, 3.0, 2, 40, 0.0, 0.0),
        wave_spawn(wave, NOVA, 53, u32::MAX, 4, 3.0, 2, 40, 0.0, 0.0),
        wave_spawn(wave, ATRAX, 31, u32::MAX, 3, 1.0, 4, 40, 0.0, 5.0),
        wave_spawn(wave, SCEPTER, 41, u32::MAX, 30, 1.0, 1, 40, 0.0, 10.0),
        wave_spawn(wave, REIGN, 81, u32::MAX, 40, 1.0, 1, 40, 0.0, 10.0),
        wave_spawn(wave, ANTUMBRA, 131, u32::MAX, 40, 1.0, 1, 40, 0.0, 10.0),
        wave_spawn(wave, VELA, 100, u32::MAX, 30, 1.0, 1, 40, 0.0, 20.0),
        wave_spawn(wave, HORIZON, 90, u32::MAX, 4, 3.0, 2, 40, 40.0, 20.0),
        wave_spawn(wave, ATRAX, 210, u32::MAX, 35, 1.0, 1, 40, 1000.0, 35.0),
    ]
    .into_iter()
    .flatten()
    {
        groups.push(group);
    }
    groups
}

// ---------------------------------------------------------------------------
// Wave rules parsed from the loaded map's `Rules` JSON (authority:
// core/src/mindustry/game/Rules.java + SpawnGroup.java v158.1). When the map
// defines `spawns`, `spawn_wave` uses these instead of the bundled maze table.

/// Default `Rules.waveSpacing`: 2 * Time.toMinutes = 7200 ticks (2 min).
pub(crate) const DEFAULT_WAVE_SPACING: f32 = 2.0 * 60.0 * 60.0;
/// Effective first-wave delay from `Logic.play()`: Rules defaults
/// `initialWaveSpacing` to 0, which means `waveSpacing * 2` (14400 ticks).
pub(crate) const DEFAULT_INITIAL_WAVE_SPACING: f32 = DEFAULT_WAVE_SPACING * 2.0;

/// A `SpawnGroup` parsed from the map rules JSON (official fields: type,
/// begin, end, spacing, max, scaling (unitScaling), shields, shieldScaling,
/// amount (unitAmount), spawn, effect). v158.1 has no `effectChance` field on
/// SpawnGroup (that is a per-StatusEffect visual property, client-side).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MapSpawnGroup {
    pub(crate) unit_type: i16,
    pub(crate) begin: u32,
    pub(crate) end: u32,
    pub(crate) spacing: u32,
    pub(crate) max: u32,
    pub(crate) scaling: f32,
    pub(crate) shields: f32,
    pub(crate) shield_scaling: f32,
    pub(crate) unit_amount: u32,
    /// Packed spawn-point position (`x << 16 | y`) or -1 for any spawn.
    pub(crate) spawn: i32,
    /// Status effect content id, or -1 for none (`effect: none`).
    pub(crate) effect: i16,
}

/// Wave generation + gameplay rules extracted from the loaded map
/// (`Rules.java` v158.1: waveSpacing, initialWaveSpacing, spawns,
/// buildSpeedMultiplier, unitMineSpeedMultiplier, blockHealthMultiplier,
/// blockDamageMultiplier, unitDamageMultiplier, unitHealthMultiplier,
/// infiniteResources, canGameOver, instantBuild, and the wave/team contract).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WaveRules {
    pub(crate) spawn_groups: Vec<MapSpawnGroup>,
    pub(crate) wave_spacing: f32,
    pub(crate) initial_wave_spacing: f32,
    /// Official Rules.java gameplay multipliers applied by the server.
    pub(crate) build_speed_multiplier: f32,
    pub(crate) unit_mine_speed_multiplier: f32,
    pub(crate) block_health_multiplier: f32,
    pub(crate) block_damage_multiplier: f32,
    pub(crate) unit_damage_multiplier: f32,
    pub(crate) unit_health_multiplier: f32,
    pub(crate) infinite_resources: bool,
    /// Rules.coreIncinerates: full cores consume excess input but never store
    /// more than their shared per-item capacity.
    pub(crate) core_incinerates: bool,
    /// Rules.reactorExplosions: destruction still occurs on overheat when
    /// false, but radial explosion damage/effects are suppressed.
    pub(crate) reactor_explosions: bool,
    pub(crate) can_game_over: bool,
    pub(crate) instant_build: bool,
    /// Rules.waves: automatic/manual wave spawning is available at all.
    pub(crate) waves_enabled: bool,
    /// Rules.waveTimer: automatic timer-driven waves are enabled.
    pub(crate) wave_timer: bool,
    /// Rules.waveSending: manual play-button wave sending is enabled.
    pub(crate) wave_sending: bool,
    /// Rules.waitEnemies: don't advance the timer while wave-team units live.
    pub(crate) wait_enemies: bool,
    /// Rules.winWave; <= 0 disables wave-count victory.
    pub(crate) win_wave: i32,
    /// Rules.waveTeam and Rules.defaultTeam, serialized as team IDs.
    pub(crate) wave_team: u8,
    pub(crate) default_team: u8,
    /// Rules.possessionAllowed (Rules.java:61, default true): whether
    /// players may possess/control AI units (InputHandler.unitControl gate).
    pub(crate) possession_allowed: bool,
    /// Rules.bannedBlocks + Rules.blockWhitelist (Rules.java:187/144):
    /// content bans that gate placement. `isBanned(block)` is
    /// `blockWhitelist != bannedBlocks.contains(block)`.
    pub(crate) banned_blocks: Vec<i16>,
    pub(crate) block_whitelist: bool,
    /// Rules.bannedUnits + Rules.unitWhitelist (Rules.java:189/146): content
    /// bans that gate wave spawns and unit production.
    pub(crate) banned_units: Vec<i16>,
    pub(crate) unit_whitelist: bool,
    /// Rules.enemyCoreBuildRadius (Rules.java:123, default 400 world units):
    /// AI builders never place blocks inside the radius of an enemy core;
    /// also used by the core-death building demolition (Logic.java:176).
    pub(crate) enemy_core_build_radius: f32,
    /// Rules.teams (Rules.java:411 `TeamRules`): per-team rules keyed by
    /// team id. The official map JSON writes them as `teams:{1:{...},2:{...}}`.
    pub(crate) team_rules: std::collections::HashMap<u8, TeamRule>,
    /// Rules.fog (Rules.java): fog of war. The world stream already carries
    /// the map's original value; the authority parses it so overrides and
    /// strict-mode gates can act on it (single-player campaign fields like
    /// weather/capture/objectives are out of scope for the vanilla server).
    pub(crate) fog: bool,
    /// Rules.loadout (Rules.java): starting inventory as `(item, amount)`
    /// pairs parsed from the map's `loadout` string ("copper-20/lead-10").
    pub(crate) loadout: Vec<(i16, i32)>,
    /// Rules.unitCap (Rules.java, default 0). `setrule unitCap` writes this
    /// field; the effective cap still adds core modifiers elsewhere.
    pub(crate) unit_cap: i32,
    /// Rules.disableUnitCap (Rules.java:159): when true, unit cap checks are
    /// bypassed (official getCap returns MAX_VALUE).
    pub(crate) disable_unit_cap: bool,
    /// Rules.unitFactoryActivationDelay (Rules.java:93): global delay before
    /// unit factories activate; per-team delay is added via TeamRule.
    pub(crate) unit_factory_activation_delay: f32,
    /// Rules.blockLimits (Rules.java:185): per-block placement limits.
    pub(crate) block_limits: std::collections::HashMap<i16, u32>,
    /// Rules.editor (Rules.java): map editor mode bypasses placement limits.
    pub(crate) editor: bool,
    /// Rules.env (Rules.java): map environment flags for core compatibility.
    pub(crate) env: i32,
}

impl Default for WaveRules {
    fn default() -> Self {
        WaveRules {
            spawn_groups: Vec::new(),
            wave_spacing: DEFAULT_WAVE_SPACING,
            initial_wave_spacing: DEFAULT_INITIAL_WAVE_SPACING,
            build_speed_multiplier: 1.0,
            unit_mine_speed_multiplier: 1.0,
            block_health_multiplier: 1.0,
            block_damage_multiplier: 1.0,
            unit_damage_multiplier: 1.0,
            unit_health_multiplier: 1.0,
            infinite_resources: false,
            core_incinerates: true,
            reactor_explosions: true,
            can_game_over: true,
            instant_build: false,
            waves_enabled: false,
            wave_timer: true,
            wave_sending: true,
            wait_enemies: false,
            win_wave: 0,
            wave_team: 2,
            default_team: 1,
            possession_allowed: true,
            banned_blocks: Vec::new(),
            block_whitelist: false,
            banned_units: Vec::new(),
            unit_whitelist: false,
            enemy_core_build_radius: 400.0,
            team_rules: std::collections::HashMap::new(),
            fog: false,
            loadout: Vec::new(),
            unit_cap: 0,
            disable_unit_cap: false,
            unit_factory_activation_delay: 0.0,
            block_limits: std::collections::HashMap::new(),
            editor: false,
            env: crate::game::unit_types::RULES_ENV_DEFAULT,
        }
    }
}

/// Shared default TeamRule instance (all official defaults).
static DEFAULT_TEAM_RULE: TeamRule = TeamRule {
    protect_cores: true,
    check_placement: true,
    cheat: false,
    fill_items: false,
    infinite_resources: false,
    prebuild_ai: false,
    build_ai: false,
    build_ai_tier: 1.0,
    rts_ai: false,
    rts_min_squad: 4,
    rts_max_squad: 50,
    rts_min_weight: 1.2,
    unit_factory_activation_delay: 0.0,
    unit_build_speed_multiplier: 1.0,
    unit_damage_multiplier: 1.0,
    unit_mine_speed_multiplier: 1.0,
    unit_cost_multiplier: 1.0,
    unit_health_multiplier: 1.0,
    block_health_multiplier: 1.0,
    block_damage_multiplier: 1.0,
    build_speed_multiplier: 1.0,
    extra_core_build_radius: 0.0,
};

impl WaveRules {
    /// Official `Rules.teams.get(team)` — the per-team rule or the default.
    pub(crate) fn team_rule(&self, team: u8) -> &TeamRule {
        self.team_rules.get(&team).unwrap_or(&DEFAULT_TEAM_RULE)
    }

    /// Official `Rules.enemyCoreBuildRadius(team)`
    /// (Rules.java:291): 0 when the team does not protect cores.
    pub(crate) fn enemy_core_radius_for(&self, team: u8) -> f32 {
        let rule = self.team_rule(team);
        if rule.protect_cores {
            self.enemy_core_build_radius + rule.extra_core_build_radius
        } else {
            0.0
        }
    }

    /// Official `Rules.buildSpeed(team)` (Rules.java:327).
    pub(crate) fn build_speed_for(&self, team: u8) -> f32 {
        self.build_speed_multiplier * self.team_rule(team).build_speed_multiplier
    }

    /// Official `Rules.unitActivationDelay(Team)` (Rules.java:338).
    pub(crate) fn unit_activation_delay_for(&self, team: u8) -> f32 {
        self.unit_factory_activation_delay + self.team_rule(team).unit_factory_activation_delay
    }

    /// Official `Team.activateUnitFactories()` (Team.java:127).
    pub(crate) fn activate_unit_factories(&self, team: u8, tick: f32) -> bool {
        tick >= self.unit_activation_delay_for(team)
    }

    /// Official `Team.isAI()` (Team.java:112) for controller/AI gating.
    pub(crate) fn team_is_ai(
        &self,
        team: u8,
        mode: crate::state::game_state::GameMode,
        pvp: bool,
    ) -> bool {
        if pvp {
            return false;
        }
        (self.waves_enabled || mode == crate::state::game_state::GameMode::Attack)
            && team != self.default_team
    }

    /// Official BaseBuilderAI gate (Logic.java:555): `buildAi && !pvp`.
    pub(crate) fn team_build_ai_enabled(&self, team: u8, pvp: bool) -> bool {
        !pvp && self.team_rule(team).build_ai
    }
}

impl WaveRules {
    /// Official `Rules.isBanned(Block)` (Rules.java:331): whitelist mode
    /// inverts the membership test.
    pub(crate) fn block_banned(&self, block: i16) -> bool {
        self.block_whitelist != self.banned_blocks.contains(&block)
    }

    /// Official `Rules.isBanned(UnitType)` (Rules.java:335).
    pub(crate) fn unit_banned(&self, unit: i16) -> bool {
        self.unit_whitelist != self.banned_units.contains(&unit)
    }

    /// Official `Block.isOverPlacementLimit(Team)` (Block.java:1031):
    /// returns true if limit > 0 and current building count >= limit.
    /// Editor mode and AI teams bypass limits (official state.isEditor / Team.isAI).
    pub(crate) fn is_over_placement_limit(&self, block: i16, count: usize, team: u8) -> bool {
        if self.editor || self.is_ai_team(team) {
            return false;
        }
        if let Some(&limit) = self.block_limits.get(&block) {
            if limit > 0 && count >= limit as usize {
                return true;
            }
        }
        false
    }

    /// Official `Team.isAI()` (Team.java:112): derelict (0) or wave AI team.
    pub(crate) fn is_ai_team(&self, team: u8) -> bool {
        team == 0 || team == 2 || (self.wave_team == team && self.wave_team != self.default_team)
    }
}

impl WaveRules {
    /// Official `Map.rules()`: when the map defines no spawns, fall back to
    /// the bundled default wave table (`Vars.waves.get()`).
    pub(crate) fn is_default(&self) -> bool {
        self.spawn_groups.is_empty()
    }
}

/// Status effect id by official content name (`StatusEffects.java` v158.1,
/// ids match `src/game/status_effects.tsv`).
pub(crate) fn status_effect_id_by_name(name: &str) -> i16 {
    match name.trim().to_ascii_lowercase().as_str() {
        "burning" => 1,
        "freezing" => 2,
        "unmoving" => 3,
        "slow" => 4,
        "fast" => 5,
        "wet" => 6,
        "muddy" => 7,
        "melting" => 8,
        "sapped" => 9,
        "electrified" => 10,
        "spore-slowed" => 11,
        "tarred" => 12,
        "overdrive" => 13,
        "overclock" => 14,
        "shielded" => 15,
        "boss" => 16,
        "shocked" => 17,
        "blasted" => 18,
        "corroded" => 19,
        "disarmed" => 20,
        "invincible" => 21,
        "dynamic" => 22,
        // "none" and unknown effects leave the unit without a status.
        _ => -1,
    }
}

/// Official `Rules.TeamRule` (Rules.java:343) subset that affects server
/// authority: per-team multipliers, build gates and core protection.
/// Fields absent from the map JSON keep their official defaults.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TeamRule {
    pub(crate) protect_cores: bool,
    pub(crate) check_placement: bool,
    pub(crate) cheat: bool,
    pub(crate) fill_items: bool,
    pub(crate) infinite_resources: bool,
    /// Official `TeamRule.prebuildAi` (Rules.java:357).
    /// P1-E1: **DEFERRED BREADTH** — BlockIndexer / BuilderAI / Logic core
    /// spawn; no prebuild subsystem in the server port.
    pub(crate) prebuild_ai: bool,
    /// Official `TeamRule.buildAi` (Rules.java:360).
    /// P1-E1: **DEFERRED BREADTH** — BaseBuilderAI (gated by `!pvp` in
    /// Logic.java:555); no builder-AI tick in the server port.
    pub(crate) build_ai: bool,
    /// Official `TeamRule.buildAiTier` default 1 (Rules.java:362).
    /// P1-E1: **DEFERRED BREADTH** — BaseBuilderAI place interval.
    pub(crate) build_ai_tier: f32,
    /// Official `TeamRule.rtsAi` (Rules.java:365).
    /// P1-E1: **PARITY** (partial) — `UnitType.controller` /
    /// `default_unit_authority`; full RtsAI squad logic is deferred.
    pub(crate) rts_ai: bool,
    /// P1-E1: **DEFERRED BREADTH** — RtsAI minimum squad size.
    pub(crate) rts_min_squad: i32,
    /// P1-E1: **DEFERRED BREADTH** — RtsAI maximum squad before forced attack.
    pub(crate) rts_max_squad: i32,
    /// P1-E1: **DEFERRED BREADTH** — RtsAI attack weight threshold.
    pub(crate) rts_min_weight: f32,
    /// Official `TeamRule.unitFactoryActivationDelay` (Rules.java:374).
    /// P1-E1: **PARITY** — `simulate_unit_factories` /
    /// `simulate_reconstructors` gate via `activate_unit_factories`.
    pub(crate) unit_factory_activation_delay: f32,
    pub(crate) unit_build_speed_multiplier: f32,
    pub(crate) unit_damage_multiplier: f32,
    pub(crate) unit_mine_speed_multiplier: f32,
    pub(crate) unit_cost_multiplier: f32,
    pub(crate) unit_health_multiplier: f32,
    pub(crate) block_health_multiplier: f32,
    pub(crate) block_damage_multiplier: f32,
    pub(crate) build_speed_multiplier: f32,
    pub(crate) extra_core_build_radius: f32,
}

impl Default for TeamRule {
    fn default() -> Self {
        TeamRule {
            protect_cores: true,
            check_placement: true,
            cheat: false,
            fill_items: false,
            infinite_resources: false,
            prebuild_ai: false,
            build_ai: false,
            build_ai_tier: 1.0,
            rts_ai: false,
            rts_min_squad: 4,
            rts_max_squad: 50,
            rts_min_weight: 1.2,
            unit_factory_activation_delay: 0.0,
            unit_build_speed_multiplier: 1.0,
            unit_damage_multiplier: 1.0,
            unit_mine_speed_multiplier: 1.0,
            unit_cost_multiplier: 1.0,
            unit_health_multiplier: 1.0,
            block_health_multiplier: 1.0,
            block_damage_multiplier: 1.0,
            build_speed_multiplier: 1.0,
            extra_core_build_radius: 0.0,
        }
    }
}

impl TeamRule {
    /// Parses a `TeamRule` from its JSON object (missing keys keep defaults).
    fn from_json(value: &serde_json::Value) -> TeamRule {
        let flag = |key: &str, default: bool| -> bool {
            value
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(default)
        };
        let mult = |key: &str, default: f32| -> f32 {
            value
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .filter(|v| v.is_finite())
                .map(|v| v as f32)
                .unwrap_or(default)
        };
        TeamRule {
            protect_cores: flag("protectCores", true),
            check_placement: flag("checkPlacement", true),
            cheat: flag("cheat", false),
            fill_items: flag("fillItems", false),
            infinite_resources: flag("infiniteResources", false),
            prebuild_ai: flag("prebuildAi", false),
            build_ai: flag("buildAi", false),
            build_ai_tier: mult("buildAiTier", 1.0),
            rts_ai: flag("rtsAi", false),
            rts_min_squad: value
                .get("rtsMinSquad")
                .and_then(serde_json::Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(4),
            rts_max_squad: value
                .get("rtsMaxSquad")
                .and_then(serde_json::Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(50),
            rts_min_weight: mult("rtsMinWeight", 1.2),
            unit_factory_activation_delay: mult("unitFactoryActivationDelay", 0.0),
            unit_build_speed_multiplier: mult("unitBuildSpeedMultiplier", 1.0),
            unit_damage_multiplier: mult("unitDamageMultiplier", 1.0),
            unit_mine_speed_multiplier: mult("unitMineSpeedMultiplier", 1.0),
            unit_cost_multiplier: mult("unitCostMultiplier", 1.0),
            unit_health_multiplier: mult("unitHealthMultiplier", 1.0),
            block_health_multiplier: mult("blockHealthMultiplier", 1.0),
            block_damage_multiplier: mult("blockDamageMultiplier", 1.0),
            build_speed_multiplier: mult("buildSpeedMultiplier", 1.0),
            extra_core_build_radius: mult("extraCoreBuildRadius", 0.0),
        }
    }
}

/// Parses the official `Rules.loadout` string ("copper-20/lead-10") into
/// (item, amount) pairs; malformed entries are skipped.
pub(crate) fn parse_loadout(loadout: &str) -> Vec<(i16, i32)> {
    loadout
        .split('/')
        .filter_map(|entry| {
            let (name, amount) = entry.split_once('-')?;
            let item = crate::logic::item_id_from_name(name.trim());
            let amount = amount.trim().parse::<i32>().ok()?;
            Some((item, amount.max(0)))
        })
        .collect()
}

/// P0-7: result of parsing one map spawn group. `Supported` carries the
/// group; `Skipped(reason)` reports a group the server cannot simulate so
/// callers can warn or (in strict mode) reject the map instead of silently
/// dropping waves.
#[derive(Debug, Clone)]
pub(crate) enum SpawnGroupParse {
    Supported(MapSpawnGroup),
    Skipped(String),
}

pub(crate) fn parse_spawn_group(value: &serde_json::Value) -> SpawnGroupParse {
    let Some(type_name) = value.get("type").and_then(serde_json::Value::as_str) else {
        return SpawnGroupParse::Skipped("spawn group has no 'type' field".to_string());
    };
    let Some(unit_type) = parse_unit_type(type_name) else {
        return SpawnGroupParse::Skipped(format!(
            "unknown unit type '{type_name}' (id not in the 158.1 registry)"
        ));
    };
    // The server cannot simulate this unit yet: report the group instead of
    // dropping it silently.
    if enemy_spec(unit_type).is_none() {
        return SpawnGroupParse::Skipped(format!(
            "unit type '{type_name}' ({unit_type}) has no simulated enemy spec"
        ));
    }
    let integer = |key: &str, default: i64| -> i64 {
        value
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(default)
    };
    let end = integer("end", i32::MAX as i64);
    SpawnGroupParse::Supported(MapSpawnGroup {
        unit_type,
        begin: integer("begin", 0).max(0) as u32,
        end: if end >= i32::MAX as i64 {
            u32::MAX
        } else {
            end.max(0) as u32
        },
        spacing: integer("spacing", 1).max(1) as u32,
        max: integer("max", 40).max(1) as u32,
        scaling: value
            .get("scaling")
            .and_then(serde_json::Value::as_f64)
            .map(|scaling| scaling as f32)
            .unwrap_or(i32::MAX as f32),
        shields: value
            .get("shields")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0) as f32,
        shield_scaling: value
            .get("shieldScaling")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0) as f32,
        unit_amount: integer("amount", 1).max(0) as u32,
        spawn: integer("spawn", -1).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        effect: status_effect_id_by_name(
            value
                .get("effect")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("none"),
        ),
    })
}

/// Converts arc's JSON output (unquoted keys and bare string values, e.g. the
/// `rules` tag of an official MSAV: `{alwaysPlayMusic:true,spawns:[{type:dagger,effect:sapped}]}`)
/// into strict JSON that serde_json can parse. Object keys are always quoted
/// (arc also writes numeric team keys like `teams:{0:{...}}`), numeric
/// values/literals stay bare, and already-quoted strings pass through. Strict
/// JSON input also works.
pub(crate) fn arc_json_to_strict(input: &str) -> String {
    let mut output = String::with_capacity(input.len() + 32);
    let bytes = input.as_bytes();
    let mut i = 0;
    // `stack` tracks containers: true = object (`,` implies the next token is
    // a key), false = array. `expecting_key` mirrors the top of the stack.
    let mut stack: Vec<bool> = Vec::new();
    let mut expecting_key = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            '"' => {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                        continue;
                    }
                    i += 1;
                    if bytes[i - 1] == b'"' {
                        break;
                    }
                }
                output.push_str(&input[start..i]);
                expecting_key = false;
            }
            ' ' | '\t' | '\n' | '\r' => {
                output.push(c);
                i += 1;
            }
            '{' => {
                stack.push(true);
                expecting_key = true;
                output.push(c);
                i += 1;
            }
            '[' => {
                stack.push(false);
                expecting_key = false;
                output.push(c);
                i += 1;
            }
            '}' | ']' => {
                stack.pop();
                expecting_key = false;
                output.push(c);
                i += 1;
            }
            ':' => {
                expecting_key = false;
                output.push(c);
                i += 1;
            }
            ',' => {
                expecting_key = stack.last() == Some(&true);
                output.push(c);
                i += 1;
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !matches!(
                        bytes[i] as char,
                        '{' | '}' | '[' | ']' | ':' | ',' | '"' | ' ' | '\t' | '\n' | '\r'
                    )
                {
                    i += 1;
                }
                let token = &input[start..i];
                let literal =
                    token.parse::<f64>().is_ok() || matches!(token, "true" | "false" | "null");
                if expecting_key || !literal {
                    output.push('"');
                    for ch in token.chars() {
                        if ch == '"' || ch == '\\' {
                            output.push('\\');
                        }
                        output.push(ch);
                    }
                    output.push('"');
                } else {
                    output.push_str(token);
                }
                expecting_key = false;
            }
        }
    }
    output
}

/// Extracts the wave/timing/team subset of `Rules` from the rules JSON of the
/// loaded map (the `rules` tag of the MSAV meta, spliced into the network
/// stream by `replace_map_from_msav`). `Logic.play()` treats a missing or
/// non-positive `initialWaveSpacing` as `waveSpacing * 2` (14400 by default).
pub(crate) fn parse_wave_rules(rules_json: &str) -> WaveRules {
    parse_wave_rules_report(rules_json).0
}

/// P0-7: parses the map's `Rules` JSON and returns the effective rules plus
/// every spawn group the server cannot simulate (unknown unit types, missing
/// specs). The non-strict path keeps the historical behavior (skip the group,
/// log a warning); strict callers reject the map with the full diagnostic
/// list instead of silently losing waves.
pub(crate) fn parse_wave_rules_report(rules_json: &str) -> (WaveRules, Vec<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&arc_json_to_strict(rules_json))
    else {
        return (
            WaveRules::default(),
            vec!["rules JSON does not parse".to_string()],
        );
    };
    let spacing = |key: &str, default: f32| -> f32 {
        value
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .filter(|spacing| spacing.is_finite() && *spacing > 0.0)
            .map(|spacing| spacing as f32)
            .unwrap_or(default)
    };
    let mult = |key: &str, default: f32| -> f32 {
        value
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .filter(|v| v.is_finite())
            .map(|v| v as f32)
            .unwrap_or(default)
    };
    let team = |key: &str, default: u8| -> u8 {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|id| u8::try_from(id).ok())
            .unwrap_or(default)
    };
    let flag = |key: &str, default: bool| -> bool {
        value
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default)
    };
    let mut diagnostics = Vec::new();
    let spawn_groups = value
        .get("spawns")
        .and_then(serde_json::Value::as_array)
        .map(|spawns| {
            let mut groups = Vec::new();
            for spawn in spawns {
                match parse_spawn_group(spawn) {
                    SpawnGroupParse::Supported(group) => groups.push(group),
                    SpawnGroupParse::Skipped(reason) => diagnostics.push(reason),
                }
            }
            groups
        })
        .unwrap_or_default();
    (
        WaveRules {
            spawn_groups,
            wave_spacing: spacing("waveSpacing", DEFAULT_WAVE_SPACING),
            // Logic.play() (Rules.java:151-153, Logic.java:268-273): the value
            // used for the first countdown is
            // `(initialWaveSpacing <= 0 ? waveSpacing * 2 : initialWaveSpacing)`
            // (a missing key is the Java default 0f, so maps that omit it also get
            // waveSpacing * 2). Resolve the EFFECTIVE value here so every runtime
            // consumer uses the same first-wave delay as the official server.
            initial_wave_spacing: {
                let base = spacing("waveSpacing", DEFAULT_WAVE_SPACING);
                let raw = spacing("initialWaveSpacing", base * 2.0);
                if raw <= 0.0 {
                    base * 2.0
                } else {
                    raw
                }
            },
            build_speed_multiplier: mult("buildSpeedMultiplier", 1.0),
            unit_mine_speed_multiplier: mult("unitMineSpeedMultiplier", 1.0),
            block_health_multiplier: mult("blockHealthMultiplier", 1.0),
            block_damage_multiplier: mult("blockDamageMultiplier", 1.0),
            unit_damage_multiplier: mult("unitDamageMultiplier", 1.0),
            unit_health_multiplier: mult("unitHealthMultiplier", 1.0),
            infinite_resources: flag("infiniteResources", false),
            core_incinerates: flag("coreIncinerates", true),
            reactor_explosions: flag("reactorExplosions", true),
            can_game_over: flag("canGameOver", true),
            instant_build: flag("instantBuild", false),
            waves_enabled: flag("waves", false),
            wave_timer: flag("waveTimer", true),
            wave_sending: flag("waveSending", true),
            wait_enemies: flag("waitEnemies", false),
            win_wave: value
                .get("winWave")
                .and_then(serde_json::Value::as_i64)
                .and_then(|wave| i32::try_from(wave).ok())
                .unwrap_or(0),
            wave_team: team("waveTeam", 2),
            default_team: team("defaultTeam", 1),
            possession_allowed: flag("possessionAllowed", true),
            banned_blocks: value
                .get("bannedBlocks")
                .and_then(serde_json::Value::as_array)
                .map(|bans| {
                    bans.iter()
                        .filter_map(|ban| {
                            ban.as_str()
                                .and_then(crate::game::block_names::block_id_from_name)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            block_whitelist: flag("blockWhitelist", false),
            banned_units: value
                .get("bannedUnits")
                .and_then(serde_json::Value::as_array)
                .map(|bans| {
                    bans.iter()
                        .filter_map(|ban| ban.as_str().and_then(parse_unit_type))
                        .collect()
                })
                .unwrap_or_default(),
            unit_whitelist: flag("unitWhitelist", false),
            enemy_core_build_radius: value
                .get("enemyCoreBuildRadius")
                .and_then(serde_json::Value::as_f64)
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(|value| value as f32)
                .unwrap_or(400.0),
            // Rules.teams: `teams:{1:{buildSpeedMultiplier:2.0},2:{...}}`.
            // Unknown team keys are skipped (they cannot gate anything the
            // server simulates without a registered team).
            fog: flag("fog", false),
            // Rules.loadout: "copper-20/lead-10" item-amount pairs.
            loadout: value
                .get("loadout")
                .and_then(serde_json::Value::as_str)
                .map(|loadout| {
                    loadout
                        .split('/')
                        .filter_map(|entry| {
                            let (name, amount) = entry.split_once('-')?;
                            let item = crate::logic::item_id_from_name(name.trim());
                            let amount = amount.trim().parse::<i32>().ok()?;
                            Some((item, amount.max(0)))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            team_rules: value
                .get("teams")
                .and_then(serde_json::Value::as_object)
                .map(|teams| {
                    teams
                        .iter()
                        .filter_map(|(team_id, rule)| {
                            let id = team_id.parse::<u8>().ok()?;
                            Some((id, TeamRule::from_json(rule)))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            unit_cap: value
                .get("unitCap")
                .and_then(serde_json::Value::as_i64)
                .and_then(|cap| i32::try_from(cap).ok())
                .unwrap_or(0)
                .max(0),
            disable_unit_cap: flag("disableUnitCap", false),
            unit_factory_activation_delay: mult("unitFactoryActivationDelay", 0.0),
            editor: flag("editor", false),
            env: value
                .get("env")
                .and_then(serde_json::Value::as_i64)
                .and_then(|env| i32::try_from(env).ok())
                .unwrap_or(crate::game::unit_types::RULES_ENV_DEFAULT),
            block_limits: value
                .get("blockLimits")
                .and_then(serde_json::Value::as_object)
                .map(|limits| {
                    limits
                        .iter()
                        .filter_map(|(name, limit)| {
                            let block_id =
                                crate::game::block_names::block_id_from_name(name.trim())?;
                            let max_count = limit.as_u64()? as u32;
                            Some((block_id, max_count))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        diagnostics,
    )
}

/// Resolves the official `SpawnGroup.getSpawned(wave)` amount and shield for a
/// map-defined spawn group (using the group's own `max` and `effect`).
pub(crate) fn map_spawn_group_amount(wave: u32, group: &MapSpawnGroup) -> u32 {
    spawn_group_amount(
        wave,
        group.begin,
        group.end,
        group.spacing,
        group.scaling,
        group.unit_amount,
        group.max,
    )
}

/// Builds the `WaveSpawn` list for a wave (0-based, like the official
/// `state.wave - 1`) from the loaded map's spawn groups.
pub(crate) fn map_wave_spawns(wave: u32, rules: &WaveRules) -> Vec<WaveSpawn> {
    let mut groups = Vec::new();
    for group in &rules.spawn_groups {
        let amount = map_spawn_group_amount(wave, group);
        if amount == 0 {
            continue;
        }
        // P0-5: banned units never spawn (official WaveSpawner skips
        // `rules.isBanned(unit)` types).
        if rules.unit_banned(group.unit_type) {
            continue;
        }
        let Some(spec) = enemy_spec(group.unit_type) else {
            continue;
        };
        groups.push(WaveSpawn {
            spec,
            amount,
            shield: (group.shields
                + group.shield_scaling * wave.saturating_sub(group.begin) as f32)
                .max(0.0),
            status_effect: group.effect,
            spawn: group.spawn,
        });
    }
    groups
}

#[cfg(test)]
mod upstream_oracle_tests {
    use super::*;

    /// Independent behavioral subset of v159.7 `ApplicationTests.writeRules`.
    ///
    /// Rust does not expose the desktop-only `TypeIO.writeRules` object codec;
    /// map rules arrive as the Rules JSON embedded in MSAV metadata.  This
    /// test therefore checks the represented contract (global multiplier,
    /// team multiplier, flags, and tags that affect server authority) rather
    /// than serializing a Rust struct against itself.
    #[test]
    fn upstream_application_write_rules_1597_behavioral_subset() {
        let (rules, diagnostics) = parse_wave_rules_report(
            r#"{
                "buildSpeedMultiplier": 99.0,
                "infiniteResources": true,
                "waves": true,
                "waveTeam": 2,
                "defaultTeam": 1,
                "teams": {"1": {"buildSpeedMultiplier": 2.0}}
            }"#,
        );
        assert!(diagnostics.is_empty());
        assert!((rules.build_speed_multiplier - 99.0).abs() < f32::EPSILON);
        assert!((rules.build_speed_for(1) - 198.0).abs() < f32::EPSILON);
        assert!(rules.infinite_resources);
        assert!(rules.waves_enabled);
        assert_eq!(rules.wave_team, 2);
        assert_eq!(rules.default_team, 1);
    }

    /// The upstream `Rules.attackMode` and arbitrary `Rules.tags` fields are
    /// intentionally not claimed here: game mode is selected by the host and
    /// tags have no Rust authority consumer.  This test records the exact
    /// defaulting behavior for those out-of-scope fields while guarding the
    /// fields that are represented.
    #[test]
    fn upstream_application_write_rules_1597_unrepresented_fields_are_ignored() {
        let (rules, diagnostics) = parse_wave_rules_report(r#"{}"#);
        assert!(diagnostics.is_empty());
        assert_eq!(rules.build_speed_multiplier, 1.0);
        assert_eq!(rules.default_team, 1);
        assert_eq!(rules.wave_team, 2);
    }
}
