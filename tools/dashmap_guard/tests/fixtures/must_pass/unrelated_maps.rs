// A guard on map A must not conflict with a mutation on unrelated map B.
fn unrelated_maps(world: &mut MyWorld) {
    let g = world.tiles.get(&1).unwrap();
    world.enemies.insert(5, EnemyUnit { id: 5 });
    let _ = g;
}
