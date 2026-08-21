// Guard bound by `if let` + mutation in the body (DM001).
fn iflet_guard_mutation(world: &mut MyWorld) {
    if let Some(tile) = world.tiles.get(&1) {
        assert_eq!(tile.block, 1);
        world.tiles.insert(2, DynamicTile { ..Default::default() });
    }
}
