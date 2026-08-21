//! Power-node configuration and placement lifecycle.
//!
//! Java materializes laser links when a powered building is placed. The power
//! graph itself then consumes those explicit links; drawing a preview does not
//! create them. Keeping this lifecycle out of packet handling lets player,
//! logic-unit and future plugin placement paths share the same invariant.

use crate::network::economy::{power_connected, power_role, PowerRole};
use crate::network::world::{DynamicTile, DynamicWorld};

pub fn node_spec(block: i16) -> Option<(f32, usize)> {
    match block {
        302 => Some((6.0, 10)),
        303 => Some((15.0, 15)),
        304 => Some((40.0, 2)),
        // LongPowerNode: configured links only (`autolink=false`), but its
        // ordinary PowerNode link validator still uses this range/capacity.
        319 => Some((500.0, 1)),
        // PowerSource extends PowerNode and keeps its default laserRange=6.
        410 => Some((6.0, 100)),
        _ => None,
    }
}

pub fn is_power_node(block: i16) -> bool {
    node_spec(block).is_some()
}

pub fn valid_configuration(config: &[u8]) -> bool {
    matches!(config, [1, _, _, _, _])
        || matches!(config, [7, _, _, _, _, _, _, _, _])
        || matches!(config, [8, count, rest @ ..]
            // PowerSource is the largest official PowerNode at 100 links.
            // Keep the wire bound aligned with node_spec so a valid source
            // schematic is never rejected or silently truncated.
            if *count <= 100 && rest.len() == *count as usize * 4)
}

fn tile_at(world: &DynamicWorld, position: i32) -> Option<DynamicTile> {
    // Round 74f: the O(1) tile lookup (exact origin + per-tick footprint
    // index); the old linear scan here ran inside the relink sweep.
    crate::network::buildings::construction::dynamic_at(world, position)
}

fn center(tile: &DynamicTile) -> (f32, f32) {
    // Building.x/y are `tile.worldx/y + block.offset`; even-sized blocks use
    // a half-tile offset while odd-sized blocks are centered on their origin.
    let offset = if crate::game::content::block_size(tile.block).is_multiple_of(2) {
        0.5
    } else {
        0.0
    };
    (
        (tile.position >> 16) as i16 as f32 + offset,
        tile.position as i16 as f32 + offset,
    )
}

fn distance_squared(left: &DynamicTile, right: &DynamicTile) -> f32 {
    let (lx, ly) = center(left);
    let (rx, ry) = center(right);
    (lx - rx).powi(2) + (ly - ry).powi(2)
}

fn footprints_adjacent(left: &DynamicTile, right: &DynamicTile) -> bool {
    left.occupied.iter().any(|left_position| {
        right.occupied.iter().any(|right_position| {
            let lx = (*left_position >> 16) as i16 as i32;
            let ly = *left_position as i16 as i32;
            let rx = (*right_position >> 16) as i16 as i32;
            let ry = *right_position as i16 as i32;
            (lx - rx).abs() + (ly - ry).abs() == 1
        })
    })
}

fn node_reaches(node: &DynamicTile, other: &DynamicTile, range: f32) -> bool {
    // PowerNode.overlaps tests the range circle against the target block's
    // hitbox. Approximate that rectangle exactly in tile coordinates instead
    // of comparing centers, which incorrectly drops large targets at the edge.
    let (nx, ny) = center(node);
    let (ox, oy) = center(other);
    let half = f32::from(crate::game::content::block_size(other.block)) / 2.0;
    let dx = ((nx - ox).abs() - half).max(0.0);
    let dy = ((ny - oy).abs() - half).max(0.0);
    dx * dx + dy * dy <= range * range
}

fn node_autolinks(block: i16) -> bool {
    // LongPowerNode sets `autolink=false`; all other official PowerNode
    // entries represented here keep the inherited true value.
    block != 319
}

fn node_requires_same_block(block: i16) -> bool {
    // LongPowerNode sets `sameBlockConnection=true`.
    block == 319
}

