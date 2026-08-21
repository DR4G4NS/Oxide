// Malformed suppressions must be surfaced (DM901) but must not block.
// dashmap-guard: allow DM900
// dashmap-guard: allow NOPE reason="bad code"
fn malformed_suppression(world: &MyWorld) {
    let tile = world.tiles.get(&1).unwrap();
    let _ = tile;
}
