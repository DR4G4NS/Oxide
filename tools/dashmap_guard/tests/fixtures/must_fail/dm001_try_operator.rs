// `?` must propagate a live guard (`let g = map.get(...)?`).
fn dm001_try_operator(world: &MyWorld) -> Option<()> {
    let tile = world.tiles.get(&1)?;
    world.tiles.insert(2, DynamicTile { ..Default::default() });
    let _ = tile;
    Some(())
}
