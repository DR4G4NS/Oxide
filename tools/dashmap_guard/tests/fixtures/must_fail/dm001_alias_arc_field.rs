use dashmap::DashMap;

type TileMap = DashMap<i32, i32>;
type SharedTiles = std::sync::Arc<TileMap>;

struct World {
    tiles: SharedTiles,
}

fn bug(world: &World) {
    let guard = world.tiles.get(&1).unwrap();
    world.tiles.insert(2, 3);
    let _ = guard;
}
