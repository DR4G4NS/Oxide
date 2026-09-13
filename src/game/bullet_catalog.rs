//! Fail-closed bullet inventory (audit C2).
//!
//! Unknown bullet ids must not silently degrade to plain direct damage. Rows
//! classified `IMPLEMENTED` *or* `DEVIATION` count as inventoried.

use std::collections::HashSet;
use std::sync::OnceLock;

static INVENTORIED: OnceLock<HashSet<i16>> = OnceLock::new();

#[derive(Clone, Copy, Debug)]
pub struct BulletCollision {
    pub collides: bool,
    pub air: bool,
    pub ground: bool,
    pub tiles: bool,
    pub team: bool,
    pub hit_size: f32,
    pub scale_life: bool,
    pub despawn_hit: bool,
}

pub fn bullet_collision(id: i16) -> BulletCollision {
    static COLLISION: OnceLock<Vec<BulletCollision>> = OnceLock::new();
    let rows = COLLISION.get_or_init(|| {
        include_str!("bullet_collision.tsv")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .enumerate()
            .map(|(index, line)| {
                let mut fields = line.split('\t');
                assert_eq!(fields.next().unwrap().parse::<usize>().unwrap(), index);
                BulletCollision {
                    collides: fields.next().unwrap().parse().unwrap(),
                    air: fields.next().unwrap().parse().unwrap(),
                    ground: fields.next().unwrap().parse().unwrap(),
                    tiles: fields.next().unwrap().parse().unwrap(),
                    team: fields.next().unwrap().parse().unwrap(),
                    hit_size: fields.next().unwrap().parse().unwrap(),
                    scale_life: fields.next().unwrap().parse().unwrap(),
                    despawn_hit: fields.next().unwrap().parse().unwrap(),
                }
            })
            .collect()
    });
    usize::try_from(id)
        .ok()
        .and_then(|i| rows.get(i))
        .copied()
        .unwrap_or(BulletCollision {
            collides: false,
            air: false,
            ground: false,
            tiles: false,
            team: false,
            hit_size: 0.0,
            scale_life: false,
            despawn_hit: false,
        })
}

fn parse_id_token(token: &str, out: &mut HashSet<i16>) {
    let token = token.trim();
    if token.is_empty() {
        return;
    }
    if let Some((lo, hi)) = token.split_once('-') {
        if let (Ok(lo), Ok(hi)) = (lo.parse::<i16>(), hi.parse::<i16>()) {
            let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            for id in a..=b {
                out.insert(id);
            }
            return;
        }
    }
    if token.contains('+') {
        for part in token.split('+') {
            parse_id_token(part, out);
        }
        return;
    }
    if token.contains('/') {
        for part in token.split('/') {
            parse_id_token(part, out);
        }
        return;
    }
    if let Ok(id) = token.parse::<i16>() {
        out.insert(id);
    }
}

fn inventory() -> &'static HashSet<i16> {
    INVENTORIED.get_or_init(|| {
        let mut ids = HashSet::new();
        for line in include_str!("bullet_inventory.tsv").lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let Some(id_field) = line.split('\t').next() else {
                continue;
            };
            parse_id_token(id_field, &mut ids);
        }
        ids
    })
}

/// True when `bullet_id` appears in the inventory TSV (including DEVIATION).
pub fn bullet_is_inventoried(bullet_id: i16) -> bool {
    inventory().contains(&bullet_id)
}

/// Direct/splash damage applied for an unmodeled id is zero (fail-closed).
pub fn inventoried_damage(bullet_id: i16, damage: f32) -> f32 {
    if bullet_is_inventoried(bullet_id) {
        damage
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventoried_family_ranges_expand() {
        assert!(bullet_is_inventoried(6));
        assert!(bullet_is_inventoried(61));
        assert!(bullet_is_inventoried(64));
        assert!(bullet_is_inventoried(113));
        assert!(!bullet_is_inventoried(9999));
        assert_eq!(inventoried_damage(9999, 40.0), 0.0);
        assert!(inventoried_damage(6, 11.0) > 0.0);
    }

    #[test]
    fn unit_weapon_bullet_ids_are_inventoried() {
        for line in include_str!("unit_weapons.tsv").lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let id: i16 = line.split('\t').nth(5).unwrap().parse().unwrap();
            assert!(
                bullet_is_inventoried(id),
                "unit weapon bullet {id} is missing from bullet_inventory.tsv (C2 fail-closed)"
            );
        }
    }

    #[test]
    fn lightning_wire_ids_are_inventoried() {
        for id in 1..=3 {
            assert!(
                bullet_is_inventoried(id),
                "lightning wire id {id} is missing from bullet_inventory.tsv (C2 fail-closed)"
            );
        }
    }

    #[test]
    fn scathe_family_bullet_ids_are_inventoried() {
        for id in 186..=195 {
            assert!(
                bullet_is_inventoried(id),
                "scathe-family bullet {id} is missing from bullet_inventory.tsv (C2 fail-closed)"
            );
        }
    }
}
