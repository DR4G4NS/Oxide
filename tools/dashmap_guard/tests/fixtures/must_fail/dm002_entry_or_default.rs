// `entry().or_default()` returns a live RefMut; the exclusive shard lock stays held.
fn dm002_entry_or_default(world: &mut MyWorld) {
    let v = world.tiles.entry(1).or_default();
    world.tiles.insert(2, DynamicTile { ..Default::default() });
    let _ = v;
}