fn block_at(world: &DynamicWorld, position: i32) -> i16 {
    if let Some(tile) = tile_at(world, position) {
        return tile.block;
    }
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    if x < 0 || y < 0 || x >= world.width || y >= world.height {
        return 0;
    }
    world.base_blocks[(y * world.width + x) as usize]
}

pub(crate) fn is_insulated_block(block: i16) -> bool {
    // block_power.tsv generated from the local 158.1 JAR: plastanium walls
    // and PowerDiode are the only vanilla blocks with Block.insulated=true.
    matches!(block, 220 | 221 | 305)
}

/// Exact integer Bresenham used by `World.raycast`/`PowerNode.insulated`.
/// Both endpoints are visited, matching the Java implementation.
pub(crate) fn insulated_between(world: &DynamicWorld, left: i32, right: i32) -> bool {
    let mut x = (left >> 16) as i16 as i32;
    let mut y = left as i16 as i32;
    let target_x = (right >> 16) as i16 as i32;
    let target_y = right as i16 as i32;
    let dx = (target_x - x).abs();
    let sx = if x < target_x { 1 } else { -1 };
    let dy = (target_y - y).abs();
    let sy = if y < target_y { 1 } else { -1 };
    let mut error = dx - dy;
    loop {
        let position = (x << 16) | (y as u16 as i32);
        if is_insulated_block(block_at(world, position)) {
            return true;
        }
        if x == target_x && y == target_y {
            return false;
        }
        let doubled = error * 2;
        if doubled > -dy {
            error -= dy;
            x += sx;
        }
        if doubled < dx {
            error += dx;
            y += sy;
        }
    }
}

/// Mirrors `PowerNode.linkValid` for manual configuration, snapshot
/// filtering and persisted-link sanitation. Either endpoint's node range may
/// make a link valid. Java does **not** consult `insulated()` here: a player
/// can keep (or tap-configure) a laser through a plastanium wall. Autolink
/// uses [`autolink_valid_for_node`] instead, matching `getPotentialLinks`.
pub fn link_valid_for_node(world: &DynamicWorld, tile: &DynamicTile, target: i32) -> bool {
    let Some(other) = tile_at(world, target) else {
        return false;
    };
    if other.position == tile.position
        || other.team != tile.team
        // Every hasPower block in the local 158.1 metadata also has
        // connectedPower=true. `power_role` is the server's exhaustive
        // hasPower/connectedPower registry, so this is the single gate used
        // by config, relink, persisted-state sanitation and snapshot fallback.
        || power_role(other.block).is_none()
    {
        return false;
    }
    let Some((range, maximum)) = node_spec(tile.block) else {
        return false;
    };
    if node_requires_same_block(tile.block) && other.block != tile.block {
        return false;
    }
    if tile.power_links.len() >= maximum && !tile.power_links.contains(&other.position) {
        return false;
    }
    if !node_reaches(tile, &other, range)
        && node_spec(other.block)
            .is_none_or(|(other_range, _)| !node_reaches(&other, tile, other_range))
    {
        return false;
    }
    if let Some((_, maximum)) = node_spec(other.block) {
        if other.power_links.len() >= maximum && !other.power_links.contains(&tile.position) {
            return false;
        }
    }
    true
}

/// `PowerNode.getPotentialLinks` / `getNodeLinks` validity: `linkValid` plus
/// the Bresenham insulated ray. Adjacent-footprint and same-graph filters
/// stay in [`autolink_candidates`], matching Java `Edges.getEdges` / `graphs`.
pub fn autolink_valid_for_node(world: &DynamicWorld, tile: &DynamicTile, target: i32) -> bool {
    let Some(other) = tile_at(world, target) else {
        return false;
    };
    link_valid_for_node(world, tile, other.position)
        && !insulated_between(world, tile.position, other.position)
}

#[derive(Clone, Copy)]
enum ReverseLinkOp {
    Remove(i32),
    Add(i32),
}

struct LinkPlan {
    links: Vec<i32>,
    reverse_ops: Vec<ReverseLinkOp>,
}

