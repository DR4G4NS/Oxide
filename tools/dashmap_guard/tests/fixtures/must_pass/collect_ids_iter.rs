// Collecting ids from an iterator releases it before mutating the map.
fn collect_ids_iter(world: &mut MyWorld) {
    let ids: Vec<i32> = world.enemies.iter().map(|e| *e.key()).collect();
    for id in ids {
        world.enemies.remove(&id);
    }
}
