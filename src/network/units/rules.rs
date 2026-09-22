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
    (base + (((wave - begin) / spacing) as f32 / scaling.max(0.000_001)) as u32).min(max)
}

#[derive(Clone, Debug)]
pub(crate) struct WaveSpawn {
    pub(crate) spec: EnemySpec,
    pub(crate) amount: u32,
    pub(crate) shield: f32,
    pub(crate) status_effect: i16,
    /// Packed spawn-point position (`x << 16 | y`) that this group must use,
    /// or -1 to spawn at any spawn overlay (official `SpawnGroup.spawn`).
    pub(crate) spawn: i32,
    /// SpawnGroup.items dropped when the unit dies (audit M9).
    pub(crate) items: Vec<(i16, i32)>,
    /// SpawnGroup.team; None means Rules.waveTeam.
    pub(crate) team: Option<u8>,
    pub(crate) payloads: Vec<i16>,
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
        items: Vec::new(),
        team: None,
        payloads: Vec::new(),
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

#[allow(clippy::too_many_arguments)]
fn wave_spawn_with_effect_and_items(
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
    items: Vec<(i16, i32)>,
) -> Option<WaveSpawn> {
    wave_spawn_with_effect(
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
        status_effect,
    )
    .map(|mut spawn| {
        spawn.items = items;
        spawn
    })
}

fn legacy_unit_id_from_name(name: &str) -> Option<i16> {
    // mindustry.io.versions.LegacyIO.unitMap (desktop 159.7 dump).
    match name.trim().to_ascii_lowercase().as_str() {
        "draug" => Some(20),
        "phantom" | "spirit" => Some(21),
        "wraith" => Some(15),
        "ghoul" => Some(16),
        "revenant" => Some(17),
        "lich" => Some(18),
        "reaper" => Some(19),
        "titan" => Some(1),
        "eruptor" => Some(11),
        "chaos-array" => Some(3),
        "eradicator" => Some(4),
        _ => None,
    }
}

pub(crate) fn initial_official_wave_groups(wave: u32) -> Vec<WaveSpawn> {
    let mut groups = Vec::new();
    for group in [
        // Verbatim mirror of game/Waves.java get() (v159.7 tree, waveVersion
        // 7). Fields in order: begin, end, spacing, unitScaling, unitAmount,
        // max, shields, shieldScaling. Java defaults kept explicit: end =
        // never (u32::MAX), max = 40.
        wave_spawn(wave, DAGGER, 0, 10, 1, 2.0, 1, 30, 0.0, 0.0),
        wave_spawn(wave, CRAWLER, 4, 13, 1, 1.5, 2, 40, 0.0, 0.0),
        wave_spawn(wave, FLARE, 12, 16, 1, 1.0, 1, 40, 0.0, 0.0),
        wave_spawn(wave, DAGGER, 11, u32::MAX, 2, 1.7, 1, 4, 0.0, 25.0),
        wave_spawn(wave, PULSAR, 13, u32::MAX, 3, 0.5, 1, 25, 0.0, 0.0),
        wave_spawn(wave, MACE, 7, 30, 3, 2.0, 1, 40, 0.0, 0.0),
        wave_spawn(wave, DAGGER, 12, u32::MAX, 2, 1.0, 4, 14, 0.0, 20.0),
        wave_spawn(wave, MACE, 28, 40, 3, 1.0, 1, 40, 0.0, 20.0),
        wave_spawn_with_effect(wave, SPIROCT, 45, u32::MAX, 3, 1.0, 1, 10, 100.0, 30.0, 13),
        // pulsar@120 overdrive: no explicit max (Java default 40).
        wave_spawn_with_effect(wave, PULSAR, 120, u32::MAX, 2, 3.0, 5, 40, 0.0, 0.0, 13),
        wave_spawn(wave, FLARE, 16, u32::MAX, 2, 1.0, 1, 20, 0.0, 20.0),
        wave_spawn_with_effect(wave, QUASAR, 82, u32::MAX, 3, 3.0, 4, 40, 0.0, 30.0, 13),
        // pulsar@41 carries 640 flat shields and NO status effect.
        wave_spawn(wave, PULSAR, 41, u32::MAX, 5, 3.0, 1, 25, 640.0, 0.0),
        wave_spawn(wave, FORTRESS, 40, u32::MAX, 5, 2.0, 2, 20, 0.0, 30.0),
        // nova@35 / dagger@42 have no unitScaling (Java `never`): constant
        // amounts, modeled with an effectively infinite scaling. Official
        // Waves.java also stamps items blastCompound x60 / pyratite x100
        // (SpawnGroup.items); units carry the stack and drop it on death (M9).
        wave_spawn_with_effect_and_items(
            wave,
            NOVA,
            35,
            60,
            3,
            1_000_000_000.0,
            4,
            40,
            0.0,
            0.0,
            13,
            vec![(14, 60)],
        ),
        wave_spawn_with_effect_and_items(
            wave,
            DAGGER,
            42,
            130,
            3,
            1_000_000_000.0,
            4,
            30,
            0.0,
            0.0,
            13,
            vec![(15, 100)],
        ),
        wave_spawn(wave, HORIZON, 40, u32::MAX, 2, 2.0, 2, 40, 0.0, 20.0),
        wave_spawn_with_effect(wave, FLARE, 50, u32::MAX, 5, 3.0, 4, 20, 100.0, 10.0, 13),
        wave_spawn(wave, ZENITH, 50, u32::MAX, 5, 3.0, 2, 16, 0.0, 30.0),
        wave_spawn(wave, NOVA, 53, u32::MAX, 4, 3.0, 2, 40, 0.0, 30.0),
        wave_spawn(wave, ATRAX, 31, u32::MAX, 3, 1.0, 4, 40, 0.0, 10.0),
        wave_spawn(wave, SCEPTER, 41, u32::MAX, 30, 1.0, 1, 40, 0.0, 30.0),
        wave_spawn(wave, REIGN, 81, u32::MAX, 40, 1.0, 1, 40, 0.0, 30.0),
        wave_spawn(wave, ANTUMBRA, 120, u32::MAX, 40, 1.0, 1, 40, 0.0, 30.0),
        wave_spawn(wave, VELA, 100, u32::MAX, 30, 1.0, 1, 40, 0.0, 30.0),
        wave_spawn(wave, CORVUS, 145, u32::MAX, 35, 1.0, 1, 40, 100.0, 30.0),
        wave_spawn(wave, HORIZON, 90, u32::MAX, 4, 3.0, 2, 40, 40.0, 30.0),
        wave_spawn(wave, TOXOPID, 210, u32::MAX, 35, 1.0, 1, 40, 1000.0, 35.0),
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
    /// SpawnGroup.items: stacks granted to the unit and dropped on death.
    pub(crate) items: Vec<(i16, i32)>,
    /// SpawnGroup.team; None means Rules.waveTeam.
    pub(crate) team: Option<u8>,
    /// SpawnGroup.payloads: unit types loaded into the spawned unit.
    pub(crate) payloads: Vec<i16>,
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
    /// Rules.fire (Rules.java: default true): gates fire creation and
    /// spread (`Fires.create` early-returns when false; audit H10).
    pub(crate) fires_enabled: bool,
    pub(crate) can_game_over: bool,
    pub(crate) instant_build: bool,
    /// Rules.allowEditRules (Rules.java, default false). Gamemode.sandbox
    /// sets this; it is what lets a sandbox host open the in-game rules UI.
    pub(crate) allow_edit_rules: bool,
    /// Rules.pvp (Rules.java, default false). Gamemode.pvp sets this; the
    /// streamed Rules JSON must carry it so the client enables PvP UI/teams.
    pub(crate) pvp: bool,
    /// Rules.waves: automatic/manual wave spawning is available at all.
    pub(crate) waves_enabled: bool,
    /// Rules.waveTimer: automatic timer-driven waves are enabled.
    pub(crate) wave_timer: bool,
    /// Rules.waveSending: manual play-button wave sending is enabled.
    pub(crate) wave_sending: bool,
    /// Rules.waitEnemies: don't advance the timer while wave-team units live.
    pub(crate) wait_enemies: bool,
    /// Rules.attackMode (Rules.java:49, default false): set by the official
    /// Gamemode.attack and Gamemode.pvp presets. Gates the winWave victory
    /// (Logic.java:357/369 only fires it when `!attackMode`) and marks the
    /// destroy-the-enemy-core objective.
    pub(crate) attack_mode: bool,
    /// Rules.unitBuildSpeedMultiplier (Rules.java, default 1): global unit
    /// factory production speed multiplier (Gamemode.pvp sets it to 2).
    pub(crate) unit_build_speed_multiplier: f32,
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
    /// Rules.dropZoneRadius (Rules.java, default 300): WaveSpawner shockwave
    /// radius around each overlay spawn (ASTRA W03).
    pub(crate) drop_zone_radius: f32,
    /// Rules.teams (Rules.java:411 `TeamRules`): per-team rules keyed by
    /// team id. The official map JSON writes them as `teams:{1:{...},2:{...}}`.
    pub(crate) team_rules: std::collections::HashMap<u8, TeamRule>,
    /// Rules.fog (Rules.java): fog of war. The world stream already carries
    /// the map's original value; the authority parses it so overrides and
    /// strict-mode gates can act on it (single-player campaign fields like
    /// weather/capture/objectives are out of scope for the vanilla server).
    pub(crate) fog: bool,
    /// Rules.loadout (Rules.java): starting inventory as `(item, amount)`
    /// pairs parsed from the map's ItemStack array.
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
    pub(crate) unit_cost_multiplier: f32,
    pub(crate) build_cost_multiplier: f32,
    pub(crate) deconstruct_refund_multiplier: f32,
    pub(crate) solar_multiplier: f32,
    pub(crate) unit_crash_damage_multiplier: f32,
    pub(crate) air_use_spawns: bool,
    pub(crate) random_wave_ai: bool,
    pub(crate) limit_map_area: bool,
    pub(crate) limit_x: i32,
    pub(crate) limit_y: i32,
    pub(crate) limit_width: i32,
    pub(crate) limit_height: i32,
    pub(crate) logic_unit_control: bool,
    pub(crate) logic_unit_build: bool,
    pub(crate) logic_unit_deconstruct: bool,
    pub(crate) unit_payload_update: bool,
    pub(crate) unit_payload_explode: bool,
    /// Rules.dragMultiplier (Rules.java:161, default 1).
    pub(crate) drag_multiplier: f32,
    /// Rules.wavesSpawnAtCores (Rules.java:39, default true).
    pub(crate) waves_spawn_at_cores: bool,
    /// Rules.onlyDepositCore (Rules.java:131, default false).
    pub(crate) only_deposit_core: bool,
    /// Rules.coreDestroyClear (Rules.java:137, default false).
    pub(crate) core_destroy_clear: bool,
    /// Rules.damageExplosions (Rules.java:65, default true).
    pub(crate) damage_explosions: bool,
    /// Rules.derelictRepair (Rules.java:53, default true).
    pub(crate) derelict_repair: bool,
    /// Rules.objectiveTimerMultiplier (Rules.java:121, default 1).
    pub(crate) objective_timer_multiplier: f32,
    /// Rules.unitCapVariable (Rules.java:75, default true).
    pub(crate) unit_cap_variable: bool,
    /// Rules.polygonCoreProtection (Rules.java:127, default false).
    pub(crate) polygon_core_protection: bool,
    /// Rules.placeRangeCheck (Rules.java:129, default false).
    pub(crate) place_range_check: bool,
    /// Rules.lighting (Rules.java:207, default false).
    pub(crate) lighting: bool,
    /// Rules.unitLight (v160.5, default true).
    pub(crate) unit_light: bool,
    /// Rules.coreBuildAndConfig (v160.5, default false).
    pub(crate) core_build_and_config: bool,
    /// Rules.staticFog (Rules.java:201, default true).
    pub(crate) static_fog: bool,
    /// Rules.ghostBlocks (Rules.java:95, default true).
    pub(crate) ghost_blocks: bool,
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
            fires_enabled: true,
            can_game_over: true,
            instant_build: false,
            allow_edit_rules: false,
            pvp: false,
            waves_enabled: false,
            wave_timer: true,
            wave_sending: true,
            wait_enemies: false,
            attack_mode: false,
            unit_build_speed_multiplier: 1.0,
            win_wave: 0,
            wave_team: 2,
            default_team: 1,
            possession_allowed: true,
            banned_blocks: Vec::new(),
            block_whitelist: false,
            banned_units: Vec::new(),
            unit_whitelist: false,
            enemy_core_build_radius: 400.0,
            drop_zone_radius: 300.0,
            team_rules: std::collections::HashMap::new(),
            fog: false,
            loadout: vec![(0, 100)],
            unit_cap: 0,
            disable_unit_cap: false,
            unit_factory_activation_delay: 0.0,
            block_limits: std::collections::HashMap::new(),
            editor: false,
            env: crate::game::unit_types::RULES_ENV_DEFAULT,
            unit_cost_multiplier: 1.0,
            build_cost_multiplier: 1.0,
            deconstruct_refund_multiplier: 0.5,
            solar_multiplier: 1.0,
            unit_crash_damage_multiplier: 1.0,
            air_use_spawns: false,
            random_wave_ai: false,
            limit_map_area: false,
            limit_x: 0,
            limit_y: 0,
            limit_width: 0,
            limit_height: 0,
            logic_unit_control: true,
            logic_unit_build: true,
            logic_unit_deconstruct: false,
            unit_payload_update: false,
            unit_payload_explode: false,
            drag_multiplier: 1.0,
            waves_spawn_at_cores: true,
            only_deposit_core: false,
            core_destroy_clear: false,
            damage_explosions: true,
            derelict_repair: true,
            objective_timer_multiplier: 1.0,
            unit_cap_variable: true,
            polygon_core_protection: false,
            place_range_check: false,
            lighting: false,
            unit_light: true,
            core_build_and_config: false,
            static_fog: true,
            ghost_blocks: true,
        }
    }
}