fn toggle_link(
    world: &DynamicWorld,
    tile: &DynamicTile,
    links: &mut Vec<i32>,
    reverse_ops: &mut Vec<ReverseLinkOp>,
    requested: i32,
) {
    let target = tile_at(world, requested)
        .map(|target| target.position)
        .unwrap_or(requested);
    if links.contains(&target) {
        links.retain(|position| *position != target);
        if tile_at(world, target).is_some_and(|other| power_role(other.block).is_some()) {
            reverse_ops.push(ReverseLinkOp::Remove(target));
        }
    } else if link_valid_for_node(world, tile, target)
        && links.len() < node_spec(tile.block).map_or(0, |(_, maximum)| maximum)
    {
        links.push(target);
        if tile_at(world, target).is_some_and(|other| other.team == tile.team) {
            reverse_ops.push(ReverseLinkOp::Add(target));
        }
    }
}

fn configuration_plan(world: &DynamicWorld, tile: &DynamicTile, config: &[u8]) -> Option<LinkPlan> {
    let mut links = tile.power_links.clone();
    let mut reverse_ops = Vec::new();
    match config {
        [1, a, b, c, d] => toggle_link(
            world,
            tile,
            &mut links,
            &mut reverse_ops,
            i32::from_be_bytes([*a, *b, *c, *d]),
        ),
        [7, rest @ ..] if rest.len() == 8 => {
            for old in links.drain(..) {
                reverse_ops.push(ReverseLinkOp::Remove(old));
            }
            let dx = i32::from_be_bytes(rest[0..4].try_into().ok()?);
            let dy = i32::from_be_bytes(rest[4..8].try_into().ok()?);
            let x = (tile.position >> 16) as i16 as i32 + dx;
            let y = tile.position as i16 as i32 + dy;
            toggle_link(
                world,
                tile,
                &mut links,
                &mut reverse_ops,
                (x << 16) | (y as u16 as i32),
            );
        }
        [8, count, rest @ ..] if rest.len() == *count as usize * 4 => {
            for old in links.drain(..) {
                reverse_ops.push(ReverseLinkOp::Remove(old));
            }
            for chunk in rest.as_chunks::<4>().0.iter().take(*count as usize) {
                let packed = i32::from_be_bytes(*chunk);
                let x = (tile.position >> 16) as i16 as i32 + (packed >> 16);
                let y = tile.position as i16 as i32 + packed as i16 as i32;
                toggle_link(
                    world,
                    tile,
                    &mut links,
                    &mut reverse_ops,
                    (x << 16) | (y as u16 as i32),
                );
            }
        }
        _ => return None,
    }
    Some(LinkPlan { links, reverse_ops })
}

/// Applies one validated PowerNode config with bidirectional link updates.
pub fn apply_configuration(world: &DynamicWorld, position: i32, config: &[u8]) -> bool {
    let Some(tile) = tile_at(world, position) else {
        return false;
    };
    let Some(plan) = configuration_plan(world, &tile, config) else {
        return false;
    };
    let changed = plan.links != tile.power_links || !plan.reverse_ops.is_empty();
    if let Some(mut live) = world.tiles.get_mut(&tile.position) {
        live.power_links = plan.links;
    } else {
        return false;
    }
    for operation in plan.reverse_ops {
        let target = match operation {
            ReverseLinkOp::Remove(target) | ReverseLinkOp::Add(target) => target,
        };
        let Some(mut other) = world.tiles.get_mut(&target) else {
            continue;
        };
        match operation {
            ReverseLinkOp::Remove(_) => other.power_links.retain(|link| *link != tile.position),
            ReverseLinkOp::Add(_) => {
                if !other.power_links.contains(&tile.position) {
                    other.power_links.push(tile.position);
                }
            }
        }
    }
    changed || sync_node_config_with_links(world, tile.position)
}

