// Guard bound by a `match` arm + helper mutation in the same arm (DM004).
fn match_mutate(world: &mut MyWorld, pos: i32) {
    world.tiles.remove(&pos);
}
fn match_guard_helper(world: &mut MyWorld) {
    match world.tiles.get(&1) {
        Some(tile) => match_mutate(world, 2),
        None => {}
    }
    let _ = 0;
}