/// Shared default TeamRule instance (all official defaults).
pub(crate) static DEFAULT_TEAM_RULE: TeamRule = TeamRule {
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
    unit_crash_damage_multiplier: 1.0,
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

    /// Official `Rules.unitBuildSpeed(team)` (Rules.java:300):
    /// `unitBuildSpeedMultiplier * teams.get(team).unitBuildSpeedMultiplier`.
    pub(crate) fn unit_build_speed_for(&self, team: u8) -> f32 {
        self.unit_build_speed_multiplier * self.team_rule(team).unit_build_speed_multiplier
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
    /// **PARITY** — `UnitType.controller` plus `assignSquads`/`handleSquad`
    /// when this flag is set; GroundAI/FlyingAI otherwise.
    pub(crate) rts_ai: bool,
    /// Official `TeamRule.rtsMinSquad` — RtsAI minimum squad size.
    pub(crate) rts_min_squad: i32,
    /// Official `TeamRule.rtsMaxSquad` — RtsAI maximum squad before forced attack.
    pub(crate) rts_max_squad: i32,
    /// Official `TeamRule.rtsMinWeight` — RtsAI attack weight threshold.
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
    pub(crate) unit_crash_damage_multiplier: f32,
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
            unit_crash_damage_multiplier: 1.0,
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
            unit_crash_damage_multiplier: mult("unitCrashDamageMultiplier", 1.0),
        }
    }
}

