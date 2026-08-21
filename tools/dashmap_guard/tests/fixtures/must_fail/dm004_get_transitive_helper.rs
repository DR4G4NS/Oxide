// DM004: `get` guard held across a helper that inserts into the same map.
fn setblock_helper(world: &mut MyWorld, pos: i32, block: i16) {
    world.tiles.insert(pos, DynamicTile { block, ..Default::default() });
}
fn dm004_get_transitive_helper(world: &mut MyWorld) {
    let tile = world.tiles.get(&9).unwrap();
    assert_eq!(tile.block, 1);
    setblock_helper(world, 10, 2);
    let _ = tile;
}
