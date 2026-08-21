// DM004: helper takes an explicit DashMap argument (`tiles`) that aliases
// the caller's `world.tiles`. Spec §7: record effects on DashMap arguments.
fn mutate_tiles(tiles: &dashmap::DashMap<i32, DynamicTile>, pos: i32) {
    tiles.insert(pos, DynamicTile { ..Default::default() });
}
fn dm004_dashmap_param(world: &mut MyWorld) {
    let tile = world.tiles.get(&1).unwrap();
    mutate_tiles(&world.tiles, 2);
    let _ = tile;
}
