// DM001: immutable `get` guard then exclusive `insert` on the same map.
fn dm001_get_insert(world: &mut MyWorld) {
    let tile = world.tiles.get(&1).unwrap();
    world.tiles.insert(2, DynamicTile { block: 3, ..Default::default() });
    let _ = tile;
}
