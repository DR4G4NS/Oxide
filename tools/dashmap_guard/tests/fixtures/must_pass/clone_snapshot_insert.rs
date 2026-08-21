// Snapshot extraction via `.map(|v| v.value().clone())` releases the guard.
fn clone_snapshot_insert(world: &mut MyWorld) {
    let snapshot = world.tiles.get(&1).map(|v| v.value().clone());
    world.tiles.insert(2, DynamicTile { ..Default::default() });
    let _ = snapshot;
}