/// Rebuilds a power node's TypeIO Point2[] config (`[8, count, dx,dy ...]`,
/// relative packed points) from its current `power_links`.
///
/// Why this exists (round 74d, verified with the 158.1 client): the client
/// activates a node's power graph ONLY through the PowerNode config
/// handlers. `Building.add()` calls `power.graph.checkAdd()` (which adds the
/// graph's PowerGraphUpdater entity) but PowerNode/PowerSource set
/// `update=false`, so `Building.init` never calls `add()` for them — their
/// graphs keep NO updater and never simulate: the bar shows "+0/s" and the
/// client graph never merges with linked buildings. The only channels that
/// run the config handler are `ConstructFinish.config` (join replay) and
/// `TileConfig` broadcasts. Block snapshots update `power.links`/`status`
/// but do NOT reflow the client graph. Keeping `tile.config` aligned with
/// `power_links` makes the join replay (and any config forward) carry the
/// links, so client graphs merge (graph.addGraph -> checkAdd) and simulate.
pub fn sync_node_config_with_links(world: &DynamicWorld, position: i32) -> bool {
    let Some(tile) = world.tiles.get(&position) else {
        return false;
    };
    if !is_power_node(tile.block) {
        return false;
    }
    let mut config = vec![8u8, tile.power_links.len().min(255) as u8];
    let node_x = (position >> 16) as i16 as i32;
    let node_y = position as i16 as i32;
    for link in tile.power_links.iter().copied().take(255) {
        let dx = (link >> 16) as i16 as i32 - node_x;
        let dy = link as i16 as i32 - node_y;
        config.extend_from_slice(&((dx << 16) | (dy as u16 as i32)).to_be_bytes());
    }
    if tile.config == config {
        return false;
    }
    drop(tile);
    if let Some(mut live) = world.tiles.get_mut(&position) {
        live.config = config;
        true
    } else {
        false
    }
}

fn graph_components(world: &DynamicWorld, vertices: &[(DynamicTile, PowerRole)]) -> Vec<usize> {
    let mut components = vec![usize::MAX; vertices.len()];
    let mut component = 0;
    for start in 0..vertices.len() {
        if components[start] != usize::MAX {
            continue;
        }
        components[start] = component;
        let mut stack = vec![start];
        while let Some(index) = stack.pop() {
            for candidate in 0..vertices.len() {
                if components[candidate] == usize::MAX
                    && power_connected(world, &vertices[index], &vertices[candidate])
                {
                    components[candidate] = component;
                    stack.push(candidate);
                }
            }
        }
        component += 1;
    }
    components
}

fn add_bidirectional(world: &DynamicWorld, left: i32, right: i32) {
    if let Some(mut tile) = world.tiles.get_mut(&left) {
        if !tile.power_links.contains(&right) {
            tile.power_links.push(right);
        }
    }
    if let Some(mut tile) = world.tiles.get_mut(&right) {
        if !tile.power_links.contains(&left) {
            tile.power_links.push(left);
        }
    }
}

pub(crate) fn remove_bidirectional_link(world: &DynamicWorld, left: i32, right: i32) {
    if let Some(mut tile) = world.tiles.get_mut(&left) {
        tile.power_links.retain(|link| *link != right);
    }
    if let Some(mut tile) = world.tiles.get_mut(&right) {
        tile.power_links.retain(|link| *link != left);
    }
}

/// Runs `Building.placed` and `PowerNodeBuild.placed` power autolinking.
/// Returns true when at least one persistent laser link was created.
pub fn autolink_after_placement(world: &DynamicWorld, position: i32) -> bool {
    let Some(placed) = tile_at(world, position) else {
        return false;
    };
    if power_role(placed.block).is_none() {
        return false;
    }
    if is_power_node(placed.block) && !node_autolinks(placed.block) {
        return sync_node_config_with_links(world, placed.position);
    }
    // PowerNodeBuild.placed does nothing when a schematic/config already
    // supplied links.
    if is_power_node(placed.block) && !placed.power_links.is_empty() {
        return false;
    }
    let linked = autolink_candidates(world, &placed);
    let config_synced = sync_node_config_with_links(world, placed.position);
    linked || config_synced
}

