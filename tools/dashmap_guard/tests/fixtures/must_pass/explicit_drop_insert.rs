// Explicit `drop(guard)` / `std::mem::drop` releases the shard lock before the mutation.
fn explicit_drop_insert(world: &mut MyWorld) {
    let g = world.tiles.get(&1).unwrap();
    drop(g);
    world.tiles.insert(2, DynamicTile { ..Default::default() });
}

fn explicit_std_mem_drop_insert(world: &mut MyWorld) {
    let g = world.tiles.get(&1).unwrap();
    std::mem::drop(g);
    world.tiles.insert(2, DynamicTile { ..Default::default() });
}
