// DM001: immutable `get` guard then exclusive `get_mut` on the same map.
fn dm001_get_get_mut(world: &mut MyWorld) {
    let tile = world.tiles.get(&1).unwrap();
    if let Some(mut t) = world.tiles.get_mut(&2) {
        t.block = 4;
    }
    let _ = tile;
}
