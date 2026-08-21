// A narrow, justified DM900 suppression with a live guard on the same root.
fn suppression_dm900(world: &MyWorld) {
    let tile = world.tiles.get(&1).unwrap();
    // dashmap-guard: allow DM900 reason="dynamic callback proven not to access tiles"
    world.dynamic_callback();
    let _ = tile;
}
