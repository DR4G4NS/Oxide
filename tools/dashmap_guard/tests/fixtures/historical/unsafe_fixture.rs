// Historical shape: a live `world.tiles.get` guard + a call to `setblock`
// which transitively mutates `world.tiles`. MUST produce DM004.
fn setblock(world: &mut MyWorld, pos: i32, block: i16) {
    let existing = world.tiles.get(&pos).map(|t| t.block);
    if let Some(old) = existing {
        if old != block {
            world.tiles.insert(pos, DynamicTile { block, ..Default::default() });
        }
    }
}
fn getblock_and_setblock_unsafe(world: &mut MyWorld) {
    let a = 1;
    let tile = world.tiles.get(&a).unwrap();
    assert_eq!(tile.block, 1);
    setblock(world, 2, 2);
    let _ = tile;
}
