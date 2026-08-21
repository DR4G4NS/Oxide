// DM002: mutable `get_mut` guard then `insert` on the same map.
fn dm002_get_mut_insert(world: &mut MyWorld) {
    let tile = world.tiles.get_mut(&1).unwrap();
    world.tiles.insert(2, DynamicTile { ..Default::default() });
    let _ = tile;
}
