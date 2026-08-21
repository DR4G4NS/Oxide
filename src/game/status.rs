//! P0-06/07/08 — StatusEntry runtime, vanilla transitions, and tick aggregate.
//!
//! Mirrors Mindustry 158.1 `StatusEntry` + `StatusComp.apply/update` without
//! porting the Java object graph. Insertion order is authoritative. Legacy
//! `(effect, duration)` JSON arrays still deserialize.

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Official `StatusEntry` (effect, remaining time, interval-damage timer,
/// optional dynamic multipliers).
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveStatus {
    pub effect: i16,
    pub time: f32,
    pub damage_time: f32,
    pub dynamic: Option<DynamicStatus>,
}

impl ActiveStatus {
    pub fn simple(effect: i16, time: f32) -> Self {
        Self {
            effect,
            time,
            damage_time: 0.0,
            dynamic: None,
        }
    }
}

impl From<(i16, f32)> for ActiveStatus {
    fn from((effect, time): (i16, f32)) -> Self {
        Self::simple(effect, time)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DynamicStatus {
    pub speed: f32,
    pub health: f32,
    pub damage: f32,
    pub reload: f32,
    pub build_speed: f32,
    pub drag: f32,
    pub armor_override: Option<f32>,
}

impl Default for DynamicStatus {
    fn default() -> Self {
        Self {
            speed: 1.0,
            health: 1.0,
            damage: 1.0,
            reload: 1.0,
            build_speed: 1.0,
            drag: 1.0,
            armor_override: None,
        }
    }
}

impl Serialize for ActiveStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.damage_time == 0.0 && self.dynamic.is_none() {
            return (self.effect, self.time).serialize(serializer);
        }
        let mut state = serializer.serialize_struct("ActiveStatus", 4)?;
        state.serialize_field("effect", &self.effect)?;
        state.serialize_field("time", &self.time)?;
        state.serialize_field("damage_time", &self.damage_time)?;
        state.serialize_field("dynamic", &self.dynamic)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for ActiveStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StatusVisitor;
        impl<'de> Visitor<'de> for StatusVisitor {
            type Value = ActiveStatus;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a [effect, time] pair or an ActiveStatus object")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let effect = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let time = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let _ = seq.next_element::<de::IgnoredAny>()?;
                Ok(ActiveStatus::simple(effect, time))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut effect = None;
                let mut time = None;
                let mut damage_time = 0.0;
                let mut dynamic = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "effect" => effect = Some(map.next_value()?),
                        "time" => time = Some(map.next_value()?),
                        "damage_time" => damage_time = map.next_value()?,
                        "dynamic" => dynamic = map.next_value()?,
                        _ => {
                            let _ = map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(ActiveStatus {
                    effect: effect.ok_or_else(|| de::Error::missing_field("effect"))?,
                    time: time.ok_or_else(|| de::Error::missing_field("time"))?,
                    damage_time,
                    dynamic,
                })
            }
        }
        deserializer.deserialize_any(StatusVisitor)
    }
}

/// Per-tick composed multipliers (`StatusComp` transients).
///
/// Gameplay consumers (158.1):
/// - `health` — spawn/heal cap (`enemy_max_health`) and incoming damage
///   (`ShieldComp.damage` divides by healthMultiplier)
/// - `speed` — `effective_unit_speed`
/// - `damage` — `effective_unit_damage_multiplier`
/// - `reload` — `effective_unit_reload_delta`
/// - `build_speed` — `effective_unit_build_speed` / BuilderComp
/// - `drag` — vanilla statuses are always 1.0; movement is kinematic so
///   Java `VelComp` drag integration has no counterpart. Dynamic
///   `statusDrag` is stored on the aggregate for dumps and future vel work.
/// - `armor_override` — `unit_effective_armor` / ShieldComp
/// - `disarmed` — `unit_can_shoot` only; reload still ticks in 158.1
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusAggregate {
    pub health: f32,
    pub speed: f32,
    pub damage: f32,
    pub reload: f32,
    pub build_speed: f32,
    pub drag: f32,
    pub armor_override: Option<f32>,
    pub disarmed: bool,
}

impl Default for StatusAggregate {
    fn default() -> Self {
        Self {
            health: 1.0,
            speed: 1.0,
            damage: 1.0,
            reload: 1.0,
            build_speed: 1.0,
            drag: 1.0,
            armor_override: None,
            disarmed: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StatusSpec {
    pub speed: f32,
    pub damage: f32,
    pub reload: f32,
    pub health: f32,
    pub build_speed: f32,
    pub drag: f32,
    pub damage_per_tick: f32,
    pub interval_damage: f32,
    pub interval_damage_time: f32,
    pub interval_pierce: bool,
    pub permanent: bool,
    pub reactive: bool,
    pub disarm: bool,
}

impl Default for StatusSpec {
    fn default() -> Self {
        Self {
            speed: 1.0,
            damage: 1.0,
            reload: 1.0,
            health: 1.0,
            build_speed: 1.0,
            drag: 1.0,
            damage_per_tick: 0.0,
            interval_damage: 0.0,
            interval_damage_time: 0.0,
            interval_pierce: false,
            permanent: false,
            reactive: false,
            disarm: false,
        }
    }
}

pub const STATUS_NONE: i16 = 0;
pub const STATUS_BURNING: i16 = 1;
pub const STATUS_FREEZING: i16 = 2;
pub const STATUS_UNMOVING: i16 = 3;
pub const STATUS_SLOW: i16 = 4;
pub const STATUS_FAST: i16 = 5;
pub const STATUS_WET: i16 = 6;
pub const STATUS_MUDDY: i16 = 7;
pub const STATUS_MELTING: i16 = 8;
pub const STATUS_SAPPED: i16 = 9;
pub const STATUS_ELECTRIFIED: i16 = 10;
pub const STATUS_SPORE_SLOWED: i16 = 11;
pub const STATUS_TARRED: i16 = 12;
pub const STATUS_OVERDRIVE: i16 = 13;
pub const STATUS_OVERCLOCK: i16 = 14;
pub const STATUS_SHIELDED: i16 = 15;
pub const STATUS_BOSS: i16 = 16;
pub const STATUS_SHOCKED: i16 = 17;
pub const STATUS_BLASTED: i16 = 18;
pub const STATUS_CORRODED: i16 = 19;
pub const STATUS_DISARMED: i16 = 20;
pub const STATUS_INVINCIBLE: i16 = 21;
pub const STATUS_DYNAMIC: i16 = 22;

pub fn status_spec(id: i16) -> StatusSpec {
    match id {
        STATUS_BURNING => StatusSpec {
            damage_per_tick: 0.167,
            ..StatusSpec::default()
        },
        STATUS_FREEZING => StatusSpec {
            speed: 0.6,
            health: 0.8,
            ..StatusSpec::default()
        },
        STATUS_UNMOVING => StatusSpec {
            speed: 0.0,
            ..StatusSpec::default()
        },
        STATUS_SLOW => StatusSpec {
            speed: 0.4,
            ..StatusSpec::default()
        },
        STATUS_FAST => StatusSpec {
            speed: 1.6,
            ..StatusSpec::default()
        },
        STATUS_WET => StatusSpec {
            speed: 0.94,
            ..StatusSpec::default()
        },
        STATUS_MUDDY => StatusSpec {
            speed: 0.94,
            ..StatusSpec::default()
        },
        STATUS_MELTING => StatusSpec {
            speed: 0.8,
            health: 0.8,
            damage_per_tick: 0.3,
            ..StatusSpec::default()
        },
        STATUS_SAPPED => StatusSpec {
            speed: 0.7,
            health: 0.8,
            ..StatusSpec::default()
        },
        STATUS_ELECTRIFIED => StatusSpec {
            speed: 0.7,
            reload: 0.6,
            ..StatusSpec::default()
        },
        STATUS_SPORE_SLOWED => StatusSpec {
            speed: 0.8,
            ..StatusSpec::default()
        },
        STATUS_TARRED => StatusSpec {
            speed: 0.6,
            ..StatusSpec::default()
        },
        STATUS_OVERDRIVE => StatusSpec {
            speed: 1.15,
            damage: 1.4,
            health: 0.95,
            damage_per_tick: -0.01,
            permanent: true,
            ..StatusSpec::default()
        },
        STATUS_OVERCLOCK => StatusSpec {
            speed: 1.15,
            damage: 1.15,
            reload: 1.25,
            ..StatusSpec::default()
        },
        STATUS_SHIELDED => StatusSpec {
            health: 3.0,
            ..StatusSpec::default()
        },
        STATUS_BOSS => StatusSpec {
            damage: 1.3,
            health: 1.5,
            permanent: true,
            ..StatusSpec::default()
        },
        STATUS_SHOCKED | STATUS_BLASTED => StatusSpec {
            reactive: true,
            ..StatusSpec::default()
        },
        STATUS_CORRODED => StatusSpec {
            interval_damage: 20.0,
            interval_damage_time: 15.0,
            ..StatusSpec::default()
        },
        STATUS_DISARMED => StatusSpec {
            disarm: true,
            ..StatusSpec::default()
        },
        STATUS_INVINCIBLE => StatusSpec {
            health: f32::INFINITY,
            ..StatusSpec::default()
        },
        STATUS_DYNAMIC => StatusSpec {
            permanent: true,
            ..StatusSpec::default()
        },
        _ => StatusSpec::default(),
    }
}

pub fn is_permanent(entry: &ActiveStatus) -> bool {
    status_spec(entry.effect).permanent || entry.time == f32::MAX
}

/// Immediate transition damage produced by `apply`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatusReaction {
    pub pierce_damage: f32,
    pub damage: f32,
}

/// `StatusComp.apply` 158.1: first-match in insertion order.
pub fn apply_status(
    statuses: &mut Vec<ActiveStatus>,
    incoming: i16,
    incoming_time: f32,
    immune: bool,
) -> StatusReaction {
    if incoming < 0 || incoming == STATUS_NONE || immune {
        return StatusReaction::default();
    }
    for existing in statuses.iter_mut() {
        if existing.effect == incoming {
            existing.time = existing.time.max(incoming_time);
            return StatusReaction::default();
        }
        if let Some(reaction) = apply_status_transition(existing, incoming, incoming_time) {
            return reaction;
        }
    }
    if status_spec(incoming).reactive {
        return StatusReaction::default();
    }
    statuses.push(ActiveStatus::simple(incoming, incoming_time));
    StatusReaction::default()
}

/// Vanilla 158.1 `StatusEffect.applyTransition` table.
pub fn apply_status_transition(
    existing: &mut ActiveStatus,
    incoming: i16,
    incoming_time: f32,
) -> Option<StatusReaction> {
    match (existing.effect, incoming) {
        (STATUS_BURNING, STATUS_TARRED) => {
            existing.time = (existing.time + incoming_time).min(300.0);
            Some(StatusReaction {
                pierce_damage: 8.0,
                damage: 0.0,
            })
        }
        (STATUS_TARRED, STATUS_BURNING) => {
            existing.effect = STATUS_BURNING;
            existing.time += incoming_time;
            Some(StatusReaction::default())
        }
        (STATUS_MELTING, STATUS_TARRED) => {
            existing.time = (existing.time + incoming_time).min(200.0);
            Some(StatusReaction {
                pierce_damage: 8.0,
                damage: 0.0,
            })
        }
        (STATUS_TARRED, STATUS_MELTING) => {
            existing.effect = STATUS_MELTING;
            existing.time += incoming_time;
            Some(StatusReaction::default())
        }
        (STATUS_WET, STATUS_SHOCKED) => Some(StatusReaction {
            pierce_damage: 0.0,
            damage: 14.0,
        }),
        (STATUS_FREEZING, STATUS_BLASTED) => Some(StatusReaction {
            pierce_damage: 18.0,
            damage: 0.0,
        }),
        (left, right) if are_opposites(left, right) => {
            existing.time -= incoming_time * 0.5;
            if existing.time <= 0.0 {
                existing.effect = incoming;
                existing.time = incoming_time;
                existing.damage_time = 0.0;
            }
            Some(StatusReaction::default())
        }
        _ => None,
    }
}

fn are_opposites(left: i16, right: i16) -> bool {
    matches!(
        (left, right),
        (STATUS_BURNING, STATUS_WET)
            | (STATUS_WET, STATUS_BURNING)
            | (STATUS_BURNING, STATUS_FREEZING)
            | (STATUS_FREEZING, STATUS_BURNING)
            | (STATUS_FREEZING, STATUS_MELTING)
            | (STATUS_MELTING, STATUS_FREEZING)
            | (STATUS_WET, STATUS_MELTING)
            | (STATUS_MELTING, STATUS_WET)
            | (STATUS_SLOW, STATUS_FAST)
            | (STATUS_FAST, STATUS_SLOW)
    )
}

/// Damage applied while ticking this frame (pierce, then normal).
#[derive(Clone, Copy, Debug, Default)]
pub struct StatusTickDamage {
    pub pierce: f32,
    pub normal: f32,
    pub changed: bool,
}

/// `StatusComp.update` body for the collection (floor reapply is separate).
pub fn tick_statuses(statuses: &mut Vec<ActiveStatus>, delta: f32) -> StatusTickDamage {
    let delta = delta.max(0.0);
    let mut damage = StatusTickDamage::default();
    let mut index = 0;
    while index < statuses.len() {
        let permanent = is_permanent(&statuses[index]);
        let before = statuses[index].time;
        statuses[index].time = (statuses[index].time - delta).max(0.0);
        if statuses[index].time != before {
            damage.changed = true;
        }
        if statuses[index].time <= 0.0 && !permanent {
            statuses.remove(index);
            damage.changed = true;
            continue;
        }
        let spec = status_spec(statuses[index].effect);
        if spec.damage_per_tick > 0.0 {
            damage.pierce += spec.damage_per_tick * delta;
        }
        // Negative damage_per_tick is heal; applied as negative normal so
        // callers can add it to health.
        if spec.damage_per_tick < 0.0 {
            damage.normal += spec.damage_per_tick * delta;
        }
        if spec.interval_damage_time > 0.0 && spec.interval_damage > 0.0 {
            statuses[index].damage_time += delta;
            if statuses[index].damage_time >= spec.interval_damage_time {
                statuses[index].damage_time %= spec.interval_damage_time;
                if spec.interval_pierce {
                    damage.pierce += spec.interval_damage;
                } else {
                    damage.normal += spec.interval_damage;
                }
            }
        }
        index += 1;
    }
    damage
}

pub fn aggregate_statuses(statuses: &[ActiveStatus]) -> StatusAggregate {
    let mut agg = StatusAggregate::default();
    for entry in statuses {
        if let Some(dynamic) = &entry.dynamic {
            agg.speed *= dynamic.speed;
            agg.health *= dynamic.health;
            agg.damage *= dynamic.damage;
            agg.reload *= dynamic.reload;
            agg.build_speed *= dynamic.build_speed;
            agg.drag *= dynamic.drag;
            if let Some(armor) = dynamic.armor_override {
                agg.armor_override = Some(armor);
            }
        } else {
            let spec = status_spec(entry.effect);
            agg.speed *= spec.speed;
            agg.health *= spec.health;
            agg.damage *= spec.damage;
            agg.reload *= spec.reload;
            agg.build_speed *= spec.build_speed;
            agg.drag *= spec.drag;
            agg.disarmed |= spec.disarm;
        }
    }
    agg
}

/// Compatibility 3-tuple used by spawn-health scaling: `(health, speed, damage)`.
pub fn status_multipliers(status_effect: i16) -> (f32, f32, f32) {
    let spec = status_spec(status_effect);
    (spec.health, spec.speed, spec.damage)
}

pub fn status_multipliers_composite(
    legacy_effect: i16,
    statuses: &[ActiveStatus],
) -> (f32, f32, f32) {
    let agg = if statuses.is_empty() && legacy_effect >= 0 {
        aggregate_statuses(&[ActiveStatus::simple(legacy_effect, 1.0)])
    } else {
        aggregate_statuses(statuses)
    };
    (agg.health, agg.speed, agg.damage)
}

pub fn sync_legacy_view(statuses: &[ActiveStatus]) -> (i16, f32) {
    match statuses.first() {
        Some(entry) => (entry.effect, entry.time),
        None => (-1, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_effect_keeps_max_and_damage_time() {
        let mut statuses = vec![ActiveStatus {
            effect: STATUS_BURNING,
            time: 40.0,
            damage_time: 7.0,
            dynamic: None,
        }];
        apply_status(&mut statuses, STATUS_BURNING, 20.0, false);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].time, 40.0);
        assert_eq!(statuses[0].damage_time, 7.0);
        apply_status(&mut statuses, STATUS_BURNING, 90.0, false);
        assert_eq!(statuses[0].time, 90.0);
        assert_eq!(statuses[0].damage_time, 7.0);
    }

    #[test]
    fn insertion_order_and_independent_damage_time() {
        let mut statuses = Vec::new();
        apply_status(&mut statuses, STATUS_FAST, 10.0, false);
        apply_status(&mut statuses, STATUS_SAPPED, 20.0, false);
        statuses[0].damage_time = 1.0;
        statuses[1].damage_time = 2.0;
        assert_eq!(statuses[0].effect, STATUS_FAST);
        assert_eq!(statuses[1].effect, STATUS_SAPPED);
        statuses.remove(0);
        assert_eq!(statuses[0].effect, STATUS_SAPPED);
        assert_eq!(statuses[0].damage_time, 2.0);
    }

    #[test]
    fn permanent_zero_time_stays() {
        let mut statuses = vec![ActiveStatus::simple(STATUS_OVERDRIVE, 0.0)];
        let tick = tick_statuses(&mut statuses, 10.0);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].time, 0.0);
        assert!(!tick.changed || statuses[0].time == 0.0);
    }

    #[test]
    fn burning_plus_tarred_caps_and_pierces() {
        let mut statuses = vec![ActiveStatus::simple(STATUS_BURNING, 200.0)];
        let reaction = apply_status(&mut statuses, STATUS_TARRED, 200.0, false);
        assert_eq!(reaction.pierce_damage, 8.0);
        assert_eq!(statuses[0].effect, STATUS_BURNING);
        assert_eq!(statuses[0].time, 300.0);
        assert_eq!(statuses.len(), 1);
    }

    #[test]
    fn tarred_plus_burning_has_no_cap_or_damage() {
        let mut statuses = vec![ActiveStatus::simple(STATUS_TARRED, 200.0)];
        let reaction = apply_status(&mut statuses, STATUS_BURNING, 200.0, false);
        assert_eq!(reaction, StatusReaction::default());
        assert_eq!(statuses[0].effect, STATUS_BURNING);
        assert_eq!(statuses[0].time, 400.0);
    }

    #[test]
    fn melting_plus_tarred_caps_at_200() {
        let mut statuses = vec![ActiveStatus::simple(STATUS_MELTING, 150.0)];
        let reaction = apply_status(&mut statuses, STATUS_TARRED, 100.0, false);
        assert_eq!(reaction.pierce_damage, 8.0);
        assert_eq!(statuses[0].time, 200.0);
    }

    #[test]
    fn wet_shocked_and_freezing_blasted() {
        let mut wet = vec![ActiveStatus::simple(STATUS_WET, 60.0)];
        assert_eq!(
            apply_status(&mut wet, STATUS_SHOCKED, 1.0, false).damage,
            14.0
        );
        assert_eq!(wet[0].effect, STATUS_WET);
        let mut freeze = vec![ActiveStatus::simple(STATUS_FREEZING, 60.0)];
        assert_eq!(
            apply_status(&mut freeze, STATUS_BLASTED, 1.0, false).pierce_damage,
            18.0
        );
    }

    #[test]
    fn direct_reactive_is_noop() {
        let mut statuses = Vec::new();
        apply_status(&mut statuses, STATUS_SHOCKED, 10.0, false);
        apply_status(&mut statuses, STATUS_BLASTED, 10.0, false);
        assert!(statuses.is_empty());
    }

    #[test]
    fn opposite_first_match_and_zero() {
        let mut statuses = vec![
            ActiveStatus::simple(STATUS_SAPPED, 50.0),
            ActiveStatus::simple(STATUS_BURNING, 10.0),
        ];
        apply_status(&mut statuses, STATUS_WET, 20.0, false);
        assert_eq!(statuses[1].effect, STATUS_WET);
        assert_eq!(statuses[1].time, 20.0);
        let mut exact = vec![ActiveStatus::simple(STATUS_BURNING, 5.0)];
        apply_status(&mut exact, STATUS_WET, 10.0, false);
        assert_eq!(exact[0].effect, STATUS_WET);
        assert_eq!(exact[0].time, 10.0);
    }

    #[test]
    fn overdrive_is_damage_not_reload() {
        let agg = aggregate_statuses(&[
            ActiveStatus::simple(STATUS_FAST, 1.0),
            ActiveStatus::simple(STATUS_OVERDRIVE, 1.0),
            ActiveStatus::simple(STATUS_SAPPED, 1.0),
        ]);
        assert!((agg.speed - 1.6 * 1.15 * 0.7).abs() < 1e-5);
        assert!((agg.damage - 1.4).abs() < 1e-6);
        assert!((agg.reload - 1.0).abs() < 1e-6);
        let clock = aggregate_statuses(&[ActiveStatus::simple(STATUS_OVERCLOCK, 1.0)]);
        assert!((clock.damage - 1.15).abs() < 1e-6);
        assert!((clock.reload - 1.25).abs() < 1e-6);
    }

    #[test]
    fn corroded_interval_modulo_once() {
        let mut statuses = vec![ActiveStatus::simple(STATUS_CORRODED, 100.0)];
        let tick = tick_statuses(&mut statuses, 40.0);
        assert!((tick.normal - 20.0).abs() < 1e-4);
        assert!((statuses[0].damage_time - (40.0 % 15.0)).abs() < 1e-4);
    }

    #[test]
    fn json_pair_roundtrip() {
        let raw = serde_json::to_string(&ActiveStatus::simple(5, 60.0)).unwrap();
        assert_eq!(raw, "[5,60.0]");
        let parsed: ActiveStatus = serde_json::from_str("[5,60.0]").unwrap();
        assert_eq!(parsed, ActiveStatus::simple(5, 60.0));
        let full: ActiveStatus =
            serde_json::from_str(r#"{"effect":19,"time":30.0,"damage_time":4.0}"#).unwrap();
        assert_eq!(full.effect, 19);
        assert_eq!(full.damage_time, 4.0);
    }

    #[test]
    fn first_match_ignores_later_opposites() {
        let mut statuses = vec![
            ActiveStatus::simple(STATUS_SAPPED, 50.0),
            ActiveStatus::simple(STATUS_BURNING, 10.0),
        ];
        apply_status(&mut statuses, STATUS_WET, 20.0, false);
        assert_eq!(statuses[0].effect, STATUS_SAPPED);
        assert_eq!(statuses[1].effect, STATUS_WET);
        assert_eq!(statuses[1].time, 20.0);
    }

    #[test]
    fn corroded_interval_phases_and_expiry_on_fire_tick() {
        let mut statuses = vec![ActiveStatus::simple(STATUS_CORRODED, 100.0)];
        let tick = tick_statuses(&mut statuses, 14.0);
        assert_eq!(tick.normal, 0.0);
        assert!((statuses[0].damage_time - 14.0).abs() < 1e-4);
        let tick = tick_statuses(&mut statuses, 1.0);
        assert!((tick.normal - 20.0).abs() < 1e-4);
        assert!((statuses[0].damage_time).abs() < 1e-4);
        let tick = tick_statuses(&mut statuses, 15.0);
        assert!((tick.normal - 20.0).abs() < 1e-4);

        let mut expiring = vec![ActiveStatus::simple(STATUS_CORRODED, 5.0)];
        let tick = tick_statuses(&mut expiring, 10.0);
        assert!(expiring.is_empty());
        assert_eq!(
            tick.normal, 0.0,
            "expiry on the fire tick skips interval damage"
        );
    }

    #[test]
    fn infinity_and_permanent_survive_huge_delta() {
        let mut statuses = vec![
            ActiveStatus::simple(STATUS_OVERDRIVE, f32::INFINITY),
            ActiveStatus::simple(STATUS_BOSS, 0.0),
        ];
        tick_statuses(&mut statuses, 1_000_000.0);
        assert_eq!(statuses.len(), 2);
        assert!(statuses[0].time.is_infinite());
        assert_eq!(statuses[0].effect, STATUS_OVERDRIVE);
        assert_eq!(statuses[1].effect, STATUS_BOSS);
        assert_eq!(statuses[1].time, 0.0);
    }

    #[test]
    fn vanilla_drag_is_identity_dynamic_is_the_only_override() {
        for id in 0..=STATUS_INVINCIBLE {
            assert_eq!(status_spec(id).drag, 1.0);
            assert_eq!(status_spec(id).build_speed, 1.0);
        }
        let dynamic = ActiveStatus {
            effect: STATUS_DYNAMIC,
            time: f32::INFINITY,
            damage_time: 0.0,
            dynamic: Some(DynamicStatus {
                drag: 1.5,
                build_speed: 4.0,
                armor_override: Some(10.0),
                ..DynamicStatus::default()
            }),
        };
        let agg = aggregate_statuses(&[dynamic]);
        assert!((agg.drag - 1.5).abs() < 1e-6);
        assert!((agg.build_speed - 4.0).abs() < 1e-6);
        assert_eq!(agg.armor_override, Some(10.0));
    }
}
