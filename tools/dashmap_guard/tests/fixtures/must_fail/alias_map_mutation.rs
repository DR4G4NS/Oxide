// Alias of a map still canonicalizes to the same identity as the original.
fn alias_map_mutation(world: &mut MyWorld) {
    let tiles = &world.tiles;
    let g = tiles.get(&1).unwrap();
    world.tiles.insert(2, DynamicTile { ..Default::default() });
    let _ = g;
}
