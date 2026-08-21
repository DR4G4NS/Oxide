// Scoping the guard to a nested block releases it before the mutation.
fn scope_end_insert(world: &mut MyWorld) {
    {
        let g = world.tiles.get(&1).unwrap();
        let _ = g;
    }
    world.tiles.insert(2, DynamicTile { ..Default::default() });
}
