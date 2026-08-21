mod model {
    use std::sync::Arc;

    pub struct World {
        pub tiles: SharedTiles,
    }

    pub type SharedTiles = Arc<TileMap>;
    pub type TileMap = dashmap::DashMap<i32, i32>;
}

fn bug(world: &model::World) {
    let guard = world.tiles.get(&1).unwrap();
    world.tiles.insert(2, 3);
    let _ = guard;
}
