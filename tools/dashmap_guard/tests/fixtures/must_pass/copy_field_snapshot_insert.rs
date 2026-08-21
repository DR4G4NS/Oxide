// Copy-field extraction via `.map(|v| v.id)` releases the guard.
fn copy_field_snapshot_insert(world: &mut MyWorld) {
    let id = world.tiles.get(&1).map(|v| v.id);
    world.tiles.insert(2, DynamicTile { id, ..Default::default() });
    let _ = id;
}