/// Self-healing relink for power nodes/sources (round 74): prunes stale
/// links (targets that were deconstructed or lost their power role) and
/// tops the link list up from nearby buildings, mirroring the official
/// placement autolink. Runs periodically from the world loop so links
/// missed by a placement path repair themselves instead of leaving the
/// graph split (drills without power, sandbox sources that never connect).
pub fn relink_power_node(world: &DynamicWorld, position: i32) -> bool {
    let Some(placed) = tile_at(world, position) else {
        return false;
    };
    let Some((_, maximum)) = node_spec(placed.block) else {
        return false;
    };
    let mut changed = false;
    let stale: Vec<i32> = placed
        .power_links
        .iter()
        .copied()
        .filter(|target| !link_valid_for_node(world, &placed, *target))
        .collect();
    if !stale.is_empty() {
        if let Some(mut live) = world.tiles.get_mut(&position) {
            live.power_links.retain(|link| !stale.contains(link));
        }
        for target in stale {
            if let Some(mut other) = world.tiles.get_mut(&target) {
                other.power_links.retain(|link| *link != position);
            }
        }
        changed = true;
    }
    // Round 74: heal one-way links — a tile that links to this node (e.g.
    // the reverse half written by an earlier autolink on the partner side)
    // gets the mirror link here, so the node's own power_links are complete
    // and the client draws the node lasers symmetrically.
    let incoming: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| {
            tile.team == placed.team
                && tile.power_links.contains(&position)
                && power_role(tile.block).is_some()
        })
        .map(|tile| *tile.key())
        .collect();
    if !incoming.is_empty() {
        let current = tile_at(world, position).unwrap_or_else(|| placed.clone());
        let incoming: Vec<i32> = incoming
            .into_iter()
            .filter(|target| link_valid_for_node(world, &current, *target))
            .collect();
        if let Some(mut live) = world.tiles.get_mut(&position) {
            for target in incoming {
                if live.power_links.len() >= maximum {
                    break;
                }
                if !live.power_links.contains(&target) {
                    live.power_links.push(target);
                    changed = true;
                }
            }
        }
    }
    let links_len = world
        .tiles
        .get(&position)
        .map(|tile| tile.power_links.len())
        .unwrap_or(0);
    if links_len >= maximum {
        let config_synced = sync_node_config_with_links(world, position);
        if changed || config_synced {
            world
                .persistence_dirty
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        return changed || config_synced;
    }
    // Refresh after pruning/healing. The original snapshot can still contain
    // `maximum` stale links, which would make the shared validator reject
    // every valid replacement as over capacity.
    let current = tile_at(world, position).unwrap_or(placed);
    let linked = node_autolinks(current.block) && autolink_candidates(world, &current);
    let config_synced = sync_node_config_with_links(world, position);
    if changed || linked || config_synced {
        world
            .persistence_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    changed || linked || config_synced
}

/// Round 74g: instant autolink for machines placed near nodes — relinks
/// every node/source whose range covers `position`. The periodic sweep is
/// the safety net; this makes the links appear immediately at placement
/// (the official links instantly at node placement, and the user expects
/// the same when placing a machine next to a node).
pub fn relink_nearby_nodes(world: &DynamicWorld, position: i32) -> bool {
    let Some(placed) = tile_at(world, position) else {
        return false;
    };
    if power_role(placed.block).is_none() {
        return false;
    }
    let nodes: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| is_power_node(tile.block))
        .filter_map(|tile| {
            if !node_autolinks(tile.block) {
                return None;
            }
            // dashmap-guard: allow DM900 reason="autolink_valid_for_node only reads world tile data while this shared iterator guard is live; no exclusive tiles operation occurs"
            autolink_valid_for_node(world, &tile, placed.position).then_some(tile.position)
        })
        .collect();
    let mut changed = false;
    for node in nodes {
        changed |= relink_power_node(world, node);
    }
    changed
}

