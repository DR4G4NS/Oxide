use std::sync::Arc;

struct World {
    tiles: SharedTiles,
}

type SharedTiles = Arc<TileMap>;
type TileMap = dashmap::DashMap<i32, i32>;

fn bug(world: &World) {
    let guard = world.tiles.get(&1).unwrap();
    world.tiles.insert(2, 3);
    let _ = guard;
}
