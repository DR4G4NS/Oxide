//! Server-authoritative behaviour for sandbox source/void blocks.
//!
//! Desktop clients run building updates locally between block snapshots.  If
//! the server omits a source, downstream factories appear to work for six
//! seconds and are then rolled back by the next authoritative snapshot.  This
//! system owns source production so prediction and authority advance together.

use crate::network::buildings::config::{selected_item, selected_liquid};
use crate::network::world::{DynamicTile, DynamicWorld};

const ITEM_SOURCE_BLOCK: i16 = 412;
const LIQUID_SOURCE_BLOCK: i16 = 414;
const ITEM_SOURCE_INTERVAL: f32 = 60.0 / 100.0;
const LIQUID_SOURCE_CAPACITY: f32 = 10_000.0;

pub struct SandboxSystem;

impl SandboxSystem {
    /// Advances source blocks once. Void behaviour lives at the item/liquid
    /// acceptance boundary because official voids discard synchronously. The
    /// item sink is an injected port: this domain never depends on the network
    /// listener that coordinates it.
    pub fn tick(
        world: &DynamicWorld,
        delta_ticks: f32,
        mut accept_item: impl FnMut(&DynamicWorld, i32, i16, Option<i32>) -> bool,
    ) -> bool {
        let delta_ticks = delta_ticks.max(0.0);
        Self::tick_item_sources(world, delta_ticks, &mut accept_item)
            | Self::tick_liquid_sources(world)
    }

    fn tick_item_sources(
        world: &DynamicWorld,
        delta_ticks: f32,
        accept_item: &mut impl FnMut(&DynamicWorld, i32, i16, Option<i32>) -> bool,
    ) -> bool {
        let sources: Vec<DynamicTile> = world
            .tiles
            .iter()
            .filter(|tile| tile.block == ITEM_SOURCE_BLOCK)
            .map(|tile| tile.value().clone())
            .collect();
        let mut changed = false;

        for source in sources {
            let Some(Some(item)) = selected_item(&source.config) else {
                continue;
            };
            let mut counter = source.transport_progress.max(0.0) + delta_ticks;
            let attempts = (counter / ITEM_SOURCE_INTERVAL).floor().max(0.0) as usize;
            counter -= attempts as f32 * ITEM_SOURCE_INTERVAL;

            // Building.proximity contains each adjacent building once. Keep
            // the deterministic d4 order and resolve footprint cells to their
            // origin before applying the official cdump round-robin.
            let mut targets = Vec::with_capacity(4);
            for rotation in 0..4 {
                let adjacent = sandbox_offset_position(source.position, rotation);
                if let Some(target) = sandbox_dynamic_at(world, adjacent) {
                    if target.position != source.position
                        && target.team == source.team
                        && !targets.contains(&target.position)
                    {
                        targets.push(target.position);
                    }
                }
            }
            let mut dump = usize::try_from(source.unloader_offset.max(0)).unwrap_or(0);
            if !targets.is_empty() {
                dump %= targets.len();
                for _ in 0..attempts {
                    let start = dump;
                    for offset in 0..targets.len() {
                        let index = (start + offset) % targets.len();
                        dump = (dump + 1) % targets.len();
                        if accept_item(world, targets[index], item, Some(source.position)) {
                            break;
                        }
                    }
                }
            }

            if let Some(mut live) = world.tiles.get_mut(&source.position) {
                let new_dump = i16::try_from(dump).unwrap_or(0);
                changed |= (live.transport_progress - counter).abs() > f32::EPSILON
                    || live.unloader_offset != new_dump;
                live.transport_progress = counter;
                live.unloader_offset = new_dump;
                // ItemSource sets one transient item around dump(), then
                // clears it. It therefore serializes an empty ItemModule.
                live.inventory.clear();
                live.stored_item = -1;
                live.stored_amount = 0;
            }
        }
        changed
    }

    fn tick_liquid_sources(world: &DynamicWorld) -> bool {
        let sources: Vec<(i32, Option<i16>)> = world
            .tiles
            .iter()
            .filter(|tile| tile.block == LIQUID_SOURCE_BLOCK)
            .filter_map(|tile| {
                selected_liquid(&tile.config).map(|selection| (tile.position, selection))
            })
            .collect();
        let mut changed = false;
        for (position, selection) in sources {
            let Some(mut source) = world.tiles.get_mut(&position) else {
                continue;
            };
            match selection {
                Some(liquid) => {
                    changed |= source.stored_liquid != liquid
                        || (source.liquid_amount - LIQUID_SOURCE_CAPACITY).abs() > 0.0001;
                    source.stored_liquid = liquid;
                    source.liquid_amount = LIQUID_SOURCE_CAPACITY;
                    source.liquid_inventory.clear();
                    source
                        .liquid_inventory
                        .push((liquid, LIQUID_SOURCE_CAPACITY));
                }
                None => {
                    changed |= source.stored_liquid != -1 || source.liquid_amount > 0.0001;
                    source.stored_liquid = -1;
                    source.liquid_amount = 0.0;
                    source.liquid_inventory.clear();
                }
            }
        }
        changed
    }
}

fn sandbox_offset_position(position: i32, rotation: u8) -> i32 {
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    let (dx, dy) = match rotation % 4 {
        0 => (1, 0),
        1 => (0, 1),
        2 => (-1, 0),
        _ => (0, -1),
    };
    ((x + dx) << 16) | ((y + dy) as u16 as i32)
}

fn sandbox_dynamic_at(world: &DynamicWorld, position: i32) -> Option<DynamicTile> {
    world.tiles.iter().find_map(|tile| {
        ((tile.position == position || tile.occupied.contains(&position)) && tile.block != 0)
            .then(|| tile.value().clone())
    })
}