fn node_covers_cell(node: &DynamicTile, cell: i32, range: f32) -> bool {
    // Treat the cell as a size-1 hitbox so a node whose laser circle covers
    // any footprint tile of a just-removed insulated wall is eligible to
    // autolink through the gap this tick.
    let (nx, ny) = center(node);
    let cx = (cell >> 16) as i16 as f32;
    let cy = cell as i16 as f32;
    let dx = ((nx - cx).abs() - 0.5).max(0.0);
    let dy = ((ny - cy).abs() - 0.5).max(0.0);
    dx * dx + dy * dy <= range * range
}

/// Same-tick restore after an insulated obstacle disappears. PowerNode has
/// `update=false` in 158.1, so Java only forms new lasers on the next
/// `placed()`; the dedicated server's relink pass is that update. Restrict
/// the sweep to nodes whose range covers a removed cell so a wall break
/// does not walk every node on the map.
pub fn relink_autolink_nodes_near(world: &DynamicWorld, cells: &[i32]) -> bool {
    if cells.is_empty() {
        return false;
    }
    let nodes: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| is_power_node(tile.block) && node_autolinks(tile.block))
        .filter_map(|tile| {
            let (range, _) = node_spec(tile.block)?;
            cells
                .iter()
                .any(|cell| node_covers_cell(&tile, *cell, range))
                .then_some(tile.position)
        })
        .collect();
    let mut changed = false;
    for node in nodes {
        changed |= relink_power_node(world, node);
    }
    changed
}

/// Same-tick restore after an insulated obstacle disappears. PowerNode has
/// `update=false` in 158.1, so Java only forms new lasers on the next
/// `placed()`; the dedicated server's relink pass is that update.
pub fn relink_after_insulated_removed(
    world: &DynamicWorld,
    origin: i32,
    block: i16,
    occupied: &[i32],
) -> bool {
    if !is_insulated_block(block) {
        return false;
    }
    let cells = if occupied.is_empty() {
        vec![origin]
    } else {
        occupied.to_vec()
    };
    relink_autolink_nodes_near(world, &cells)
}

/// Repairs persisted PowerNode state before a loaded world is published.
/// Links are canonical, bidirectional, within the node's capacity and mirrored
/// by the TypeIO Point2[] config that activates the client-side graph.
pub fn normalize_power_links(world: &DynamicWorld) -> Vec<(i32, Vec<u8>)> {
    let mut nodes: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| is_power_node(tile.block))
        .map(|tile| tile.position)
        .collect();
    nodes.sort_unstable();
    let before: std::collections::HashMap<i32, (Vec<i32>, Vec<u8>)> = nodes
        .iter()
        .filter_map(|position| {
            world
                .tiles
                .get(position)
                .map(|tile| (*position, (tile.power_links.clone(), tile.config.clone())))
        })
        .collect();

    let sanitize = |position: i32| {
        let Some(tile) = tile_at(world, position) else {
            return;
        };
        let Some((_, maximum)) = node_spec(tile.block) else {
            return;
        };
        let mut seen = std::collections::HashSet::new();
        let mut kept = Vec::new();
        let mut removed = Vec::new();
        for target in tile.power_links.iter().copied() {
            if kept.len() < maximum
                && seen.insert(target)
                && link_valid_for_node(world, &tile, target)
            {
                kept.push(target);
            } else {
                removed.push(target);
            }
        }
        if kept != tile.power_links {
            if let Some(mut live) = world.tiles.get_mut(&position) {
                live.power_links = kept.clone();
            }
        }
        for target in removed {
            if let Some(mut other) = world.tiles.get_mut(&target) {
                other.power_links.retain(|link| *link != position);
            }
        }
        for target in kept {
            if let Some(mut other) = world.tiles.get_mut(&target) {
                if !other.power_links.contains(&position) {
                    other.power_links.push(position);
                }
            }
        }
    };

    // Invalid persisted edges must be removed before autolink computes graph
    // components; otherwise an out-of-range edge can suppress the valid local
    // replacement. Sanitize again after healing to enforce source capacity on
    // incoming reverse links.
    for position in &nodes {
        sanitize(*position);
    }
    for position in &nodes {
        relink_power_node(world, *position);
    }
    for position in &nodes {
        sanitize(*position);
    }
    for position in &nodes {
        sync_node_config_with_links(world, *position);
    }

    let updates: Vec<_> = nodes
        .into_iter()
        .filter_map(|position| {
            world.tiles.get(&position).and_then(|tile| {
                let changed = before.get(&position).is_none_or(|(links, config)| {
                    links != &tile.power_links || config != &tile.config
                });
                changed.then(|| (position, tile.config.clone()))
            })
        })
        .collect();
    if !updates.is_empty() {
        world
            .persistence_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    updates
}

