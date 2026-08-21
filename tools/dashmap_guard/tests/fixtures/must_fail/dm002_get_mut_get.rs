// DM002: mutable `get_mut` guard then ANY re-entrant map op (here `get`).
fn dm002_get_mut_get(world: &mut MyWorld) {
    let tile = world.tiles.get_mut(&1).unwrap();
    let other = world.tiles.get(&2);
    let _ = (tile, other);
}
