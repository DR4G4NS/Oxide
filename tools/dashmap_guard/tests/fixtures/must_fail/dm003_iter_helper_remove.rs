// DM003: iterator guard held across a helper that removes from the same map.
fn detach_unit_control(world: &mut MyWorld, id: i32) {
    world.enemies.remove(&id);
}
fn dm003_iter_helper_remove(world: &mut MyWorld) {
    for e in world.enemies.iter() {
        detach_unit_control(world, *e.key());
    }
}