fn autolink_candidates(world: &DynamicWorld, placed: &DynamicTile) -> bool {
    if power_role(placed.block).is_none() {
        return false;
    }

    let vertices: Vec<(DynamicTile, PowerRole)> = world
        .tiles
        .iter()
        .filter_map(|tile| power_role(tile.block).map(|role| (tile.value().clone(), role)))
        .collect();
    let Some(placed_index) = vertices
        .iter()
        .position(|(tile, _)| tile.position == placed.position)
    else {
        return false;
    };
    let components = graph_components(world, &vertices);
    let placed_component = components[placed_index];
    let mut candidates: Vec<usize> = vertices
        .iter()
        .enumerate()
        .filter_map(|(index, (other, _))| {
            if index == placed_index
                || other.team != placed.team
                || components[index] == placed_component
                || footprints_adjacent(placed, other)
            {
                return None;
            }
            let valid = if node_spec(placed.block).is_some() && node_autolinks(placed.block) {
                autolink_valid_for_node(world, placed, other.position)
            } else if node_spec(other.block).is_some() && node_autolinks(other.block) {
                autolink_valid_for_node(world, other, placed.position)
            } else {
                false
            };
            valid.then_some(index)
        })
        .collect();
    candidates.sort_by(|left, right| {
        let left_tile = &vertices[*left].0;
        let right_tile = &vertices[*right].0;
        // Official ordering prefers nodes, then nearest buildings.
        is_power_node(right_tile.block)
            .cmp(&is_power_node(left_tile.block))
            .then_with(|| {
                distance_squared(placed, left_tile).total_cmp(&distance_squared(placed, right_tile))
            })
    });

    let maximum = node_spec(placed.block).map_or(usize::MAX, |(_, maximum)| maximum);
    let remaining = maximum.saturating_sub(placed.power_links.len());
    let mut linked_components = std::collections::HashSet::new();
    let mut linked = 0usize;
    for index in candidates {
        if linked >= remaining || !linked_components.insert(components[index]) {
            continue;
        }
        let other = &vertices[index].0;
        if let Some((_, other_maximum)) = node_spec(other.block) {
            if other.power_links.len() >= other_maximum {
                continue;
            }
        }
        add_bidirectional(world, placed.position, other.position);
        linked += 1;
    }
    linked > 0
}

/// P0-09: strip this building from every power component. Reverse links
/// pointing at `position` are cleared so a later drop cannot resurrect them.
pub fn disconnect_building(world: &DynamicWorld, position: i32) {
    let links = world
        .tiles
        .get(&position)
        .map(|tile| tile.power_links.clone())
        .unwrap_or_default();
    if let Some(mut live) = world.tiles.get_mut(&position) {
        live.power_links.clear();
    }
    for target in links {
        if let Some(mut other) = world.tiles.get_mut(&target) {
            other.power_links.retain(|link| *link != position);
            drop(other);
            sync_node_config_with_links(world, target);
        }
    }
    let reverse: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| tile.position != position && tile.power_links.contains(&position))
        .map(|tile| tile.position)
        .collect();
    for other_pos in reverse {
        if let Some(mut other) = world.tiles.get_mut(&other_pos) {
            other.power_links.retain(|link| *link != position);
        }
        sync_node_config_with_links(world, other_pos);
    }
}
