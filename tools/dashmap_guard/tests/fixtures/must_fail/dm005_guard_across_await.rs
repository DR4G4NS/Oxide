// DM005: a DashMap guard surviving across `.await` is forbidden.
async fn fut_do_nothing() {}
async fn dm005_guard_across_await(world: &MyWorld) {
    let tile = world.tiles.get(&1).unwrap();
    fut_do_nothing().await;
    let _ = tile;
    world.tiles.insert(2, DynamicTile { ..Default::default() });
}
