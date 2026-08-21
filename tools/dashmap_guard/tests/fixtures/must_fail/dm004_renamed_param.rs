// DM004: helper parameter is spelled `w`, caller passes `world`.
// Argument substitution must prove these are the same map.
fn setblock_renamed(w: &mut MyWorld, pos: i32, block: i16) {
    w.tiles.insert(pos, DynamicTile { block, ..Default::default() });
}
fn dm004_renamed_param(world: &mut MyWorld) {
    let tile = world.tiles.get(&1).unwrap();
    assert_eq!(tile.block, 1);
    setblock_renamed(world, 2, 2);
    let _ = tile;
}
