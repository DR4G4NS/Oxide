//! Official map-generator filter pipeline enough to host generated maps (audit M21).
//! Noise uses the same simplex as logic `op noise` (Simplex.raw2d).
//!
//! Filter order mirrors the editor defaults: Noise → RiverNoise → Ore → Scatter
//! → EnemySpawn (keep one spawn overlay).

use crate::logic::simplex_raw2d_seeded;

#[derive(Clone, Debug)]
pub struct GeneratedMap {
    pub width: usize,
    pub height: usize,
    pub floors: Vec<i16>,
    pub overlays: Vec<i16>,
    pub blocks: Vec<i16>,
}

struct TileGen {
    floor: i16,
    overlay: i16,
    block: i16,
}

/// Octave Simplex.noise2d stand-in: sum seeded raw2d octaves and map to ~[0, 1].
fn octave_noise(seed: i32, octaves: i32, persistence: f64, scale: f64, x: f64, y: f64) -> f64 {
    let mut amp = 1.0;
    let mut freq = scale.max(0.0001);
    let mut sum = 0.0;
    let mut max = 0.0;
    for _ in 0..octaves.max(1) {
        sum += simplex_raw2d_seeded(seed, x * freq + 10.0, y * freq + 10.0) * amp;
        max += amp;
        amp *= persistence;
        freq *= 2.0;
    }
    ((sum / max.max(0.0001)) + 1.0) * 0.5
}

fn ridged_noise(seed: i32, scale: f64, x: f64, y: f64) -> f64 {
    1.0 - simplex_raw2d_seeded(seed, x / scale + 10.0, y / scale + 10.0).abs()
}

fn scatter_chance(seed: i32, x: i32, y: i32) -> f32 {
    let mut h = seed
        .wrapping_add(x.wrapping_mul(374_761_393))
        .wrapping_add(y.wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    (h as u32 as f32) / (u32::MAX as f32)
}

/// Procedural Serpulo-like terrain via the editor filter sequence.
pub fn generate_map(width: usize, height: usize, seed: i32) -> GeneratedMap {
    let width = width.max(16);
    let height = height.max(16);
    let stone = crate::game::block_names::block_id_from_name("stone").unwrap_or(33);
    let stone_wall = crate::game::block_names::block_id_from_name("stone-wall").unwrap_or(80);
    let water = crate::game::block_names::block_id_from_name("shallow-water").unwrap_or(22);
    let deep = crate::game::block_names::block_id_from_name("deep-water").unwrap_or(21);
    let copper = crate::game::block_names::block_id_from_name("ore-copper").unwrap_or(167);
    let lead = crate::game::block_names::block_id_from_name("ore-lead").unwrap_or(168);
    let coal = crate::game::block_names::block_id_from_name("ore-coal").unwrap_or(170);
    let titanium = crate::game::block_names::block_id_from_name("ore-titanium").unwrap_or(171);
    let thorium = crate::game::block_names::block_id_from_name("ore-thorium").unwrap_or(172);
    let scrap = crate::game::block_names::block_id_from_name("ore-scrap").unwrap_or(169);
    let boulder = crate::game::block_names::block_id_from_name("boulder").unwrap_or(111);
    let spawn = crate::game::block_names::block_id_from_name("spawn").unwrap_or(1);

    let mut tiles: Vec<TileGen> = (0..width * height)
        .map(|_| TileGen {
            floor: stone,
            overlay: 0,
            block: 0,
        })
        .collect();

    // NoiseFilter: stone floor, stone-wall where noise is high.
    for y in 0..height {
        for x in 0..width {
            let n = octave_noise(seed, 3, 0.5, 1.0 / 40.0, x as f64, y as f64);
            let tile = &mut tiles[y * width + x];
            if n > 0.55 {
                tile.block = stone_wall;
            }
        }
    }

    // RiverNoiseFilter: water / deep-water in ridged valleys.
    for y in 0..height {
        for x in 0..width {
            let n = ridged_noise(seed.wrapping_add(1), 40.0, x as f64, y as f64);
            if n >= 0.72 {
                let tile = &mut tiles[y * width + x];
                tile.floor = if n >= 0.82 { deep } else { water };
                tile.block = 0;
                tile.overlay = 0;
            }
        }
    }

    // OreFilter × copper/lead/coal/titanium/thorium/scrap.
    for (ore_seed, ore, threshold, scale) in [
        (seed ^ 0x51ED, copper, 0.81, 23.0),
        (seed ^ 0xC0DE, lead, 0.82, 23.0),
        (seed ^ 0x0A11, coal, 0.84, 25.0),
        (seed ^ 0x71A7, titanium, 0.86, 27.0),
        (seed ^ 0x7A07, thorium, 0.88, 29.0),
        (seed ^ 0x5C2A, scrap, 0.90, 31.0),
    ] {
        for y in 0..height {
            for x in 0..width {
                let tile = &mut tiles[y * width + x];
                if tile.floor == water || tile.floor == deep || tile.block != 0 {
                    continue;
                }
                let n = octave_noise(ore_seed, 2, 0.3, 1.0 / scale, x as f64, y as f64);
                if n > threshold {
                    tile.overlay = ore;
                }
            }
        }
    }

    // ScatterFilter: boulders on stone.
    for y in 0..height {
        for x in 0..width {
            let tile = &mut tiles[y * width + x];
            if tile.block != 0 || tile.floor != stone {
                continue;
            }
            if scatter_chance(seed ^ 0xB0B, x as i32, y as i32) <= 0.013 {
                tile.block = boulder;
            }
        }
    }

    // EnemySpawnFilter: candidate spawn overlays on coastal land, keep one.
    let mut spawns = Vec::new();
    for y in 1..height.saturating_sub(1) {
        for x in 1..width.saturating_sub(1) {
            let edge = x <= 3 || y <= 3 || x >= width - 4 || y >= height - 4;
            if !edge {
                continue;
            }
            let tile = &tiles[y * width + x];
            if tile.block == 0 && tile.floor != water && tile.floor != deep {
                spawns.push((x, y));
            }
        }
    }
    if let Some(&(sx, sy)) = spawns.first() {
        tiles[sy * width + sx].overlay = spawn;
        tiles[sy * width + sx].block = 0;
    }

    GeneratedMap {
        width,
        height,
        floors: tiles.iter().map(|tile| tile.floor).collect(),
        overlays: tiles.iter().map(|tile| tile.overlay).collect(),
        blocks: tiles.iter().map(|tile| tile.block).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_map_is_deterministic_and_has_water() {
        let a = generate_map(32, 32, 7);
        let b = generate_map(32, 32, 7);
        assert_eq!(a.floors, b.floors);
        assert!(a.floors.iter().any(|floor| *floor != a.floors[0]));
        assert_eq!(a.overlays.len(), 32 * 32);
        assert!(a.overlays.iter().any(|overlay| *overlay != 0));
        assert!(a.blocks.iter().any(|block| *block != 0));
    }
}