/// Accept vanilla ItemStack arrays and the legacy Oxide operator shorthand.
/// Only arrays are emitted to clients.
pub(crate) fn parse_loadout(value: &serde_json::Value) -> Option<Vec<(i16, i32)>> {
    if let Some(entries) = value.as_array() {
        return entries
            .iter()
            .map(|entry| {
                let name = entry.get("item")?.as_str()?;
                let item = crate::logic::item_id_from_name(name);
                if crate::logic::item_name_from_id(item) != Some(name) {
                    return None;
                }
                let amount = i32::try_from(entry.get("amount")?.as_i64()?).ok()?;
                Some((item, amount.max(0)))
            })
            .collect();
    }
    Some(
        value
            .as_str()?
            .split('/')
            .filter_map(|entry| {
                let (name, amount) = entry.rsplit_once('-')?;
                let item = crate::logic::item_id_from_name(name.trim());
                let amount = amount.trim().parse::<i32>().ok()?;
                Some((item, amount.max(0)))
            })
            .collect(),
    )
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
    // ASTRA W04: Java SpawnGroup falls back to dagger for a missing, unknown
    // or internal type, and remaps historical names through LegacyIO.unitMap.
    let unit_type = match value.get("type") {
        Some(serde_json::Value::String(name)) => parse_unit_type(name)
            .or_else(|| legacy_unit_id_from_name(name))
            .unwrap_or(0),
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .and_then(|id| i16::try_from(id).ok())
            .filter(|id| crate::game::unit_types::unit_name_from_id(*id).is_some())
            .unwrap_or(0),
        _ => 0,
    };
    let unit_type = if crate::game::unit_types::unit_type_internal(unit_type) {
        0
    } else {
        unit_type
    };
    if enemy_spec(unit_type).is_none() {
        return SpawnGroupParse::Skipped(format!(
            "unit type {unit_type} has no simulated enemy spec"
        ));
    }
    let integer = |key: &str, default: i64| -> i64 {
        value
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(default)
    };
    let end = integer("end", i32::MAX as i64);
    let unit_amount = integer("unitAmount", integer("amount", 1)).max(0) as u32;
    SpawnGroupParse::Supported(MapSpawnGroup {
        unit_type,
        begin: integer("begin", 0).max(0) as u32,
        end: if end >= i32::MAX as i64 {
            u32::MAX
        } else {
            end.max(0) as u32
        },
        spacing: integer("spacing", 1).max(1) as u32,
        // ASTRA W04: max=0 is a valid "spawn nobody" value; do not raise it.
        max: integer("max", 40).max(0) as u32,
        scaling: value
            .get("scaling")
            .or_else(|| value.get("unitScaling"))
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
        unit_amount,
        spawn: integer("spawn", -1).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        effect: parse_spawn_effect(value.get("effect")),
        items: parse_spawn_items(value.get("items")),
        team: value
            .get("team")
            .and_then(serde_json::Value::as_u64)
            .and_then(|id| u8::try_from(id).ok()),
        payloads: value
            .get("payloads")
            .and_then(serde_json::Value::as_array)
            .map(|payloads| {
                payloads
                    .iter()
                    .filter_map(|payload| {
                        payload
                            .as_str()
                            .and_then(parse_unit_type)
                            .or_else(|| payload.as_i64().and_then(|id| i16::try_from(id).ok()))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn parse_spawn_effect(value: Option<&serde_json::Value>) -> i16 {
    match value {
        Some(serde_json::Value::String(name)) => status_effect_id_by_name(name),
        Some(serde_json::Value::Number(n)) => match n.as_i64() {
            Some(8) => 16, // historical boss id
            Some(id) => i16::try_from(id).unwrap_or(-1),
            None => -1,
        },
        _ => -1,
    }
}

fn parse_spawn_items(value: Option<&serde_json::Value>) -> Vec<(i16, i32)> {
    match value {
        Some(serde_json::Value::Array(stacks)) => {
            stacks.iter().filter_map(parse_item_stack).collect()
        }
        Some(object) if object.is_object() => parse_item_stack(object).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn parse_item_stack(stack: &serde_json::Value) -> Option<(i16, i32)> {
    let item = stack
        .get("item")
        .and_then(serde_json::Value::as_str)
        .map(crate::logic::item_id_from_name)
        .or_else(|| {
            stack
                .get("item")
                .and_then(serde_json::Value::as_i64)
                .and_then(|id| i16::try_from(id).ok())
        })?;
    let amount = stack
        .get("amount")
        .and_then(serde_json::Value::as_i64)
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(1)
        .max(0);
    (amount > 0).then_some((item, amount))
}

/// Projects live `WaveRules` onto the map's original Rules JSON.
///
/// Uninterpreted keys (`spawns`, `class` hints, unknown TeamRule fields,
/// tags) stay on the original object. Every field the authority interprets
/// is written from `rules` so join, SetRules and MSAV export share one
/// document. Nested `teams` objects are merged field-by-field: a partial
/// TeamRule never replaces the whole team object (ASTRA R01).
pub(crate) fn serialize_live_rules_json(map_rules: &str, rules: &WaveRules) -> String {
    let mut root = match serde_json::from_str::<serde_json::Value>(&arc_json_to_strict(map_rules)) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    insert_f32(&mut root, "waveSpacing", rules.wave_spacing);
    insert_f32(&mut root, "initialWaveSpacing", rules.initial_wave_spacing);
    insert_f32(
        &mut root,
        "buildSpeedMultiplier",
        rules.build_speed_multiplier,
    );
    insert_f32(
        &mut root,
        "unitMineSpeedMultiplier",
        rules.unit_mine_speed_multiplier,
    );
    insert_f32(
        &mut root,
        "blockHealthMultiplier",
        rules.block_health_multiplier,
    );
    insert_f32(
        &mut root,
        "blockDamageMultiplier",
        rules.block_damage_multiplier,
    );
    insert_f32(
        &mut root,
        "unitDamageMultiplier",
        rules.unit_damage_multiplier,
    );
    insert_f32(
        &mut root,
        "unitHealthMultiplier",
        rules.unit_health_multiplier,
    );
    insert_bool(&mut root, "infiniteResources", rules.infinite_resources);
    insert_bool(&mut root, "coreIncinerates", rules.core_incinerates);
    insert_bool(&mut root, "reactorExplosions", rules.reactor_explosions);
    insert_bool(&mut root, "fire", rules.fires_enabled);
    insert_bool(&mut root, "canGameOver", rules.can_game_over);
    insert_bool(&mut root, "instantBuild", rules.instant_build);
    insert_bool(&mut root, "allowEditRules", rules.allow_edit_rules);
    insert_bool(&mut root, "pvp", rules.pvp);
    insert_bool(&mut root, "waves", rules.waves_enabled);
    insert_bool(&mut root, "waveTimer", rules.wave_timer);
    insert_bool(&mut root, "waveSending", rules.wave_sending);
    insert_bool(&mut root, "waitEnemies", rules.wait_enemies);
    insert_bool(&mut root, "attackMode", rules.attack_mode);
    insert_f32(
        &mut root,
        "unitBuildSpeedMultiplier",
        rules.unit_build_speed_multiplier,
    );
    insert_i64(&mut root, "winWave", i64::from(rules.win_wave));
    insert_u64(&mut root, "waveTeam", u64::from(rules.wave_team));
    insert_u64(&mut root, "defaultTeam", u64::from(rules.default_team));
    insert_bool(&mut root, "possessionAllowed", rules.possession_allowed);
    insert_bool(&mut root, "blockWhitelist", rules.block_whitelist);
    insert_bool(&mut root, "unitWhitelist", rules.unit_whitelist);
    insert_f32(
        &mut root,
        "enemyCoreBuildRadius",
        rules.enemy_core_build_radius,
    );
    insert_f32(&mut root, "dropZoneRadius", rules.drop_zone_radius);
    insert_bool(&mut root, "fog", rules.fog);
    insert_i64(&mut root, "unitCap", i64::from(rules.unit_cap));
    insert_bool(&mut root, "disableUnitCap", rules.disable_unit_cap);
    insert_f32(
        &mut root,
        "unitFactoryActivationDelay",
        rules.unit_factory_activation_delay,
    );
    insert_bool(&mut root, "editor", rules.editor);
    insert_i64(&mut root, "env", i64::from(rules.env));
    insert_f32(&mut root, "unitCostMultiplier", rules.unit_cost_multiplier);
    insert_f32(
        &mut root,
        "buildCostMultiplier",
        rules.build_cost_multiplier,
    );
    insert_f32(
        &mut root,
        "deconstructRefundMultiplier",
        rules.deconstruct_refund_multiplier,
    );
    insert_f32(&mut root, "solarMultiplier", rules.solar_multiplier);
    insert_f32(
        &mut root,
        "unitCrashDamageMultiplier",
        rules.unit_crash_damage_multiplier,
    );
    insert_bool(&mut root, "airUseSpawns", rules.air_use_spawns);
    insert_bool(&mut root, "randomWaveAI", rules.random_wave_ai);
    insert_bool(&mut root, "limitMapArea", rules.limit_map_area);
    insert_i64(&mut root, "limitX", i64::from(rules.limit_x));
    insert_i64(&mut root, "limitY", i64::from(rules.limit_y));
    insert_i64(&mut root, "limitWidth", i64::from(rules.limit_width));
    insert_i64(&mut root, "limitHeight", i64::from(rules.limit_height));
    insert_bool(&mut root, "logicUnitControl", rules.logic_unit_control);
    insert_bool(&mut root, "logicUnitBuild", rules.logic_unit_build);
    insert_bool(
        &mut root,
        "logicUnitDeconstruct",
        rules.logic_unit_deconstruct,
    );
    insert_bool(&mut root, "unitPayloadUpdate", rules.unit_payload_update);
    insert_bool(&mut root, "unitPayloadsExplode", rules.unit_payload_explode);
    insert_f32(&mut root, "dragMultiplier", rules.drag_multiplier);
    insert_bool(&mut root, "wavesSpawnAtCores", rules.waves_spawn_at_cores);
    insert_bool(&mut root, "onlyDepositCore", rules.only_deposit_core);
    insert_bool(&mut root, "coreDestroyClear", rules.core_destroy_clear);
    insert_bool(&mut root, "damageExplosions", rules.damage_explosions);
    insert_bool(&mut root, "derelictRepair", rules.derelict_repair);
    insert_f32(
        &mut root,
        "objectiveTimerMultiplier",
        rules.objective_timer_multiplier,
    );
    insert_bool(&mut root, "unitCapVariable", rules.unit_cap_variable);
    insert_bool(
        &mut root,
        "polygonCoreProtection",
        rules.polygon_core_protection,
    );
    insert_bool(&mut root, "placeRangeCheck", rules.place_range_check);
    insert_bool(&mut root, "lighting", rules.lighting);
    insert_bool(&mut root, "unitLight", rules.unit_light);
    insert_bool(
        &mut root,
        "coreBuildAndConfig",
        rules.core_build_and_config,
    );
    insert_bool(&mut root, "staticFog", rules.static_fog);
    insert_bool(&mut root, "ghostBlocks", rules.ghost_blocks);
    root.insert("loadout".into(), serialize_loadout(&rules.loadout));
    root.insert(
        "bannedBlocks".into(),
        serde_json::Value::Array(
            rules
                .banned_blocks
                .iter()
                .filter_map(|id| crate::game::block_names::block_name_from_id(*id))
                .map(|name| serde_json::Value::String(name.to_string()))
                .collect(),
        ),
    );
    root.insert(
        "bannedUnits".into(),
        serde_json::Value::Array(
            rules
                .banned_units
                .iter()
                .filter_map(|id| crate::game::unit_types::unit_name_from_id(*id))
                .map(|name| serde_json::Value::String(name.to_string()))
                .collect(),
        ),
    );
    if !rules.block_limits.is_empty() || root.contains_key("blockLimits") {
        let mut limits = serde_json::Map::new();
        let mut names: Vec<(String, u32)> = rules
            .block_limits
            .iter()
            .filter_map(|(id, limit)| {
                crate::game::block_names::block_name_from_id(*id)
                    .map(|name| (name.to_string(), *limit))
            })
            .collect();
        names.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, limit) in names {
            limits.insert(name, serde_json::json!(limit));
        }
        root.insert("blockLimits".into(), serde_json::Value::Object(limits));
    }
    overlay_team_rules(&mut root, rules);
    serde_json::Value::Object(root).to_string()
}

fn insert_bool(root: &mut serde_json::Map<String, serde_json::Value>, key: &str, value: bool) {
    root.insert(key.to_string(), serde_json::Value::Bool(value));
}

fn insert_f32(root: &mut serde_json::Map<String, serde_json::Value>, key: &str, value: f32) {
    root.insert(key.to_string(), serde_json::json!(f64::from(value)));
}

fn insert_i64(root: &mut serde_json::Map<String, serde_json::Value>, key: &str, value: i64) {
    root.insert(key.to_string(), serde_json::json!(value));
}

fn insert_u64(root: &mut serde_json::Map<String, serde_json::Value>, key: &str, value: u64) {
    root.insert(key.to_string(), serde_json::json!(value));
}

fn serialize_loadout(loadout: &[(i16, i32)]) -> serde_json::Value {
    serde_json::Value::Array(
        loadout
            .iter()
            .filter_map(|(item, amount)| {
                crate::logic::item_name_from_id(*item)
                    .map(|name| serde_json::json!({"item": name, "amount": amount}))
            })
            .collect(),
    )
}

fn overlay_team_rules(root: &mut serde_json::Map<String, serde_json::Value>, rules: &WaveRules) {
    if rules.team_rules.is_empty() && !root.contains_key("teams") {
        return;
    }
    let mut teams = match root.remove("teams") {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let mut ids: Vec<u8> = rules.team_rules.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let Some(rule) = rules.team_rules.get(&id) else {
            continue;
        };
        let key = id.to_string();
        let mut team = match teams.remove(&key) {
            Some(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        overlay_team_rule(&mut team, rule);
        if team.is_empty() {
            continue;
        }
        teams.insert(key, serde_json::Value::Object(team));
    }
    teams.retain(|_, value| match value {
        serde_json::Value::Object(map) => !map.is_empty(),
        _ => true,
    });
    if !teams.is_empty() {
        root.insert("teams".into(), serde_json::Value::Object(teams));
    }
}

fn overlay_team_rule(team: &mut serde_json::Map<String, serde_json::Value>, rule: &TeamRule) {
    insert_bool(team, "protectCores", rule.protect_cores);
    insert_bool(team, "checkPlacement", rule.check_placement);
    insert_bool(team, "cheat", rule.cheat);
    insert_bool(team, "fillItems", rule.fill_items);
    insert_bool(team, "infiniteResources", rule.infinite_resources);
    insert_bool(team, "prebuildAi", rule.prebuild_ai);
    insert_bool(team, "buildAi", rule.build_ai);
    insert_f32(team, "buildAiTier", rule.build_ai_tier);
    insert_bool(team, "rtsAi", rule.rts_ai);
    insert_i64(team, "rtsMinSquad", i64::from(rule.rts_min_squad));
    insert_i64(team, "rtsMaxSquad", i64::from(rule.rts_max_squad));
    insert_f32(team, "rtsMinWeight", rule.rts_min_weight);
    insert_f32(
        team,
        "unitFactoryActivationDelay",
        rule.unit_factory_activation_delay,
    );
    insert_f32(
        team,
        "unitBuildSpeedMultiplier",
        rule.unit_build_speed_multiplier,
    );
    insert_f32(team, "unitDamageMultiplier", rule.unit_damage_multiplier);
    insert_f32(
        team,
        "unitMineSpeedMultiplier",
        rule.unit_mine_speed_multiplier,
    );
    insert_f32(team, "unitCostMultiplier", rule.unit_cost_multiplier);
    insert_f32(team, "unitHealthMultiplier", rule.unit_health_multiplier);
    insert_f32(team, "blockHealthMultiplier", rule.block_health_multiplier);
    insert_f32(team, "blockDamageMultiplier", rule.block_damage_multiplier);
    insert_f32(team, "buildSpeedMultiplier", rule.build_speed_multiplier);
    insert_f32(team, "extraCoreBuildRadius", rule.extra_core_build_radius);
    insert_f32(
        team,
        "unitCrashDamageMultiplier",
        rule.unit_crash_damage_multiplier,
    );
}

/// Overwrites top-level scalar entries of an arc-JSON `Rules` document,
/// leaving every other byte untouched. Used to project the live rules onto the
/// world stream a client receives: rewriting the whole blob through serde
/// would have to re-emit arc's bare identifiers, `class` hints and nested
/// spawn groups, so the keys are patched in place instead.
///
/// Only depth-1 keys are considered — `teams:{1:{infiniteResources:true}}`
/// must not be mistaken for the global flag. Keys that are absent are
/// inserted right after the opening brace, which arc's reader accepts.
pub(crate) fn patch_rules_json(input: &str, overrides: &[(&str, String)]) -> String {
    let mut json = input.trim().to_owned();
    if !json.starts_with('{') {
        // Not an object (empty/absent map rules): emit the overrides alone.
        let body = overrides
            .iter()
            .map(|(key, value)| format!("\"{key}\":{value}"))
            .collect::<Vec<_>>()
            .join(",");
        return format!("{{{body}}}");
    }
    for (key, value) in overrides {
        json = match locate_top_level_value(&json, key) {
            Some(range) => {
                let mut patched = String::with_capacity(json.len() + value.len());
                patched.push_str(&json[..range.start]);
                patched.push_str(value);
                patched.push_str(&json[range.end..]);
                patched
            }
            None => {
                let separator = if json[1..].trim_start().starts_with('}') {
                    ""
                } else {
                    ","
                };
                format!("{{\"{key}\":{value}{separator}{}", &json[1..])
            }
        };
    }
    json
}

/// Byte range of the scalar value bound to a depth-1 `key` in an arc-JSON
/// object, or `None` when the key is absent or its value is not a scalar.
fn locate_top_level_value(json: &str, key: &str) -> Option<std::ops::Range<usize>> {
    let bytes = json.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
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
                if depth == 1 && json[start + 1..i.saturating_sub(1)] == *key {
                    if let Some(range) = scalar_value_after(json, i) {
                        return Some(range);
                    }
                }
            }
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            _ => {
                // Bare arc identifier: a depth-1 one followed by `:` is a key.
                let start = i;
                while i < bytes.len() && !b"{}[]:,\" \t\r\n".contains(&bytes[i]) {
                    i += 1;
                }
                if i == start {
                    i += 1;
                } else if depth == 1 && &json[start..i] == key {
                    if let Some(range) = scalar_value_after(json, i) {
                        return Some(range);
                    }
                }
            }
        }
    }
    None
}

/// Byte range of the scalar that follows the `:` at or after `from`.
fn scalar_value_after(json: &str, from: usize) -> Option<std::ops::Range<usize>> {
    let bytes = json.as_bytes();
    let mut i = from;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b':' {
        return None;
    }
    i += 1;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let start = i;
    if i < bytes.len() && (bytes[i] == b'{' || bytes[i] == b'[') {
        return None;
    }
    if i < bytes.len() && bytes[i] == b'"' {
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
    } else {
        while i < bytes.len() && !b",}]".contains(&bytes[i]) {
            i += 1;
        }
    }
    (start < i).then_some(start..i)
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
            // Global factory speed multiplier (Rules.unitBuildSpeed);
            // Gamemode.pvp overrides it to 2.
            unit_build_speed_multiplier: mult("unitBuildSpeedMultiplier", 1.0),
            infinite_resources: flag("infiniteResources", false),
            core_incinerates: flag("coreIncinerates", true),
            reactor_explosions: flag("reactorExplosions", true),
            fires_enabled: flag("fire", true),
            can_game_over: flag("canGameOver", true),
            instant_build: flag("instantBuild", false),
            allow_edit_rules: flag("allowEditRules", false),
            pvp: flag("pvp", false),
            waves_enabled: flag("waves", false),
            wave_timer: flag("waveTimer", true),
            wave_sending: flag("waveSending", true),
            wait_enemies: flag("waitEnemies", false),
            attack_mode: flag("attackMode", false),
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
            drop_zone_radius: value
                .get("dropZoneRadius")
                .and_then(serde_json::Value::as_f64)
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(|value| value as f32)
                .unwrap_or(300.0),
            // Rules.teams: `teams:{1:{buildSpeedMultiplier:2.0},2:{...}}`.
            // Unknown team keys are skipped (they cannot gate anything the
            // server simulates without a registered team).
            fog: flag("fog", false),
            // Rules.loadout is a Seq<ItemStack>; omission keeps Java's copper x100.
            loadout: value
                .get("loadout")
                .and_then(parse_loadout)
                .unwrap_or_else(|| vec![(0, 100)]),
            team_rules: value
                .get("teams")
                .and_then(serde_json::Value::as_object)
                .map(|teams| {
                    teams
                        .iter()
                        .filter_map(|(team_id, rule)| {
                            let id = team_id.parse::<u8>().ok()?;
                            if rule.as_object().is_some_and(serde_json::Map::is_empty) {
                                return None;
                            }
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
            unit_cost_multiplier: mult("unitCostMultiplier", 1.0),
            build_cost_multiplier: mult("buildCostMultiplier", 1.0),
            deconstruct_refund_multiplier: mult("deconstructRefundMultiplier", 0.5),
            solar_multiplier: mult("solarMultiplier", 1.0),
            unit_crash_damage_multiplier: mult("unitCrashDamageMultiplier", 1.0),
            air_use_spawns: flag("airUseSpawns", false),
            random_wave_ai: flag("randomWaveAI", false),
            limit_map_area: flag("limitMapArea", false),
            limit_x: value
                .get("limitX")
                .and_then(serde_json::Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(0),
            limit_y: value
                .get("limitY")
                .and_then(serde_json::Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(0),
            limit_width: value
                .get("limitWidth")
                .and_then(serde_json::Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(0),
            limit_height: value
                .get("limitHeight")
                .and_then(serde_json::Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(0),
            logic_unit_control: flag("logicUnitControl", true),
            logic_unit_build: flag("logicUnitBuild", true),
            logic_unit_deconstruct: flag("logicUnitDeconstruct", false),
            unit_payload_update: flag("unitPayloadUpdate", false),
            unit_payload_explode: flag("unitPayloadsExplode", false)
                || flag("unitPayloadExplode", false),
            drag_multiplier: mult("dragMultiplier", 1.0),
            waves_spawn_at_cores: flag("wavesSpawnAtCores", true),
            only_deposit_core: flag("onlyDepositCore", false),
            core_destroy_clear: flag("coreDestroyClear", false),
            damage_explosions: flag("damageExplosions", true),
            derelict_repair: flag("derelictRepair", true),
            objective_timer_multiplier: mult("objectiveTimerMultiplier", 1.0),
            unit_cap_variable: flag("unitCapVariable", true),
            polygon_core_protection: flag("polygonCoreProtection", false),
            place_range_check: flag("placeRangeCheck", false),
            lighting: flag("lighting", false),
            unit_light: flag("unitLight", true),
            core_build_and_config: flag("coreBuildAndConfig", false),
            static_fog: flag("staticFog", true),
            ghost_blocks: flag("ghostBlocks", true),
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
        // ASTRA W04: a fabrication ban is not a wave ban. WaveSpawner.spawnEnemies
        // does not consult Rules.bannedUnits.
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
            items: group.items.clone(),
            team: group.team,
            payloads: group.payloads.clone(),
        });
    }
    groups
}

#[cfg(test)]
mod patch_rules_json_tests {
    use super::*;

    #[test]
    fn overwrites_existing_bare_arc_key_in_place() {
        let patched = patch_rules_json(
            "{alwaysPlayMusic:true,infiniteResources:false,waveSpacing:7200.0}",
            &[("infiniteResources", "true".into())],
        );
        assert_eq!(
            patched,
            "{alwaysPlayMusic:true,infiniteResources:true,waveSpacing:7200.0}"
        );
        assert!(parse_wave_rules(&patched).infinite_resources);
    }

    #[test]
    fn inserts_absent_key_and_handles_the_empty_object() {
        let patched = patch_rules_json("{waves:true}", &[("infiniteResources", "true".into())]);
        assert_eq!(patched, "{\"infiniteResources\":true,waves:true}");
        assert!(parse_wave_rules(&patched).infinite_resources);
        assert_eq!(
            patch_rules_json("{}", &[("attackMode", "true".into())]),
            "{\"attackMode\":true}"
        );
        assert_eq!(
            patch_rules_json("", &[("attackMode", "true".into())]),
            "{\"attackMode\":true}"
        );
    }

    #[test]
    fn never_touches_nested_team_rules_or_spawn_groups() {
        // `teams:{1:{infiniteResources:...}}` is a TeamRule, not the global
        // flag; spawn groups must survive byte for byte.
        let source = "{spawns:[{type:dagger,effect:sapped,end:1}],teams:{1:{infiniteResources:false}},infiniteResources:false}";
        let patched = patch_rules_json(source, &[("infiniteResources", "true".into())]);
        assert_eq!(
            patched,
            "{spawns:[{type:dagger,effect:sapped,end:1}],teams:{1:{infiniteResources:false}},infiniteResources:true}"
        );
        let rules = parse_wave_rules(&patched);
        assert!(rules.infinite_resources, "global flag patched");
        assert_eq!(rules.spawn_groups.len(), 1, "spawn group preserved");
        assert!(
            !rules.team_rule(1).infinite_resources,
            "nested TeamRule untouched"
        );
    }

    #[test]
    fn quoted_keys_and_string_values_that_look_like_keys_are_safe() {
        let patched = patch_rules_json(
            "{\"tags\":\"infiniteResources\",\"waveTimer\":true}",
            &[("waveTimer", "false".into())],
        );
        assert_eq!(
            patched,
            "{\"tags\":\"infiniteResources\",\"waveTimer\":false}"
        );
        assert!(!parse_wave_rules(&patched).wave_timer);
    }

    #[test]
    fn applies_every_override_in_one_pass() {
        let patched = patch_rules_json(
            "{waveSpacing:7200.0,waveTimer:true}",
            &[
                ("waveTimer", "false".into()),
                ("waveSpacing", "120".into()),
                ("attackMode", "true".into()),
            ],
        );
        let rules = parse_wave_rules(&patched);
        assert!(!rules.wave_timer);
        assert_eq!(rules.wave_spacing, 120.0);
        assert!(rules.attack_mode);
    }
}

#[cfg(test)]
mod live_rules_json_tests {
    use super::*;

    #[test]
    fn loadout_uses_vanilla_item_stack_arrays_including_explicit_empty() {
        for (map, expected) in [
            ("{}", vec![(0, 100)]),
            ("{loadout:[]}", vec![]),
            (
                "{loadout:[{item:lead,amount:50},{item:surge-alloy,amount:7}]}",
                vec![(1, 50), (12, 7)],
            ),
            (
                r#"{"loadout":"copper-20/surge-alloy-7"}"#,
                vec![(0, 20), (12, 7)],
            ),
        ] {
            let rules = parse_wave_rules(map);
            assert_eq!(rules.loadout, expected, "{map}");
            let live = serialize_live_rules_json(map, &rules);
            let value: serde_json::Value = serde_json::from_str(&live).unwrap();
            assert!(value["loadout"].is_array(), "{live}");
            assert_eq!(parse_wave_rules(&live).loadout, expected);
        }
        let mut rules = parse_wave_rules("{}");
        rules.loadout.clear();
        assert!(parse_wave_rules(&serialize_live_rules_json("{}", &rules))
            .loadout
            .is_empty());
    }

    #[test]
    fn r01_preserves_uninterpreted_keys_and_writes_live_interpreted_fields() {
        // ASTRA R01: join/SetRules/save share one document. Custom spawns and
        // unknown TeamRule keys survive; a live setrule is visible.
        let map = "{alwaysPlayMusic:true,spawns:[{type:dagger,begin:0,end:2,unitAmount:3,items:{item:pyratite,amount:7}}],waveSpacing:3600,waitEnemies:false,winWave:40,defaultTeam:1,waveTeam:2,unitCap:8,unitCapVariable:true,unitBuildSpeedMultiplier:1,teams:{1:{rtsAi:true,unitDamageMultiplier:2,customHint:true}}}";
        let mut rules = parse_wave_rules(map);
        rules.wait_enemies = true;
        rules.unit_cap = 20;
        rules
            .team_rules
            .entry(1)
            .or_default()
            .unit_health_multiplier = 3.0;
        let live = serialize_live_rules_json(map, &rules);
        assert!(
            live.contains("alwaysPlayMusic"),
            "uninterpreted map key kept: {live}"
        );
        assert!(
            live.contains("customHint"),
            "unknown TeamRule field kept: {live}"
        );
        assert!(
            live.contains("pyratite"),
            "spawn group items object kept: {live}"
        );
        let parsed = parse_wave_rules(&live);
        assert!(parsed.wait_enemies, "live waitEnemies");
        assert_eq!(parsed.unit_cap, 20);
        assert_eq!(parsed.win_wave, 40);
        assert_eq!(parsed.wave_spacing, 3600.0);
        assert_eq!(parsed.default_team, 1);
        assert_eq!(parsed.wave_team, 2);
        assert!(parsed.unit_cap_variable);
        assert_eq!(parsed.spawn_groups.len(), 1, "custom spawns preserved");
        assert_eq!(parsed.team_rule(1).unit_health_multiplier, 3.0);
        assert_eq!(parsed.team_rule(1).unit_damage_multiplier, 2.0);
        assert!(parsed.team_rule(1).rts_ai);
    }

    #[test]
    fn r01_does_not_replace_a_team_object_with_a_partial_rule() {
        let map = "{teams:{1:{unitDamageMultiplier:2,buildAi:true,mystery:1}}}";
        let mut rules = parse_wave_rules(map);
        rules.team_rules.entry(1).or_default().unit_cost_multiplier = 4.0;
        let live = serialize_live_rules_json(map, &rules);
        let value: serde_json::Value = serde_json::from_str(&live).unwrap();
        assert_eq!(value["teams"]["1"]["mystery"], 1);
        assert_eq!(value["teams"]["1"]["buildAi"], true);
        assert_eq!(value["teams"]["1"]["unitCostMultiplier"], 4.0);
        assert_eq!(value["teams"]["1"]["unitDamageMultiplier"], 2.0);
    }

    #[test]
    fn w04_parses_item_object_team_payloads_legacy_type_and_max_zero() {
        let parsed = parse_wave_rules(
            "{spawns:[{type:draug,items:{item:pyratite,amount:7},team:5,payloads:[dagger],max:0,effect:8}]}",
        );
        assert_eq!(parsed.spawn_groups.len(), 1);
        let group = &parsed.spawn_groups[0];
        assert_eq!(group.unit_type, 20, "draug remaps to mono");
        assert_eq!(group.items, vec![(15, 7)]);
        assert_eq!(group.team, Some(5));
        assert_eq!(group.payloads, vec![0]);
        assert_eq!(group.max, 0);
        assert_eq!(group.effect, 16, "historical effect 8 is boss");
        assert_eq!(map_spawn_group_amount(0, group), 0);
        let omitted = parse_wave_rules("{spawns:[{begin:0}]}");
        assert_eq!(
            omitted.spawn_groups[0].unit_type, 0,
            "missing type is dagger"
        );
    }
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

    #[test]
    fn java_defaults_for_deconstruct_payloads_and_drag() {
        let (rules, _) = parse_wave_rules_report("{}");
        assert!(
            !rules.logic_unit_deconstruct,
            "Rules.logicUnitDeconstruct defaults to false"
        );
        assert!(!rules.unit_payload_update);
        assert!(!rules.unit_payload_explode);
        assert!((rules.drag_multiplier - 1.0).abs() < f32::EPSILON);
        assert!(rules.waves_spawn_at_cores);
        assert!(rules.derelict_repair);
        assert!(rules.damage_explosions);
        let (payloads, _) = parse_wave_rules_report(r#"{"unitPayloadExplode":true}"#);
        assert!(payloads.unit_payload_explode);
        let (official, _) = parse_wave_rules_report(r#"{"unitPayloadsExplode":true}"#);
        assert!(official.unit_payload_explode);
    }

    #[test]
    fn official_waves_stamp_nova_blast_compound_and_dagger_pyratite() {
        let nova = initial_official_wave_groups(35)
            .into_iter()
            .find(|group| group.spec.unit_type == NOVA.unit_type)
            .expect("nova group at wave 35");
        assert_eq!(nova.items, vec![(14, 60)]);
        let dagger = initial_official_wave_groups(42)
            .into_iter()
            .find(|group| group.spec.unit_type == DAGGER.unit_type && !group.items.is_empty())
            .expect("dagger group at wave 42 with items");
        assert_eq!(dagger.items, vec![(15, 100)]);
        assert!(
            initial_official_wave_groups(34)
                .iter()
                .all(|group| group.spec.unit_type != NOVA.unit_type),
            "nova items group begins at wave 35"
        );
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
